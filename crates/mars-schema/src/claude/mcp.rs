//! Claude plugin MCP servers, translated into Mars [`McpServerDef`]s.
//!
//! Sources: the plugin root's `.mcp.json` (the `mcpServers` wrapper is optional) and
//! `plugin.json`'s `mcpServers` (an inline map, a path to a JSON file inside the plugin, or an
//! array of those); a manifest server replaces a `.mcp.json` server of the same name.
//! Formats: <https://code.claude.com/docs/en/plugins/components> and
//! <https://code.claude.com/docs/en/mcp>.
//!
//! A server is imported whole or not at all: one field Mars cannot represent leaves the server
//! out, with one diagnostic per such field, because importing it without that field would
//! connect differently from what the plugin declares. A field that changes nothing about how
//! the server connects (`timeout`, unknown keys) is dropped with a warning and the server stays.
//! Every server imports as `local`.

use std::collections::BTreeMap;

use indexmap::IndexMap;
use serde_json::{Map, Value};

use crate::claude::manifest::{MARKETPLACE_MANIFEST, PLUGIN_MANIFEST, PluginSpec, is_plugin_name};
use crate::diagnostic::{PathSegment, SchemaDiagnostic, Severity};
use crate::mcp_server::{EnvRef, HeaderValue, McpServerDef, McpTransport};
use crate::tree::TreeSource;

const MCP_JSON: &str = ".mcp.json";

/// One server Claude declares, translated.
pub(crate) struct ImportedServer {
    /// The server's name in the plugin: its directory under `mcp/`.
    pub name: String,
    /// The file that declared it.
    pub source_path: String,
    pub def: McpServerDef,
}

#[derive(Default)]
pub(crate) struct Imported {
    pub servers: Vec<ImportedServer>,
    /// By source path.
    pub diagnostics: Vec<(String, SchemaDiagnostic)>,
}

/// A server as declared, before translation.
struct Declared<'a> {
    file: String,
    /// Where the server's map sits in `file`, for diagnostics.
    prefix: Vec<PathSegment>,
    config: &'a Value,
}

pub(crate) fn import(source: &impl TreeSource, spec: &PluginSpec) -> Imported {
    let mut out = Imported::default();
    // Parsed documents outlive the declarations that borrow them.
    let mut documents: Vec<(String, Value)> = Vec::new();
    if let Some(value) = read_json(source, MCP_JSON, &mut out) {
        documents.push((MCP_JSON.to_owned(), value));
    }
    let manifest_value = source
        .read(PLUGIN_MANIFEST)
        .and_then(|bytes| serde_json::from_slice::<Value>(&bytes).ok())
        .and_then(|manifest| manifest.get("mcpServers").cloned());
    match manifest_value {
        Some(value) => {
            let entries = match value {
                Value::Array(entries) => entries,
                other => vec![other],
            };
            for (index, entry) in entries.into_iter().enumerate() {
                match entry {
                    Value::Object(_) => documents.push((PLUGIN_MANIFEST.to_owned(), entry)),
                    Value::String(reference) => {
                        documents.extend(manifest_file(source, &reference, index, &mut out));
                    }
                    other => out.warn(
                        PLUGIN_MANIFEST,
                        "mcp.manifest-entry-unsupported",
                        vec![key("mcpServers")],
                        format!("`mcpServers` entry {other} is not a map or a path to a JSON file, so it was left out"),
                    ),
                }
            }
        }
        // The marketplace entry declares servers the plugin's own files do not carry; only
        // `plugin.json` is read.
        None if spec.mcp => out.warn(
            MARKETPLACE_MANIFEST,
            "mcp.manifest-entry-unsupported",
            Vec::new(),
            "the marketplace entry declares `mcpServers`, which are not imported; declare them in the plugin's `.mcp.json`".into(),
        ),
        None => {}
    }

    // In order: `.mcp.json` first, then each manifest entry, so a later server of the same name
    // replaces an earlier one.
    let mut declared: BTreeMap<String, Declared<'_>> = BTreeMap::new();
    for (file, document) in &documents {
        let Some((prefix, servers)) = server_map(document) else {
            out.warn(
                file,
                "mcp.no-servers",
                Vec::new(),
                format!("`{file}` holds no map of MCP servers, so nothing was imported from it"),
            );
            continue;
        };
        for (name, config) in servers {
            let mut prefix = prefix.clone();
            prefix.push(key(name));
            declared.insert(
                name.clone(),
                Declared {
                    file: file.clone(),
                    prefix,
                    config,
                },
            );
        }
    }

    for (name, declared) in declared {
        if !is_plugin_name(&name) {
            out.warn(
                &declared.file,
                "mcp.name-invalid",
                declared.prefix,
                format!(
                    "MCP server `{name}` was not imported: its name becomes a directory, so it may use only letters, digits, `-`, `_` and `.`, starting with a letter or digit"
                ),
            );
            continue;
        }
        let shadow = format!("mcp/{name}");
        if source.is_dir(&shadow) {
            out.warn(
                &declared.file,
                "mcp.shadowed-by-authored",
                declared.prefix,
                format!("MCP server `{name}` was not imported: the plugin also ships `{shadow}/`, which is used instead"),
            );
            continue;
        }
        if let Some(def) = translate(&name, &declared, &mut out) {
            out.servers.push(ImportedServer {
                name,
                source_path: declared.file,
                def,
            });
        }
    }
    out
}

impl Imported {
    fn warn(&mut self, file: &str, code: &str, path: Vec<PathSegment>, message: String) {
        self.diagnostics.push((
            file.to_owned(),
            SchemaDiagnostic::new(code, Severity::Warning, path, message),
        ));
    }
}

fn key(name: &str) -> PathSegment {
    PathSegment::Key(name.to_owned())
}

fn read_json(source: &impl TreeSource, file: &str, out: &mut Imported) -> Option<Value> {
    let bytes = source.read(file)?;
    match serde_json::from_slice(&bytes) {
        Ok(value) => Some(value),
        Err(error) => {
            out.warn(
                file,
                "mcp.invalid-json",
                Vec::new(),
                format!("`{file}` is not valid JSON ({error}), so nothing was imported from it"),
            );
            None
        }
    }
}

/// A `plugin.json` `mcpServers` string: a JSON file inside the plugin. A `.mcpb`/`.dxt` bundle or
/// a URL needs fetching or unpacking, which a pure reader of the tree cannot do.
fn manifest_file(
    source: &impl TreeSource,
    reference: &str,
    index: usize,
    out: &mut Imported,
) -> Option<(String, Value)> {
    let path = reference
        .strip_prefix("./")
        .unwrap_or(reference)
        .trim_end_matches('/');
    let local = !reference.contains("://")
        && !path.is_empty()
        && !path.starts_with('/')
        && !path.contains('\\')
        && path.split('/').all(|part| !matches!(part, "" | "." | ".."));
    if !local || !path.ends_with(".json") {
        out.warn(
            PLUGIN_MANIFEST,
            "mcp.manifest-entry-unsupported",
            vec![key("mcpServers"), PathSegment::Index(index)],
            format!("`mcpServers` entry `{reference}` is not a JSON file inside the plugin (a `.mcpb` bundle or a URL cannot be imported), so it was left out"),
        );
        return None;
    }
    if !source.is_file(path) {
        out.warn(
            PLUGIN_MANIFEST,
            "mcp.manifest-entry-unsupported",
            vec![key("mcpServers"), PathSegment::Index(index)],
            format!("`mcpServers` entry `{reference}` names a file the plugin does not contain, so it was left out"),
        );
        return None;
    }
    read_json(source, path, out).map(|value| (path.to_owned(), value))
}

/// The map of servers in a document, and where it sits: `{"mcpServers": {...}}` or the bare map.
fn server_map(document: &Value) -> Option<(Vec<PathSegment>, &Map<String, Value>)> {
    let object = document.as_object()?;
    match object.get("mcpServers") {
        Some(Value::Object(servers)) => Some((vec![key("mcpServers")], servers)),
        _ => Some((Vec::new(), object)),
    }
}

/// Keys that change nothing about how a server connects, or that Claude reads for its own UI.
const IGNORED_KEYS: [&str; 3] = ["timeout", "alwaysLoad", "description"];
/// Keys whose absence would make the imported server connect differently.
const UNSUPPORTED_KEYS: [(&str, &str); 2] = [
    (
        "headersHelper",
        "runs a command to produce request headers; Mars passes headers by value or environment variable",
    ),
    (
        "oauth",
        "pre-registers an OAuth client; each harness runs its own OAuth flow",
    ),
];

/// Translate one server, or say why not. Every unconvertible field is reported before giving up.
fn translate(name: &str, declared: &Declared<'_>, out: &mut Imported) -> Option<McpServerDef> {
    let mut problems = Problems {
        name,
        declared,
        out,
        failed: false,
    };
    let Some(config) = declared.config.as_object() else {
        problems.report(None, "mcp.invalid-server", "is not a map".into());
        return None;
    };
    for (field, why) in UNSUPPORTED_KEYS {
        if config.contains_key(field) {
            problems.report(
                Some(field),
                "mcp.field-unsupported",
                format!("sets `{field}`, which {why}"),
            );
        }
    }
    for field in config.keys() {
        let known = matches!(
            field.as_str(),
            "type" | "command" | "args" | "env" | "url" | "headers"
        ) || UNSUPPORTED_KEYS
            .iter()
            .any(|(unsupported, _)| unsupported == field);
        if !known {
            let note = if IGNORED_KEYS.contains(&field.as_str()) {
                "has no equivalent"
            } else {
                "is not known"
            };
            problems.out.warn(
                &declared.file,
                "mcp.field-dropped",
                path_to(declared, field),
                format!("MCP server `{name}`: `{field}` {note} and was dropped"),
            );
        }
    }

    let declared_type = match config.get("type") {
        None => None,
        Some(Value::String(text)) => Some(text.as_str()),
        Some(_) => {
            problems.report(
                Some("type"),
                "mcp.invalid-server",
                "has a `type` that is not text".into(),
            );
            return None;
        }
    };
    let transport = match declared_type {
        None | Some("stdio") if config.contains_key("command") => problems.stdio(config),
        None if config.contains_key("url") => {
            problems.report(
                Some("url"),
                "mcp.url-without-type",
                "has a `url` and no `type`; Claude reads that as invalid. Set `\"type\": \"http\"`"
                    .into(),
            );
            None
        }
        Some("stdio") | None => {
            problems.report(None, "mcp.invalid-server", "sets no `command`".into());
            None
        }
        Some("http" | "streamable-http") => problems.http(config),
        Some(other) => {
            problems.report(
                Some("type"),
                "mcp.transport-unsupported",
                format!("uses transport `{other}`; Mars imports `stdio` and `http`"),
            );
            None
        }
    };
    if problems.failed {
        return None;
    }
    Some(McpServerDef {
        name: None,
        transport: transport?,
        visibility: "local".into(),
        targets: Vec::new(),
    })
}

struct Problems<'a, 'b> {
    name: &'a str,
    declared: &'a Declared<'b>,
    out: &'a mut Imported,
    failed: bool,
}

fn path_to(declared: &Declared<'_>, field: &str) -> Vec<PathSegment> {
    let mut path = declared.prefix.clone();
    path.push(key(field));
    path
}

impl Problems<'_, '_> {
    /// One reason the server cannot be imported.
    fn report(&mut self, field: Option<&str>, code: &str, what: String) {
        self.failed = true;
        let path = field.map_or_else(
            || self.declared.prefix.clone(),
            |f| path_to(self.declared, f),
        );
        self.out.warn(
            &self.declared.file,
            code,
            path,
            format!("MCP server `{}` was not imported: it {what}", self.name),
        );
    }

    fn text(&mut self, field: &str, value: &Value) -> Option<String> {
        match value.as_str() {
            Some(text) => Some(text.to_owned()),
            None => {
                self.report(
                    Some(field),
                    "mcp.invalid-server",
                    format!("has a `{field}` that is not text"),
                );
                None
            }
        }
    }

    /// A string with no interpolation, or a report.
    fn plain(&mut self, field: &str, value: &Value) -> Option<String> {
        let text = self.text(field, value)?;
        match interpolation(&text) {
            Interpolation::Literal => Some(text),
            other => {
                self.interpolated(field, &other);
                None
            }
        }
    }

    fn interpolated(&mut self, field: &str, found: &Interpolation<'_>) {
        let (code, what) = match found {
            Interpolation::PluginVariable(variable) => (
                "mcp.plugin-variable",
                format!(
                    "uses `${{{variable}}}` in `{field}`, which names the plugin's install location or settings and has no equivalent here"
                ),
            ),
            Interpolation::Default => (
                "mcp.default-value",
                format!(
                    "uses a `${{VAR:-default}}` in `{field}`; Mars passes an environment variable by name, with no default"
                ),
            ),
            _ => (
                "mcp.interpolation-unsupported",
                match field {
                    "command" | "args" | "url" => format!(
                        "uses `${{...}}` in `{field}`; only `env` values and `headers` can read an environment variable"
                    ),
                    _ => format!(
                        "uses `${{...}}` in `{field}` other than exactly `${{VAR}}`{}",
                        if field.starts_with("headers") {
                            " or `<text>${VAR}`"
                        } else {
                            ""
                        }
                    ),
                },
            ),
        };
        self.report(Some(field), code, what);
    }

    fn stdio(&mut self, config: &Map<String, Value>) -> Option<McpTransport> {
        if config.contains_key("url") || config.contains_key("headers") {
            self.report(
                None,
                "mcp.invalid-server",
                "sets both `command` and `url`/`headers`".into(),
            );
            return None;
        }
        let command = self.plain("command", &config["command"]);
        let mut args = Vec::new();
        match config.get("args") {
            None => {}
            Some(Value::Array(values)) => {
                for value in values {
                    args.extend(self.plain("args", value));
                }
            }
            Some(_) => self.report(
                Some("args"),
                "mcp.invalid-server",
                "has `args` that is not a list".into(),
            ),
        }
        let mut env = IndexMap::new();
        match config.get("env") {
            None => {}
            Some(Value::Object(values)) => {
                for (var, value) in values {
                    let field = format!("env.{var}");
                    let Some(text) = self.text(&field, value) else {
                        continue;
                    };
                    match interpolation(&text) {
                        Interpolation::Variable { prefix: "", var: from } => {
                            env.insert(var.clone(), EnvRef::Env { var: from.to_owned() });
                        }
                        Interpolation::Literal => self.report(
                            Some(&field),
                            "mcp.literal-env",
                            format!("sets `{field}` to a literal value; Mars passes only a reference to an environment variable, `${{VAR}}`"),
                        ),
                        other => self.interpolated(&field, &other),
                    }
                }
            }
            Some(_) => self.report(
                Some("env"),
                "mcp.invalid-server",
                "has an `env` that is not a map".into(),
            ),
        }
        Some(McpTransport::Stdio {
            command: command?,
            args,
            env,
        })
        .filter(|_| !self.failed)
    }

    fn http(&mut self, config: &Map<String, Value>) -> Option<McpTransport> {
        if config.contains_key("command")
            || config.contains_key("args")
            || config.contains_key("env")
        {
            self.report(
                None,
                "mcp.invalid-server",
                "sets both `url` and `command`/`args`/`env`".into(),
            );
            return None;
        }
        let url = match config.get("url") {
            Some(url) => self.plain("url", url),
            None => {
                self.report(None, "mcp.invalid-server", "sets no `url`".into());
                None
            }
        };
        let mut headers = IndexMap::new();
        match config.get("headers") {
            None => {}
            Some(Value::Object(values)) => {
                for (header, value) in values {
                    let field = format!("headers.{header}");
                    let Some(text) = self.text(&field, value) else {
                        continue;
                    };
                    match interpolation(&text) {
                        Interpolation::Literal => {
                            headers.insert(header.clone(), HeaderValue::Literal(text));
                        }
                        Interpolation::Variable { var, .. } if is_credential_variable(var) => {
                            self.report(
                                Some(&field),
                                "mcp.credential-blocked",
                                format!("sends `${{{var}}}` in `{field}`; Claude does not send that credential to a remote server, so the imported server would connect differently"),
                            );
                        }
                        Interpolation::Variable { prefix, var } => {
                            headers.insert(
                                header.clone(),
                                HeaderValue::Env {
                                    env: EnvRef::Env {
                                        var: var.to_owned(),
                                    },
                                    prefix: prefix.to_owned(),
                                },
                            );
                        }
                        other => self.interpolated(&field, &other),
                    }
                }
            }
            Some(_) => self.report(
                Some("headers"),
                "mcp.invalid-server",
                "has `headers` that is not a map".into(),
            ),
        }
        Some(McpTransport::Http { url: url?, headers }).filter(|_| !self.failed)
    }
}

/// The variables Claude substitutes in a plugin's servers, besides `${user_config.*}`.
const PLUGIN_VARIABLES: [&str; 3] = [
    "CLAUDE_PLUGIN_ROOT",
    "CLAUDE_PLUGIN_DATA",
    "CLAUDE_PROJECT_DIR",
];

/// Environment variables Claude Code 2.1.312 reads as empty in a remote server's `url` and
/// `headers`, extracted from its binary: Claude does not publish the full list (its docs name
/// a few "such as" examples). Mars's renderers would send the real value, so these are refused.
/// The check is a deliberate superset of Claude's: see [`is_credential_variable`].
const CREDENTIAL_VARIABLES: [&str; 109] = [
    "ANTHROPIC_API_KEY",
    "CLAUDE_CODE_OAUTH_TOKEN",
    "CLAUDE_CODE_SESSIONS_OAUTH_TOKEN",
    "CLAUDE_CODE_ARTIFACTS_API_TOKEN",
    "CLAUDE_CODE_MEMORY_API_TOKEN",
    "CLAUDE_CODE_SLACK_TAG_TOKEN",
    "ANTHROPIC_AUTH_TOKEN",
    "ANTHROPIC_FOUNDRY_API_KEY",
    "ANTHROPIC_FOUNDRY_AUTH_TOKEN",
    "ANTHROPIC_AWS_API_KEY",
    "ANTHROPIC_CUSTOM_HEADERS",
    "AWS_SECRET_ACCESS_KEY",
    "AWS_SESSION_TOKEN",
    "AWS_BEARER_TOKEN_BEDROCK",
    "GOOGLE_APPLICATION_CREDENTIALS",
    "GOOGLE_GHA_CREDS_PATH",
    "AZURE_CLIENT_SECRET",
    "IDENTITY_HEADER",
    "MSI_SECRET",
    "AZURE_CLIENT_CERTIFICATE_PATH",
    "AZURE_CLIENT_CERTIFICATE_PASSWORD",
    "AZURE_PASSWORD",
    "AZURE_FEDERATED_TOKEN_FILE",
    "AWS_WEB_IDENTITY_TOKEN_FILE",
    "AWS_CONTAINER_CREDENTIALS_RELATIVE_URI",
    "AWS_CONTAINER_CREDENTIALS_FULL_URI",
    "AWS_CONTAINER_AUTHORIZATION_TOKEN",
    "AWS_CONTAINER_AUTHORIZATION_TOKEN_FILE",
    "CLOUDSDK_AUTH_ACCESS_TOKEN",
    "GOOGLE_OAUTH_ACCESS_TOKEN",
    "CLAUDE_CODE_OAUTH_REFRESH_TOKEN",
    "HF_TOKEN",
    "HUGGING_FACE_HUB_TOKEN",
    "HUGGINGFACEHUB_API_TOKEN",
    "NODE_AUTH_TOKEN",
    "NUGET_AUTH_TOKEN",
    "CARGO_REGISTRY_TOKEN",
    "TWINE_PASSWORD",
    "TWINE_USERNAME",
    "PYPI_TOKEN",
    "PYPI_API_TOKEN",
    "UV_PUBLISH_TOKEN",
    "UV_PUBLISH_PASSWORD",
    "UV_PUBLISH_USERNAME",
    "FLIT_PASSWORD",
    "FLIT_USERNAME",
    "HATCH_INDEX_AUTH",
    "HATCH_INDEX_USER",
    "GEM_HOST_API_KEY",
    "MATURIN_PYPI_TOKEN",
    "MATURIN_PASSWORD",
    "MATURIN_USERNAME",
    "CONAN_LOGIN_USERNAME",
    "CONAN_PASSWORD",
    "ANACONDA_API_TOKEN",
    "BINSTAR_API_TOKEN",
    "VAULT_TOKEN",
    "VAULT_AUTH_TOKEN",
    "VAULT_ROLE_ID",
    "VAULT_SECRET_ID",
    "CONSUL_HTTP_TOKEN",
    "CONSUL_HTTP_AUTH",
    "NOMAD_TOKEN",
    "NOMAD_HTTP_AUTH",
    "CI_REGISTRY_USER",
    "CI_DEPLOY_USER",
    "JF_USER",
    "FASTLANE_SESSION",
    "MATCH_GIT_BASIC_AUTHORIZATION",
    "SONAR_TOKEN",
    "SONARQUBE_SCANNER_PARAMS",
    "SONAR_SCANNER_JSON_PARAMS",
    "SLACK_WEBHOOK_URL",
    "SLACK_WEBHOOK",
    "DISCORD_WEBHOOK",
    "DISCORD_WEBHOOK_URL",
    "TEAMS_WEBHOOK_URL",
    "MS_TEAMS_WEBHOOK_URI",
    "ANTHROPIC_IDENTITY_TOKEN",
    "ANTHROPIC_IDENTITY_TOKEN_FILE",
    "CLOUDSDK_AUTH_ACCESS_TOKEN_FILE",
    "CLOUDSDK_AUTH_AUTHORIZATION_TOKEN_FILE",
    "AZURE_AUTH_LOCATION",
    "ACTIONS_ID_TOKEN_REQUEST_TOKEN",
    "ACTIONS_ID_TOKEN_REQUEST_URL",
    "ACTIONS_RUNTIME_TOKEN",
    "ACTIONS_RUNTIME_URL",
    "ALL_INPUTS",
    "VSS_NUGET_EXTERNAL_FEED_ENDPOINTS",
    "ARTIFACTS_CREDENTIALPROVIDER_EXTERNAL_FEED_ENDPOINTS",
    "VSS_NUGET_ACCESSTOKEN",
    "ARTIFACTS_CREDENTIALPROVIDER_ACCESSTOKEN",
    "COMPOSER_AUTH",
    "OVERRIDE_GITHUB_TOKEN",
    "DEFAULT_WORKFLOW_TOKEN",
    "SSH_SIGNING_KEY",
    "AWS_SHARED_CREDENTIALS_FILE",
    "AWS_CONFIG_FILE",
    "CLOUDSDK_CONFIG",
    "AZURE_CONFIG_DIR",
    "KUBECONFIG",
    "NETRC",
    "PGPASSFILE",
    "PGSERVICEFILE",
    "DOCKER_CONFIG",
    "GH_CONFIG_DIR",
    "GNUPGHOME",
    "NPM_CONFIG_USERCONFIG",
    "NPM_CONFIG_GLOBALCONFIG",
];

/// Prefix families refused whole, covering names newer Claude versions may add. Suffix families
/// (`_TOKEN`, `_SECRET`) are not used: they would refuse ordinary plugin auth such as
/// `Bearer ${GITHUB_TOKEN}`, which Claude sends.
const CREDENTIAL_PREFIXES: [&str; 9] = [
    "ANTHROPIC_",
    "CLAUDE_CODE_",
    "AWS_",
    "AZURE_",
    "GOOGLE_",
    "CLOUDSDK_",
    "VAULT_",
    "OTEL_",
    "GIT_CONFIG_",
];

/// Whether a header may not send `${var}`. Wrongly refusing one server costs less than sending
/// a credential to a plugin's server, so this refuses more than Claude does: the name is
/// uppercased as Claude does, and `INPUT_` is stripped first.
fn is_credential_variable(var: &str) -> bool {
    let upper = var.to_ascii_uppercase();
    let name = upper.strip_prefix("INPUT_").unwrap_or(&upper);
    CREDENTIAL_VARIABLES.contains(&name)
        || CREDENTIAL_PREFIXES
            .iter()
            .any(|prefix| name.starts_with(prefix))
}

/// What `${...}` a string holds.
enum Interpolation<'a> {
    Literal,
    /// Exactly one `${VAR}`, at the end, after literal `prefix` text.
    Variable {
        prefix: &'a str,
        var: &'a str,
    },
    /// `${CLAUDE_PLUGIN_ROOT}`, `${CLAUDE_PLUGIN_DATA}`, `${CLAUDE_PROJECT_DIR}`, `${user_config.X}`.
    PluginVariable(&'a str),
    /// `${VAR:-default}`.
    Default,
    /// Anything else: several, mid-string, not a variable name, unterminated.
    Other,
}

fn interpolation(text: &str) -> Interpolation<'_> {
    let mut names = Vec::new();
    let mut rest = text;
    while let Some(start) = rest.find("${") {
        let Some(end) = rest[start..].find('}') else {
            return Interpolation::Other;
        };
        names.push(&rest[start + 2..start + end]);
        rest = &rest[start + end + 1..];
    }
    if let Some(variable) = names
        .iter()
        .find(|name| PLUGIN_VARIABLES.contains(name) || name.starts_with("user_config."))
    {
        return Interpolation::PluginVariable(variable);
    }
    if names.iter().any(|name| name.contains(":-")) {
        return Interpolation::Default;
    }
    match names.as_slice() {
        [] => Interpolation::Literal,
        [var] if rest.is_empty() && is_variable_name(var) => Interpolation::Variable {
            prefix: &text[..text.len() - var.len() - 3],
            var,
        },
        _ => Interpolation::Other,
    }
}

fn is_variable_name(name: &str) -> bool {
    name.bytes()
        .next()
        .is_some_and(|byte| byte.is_ascii_alphabetic() || byte == b'_')
        && name
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'_')
}

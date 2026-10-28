//! The `mcpServers` entry: the one codec for the shape Claude's `.mcp.json` declares and
//! Cursor, Pi and a generated Claude/Codex plugin tree copy.
//!
//! [`parse`] reads one entry into an [`McpServerDef`] transport, or says why it cannot (the
//! Claude plugin import); [`render`] writes one back in each [`Flavor`]. `parse(render(..))`
//! returns the transport for the flavors Claude reads ([`Flavor::Claude`], [`Flavor::Plugin`]).
//! Opencode and Codex write different formats and stay in the binary; they share only the
//! helpers here ([`env_map`], [`header_map`], [`partition_env`]).
//!
//! A stdio `env` value is exactly `${VAR}`; `env_vars` is a list of names forwarded unchanged
//! (the same as `NAME: "${NAME}"`). A remote header is a literal, or `<prefix>${VAR}`.

use indexmap::IndexMap;
use serde_json::{Map, Value};

use crate::mcp_server::{EnvRef, HeaderValue, McpTransport};

/// `mcp.name-invalid`: a server name that cannot be a directory under `mcp/`. The one name rule,
/// for the plugin import and for sync alike.
pub const NAME_INVALID: &str = "mcp.name-invalid";

/// Whether `name` may name a server that is installed as a directory.
pub fn is_server_name(name: &str) -> bool {
    crate::claude::manifest::is_plugin_name(name)
}

/// What [`NAME_INVALID`] says a name may be.
pub const NAME_RULE: &str = crate::claude::manifest::PLUGIN_NAME_RULE;

/// How an entry is spelled for the file that holds it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Flavor {
    /// Claude's project `.mcp.json` and Pi: `${VAR}` references, a remote entry typed `http`.
    Claude,
    /// Cursor: `${env:VAR}` references, and no `type` (not known to accept one).
    Cursor,
    /// A generated plugin tree, shared by Claude and Codex: a same-name stdio reference is a
    /// forwarded name in `env_vars`, a renamed one an `env` `${VAR}` that Codex would pass
    /// through literally (reported in [`McpRender::lossy`]), a remote entry typed `http`.
    Plugin,
}

/// One server rendered into a harness's native shape.
#[derive(Debug, Clone)]
pub struct McpRender {
    pub value: Value,
    /// What the native shape could not carry, one full sentence each; each becomes a warning.
    pub lossy: Vec<String>,
}

impl McpRender {
    pub fn exact(value: Value) -> Self {
        Self {
            value,
            lossy: Vec::new(),
        }
    }
}

/// Split a stdio server's env by whether a harness can forward it by name:
/// `key == variable` is `forwarded`, anything else is `renamed` as `(key, variable)`.
pub fn partition_env(env: &IndexMap<String, EnvRef>) -> (Vec<&str>, Vec<(&str, &str)>) {
    let mut forwarded = Vec::new();
    let mut renamed = Vec::new();
    for (key, env_ref) in env {
        let variable = env_ref.var_name();
        if key == variable {
            forwarded.push(variable);
        } else {
            renamed.push((key.as_str(), variable));
        }
    }
    (forwarded, renamed)
}

/// A stdio server's env as a JSON object of `KEY: <reference to the variable>`.
pub fn env_map<'a>(
    env: impl IntoIterator<Item = (&'a String, &'a EnvRef)>,
    interpolate: impl Fn(&str) -> String,
) -> Value {
    env.into_iter()
        .map(|(key, env_ref)| (key.clone(), Value::String(interpolate(env_ref.var_name()))))
        .collect::<Map<_, _>>()
        .into()
}

/// A remote server's headers as a JSON object: a literal as written, an env header as its
/// prefix followed by the harness's reference to the variable.
pub fn header_map(
    headers: &IndexMap<String, HeaderValue>,
    interpolate: impl Fn(&str) -> String,
) -> Value {
    headers
        .iter()
        .map(|(name, value)| {
            let text = match value {
                HeaderValue::Literal(text) => text.clone(),
                HeaderValue::Env { env, prefix } => {
                    format!("{prefix}{}", interpolate(env.var_name()))
                }
            };
            (name.clone(), Value::String(text))
        })
        .collect::<Map<_, _>>()
        .into()
}

fn braced(var: &str) -> String {
    format!("${{{var}}}")
}

/// Render one server's transport as an `mcpServers` entry.
pub fn render(transport: &McpTransport, flavor: Flavor) -> McpRender {
    let interpolate = |var: &str| match flavor {
        Flavor::Cursor => format!("${{env:{var}}}"),
        Flavor::Claude | Flavor::Plugin => braced(var),
    };
    let mut lossy = Vec::new();
    let value = match transport {
        McpTransport::Stdio { command, args, env } => {
            let mut entry = serde_json::json!({ "command": command, "args": args });
            if flavor == Flavor::Plugin {
                let (forwarded, renamed) = partition_env(env);
                if !renamed.is_empty() {
                    entry["env"] = env_map(
                        env.iter()
                            .filter(|(key, env_ref)| key.as_str() != env_ref.var_name()),
                        interpolate,
                    );
                }
                if !forwarded.is_empty() {
                    entry["env_vars"] = serde_json::json!(forwarded);
                }
                lossy.extend(renamed.into_iter().map(|(key, variable)| {
                    format!(
                        "Codex 0.164.0 passes `${{{variable}}}` literally instead of reading `{variable}`, so `{key}` is not set from it."
                    )
                }));
            } else if !env.is_empty() {
                entry["env"] = env_map(env, interpolate);
            }
            entry
        }
        McpTransport::Http { url, headers } => {
            let mut entry = serde_json::json!({ "url": url });
            if flavor != Flavor::Cursor {
                entry["type"] = "http".into();
            }
            if !headers.is_empty() {
                entry["headers"] = header_map(headers, interpolate);
            }
            if flavor == Flavor::Plugin {
                lossy.extend(headers.iter().filter_map(|(name, value)| match value {
                    HeaderValue::Env { env, .. } => Some(format!(
                        "Codex 0.164.0 passes `${{{0}}}` literally instead of reading `{0}`, so header `{name}` is not set from it.",
                        env.var_name()
                    )),
                    HeaderValue::Literal(_) => None,
                }));
            }
            entry
        }
    };
    McpRender { value, lossy }
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

/// A diagnostic about one field of a server, for the caller to place in its file.
pub(crate) struct Note {
    /// The field it is about; `None` is the server as a whole.
    pub field: Option<String>,
    pub code: &'static str,
    pub message: String,
}

/// A server translated, or why not. A server is left out (`transport` is `None`) with one note
/// per field it cannot represent; a note beside a transport is a field dropped.
pub(crate) struct Parsed {
    pub transport: Option<McpTransport>,
    pub notes: Vec<Note>,
}

/// Translate one server, or say why not. Every unconvertible field is reported before giving up.
pub(crate) fn parse(name: &str, config: &Value) -> Parsed {
    let mut problems = Problems {
        name,
        notes: Vec::new(),
        failed: false,
    };
    let transport = transport(&mut problems, config);
    Parsed {
        transport: if problems.failed { None } else { transport },
        notes: problems.notes,
    }
}

fn transport(problems: &mut Problems<'_>, config: &Value) -> Option<McpTransport> {
    let name = problems.name;
    let Some(config) = config.as_object() else {
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
            "type" | "command" | "args" | "env" | "env_vars" | "url" | "headers"
        ) || UNSUPPORTED_KEYS
            .iter()
            .any(|(unsupported, _)| unsupported == field);
        if !known {
            let note = if IGNORED_KEYS.contains(&field.as_str()) {
                "has no equivalent"
            } else {
                "is not known"
            };
            problems.notes.push(Note {
                field: Some(field.clone()),
                code: "mcp.field-dropped",
                message: format!("MCP server `{name}`: `{field}` {note} and was dropped"),
            });
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
    match declared_type {
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
    }
}

struct Problems<'a> {
    name: &'a str,
    notes: Vec<Note>,
    failed: bool,
}

impl Problems<'_> {
    /// One reason the server cannot be imported.
    fn report(&mut self, field: Option<&str>, code: &'static str, what: String) {
        self.failed = true;
        self.notes.push(Note {
            field: field.map(str::to_owned),
            code,
            message: format!("MCP server `{}` was not imported: it {what}", self.name),
        });
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
        // `env_vars` is the same-name form of `env`: `NAME` is `NAME: "${NAME}"`.
        match config.get("env_vars") {
            None => {}
            Some(Value::Array(names)) => {
                for value in names {
                    let Some(var) = self.text("env_vars", value) else {
                        continue;
                    };
                    if is_variable_name(&var) {
                        env.insert(var.clone(), EnvRef::Env { var });
                    } else {
                        self.report(
                            Some("env_vars"),
                            "mcp.invalid-server",
                            format!("lists `{var}` in `env_vars`, which is not a variable name"),
                        );
                    }
                }
            }
            Some(_) => self.report(
                Some("env_vars"),
                "mcp.invalid-server",
                "has `env_vars` that is not a list".into(),
            ),
        }
        let command = command?;
        (!self.failed).then_some(McpTransport::Stdio { command, args, env })
    }

    fn http(&mut self, config: &Map<String, Value>) -> Option<McpTransport> {
        if config.contains_key("command")
            || config.contains_key("args")
            || config.contains_key("env")
            || config.contains_key("env_vars")
        {
            self.report(
                None,
                "mcp.invalid-server",
                "sets both `url` and `command`/`args`/`env`/`env_vars`".into(),
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
        let url = url?;
        (!self.failed).then_some(McpTransport::Http { url, headers })
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
/// `NPM_TOKEN` and the proxy variables are named by the docs (<https://code.claude.com/docs/en/mcp>).
/// A name a [`CREDENTIAL_PREFIXES`] family already covers is not listed. The check is a
/// deliberate superset of Claude's: see [`is_credential_variable`].
const CREDENTIAL_VARIABLES: [&str; 71] = [
    "IDENTITY_HEADER",
    "MSI_SECRET",
    "HF_TOKEN",
    "HUGGING_FACE_HUB_TOKEN",
    "HUGGINGFACEHUB_API_TOKEN",
    "NODE_AUTH_TOKEN",
    "NPM_TOKEN",
    "HTTPS_PROXY",
    "HTTP_PROXY",
    "ALL_PROXY",
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

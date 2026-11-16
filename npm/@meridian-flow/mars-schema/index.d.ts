// Types for @meridian-flow/mars-schema. The wasm-bindgen declarations under node/ and web/
// are untyped (`any`); this file is the contract. Every result is a plain object or array
// (`null` for an absent value, `Uint8Array` for bytes), pinned by the shipped `fixtures/`.

export type JsonValue =
  | null
  | boolean
  | number
  | string
  | JsonValue[]
  | { [key: string]: JsonValue };

/**
 * What every export throws: an `Error` whose `code` says which failure, so a caller maps codes
 * and never reads messages. `input.invalid` is an argument that is not the shape the export
 * documents (a missing, misspelled or unknown key included). The rest come from the crate's
 * error types: `read.unrecognized-path`, `tree.invalid-path`, `edit.*`
 * (`integer-too-large`, `duplicate-key`, `key-too-long`), `input.lone-surrogate`,
 * `filter.inconsistent`, and `VersionErrorCode`.
 */
export interface MarsSchemaError extends Error {
  code: string;
}

/** Codes of the version exports: `selectVersion`, `satisfies` and `satisfiesRequirement`. */
export type VersionErrorCode =
  /** No tag meets every constraint; the message names each requirement and the versions there are. */
  | "version.no-satisfying"
  /** Two different branch or commit pins. */
  | "version.pins-differ"
  /** A branch or commit pin beside a semver range. */
  | "version.pin-beside-range"
  /** A `version` argument that is not semver. */
  | "version.invalid-version"
  /** A `requires-mars` that is not a requirement. */
  | "version.invalid-requirement"
  /** A `locked` entry that is not a version tag. */
  | "version.invalid-tag";

/** Build-time version of the schema crate. */
export function version(): string;

// ---------------------------------------------------------------------------------------
// read
// ---------------------------------------------------------------------------------------

export interface Diagnostic {
  /** Stable code, e.g. `field.unknown`, `yaml.invalid`, `field.retired`. */
  code: string;
  severity: "error" | "warning" | "info";
  /** Key path into the authored file, e.g. `["model-policies", 0, "when"]`. */
  path: (string | number)[];
  message: string;
  /**
   * Byte offsets into the text. Present only for fence and YAML-syntax errors (and not yet
   * set by any reader): locate every other diagnostic by `path`, with your own parser.
   */
  range?: { start: number; end: number };
}

/** The v1 reading of one file. */
export interface DefinitionRead<Fields> {
  /**
   * Every top-level key with its value in file order, unknown keys included. A key whose
   * value has no JSON form is absent and has a `yaml.non-json-value` diagnostic
   * (`toml.non-json-value` for `mars.toml`). That covers a YAML tag, a non-string map key,
   * an integer beyond ±(2^53 − 1), NaN or infinity (TOML), and a `__proto__` key at any depth.
   * Mapping keys, nested ones included, are in file order (JavaScript lists integer-like keys
   * such as `"1"` first). For `mars.toml`, the pairs of the `[package]` table in file order;
   * a TOML datetime is its TOML text, a string (`"1979-05-27"`).
   */
  frontmatter: [string, JsonValue][];
  /** Typed fields in file spellings; `null` when the file cannot be read at all. */
  fields: Fields | null;
  /** Text after the frontmatter; `null` for `mars.toml` or an unreadable file. */
  body: string | null;
  diagnostics: Diagnostic[];
}

export type Effort = "none" | "low" | "medium" | "high" | "xhigh" | "max";
export type Sandbox = "default" | "read-only" | "workspace-write" | "danger-full-access";
export type Approval = "default" | "auto" | "confirm" | "never";
export type Network = "allow" | "deny" | "allowlist";
export type Harness = "claude" | "codex" | "pi" | "opencode" | "cursor";

export interface ToolRule {
  /** A canonical capability, `*`, `mcp`, or the author's spelling of an unknown name. */
  capability: string;
  pattern: string | null;
  action: "allow" | "deny" | "ask";
  /**
   * Where the rule came from: `"profile"` (the Agent's own `tools`) or `"package"` (the package's
   * `[package.tools]`, added by `prepare` to an Agent that does not name the key). `read` of one
   * file reports only `"profile"`; the launch bundle also has `"overlay"` and `"project"`.
   */
  origin: "profile" | "package" | "overlay" | "project";
}

export interface ModelPolicyRule {
  match: { model: string } | { alias: string } | { "model-glob": string };
  override?: { effort?: Effort; sandbox?: Sandbox; approval?: Approval; network?: Network };
  "no-fallback"?: boolean;
  /** One non-empty line. */
  when?: string;
}

export interface AgentFields {
  name: string | null;
  description: string | null;
  harness: Harness | null;
  model: string | null;
  mode: "primary" | "subagent" | null;
  "model-invocable": boolean;
  "user-invocable": boolean;
  approval: Approval | null;
  sandbox: Sandbox | null;
  /** Shell network mode; `null` follows the sandbox. */
  network: Network | null;
  /** Canonical hosts (lowercase, no trailing dot); `{ replace }` clears the lower layers' hosts. */
  "network-allow": string[] | { replace: string[] };
  "network-deny": string[];
  effort: Effort | null;
  skills: { load: string[]; available: string[] };
  subagents: string[];
  tools: ToolRule[];
  /** Per-namespace passthrough blocks; `meridian-flow` and `meridian-cli` are the consumer namespaces. */
  "harness-overrides": { [namespace: string]: { [key: string]: JsonValue } };
  "model-policies": ModelPolicyRule[];
}

export interface SkillFields {
  name: string | null;
  description: string | null;
  license: string | null;
  compatibility: string | null;
  metadata: JsonValue | null;
  "model-invocable": boolean;
  "user-invocable": boolean;
}

/** `[package]` of `mars.toml`. Absent optional keys are omitted, not `null`. */
export interface ManifestFields {
  name: string;
  version: string;
  description?: string;
  authors?: JsonValue;
  homepage?: JsonValue;
  license?: JsonValue;
  schema?: number;
  "uses-providers"?: string[];
  "requires-mars"?: string;
  "requires-meridian"?: string;
  /** Rules added to every agent the package ships; omitted when empty. */
  tools?: Record<string, "allow" | "deny" | "ask">;
}

/**
 * A `[dependencies]` or `[local-dependencies]` entry, in the file's spellings. Exactly one of
 * `url` and `path` is set; a bare `host/path` URL comes back as `https://host/path`. A key Mars
 * does not read is passed through, so a caller can warn.
 */
export interface DependencyEntry {
  url?: string;
  path?: string;
  subpath?: string;
  version?: string;
  dialect?: Dialect;
  plugin?: string;
  agents?: string[];
  skills?: string[];
  exclude?: string[];
  rename?: { [from: string]: string };
  only_skills?: boolean;
  only_agents?: boolean;
  [unknown: string]: JsonValue | undefined;
}

/**
 * The reading of `mars.toml`: the `[package]` table as any definition reading, and the
 * dependencies the package declares in declaration order, one `selectVersion` call per entry. A
 * table is omitted when empty, and both when `fields` is `null`. An entry that breaks a rule is
 * left out with a `dependency.*` diagnostic (an error in `[dependencies]`, a warning in
 * `[local-dependencies]`, which a dependent never installs from).
 */
export interface ManifestRead extends DefinitionRead<ManifestFields> {
  dependencies?: { [name: string]: DependencyEntry };
  "local-dependencies"?: { [name: string]: DependencyEntry };
}

export type Reading =
  | ManifestRead
  | DefinitionRead<AgentFields>
  | DefinitionRead<SkillFields>;

/**
 * Read one file; the package-relative `path` picks the reader. Any path other than
 * `mars.toml`, `agents/<slug>.md` or `skills/<slug>/SKILL.md` throws.
 */
export function read(path: "mars.toml", text: string): ManifestRead;
export function read(path: `agents/${string}.md`, text: string): DefinitionRead<AgentFields>;
export function read(path: `skills/${string}/SKILL.md`, text: string): DefinitionRead<SkillFields>;
export function read(path: string, text: string): Reading;

// ---------------------------------------------------------------------------------------
// edit
// ---------------------------------------------------------------------------------------

/**
 * YAML for the value of the mapping key whose own column is `indent`, to splice right after
 * `key:`.
 * - A scalar is one line with no leading newline: splice it after `key: `.
 * - A non-empty collection starts with `\n`, and its children sit at column `indent + 2`:
 *   splice it right after `key:`. An empty one is the one-line `[]` or `{}`.
 * - The output never ends in a newline.
 * - Strings are plain only when they are a safe word; everything else, every multi-line
 *   string included, is JSON double-quoted. Keys follow the same rule.
 *
 * Floats of any finite magnitude are written so both YAML parsers read the same number
 * (`1e+300`). Throws for an integer beyond ±(2^53 − 1), NaN, Infinity, a lone surrogate, and a
 * mapping key written as more than 1024 characters (YAML's limit for an implicit key).
 */
export function emitValue(value: JsonValue, indent: number): string;

/**
 * The full frontmatter block between the `---` fences for these top-level pairs, in order,
 * ending in exactly one newline (`""` for no keys). Mapping keys keep the order given
 * (a JavaScript object lists integer-like keys such as `"1"` first, whatever order they
 * were set in). Throws for a duplicate key, a key over 1024 characters, and a lone surrogate.
 *
 * Do not call it on a reading that has a `yaml.non-json-value` diagnostic: the keys the
 * reading left out (tags, non-string keys, out-of-range integers, `__proto__`) are not in
 * `frontmatter`, so the block would silently drop them from the file.
 */
export function rewriteBlock(frontmatter: [string, JsonValue][]): string;

// ---------------------------------------------------------------------------------------
// vocabulary
// ---------------------------------------------------------------------------------------

export type NativeEntry =
  | { kind: "tools"; tools: string[] }
  | { kind: "scoped-tools"; unscoped: string[]; scoped: string[]; note: string }
  | { kind: "absent" }
  | { kind: "no-switch"; note: string };

export interface Vocabulary {
  capabilities: { name: string; aliases: string[] }[];
  /** Per host (`claude`, `codex`, `opencode`, `pi`, `cursor`, `meridian-flow`): capability → native tools. */
  natives: { [host: string]: { [capability: string]: NativeEntry } };
}

export function vocabulary(): Vocabulary;

// ---------------------------------------------------------------------------------------
// project
// ---------------------------------------------------------------------------------------

export type Host = Harness | "meridian-flow";

/** One rule's lowering on a host. */
export interface ProjectedRule {
  /** The rule as given, in the shape `read` reports it. `origin` is the input's, `"profile"` when it gave none. */
  rule: ToolRule;
  /**
   * `known`: Mars names the tool; `absent`: the host has no such tool; `unknown`: Mars cannot
   * name it (the host has no switch for it, Mars has no row, or it is not a tool Mars knows),
   * with the reason in `note`; `unscopable`: the rule has a pattern and the host's tool takes
   * none. `null` for `*`, which names no tool.
   *
   * On `meridian-flow`, `mcp(...)` rules and MCP wire tokens lower onto the connection tools
   * `mcp_describe`, `mcp_call` and `mcp_run`: a deny of any server or tool covers all of them
   * (which can be broader than written), as does an ask with no allow-list; an allow with no
   * allow-list loses nothing, and an allow or ask under an allow-list is `known` with no native
   * names and a `note` saying none is granted, since granting them would give every connection.
   */
  status: "known" | "absent" | "unknown" | "unscopable" | null;
  /** The permission names the rule keeps. Empty for a gap, and when a stricter rule won every name. */
  native: string[];
  /** The unscoped tool names behind `native`. */
  native_tools: string[];
  /** Why the rule is not applied as written: the gap's reason, or the overlap with a stricter rule. */
  note: string | null;
  /** The stricter rule (`deny bash`) that wins every call or name this rule has. */
  covered_by: { action: "allow" | "deny" | "ask"; key: string } | null;
}

export interface ToolProjection {
  host: Host;
  /** What a tool no rule names gets: the strictest `*` rule's action, or `null` without one. */
  default: "allow" | "deny" | "ask" | null;
  /** In the order given. */
  rules: ProjectedRule[];
  /** The native names that end up allowed, denied and asked, after stricter rules win shared names. */
  allow: string[];
  deny: string[];
  ask: string[];
  /** Every allow and ask rule's names before conflicts were resolved: what an allowlist keeps available. */
  available: string[];
}

/**
 * Lower tool rules (an Agent's `tools` as `prepare` reports it) onto a host, the way `mars sync`
 * does for a harness. Throws `input.invalid` for an unknown host or a rule that does not parse.
 * A consumer must refuse a rule whose `status` is `unscopable` or `unknown` rather than read only
 * `allow`/`deny`/`ask`: an unscopable `read(src/**): deny` leaves `allow: ["read"]` granting every read.
 * When a tool is in both `available` and `deny`, `deny` wins.
 */
export function project(rules: { /** A capability, or a whole key such as `bash(git *)` or `Bash(git push:*)` when `pattern` is absent. */ capability: string; pattern?: string | null; action: "allow" | "deny" | "ask"; origin?: ToolRule["origin"] }[], host: Host): ToolProjection;

// ---------------------------------------------------------------------------------------
// prepare, inspect, normalize
// ---------------------------------------------------------------------------------------

export type Dialect = "claude" | "agent-skills" | "codex" | "opencode" | "cursor" | "mars-native";

/** A foreign dialect a root is read as, and why: `prepare` classifies with it, `inspect` reports it. */
export interface DialectChoice {
  dialect: Dialect;
  evidence: {
    source: "declared" | "predicted";
    /** The files that decided a predicted dialect; empty for a declared one. */
    paths: string[];
  };
}

export type Classification =
  | { kind: "schema-1" }
  | { kind: "schema-newer"; schema: number }
  | { kind: "invalid" }
  | ({ kind: "foreign" } & DialectChoice)
  | { kind: "marketplace" }
  | { kind: "empty" };

export type ChangeRecord =
  | { kind: "none" }
  /** The file has no source: `normalize` wrote it (a package's generated `mars.toml`). */
  | { kind: "generated" }
  | { kind: "fully-rewritten" };

export interface PreparedManifest {
  /** Absent for a generated manifest. */
  source_path?: string;
  output_path: string;
  normalized_bytes: Uint8Array;
  reading: ManifestRead;
  change: ChangeRecord;
}

export type PreparedDefinition = {
  slug: string;
  source_path: string;
  /** `skills/<slug>/SKILL.md` or `agents/<slug>.md` for a Claude-shaped source. */
  output_path: string;
  normalized_bytes: Uint8Array;
  change: ChangeRecord;
} & (
  | { kind: "agent"; reading: DefinitionRead<AgentFields> }
  | { kind: "skill"; reading: DefinitionRead<SkillFields> }
);

/**
 * An MCP server translated from a foreign plugin, written as `mcp/<name>/mcp.toml`. The staged
 * `mcp/<name>/mcp.toml`, not this list, is what `mars sync` installs from.
 */
export interface PreparedMcpServer {
  name: string;
  /** The file that declared it (`.mcp.json`, `.claude-plugin/plugin.json`). */
  source_path: string;
  output_path: string;
  /** The rendered `mcp.toml`. */
  normalized_bytes: Uint8Array;
}

export interface Prepared {
  classification: Classification;
  manifest?: PreparedManifest;
  definitions: PreparedDefinition[];
  /** A Claude plugin's `.mcp.json` and `plugin.json` servers, translated (`mcp.*` diagnostics report the rest). Absent when empty. */
  mcp_servers?: PreparedMcpServer[];
  payload: { source_path: string; output_path: string }[];
  /** What `selectItems` reads of this package; pass this result to it whole. */
  selection: SelectionFacts;
  /**
   * Package-level findings by source path: what discovery and translation met and could not
   * convert (`skill.*`, `agent.*`, `import.*`). A file's own reading diagnostics are on its
   * `reading`. `manifest.version-missing`, `skill.name-invalid` and `skill.description-too-long`
   * are `info`: nothing the author wrote is lost. `skill.name-mismatch` no longer exists;
   * `check` reports `definition.name-mismatch` (`info`) for every dialect.
   */
  diagnostics: { [sourcePath: string]: Diagnostic[] };
  /**
   * What translating a foreign package changed in each file, by source path: the keys it
   * dropped, moved or repaired (skill tool rules, Claude-only keys, a repaired `description`, a
   * key set aside under `harness-overrides` or dropped). A change coded `field.restriction-lost`
   * means a restriction (a `deny` or `ask` tool entry, `permission`, `sandbox`, `approval`) was
   * dropped or moved, so the agent is less restricted than its file says; a caller reports
   * these every time. A file changed when its `change.kind` is not `none` or `generated`.
   * Absent when empty.
   */
  changes?: { [sourcePath: string]: Diagnostic[] };
}

/**
 * Reads one root held in memory: classify it, select its definitions, translate a foreign
 * package, and read each result. This is the supported way to read a single
 * package, a consumer's `.mars-src` (`"authored-local"`) or a project directory (`"project"`,
 * where a `mars.toml` with no `[package]` is consumer configuration) without choosing items;
 * `normalize` is `prepare` plus a selection, a generated `mars.toml` and provenance. Throws for
 * an unrecognized `context` or `declaredDialect`.
 */
export function prepare(input: {
  files: Map<string, Uint8Array>;
  context?: "package" | "authored-local" | "project";
  declaredDialect?: Dialect;
}): Prepared;

/**
 * What a package's files say is wrong with it, wherever it runs: every finding `mars check`
 * reports about an Agent, a skill or `mars.toml`, except those about a harness target or an
 * older Mars. (`mars check` also reports the `requires-mars` floor, hooks, plugin output, model
 * aliases, harness field loss and an empty package, so it can fail a package this accepts.)
 * A level says what the author loses: an `error`, that it won't load or install; a `warning`,
 * that something written won't take effect; an `info`, that nothing written is lost. Show errors
 * and warnings by default, and info on request.
 *
 * Takes `prepare`'s input plus the dependencies, and prepares the root as `prepare` does. Throws
 * `input.invalid` for an unknown key or an unrecognized `context`.
 */
export function check(input: {
  files: Map<string, Uint8Array>;
  context?: "package" | "authored-local" | "project";
  declaredDialect?: Dialect;
  /**
   * The package's whole dependency closure, flat: every package reachable through any
   * `[dependencies]` entry, filtered or not, one entry per source. The key is the name the
   * source has in the `[dependencies]` table that declares it. Each value holds `prepare`'s
   * result for the dependency, as `selectItems` takes a package; only its `selection` is read
   * (`extra_items` is accepted and ignored). A plain object or a `Map`; an unknown key in an
   * entry throws `input.invalid` either way.
   */
  dependencies?: { [name: string]: SelectionPackage } | Map<string, SelectionPackage>;
}): Check;

export interface Check {
  /**
   * Findings in the root's own files, `mars.toml` first and then the paths in lexical order: for
   * each path, what `prepare` returns for the same input plus the check's own, errors first.
   * Dropping the check's own codes leaves what `prepare` returns for the path. A file with an
   * `error` is unusable; an `error` on `mars.toml` makes the whole package unusable.
   */
  diagnostics: { [sourcePath: string]: Diagnostic[] };
  /**
   * `skill-ref.dependency` only: a dependency's own Agent names a skill no package ships. By the
   * dependency's name, then the Agent's output path in it. Not the root's to fix.
   */
  dependency_diagnostics: { [dependency: string]: { [outputPath: string]: Diagnostic[] } };
  /**
   * Dependencies reachable from the root that `dependencies` lacks; `declared_in` is null for
   * the root. While this is not empty the result is not a verdict: no skill ref was judged.
   * Supply them and call again.
   */
  missing_dependencies: { name: string; declared_in: string | null }[];
  /**
   * Dependencies reachable from the root that `dependencies` has but could not be read (their
   * `selection.unreadable` is true: an invalid or newer-schema manifest); `declared_in` is null
   * for the root. While this is not empty the result is not a verdict: no skill ref was judged,
   * because an unreadable dependency may be the one that ships the skill. Its own error is in
   * `prepare`'s result for it.
   */
  unreadable_dependencies: { name: string; declared_in: string | null }[];
}

export type Tree = Map<string, Uint8Array>;

export type Kind =
  | "schema-1"
  | "schema-newer"
  | "marketplace"
  | "plugin"
  | "skill-tree"
  | "invalid"
  | "empty";

export type Checkout =
  | { kind: "sha"; sha: string }
  | { kind: "ref"; name: string }
  | { kind: "default-branch" };

export type PluginSource =
  /** A directory of the marketplace's own tree; `""` is its root. */
  | { kind: "local"; path: string }
  /** A Git repository the caller fetches; `path` roots the plugin inside it. */
  | { kind: "remote"; repository: string; path?: string; checkout: Checkout }
  /** `npm`, `archive`, `command`, or a shape this version does not know. */
  | { kind: "unsupported"; reason: string };

export interface ComponentLists {
  skills?: string[];
  agents?: string[];
  commands?: string[];
  hooks?: boolean;
  mcp?: boolean;
}

/** One marketplace `plugins[]` entry. Pass it back as `hint.plugin.entry` for a fetched tree. */
export interface MarketplaceEntry {
  name: string;
  description?: string;
  version?: string;
  source: PluginSource;
  strict?: boolean;
  lists: ComponentLists;
}

export interface Hint {
  /** Overrides prediction, never a Mars manifest. */
  dialect?: Dialect;
  /** The package root inside the tree. */
  subpath?: string;
  /** Read the root as this plugin of a marketplace (a fetched remote plugin tree). */
  plugin?: { entry: MarketplaceEntry; at_marketplace_root?: boolean };
}

export interface InspectedItem {
  kind: "skill" | "agent" | "command";
  slug: string;
  /** The definition file: `…/SKILL.md`, `agents/x.md` or `commands/x.md`. */
  source_path: string;
  /** The skill's directory, whose files travel with it. */
  skill_root?: string;
  /** Where the definition lands in schema v1; a command is reported and has none. */
  output_path?: string;
  /** A skill's payload files, by source path. */
  payload?: string[];
}

export interface Inspection {
  kind: Kind;
  /** The dialect read, how it was chosen, and the files that decided it. */
  dialect?: DialectChoice;
  /** Other harnesses' copies of the package: found, not read. A `weak` one never decides. */
  also_found: { dialect: Dialect; paths: string[]; weak: boolean }[];
  name?: string;
  version?: string;
  description?: string;
  /** A marketplace's plugins. */
  plugins: {
    entry: MarketplaceEntry;
    at_marketplace_root: boolean;
    /** Items of a plugin in the marketplace's own tree; absent for a remote one. */
    items?: InspectedItem[];
    /** Why the plugin cannot be imported. */
    refused?: string;
    diagnostics: { [path: string]: Diagnostic[] };
  }[];
  /** The items of a plugin or skill tree. */
  items: InspectedItem[];
  /** For `kind: "invalid"`, the manifest's findings under `mars.toml` (`manifest.schema-missing` for a package with no `schema`). */
  diagnostics: { [path: string]: Diagnostic[] };
}

/**
 * What a repository is, before anything is selected. For a remote marketplace plugin: fetch it,
 * then call this on the fetched tree with `hint.plugin` set to the plugin's entry.
 */
export function inspect(input: { tree: Tree; hint?: Hint }): Inspection;

export interface SourceInfo {
  name?: string;
  url?: string;
  requested_ref?: string;
  resolved_commit?: string;
  subpath?: string;
}

export interface Selection {
  /** Marketplace plugin names; absent takes every plugin. */
  plugins?: string[];
  /** Definition source paths by plugin name (`""` for a root that is not a plugin). */
  items?: { [plugin: string]: string[] };
}

export interface NormalizedPackage {
  /** The plugin name, or the package name. */
  id: string;
  provenance: {
    /** The version of the rules that produced these files. */
    translator: string;
    dialect?: Dialect;
    source: SourceInfo;
    marketplace?: { plugin: string; plugin_source: PluginSource };
    selected_items: string[];
  };
  prepared: Prepared;
}

export interface Normalized {
  kind: Kind;
  packages: NormalizedPackage[];
  /** Marketplace-level findings and the reasons a package was left out, by path. */
  diagnostics: { [path: string]: Diagnostic[] };
}

/**
 * Schema v1 packages for a selection. A remote marketplace plugin is fetched by the caller (any
 * `git-subdir` path applied) and normalized as its own tree with `hint.plugin` set to its entry;
 * `source` then describes the fetched repository.
 */
export function normalize(input: {
  tree: Tree;
  source?: SourceInfo;
  hint?: Hint;
  selection?: Selection;
}): Normalized;

/**
 * Whether `version` meets a dependency entry's `version` text: `null` or `"latest"` is any
 * version, `v2` is `>=2.0.0, <3.0.0`, a bare `0.20` is `^0.20`, and a branch or commit pin is
 * met by none. `version` that is not semver throws.
 */
export function satisfies(constraint: string | null, version: string): boolean;

/**
 * Whether `version` meets a package's `requires-mars` / `requires-meridian`, where a bare
 * `0.20` means `>=0.20.0` (not the caret range `satisfies` reads). A requirement that is not
 * one, or a `version` that is not semver, throws.
 */
export function satisfiesRequirement(requirement: string, version: string): boolean;

/**
 * Which ref a package ends at, from every constraint on it and the repository's tags. Settles
 * first (one branch or commit pin wins; two pins, or a pin beside a semver range, throw), then
 * keeps `locked` (`{ locked: true }`) when it meets every range, even if `available` no longer
 * lists its tag (only an upgrade moves a lock), else chooses the newest tag in `available` that
 * meets them. A `latest` (or `null`) constraint allows any tag and makes the newest win over
 * `locked`. A tag that is not a version (`nightly`) is ignored, and with none left the result is
 * `untagged`: the default branch. Constraints are `[requester, version | null]` as the manifests spell them
 * (`v2`, `^1.2`, `>=0.5`, a branch, `latest`); see `satisfies` for the grammar.
 */
export function selectVersion(input: {
  available: string[];
  constraints: [requester: string, version: string | null][];
  locked?: string;
}): { pin: string } | { locked: true } | { tag: string } | { untagged: true };

/** An entry's filter, as `requests` take it. `rename` is not here: selection ignores it, so it is refused. */
export interface FilterConfig {
  agents?: string[];
  skills?: string[];
  exclude?: string[];
  only_skills?: boolean;
  only_agents?: boolean;
}

/** `selectItems` decides only agents, skills and bootstrap documents; the caller selects hooks and MCP servers, and an `mcp-server` in `extra_items` throws. */
export type ItemKind = "agent" | "skill" | "hook" | "mcp-server" | "bootstrap-doc";

/**
 * An item `prepare` does not read (a bootstrap document, a hook), found by the caller.
 */
export interface ExtraItem {
  kind: ItemKind;
  slug: string;
  /** Package-relative, `/`-separated. */
  path: string;
}

/** The part of a prepared package that selection reads (`Prepared.selection`). */
export interface SelectionFacts {
  definitions: { kind: "agent" | "skill"; slug: string; output_path: string; skills: string[] }[];
  /** The manifest's `[dependencies]` entries in declaration order, never `[local-dependencies]`. */
  edges: { name: string; filter: SelectionFilter }[];
  uses_providers: string[];
  /**
   * The package could not be read (`prepared.classification` is `invalid` or `schema-newer`), so
   * `definitions` is empty because nothing was read. `check` judges no skill ref against it.
   * Absent when false.
   */
  unreadable?: boolean;
}

export type SelectionFilter =
  | "all"
  | { include: { agents: string[]; skills: string[] } }
  | { exclude: string[] }
  | "only-skills"
  | "only-agents";

export interface SelectionPackage {
  /** The result of `prepare` for this package, as it returned it. */
  prepared: Prepared;
  extra_items?: ExtraItem[];
}

export type Requester =
  /** An entry of the consumer or of `root`, an item of `root`, or one of the root's Agents' skill refs. */
  | { by: "consumer" }
  /** An unfiltered entry of this dependency's manifest, reached through unfiltered entries only. */
  | { by: "package"; package: string }
  /** A skill ref of this Agent. */
  | { by: "item"; package: string; kind: ItemKind; slug: string };

export interface SelectedItem {
  /** The package it is from; `null` for an item of `root`. */
  package: string | null;
  kind: ItemKind;
  slug: string;
  /** An Agent's file, a skill's directory (`.` for a flat skill), a document's or hook's directory. */
  path: string;
  required_by: Requester[];
}

/** A skill ref that no package ships. */
export interface MissingSkill {
  skill: string;
  /** The Agent that names it. */
  agent: string;
  /** Its package; `null` for the root. */
  package: string | null;
  /** The packages looked in, sorted; the root is not among them. */
  searched: string[];
  /** For a name shaped like a provider's, when no package declares that provider. */
  hint: string | null;
}

export interface SelectionResult {
  /** In registry order (below), the root first, then each package's own item order. */
  items: SelectedItem[];
  missing_skills: MissingSkill[];
  /** `selection.duplicate-slug`: two packages (the root counts) install the same Agent or skill slug. */
  diagnostics: Diagnostic[];
}

/**
 * The Agents, skills and bootstrap documents an install of `root` and its dependencies
 * yields, and why each is selected. Hooks and MCP servers are selected by `mars sync` (visibility
 * and depth), so Flow and sync are guaranteed to agree only on these three kinds. `{ root, packages }` is the whole call for an
 * imported package: `root` is selected whole, and its manifest entries (`[dependencies]`, with
 * their filters and `exclude`) are applied as the consumer's own requests, so a filtered
 * dependency contributes what its filter names plus the skills the selected Agents name.
 * `requests` holds further consumer entries, in order, for a project whose configuration is not
 * in `root`'s manifest.
 *
 * Pass each package's `prepare` result whole (selection reads `selection`). `packages` holds the
 * dependencies by name in any order; the registry order that skill refs resolve in is computed
 * here (depth first from `root`, then each request, descending manifest entries in declaration
 * order). A package nothing reaches is never selected from. Installed names (renames, collision
 * suffixes) are the caller's.
 *
 * Unknown keys throw `input.invalid`, at the top level, in a package and in a request.
 */
export function selectItems(input: {
  requests?: ({ name: string } & FilterConfig)[];
  packages: { [name: string]: SelectionPackage } | Map<string, SelectionPackage>;
  root?: SelectionPackage;
}): SelectionResult;

/** What a hook installs on one target: the schema's own `HookOutput`, as `mars sync` reports it. */
export type HookTargetOutput =
  | {
      kind: "merge";
      /** Finished entries per native event. A native hook's `[]` is kept: it turns that event off. */
      events: { [event: string]: unknown[] };
      /** Unknown events a dependency's native hook lists, left out of `events`. */
      skippedEvents: string[];
      /** Events the target does not list that `unchecked = true` let through. */
      uncheckedEvents: string[];
      /** Events a native hook declares that `[hooks]` also declares: the native hook's replace the generated entries. */
      overrides: string[];
      skips: HookSkip[];
    }
  | {
      kind: "file";
      files: { path: string; content: string }[];
      /** What is wrong with the authored native hook, if anything; it is installed regardless. */
      nativeHookProblem: string | null;
      skips: HookSkip[];
      /** Rendered events whose portable answers do not all carry over to the target. */
      answerLimits: HookSkip[];
    };

/** Something a target does not run. `event` is `null` when the whole target is skipped. */
export interface HookSkip {
  event: string | null;
  reason: string;
}

/**
 * A `hook.toml`'s `[hooks]` table as each target in `targets` (roots such as `.claude`,
 * `.codex`) takes it, from the same render `mars sync` uses. `tokens` fills `${MARS_HOOK_DIR}`
 * and `${MARS_HOOK_TARGET}`; one left out stays as written (`hookTarget` defaults to the
 * target's root). When `diagnostics` holds an error, `targets` is empty. A root with no hook
 * mechanism throws `input.invalid`.
 */
export function renderHooks(
  hookToml: string,
  targets: string[],
  tokens?: { hookDir?: string; hookTarget?: string } | null,
): { diagnostics: Diagnostic[]; targets: { [root: string]: HookTargetOutput } };

// ---------------------------------------------------------------------------------------
// resolve
// ---------------------------------------------------------------------------------------

/** One `[dependencies.<name>]` row of `mars.lock`, as `mars sync` writes it. Urls are the lock's canonical spelling (`github.com/owner/repo`). */
export interface LockedDependency {
  url?: string;
  version?: string;
  commit?: string;
  /** The marketplace plugin installed from this source; `url`, `version` and `commit` are then the marketplace's. */
  plugin?: string;
  /** Where the plugin's files came from. */
  content?: { url?: string; subpath?: string; commit?: string };
}

/** A fetch `resolve` still needs. Each need is its own key: answer it once. */
export type Need =
  | { kind: "tags"; url: string }
  | { kind: "ref"; url: string; ref: string }
  /** `path: null` is the whole repository. `commit` can be an annotated tag's object id, from a lock Mars 0.34 or earlier wrote. */
  | { kind: "tree"; url: string; commit: string; path: string | null };

export type Answer =
  /** Every tag; the `commit` of an annotated tag is the commit it points at, never the tag object. Order and non-version tags don't matter. */
  | { need: { kind: "tags"; url: string }; tags: { tag: string; commit: string }[] }
  /** A branch, `HEAD` or a commit-as-ref, as a commit. */
  | { need: { kind: "ref"; url: string; ref: string }; commit: string }
  /**
   * The files at `commit` under `path`, repository-relative (`path: null` is the whole repository).
   * It answers every tree need at the same `url` and `commit` whose `path` is inside its own, so one
   * whole-repository answer serves every later need there. Files only, a symlink as its target's
   * content; binary files may be left out.
   */
  | { url: string; commit: string; path: string | null; files: { [path: string]: Uint8Array } | Map<string, Uint8Array> }
  /** The commit or ref doesn't exist. A locked commit that is missing is dropped with a warning; any other is `unavailable`. */
  | { need: Need; missing: true }
  /** Any other error (network, auth, a 5xx, a limit of your own). Always `unavailable`; it never drops a lock. */
  | { need: Need; failed: string };

export interface ResolveDiagnostic {
  /** `warning`: the sync fell short of what was asked; `advisory`: a note that doesn't imply it; `info`. */
  level: "warning" | "advisory" | "info";
  /** Stable code, e.g. `locked-commit-unreachable`, `requires-mars-fallback`. */
  code: string;
  message: string;
}

/** A requester (`"root"` for the root's own `mars.toml`) and the constraint it asked for (`null`: any version). */
export interface Requester {
  by: string;
  constraint: string | null;
}

export type ResolveFailure = { message: string } & (
  /** No version meets every constraint. */
  | { kind: "conflict"; name: string; requesters: Requester[] }
  /** Two names for one source, or one name for two sources (`existing` is the one it had first). */
  | { kind: "identity"; detail: "one-source-two-names" | "one-name-two-sources"; names: string[]; source: string; existing?: string }
  /** A pinned or locked version needs a newer Mars or Meridian. `engine` is `"mars"` or `"meridian"`; the first unmet requirement. */
  | { kind: "engine-incompatible"; name: string; requirement: string | null; engine: "mars" | "meridian" | null }
  /** Every candidate version was excluded by its engine requirement. */
  | { kind: "engine-unsatisfiable"; name: string; excluded: { version: string; requirement: string; engine: "mars" | "meridian" }[] }
  | { kind: "engine-requirement-invalid"; name: string; requirement: string; engine: string }
  /** The `engine` version given does not parse. */
  | { kind: "engine-version-invalid"; engine: string; version: string }
  | {
      kind: "plugin";
      name: string;
      detail: "not-a-marketplace" | "not-listed" | "refused" | "choice-needed" | "not-one-package" | "unusable-path";
      plugin?: string;
    }
  | { kind: "subpath"; name: string; subpath: string; detail: "missing" | "not-a-directory" | "traversal" }
  /** A package's manifest is not TOML. */
  | { kind: "unreadable"; name: string; diagnostics: Diagnostic[] }
  /** A fetched package (`declared_in`, or `"root"`) declares a path dependency. */
  | { kind: "path"; name: string; declared_in: string }
  /** A `failed` answer, or a `missing` one that did not come from a lock. */
  | { kind: "unavailable"; need: Need; reason: string }
  | { kind: "bounds"; limit: "packages" | "bytes"; at: string }
  | { kind: "oscillation"; name: string; steps: { version: string | null; commit: string | null }[] }
);

export interface ResolvedDependency {
  /** Where to vendor from: the repository (the marketplace's, for a plugin), at `commit`. */
  url: string;
  commit: string;
  subpath: string | null;
  /** The tag chosen; `null` when untagged or pinned to a ref. */
  version: string | null;
  /** Set for a marketplace plugin: where its files are (another repository, or a folder of the marketplace's own tree). */
  content?: { url: string; subpath: string | null; commit: string };
  required_by: Requester[];
}

/**
 * Which version of which source, for a whole dependency graph: the rules of `mars sync` without
 * the fetching. Stateless: each call replays the resolution from every answer so far, so keep the
 * list of answers and call again with it grown until the result is not `needs`.
 *
 * - `{ needs }`: every open need on the current frontier (one with no answer), sorted; fetch them in
 *   parallel. A later need can depend on an earlier answer, so a call can return needs more than
 *   once. While any need is open the result is `needs`: a failure found after one waits.
 * - `{ resolved }`: `lock` is exactly mars.lock's `[dependencies.<name>]` rows, to store verbatim;
 *   `dependencies` is what to vendor and to hand to `check` and `selectItems`.
 * - `{ failed }`: the first failure; resolution stops there. `message` is the text `mars sync`
 *   prints for it.
 *
 * `refresh` ignores `locked` for the names it lists (`mars upgrade <name>`), or for every name
 * (`"all"`, `mars upgrade`). `engine.mars` judges `requires-mars`; `engine.meridian`, when given,
 * `requires-meridian`. `bounds.packages` caps the dependencies in the closure; `bounds.bytes` the
 * bytes of the files Mars reads, counted afresh at each restart and checked at the end of each
 * pass. A path dependency is never a need: it is the `path` failure. `supplied` (an edited
 * manifest over a fetched tree) is not supported yet and throws.
 *
 * A malformed argument throws `input.invalid`.
 */
export function resolve(input: {
  /** The root's `mars.toml` text; only its `[dependencies]` matter. */
  root: { manifest: string };
  supplied?: { [name: string]: { manifest: string } };
  locked?: { [name: string]: LockedDependency };
  refresh?: "all" | string[];
  engine: { mars: string; meridian?: string };
  bounds?: { packages?: number; bytes?: number };
  answers: Answer[];
}):
  | { needs: Need[] }
  | { resolved: { lock: { [name: string]: LockedDependency }; dependencies: { [name: string]: ResolvedDependency } }; diagnostics: ResolveDiagnostic[] }
  | { failed: ResolveFailure; diagnostics: ResolveDiagnostic[] };

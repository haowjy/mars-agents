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

/** Build-time version of the schema crate. */
export function version(): string;

// ---------------------------------------------------------------------------------------
// read
// ---------------------------------------------------------------------------------------

export interface Diagnostic {
  /** Stable code, e.g. `field.unknown`, `yaml.invalid`, `field.removed`. */
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
  /** Per-namespace passthrough blocks; `flow` and `meridian` are the consumer namespaces. */
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
}

export type Reading =
  | DefinitionRead<ManifestFields>
  | DefinitionRead<AgentFields>
  | DefinitionRead<SkillFields>;

/**
 * Read one file; the package-relative `path` picks the reader. Any path other than
 * `mars.toml`, `agents/<slug>.md` or `skills/<slug>/SKILL.md` throws.
 */
export function read(path: "mars.toml", text: string): DefinitionRead<ManifestFields>;
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
  /** Per host (`claude`, `codex`, `opencode`, `pi`, `cursor`, `flow`): capability → native tools. */
  natives: { [host: string]: { [capability: string]: NativeEntry } };
}

export function vocabulary(): Vocabulary;

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
  | { kind: "mars-legacy" }
  | { kind: "schema-newer"; schema: number }
  | { kind: "invalid" }
  | ({ kind: "foreign" } & DialectChoice)
  | { kind: "marketplace" }
  | { kind: "empty" };

export type ChangeRecord =
  | { kind: "none" }
  /** The file has no source: `normalize` wrote it (a package's generated `mars.toml`). */
  | { kind: "generated" }
  | { kind: "edited"; paths: (string | number)[][] }
  | { kind: "fully-rewritten" };

export interface PreparedManifest {
  /** Absent for a generated manifest. */
  source_path?: string;
  output_path: string;
  normalized_bytes: Uint8Array;
  reading: DefinitionRead<ManifestFields>;
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

export interface Prepared {
  classification: Classification;
  manifest?: PreparedManifest;
  definitions: PreparedDefinition[];
  payload: { source_path: string; output_path: string }[];
  /**
   * Package-level findings by source path: what discovery and translation met and could not
   * convert (`skill.*`, `agent.*`, `import.*`). A file's own reading diagnostics are on its
   * `reading`.
   */
  diagnostics: { [sourcePath: string]: Diagnostic[] };
  /**
   * What preparing changed in each file, by source path: the keys a pre-v1 upgrade
   * (`classification.kind === "mars-legacy"`) or a foreign translation dropped, moved or
   * repaired (legacy keys, skill tool rules, Claude-only keys, a repaired `description`, a key
   * set aside under `harness-overrides` or dropped). A change coded `field.restriction-lost`
   * means a restriction (a `deny` or `ask` tool entry, `permission`, `sandbox`, `approval`) was
   * dropped or moved, so the agent is less restricted than its file says; a caller reports
   * these every time. A file changed when its `change.kind` is not `none` or `generated`.
   * Absent when empty.
   */
  changes?: { [sourcePath: string]: Diagnostic[] };
}

/**
 * Reads one root held in memory: classify it, select its definitions, upgrade a pre-v1 package
 * or translate a foreign one, and read each result. This is the supported way to read a single
 * package, a consumer's `.mars-src` (`"authored-local"`) or a project directory (`"project"`,
 * where a `mars.toml` with no `[package]` is consumer configuration) without choosing items;
 * `normalize` is `prepare` plus a selection, a generated `mars.toml` and provenance. Throws for
 * an unrecognized `context` or `declaredDialect`.
 */
export function prepare(input: {
  files: Map<string, Uint8Array>;
  context?: "package" | "authored-local" | "project";
  declaredDialect?: Dialect;
  /**
   * `migrate` adds `mars migrate`'s repair: a removed Agent key whose replacement is a
   * `harness-overrides` key moves there. A reader leaves it off, because an authored schema 1
   * file is read exactly as written.
   */
  migrate?: boolean;
}): Prepared;

export type Tree = Map<string, Uint8Array>;

export type Kind =
  | "schema-1"
  | "mars-legacy"
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
 * The ref a package ends at, from every `[requester, version]` on it, whether or not the entry
 * selects anything: the one branch or commit pin if there is one, `null` if `selectVersion`
 * should choose among the tags. Two different pins, or a pin beside a semver range, throw
 * (`Settle` in the crate: the message names both requesters).
 */
export function settle(constraints: [requester: string, version: string | null][]): string | null;

/**
 * Chooses one of `available` (tags: one leading `v` allowed, the rest strict semver) that meets
 * every constraint and is not in `excluded`. The newest allowed wins whatever the order of
 * `available`. `policy` defaults to `"prefer-lock-then-latest"`: `locked` when allowed, else
 * the newest. `"lock-only"` returns `locked` or throws. Nothing allowed throws, with a message
 * naming each requirement and the versions there are.
 */
export function selectVersion(input: {
  available: string[];
  /** `[requester, version]` semver ranges; a pin or `latest` throws (settle those first). */
  constraints: [requester: string, version: string][];
  locked?: string;
  policy?: "prefer-lock-then-latest" | "latest-only" | "lock-only";
  excluded?: string[];
}): string;

/** An entry's filter as the manifest or `mars.toml` spells it. */
export interface FilterConfig {
  agents?: string[];
  skills?: string[];
  exclude?: string[];
  rename?: { [from: string]: string };
  only_skills?: boolean;
  only_agents?: boolean;
}

/**
 * An item `prepare` does not read (a bootstrap document, a hook), found by the caller. `kind`
 * is `"agent"`, `"skill"`, `"hook"`, `"mcpserver"` or `"bootstrapdoc"`.
 */
export interface ExtraItem {
  kind: SelectedItem["kind"];
  slug: string;
  /** Package-relative, `/`-separated. */
  path: string;
}

export interface SelectionPackage {
  /** The result of `prepare` for this package, as it returned it. */
  prepared: Prepared;
  extraItems?: ExtraItem[];
}

export type Requester =
  /** An entry of the consumer or of `root`, or one of the root's Agents' skill refs. */
  | { by: "consumer" }
  /** An unfiltered entry of this package's manifest, reached through unfiltered entries only. */
  | { by: "package"; package: string }
  /** A skill ref of this Agent. */
  | { by: "item"; package: string; kind: SelectedItem["kind"]; slug: string };

export interface SelectedItem {
  package: string;
  kind: "agent" | "skill" | "hook" | "mcpserver" | "bootstrapdoc";
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
  /** In registry order (below), then each package's own item order. */
  items: SelectedItem[];
  missing_skills: MissingSkill[];
  /** `selection.duplicate-slug`: two packages install the same Agent or skill slug. */
  diagnostics: Diagnostic[];
}

/**
 * The Agents, skills and package-level documents a consumer installs from a settled graph, and
 * why each is selected. Pass each package's `prepare` result whole: its edges (the manifest's
 * `[dependencies]` entries) and `uses-providers` are read from it. `packages` holds the graph's
 * packages by name in any order; the registry order that skill refs resolve in is computed here
 * (depth first from `root`, then each request, descending manifest entries in declaration
 * order). A package no request reaches is never selected from.
 *
 * `requests` are the consumer's own entries, in order. `root` is the package being selected for
 * (a project's own package): it is requested whole as the first request and its own items are
 * not in the result, but its Agents' skill refs resolve like any package's. Installed names
 * (renames, collision suffixes) are the caller's.
 */
export function selectItems(input: {
  requests: ({ name: string } & FilterConfig)[];
  packages: { [name: string]: SelectionPackage };
  root?: SelectionPackage;
}): SelectionResult;

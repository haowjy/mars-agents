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

/**
 * Web build only: instantiate the WASM module before calling anything else
 * (`await init()`). The Node build is synchronous and exports no `init`.
 */
export default function init(
  module_or_path?: RequestInfo | URL | Response | BufferSource | WebAssembly.Module | Promise<unknown>,
): Promise<unknown>;

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
   * value has no JSON form (a YAML tag, a non-string map key, an integer beyond ±(2^53 − 1)) is absent and has a
   * `yaml.non-json-value` diagnostic. Mapping keys, nested ones included, are in file order (JavaScript lists integer-like keys such as `"1"` first). For `mars.toml`, the
   * pairs of the `[package]` table.
   */
  frontmatter: [string, JsonValue][];
  /** Typed fields in file spellings; `null` when the file cannot be read at all. */
  fields: Fields | null;
  /** Text after the frontmatter; `null` for `mars.toml` or an unreadable file. */
  body: string | null;
  diagnostics: Diagnostic[];
}

export type Effort = "none" | "low" | "medium" | "high" | "xhigh";
export type Sandbox = "default" | "read-only" | "workspace-write" | "danger-full-access";
export type Approval = "default" | "auto" | "confirm" | "never";
export type Harness = "claude" | "codex" | "pi" | "opencode" | "cursor";

export interface ToolRule {
  /** A canonical capability, `*`, `mcp`, or the author's spelling of an unknown name. */
  capability: string;
  pattern: string | null;
  action: "allow" | "deny" | "ask";
}

export interface ModelPolicyRule {
  match: { model: string } | { alias: string } | { "model-glob": string };
  override?: { effort?: Effort; sandbox?: Sandbox; approval?: Approval };
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
  effort: Effort | null;
  skills: { load: string[]; available: string[] };
  subagents: string[];
  tools: ToolRule[];
  /** Per-namespace passthrough blocks; `flow` is the consumer namespace. */
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
 * Throws for an integer beyond ±(2^53 − 1), NaN and Infinity.
 */
export function emitValue(value: JsonValue, indent: number): string;

/**
 * The full frontmatter block between the `---` fences for these top-level pairs, in order,
 * ending in exactly one newline (`""` for no keys). Mapping keys keep the order given
 * (a JavaScript object lists integer-like keys such as `"1"` first, whatever order they
 * were set in). Throws for a duplicate key.
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
// prepare (provisional: P5 replaces it with inspect/normalize and adds kinds)
// ---------------------------------------------------------------------------------------

export type Dialect = "claude" | "codex" | "opencode" | "cursor" | "mars-native";

export type Classification =
  | { kind: "schema-1" }
  | { kind: "mars-legacy" }
  | { kind: "schema-newer"; schema: number }
  | { kind: "invalid" }
  | {
      kind: "foreign";
      dialect: Dialect;
      evidence: { source: "declared" | "predicted"; paths: string[] };
    }
  | { kind: "empty" };

export type ChangeRecord =
  | { kind: "none" }
  | { kind: "edited"; paths: (string | number)[][] }
  | { kind: "fully-rewritten" };

export interface PreparedManifest {
  source_path: string;
  output_path: string;
  normalized_bytes: Uint8Array;
  reading: DefinitionRead<ManifestFields>;
  change: ChangeRecord;
}

export type PreparedDefinition = {
  slug: string;
  source_path: string;
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
   * Package-level diagnostics by source path (discovery and upgrade). A file's own reading
   * diagnostics are on its `reading`.
   */
  diagnostics: { [sourcePath: string]: Diagnostic[] };
}

/** Throws for an unrecognized `context` or `declaredDialect`. */
export function prepare(input: {
  files: Map<string, Uint8Array>;
  context?: "package" | "authored-local";
  declaredDialect?: Dialect;
}): Prepared;

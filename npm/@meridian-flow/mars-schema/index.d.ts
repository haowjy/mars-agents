export type JsonValue = null | boolean | number | string | JsonValue[] | { [key: string]: JsonValue };
export interface ReadInput { kind: "agent" | "skill"; text: string }
export interface ReadResult<T = JsonValue> { frontmatter: [string, JsonValue][]; fields: T | null; body: string | null; unknown: [string, JsonValue][]; diagnostics: Diagnostic[] }
export interface Diagnostic { code: string; severity: "error" | "warning" | "info"; path: (string | number)[]; message: string; range?: { start: number; end: number } }
export function version(): string;
export function read(input: ReadInput): ReadResult;
export function emitValue(value: JsonValue, indent: number): string;
export function rewriteBlock(frontmatter: [string, JsonValue][]): string;
export function vocabulary(): JsonValue;
export function prepare(input: { files: Map<string, Uint8Array>; context?: "package" | "authored-local" }): JsonValue;

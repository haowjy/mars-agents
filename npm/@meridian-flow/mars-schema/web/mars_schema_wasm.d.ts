/* tslint:disable */
/* eslint-disable */

export function emitValue(value: any, indent: number): string;

/**
 * Prepare a package from `{files: {path: Uint8Array}, context, declaredDialect}`.
 */
export function prepare(input: any): any;

/**
 * Read one definition. `kind` is `agent` or `skill`; `text` is UTF-8 markdown.
 */
export function read(input: any): any;

export function rewriteBlock(value: any): string;

/**
 * Returns the Mars schema package version.
 */
export function version(): string;

export function vocabulary(): any;

export type InitInput = RequestInfo | URL | Response | BufferSource | WebAssembly.Module;

export interface InitOutput {
    readonly memory: WebAssembly.Memory;
    readonly emitValue: (a: number, b: number, c: number) => void;
    readonly prepare: (a: number, b: number) => void;
    readonly read: (a: number, b: number) => void;
    readonly rewriteBlock: (a: number, b: number) => void;
    readonly version: (a: number) => void;
    readonly vocabulary: (a: number) => void;
    readonly __wbindgen_export: (a: number, b: number) => number;
    readonly __wbindgen_export2: (a: number, b: number, c: number, d: number) => number;
    readonly __wbindgen_export3: (a: number) => void;
    readonly __wbindgen_add_to_stack_pointer: (a: number) => number;
    readonly __wbindgen_export4: (a: number, b: number, c: number) => void;
}

export type SyncInitInput = BufferSource | WebAssembly.Module;

/**
 * Instantiates the given `module`, which can either be bytes or
 * a precompiled `WebAssembly.Module`.
 *
 * @param {{ module: SyncInitInput }} module - Passing `SyncInitInput` directly is deprecated.
 *
 * @returns {InitOutput}
 */
export function initSync(module: { module: SyncInitInput } | SyncInitInput): InitOutput;

/**
 * If `module_or_path` is {RequestInfo} or {URL}, makes a request and
 * for everything else, calls `WebAssembly.instantiate` directly.
 *
 * @param {{ module_or_path: InitInput | Promise<InitInput> }} module_or_path - Passing `InitInput` directly is deprecated.
 *
 * @returns {Promise<InitOutput>}
 */
export default function __wbg_init (module_or_path?: { module_or_path: InitInput | Promise<InitInput> } | InitInput | Promise<InitInput>): Promise<InitOutput>;

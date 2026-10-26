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

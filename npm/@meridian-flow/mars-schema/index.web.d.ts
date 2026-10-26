// Types for the web (browser) build of @meridian-flow/mars-schema: the same API, which must be
// instantiated first with `await init()`. The Node build is synchronous and has no `init`.
export * from "./index";

export default function init(
  module_or_path?: RequestInfo | URL | Response | BufferSource | WebAssembly.Module | Promise<unknown>,
): Promise<unknown>;

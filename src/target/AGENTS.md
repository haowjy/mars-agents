# Target adapters

Target adapters isolate harness-specific paths and MCP formats from the shared compiler. Hook placement and rendering belong to `mars-schema::hooks` rather than the adapter's MCP specification.

MCP ownership applies to entries, not the containing file. Reconcile removes or replaces only recorded, unchanged entries; edited entries are preserved unless explicitly forced, and failed writes retain previous records for retry. Hook entries are unkeyed array elements, so their removal and rewrite use the shared retention plan instead of MCP's keyed reconciliation.

Config writes preserve existing permissions and write through symlinks so a user's link remains intact. Generated agent filenames are validated for Windows on every platform so packages remain portable.

An empty MCP config file is removed only when reconciliation changed it and left it empty; an untouched empty file may belong to the user. Ordered JSON editing preserves the user's key order without enabling `serde_json/preserve_order` globally, which would change unrelated JSON output.

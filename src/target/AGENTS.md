# Target adapters

Hook placement and rendering belong to `mars-schema::hooks`, not the adapter's MCP specification. Hook entries are unkeyed array elements, so their removal and rewrite use the shared retention plan, not MCP's keyed reconciliation.

MCP ownership applies to entries, not the containing file. Reconcile removes or replaces only recorded, unchanged entries; edited entries are kept unless forced, and failed writes retain previous records for retry. An empty MCP config file is removed only when reconciliation changed it and left it empty: an untouched empty file may be the user's.

Config writes preserve existing permissions and write through symlinks so a user's link survives. Generated agent filenames are validated for Windows on every platform. Ordered JSON editing preserves the user's key order without enabling `serde_json/preserve_order` globally, which would change unrelated JSON output.

# Config

Layers compose user, project, then local into `EffectiveConfig`. Consume that result rather than recomposing files, and never save it back to a source file: use the existing save path, which patches the authored TOML and verifies the result so unknown keys and comments survive.

Model aliases and skill/agent overlays replace by key. An agent's network host sets compose across project and local layers; scalar fields in the local agent entry replace the project entry, and tool rule entries merge by key. Preserve absent versus explicitly empty.

`read.rs` is the one reader of all three files. Serde reads the keys Mars knows; every key it leaves unread is looked up in `mars_schema::keys`: a removed key fails the load with its replacement, anything else is a warning that names file, table and key, and the key stays in the file. Don't add a per-key check, a `flatten` map for unknown keys (it loses the line of a bad value) or a second parse path: a new key is a struct field, a removed one is a table row, and a file Mars reads goes through `parse_config` or `parse_local` so `save` and `save_local` refuse what a load would. `migrations/consumer.rs` applies the rows' rewrites for `mars migrate` and must never widen an agent's tool access. Plugin-only metadata stays raw at load so invalid publication metadata can't block sync; plugin build validation checks it.

Only known harness targets grant routing permission. Generic targets and paths materialize content without granting harness access; an explicitly empty scope must not become unrestricted. See [config contracts](.context/CONTEXT.md).

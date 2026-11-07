# Config

Configuration composes user, project, then local layers into `EffectiveConfig`. Consume that result rather than recomposing files; never save the composed result back to a source file.

Model aliases and skill/agent overlays replace by key. Agent network and tool permission sets compose across project and local layers, while scalar fields in the local agent entry replace the project entry. Plain permission lists add to lower rules; `{ replace = [...] }` clears the lower rules for that action. Preserve absent versus explicitly empty fields.

Unknown network-looking keys warn because silently ignoring a restriction could fail open. Other unknown settings and overlay keys remain forward-compatible. Plugin-only metadata stays raw during config loading, so invalid publication metadata cannot block sync; plugin build validation applies its defaults and checks.

Only known harness targets grant routing permission. Generic targets and paths materialize content without granting harness access; an explicitly empty scope must not become unrestricted. See [config contracts](.context/CONTEXT.md).

Use the existing save path: it patches the authored TOML and verifies the result, preserving unknown keys and unrelated comments instead of replacing the file with a serialization of the model.

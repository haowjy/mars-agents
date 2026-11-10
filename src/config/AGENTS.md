# Config

Layers compose user, project, then local into `EffectiveConfig`. Consume that result rather than recomposing files, and never save it back to a source file: use the existing save path, which patches the authored TOML and verifies the result so unknown keys and comments survive.

Model aliases and skill/agent overlays replace by key. Agent network and tool permission sets compose across project and local layers; scalar fields in the local agent entry replace the project entry. Plain permission lists add to lower rules; `{ replace = [...] }` clears the lower rules for that action. Preserve absent versus explicitly empty.

Unknown network-looking keys warn because ignoring a restriction could fail open; other unknown settings and overlay keys stay forward-compatible. Plugin-only metadata stays raw at load so invalid publication metadata can't block sync; plugin build validation checks it.

Only known harness targets grant routing permission. Generic targets and paths materialize content without granting harness access; an explicitly empty scope must not become unrestricted. See [config contracts](.context/CONTEXT.md).

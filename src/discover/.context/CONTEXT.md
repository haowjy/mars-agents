# Source discovery

Hidden directories are excluded from the convention walk so generated harness
outputs and control directories do not become package content. A hidden foreign
layout remains importable when explicitly selected as the dependency subpath.
Other configured non-hidden output trees remain discoverable.

Hooks and bootstrap documents with the same `(kind, name)` collide within one
source; there is no silent shadowing.

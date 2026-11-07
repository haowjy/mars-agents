# Tests

Protect behavior at its owning boundary. Keep ownership, crash recovery, path safety and distinct failure phases explicit; assert actual destinations, lock ownership and emitted data. Delete redundant scaffolding and duplicate coverage, not distinct failure cases.

For local non-network CLI fixtures use `common::offline_mars(root)`. Catalog and probe tests need local servers, fake executables and an explicitly controlled PATH; bare `mars()` inherits the environment. Never use a live model-backed agent as a fixture or mutate assumed-absent absolute paths: arrange absence under a temporary directory.

Explain what retains protection when deleting a test. Track ignored specifications as work rather than treating them as passing coverage. See [smoke conventions](smoke/README.md).

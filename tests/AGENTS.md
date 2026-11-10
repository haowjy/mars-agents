# Tests

Assert actual destinations, lock ownership and emitted data. Delete redundant scaffolding and duplicate coverage, not distinct failure cases (ownership, crash recovery, path safety).

For local non-network CLI fixtures use `common::offline_mars(root)`; bare `mars()` inherits the environment. Catalog and probe tests need local servers, fake executables and an explicitly controlled PATH. Never use a live model-backed agent as a fixture or mutate assumed-absent absolute paths: arrange absence under a temporary directory.

Ignored specifications aren't passing coverage. See [smoke conventions](smoke/README.md).

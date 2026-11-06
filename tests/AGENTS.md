# Tests

Use `common::offline_mars(root)` for local CLI fixtures: bare `mars()` inherits
the environment. Catalog and probe fixtures need local servers, fake executables
and a controlled PATH. A live model-backed agent is not a test fixture.

[Smoke conventions](smoke/README.md)

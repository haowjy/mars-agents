# Tests

Keep tests that protect named behavior, not an arbitrary suite size. Library
`#[test]` cases may exercise filesystems, HTTP or resolution: Cargo's `--lib`
bucket is not a testing tier.

- Prefer one owning boundary. Delete derived-trait/default scaffolding and
  duplicate roundtrips when an existing behavior test covers the same risk.
- Keep ownership, crash recovery, path/referent safety and distinct failure
  phases explicit. Similar setup does not make their contracts interchangeable.
- Assert actual destinations, lock ownership and emitted data. A missing file at
  an obsolete path, empty diagnostic array or absent prompt field proves nothing.
- Register tests on their bodies; no forwarding wrappers to preserve test names.
- For local, non-network/non-Git CLI fixtures, use `common::offline_mars(root)`:
  temporary HOME/cache, empty PATH and offline behavior. Its environment lives
  under the fixture's excluded `.mars/test-env` directory.
- Catalog/probe tests use local servers and fake executables with
  `configure_assert_cmd` / `mars_cmd`; explicitly control PATH. Never use a live
  model-backed agent as a test fixture. Bare `mars()` inherits the environment;
  remaining migrations are tracked in issue #167.
- Never mutate assumed-absent absolute paths. Arrange absence under a TempDir.
- Test deletions need a retained-protection explanation, not replacement tests by
  default. Ignored specifications belong in tracked work, not a green-suite claim.

[Smoke conventions](smoke/README.md).

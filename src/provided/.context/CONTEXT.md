# Provided skill store

`mars provide` lets a host program (Meridian today) install skills it ships into
`.mars/skills/` and record their names in `.mars/provided.lock`. Provided skills are not
dependencies: they are absent from `mars.toml`, `mars.lock` and the resolve graph, so nothing
projects them into a managed target and every committed file stays byte-identical with or without
a host. Not-yet-implemented readers (resolver, ref checks, sync ownership) are the other half of
that story; until they land, a package item whose destination is a provided skill is still an
unmanaged collision.

## The fast path runs before the lock

Hosts launch in parallel and during long syncs, so taking `.mars/sync.lock` first would turn every
no-op into `busy` and a fallback read. The unlocked check reads `.mars/provided.lock` and
`mars.lock`, and both are only ever published by tmp + rename, so it sees a whole file either way.
`FileLock::try_acquire` reports held-ness separately from failure; treating every error as
contention would silently disable the refresh on a broken lock file.

The fast path proves the source is what the record applied (`version`, `digest`, none claimed
by `mars.lock`) **and** that every recorded installed tree still hashes to its source skill's
digest. Existence is not ownership: a sync that replaced a recorded directory with a package's
copy and died before publishing `mars.lock` leaves foreign bytes under a recorded name, and the
host would read them as its own. Hashing a host's few small skills costs milliseconds, against a
host launch measured in hundreds.

## Ownership and the apply

A name is Mars-owned here only if this provider's record lists it. Everything else follows from
that: absent → install, recorded and different → replace, unrecorded and byte-equal → adopt,
unrecorded and different → `unmanaged-collision` (report, never replace). A name `mars.lock`
claims belongs to a package: the record drops it (`shadowed`) and its directory is left alone,
because removals only ever name directories this record lists.

Crash safety is the pre-record, not a journal: step 6 writes this provider's section with
`digest = "pending"` and the union of the old record and everything about to be written, then each
directory installs atomically, then step 8 writes the final digest and names. A crash anywhere in
between leaves a record no run can read as current, so the next run replans from the files on disk
and converges — including a skill the host dropped meanwhile, which is still on record and still
removed. `"pending"` is deliberately not a digest: it must never compare equal to one.

The tree is copied as authored (no normalization, no `prepare`), because the digest and the
byte-equal adopt rule compare raw bytes. `mars_schema::skill::read` is used only to refuse a
`SKILL.md` that error-level diagnostics would make every later reader drop; warnings (schema v1's
`mode:` on Meridian's bundle, for instance) are reported and the skill still installs.

## `.mars/provided.lock`

`version = 1`, one `[providers.<name>]` table holding `version` (opaque, for display and change
detection), `digest` and `skills`. No paths, no per-item checksums: `.mars/` is derived and
gitignored, so nothing committed carries a provider, a path or a version. A corrupt file is a
warning and an empty record (adoption recovers what the disk can prove); an unknown version is
refused, because a newer Mars wrote it. Recorded names are validated as single path components
before use, since the file is written by another build.

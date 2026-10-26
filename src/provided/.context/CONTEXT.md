# Provided skill store

`mars provide` lets a host program (Meridian today) install skills it ships into
`.mars/skills/` and record their names in `.mars/provided.lock`. Provided skills are not
dependencies: they are absent from `mars.toml`, `mars.lock` and the resolve graph, so nothing
projects them into a managed target and every committed file stays byte-identical with or without
a host. The readers (sync's ref validator, sync ownership, `mars check`, `mars doctor`) are the
other half of that story: see "What readers see".

## The fast path runs before the lock

Hosts launch in parallel and during long syncs, so taking `.mars/sync.lock` first would turn every
no-op into `busy` and a fallback read. The unlocked check reads `.mars/provided.lock` and
`mars.lock`, and both are only ever published by tmp + rename, so it sees a whole file either way.
`FileLock::try_acquire` reports held-ness separately from failure; treating every error as
contention would silently disable the refresh on a broken lock file.

`up-to-date` means **a re-plan would do nothing**, and both paths decide it from the same
`reconcile::Plan` over the files on disk, which is the only definition that keeps them from
drifting. So the fast path checks the source is what the record applied (`version`, `digest`) and
that every source skill — not only the recorded ones — is either claimed by `mars.lock` or recorded
and installed with a tree that hashes to its source digest. Hashing is not redundant with existence:
a sync that replaced a recorded directory with a package's copy and died before publishing
`mars.lock` leaves foreign bytes under a recorded name, which only a content check can see. And
checking unrecorded source skills is what lets a skill left out earlier (shadowed by a package, or
collided with someone else's directory) install once its blocker is gone; the record's own
consistency proves nothing about the names it omits. Hashing a host's few small skills costs
milliseconds, against a host launch measured in hundreds.

A locked run whose plan also writes nothing returns `up-to-date` without either `provided.lock`
write, so a lingering shadow or collision does not rewrite the file on every launch. Shadowed names
and collisions are state of the disk, not of the run: both are reported on every status while they
last, and the source's schema warnings are plain-text output only for `applied` (JSON keeps them on
every status).

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
gitignored, so nothing committed carries a provider, a path or a version. A corrupt file is an
empty record with a warning (adoption recovers what the disk can prove); an unknown version is an
empty record that `mars provide` refuses and a reader ignores, because a newer Mars wrote it. A
recorded name is validated before use as a single path component **and** as one of that table's own
`<skill>__<provider>` names: the file is written by another build, and a name that belongs to
another provider would otherwise let a record point a removal at that provider's directory.

## What readers see

`names.rs` is the readers' view: `sync::load_config` loads `ProvidedNames` once the sync lock is
held, and nothing else writes or re-reads the file during a sync. A reader never fails because of
the record, so a plain run without the host neither deletes provided skills nor trips on them: an
absent, corrupt or newer-than-this-Mars file reads as "nothing provided" (one
`provided-lock-unusable` warning for the last two). Only `mars provide`, the writer, refuses a
newer record. A read error that is not a missing file still fails the sync.

Two facts come out of it, and each has one consumer. `provides(provider, skill)` feeds
`validate::classify_unresolved_ref`, the one classifier behind sync's `provided-skill-*` /
`missing-skill` reporting, `mars check` and the resolver's deferral; the resolver and `check` pass
no provided names, so a dependency ref's validity depends on the requesting package alone and never
on what a machine has installed. `owns(dest)` feeds `sync::ownership::Ownership`, the one ownership
predicate: a destination is Mars-owned if `mars.lock` claims it or a provider installed it, so a
package item (or a `.mars-src/` item) replaces a provided directory instead of colliding, on every
machine. The next `mars provide` sees the claim in `mars.lock` and drops its record (`shadowed`).

Availability is the disk's and ownership is the record's. `provides` answers true only while
`.mars/skills/<name>` is a directory, decided once in `ProvidedNames::load`, because a recorded name
with no directory is a leftover and a ref it "satisfies" is a ref an agent cannot load. `owns` takes
the full record instead: owning an absent path is harmless, and a crash between a package's replace
and the `mars.lock` write must still find the directory counted as Mars's.

A sync that replaces a provided directory journals nothing (`sync/recovery.rs` journals only absent
destinations), so a crash between the replace and the `mars.lock` write leaves the package's bytes
under a recorded name. That is safe: `mars provide` hashes recorded trees and replaces them with the
provider's copy, and the next sync replaces them again; a sync that runs first finds the same bytes
already in place. Either order converges on the lock an uninterrupted sync writes.

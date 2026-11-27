# The user level

The user level holds the agents and skills you install for yourself rather than for one
project. Mars treats it as a root of its own, with a manifest, a lock and a store, but it has
no targets. A user-level sync writes only its store and its lock and never touches `~/.claude`,
`~/.codex` or any other harness folder.

## Where its files are

| File | What it is |
|---|---|
| `<config>/mars.toml` | The user level's manifest: a project `mars.toml` (`[dependencies]`, `[local-dependencies]`, `[settings]`, …) with no `targets` |
| `<config>/mars.lock` | Its lock, in the same format as a project's |
| `<config>/.mars-src/` | Your own items, written by hand, as in a project's `.mars-src/` |
| `<config>/config.toml` | Unchanged: the settings layer beneath every root, the user level included ([User config](../config/mars-toml.md#user-config)) |
| `<data>/` | The store: what a project keeps in `.mars/` |

`<config>` is `MARS_CONFIG_DIR`, else `$XDG_CONFIG_HOME/mars`, else `~/.config/mars`.
`<data>` is `MARS_DATA_DIR`, else `$XDG_DATA_HOME/mars`, else `~/.local/share/mars`. The same
rule holds on Linux and macOS. A relative `XDG_*` or `HOME` is ignored; a relative `MARS_*_DIR`
is taken from the working directory.

No `init` step exists. With no `mars.toml` the user level is empty, and `mars --user add`
creates the file. A `targets` or `managed_root` key in the user `mars.toml` or `mars.local.toml`
is an error.

## Choosing the root

- `--user` uses the user level. It cannot be combined with `--root`.
- Commands that only read installed items (`build launch-bundle`, `agents`, `skills`,
  `models` other than `models alias`, `list`, `why`) use the nearest project, and the user level when no project
  encloses the working directory.
- Commands that write (`sync`, `add`, `remove`, `upgrade`, `models alias`, …) use the nearest project. With
  none they fail and name `--user`: a write to the user level is always asked for.
- `--user` works with `sync`, `add`, `remove`, `upgrade`, `outdated`, `list`, `why`,
  `rename`, `override`, `adopt`, `repair`, `doctor`, `validate`, `check`, `agents`, `skills`,
  `models`, `resolve`, `export` and `build launch-bundle`. It is refused, with the reason, by
  `init`, `link`, `unlink` (the user level has no targets), `provide` (a host provides to
  projects) and `version` and `build plugin` (they author a package).

The user level never restricts routing: it has no `targets`, so every harness is allowed.

Every `--json` output that is an object carries the root it used:

```json
{"root": {"kind": "project", "path": "/home/me/repo"}}
{"root": {"kind": "user", "path": "/home/me/.config/mars",
          "config_dir": "/home/me/.config/mars", "store_dir": "/home/me/.local/share/mars"}}
```

`mars list --json` and `mars outdated --json` print a top-level array, which has no place for
the key; their shape is unchanged.

## The home directory is never a project

The walk-up for `mars.toml` skips your home directory, including when it starts there, so
`cd ~ && mars sync` does not open `~/mars.toml`. `mars init` and the automatic init of
`mars add` and `mars link` refuse the home directory and name `--user`.

### Moving a home project

Before the user level, a `~/mars.toml` was how people installed for themselves. Mars no longer
opens it by walk-up, and warns whenever the walk-up passes one. To move it:

1. Copy `~/mars.toml` to `<config>/mars.toml` and delete its `targets` line.
2. Move `~/.mars-src/`, if you have one, to `<config>/.mars-src/`. A path dependency written
   relative to `~` must be rewritten relative to `<config>`, or as an absolute path.
3. Run `mars --user sync`.
4. Run `mars --root ~ unlink <target>` for each target the old file named, which removes what
   Mars installed there.
5. Delete `~/mars.toml`, `~/mars.lock` and `~/.mars/`.

For this minor release, `--root` naming the home directory exactly still opens it as a project,
with the same warning, so step 4 works and an older Meridian that passes `--root ~` keeps
working. The next minor release refuses it.

## `--fallback-root`

`mars agents`, `mars skills` and `mars build launch-bundle` still accept `--fallback-root`, a
second project consulted for items the root lacks, and warn that it is deprecated. It goes in
the next minor release: install those items at the user level instead.

# Config contracts

A local agent overlay replaces the project entry's scalar fields, even when the local entry only adds permissions. Permission sets compose separately, so an omitted local set preserves the project set. Repeat any project scalar that the local entry must retain.

Target configuration controls both materialization and harness permission. Known harness targets grant permission; generic targets and paths do not. An explicitly supplied empty scope denies all harnesses, while absent targets are unrestricted. Provenance records the winning field and file even when two layers have equal values.

Use the shared routing-settings conversion so models commands and build policy interpret harness names and diagnostics identically.

The user folders follow the XDG rule on Linux and macOS (`platform/config.rs`): `MARS_CONFIG_DIR`, else `$XDG_CONFIG_HOME/mars`, else `~/.config/mars`, and the data folder `MARS_DATA_DIR`, else `$XDG_DATA_HOME/mars`, else `~/.local/share/mars`; Windows keeps `dirs`. Mars 0.40 and earlier read macOS config from `~/Library/Application Support/mars/config.toml`. For one minor release a macOS user whose new path has no `config.toml` and whose old one does still gets it read, with the `user-config-moved` warning. After that minor, delete `user_config_file`'s fallback, `UserConfig::moved_to` and the warning. `curated.toml` (the user tier of `mars models list` curation) takes the same fallback, and `mars models list` prints the warning.

A relative `HOME` or `XDG_*` is ignored, as the XDG spec says, because it would name a different folder from every working directory; a relative `MARS_*_DIR` resolves against the working directory, since the user typed it. With no absolute home and no variable set, the user level and `config.toml` cannot be found and commands that read them fail asking for `MARS_CONFIG_DIR`.

The user level's manifest (`<config>/mars.toml`) is read and saved by the project code; its one rule is that `targets` and `managed_root` are errors in it and in its `mars.local.toml` (`refuse_user_targets`), since the user level has no targets and so never restricts routing. An absent user manifest loads as an empty `Config`, so nothing needs creating before `mars --user add`.

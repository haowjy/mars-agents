# src/config/

Agent overlays replace keyed entries except network and tool permission sets,
which compose. A local block that adds only a permission set still replaces
project scalars; repeat any project scalar the local entry needs.

An explicitly empty target list permits no harness; absent targets and
`managed_root` permit discovery. Generic and path targets do not grant harness
permission. Both model commands and build policy use `routing_settings::resolve`
so they report the same invalid configuration.

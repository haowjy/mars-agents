# src/compiler/

A hook or token-using MCP server gets a target config entry only after its
directory was installed for that target. Otherwise the entry could point to
unmanaged or stale content.

[MCP and reconciliation contracts](.context/CONTEXT.md)

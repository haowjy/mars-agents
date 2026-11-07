# Config contracts

A local agent overlay replaces the project entry's scalar fields, even when the local entry only adds permissions. Permission sets compose separately, so an omitted local set preserves the project set. Repeat any project scalar that the local entry must retain.

Target configuration controls both materialization and harness permission. Known harness targets grant permission; generic targets and paths do not. An explicitly supplied empty scope denies all harnesses, while absent targets and managed root are unrestricted. `targets` takes precedence over `managed_root` after layering; provenance records the winning field and file even when two layers have equal values.

Use the shared routing-settings conversion so models commands and build policy interpret harness names and diagnostics identically.

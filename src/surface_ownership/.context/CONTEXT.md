# Surface Ownership Lifecycle

Path ownership is scoped by `(target_root, dest_path)` and carried by an
`OutputRecord`. Its lifecycle state determines what the record proves:

| State | Claim | Permitted use |
|---|---|---|
| `Installed { installed_checksum }` | Mars-installed bytes are at the path | Content comparison, overwrite, and deletion authority |
| `PendingDeletion` | A prior deletion was not confirmed | Retry deletion only |

Deletion authority and replacement authority are distinct powers:

- **Either** lifecycle state authorizes deletion of the recorded path.
- **Only** `Installed` authorizes overwriting an existing path without
  `--force`, because the checksum asserts what content mars put there.
- `PendingDeletion` at an existing path is an unmanaged collision:
  mars preserves the local content and reports the collision. `--force`
  explicitly adopts that path back to `Installed`.

A failed removal converts the retained output to `PendingDeletion`; it does not
carry forward a stale checksum. A successful write converts the same path back
to `Installed`. An already-absent path confirms deletion and must not produce a
pending record.

Config-entry ownership is separate: it is structural ownership of an emitted
JSON entry, not ownership of the containing file path.

## Deferred interrupted-write recovery

A pre-write state is intentionally not part of this lifecycle change. Safe
recovery would require publishing intent before output, a second atomic lock
write, transaction ordering across every output surface, and a fault-injection
test at the write/finalize boundary. That pipeline transaction belongs in a
separate lane; adding an enum variant without those semantics would grant unsafe
adoption authority.

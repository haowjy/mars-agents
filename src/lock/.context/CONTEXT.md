# Lock ownership

Output identity is `(target_root, dest_path)`. Native targets can emit different
bytes at the same relative path, so canonical diff needs the `.mars` checksum.
A pending deletion records authority to retry deletion, not installed bytes.

Canonical ownership combines the lock with `provided.lock`; see
[lock-file ownership](../../../docs/internals/lock-file.md).

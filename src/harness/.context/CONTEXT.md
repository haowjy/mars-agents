# Harness runtime descriptors

`mars-schema` owns harness identity; the runtime registry is the one row per harness (binary, target directory, display name, class), and owns executable descriptors, provider affinity and candidate order. Facts derivable from `HarnessId` (name, skill variant key) are not stored. Lowering policy in `src/compiler` is a match on `HarnessId`, so `src/harness` never depends on the compiler. Keep those authorities shared so callers do not disagree about valid harnesses or routing order.

A capability session retains the probe outcome and its observation together. Re-reading the cache while formatting the same command could pair a decision with a different refresh's timestamp or error. Windows `.cmd` and `.bat` executable lookup belongs to the platform resolver; do not duplicate its suffix handling.

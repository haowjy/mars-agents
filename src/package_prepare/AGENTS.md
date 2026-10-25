# Package preparation

This module owns package classification before staging. A consumer supplies an authored root and an owned scratch staging root; relative dependency resolution always remains anchored at the authored root.

Preparation selects definitions through discovery, upgrades legacy bytes or lifts foreign bytes, and runs the schema readers while writing only scratch space. Consumers may report the returned diagnostics, but mutating commands must reject errors before publishing scratch trees or changing project state. `sync.lock` is the sole pre-gate project write.

Do not add caller-specific classification, legacy scans, or semantic frontmatter reads. Extend the prepared result instead.

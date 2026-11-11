---
name: runner
description: Runs things
model-policies:
- match:
    model: x
  override:
    sandbox: read-only
- match:
    model: y
  override: {}
---
Body

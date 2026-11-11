---
name: runner
description: Runs things
model-policies:
  - match: { model: x }
    override: { effort: 3, sandbox: read-only }
  - match: { model: y }
    override: { approval: bogus, network: 5 }
---
Body

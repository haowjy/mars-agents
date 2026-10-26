---
name: n
description: A Claude agent whose network keys are partly invalid
network-allow:
- good.example
network-deny:
- blocked.example
harness-overrides:
  claude:
    network: nope
---
Body.

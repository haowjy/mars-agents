---
name: n
description: A Claude agent whose network keys are partly invalid
harness-overrides:
  claude:
    network: nope
    network-allow:
    - good.example
    - https://bad.example
    network-deny:
    - blocked.example
    - bad host
---
Body.

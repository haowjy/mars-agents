---
name: demo
description: Demo
harness-overrides:
  meridian-flow:
    other: 1
    permission: read
  claude:
    x: 1
  meridian-cli:
    capabilities:
      spawn: true
    resident-rearm-budget: 4
---
body

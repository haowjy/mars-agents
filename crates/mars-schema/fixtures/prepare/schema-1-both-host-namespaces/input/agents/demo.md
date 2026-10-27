---
name: demo
description: Demo
harness-overrides:
  meridian:
    capabilities:
      spawn: false
      other: false
    resident-rearm-budget: 4
  meridian-cli:
    capabilities:
      spawn: true
      extra: true
    resident-rearm-budget: 2
  flow:
    permission: read
  meridian-flow:
    permission: edit
---
body

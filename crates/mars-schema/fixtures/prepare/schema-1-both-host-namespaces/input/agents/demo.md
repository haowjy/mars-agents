---
name: demo
description: Demo
harness-overrides:
  meridian:
    capabilities:
      spawn: false
    resident-rearm-budget: 4
  meridian-cli:
    resident-rearm-budget: 2
  flow:
    permission: read
  meridian-flow:
    permission: edit
---
body

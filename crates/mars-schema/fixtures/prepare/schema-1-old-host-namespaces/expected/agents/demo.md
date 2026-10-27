---
name: demo
description: Demo
harness-overrides:
  meridian-flow:   # host block
    permission: read
  claude:
    x: 1   # kept
  meridian-cli:
    capabilities:
      spawn: true
---
body

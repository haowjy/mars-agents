---
name: g
description: A Claude agent whose policy keys are partly invalid
tools:
  Read: allow
  Write: deny
harness-overrides:
  claude:
    permission:
      edit: deny
---
Body.

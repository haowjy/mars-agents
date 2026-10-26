---
name: reviewer
description: Reviews code and never writes
tools:
  write: deny
harness-overrides:
  opencode:
    permission:
      edit: deny
---
Review the diff.

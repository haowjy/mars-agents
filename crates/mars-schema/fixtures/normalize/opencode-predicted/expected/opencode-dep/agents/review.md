---
name: review
description: Reviews code and never writes
tools:
  write: deny
  edit: deny
harness-overrides:
  opencode:
    temperature: 0.1
    permission:
      edit: deny
---
Review the diff.

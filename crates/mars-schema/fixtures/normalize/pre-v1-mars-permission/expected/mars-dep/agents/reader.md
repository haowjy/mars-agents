---
name: reader
description: Reads and searches only
tools:
  '*': deny
  Read: allow
  Grep: allow
harness-overrides:
  flow:
    permission: read
---
Read the code.

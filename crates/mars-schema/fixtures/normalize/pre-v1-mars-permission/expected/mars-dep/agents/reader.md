---
name: reader
description: Reads and searches only
tools:
  '*': deny
  Read: allow
  Grep: allow
harness-overrides:
  meridian-flow:
    permission: read
---
Read the code.

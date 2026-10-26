---
name: reader
description: A Claude agent that may only read
tools:
  '*': deny
  Read: allow
  Grep: allow
  Glob: allow
---
Read the code.

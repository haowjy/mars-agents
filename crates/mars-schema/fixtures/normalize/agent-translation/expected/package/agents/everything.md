---
name: everything
description: Agent
tools:
  '*': deny
  Read: allow
  Grep: allow
  Bash(git status *): allow
  agent: allow
  mcp(github/get_issue): allow
  mcp(github): allow
  mcp(docs): allow
  Write: deny
  Bash(rm *): deny
  agent(untrusted): deny
x-custom: 1
effort: high
---
Body

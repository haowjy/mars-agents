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
effort: high
x-custom: 1
harness-overrides:
  claude:
    color: purple
    initialPrompt: /go
    maxTurns: 4
---
Body

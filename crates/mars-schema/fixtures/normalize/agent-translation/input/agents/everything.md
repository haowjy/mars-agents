---
name: everything
description: Agent
tools: Read, Grep, Bash(git status *), Task, mcp__github__get_issue
disallowedTools: Write, Bash(rm *), Agent(untrusted)
mcpServers:
  - github
  - docs:
      command: docs-server
model: inherit
color: purple
initialPrompt: /go
maxTurns: 4
effort: high
x-custom: 1
---
Body

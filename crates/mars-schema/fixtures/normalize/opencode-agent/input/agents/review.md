---
description: Reviews a change without editing it
mode: subagent
model: anthropic/claude-sonnet-4
temperature: 0.1
tools:
  write: false
  edit: false
  bash: false
  read: true
permission:
  edit: deny
  bash:
    "git push": ask
    "*": allow
---
Review the diff and report findings.

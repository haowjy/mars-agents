---
description: Reviews a change without editing it
mode: subagent
model: anthropic/claude-sonnet-4
tools:
  write: deny
  edit: deny
  bash: deny
  read: allow
harness-overrides:
  opencode:
    temperature: 0.1
    permission:
      edit: deny
      bash:
        git push: ask
        '*': allow
---
Review the diff and report findings.

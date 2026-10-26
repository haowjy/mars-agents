---
name: everything
description: Does it all. When asked about everything.
user-invocable: false
license: MIT
compatibility: any
metadata:
  team: core
  type: reference
  version: '1.4'
x-custom: kept
model-invocable: false
harness-overrides:
  claude:
    argument-hint: '[file]'
    model: sonnet
    hooks:
      PreToolUse: []
---
Use ${CLAUDE_SKILL_DIR}/scripts/go.sh.

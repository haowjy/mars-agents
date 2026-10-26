---
name: everything
description: Does it all.
when_to_use: When asked about everything.
disable-model-invocation: true
user-invocable: false
license: MIT
compatibility: any
metadata:
  team: core
type: reference
version: 1.4
allowed-tools: Bash(git status *) Bash(git add *) Read
disallowed-tools: WebFetch
argument-hint: "[file]"
model: sonnet
hooks:
  PreToolUse: []
x-custom: kept
---
Use ${CLAUDE_SKILL_DIR}/scripts/go.sh.

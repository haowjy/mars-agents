---
name: demo
model-policies:
  - match: {alias: fast}
    override:
      effort: low
      network: allow
      autocompact: true
      autocompact_pct: 80
      harness: claude
---

---
model-policies:
  - match: {alias: opus}
    override: {network: false}
  - match: {alias: sonnet}
    override: {network: default}
  - match: {alias: haiku}
    override: {network-allow: [example.com]}
  - match: {model: gpt-6}
    override: {network-deny: [example.com]}
---
body
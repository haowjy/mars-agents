---
z: 1
tools:
  write: allow
  "*": deny
  bash: ask
harness-overrides:
  flow:
    zeta: {b: 1, a: 2}
    alpha: [{m: 1, k: 2}]
  claude:
    model: x
x-last:
  second: 2
  first: 1
---
body

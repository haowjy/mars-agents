---
model-policies:
  - match: {alias: a}
    when: "one line"
  - match: {alias: b}
    when: ""
  - match: {alias: c}
    when: "two\nlines"
  - match: {alias: d}
    when: "trailing\n"
---
body

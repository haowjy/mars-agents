# src/cli/

Under `--json`, stdout carries exactly one JSON document. The output helpers send
status messages to stderr, but raw `println!` is not covered: guard it with `json`.

[Follow-ups](.context/TODO)

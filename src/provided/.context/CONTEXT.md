# Provided skills

The unlocked fast path checks every source skill and hashes installed trees:
recorded names can hold package bytes after an interrupted sync, and an omitted
skill can become installable after its blocker disappears.

A pending provider record covers old and new names before installs begin, so a
crash forces the next run to replan and retains authority to remove dropped
skills. The digest covers bytes and executable bits.

Availability requires the recorded directory to exist; ownership uses the full
record. This lets sync replace provided content safely after an interrupted write
without treating a missing directory as a usable skill.

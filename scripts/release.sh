#!/usr/bin/env bash
set -euo pipefail

cat >&2 <<'EOF'
scripts/release.sh is deprecated.

Mars releases are CI-owned:
  1. Write the changelog entry in the PR body's `## Changelog` section, never in
     CHANGELOG.md, and merge or push normal changes to main.
  2. .github/workflows/release-on-main.yml renders those sections into CHANGELOG.md
     and creates the patch release commit and vX.Y.Z tag.
  3. .github/workflows/release.yml publishes from the tag and uses the rendered
     section as the GitHub Release body.

For emergency/backfill releases, use:
  scripts/manually-release.sh patch --push
  scripts/manually-release.sh patch --notes-file notes.md --push
  scripts/manually-release.sh tag-current --push

Do not manually edit versions or push v* tags outside the helper.
EOF

exit 1

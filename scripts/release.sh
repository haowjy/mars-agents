#!/usr/bin/env bash
set -euo pipefail

cat >&2 <<'EOF'
scripts/release.sh is deprecated.

Mars releases are CI-owned:
  1. Merge or push normal changes to main.
  2. .github/workflows/release-on-main.yml creates the patch release commit and vX.Y.Z tag.
  3. .github/workflows/release.yml publishes from the tag.

For emergency/backfill releases, use:
  scripts/manually-release.sh patch --push
  scripts/manually-release.sh tag-current --push

Do not manually edit versions or push v* tags outside the helper.
EOF

exit 1

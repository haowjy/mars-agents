#!/usr/bin/env bash
set -euo pipefail

cat >&2 <<'EOF'
scripts/release.sh is deprecated.

Mars releases are CI-owned:
  1. Merge or push normal changes to main.
  2. .github/workflows/release-on-main.yml creates the patch release commit and vX.Y.Z tag.
  3. .github/workflows/release.yml publishes from the tag.

Do not manually edit versions or push v* tags.
EOF

exit 1

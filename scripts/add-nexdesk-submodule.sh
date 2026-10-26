#!/usr/bin/env bash
# Add NexDesk as a git submodule at integrations/nexdesk (run once, inside the Aujar git repo).
#
#   scripts/add-nexdesk-submodule.sh            # track the default branch
#   scripts/add-nexdesk-submodule.sh v0.2.0     # pin a tag or commit (recommended)
set -euo pipefail

cd "$(dirname "$0")/.."

REPO_URL="https://github.com/4mritGiri/nexdesk.git"
DEST="integrations/nexdesk"
REF="${1:-}"

if ! git rev-parse --is-inside-work-tree >/dev/null 2>&1; then
    echo "error: run this inside the Aujar git repository (git init / git clone first)." >&2
    exit 1
fi

if git ls-files --error-unmatch "$DEST" >/dev/null 2>&1; then
    echo "$DEST is already a submodule; updating it."
    git submodule update --init -- "$DEST"
else
    # An empty placeholder directory would make `git submodule add` refuse.
    rmdir "$DEST" 2>/dev/null || true
    git submodule add "$REPO_URL" "$DEST"
fi

if [ -n "$REF" ]; then
    git -C "$DEST" fetch --tags --quiet
    git -C "$DEST" checkout --quiet "$REF"
fi

git add .gitmodules "$DEST"

echo
echo "NexDesk is at: $(git -C "$DEST" rev-parse --short HEAD)"
echo "Commit with:   git commit -m 'chore: add nexdesk submodule'"
echo "Build it with: scripts/build-nexdesk.sh"

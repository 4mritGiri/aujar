#!/usr/bin/env bash
# Check every commit message in a range against CONTRIBUTING.md (see check-commit-msg.sh):
#
#   scripts/check-pr-commits.sh <base> <head>      # e.g. origin/main HEAD
#
# Run by .github/workflows/commits.yml on pull requests. Merge commits and commits by bots
# (Dependabot's "Bump x" messages) are skipped; fixup!/squash! commits are refused, because
# they must be squashed before merging.
set -euo pipefail

BASE="${1:-}"
HEAD_REF="${2:-}"

if [ -z "$BASE" ] || [ -z "$HEAD_REF" ] || [ "${1:-}" = "-h" ] || [ "${1:-}" = "--help" ]; then
    sed -n '2,9p' "$0" | sed 's/^# \{0,1\}//'
    [ "${1:-}" = "-h" ] || [ "${1:-}" = "--help" ] && exit 0
    exit 2
fi

CHECK="$(cd "$(dirname "$0")" && pwd)/check-commit-msg.sh"
TMP="$(mktemp)"
trap 'rm -f "$TMP"' EXIT

checked=0
skipped=0
failed=0

while read -r sha; do
    [ -n "$sha" ] || continue

    author="$(git show -s --format=%an "$sha")"
    subject="$(git show -s --format=%s "$sha")"

    case "$author" in
        *"[bot]")
            skipped=$((skipped + 1))
            echo "skipped (bot: $author): ${sha:0:9} $subject"
            continue
            ;;
    esac

    git show -s --format=%B "$sha" >"$TMP"
    checked=$((checked + 1))

    if output="$("$CHECK" --strict "$TMP" 2>&1)"; then
        echo "ok: ${sha:0:9} $subject"
    else
        failed=$((failed + 1))
        echo "::error title=Commit message::${sha:0:9} $subject"
        printf '%s\n' "$output" | sed 's/^/    /'
        echo "    commit: ${sha:0:9}"
    fi
done < <(git rev-list --no-merges "$BASE..$HEAD_REF")

echo
echo "$checked commit(s) checked, $skipped skipped, $failed with problems"

if [ "$failed" -gt 0 ]; then
    echo "Fix them with 'git rebase -i $BASE' (reword), keeping the Signed-off-by line (git commit -s)." >&2
    exit 1
fi

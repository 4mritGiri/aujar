#!/usr/bin/env bash
# Check a commit message against the two rules in CONTRIBUTING.md:
#
#   1. Conventional Commits:  <type>[(scope)][!]: <subject>
#        types: feat fix docs style refactor perf test build ci chore revert
#   2. DCO sign-off:          a `Signed-off-by: Name <email>` line  (git commit -s)
#
#   scripts/check-commit-msg.sh <message file>             # what the commit-msg hook runs
#   scripts/check-commit-msg.sh --strict <message file>    # CI: also refuses fixup!/squash!
#
# Merge commits, `Revert "..."` and (outside --strict) fixup!/squash!/amend! commits are not
# checked: git writes those messages, and fixups are squashed before they land. Exit status: 0
# fine, 1 the message breaks a rule, 2 bad usage.
set -euo pipefail

STRICT=0
FILE=""

for arg in "$@"; do
    case "$arg" in
        --strict) STRICT=1 ;;
        -h | --help)
            sed -n '2,13p' "$0" | sed 's/^# \{0,1\}//'
            exit 0
            ;;
        *) FILE="$arg" ;;
    esac
done

if [ -z "$FILE" ] || [ ! -f "$FILE" ]; then
    echo "usage: scripts/check-commit-msg.sh [--strict] <commit message file>" >&2
    exit 2
fi

# What git keeps: no comment lines, and nothing after the "scissors" line that
# `git commit --verbose` adds above the diff.
message="$(sed '/^# -* >8 -*$/,$d' "$FILE" | grep -v '^#' || true)"
subject="$(printf '%s\n' "$message" | sed -n '/./{p;q}')"

# An empty message is git's to refuse.
if [ -z "$subject" ]; then
    exit 0
fi

case "$subject" in
    "Merge "* | "Revert \""*)
        exit 0
        ;;
    "fixup! "* | "squash! "* | "amend! "*)
        if [ "$STRICT" -eq 1 ]; then
            echo "error: '$subject'" >&2
            echo "  A fixup!/squash! commit must be squashed into the commit it fixes before merging:" >&2
            echo "  git rebase -i --autosquash <base>" >&2
            exit 1
        fi
        exit 0
        ;;
esac

types='feat|fix|docs|style|refactor|perf|test|build|ci|chore|revert'
conventional="^($types)(\\([A-Za-z0-9._/-]+\\))?!?: [^ ].*\$"
signoff='^Signed-off-by: .+ <[^<> @]+@[^<> @]+>[[:space:]]*$'

problems=()

if ! [[ "$subject" =~ $conventional ]]; then
    problems+=("the first line is not a Conventional Commit: <type>[(scope)][!]: <subject>
      types: ${types//|/ }
      examples: feat(launcher): add a dictionary lookup
                fix: do not crash on an empty settings file
                refactor(ui)!: rename the settings sections")
fi

if ! grep -Eq "$signoff" <<<"$message"; then
    problems+=("there is no 'Signed-off-by: Your Name <you@example.com>' line (the DCO, see
      https://developercertificate.org/). Add it with:  git commit -s   (or, for the commit you
      just made:  git commit --amend -s --no-edit)")
fi

if [ "${#problems[@]}" -gt 0 ]; then
    echo "error: this commit message breaks CONTRIBUTING.md:" >&2
    echo "  subject: $subject" >&2

    for problem in "${problems[@]}"; do
        echo "  - $problem" >&2
    done

    exit 1
fi

if [ "${#subject}" -gt 72 ]; then
    echo "note: the first line is ${#subject} characters; 72 or fewer reads better in git log." >&2
fi

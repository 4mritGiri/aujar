#!/usr/bin/env bash
# Turn the repository's git hooks (.githooks/) on or off for this clone.
#
#   scripts/install-hooks.sh              # on: git config core.hooksPath .githooks
#   scripts/install-hooks.sh --uninstall  # off
#   scripts/install-hooks.sh --status     # which are active
#   scripts/install-hooks.sh --force      # take over a different core.hooksPath
#
# The hooks (all can be skipped once with --no-verify, or AUJAR_SKIP_HOOKS=1):
#   commit-msg  Conventional Commits and the DCO sign-off (CONTRIBUTING.md)
#   pre-commit  no build output or files over 5 MiB; Rust formatted (cargo fmt --check)
#   pre-push    cargo clippy, the same gate CI runs
set -euo pipefail

cd "$(dirname "$0")/.."

die() {
    echo "error: $*" >&2
    exit 1
}

git rev-parse --is-inside-work-tree >/dev/null 2>&1 ||
    die "this is not a git checkout (a zip download has no .git): nothing to install"

ACTION=install
FORCE=0

for arg in "$@"; do
    case "$arg" in
        --uninstall) ACTION=uninstall ;;
        --status) ACTION=status ;;
        --force) FORCE=1 ;;
        -h | --help)
            sed -n '2,12p' "$0" | sed 's/^# \{0,1\}//'
            exit 0
            ;;
        *) die "unknown option: $arg (try --help)" ;;
    esac
done

current="$(git config --local --get core.hooksPath || true)"

case "$ACTION" in
    status)
        if [ "$current" = ".githooks" ]; then
            echo "git hooks: on (core.hooksPath = .githooks)"
        elif [ -n "$current" ]; then
            echo "git hooks: core.hooksPath is '$current', not this repository's .githooks"
        else
            echo "git hooks: off (scripts/install-hooks.sh turns them on)"
        fi
        ;;
    install)
        if [ -n "$current" ] && [ "$current" != ".githooks" ] && [ "$FORCE" -eq 0 ]; then
            die "core.hooksPath is already '$current' (another hook manager?). Use --force to replace it."
        fi

        chmod +x .githooks/* scripts/check-commit-msg.sh
        git config --local core.hooksPath .githooks

        echo "git hooks: on. Every commit is checked for Conventional Commits and a Signed-off-by"
        echo "line; pre-commit keeps build output out; pre-push runs clippy."
        echo "Tip: 'git commit -s' adds the sign-off. Turn off with scripts/install-hooks.sh --uninstall."
        ;;
    uninstall)
        if [ "$current" = ".githooks" ]; then
            git config --local --unset core.hooksPath
            echo "git hooks: off"
        else
            echo "git hooks were not installed by this script (core.hooksPath='${current}'): left alone"
        fi
        ;;
esac

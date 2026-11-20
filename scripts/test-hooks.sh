#!/usr/bin/env bash
# Test the git hooks and the commit-message check in a throw-away repository: no network, no
# cargo, takes a second.   scripts/test-hooks.sh   (or: just hooks-test)
set -euo pipefail

SRC="$(cd "$(dirname "$0")/.." && pwd)"
WORK="$(mktemp -d /tmp/aujar-hooks-test.XXXXXX)"
trap 'rm -rf "$WORK"' EXIT

passed=0
failed=0

# expect <pass|fail> <description> <command...>
expect() {
    local want="$1" what="$2"
    shift 2

    local got=pass
    "$@" >/dev/null 2>&1 || got=fail

    if [ "$got" = "$want" ]; then
        passed=$((passed + 1))
    else
        failed=$((failed + 1))
        echo "FAILED (wanted $want, got $got): $what"
    fi
}

msg() { printf '%b' "$1" >"$WORK/msg"; }
SO='\n\nSigned-off-by: Ada Lovelace <ada@example.com>\n'
check() { "$SRC/scripts/check-commit-msg.sh" "$@" "$WORK/msg"; }

# ---- the commit message rules ------------------------------------------------------------

msg "feat: add a thing$SO";                  expect pass "plain feat" check
msg "fix(ui)!: break it$SO";                 expect pass "scope and breaking mark" check
msg "docs(dev/setup): words\n\nBody.$SO";    expect pass "scope with a slash and a body" check
msg "chore(deps): bump tokio$SO";            expect pass "chore(deps)" check
msg "add a thing$SO";                        expect fail "no type" check
msg "feat:no space$SO";                      expect fail "no space after the colon" check
msg "Feat: capital$SO";                      expect fail "capitalised type" check
msg "wip: later$SO";                         expect fail "unknown type" check
msg "feat: missing sign-off\n";              expect fail "no sign-off" check
msg "feat: bad\n\nSigned-off-by: Ada <nope>\n"; expect fail "sign-off without an email" check
msg "feat: x\n# ------------------------ >8 ------------------------\nSigned-off-by: Ada <a@b.c>\n"
expect fail "sign-off only below the scissors line" check
msg "feat: c\n# a comment\n\nSigned-off-by: Ada <a@b.c>\n"; expect pass "comment lines are ignored" check
msg "Merge branch 'x' into main\n";          expect pass "merge commit" check
msg 'Revert "feat: x"\n';                    expect pass "revert commit" check
msg "fixup! feat: x\n";                      expect pass "fixup outside strict" check
msg "fixup! feat: x\n";                      expect fail "fixup in strict mode" check --strict
msg "";                                      expect pass "empty message is git's to refuse" check

# ---- the hooks, in a real repository -----------------------------------------------------

REPO="$WORK/repo"
mkdir -p "$REPO"
git -C "$REPO" init -q
git -C "$REPO" config user.name "Test"
git -C "$REPO" config user.email "test@example.com"
git -C "$REPO" config commit.gpgsign false
git -C "$REPO" config core.hooksPath "$SRC/.githooks"

# A commit message file that the commit-msg hook is given.
msg "feat: x$SO"
expect pass "commit-msg hook accepts a good message" "$SRC/.githooks/commit-msg" "$WORK/msg"
msg "x"
expect fail "commit-msg hook rejects a bad message" "$SRC/.githooks/commit-msg" "$WORK/msg"
msg "x"
expect pass "AUJAR_SKIP_HOOKS skips the hook" env AUJAR_SKIP_HOOKS=1 "$SRC/.githooks/commit-msg" "$WORK/msg"

hook() { (cd "$REPO" && "$SRC/.githooks/pre-commit"); }

echo "hello" >"$REPO/notes.txt"
git -C "$REPO" add notes.txt
expect pass "pre-commit accepts an ordinary file" hook

mkdir -p "$REPO/apps/x/target/release"
echo "obj" >"$REPO/apps/x/target/release/a.o"
git -C "$REPO" add -f apps/x/target/release/a.o
expect fail "pre-commit refuses build output in a nested target/" hook
git -C "$REPO" rm -q --cached -f apps/x/target/release/a.o

head -c $((6 * 1024 * 1024)) /dev/zero >"$REPO/big.bin"
git -C "$REPO" add big.bin
expect fail "pre-commit refuses a file over 5 MiB" hook
git -C "$REPO" rm -q --cached -f big.bin

expect pass "pre-commit can be skipped" env AUJAR_SKIP_HOOKS=1 bash -c "cd '$REPO' && '$SRC/.githooks/pre-commit'"

# Whole flow with git itself: the hook sees the real message file.
(cd "$REPO" && git add notes.txt)
expect fail "git commit without sign-off is refused" git -C "$REPO" commit -q -m "feat: no signoff"
expect pass "git commit -s with a Conventional message works" git -C "$REPO" commit -q -s -m "feat: with signoff"
expect fail "git commit -s with a bad subject is refused" git -C "$REPO" commit -q -s --allow-empty -m "bad subject"
expect pass "--no-verify skips the hooks" git -C "$REPO" commit -q --no-verify --allow-empty -m "anything"

# pre-push: deleting a ref pushes nothing, so it must not demand cargo or clippy.
zero="0000000000000000000000000000000000000000"
expect pass "pre-push ignores a branch deletion" bash -c "printf '(delete) $zero refs/heads/x abc\n' | '$SRC/.githooks/pre-push'"

# ---- the installer -----------------------------------------------------------------------

INST="$WORK/clone"
mkdir -p "$INST/.githooks" "$INST/scripts"
cp "$SRC"/.githooks/* "$INST/.githooks/"
cp "$SRC/scripts/install-hooks.sh" "$SRC/scripts/check-commit-msg.sh" "$INST/scripts/"
git -C "$INST" init -q

install() { "$INST/scripts/install-hooks.sh" "$@"; }

expect pass "install turns the hooks on" install
[ "$(git -C "$INST" config --local --get core.hooksPath)" = ".githooks" ] || { failed=$((failed + 1)); echo "FAILED: hooksPath not set"; }
expect pass "install again is harmless" install
expect pass "status runs" install --status
expect pass "uninstall turns them off" install --uninstall
[ -z "$(git -C "$INST" config --local --get core.hooksPath || true)" ] || { failed=$((failed + 1)); echo "FAILED: hooksPath still set"; }

git -C "$INST" config --local core.hooksPath ".husky"
expect fail "install will not replace another hook manager" install
expect pass "uninstall leaves another hook manager alone" install --uninstall
[ "$(git -C "$INST" config --local --get core.hooksPath)" = ".husky" ] || { failed=$((failed + 1)); echo "FAILED: foreign hooksPath was changed"; }
expect pass "--force replaces it" install --force

# ---- the pull-request check (CI) ---------------------------------------------------------

PR="$WORK/pr"
mkdir -p "$PR"
git -C "$PR" init -q
git -C "$PR" config user.name "Test"
git -C "$PR" config user.email "test@example.com"
git -C "$PR" config commit.gpgsign false
git -C "$PR" commit -q --no-verify --allow-empty -m "init"
base="$(git -C "$PR" rev-parse HEAD)"

git -C "$PR" commit -q --no-verify --allow-empty -m "feat: good one" -m "Signed-off-by: Test <test@example.com>"
good="$(git -C "$PR" rev-parse HEAD)"

pr_check() { (cd "$PR" && "$SRC/scripts/check-pr-commits.sh" "$@"); }

expect pass "a range of good commits passes" pr_check "$base" "$good"

git -C "$PR" commit -q --no-verify --allow-empty -m "oops no type"
bad="$(git -C "$PR" rev-parse HEAD)"
expect fail "a range with a bad commit fails" pr_check "$base" "$bad"

git -C "$PR" commit -q --no-verify --allow-empty --author="dependabot[bot] <bot@example.com>" -m "Bump serde from 1 to 2"
bot="$(git -C "$PR" rev-parse HEAD)"
expect pass "a bot commit after the bad one is skipped (range excludes the bad one)" pr_check "$bad" "$bot"

git -C "$PR" commit -q --no-verify --allow-empty -m "fixup! feat: good one"
fix="$(git -C "$PR" rev-parse HEAD)"
expect fail "a fixup! commit is refused in a pull request" pr_check "$bot" "$fix"
expect fail "missing arguments are an error" pr_check

echo
echo "hooks: $passed passed, $failed failed"
[ "$failed" -eq 0 ]

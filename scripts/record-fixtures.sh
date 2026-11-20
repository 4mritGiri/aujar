#!/usr/bin/env bash
# Record real answers of the online lookups (dictionary, weather, stocks, ...) as fixtures that
# the offline mode replays: `just dev-offline`, the daemon test in crates/aujar-daemon/tests.
#
#   scripts/record-fixtures.sh                          # the default set of queries
#   scripts/record-fixtures.sh "define ephemeral" "weather oslo"
#   AUJAR_FIXTURES=/some/dir scripts/record-fixtures.sh  # record somewhere else
#
# It needs the network, once. It starts a throw-away daemon (own socket, own settings and
# state, no global shortcut) with AUJAR_FIXTURES_RECORD=1, asks it each query until the answer
# arrives, and stops it. Each answer is saved as dev/fixtures/<url>.http: the HTTP status on
# the first line and the body after it. Request headers are never saved.
#
# Fixtures are real data from other people's services: commit only small, stable ones (a few
# words, a city, a ticker), not a day's sports scoreboard. Review `git status` before adding.
set -euo pipefail

cd "$(dirname "$0")/.."

FIXTURES="${AUJAR_FIXTURES:-$PWD/dev/fixtures}"

die() {
    echo "error: $*" >&2
    exit 1
}

case "${1:-}" in
    -h | --help)
        sed -n '2,15p' "$0" | sed 's/^# \{0,1\}//'
        exit 0
        ;;
esac

command -v cargo >/dev/null 2>&1 || die "cargo not found; run ./scripts/setup-dev.sh"
command -v curl >/dev/null 2>&1 || die "curl is needed to record real answers"

QUERIES=("$@")

if [ "${#QUERIES[@]}" -eq 0 ]; then
    QUERIES=("define fire" "define serendipity" "weather kathmandu" "stock aapl")
fi

cargo build -q -p aujar-launcher-app

TARGET="$(cargo metadata --format-version 1 --no-deps 2>/dev/null |
    grep -o '"target_directory":"[^"]*"' | head -1 | cut -d'"' -f4)"
AUJAR="${TARGET:-target}/debug/aujar"
[ -x "$AUJAR" ] || die "built binary not found at $AUJAR"

WORK="$(mktemp -d /tmp/aujar-record.XXXXXX)"
SOCKET="$WORK/aujar.sock"
PID=""

cleanup() {
    if [ -n "$PID" ]; then
        kill -TERM "$PID" 2>/dev/null || true
    fi
    rm -rf "$WORK"
}
trap cleanup EXIT

# Settings and state of the throw-away daemon stay out of ~/.config and ~/.local.
as_recorder() {
    env XDG_CONFIG_HOME="$WORK/config" \
        XDG_STATE_HOME="$WORK/state" \
        XDG_CACHE_HOME="$WORK/cache" \
        AUJAR_SOCKET="$SOCKET" \
        AUJAR_NO_HOTKEYS=1 \
        AUJAR_FIXTURES="$FIXTURES" \
        AUJAR_FIXTURES_RECORD=1 \
        "$@"
}

mkdir -p "$FIXTURES"

(as_recorder "$AUJAR" daemon </dev/null >"$WORK/daemon.log" 2>&1) &
PID=$!

for _ in $(seq 1 100); do
    [ -S "$SOCKET" ] && break
    kill -0 "$PID" 2>/dev/null || die "the daemon exited at startup; see $WORK/daemon.log"
    sleep 0.1
done
[ -S "$SOCKET" ] || die "the daemon did not create its socket"

before="$(find "$FIXTURES" -name '*.http' | wc -l)"
failures=0

for query in "${QUERIES[@]}"; do
    printf 'recording: %s ... ' "$query"

    answered=0
    out=""

    # The first answer is the "Looking up ..." placeholder: ask again until it is replaced.
    for _ in $(seq 1 60); do
        out="$(as_recorder "$AUJAR" search "$query" 2>&1 || true)"

        if ! grep -q "Looking up" <<<"$out"; then
            answered=1
            break
        fi

        sleep 0.5
    done

    if [ "$answered" -eq 0 ]; then
        echo "timed out"
        failures=$((failures + 1))
    elif grep -q "t look up" <<<"$out"; then
        echo "the lookup failed:"
        sed 's/^/    /' <<<"$out"
        failures=$((failures + 1))
    else
        echo "ok"
    fi
done

after="$(find "$FIXTURES" -name '*.http' | wc -l)"

echo
echo "fixtures in $FIXTURES: $after ($((after - before)) new)"
find "$FIXTURES" -name '*.http' -newer "$WORK" -printf '  new or updated: %f (%s bytes)\n' 2>/dev/null || true

if [ "$failures" -gt 0 ]; then
    echo "$failures quer$([ "$failures" -eq 1 ] && echo y || echo ies) did not record. Is the network up?" >&2
    exit 1
fi

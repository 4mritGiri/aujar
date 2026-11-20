#!/usr/bin/env bash
# Run a development copy of Aujar that rebuilds and restarts itself when you save a file.
#
#   scripts/dev.sh                 # daemon + UI, rebuilt on every change
#   scripts/dev.sh --no-ui         # daemon only (CLI and Mouse Borders work)
#   scripts/dev.sh --no-watch      # run once, no rebuild on change
#   scripts/dev.sh --isolate       # own settings and state (does not touch ~/.config/aujar)
#   scripts/dev.sh --hotkeys       # also register the global shortcut (stop the installed
#                                  # service first, it owns the shortcut)
#   scripts/dev.sh --ipc-trace     # log every daemon request (kind + timing, never contents)
#   scripts/dev.sh --offline       # answer the online lookups (define, weather, stock, ...) from
#                                  # the recorded files in dev/fixtures instead of the network
#
# It runs on its own socket (default /tmp/aujar-dev.sock), so your installed Aujar keeps working.
# Use the CLI against it from another terminal:
#
#   export AUJAR_SOCKET=/tmp/aujar-dev.sock
#   cargo aujar doctor
#   cargo aujar launcher toggle      # shows the dev UI
#
# Stop with Ctrl+C: both processes and the socket are cleaned up.
set -euo pipefail

cd "$(dirname "$0")/.."

SOCKET="${AUJAR_SOCKET:-/tmp/aujar-dev.sock}"
WATCH=1
UI=1
ISOLATE=0
HOTKEYS=0
IPC_TRACE=0
OFFLINE=0

for arg in "$@"; do
    case "$arg" in
        --no-ui) UI=0 ;;
        --no-watch) WATCH=0 ;;
        --isolate) ISOLATE=1 ;;
        --hotkeys) HOTKEYS=1 ;;
        --ipc-trace) IPC_TRACE=1 ;;
        --offline) OFFLINE=1 ;;
        -h | --help)
            sed -n '2,21p' "$0" | sed 's/^# \{0,1\}//'
            exit 0
            ;;
        *)
            echo "unknown option: $arg (try --help)" >&2
            exit 2
            ;;
    esac
done

have() { command -v "$1" >/dev/null 2>&1; }

if ! have cargo; then
    echo "cargo not found. Run ./scripts/setup-dev.sh first." >&2
    exit 1
fi

if [ "$WATCH" -eq 1 ] && ! cargo watch --version >/dev/null 2>&1; then
    echo "cargo-watch is not installed, so changes will not trigger a rebuild."
    echo "Install it with ./scripts/setup-dev.sh (or: cargo install cargo-watch --locked)."
    echo "Continuing without watching."
    echo
    WATCH=0
fi

# ---- environment for both processes ------------------------------------------------------

export AUJAR_SOCKET="$SOCKET"
# Keep the installed daemon's global shortcut; a second registration would fight it.
if [ "$HOTKEYS" -eq 0 ]; then
    export AUJAR_NO_HOTKEYS=1
fi
# `aujar=debug` covers every Aujar crate (daemon, launcher engine, UI, Mouse Borders, ...).
export RUST_LOG="${RUST_LOG:-info,aujar=debug}"
if [ "$IPC_TRACE" -eq 1 ]; then
    export AUJAR_IPC_TRACE=1
fi
if [ "$OFFLINE" -eq 1 ]; then
    # Replay recorded answers; record new ones with ./scripts/record-fixtures.sh.
    export AUJAR_FIXTURES="$PWD/dev/fixtures"
    echo "offline lookups: answers come from $AUJAR_FIXTURES"
fi

if [ "$ISOLATE" -eq 1 ]; then
    DEV_HOME="${AUJAR_DEV_HOME:-/tmp/aujar-dev-home}"
    mkdir -p "$DEV_HOME/config" "$DEV_HOME/state"
    export XDG_CONFIG_HOME="$DEV_HOME/config"
    export XDG_STATE_HOME="$DEV_HOME/state"
    echo "isolated settings: $DEV_HOME (fontconfig also reads XDG_CONFIG_HOME, so UI fonts may differ)"
fi

# ---- process management ------------------------------------------------------------------

# Job control gives every background job its own process group, so stopping one stops
# cargo-watch together with the program it started.
set -m
PIDS=()

cleanup() {
    trap - EXIT INT TERM
    echo
    echo "stopping..."
    local pid
    for pid in "${PIDS[@]}"; do
        kill -TERM -- "-$pid" 2>/dev/null || true
    done
    for pid in "${PIDS[@]}"; do
        wait "$pid" 2>/dev/null || true
    done
    rm -f "$SOCKET"
}
trap cleanup EXIT INT TERM

# Run "cargo <args>" once, or under cargo-watch re-running it on changes in the given paths.
# usage: launch <label> <watch paths, space separated> <cargo args...>
launch() {
    local label="$1" paths="$2"
    shift 2

    if [ "$WATCH" -eq 1 ]; then
        local watch_args=() path
        for path in $paths; do
            watch_args+=(-w "$path")
        done
        cargo watch -q "${watch_args[@]}" -x "$*" &
    else
        # shellcheck disable=SC2086
        cargo $* &
    fi
    PIDS+=("$!")
    echo "started $label (pid $!)"
}

# ---- go ----------------------------------------------------------------------------------

if [ -S "$SOCKET" ]; then
    echo "$SOCKET exists. If no dev daemon is running, remove it: rm $SOCKET" >&2
    exit 1
fi

echo "dev socket: $SOCKET"
echo "logs:       RUST_LOG=$RUST_LOG"
[ "$WATCH" -eq 1 ] && echo "watching:   changes rebuild and restart" || echo "watching:   off"
echo

launch daemon "crates modules apps/aujar-launcher Cargo.toml" "run -p aujar-launcher-app -- daemon"

if [ "$UI" -eq 1 ]; then
    # The UI is a heavy build; it waits for the daemon's socket instead of failing to connect.
    for _ in $(seq 1 600); do
        [ -S "$SOCKET" ] && break
        sleep 0.5
    done
    if [ ! -S "$SOCKET" ]; then
        echo "the daemon did not come up in time; see its output above" >&2
        exit 1
    fi
    launch ui "crates modules apps/aujar-ui Cargo.toml" "run -p aujar-ui"
fi

echo
echo "ready. In another terminal:  export AUJAR_SOCKET=$SOCKET && cargo aujar launcher toggle"
echo "Ctrl+C to stop."

# Return as soon as any of them exits (a crash, or Ctrl+C reaching the foreground group).
wait -n 2>/dev/null || true

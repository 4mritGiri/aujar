#!/usr/bin/env bash
# Build NexDesk from the submodule. NexDesk has its own Cargo workspace, toolchain file and
# patched IronRDP (vendor/), so it is built inside its own directory, never with Aujar's cargo.
#
#   scripts/build-nexdesk.sh                 # build only
#   scripts/build-nexdesk.sh --install       # also install to ~/.local/bin (beside aujar)
set -euo pipefail

cd "$(dirname "$0")/.."

NEXDESK_DIR="integrations/nexdesk"

if [ ! -f "$NEXDESK_DIR/Cargo.toml" ]; then
    echo "NexDesk sources are missing. Run:  git submodule update --init $NEXDESK_DIR" >&2
    echo "(or scripts/add-nexdesk-submodule.sh if the submodule was never added)." >&2
    exit 1
fi

(
    cd "$NEXDESK_DIR"
    # nexdesk (GPUI manager) + nexdesk-rdp (isolated RDP engine). The manager looks for the
    # engine next to its own executable, so the two must always be installed together.
    cargo build --release -p nexdesk -p nexdesk-rdp
)

if [ "${1:-}" = "--install" ]; then
    install -Dm755 "$NEXDESK_DIR/target/release/nexdesk" "$HOME/.local/bin/nexdesk"
    install -Dm755 "$NEXDESK_DIR/target/release/nexdesk-rdp" "$HOME/.local/bin/nexdesk-rdp"
    echo "Installed nexdesk and nexdesk-rdp to ~/.local/bin"
    echo "Check with:  aujar nexdesk status"
fi

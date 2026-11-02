#!/usr/bin/env bash
# Build NexDesk from the submodule. NexDesk has its own Cargo workspace, toolchain file and
# patched IronRDP (vendor/), so it is built inside its own directory, never with Aujar's cargo.
#
#   scripts/build-nexdesk.sh                 # build only
#   scripts/build-nexdesk.sh --install       # also install to ~/.local/bin (beside aujar)
#   scripts/build-nexdesk.sh --deb           # also build NexDesk's own .deb into ./dist
#   (the options can be combined)
set -euo pipefail

cd "$(dirname "$0")/.."

NEXDESK_DIR="integrations/nexdesk"
INSTALL=0
DEB=0

for arg in "$@"; do
    case "$arg" in
        --install) INSTALL=1 ;;
        --deb) DEB=1 ;;
        -h | --help)
            sed -n '2,8p' "$0" | sed 's/^# \{0,1\}//'
            exit 0
            ;;
        *)
            echo "unknown option: $arg (try --help)" >&2
            exit 2
            ;;
    esac
done

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

if [ "$INSTALL" = 1 ]; then
    install -Dm755 "$NEXDESK_DIR/target/release/nexdesk" "$HOME/.local/bin/nexdesk"
    install -Dm755 "$NEXDESK_DIR/target/release/nexdesk-rdp" "$HOME/.local/bin/nexdesk-rdp"
    echo "Installed nexdesk and nexdesk-rdp to ~/.local/bin"
    echo "Check with:  aujar nexdesk status"
fi

if [ "$DEB" = 1 ]; then
    mkdir -p dist
    # NexDesk's own packaging script: dpkg-deb only, from the binaries built above.
    (cd "$NEXDESK_DIR" && OUT_DIR="$OLDPWD/dist" packaging/make-deb.sh)
fi

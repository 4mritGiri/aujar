#!/usr/bin/env bash
# Build Aujar and install it for the current user only (no root needed).
set -euo pipefail

cd "$(dirname "$0")/.."

cargo build --release -p aujar-launcher-app

install -Dm755 target/release/aujar "$HOME/.local/bin/aujar"
install -Dm644 packaging/desktop/dev.aujar.Aujar.desktop \
    "$HOME/.local/share/applications/dev.aujar.Aujar.desktop"

mkdir -p "$HOME/.config/systemd/user"
sed 's#^ExecStart=.*#ExecStart=%h/.local/bin/aujar daemon#' packaging/systemd/aujar.service \
    > "$HOME/.config/systemd/user/aujar.service"

systemctl --user daemon-reload
systemctl --user enable --now aujar.service

echo "Installed to ~/.local/bin/aujar (make sure ~/.local/bin is on your PATH)."
echo "Daemon status: systemctl --user status aujar"

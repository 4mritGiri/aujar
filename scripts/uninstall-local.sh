#!/usr/bin/env bash
# Remove the per-user install created by scripts/install-local.sh.
set -euo pipefail

systemctl --user disable --now aujar.service 2>/dev/null || true
rm -f "$HOME/.config/systemd/user/aujar.service" \
      "$HOME/.local/bin/aujar" \
      "$HOME/.local/share/applications/dev.aujar.Aujar.desktop"
systemctl --user daemon-reload

echo "Aujar removed."

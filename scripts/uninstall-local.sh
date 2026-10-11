#!/usr/bin/env bash
# Remove the per-user install created by scripts/install-local.sh.
set -euo pipefail

systemctl --user disable --now aujar-ui.service aujar.service 2>/dev/null || true
rm -f "$HOME/.config/systemd/user/aujar.service" \
      "$HOME/.config/systemd/user/aujar-ui.service" \
      "$HOME/.local/bin/aujar" \
      "$HOME/.local/bin/aujar-ui" \
      "$HOME/.local/share/bash-completion/completions/aujar" \
      "$HOME/.local/share/zsh/site-functions/_aujar" \
      "$HOME/.config/fish/completions/aujar.fish" \
      "$HOME/.local/share/applications/dev.aujar.Aujar.desktop"
systemctl --user daemon-reload

echo "Aujar removed."

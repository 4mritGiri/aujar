#!/usr/bin/env bash
# Build Aujar and install it for the current user only (no root needed).
set -euo pipefail

cd "$(dirname "$0")/.."

# The launcher UI (aujar-ui, GPUI) is a workspace member but not a default one, so it is
# named explicitly. Without it running, the shortcut has nothing to show ("0 subscribers").
#
# Two separate invocations on purpose: the daemon's `ashpd` uses the tokio runtime while GPUI's
# uses async-io, and building both in one command unifies the features and fails to compile
# ("You can't enable both async-io & tokio features at once").
cargo build --release -p aujar-launcher-app
cargo build --release -p aujar-ui

install -Dm755 target/release/aujar "$HOME/.local/bin/aujar"
install -Dm755 target/release/aujar-ui "$HOME/.local/bin/aujar-ui"
# -C: leave the file (and its timestamp) alone when unchanged, so re-running this script does
# not make GNOME Shell rescan its applications and redraw the dash.
install -C -Dm644 packaging/desktop/dev.aujar.Aujar.desktop \
    "$HOME/.local/share/applications/dev.aujar.Aujar.desktop"

mkdir -p "$HOME/.config/systemd/user"
sed 's#^ExecStart=.*#ExecStart=%h/.local/bin/aujar daemon#' packaging/systemd/aujar.service \
    > "$HOME/.config/systemd/user/aujar.service"

sed 's#^ExecStart=.*#ExecStart=%h/.local/bin/aujar-ui#' packaging/systemd/aujar-ui.service \
    > "$HOME/.config/systemd/user/aujar-ui.service"

# Shell completions (bash picks these up automatically; zsh needs the dir on fpath).
"$HOME/.local/bin/aujar" completions bash | install -Dm644 /dev/stdin "$HOME/.local/share/bash-completion/completions/aujar"
"$HOME/.local/bin/aujar" completions zsh  | install -Dm644 /dev/stdin "$HOME/.local/share/zsh/site-functions/_aujar"
"$HOME/.local/bin/aujar" completions fish | install -Dm644 /dev/stdin "$HOME/.config/fish/completions/aujar.fish"

systemctl --user daemon-reload
systemctl --user enable aujar.service aujar-ui.service

# Stop the UI first, restart the daemon, then start the UI: it never sees a half-restarted daemon.
systemctl --user stop aujar-ui.service
# `restart` (not `enable --now`): a daemon that is already running keeps its old code otherwise.
systemctl --user restart aujar.service

# A UI started by hand (cargo run, a terminal) is invisible to systemctl, and a second UI
# refuses to start next to it. Say so instead of failing quietly.
strays=$(pgrep -u "$(id -u)" -a -x 'aujar-ui' || true)
if [ -n "$strays" ]; then
    echo "warning: aujar-ui is already running outside systemd, so the service cannot start:" >&2
    echo "$strays" >&2
    echo "         stop it (pkill -x aujar-ui) and run: systemctl --user restart aujar-ui" >&2
fi

systemctl --user start aujar-ui.service

echo "Installed to ~/.local/bin/aujar (make sure ~/.local/bin is on your PATH)."
echo "Daemon status: systemctl --user status aujar aujar-ui"
echo "zsh completion: add  fpath=(~/.local/share/zsh/site-functions \$fpath)  before compinit / oh-my-zsh in ~/.zshrc"

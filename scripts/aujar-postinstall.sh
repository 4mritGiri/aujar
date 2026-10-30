#!/usr/bin/env bash
# Post-install script for Aujar .deb package
# Run this after installing the .deb to enable and start services

set -e

echo "Enabling and starting Aujar services..."

# Enable both services
systemctl --user enable --now aujar.service aujar-ui.service

echo "Done! Aujar daemon and launcher UI are now running."
echo "You can use Ctrl+Alt+Space to show the launcher."
echo "Check status with: systemctl --user status aujar aujar-ui"

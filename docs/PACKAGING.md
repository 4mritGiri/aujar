# Packaging

Artifacts installed by every package:

| File | Destination |
|---|---|
| `aujar` binary | `/usr/bin/aujar` |
| `packaging/systemd/aujar.service` | `/usr/lib/systemd/user/aujar.service` |
| `packaging/desktop/aujar.desktop` | `/usr/share/applications/aujar.desktop` |
| `LICENSE` | `/usr/share/licenses/aujar/` |

The systemd unit's `ExecStart` is `%h/.local/bin/aujar daemon` for source installs; system
packages must override it to `/usr/bin/aujar daemon`.

| Format | Status | Plan |
|---|---|---|
| Source tarball + `SHA256SUMS` | ✅ release workflow | |
| `.deb` | 📅 | `cargo-deb` |
| `.rpm` | 📅 | `cargo-generate-rpm` |
| AUR | 📅 | `aujar-git`, `aujar` |
| Flatpak | 📅 | Daemon talks to the host via portals; window management needs `--talk-name` permissions |
| AppImage | 📅 | |
| Nix flake | 📅 | |

Packagers: see [SECURITY](../SECURITY.md) for how releases will be signed.

# Aujar Mouse Borders

This archive contains `aujar-mwb`, the cross-platform Mouse Borders agent. It shares a
keyboard, mouse, clipboard, and files with the other computers in its paired layout.

## Install

### Linux

Place `aujar-mwb` somewhere on your `PATH` (for example `~/.local/bin`) and make it
executable if your archive tool did not preserve permissions:

```sh
install -Dm755 aujar-mwb ~/.local/bin/aujar-mwb
aujar-mwb autostart enable
```

For input capture, the Linux backend requires access to `/dev/input` and `/dev/uinput`.
Run `aujar-mwb doctor` after installation for the exact setup needed on the current
machine.

### macOS

Move `aujar-mwb` to a directory on your `PATH`, such as `/usr/local/bin`, then enable it:

```sh
sudo install -m 755 aujar-mwb /usr/local/bin/aujar-mwb
aujar-mwb autostart enable
```

macOS will ask for Accessibility and Input Monitoring permissions when the agent first
runs. Grant both to the terminal that starts it, or to the installed binary if macOS
offers that choice.

### Windows

Extract the ZIP to a permanent per-user directory, for example
`%LOCALAPPDATA%\Aujar`, then open PowerShell in that directory:

```powershell
.\aujar-mwb.exe autostart enable
```

Windows may show a SmartScreen warning for an unsigned release. Verify the release
checksum before running it. The agent does not need elevation; run it as administrator
only if it must control elevated applications.

## First run

Copy `mouse-borders.example.toml` to `settings.toml` in Aujar's configuration directory if
you want a starting point:

| Platform | Configuration path |
|---|---|
| Linux | `$XDG_CONFIG_HOME/aujar/settings.toml` (normally `~/.config/aujar/settings.toml`) |
| macOS | `~/Library/Application Support/Aujar/settings.toml` |
| Windows | `%APPDATA%\Aujar\settings.toml` |

Then use `aujar-mwb pair` or `aujar-mwb join` to connect computers.
Run `aujar-mwb --help` for all commands and `aujar-mwb doctor` for platform-specific
permission checks.

Each GitHub release includes a `SHA256SUMS` file. Verify the archive before extracting:

```sh
sha256sum -c SHA256SUMS
```

On Windows, use `Get-FileHash .\<archive>.zip -Algorithm SHA256` and compare it with
the value in `SHA256SUMS`.

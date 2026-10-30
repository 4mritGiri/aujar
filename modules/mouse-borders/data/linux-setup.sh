#!/bin/sh
# One-time device access for Aujar Mouse Borders on Linux.
#
# Runs as root, started by `aujar mwb setup` or by the Settings window through pkexec/sudo:
#
#     sh -c "<this script>" aujar-mwb-setup <user name>
#
# It is safe to run again. It does exactly this, and nothing else:
#   1. makes the `uinput` kernel module load now and at every boot,
#   2. lets the `input` group open /dev/uinput (to type and click for other machines),
#   3. puts <user> in the `input` group (to read this machine's keyboard and mouse),
#   4. gives <user> access to the devices that already exist, so no log-out is needed.
#
# To undo it: remove /etc/udev/rules.d/70-aujar-mouse-borders.rules and
# /etc/modules-load.d/aujar-mouse-borders.conf, then `sudo gpasswd -d <user> input`.
set -eu

user="${1:-}"

# The user name ends up in commands run as root: accept only plain account names.
case "$user" in
    '' | -* | *[!A-Za-z0-9._-]*)
        echo "aujar-mwb-setup: invalid user name" >&2
        exit 2
        ;;
esac

if [ "$(id -u)" != 0 ]; then
    echo "aujar-mwb-setup: must run as root" >&2
    exit 2
fi

if ! id -u "$user" >/dev/null 2>&1; then
    echo "aujar-mwb-setup: no such user: $user" >&2
    exit 2
fi

# 1. The uinput module, now and at boot (a package may already ship the boot entry).
if [ ! -e /usr/lib/modules-load.d/aujar-mouse-borders.conf ]; then
    mkdir -p /etc/modules-load.d
    cat > /etc/modules-load.d/aujar-mouse-borders.conf <<'CONF'
# Mouse Borders injects input through the uinput kernel module.
uinput
CONF
fi

modprobe uinput 2>/dev/null || echo "aujar-mwb-setup: could not load the uinput module" >&2

# 2. The `input` group may open /dev/uinput (a package may already ship the rule).
if ! getent group input >/dev/null 2>&1; then
    groupadd -r input
fi

if [ ! -e /usr/lib/udev/rules.d/70-aujar-mouse-borders.rules ] \
    && [ ! -e /lib/udev/rules.d/70-aujar-mouse-borders.rules ]; then
    mkdir -p /etc/udev/rules.d
    cat > /etc/udev/rules.d/70-aujar-mouse-borders.rules <<'RULE'
# Let members of the `input` group create virtual keyboards and mice, which Mouse Borders
# uses to inject input from other machines. (Reading /dev/input/event* already needs this
# group; see docs/MOUSE_BORDERS.md#permissions for what that implies.)
KERNEL=="uinput", SUBSYSTEM=="misc", GROUP="input", MODE="0660", OPTIONS+="static_node=uinput"
RULE
fi

if command -v udevadm >/dev/null 2>&1; then
    udevadm control --reload-rules || true
    udevadm trigger --subsystem-match=misc --sysname-match=uinput || true
    udevadm settle --timeout=5 || true
fi

# Where udev did not apply the rule (no udev, or a node created before it existed).
if [ -e /dev/uinput ] && [ "$(stat -c %G /dev/uinput 2>/dev/null)" != input ]; then
    chgrp input /dev/uinput || true
    chmod 0660 /dev/uinput || true
fi

# 3. The user joins `input`. A new group only reaches programs started after the next login.
if ! id -nG "$user" | tr ' ' '\n' | grep -qx input; then
    if command -v usermod >/dev/null 2>&1; then
        usermod -aG input "$user"
    elif command -v gpasswd >/dev/null 2>&1; then
        gpasswd -a "$user" input >/dev/null
    elif command -v adduser >/dev/null 2>&1; then
        adduser "$user" input >/dev/null
    else
        echo "aujar-mwb-setup: no tool to add $user to the input group" >&2
        exit 3
    fi
fi

# 4. So that it works right away: a per-user ACL on the devices that exist now. It ends when
#    the device goes away or at reboot; by then the group membership has taken over.
if command -v setfacl >/dev/null 2>&1; then
    for device in /dev/uinput /dev/input/event*; do
        if [ -e "$device" ]; then
            setfacl -m "u:$user:rw" "$device" 2>/dev/null || true
        fi
    done
fi

echo "aujar-mwb-setup: done"

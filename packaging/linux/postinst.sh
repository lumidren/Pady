#!/bin/sh
# Pady post-install: apply the udev rule now, so the touchpad and the virtual
# input device work without logging out or rebooting.
set -e
if command -v modprobe >/dev/null 2>&1; then
    modprobe uinput 2>/dev/null || true
fi
if command -v udevadm >/dev/null 2>&1; then
    udevadm control --reload-rules 2>/dev/null || true
    udevadm trigger --subsystem-match=input --action=change 2>/dev/null || true
    udevadm trigger --subsystem-match=misc --sysname-match=uinput --action=change 2>/dev/null || true
fi
exit 0

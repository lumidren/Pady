#!/bin/sh
# Pady post-removal: forget the udev rule. Access granted to the current
# session ends at the next sign-in or device change.
if command -v udevadm >/dev/null 2>&1; then
    udevadm control --reload-rules 2>/dev/null || true
fi
exit 0

# Changelog

## Unreleased

### Added
- **Pinch gestures**: `pinch:N:in` and `pinch:N:out` for 3–5 fingers. With `repeat`, a single pinch keeps stepping, so the new default 4-finger pinch zooms smoothly with `ctrl+minus` / `ctrl+equal`.
- **Per-app profiles**: override any tap, hold, swipe or pinch while specific apps are in front, or switch a gesture off there. The profile editor can detect an app when you switch to it, and `pady-cli app` prints app names as you switch.
- **Level overlay**: while volume or brightness changes, a pill near the bottom of the screen under your cursor shows the level. It can't take focus or clicks. Toggle it with `show_overlay`.
- `pady-cli doctor`: a one-screen health check covering config, touchpad, clashing OS gestures, app detection and autostart.
- `pinch_threshold_mm` setting.
- Linux: palm rejection, touchpads plugged in after start are picked up, and per-app profiles on X11/XWayland and KDE Wayland.

### Changed
- Gesture events report which profile a binding came from, and actions report the resulting volume or brightness.
- Windows touchpad conflicts are merged into one warning per finger count.

## 0.1.0

- First version: 3–5 finger taps, holds, swipes (with scrubbing) and three-finger drag, on Windows Precision Touchpads and Linux evdev.
- Actions: keyboard shortcuts, volume, mute, brightness, media keys, links (optionally in a chosen browser), launch or switch to apps, commands and lock.
- Tauri desktop app with a tray icon, live touchpad view, settings and conflict detection, and `pady-cli` for the terminal.

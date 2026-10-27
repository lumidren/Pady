# Changelog

## 0.4.4 (2026-10-27)

### Changed
- The About page ends with a small "Made by" line, with links to the creator's LinkedIn and GitHub.

## 0.4.2 (2026-10-27)

### Fixed
- **A crash during a three-finger drag no longer leaves the mouse button held down** on Windows. Pady lets go of the button before it exits. (On Linux the system already did.)
- The website no longer says Pady works entirely offline: it checks GitHub for updates twice a day, which you can turn off.

## 0.4.1 (2026-10-27)

### Changed
- **Pady starts with your computer by default.** The first launch registers it to start quietly in the tray when you sign in, so gestures are always on. Turning it off in Settings is remembered.

## 0.4.0 (2026-10-27)

### Added
- **Auto-update.** Pady checks GitHub for a new version twice a day and offers it in the tray menu and as a notice in the window; one click downloads it, verifies its signature, installs it and restarts. Windows installs and the AppImage update themselves; .deb/.rpm installs are told to update through the package manager. Turn checking off in Settings.
- **MIT license.**
- The release workflow signs installers through SignPath (free for open source) once the project is set up there, and publishes the files the updater needs.

## 0.3.2 (2026-10-27)

### Changed
- **Volume uses Windows' own slider.** Volume and mute gestures press the real volume keys, so Windows shows its familiar volume flyout, and each step takes well under a millisecond instead of about 13 ms. Pady's own level pill is now off by default on Windows (it stays on by default on Linux, and is still a setting).
- **Volume and brightness swipes glide.** A repeating swipe bound to volume or brightness now steps 2% at a time every couple of millimetres, so the level follows your finger like a slider instead of jumping 4% every 7 mm.

## 0.3.1 (2026-10-25)

### Performance
- **The open settings window no longer keeps the CPU busy while idle.** A card under the mouse pointer replayed its animation forever (about 19% of a CPU core); it now plays twice. The Live page and the tutorial's touchpad draw only while fingers are down, and the tutorial's welcome animation rests after three loops.
- New measuring tools: `cargo bench -p pady-core` (cost per touch frame, gesture → action latency) and `cargo run -p pady-actions --release --example time_actions` (how long volume, mute and brightness take).

### Security and robustness
- Pady's commands now need a permission that only the main window has; the overlay window can only listen for level updates.
- Stricter limits on what devices report: Raw Input records are size-checked before they're read, descriptor ranges no longer overflow, and absurd contact or slot counts are capped (a bogus device could otherwise make Pady grow without limit or exit at start-up).
- Hand-edited settings are clamped to workable ranges and volume/brightness steps to ±100%, and a driver glitch can no longer turn one frame into millions of repeated steps.
- Fuzz tests now feed random device reports, multitouch events, configs, triggers, shortcuts and frames through every parser on each CI run.

## 0.3.0 (2026-10-25)

### Added
- **Double and triple taps** for 3, 4 and 5 fingers (`doubletap:N`, `tripletap:N`). Single taps only wait to be told apart where a multi-tap is bound; the "Double-tap speed" setting controls how long.
- **Guided first-run tutorial.** It checks the touchpad, helps turn off clashing OS gestures, picks a starter preset and lets you practise with live feedback. You can reopen it from About.
- **Starter presets**: Recommended, Like a Mac, Media, Productivity and Minimal, in the tutorial, in Settings and through `pady-cli config preset <id>` / `pady-cli presets`.
- **Pause in full-screen games** (Windows): gestures, drags included, are ignored while a game runs in exclusive full screen.
- **Linux helper tools.** *Fix permissions* also installs missing helpers (brightnessctl, xdotool, wmctrl, playerctl) with the system package manager, and Pady flags bindings that need one.
- A redesigned README with screenshots and an animated demo, plus contributing and security guides, issue and PR templates, and Dependabot.

### Performance
- **The settings window is freed when closed.** In the tray, Pady holds no WebView at all, and the volume/brightness overlay is drawn natively on Windows.
- **No CPU wake-ups while idle.** The engine sleeps until a frame arrives or a timer is due, instead of polling 60 times a second.

## 0.2.0 (2026-10-25)

### Added
- **Installers.**
  - Windows: a per-user setup `.exe` that installs WebView2 if it's missing, with the VC++ runtime linked statically so nothing else is needed, plus an MSI for managed PCs.
  - Linux: `.deb` and `.rpm` packages that declare their libraries and install a udev rule for touchpad and `/dev/uinput` access, plus an AppImage.
  - Uninstalling also removes autostart, and can remove your settings.
- **Fix permissions** (Linux): installs the same udev rule from inside the app (one password prompt), and as `pady-cli fix-permissions`.
- A release pipeline that builds every installer with SHA-256 checksums, and signs Windows builds once a certificate secret is added.
- A log file (`%LOCALAPPDATA%\com.lumidren.pady\logs` on Windows, `~/.local/share/com.lumidren.pady/logs` on Linux), rotated at 1 MB, that also records crashes.
- **Pinch gestures**: `pinch:N:in` and `pinch:N:out` for 3–5 fingers. With `repeat`, a single pinch keeps stepping, so the new default 4-finger pinch zooms smoothly with `ctrl+minus` / `ctrl+equal`.
- **Per-app profiles**: override any tap, hold, swipe or pinch while specific apps are in front, or switch a gesture off there. The profile editor can detect an app when you switch to it, and `pady-cli app` prints app names as you switch.
- **Level overlay**: while volume or brightness changes, a pill near the bottom of the screen under your cursor shows the level. It can't take focus or clicks. Toggle it with `show_overlay`.
- `pady-cli doctor`: a one-screen health check covering config, touchpad, clashing OS gestures, app detection and autostart.
- `pinch_threshold_mm` setting.
- Linux: palm rejection, touchpads plugged in after start are picked up, and per-app profiles on X11/XWayland and KDE Wayland.

### Changed
- Gesture events report which profile a binding came from, including when a profile switches a gesture off ("Off in this app"), and actions report the resulting volume or brightness.
- Quitting Pady in the middle of a drag releases the mouse button instead of leaving it held down.
- `pady-cli.exe` no longer needs the VC++ Redistributable.
- Windows touchpad conflicts are merged into one warning per finger count.

## 0.1.0

- First version: 3–5 finger taps, holds, swipes (with scrubbing) and three-finger drag, on Windows Precision Touchpads and Linux evdev.
- Actions: keyboard shortcuts, volume, mute, brightness, media keys, links (optionally in a chosen browser), launch or switch to apps, commands and lock.
- Tauri desktop app with a tray icon, live touchpad view, settings and conflict detection, and `pady-cli` for the terminal.

# Changelog

## 0.7.1 (2026-10-29)

### Added

- **More animation options.** A Glow switch for every glow (notices, the swipe glow and the app's own highlights), glow strength, animation speed and how long notices stay.

### Fixed

- A touchpad that reports a broken finger position for a moment no longer fires a swipe, pinch or drag, and a swipe keeps working after one.
- Pausing Pady now stops the very next tap or hold too.
- On Windows, apps launched with arguments containing backslashes or quotes get exactly the arguments you wrote.
- A link starting with `-` is refused instead of being handed to the browser as an option.
- On Linux with `pactl`, volume steps stop at 100% instead of over-amplifying.
- The Test button keeps volume and brightness steps in range, like saved gestures.
- Smaller fixes for touches at absurd positions, very long sessions, and strokes too broken to become a glyph.
- The website's live demo no longer shows a warning on load or an old version number, and always loads matching files after an update.

## 0.6.1 (2026-10-29)

### Changed

- **Copy and paste moved to three fingers.** Pinch three fingers together to copy, spread them to paste. If you have the five-finger version from 0.6.0 and three fingers are free, Pady moves it for you once.

## 0.6.0 (2026-10-29)

### Added
- **Copy and paste with one hand.** Close your hand on the touchpad (a five-finger pinch in) to copy, and open it to paste. It's in the Recommended, Like a Mac and Productivity sets; if you set Pady up before, the app offers to add it once.
- **"Pasted" notice.** Pasting with a gesture gets its own glowing corner notice, like "Copied".
- **Swipe glow.** A soft glow in Pady's colours grows at the screen edge a swipe is heading for, and flashes when it fires (only for swipes that do something). Windows.
- **Pinch steps.** A corner notice counts each zoom step while you pinch. Windows.
- **Action notices.** Screenshots, play/pause and track changes get a small corner notice, since there's nothing else to see. Windows.
- **Gesture hints** (off by default): a corner notice names each gesture and what it did, while you learn your gestures. Windows.
- **Show touches** (off by default): glowing fingertips follow your fingers on a small touchpad in the corner, for recording tutorials. Windows.
- **Gesture previews.** On the Gestures page, the cards take turns playing a little loop of the fingers doing them.
- **Live touchpad in Settings.** The Pinch section shows your fingers as you move them.
- **First-time celebrations.** A small burst of confetti the first time each gesture works.
- **An Animations section in Settings**, with one switch for everything and one for each animation.

### Changed
- "Copied" only shows for copies you make in the app in front of you, not when another program (a clipboard manager, cloud clipboard sync, a password manager) changes the clipboard.

### Performance
- The glow and the touch pad are drawn once; each frame only moves them and changes their opacity, paced by the display, at about 0.2 ms of work per frame. With nothing moving, the thread sleeps and costs nothing.

## 0.5.3 (2026-10-28)

### Changed
- **Pinches register the way people actually pinch.** Pady now follows the span of your whole hand (the two fingers furthest apart) instead of the average spread, so pinches with a tilted hand, or where only the thumb moves, count straight away. When a pinch and a swipe are a close call, the pinch wins, and a pinch so fast that the touchpad merges two fingers still counts.
- **Pinch has its own section in Settings**, with a "Try it now" tester that lights up when Pady sees your pinch, and the pinch sensitivity slider.
- **The corner notices glow.** "Copied" and "Gestures paused" are bigger and wrapped in a soft purple aura whose colours turn around the pill; it blooms as the notice lands and breathes while it rests. Every frame waits for the display, so it stays smooth.

## 0.5.2 (2026-10-28)

### Added
- **A "Copied" notice.** A small glass notice slides in at the top-right corner each time you copy something, and fades away a second later. Pady is told *that* you copied, never *what*. Turn it off in Settings.
- **Pady never ignores a gesture silently.** If a gesture is skipped because a full-screen game is in front, a notice in the corner says so.

### Fixed
- **A watchdog keeps gestures alive.** Every two seconds Pady checks its touchpad listener and replaces it with a fresh one after the computer slept, if it ever stopped, or if no touch has arrived for ten minutes while the computer is in use.
- **Gestures keep working after the laptop sleeps.** Windows can keep reporting a full-screen game after waking or unlocking (the lock screen counts), which paused every gesture until another window came to the front. Pady now also checks that the window in front really fills the screen and isn't part of Windows itself.
- After waking from sleep or unlocking, Pady re-registers for touchpad input and clears any touch that was cut off, so the first gesture starts clean.
- The log now says when gestures pause for a full-screen game and when they resume, and which app was in front.

### Performance
- **Gestures stay instant on a busy computer.** On Windows, the touchpad, engine and action threads run at the highest normal priority, so a game, a compile or a video call can't make a gesture wait.
- **Holds, double taps and drag release fire on time.** Windows timers normally tick every ~15.6 ms; Pady asks for 1 ms timers only while one of these is pending and gives them back straight after, so idle battery use is unchanged.

### Security
- **The updater no longer includes the `zip` library**, which had three known vulnerabilities (RUSTSEC-2026-0367, -0368, -0369). Pady's updates are a plain setup `.exe` or AppImage, so the zip support was never used.
- CI checks every dependency against the RustSec advisory database on each push and once a week.

## 0.5.1 (2026-10-27)

### Fixed
- **The first gesture after a long break is as quick as any other** on Windows. Pady no longer lets Windows put it in efficiency mode or page its memory out while it waits, and runs slightly above normal priority. It keeps about 48 MB resident; idle CPU is still zero.

### Added
- **Feedback from the app.** About has *Report a bug*, *Suggest a feature* and *Share feedback* buttons that open a short GitHub form with your Pady version filled in.
- The website and README now warn up front to set Windows' own three- and four-finger gestures to *Nothing*.

## 0.5.0 (2026-10-27)

### Added
- **Browse for an app.** "Launch an app" and "Switch to an app" have a Browse… button that opens a file picker for apps and shortcuts (the Start menu on Windows).
- **Apps by name.** Type an app's name as it appears in Start (like `Antigravity`) and Pady finds its Start menu shortcut. "Switch to an app" also recognises shortcuts.

### Changed
- New motto: *Your touchpad, finally awake.*

## 0.4.6 (2026-10-27)

### Changed
- The About page ends with a small "made by" signature, with links to the creator's LinkedIn and GitHub.

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

<p align="center">
  <img src="crates/pady-app/icons/128x128@2x.png" width="112" alt="Pady icon">
</p>

<h1 align="center">Pady</h1>

<p align="center">
  <b>Your touchpad, your shortcuts.</b><br>
  Map three-, four- and five-finger taps, holds, swipes and drags to anything on Windows and Linux.
</p>

<p align="center">
  <img src="docs/screenshot.png" width="820" alt="Pady's gesture editor">
</p>

---

## What it does

| Gesture | Example action |
|---|---|
| **3-finger drag** | Drag windows and select text without holding the button (the macOS feature Windows never got) |
| **4-finger swipe up / down** | *Scrub* the volume: keep sliding and it keeps stepping |
| **4-finger swipe left / right** | Dim or brighten the screen, also scrubbable |
| **3-finger tap** | Play / pause |
| **4-finger tap** | Open Chrome at a specific link, or bring an app to the front |
| **4-finger hold** | Lock the screen |
| **4-finger pinch in / out** | Zoom out / in (`ctrl+minus` / `ctrl+equal`), stepping as you keep pinching |

Every slot (tap, hold, swipe up/down/left/right, pinch in/out and drag, for 3, 4 or 5 fingers) can run any of these:

- **Keyboard shortcut**, e.g. `ctrl+shift+t`, `win+d`, `alt+f4`, recorded straight from your keyboard
- **Volume** ±N % (exact Core Audio steps on Windows; PipeWire, Pulse or ALSA on Linux) and **mute**
- **Brightness** ±N % (laptop panels via WMI, or `brightnessctl`/sysfs/logind; external monitors via DDC/CI)
- **Media keys** (play/pause, next, previous, stop, plus volume keys that show the OS overlay)
- **Open a link**, in your default browser or a specific one (`chrome`, `firefox`, `edge`, `brave`)
- **Launch an app**, or **switch to it** (focus it if running, minimise it if it's already in front, launch it if not)
- **Run a command**
- **Lock the screen**

On top of that:

- **Per-app profiles.** Give a gesture a different job while Chrome, VS Code or a game is in front, or switch it off there. Everything else falls back to your global bindings.
- **Level overlay.** While you scrub volume or brightness, a small pill near the bottom of the screen shows the level. It can't take focus or clicks, so the app you're using never notices it.
- **Live view.** Watch your fingers on a to-scale touchpad and see every gesture Pady recognises, which action it ran and which profile it came from.

Pady lives in the tray, starts with your computer if you want it to, and applies changes instantly, including when you edit the config file by hand.

## How it works

```text
 touchpad ──▶ pady-input ──TouchFrame (mm)──▶ Recognizer ──Gesture──▶ Engine ──Action──▶ pady-actions ──▶ OS
             Raw Input HID (Windows)                       tap · hold · swipe     bindings   SendInput · Core Audio · WMI
             evdev multitouch (Linux)                      steps · drag stream               uinput · wpctl · logind
```

- **Real finger data, not OS gestures.** On Windows, Pady reads the Precision Touchpad's raw HID reports (every finger's position, contact id and palm flag) through Raw Input, so it keeps working while other apps are focused. On Linux it reads the kernel's multitouch protocol from evdev.
- **Device-independent tuning.** Every report is converted to millimetres from the pad's HID or evdev descriptor, so "swipe 10 mm" feels the same on every laptop.
- **A tested recognizer.** Movement is measured from fingers present in consecutive frames, so a finger landing or lifting never reads as a jump. Taps tolerate fingers landing one by one, swipes can keep stepping, and drags survive a quick re-grip.
- **Actions never block touch.** The engine and the action executor run on separate threads, so a slow app launch can't make the next gesture lag.

The repository is a Cargo workspace:

| Crate | What's in it |
|---|---|
| `pady-core` | Recognizer, config (TOML), triggers, actions, engine. No OS code, fully unit tested |
| `pady-input` | Windows Raw Input HID backend and Linux evdev backend, with protocol logic (`hid.rs`, `mt.rs`) tested on every OS |
| `pady-actions` | Windows and Linux executors, start-with-system, detection of OS gestures that would clash |
| `pady-cli` | `pady-cli monitor` to watch fingers and gestures live, `pady-cli run` to run headless |
| `pady-app` | The Tauri 2 desktop app: tray icon plus a settings window (static HTML/CSS/JS in `ui/`) |

## Install

Download the file for your system from the [latest release](https://github.com/lumidren/Pady/releases/latest):

| System | File | What happens |
|---|---|---|
| **Windows 10 / 11** | `Pady_<version>_x64-setup.exe` | Installs for your user, no admin rights needed. It adds Pady to the Start menu and installs Microsoft WebView2 if it's missing. Nothing else is required. |
| Windows (managed PCs) | `Pady_<version>_x64_en-US.msi` | Per-machine install for Intune / Group Policy. |
| **Ubuntu, Debian, Mint** | `Pady_<version>_amd64.deb` | `sudo apt install ./Pady_*_amd64.deb` installs the libraries Pady needs and sets up touchpad access. |
| **Fedora, openSUSE** | `Pady-<version>-1.x86_64.rpm` | `sudo dnf install ./Pady-*.x86_64.rpm`, same as the .deb. |
| Any Linux | `Pady_<version>_amd64.AppImage` | `chmod +x` and run it. On first launch, click **Fix permissions** (one password prompt). |

Pady starts in the tray. Turn on **Start with Windows/Linux** in Settings to have it start when you sign in. To uninstall, use *Settings › Apps* on Windows (tick "Delete the application data" to remove your settings too) or your package manager on Linux.

> **Windows SmartScreen:** until the installer is code-signed, Windows may say "Windows protected your PC". Click **More info › Run anyway**. The setup and its checksums come from this repository's release page.

## Build from source

You need [Rust](https://rustup.rs) (1.82 or newer).

- **Windows:** you also need the *Desktop development with C++* workload from Visual Studio Build Tools. WebView2 ships with Windows 11.
- **Linux:** install the Tauri system libraries:
  ```bash
  sudo apt install libwebkit2gtk-4.1-dev libayatana-appindicator3-dev librsvg2-dev libxdo-dev libssl-dev
  ```

```bash
git clone https://github.com/lumidren/Pady
cd Pady
cargo run -p pady-app --release        # the desktop app
cargo run -p pady-cli -- monitor       # or: watch your fingers in the terminal
```

To build the installers yourself, run this inside `crates/pady-app`:

```bash
npx @tauri-apps/cli@2 build --bundles nsis,msi        # Windows
npx @tauri-apps/cli@2 build --bundles deb,rpm,appimage # Linux
```

[RELEASING.md](RELEASING.md) covers tagging a release, the CI pipeline and code signing.

## Setup

### Windows

Windows keeps running its own touchpad gestures alongside Pady; apps can read touchpad input but can't block it. For every finger count you rebind, open **Settings › Bluetooth & devices › Touchpad** and set *Three-finger gestures* and/or *Four-finger gestures* to **Nothing**. Pady detects when this is still needed and shows a banner with a button that opens the right page.

Five-finger gestures have no Windows equivalent, so they never clash.

### Linux

Pady needs to read your touchpad and to send keys and pointer motion through `/dev/uinput`. The .deb and .rpm packages install a udev rule ([`packaging/linux/70-pady.rules`](packaging/linux/70-pady.rules)) that grants this to whoever is signed in at the computer. No group changes or re-login are needed. With the AppImage or a source build, click **Fix permissions** in Pady, or run `pady-cli fix-permissions`, to install the same rule.

GNOME uses three-finger swipes and KDE Plasma uses four-finger swipes for their own gestures; Pady warns you when your bindings overlap them. Switching to an app (`focus_or_launch`) works on X11 and XWayland apps through `wmctrl`; on native Wayland it falls back to launching.

## Configuration

Everything you set in the window is saved to a readable TOML file:

- Windows: `%APPDATA%\Pady\config.toml`
- Linux: `~/.config/pady/config.toml`

```toml
[settings]
enabled = true
tap_max_ms = 220          # a tap must lift within this…
tap_slop_mm = 4.0         # …and not wander further than this
swipe_threshold_mm = 10.0 # travel before a swipe or drag starts
swipe_step_mm = 7.0       # spacing between repeats while sliding
hold_ms = 600
drag_speed = 9.0          # cursor pixels per mm while dragging
drag_acceleration = 1.0
drag_release_ms = 350     # lift and re-place within this to keep dragging

[[binding]]
trigger = "drag:3"
action = { type = "drag" }

[[binding]]
trigger = "swipe:4:up"
action = { type = "volume", step = 4 }
repeat = true

[[binding]]
trigger = "tap:4"
action = { type = "open_url", url = "https://youtube.com", browser = "chrome" }

[[binding]]
trigger = "hold:3"
action = { type = "focus_or_launch", program = "code" }
```

**Triggers:** `tap:N`, `hold:N`, `swipe:N:up|down|left|right`, `pinch:N:in|out` and `drag:N`, where N is 3 to 5. Swipes and pinches with `repeat = true` keep firing every `swipe_step_mm` while the fingers keep moving. A `drag:N` binding takes over all movement with N fingers, so swipes with that finger count are disabled. The app flags them.

**Actions:** `none`, `keys{combo}`, `volume{step}`, `mute`, `brightness{step}`, `media{key}`, `open_url{url, browser?}`, `launch{program, args?}`, `focus_or_launch{program, args?}`, `run{command}`, `lock` and `drag`.

### Per-app profiles

A profile overrides global bindings, trigger by trigger, while one of its apps is in front. App names match the executable, ignoring case, folder and `.exe`. A binding with `type = "none"` switches that gesture off in those apps.

```toml
[[profile]]
name = "Browser"
apps = ["chrome.exe", "msedge.exe", "firefox"]

[[profile.binding]]
trigger = "swipe:4:left"
action = { type = "keys", combo = "ctrl+shift+tab" }   # previous tab

[[profile.binding]]
trigger = "swipe:4:right"
action = { type = "keys", combo = "ctrl+tab" }         # next tab

[[profile.binding]]
trigger = "tap:4"
action = { type = "none" }                             # off in the browser
```

You don't need to know exe names: in the profile editor, click **Detect app** and switch to the app, or run `pady-cli app` and switch between apps to see their names.

Drag can't be overridden per app: Pady has to decide whether moving fingers drag before it knows where the gesture is going. On Linux, finding the app in front works on X11/XWayland (`xdotool` or `xprop`) and on KDE Wayland with `kdotool`.

## Command line

```text
pady-cli devices          List detected touchpads
pady-cli monitor          Show fingers and recognised gestures live (no actions run)
pady-cli run              Run Pady headless with your config
pady-cli action <toml>    Run one action now: pady-cli action '{ type = "volume", step = 5 }'
pady-cli app              Print the app in front as you switch (names for profiles)
pady-cli doctor           Check config, touchpad, clashing OS gestures and permissions
pady-cli fix-permissions  Linux: install the udev rule Pady needs (asks for a password)
pady-cli config show      Print the active config
```

## Development

```bash
cargo test --workspace                 # recognizer, config, HID and multitouch protocol tests
cargo clippy --workspace --all-targets
node scripts/make-icons.mjs            # regenerate the app icons
```

To preview the UI in a browser without the backend, open `crates/pady-app/ui/index.html` directly. A built-in mock backend simulates a touchpad with moving fingers.

## Roadmap

- Rotate gestures and diagonal swipes
- Edge swipes (start a swipe from the touchpad's edge for a separate set of bindings)
- Double taps and tap-then-drag combinations
- Exclusive mode on Linux (grab the touchpad and re-emit pointer motion) to fully replace desktop gestures
- Pause automatically while a full-screen game is in front
- Import and export of configs, and shareable profile presets
- Signed installers and auto-update

# Contributing to Pady

Thanks for your interest! Bug reports, gesture ideas and pull requests are all welcome.

## Getting set up

1. Install [Rust](https://rustup.rs) 1.82 or newer.
   - **Windows:** Visual Studio Build Tools with the *Desktop development with C++* workload.
   - **Linux:** `sudo apt install libwebkit2gtk-4.1-dev libayatana-appindicator3-dev librsvg2-dev libxdo-dev libssl-dev`
2. Clone the repository and run the app:
   ```bash
   git clone https://github.com/lumidren/Pady
   cd Pady
   cargo run -p pady-app
   ```
3. Watch what the recognizer sees, without running any actions:
   ```bash
   cargo run -p pady-cli -- monitor
   ```

## Where things live

| Path | What it is |
|---|---|
| `crates/pady-core` | Gesture recognizer, config, presets and engine. No OS code; this is where most logic and tests live. |
| `crates/pady-input` | Touchpad backends: Windows Raw Input (`windows.rs`, `hid.rs`) and Linux evdev (`linux.rs`, `mt.rs`). |
| `crates/pady-actions` | What actions do on each OS, plus conflict detection, autostart and permissions. |
| `crates/pady-overlay` | The native Windows volume/brightness overlay. |
| `crates/pady-cli` | The `pady-cli` terminal tool. |
| `crates/pady-app` | The Tauri app: backend in `src/main.rs`, UI in `ui/` (plain HTML/CSS/JS, no build step). |
| `packaging/linux` | The udev rule and package scripts for .deb/.rpm. |

## Working on the UI

Open `crates/pady-app/ui/index.html` directly in a browser. Without the Tauri backend, `mock.js` provides a fake one with a simulated touchpad, so every page works. These query parameters show specific states:

| Parameter | Shows |
|---|---|
| `?platform=linux` | Linux labels and presets |
| `?onboarding` | The first-run tutorial |
| `?nodevice` | No touchpad found |
| `?nopermission` | A Linux machine without touchpad access |
| `?noconflicts`, `?noprofiles` | A clean configuration |
| `?theme=light` | The light theme |
| `?still` | No simulated gestures, for still screenshots |

To refresh the README screenshots after UI changes, run `node scripts/screenshots.mjs`. It drives headless Chrome or Edge against the mock and writes 2× PNGs to `docs/screenshots`.

## Before you open a pull request

```bash
cargo fmt --all
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
```

CI runs the same checks on Windows and Ubuntu, and `node --check` on the UI scripts.

- **Commits** follow [Conventional Commits](https://www.conventionalcommits.org) (`feat(core): …`, `fix(app): …`). Keep each commit focused.
- **Comments** explain *why*, not what.
- **New behaviour** should come with a test in `pady-core` where possible. The recognizer tests show how to script finger movements.

## Adding an action

1. Add the variant to `Action` in `crates/pady-core/src/action.rs`, with a label.
2. Implement it in `crates/pady-actions/src/windows.rs` and `linux.rs`.
3. Teach the UI about it in `crates/pady-app/ui/model.js` (catalogue and fields) and `mock.js`.
4. Document it in the README's action table.

## Reporting bugs

Use the bug report template and include the output of `pady-cli doctor`. It covers the touchpad, permissions and clashing OS gestures in one go.

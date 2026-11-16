# Pady threat model

Scope: the `main` branch of [lumidren/Pady](https://github.com/lumidren/Pady) at v1.1.1 (commit `c17f9d3`). Written 2026-11-16 as the first step of the assessment in [REPORT.md](REPORT.md).

## What Pady is

Pady is a desktop app for Windows 10/11 and Linux that turns three-, four- and five-finger touchpad gestures (and recorded two-finger glyphs) into actions: keyboard shortcuts, volume and brightness, media keys, opening links, launching programs, running shell commands, locking the session and dragging.

| Part | Where | Stack |
|---|---|---|
| Gesture engine | `crates/pady-core` | Rust, no OS calls: recognizer, config, backups, glyph matcher |
| Touchpad input | `crates/pady-input` | Raw Input + HID parser (Windows), evdev (Linux) |
| Action executors | `crates/pady-actions` | `SendInput`, `ShellExecuteW`, WMI, registry (Windows); uinput and helper programs (Linux) |
| On-screen overlay | `crates/pady-overlay` | Native layered windows, GDI+ (Windows) |
| Desktop app | `crates/pady-app` | Tauri 2: a tray process plus a settings window (static HTML/JS in a WebView) |
| Command line | `crates/pady-cli` | Same engine, headless |
| Website | `site/` | Static pages on GitHub Pages, with the app UI as a demo |
| Build and release | `.github/workflows` | CI, draft releases, signed update files, Pages deploy |

Pady has no server, no accounts, no database and no listening network port. So the usual web-application classes (login brute force, sessions, IDOR, SQL injection, CSRF, uploads) have nothing to attach to; the report says so per section instead of padding.

## Assets

| # | Asset | Why it matters |
|---|---|---|
| A1 | Code execution as the signed-in user | Pady's job is to run things: `run`, `launch`, `open_url` and key presses. Whoever controls what Pady runs controls the account. |
| A2 | The config file (`%APPDATA%\Pady\config.toml`, `~/.config/pady/config.toml`) and its backups | It decides what every gesture runs. Its integrity is the user's gesture setup. |
| A3 | The user's input and screen | Pady injects keys and mouse input, and draws on top of everything. On Linux it also opens `/dev/uinput` and touchpad devices to the user. |
| A4 | The update channel | `latest.json` and the installers on GitHub Releases, the updater signing key, the code-signing identity. |
| A5 | The build and release pipeline | GitHub Actions workflows, their secrets, third-party actions, crates and the Tauri CLI from npm. |
| A6 | Availability | A gesture tool that crashes, hangs or holds a mouse button down breaks the whole desktop session. |
| A7 | Privacy | The log file, the clipboard ("Copied" notice), the window in front (profiles), what the update check reveals. |
| A8 | Root on Linux | *Fix permissions* runs a script through `pkexec`. |

## Attackers

| # | Attacker | Can do | Relevant? |
|---|---|---|---|
| T1 | A hostile or broken input device (a USB gadget posing as a touchpad) | Send any HID descriptor and any report bytes | Yes: the only attacker-controlled bytes Pady parses without the user asking |
| T2 | Someone who gets a file onto the machine: a shared config, a file in a folder Pady or `pady-cli` is started from | Supply a config, plant a program next to where Pady runs | Yes |
| T3 | Another program running as the same user | Write the config, send window messages, read the log | Partly: it already has the user's rights, so Pady only has to avoid being a *stepping stone* to more (other users, root, a sandbox escape) |
| T4 | Another local user without admin rights | Write to shared locations (`%ProgramData%`, `/tmp`) | Yes, where Pady reads from shared places |
| T5 | A network attacker (same Wi-Fi, a hostile proxy) | Tamper with the update check and download | Yes |
| T6 | Content shown in the settings window | Strings that come from outside: device names, app names, config text, release notes, error messages | Yes: script in that window can call `test_action` (A1) |
| T7 | A compromised dependency or GitHub Action | Run code in CI or inside the shipped binary | Yes |
| T8 | A contributor sending a pull request from a fork | Change workflows and build scripts in the PR | Yes |
| T9 | A visitor to the website | Craft URLs for the Pages site and the UI demo | Yes, low value: no accounts or data there |
| – | Anonymous internet user, logged-in low-privilege user, malicious tenant | – | No: there is no service to reach |

## Trust boundaries and data flows

```text
 [touchpad / USB gadget] --HID reports--> (F1) pady-input parsers --> frames --> recognizer --> engine
 [config.toml, backups]  --TOML--------> (F2) pady-core config ------------------------------^
 [settings window JS]    --IPC (18 commands)--> (F3) pady-app commands --> config, executors
 [strings from outside]  --state/events--> (F4) settings window DOM
 [engine]                --actions-----> (F5) executors --> SendInput, ShellExecute, cmd /C, sh -c, helper programs
 [GitHub Releases]       --HTTPS + minisign--> (F6) updater --> installer runs
 [repo, PRs, crates, npm, actions] --> (F7) CI and release workflows --> installers, latest.json, Pages
 [pkexec]                --fixed script--> (F8) root: udev rule, modules-load, package install
 [engine, executors]     --text--------> (F9) log file
 [GitHub API]            --JSON--------> (F10) website download buttons
```

Boundaries that matter most: device bytes into the parser (F1), anything into the settings window's DOM (F4, because of F3's `test_action`), the network into the updater (F6), and outside code into the release job that holds the signing keys (F7).

## STRIDE per flow, ranked by risk

Risk is likelihood × impact for Pady's users, before the tests in the report. "Covered by" names the tests in `security/tests` or the review that checks it.

| Rank | Flow | S | T | R | I | D | E | Main threats | Covered by |
|---|---|---|---|---|---|---|---|---|---|
| 1 | F7 CI and release | ● | ● | ● | ● | | ● | A third-party action or npm package reads the updater key or code-signing certificate in the release job; a fork PR reaching secrets; unpinned actions changing under a tag; who may trigger a release | `ci_pipeline.rs`, workflow review |
| 2 | F6 updater | ● | ● | | | ● | ● | A tampered `latest.json` or installer; downgrade to an old signed version; plain-HTTP endpoints | `updater_config.rs`, review of tauri-plugin-updater |
| 3 | F4 → F3 settings window | | ● | | ● | | ● | HTML or script injection through a device name, app name, profile name, error text or release notes, then `test_action` → `run` | `ui_sinks.rs`, CSP tests |
| 4 | F5 executors | ● | ● | | | | ● | A program planted where a bare name resolves (current folder); option injection through links and arguments; shell quoting | `windows_exec.rs`, `links.rs` |
| 5 | F2 config and backups | | ● | ● | | ● | | A torn or half-written config (lost setup); a hostile shared config (huge, deeply nested, absurd values); backups silently stopping | `config_integrity.rs`, `config_limits.rs`, `backups.rs` |
| 6 | F1 device input | ● | | | | ● | ● | Memory-safety bugs in `unsafe` HID parsing; panics; unbounded growth; action floods | `device_fuzz.rs`, existing `fuzzed_*` tests |
| 7 | F8 Linux root helper | | ● | | | | ● | Injection into the `pkexec` script; the udev rule granting more than needed (`/dev/uinput` to every program of the user) | review, existing script test |
| 8 | F9 log | | ● | ● | ● | | | Secrets from commands and links written to the log; forged log lines through newlines in device or app names | `log_hygiene.rs` |
| 9 | F3 IPC surface | ● | | | | | ● | The overlay window or a remote page calling commands; hostile `save_config` payloads | existing capability tests, `config_limits.rs` |
| 10 | F10 website | | ● | | | | | DOM injection from API data; hostile query strings in the demo | `site.rs` |

## Out of scope

- Other machines, networks and services: nothing outside this repository and this laptop was touched.
- Production infrastructure: GitHub itself, the Pages deployment and the published releases were read, never written.
- An attacker who is already administrator or already runs code as the user and wants nothing more than that user's rights.

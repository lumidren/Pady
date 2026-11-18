# Security

## Supported versions

Security fixes go into the newest release only. Pady updates itself, so the newest release is the one to be on.

| Version | Supported |
|---|---|
| The latest release (1.1.x) | Yes |
| Anything older | No: update first |

## Reporting a vulnerability

Please report security issues **privately** through [GitHub's security advisories](https://github.com/lumidren/Pady/security/advisories/new) rather than public issues. Include:

- the Pady version (About page, or `pady-cli doctor`) and your operating system;
- steps to reproduce, or a proof of concept;
- what an attacker gains.

If you attach a log, look through it first: it can name the programs your gestures start and the sites they open.

You'll get a reply as soon as possible, and credit in the release notes if you'd like it. Please give a fix a reasonable time to ship before publishing details.

## What Pady can access, and why

Pady needs some system access to do its job. It keeps that access as narrow as possible:

- **Touchpad input only.** On Windows, Pady registers for the touchpad's Raw Input collection only; it never reads the keyboard. On Linux, the udev rule (`packaging/linux/70-pady.rules`) grants the signed-in user read access to devices tagged as touchpads, not keyboards or mice.
- **Simulated input.** To run keyboard shortcuts and three-finger drag, Pady sends input: through `SendInput` on Windows, and through `/dev/uinput` on Linux, which the same udev rule opens to the signed-in user. On Linux that means any program you run can open `/dev/uinput` too (see "Known limits").
- **Administrator rights on Linux, only when asked.** *Fix permissions* runs a fixed script through `pkexec`. It writes Pady's udev rule and a `modules-load.d` file, reloads udev, and installs missing helper packages with the system package manager. The script lives in `crates/pady-actions/src/linux.rs` (`fix_permissions_script`).
- **Almost no network.** Pady has no telemetry and no accounts. Its only connection is the update check: twice a day it fetches `latest.json` from Pady's GitHub releases (turn it off with **Check for updates** in Settings). Updates are installed only if they're signed with Pady's updater key. Actions you configure yourself, such as opening a link, are the other exception.
- **The clipboard is watched, not read.** For the "Copied" notice Pady is told that the clipboard changed and by which program. It opens the clipboard in one case only, and only if you switch it on (**Settings › General › Selected text in quick notes**, off by default): to start a quick note with the text you had selected, Pady presses Ctrl+C, reads that text, and puts back what was on the clipboard. The text goes into your note and nowhere else.
- **Notes stay on your computer.** Quick notes are plain text in `notes.json` next to the config. They are never sent anywhere; *Export all notes* writes a file where you choose.
- **Local files only.** Settings live in `%APPDATA%\Pady` or `~/.config/pady`, and logs in the app's local data folder. Logs contain gesture and device information, never what you type. A failed action is logged by the program it runs or the site it opens, without arguments, paths or queries.

## What your config can do

`config.toml` decides what every gesture runs, including shell commands. Treat a config someone gives you like a script: read it before you use it. Anything that can write that file can run programs as you, which is true of any program already running as you.

## How Pady is hardened

- **The settings window runs only the app's own files**, under a strict Content Security Policy: scripts from the app itself, no remote content, no plugins, frames, base or form targets. Each of Pady's commands needs a permission that only the main window has; the on-screen overlay can only listen for level updates. Text from outside (device, app and profile names, errors) is always shown as text, never as HTML.
- **Programs are found in Windows' own places.** A program named without a folder (`chrome.exe`) is looked up in App Paths, the Windows folders and the absolute folders on `PATH`, never in the current folder. A link without a scheme is opened as `https://`, never as a file name.
- **The config is written safely.** Every save goes to its own temp file, is flushed to disk and renamed into place, so neither a crash nor a second save at the same moment leaves half a config. Pady keeps the last 50 versions as backups.
- **Limits on what a config may hold.** A config over 4 MB is refused; profiles, apps per profile, names and the length of commands are capped; hand-edited settings are kept in range.
- **What devices report is bounded.** Raw Input records are size-checked before they're read, descriptor ranges are computed without overflow, contact and slot counts are capped, and impossible positions are dropped, so a broken or hostile device can't crash Pady or make it grow without limit. A single jump in finger position can't turn into a flood of repeated actions, and volume or brightness steps that queue up run as one change.
- **Built with Windows' exploit protections.** Address space layout randomization, data execution prevention and (after 1.1.2) Control Flow Guard.
- **One log event per line.** Line breaks and escape codes in names that come from outside are written as visible escapes.
- **A pinned build.** Every GitHub Action is pinned to an exact commit, the Tauri CLI to an exact version, and builds use the lockfile. The updater signing key is given only to the two build steps that sign. Workflows run with a read-only token except the one that creates the draft release, and pull requests from forks never see secrets.
- **Checked on every push:**
  - the attack tests in [`security/`](security/README.md), which prove each past finding stays fixed;
  - fuzz tests (named `fuzzed_*`) that feed random device descriptors and reports, multitouch event streams, configs, triggers, shortcuts and touch frames through every parser;
  - [`cargo audit`](https://rustsec.org) against the RustSec advisory database;
  - gitleaks over the whole history, and CodeQL over the Rust code, the UI scripts and the workflows.

The full assessment, with each finding's proof and fix, is in [security/REPORT.md](security/REPORT.md); the threat model is in [security/THREAT-MODEL.md](security/THREAT-MODEL.md).

## Known limits

- **Linux: `/dev/uinput` is open to your whole session.** The udev rule lets the signed-in user's programs, not just Pady, create a virtual keyboard. That is narrower than joining the `input` group, and on X11 any program can already send input; on Wayland it is an ability programs don't otherwise have. Keeping the device to Pady alone would take a small privileged helper, which Pady doesn't have yet.
- **Windows installers are not code-signed yet.** Until they are, check a download against the SHA-256 sums published with each release. Updates installed by Pady itself are always verified against its updater key.
- **Six crates deep in Tauri's Linux dependencies are flagged unmaintained or unsound** by RustSec (none with a known vulnerability, and not in code paths Pady calls). They'll go when Tauri updates them.

# Security

## Reporting a vulnerability

Please report security issues **privately** through [GitHub's security advisories](https://github.com/lumidren/Pady/security/advisories/new) rather than public issues. Include steps to reproduce and the Pady version. You'll get a reply as soon as possible, and credit in the release notes if you'd like.

## What Pady can access, and why

Pady needs some system access to do its job. It keeps that access as narrow as possible:

- **Touchpad input only.** On Windows, Pady registers for the touchpad's Raw Input collection only; it never reads the keyboard. On Linux, the udev rule (`packaging/linux/70-pady.rules`) grants the signed-in user read access to devices tagged as touchpads, not keyboards or mice.
- **Simulated input.** To run keyboard shortcuts and three-finger drag, Pady sends input: through `SendInput` on Windows, and through `/dev/uinput` on Linux, which the same udev rule opens to the signed-in user.
- **Administrator rights on Linux, only when asked.** *Fix permissions* runs a fixed script through `pkexec`. It writes Pady's udev rule and a `modules-load.d` file, reloads udev, and installs missing helper packages with the system package manager. The script lives in `crates/pady-actions/src/linux.rs` (`fix_permissions_script`).
- **No network.** Pady doesn't talk to the internet. It has no telemetry, no update checks and no accounts. The only exceptions are actions you configure yourself, such as opening a link.
- **Local files only.** Settings live in `%APPDATA%\Pady` or `~/.config/pady`, and logs in the app's local data folder. Logs contain gesture and device information, never what you type.

## How Pady is hardened

- **The settings window runs only the app's own files**, under a strict Content Security Policy (scripts from the app itself, no remote content). Each of Pady's commands needs a permission that only the main window has; the on-screen overlay can only listen for level updates.
- **What devices report is bounded.** Raw Input records are size-checked before they're read, descriptor ranges are computed without overflow, and contact and slot counts are capped, so a broken or hostile device can't crash Pady or make it grow without limit.
- **Hand-edited settings are kept in range**, and a single jump in finger position can't turn into a flood of repeated actions.
- **Fuzz tests** feed random device descriptors and reports, multitouch event streams, configs, triggers, shortcuts and touch frames through every parser on each CI run (`cargo test`, tests named `fuzzed_*`).
- **Dependencies** are checked with [`cargo audit`](https://rustsec.org) against the RustSec advisory database, and Dependabot keeps them current.

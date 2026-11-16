# Pady security and performance assessment

| | |
|---|---|
| Target | [lumidren/Pady](https://github.com/lumidren/Pady), branch `main`, starting at v1.1.1 (`c17f9d3`) |
| Dates | 16–17 November 2026 |
| Kind | Authorized white-box assessment by the project's maintainer: source review, attack tests, fuzzing, load tests |
| Where | One Windows 11 laptop (the test machine) and the project's own GitHub Actions runs. Nothing else was scanned or contacted. |
| Result | 13 findings: 2 high, 8 low, 3 informational. 11 fixed and verified, 2 open (one by design, one upstream). |

## Executive summary

Pady had been hardened before this assessment (a strict window policy, fuzzed parsers, signed updates, audited dependencies), and that work held: no memory-safety bug, no way to inject script into the settings window, no weakness in the updater and no secret in the repository's history was found.

What was found sits one layer out, where Pady meets the operating system, the disk and its own build pipeline:

- **A program planted in the current folder ran in place of the app a gesture names** (PADY-03, high). `launch chrome.exe` let Windows look in the current folder first.
- **The release job trusted too much** (PADY-08, high as a risk, no sign of abuse): actions pinned to movable tags, and the updater signing key readable by every step.
- **The config could be lost or go unprotected**: two saves at once tore the file (PADY-01), one misdated backup switched backups off (PADY-02), and nothing bounded the file's size (PADY-06).
- **Shell commands with quotes never ran as typed on Windows** (PADY-04).
- **The log kept commands and links in full**, and bug reports ask for that log (PADY-05).
- **A fast swipe queued level changes for up to a second after it ended**; a hostile device stretched that to 44 seconds (PADY-10).

All of these are fixed, each with a test in `security/tests` that failed before the fix and passes after. Two items stay open on purpose and are described at the end: how the Linux udev rule shares `/dev/uinput`, and six unmaintained or unsound crates deep in Tauri's Linux dependencies.

Nothing here has been released yet: the fixes are on `main`, after v1.1.1.

## Scope and method

In scope: everything in the repository (six Rust crates, the settings UI, the website, packaging, CI and release workflows) and the threat model in [THREAT-MODEL.md](THREAT-MODEL.md).

How each finding was established:

1. Read the code path from input to effect.
2. Write an attack as an automated test and run it against the unfixed code (red).
3. Fix, then run the same test (green), plus the whole workspace's tests, `cargo clippy` for Windows and Linux, and CI.

Tools: the Rust toolchain already installed (rustc 1.99.1), `cargo audit` 0.22.2, Python and Node for scripts, and in CI gitleaks and CodeQL. No scanner was installed on the machine.

Severity uses CVSS 3.1 base scores. Pady has no network service, so most vectors are local (`AV:L`); scores are low even where the bug is real, and the text says what it means in practice.

## Findings

| ID | Title | Severity | CVSS 3.1 | Status |
|---|---|---|---|---|
| PADY-03 | A program planted in the current folder is started in place of the named app (Windows) | High | 7.0 `AV:L/AC:H/PR:N/UI:R/S:U/C:H/I:H/A:H` | Fixed |
| PADY-08 | Release pipeline: movable action tags, signing key visible to every step, unrestricted CI token | High | 8.1 `AV:N/AC:H/PR:N/UI:N/S:U/C:H/I:H/A:H` | Fixed |
| PADY-01 | Two saves at once leave a torn config file | Low | 3.6 `AV:L/AC:H/PR:L/UI:N/S:U/C:N/I:L/A:L` | Fixed |
| PADY-02 | A backup dated in the future switches backups off | Low | 3.3 `AV:L/AC:L/PR:L/UI:N/S:U/C:N/I:L/A:N` | Fixed |
| PADY-04 | `run` commands with double quotes are mangled (Windows) | Low | 3.3 `AV:L/AC:L/PR:L/UI:N/S:U/C:N/I:N/A:L` | Fixed |
| PADY-05 | Commands and links are logged in full; outside text can forge log lines | Low | 3.6 `AV:L/AC:H/PR:L/UI:N/S:U/C:L/I:L/A:N` | Fixed |
| PADY-06 | No limits on the config file's size or contents | Low | 3.3 `AV:L/AC:L/PR:N/UI:R/S:U/C:N/I:N/A:L` | Fixed |
| PADY-09 | The website builds HTML from GitHub's API answer | Low | 3.1 `AV:N/AC:H/PR:H/UI:R/S:U/C:L/I:L/A:N` | Fixed |
| PADY-10 | Level changes queue up and keep running after the swipe has ended | Low | 2.4 `AV:P/AC:L/PR:N/UI:N/S:U/C:N/I:N/A:L` | Fixed |
| PADY-11 | Linux: the udev rule opens `/dev/uinput` to every program of the signed-in user | Low | 3.3 `AV:L/AC:L/PR:L/UI:N/S:U/C:N/I:L/A:N` | Open (by design) |
| PADY-07 | Window policy lacks `base-uri`, `form-action`, `object-src`, `frame-ancestors` | Info | – | Fixed |
| PADY-12 | Six unmaintained or unsound crates in Tauri's Linux dependencies | Info | – | Open (upstream) |
| PADY-13 | Developer script builds page code with `JSON.stringify` alone | Info | – | Fixed |

### PADY-03: a program planted in the current folder is started in place of the named app

**Severity:** High (7.0). **Status:** fixed in `3762e19`.

**Description.** `launch`, `focus_or_launch` and `open_url` handed bare names such as `chrome.exe` to `ShellExecuteW`. For a name without a folder, Windows looks in the process's *current folder* before App Paths and `PATH`. A file of that name there is what runs. The same held for the "link" of an `open_url` without a scheme: `example.com` is a valid program name on Windows (`.com` files run), and the shell looked for it as a file first.

**Affected code (v1.1.1).** `crates/pady-actions/src/windows.rs:128-141` (the three actions) and `:681` (`shell_open`).

**Proof of concept.** `security/tests/windows_launch.rs`. It copies a harmless system program to a temp folder under a name nothing else has, makes that folder current and runs `launch` with the bare name:

```text
before: `pady-proof-6404.exe` from the current folder was started   (launch, focus_or_launch, open_url, and as "browser")
after:  Windows could not find `pady-proof-…exe` (not an installed app, and not on PATH)
```

**Impact.** Code execution as the user, by whoever can put a file where Pady or `pady-cli` gets started: a downloaded or shared folder, a USB stick, a cloned repository. `pady-cli run` from such a folder is the likeliest case; the installed app usually starts in its own folder or `System32`, which lowers the odds (hence `AC:H`), but a portable copy started from Downloads has that folder as its current one.

**Fix.** Pady resolves bare names itself: App Paths (per user, then machine), the Windows folders, then absolute `PATH` entries, and never the current folder (`locate_program`). A link without a scheme becomes `https://…` and is never looked up as a file (`pady_actions::link`, also applied on Linux, where `xdg-open` has the same habit). Full paths, which are the user's own explicit choice, work as before.

### PADY-08: the release pipeline trusted more than it had to

**Severity:** High as a risk (8.1); nothing suggests it was ever used. **Status:** fixed in `63ae1a7`, scanning added in `f174721`.

**Description.** Three things in `.github/workflows`:

1. Every action was pinned to a tag or branch (`@v4`, `@v2`, `@stable`). Tags can be moved: whoever controls `Swatinem/rust-cache`, `softprops/action-gh-release`, `dtolnay/rust-toolchain` or `signpath/github-action-submit-signing-request` decided what ran next time, in a job with `contents: write`.
2. `TAURI_SIGNING_PRIVATE_KEY` and its password were environment variables of the whole `bundle` job (`release.yml:34-36`), readable by each of its ~15 steps, including those third-party actions and the Tauri CLI fetched from npm at build time.
3. `ci.yml` set no `permissions`, leaving the token's reach to the repository default.

**Impact.** The updater key is what every installed copy of Pady trusts. With it, an attacker who also gets a file onto the releases page (the same job's token can) ships code to every user through auto-update. The precondition is a compromised upstream action or package, which has happened to widely used actions before.

**Fix.** All actions are pinned to full commit ids, with the version in a comment so Dependabot keeps updating them. The key is passed only to the two steps that sign. `ci.yml` has `contents: read`. Checkouts use `persist-credentials: false`. `security/tests/ci_pipeline.rs` fails the build if any of that slips. A new `security.yml` runs gitleaks over the whole history and CodeQL over the Rust code, the UI scripts and the workflows.

**Still worth doing by hand** (repository settings, not code): see "What needs you" at the end.

### PADY-01: two saves at once leave a torn config file

**Severity:** Low (3.6). **Status:** fixed in `d43a43c`, cleanup in `5c0610a`.

**Description.** `Config::save` wrote `config.toml.tmp` and renamed it over the config (`crates/pady-core/src/config.rs:320-328` in v1.1.1). The app saves from several threads (window commands, the tray's on/off switch, first-run setup) and `pady-cli` saves too. All shared that one temp file, so one save could rename it into place while another was still writing it.

**Proof of concept.** `security/tests/config_integrity.rs`: six threads save three different configs for four seconds while a reader checks that the file always holds one whole config.

```text
before: 11 torn reads in 4 s (and saves failing because the temp file had been renamed away)
after:  0 torn reads, 0 failed saves in 293 reads
```

**Impact.** A torn config doesn't parse. At the next start Pady moves it aside as `config.broken.toml` and falls back to defaults: the user's gestures are gone until they restore a backup.

**Fix.** Each save writes its own temp file (`config.toml.<pid>-<n>.tmp`, created exclusively), flushes it to disk, then renames; saves within a process take turns; a rename blocked for a moment by another program is retried; a failed save removes its temp file; leftovers from a killed process are removed once a minute old. `security/tests/config_crash.rs` kills a saving process 25 times at random moments and locks the file under a save: the config reads back whole every time.

### PADY-02: a backup dated in the future switches backups off

**Severity:** Low (3.3). **Status:** fixed in `77234e2`.

**Description.** A backup's id is the time it was taken. To avoid a copy per keystroke, a save is skipped when the newest copy is under two minutes old, computed as `now - newest` with the difference clamped at zero (`crates/pady-core/src/backups.rs:63` in v1.1.1). A newest copy dated *after* now therefore made every ordinary save look too recent, for as long as its date stayed in the future. `newest + 1` also overflowed for the largest id.

**Proof of concept.** `security/tests/backups.rs` plants `config-<a year from now>.toml` and saves: no backup was made (before); with id `u64::MAX` debug builds panicked with "attempt to add with overflow".

**Impact.** Backups silently stop. It takes only a clock that was wrong for a while (a flat CMOS battery, a time-zone slip, dual boot) or anything that can drop a file in the folder. The user finds out when they need a backup.

**Fix.** Copies dated after now no longer count as "the last copy", are pruned first, and ids can't overflow or overwrite an existing copy.

### PADY-04: `run` commands with double quotes are mangled on Windows

**Severity:** Low (3.3). **Status:** fixed in `8889b78`.

**Description.** `run` started `cmd` with the arguments `/C` and the command through Rust's `Command::args` (`windows.rs:142`). Rust quotes arguments for the C runtime's rules, escaping `"` as `\"`. `cmd.exe` doesn't read those rules, so `"C:\Program Files\App\app.exe" --flag` reached it as something else.

**Proof of concept.** `security/tests/windows_run.rs` runs `echo proof> "<folder with a space>\proof file.txt"`: the file never appeared (before). A second test passes quotes, a caret, a percent sign and `&`.

**Impact.** Commands with quoted paths silently did nothing, or ran as something other than what was typed. Not attacker-controlled (the command is the user's own), but a `run` that does something different from what it says is a correctness bug in the most powerful action.

**Fix.** The command goes to `cmd.exe /S /C "<command>"` as a raw argument: with `/S`, `cmd` strips exactly the outer quotes and runs the rest as typed. `cmd.exe` is taken from the system folder instead of being searched for.

### PADY-05: commands and links logged in full; outside text can forge log lines

**Severity:** Low (3.6). **Status:** fixed in `79a5813`.

**Description.** When an action failed, the engine logged its whole label and the executor's error (`crates/pady-core/src/engine.rs:495` in v1.1.1): the full shell command of a `run`, the full address of an `open_url`. Separately, names from outside (a device's product string, a program's name, the text of a broken config) were logged as they came, line breaks and terminal escape codes included.

**Proof of concept.** `security/tests/log_hygiene.rs` binds gestures to a `curl` with a bearer token, a webhook address with a key and a `launch` with a `--token` argument, makes them fail and reads what was logged:

```text
before: Run `curl -H "Authorization: Bearer sk-live-4242424242424242" https://api.example.com/lights/on` failed: …
after:  Run `curl …` failed: cannot run `curl …`: No such file or directory
```

**Impact.** `pady.log` is what the bug-report template asks people to paste into a public issue. Webhook keys and tokens in home-automation gestures would go with it. Forged lines could mislead whoever reads a log.

**Fix.** The log names a command by its program and a link by its site, and scrubs both (and `launch` arguments) from the error text; the settings window still shows everything. The app and `pady-cli` write every message through `pady_core::log_line`, which shows control characters, line separators and bidirectional overrides as escapes.

### PADY-06: no limits on the config file

**Severity:** Low (3.3). **Status:** fixed in `e71f40d`.

**Description.** `Config::load` read the whole file whatever its size (`config.rs:303`), on start-up, two seconds after every change on disk, and once per backup when listing them; backups copied it up to 50 times. Nothing capped the number of profiles, each of which every gesture checks against the app in front.

**Proof of concept.** Measured before the fix: a 64 MB config loads in 87 ms and is copied into a backup in 63 ms (up to 3.2 GB for 50 copies); with 200,000 profiles each gesture waits 26 ms for its profile lookup. `security/tests/config_limits.rs` keeps the limits in place.

**Impact.** A shared "gesture pack", a sync conflict or a script appending to the file can fill the disk with backups and make every gesture slow. It needs the user to accept such a file, hence low.

**Fix.** A config over 4 MB is refused before it is read, never written and never backed up. Normalising caps profiles (200), apps per profile (100), names (200 characters) and drops bindings whose command, link, shortcut or argument list is absurdly long (over 4096 bytes or 64 arguments) rather than cutting them. Profile matching no longer allocates: 4× faster at 200 profiles.

### PADY-09: the website builds HTML from GitHub's API answer

**Severity:** Low (3.1). **Status:** fixed in `fb4b8b3`.

**Description.** `site/site.js` fetched the latest release from GitHub's API and wrote asset names and URLs into the page with `innerHTML` and `insertAdjacentHTML` (lines 12, 41 and 83 in v1.1.1), and assigned the URLs to `href` unchecked.

**Impact.** An asset named with markup, or a `javascript:` URL, would run script on the project's own site. Only someone who can publish release assets could do that (or a wrong API answer), so the risk is low; the site has no accounts to steal.

**Fix.** Links are built as elements with `textContent`, and a URL is used only if it starts with `https://github.com/lumidren/Pady/releases/`. `security/tests/site.rs` checks both, and that the pages load no script from elsewhere.

### PADY-10: level changes queue up and keep running after the swipe has ended

**Severity:** Low (2.4), and a real performance bug. **Status:** fixed in `b9f400e`.

**Description.** A repeating swipe sends one action per step to the action thread, which ran them one at a time. A step costs under a millisecond for volume on Windows, about 5 ms for brightness, and 10–30 ms on Linux, where each step starts a helper program. Steps arrive much faster than that.

**Proof of concept.** `security/tests/action_flood.rs`, with an executor that takes 5 ms per step:

| | Before | After |
|---|---|---|
| A fast 400 mm swipe (about 200 steps) | 195 calls, action thread busy 1.07 s after the lift | 2 calls, 11 ms |
| A device racing its fingers 18 mm per frame for 1,000 frames | 7,993 calls, busy 44 s | 2 calls, 13 ms |
| A slow swipe | 6 calls of +2% | 6 calls of +2% (unchanged) |

**Impact.** The volume or brightness kept moving after the fingers lifted, every later gesture waited behind the backlog, and on Linux each step was another process. A hostile or broken touchpad could keep that up indefinitely.

**Fix.** Repeat steps of the same swipe that are already waiting are taken together, and for volume and brightness they run as one change of their sum (capped at 100%). Other repeating actions still run once per step, as before.

### PADY-11: the udev rule opens `/dev/uinput` to every program of the signed-in user (open)

**Severity:** Low (3.3). **Status:** open by design; documented in `SECURITY.md`.

`packaging/linux/70-pady.rules` tags `/dev/uinput` and touchpad devices with `uaccess`, so the signed-in user's session can open them without joining the `input` group. That is the least-privilege option available without a daemon, and far narrower than the `input` group. It still means any program running as that user can create a virtual keyboard: on X11 that changes nothing (any client can already send input), on Wayland it is a capability programs don't otherwise have. Reviewed, not tested on a Linux machine.

A fix would be a small privileged helper that owns `/dev/uinput` and accepts only Pady's requests. That is a design change for a future release, not a patch.

### PADY-07, PADY-12, PADY-13 (informational)

- **PADY-07.** The window's Content Security Policy had no `base-uri`, `form-action`, `object-src` or `frame-ancestors`. Nothing can inject markup today, so there was no way to use this; all four are now `'none'` (`0918752`), checked by `security/tests/window_policy.rs`.
- **PADY-12.** `cargo audit`: 0 vulnerabilities in 548 crates. Six warnings, all pulled in by Tauri's Linux GTK stack: `proc-macro-error`, `zbus_names`, `zvariant`, `zvariant_derive` (unmaintained), `glib` 0.18.5 and `rustix` 1.1.5 (unsound in functions Pady doesn't call). Nothing to change in this repository; they go away when Tauri moves on.
- **PADY-13.** CodeQL's only two alerts were one line of `scripts/screenshots.mjs`, a developer tool that builds page code from constant selectors with `JSON.stringify`. Not shipped and not exploitable; values now go through an escaping `literal()` (`d9e6149`, `8183657`).

## Tested and found sound

| Area | What was tried | Result |
|---|---|---|
| Script injection into the settings window | Followed every string from outside (device, app, profile and glyph names, errors, release notes, config text) to where it is shown | All go through text nodes. The only three `innerHTML` sinks receive built-in drawings. `window_policy.rs` fails if a new sink appears. |
| Command surface | The overlay window or a remote page calling commands; hostile `save_config` payloads | Only the main window has command permissions; no `remote` access; payloads are normalised (existing tests, extended by `config_limits.rs`). |
| Updater | Endpoints, transport, key, downgrade, betas | HTTPS from the project's releases only; signature checked against a compiled-in key; only newer versions are offered; betas don't self-update. Pinned by `updater_config.rs`. Not tested end to end against a fake server. |
| Device input | Random descriptors and reports, absurd coordinates, NaN, contact-count lies (existing `fuzzed_*` tests); a million random frames through the engine | No panic, no growth. Absurd jumps are dropped. |
| Config parser | 50,000-deep nested arrays and tables; hostile values | An error, never a crash; values clamped. |
| Leaks | 20,000 rounds of the per-gesture system calls; a million frames; a 30-minute soak | Handles and memory flat (`leaks.rs`, `docs/benchmarks.md`). |
| Secrets | gitleaks over the whole history of `main`, in CI | No leaks detected. |
| Fork pull requests | Workflow triggers | No `pull_request_target`; fork PRs get no secrets. |
| Linux root helper | The `pkexec` script | A fixed string with no user input; installs a fixed rule and fixed package names. |
| Clipboard notice | What the "Copied" notice reads | Counts formats and the owner window only; never the contents. |

## Not applicable

Pady has no server, accounts, sessions, database, uploads or listening ports, so these sections of a web assessment have nothing to test: authentication and sessions, access control between users, SQL and template injection, SSRF, file uploads, TLS configuration of a service, CORS and CSRF, Android and iOS, cloud infrastructure.

## Possible but unproven

- **Repeating `run` actions.** PADY-10's fix merges level changes only. A user who binds a repeating swipe to a `run` command still gets one process per step, and a hostile touchpad could produce many steps. A device that hostile could also pose as a keyboard and type commands, so this adds little; it's a robustness limit rather than a hole.
- **Frames faster than the engine can take them.** The queue between the touchpad reader and the engine is unbounded. Fed 23 million frames a second (a real pad sends 125–250, USB allows 8,000), it held 1.4 million frames and 106 MB until it caught up 0.2 s later. No real device can do this.
- **`localhost:8080/x` as a link** reads as scheme `localhost` and is passed through; Windows then says it can't open it. Harmless, mildly confusing.
- **Linux `gtk-launch` fallback.** A `launch` program name starting with `-` would be read by `gtk-launch` as an option. It comes from the user's own config.

## Not tested, and why

- **Linux at run time.** No Linux machine was available. Linux code was reviewed, type-checked and linted with a Linux target, and CI ran the test suite on Ubuntu; the executors, the udev rule and `pkexec` flow were not exercised by hand.
- **A real hostile USB device.** The parsers were fuzzed with bytes; no hardware gadget was plugged in.
- **The updater end to end** (a fake update server, a tampered installer). Its configuration and the plugin's checks were reviewed instead.
- **The installers** (NSIS, MSI, deb, rpm, AppImage) and their uninstall paths.
- **The running app's window** under a debugger. Injection was assessed from the source and the policy, not by attacking a live WebView.
- **Start-up time and memory of the app built from `main`**: that would have meant stopping the copy of Pady in use on the test machine. Frame rates of the settings UI could not be measured either (the test browser was not visible).

## What needs you

1. **Release these fixes** when you're ready (they are on `main` only). Ask and I'll prepare 1.1.2.
2. **Repository settings** I can't and shouldn't change for you:
   - Settings › Actions › General › Workflow permissions: **Read repository contents** as the default.
   - Settings › Branches: protect `main` (require the CI checks; no force pushes).
   - Settings › Code security: turn on **Private vulnerability reporting** (SECURITY.md sends reporters there), **Secret scanning** and **Push protection**.
3. **Dependabot's pull requests** now bump the pinned actions to new major versions. Review and merge them one at a time; the release workflow has no test run, so check the next release's draft carefully after merging.
4. **The updater key** does not need rotating: no leak was found and no run looks tampered with. Rotating it would cut every installed copy off from updates, so only do it on real evidence.
5. **Code signing** for the Windows installer is still pending (SignPath). Until then first-time downloads rely on HTTPS and the published SHA-256 sums.

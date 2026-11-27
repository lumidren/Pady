# Configuration

Everything you set in the window is saved to a readable TOML file. Edit it by hand if you like: Pady notices the change within about two seconds and applies it, with no restart. If the file can't be read, Pady keeps it as `config.broken.toml` and starts with the defaults.

| System | Config file | Logs |
|---|---|---|
| Windows | `%APPDATA%\Pady\config.toml` | `%LOCALAPPDATA%\com.lumidren.pady\logs` |
| Linux | `~/.config/pady/config.toml` | `~/.local/share/com.lumidren.pady/logs` |

`pady-cli config path` prints the location, and **Settings** has a button that opens the folder. Logs are rotated at 1 MB and hold gestures and errors, never what you type.

<details>
<summary><b>An example config</b></summary>

```toml
[settings]
enabled = true
tap_max_ms = 220
swipe_threshold_mm = 10.0
swipe_step_mm = 7.0
hold_ms = 600
pause_in_fullscreen = true
check_updates = true

# All apps
[[binding]]
trigger = "drag:3"
action = { type = "drag" }

[[binding]]
trigger = "tap:3"
action = { type = "media", key = "play_pause" }

[[binding]]
trigger = "swipe:4:up"
action = { type = "volume", step = 4 }
repeat = true                  # keep stepping while the fingers keep sliding

[[binding]]
trigger = "pinch:3:in"
action = { type = "keys", combo = "ctrl+c" }

[[binding]]
trigger = "hold:5"
action = { type = "open_url", url = "https://github.com/lumidren/Pady", browser = "firefox" }

# Only while VS Code is in front
[[profile]]
name = "Code"
apps = ["code"]

[[profile.binding]]
trigger = "swipe:4:left"
action = { type = "keys", combo = "ctrl+pageup" }
```

Settings you leave out take their defaults, and a later binding for the same trigger replaces an earlier one. The top of the file Pady writes lists every trigger and action.
</details>

## Settings

All distances are millimetres on the touchpad surface, so they feel the same on every laptop. Hand-edited values are kept within a workable range.

| Setting | Default | Meaning |
|---|---|---|
| `enabled` | true | Master switch. When off, Pady watches but never acts. |
| `tap_max_ms` | 220 | A tap must lift within this many milliseconds… |
| `tap_slop_mm` | 4 | …and no finger may wander further than this. |
| `multi_tap_ms` | 250 | Longest gap between the taps of a double or triple tap. |
| `hold_ms` | 600 | Resting this long without moving counts as a hold. |
| `swipe_threshold_mm` | 10 | Travel before a swipe or drag starts. |
| `swipe_step_mm` | 7 | Spacing between repeats while sliding. Volume and brightness swipes glide 3.5 times as often, in 2% steps. Pinches step every 14 mm of hand span. |
| `pinch_threshold_mm` | 8 | Change in hand span before a pinch starts. |
| `drag_speed` | 9 | Cursor pixels per millimetre while dragging. |
| `drag_acceleration` | 1 | How much faster quick flicks move the cursor (0 = linear). |
| `drag_release_ms` | 350 | Lift and re-place within this to keep dragging (0 = let go at once). |
| `show_overlay` | false on Windows, true on Linux | The level pill for volume and brightness. |
| `pause_in_fullscreen` | true | Ignore gestures while a game runs in exclusive full screen (Windows). |
| `show_copy_notice` | true | The "Copied" notice (Windows). |
| `note_selection` | false | Start a quick note with the selected text. Pady presses Ctrl+C to read it and puts the clipboard back. |
| `trim_paste` | false | A gesture that pastes (Ctrl+V or Shift+Insert) first removes the spaces, tabs and line breaks around a single copied line (Windows). Text of several lines is left as it is. Pady reads the copied text to do this. |
| `check_updates` | true | Look for a new version on GitHub twice a day. |

The animation and notice switches (`animations`, `anim_*`, `glow_strength`, `anim_speed`, `notice_time`) are easiest to set in **Settings**.

## Backups

Before Pady changes your config, it keeps a copy in a `backups` folder next to it (`config-<time>.toml`), so a slip in the app, a bad import or a broken update can always be undone. **Settings › Backups** lists the copies with how many gestures, profiles and glyphs each holds, and **Restore** brings one back. The current config is copied first, so a restore can itself be undone.

- One copy each time Pady starts, and before every save.
- Identical copies are skipped. While you're busy changing things, only the state from before the burst is kept (at most one copy per two minutes), so a dozen toggles don't push real history out.
- Resetting, applying a preset and restoring always take a copy.
- The newest 50 copies are kept.

## Command line

`pady-cli` is a small terminal companion. It's a separate download on the release page, or build it with `cargo build --release -p pady-cli`.

```text
pady-cli doctor                Check config, touchpad, clashing OS gestures and permissions
pady-cli devices               List detected touchpads
pady-cli monitor               Show fingers and recognised gestures live (no actions run)
pady-cli run                   Run Pady headless with your config (actions run)
pady-cli app                   Print the app in front as you switch (names for profiles)
pady-cli action <toml>         Run one action now: pady-cli action '{ type = "volume", step = 5 }'
pady-cli presets               List starter presets
pady-cli config [path]         Print where the config file is
pady-cli config show           Print the config
pady-cli config reset          Go back to the defaults
pady-cli config preset <id>    Apply a preset
pady-cli fix-permissions       Linux: install the udev rule and helper tools Pady needs
```

Set `RUST_LOG=debug` for more detail.

---

Back to the [README](../README.md).

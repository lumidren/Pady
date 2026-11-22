//! What Pady promises about privacy, held to by the build.
//!
//! README and SECURITY.md say: Pady reads only the touchpad, never the
//! keyboard; it is told *that* something was copied, never what; it opens
//! the clipboard for two things only, a quick note's selected text and
//! trimming a line before a paste, each only when switched on; and its only
//! network connection is the update check.
//! These tests fail if code that would break a promise is ever added.

use pady_security::{read, repo};

/// Every Rust source file in `crates/`, without comment lines.
fn sources() -> Vec<(String, String)> {
    fn walk(dir: &std::path::Path, out: &mut Vec<(String, String)>) {
        for entry in std::fs::read_dir(dir).unwrap().flatten() {
            let path = entry.path();
            if path.is_dir() {
                walk(&path, out);
            } else if path.extension().is_some_and(|x| x == "rs") {
                let text = std::fs::read_to_string(&path).unwrap();
                let code: String =
                    text.lines().filter(|l| !l.trim_start().starts_with("//")).collect::<Vec<_>>().join("\n");
                out.push((path.display().to_string(), code));
            }
        }
    }
    let mut out = Vec::new();
    walk(&repo().join("crates"), &mut out);
    assert!(out.len() > 30, "found the source files");
    out
}

/// The one file that may open the clipboard: it reads the selected text
/// for a quick note (Ctrl+C, read, put everything back) and trims a copied
/// line before a paste.
const CLIPBOARD_READER: &str = "clipboard.rs";

#[test]
fn the_clipboard_is_watched_but_never_read() {
    for (file, code) in sources() {
        if file.replace('\\', "/").ends_with(&format!("pady-actions/src/{CLIPBOARD_READER}")) {
            continue;
        }
        for read in ["GetClipboardData", "OpenClipboard", "EnumClipboardFormats", "GetClipboardFormatName"] {
            assert!(!code.contains(read), "{file} calls {read}: the \"Copied\" notice must not see what was copied");
        }
    }
}

/// The selected text is read in one place, for one feature, behind a
/// switch that is off unless the user turns it on.
#[test]
fn the_clipboard_is_opened_only_for_a_quick_note_and_only_when_asked() {
    let mut callers = Vec::new();
    for (file, code) in sources() {
        let file = file.replace('\\', "/");
        if code.contains("selected_text(") && !file.ends_with("pady-actions/src/clipboard.rs") {
            callers.push(file);
        }
    }
    callers.sort();
    let expected = ["pady-actions/src/lib.rs", "pady-app/src/quick.rs"];
    assert_eq!(callers.len(), expected.len(), "{callers:#?}");
    for (caller, want) in callers.iter().zip(expected) {
        assert!(caller.ends_with(want), "{caller} reads the selected text");
    }
    // In the app, the read sits behind the setting...
    let quick = read("crates/pady-app/src/quick.rs");
    let setting = quick.find("settings.note_selection").expect("the switch is checked");
    let reading = quick.find("pady_actions::selected_text(").expect("the selection is read");
    let early_return = quick[setting..reading].contains("if !capture {") && quick[setting..reading].contains("return;");
    assert!(early_return, "without the switch the gesture must return before reading");
    // ...and the setting is off unless someone turns it on.
    let config = read("crates/pady-core/src/config.rs");
    assert!(config.contains("note_selection: false,"), "note_selection must default to off");
    // What was on the clipboard is put back, and kept out of Windows' history.
    let clipboard = read("crates/pady-actions/src/clipboard.rs");
    assert!(clipboard.contains("let restored = restore(&saved);") && clipboard.contains("keep_out_of_history()"));
}

/// A paste gesture reads the copied text in one place, behind a switch that
/// is off unless the user turns it on, and never writes it anywhere.
#[test]
fn a_paste_reads_the_copied_text_only_when_asked() {
    let mut callers = Vec::new();
    let mut switches = Vec::new();
    for (file, code) in sources() {
        let file = file.replace('\\', "/");
        if code.contains("trim_for_paste(") && !file.ends_with("pady-actions/src/clipboard.rs") {
            callers.push(file.clone());
        }
        if code.contains("set_trim_paste(") && !file.ends_with("pady-actions/src/lib.rs") {
            switches.push(file);
        }
    }
    assert_eq!(callers.len(), 1, "{callers:#?}");
    assert!(callers[0].ends_with("pady-actions/src/windows.rs"), "{callers:#?}");
    // The read sits behind the switch, in the same condition as the paste shortcut...
    let windows = read("crates/pady-actions/src/windows.rs");
    let reading = windows.find("crate::clipboard::trim_for_paste()").expect("the copied line is trimmed");
    let gate = "if combo.is_paste() && crate::trim_paste() {";
    let gated = windows[..reading].rfind(gate).expect("the switch is checked");
    assert!(!windows[gated + gate.len()..reading].contains('}'), "the read must be inside the check");
    // ...only the app's setting turns the switch, and the setting starts off.
    assert_eq!(switches.len(), 1, "{switches:#?}");
    assert!(switches[0].ends_with("pady-app/src/main.rs"), "{switches:#?}");
    assert!(read("crates/pady-app/src/main.rs").contains("pady_actions::set_trim_paste(s.trim_paste);"));
    let actions = read("crates/pady-actions/src/lib.rs");
    assert!(actions.contains("static TRIM_PASTE: AtomicBool = AtomicBool::new(false);"), "off until switched on");
    assert!(read("crates/pady-core/src/config.rs").contains("trim_paste: false,"), "trim_paste must default to off");
    // What was read is compared in memory: never logged, saved or sent.
    let clipboard = read("crates/pady-actions/src/clipboard.rs");
    let tests = clipboard.find("#[cfg(test)]").unwrap_or(clipboard.len());
    for leak in ["log::", "println!", "eprintln!", "std::fs", "File::", "dbg!"] {
        assert!(!clipboard[..tests].contains(leak), "clipboard.rs uses {leak}: the copied text must stay in memory");
    }
    // The trimmed line is kept out of Windows' clipboard history, like Pady's other writes.
    let trim = &clipboard[clipboard.find("pub(crate) fn trim_for_paste").expect("the function")..tests];
    assert!(trim.contains("restore(&with_text(saved, line))"));
}

#[test]
fn only_the_touchpad_is_registered_for_raw_input() {
    let input = read("crates/pady-input/src/windows.rs");
    let registrations = input.matches("RegisterRawInputDevices(").count();
    assert!(registrations >= 1);
    // Every registration is for the digitizer page's touchpad usage.
    assert_eq!(input.matches("usUsagePage: PAGE_DIGITIZER").count(), registrations);
    assert_eq!(input.matches("usUsage: USAGE_TOUCHPAD").count(), registrations);
    for (file, code) in sources() {
        // Keyboard hooks and key-state polling would be a keylogger's tools.
        for hook in ["SetWindowsHookEx", "WH_KEYBOARD", "RIM_TYPEKEYBOARD", "GetKeyboardState("] {
            assert!(!code.contains(hook), "{file} uses {hook}");
        }
    }
    // Linux: the udev rule opens touchpads and the virtual input device, nothing else.
    let rules = read("packaging/linux/70-pady.rules");
    let granted: Vec<&str> =
        rules.lines().filter(|l| !l.trim_start().starts_with('#') && l.contains("uaccess")).collect();
    assert_eq!(granted.len(), 2, "{granted:#?}");
    assert!(granted[0].contains("ID_INPUT_TOUCHPAD") && granted[1].contains("uinput"), "{granted:#?}");
    assert!(!rules.contains("ID_INPUT_KEYBOARD") && !rules.contains("ID_INPUT_MOUSE"));
}

#[test]
fn nothing_but_the_updater_talks_to_the_network() {
    for (file, code) in sources() {
        for net in ["TcpStream", "TcpListener", "UdpSocket", "reqwest::", "ureq::", "hyper::", "tungstenite"] {
            assert!(!code.contains(net), "{file} uses {net}: Pady's only connection is the update check");
        }
    }
    // Direct dependencies that can open connections: the updater plugin only.
    for manifest in ["pady-core", "pady-input", "pady-actions", "pady-overlay", "pady-cli", "pady-app"] {
        let text = read(&format!("crates/{manifest}/Cargo.toml"));
        for line in text.lines().filter(|l| !l.trim_start().starts_with('#')) {
            let name = line.split(['=', ' ', '.']).next().unwrap_or("");
            assert!(
                !["reqwest", "ureq", "hyper", "tokio-tungstenite", "curl", "isahc", "surf"].contains(&name),
                "crates/{manifest} depends on {name}"
            );
        }
    }
    // The settings window can't fetch anything either: its policy allows
    // connections to the app itself only.
    let conf = read("crates/pady-app/tauri.conf.json");
    assert!(conf.contains("default-src 'self' ipc: http://ipc.localhost"));
    assert!(!conf.contains("connect-src"), "a connect-src would widen what the window may contact");
}

//! How Pady starts programs on Windows.
//!
//! PADY-03: a program planted in the *current folder* was started in place
//! of the app a gesture names. `launch`, `focus_or_launch` and `open_url`
//! hand bare names such as `chrome.exe` to `ShellExecuteW`, which looks in
//! the current folder before anywhere else. Whoever can put a file where
//! Pady or `pady-cli` is started from (a downloaded folder, a shared drive,
//! a cloned repository) got it run by the user's next gesture.
#![cfg(windows)]

use pady_core::Action;
use pady_security::Scratch;
use std::path::PathBuf;
use std::sync::Mutex;
use std::time::Duration;

/// The current folder is one per process: tests that change it take turns.
static CWD: Mutex<()> = Mutex::new(());

fn run(action: Action) -> Result<(), String> {
    pady_actions::executor().run(&action).map(|_| ()).map_err(|e| e.to_string())
}

/// A harmless program under a name nothing else on this machine has.
fn plant(dir: &Scratch) -> String {
    let name = format!("pady-proof-{}.exe", std::process::id());
    let system = std::env::var_os("SystemRoot").map(PathBuf::from).expect("SystemRoot");
    std::fs::copy(system.join("System32").join("whoami.exe"), dir.path(&name)).expect("copy whoami.exe");
    name
}

/// Runs `f` with the planted program's folder as the current folder.
fn in_planted_folder(f: impl FnOnce(&str)) {
    let _turn = CWD.lock().unwrap_or_else(|e| e.into_inner());
    let dir = Scratch::new("planted");
    let name = plant(&dir);
    let before = std::env::current_dir().unwrap();
    std::env::set_current_dir(&dir.0).unwrap();
    let outcome = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| f(&name)));
    std::env::set_current_dir(before).unwrap();
    if let Err(panic) = outcome {
        std::panic::resume_unwind(panic);
    }
}

#[test]
fn launch_does_not_start_a_program_planted_in_the_current_folder() {
    in_planted_folder(|name| {
        let result = run(Action::Launch { program: name.into(), args: Vec::new() });
        assert!(result.is_err(), "`{name}` from the current folder was started");
        let result = run(Action::FocusOrLaunch { program: name.into(), args: Vec::new() });
        assert!(result.is_err(), "`{name}` from the current folder was started by focus_or_launch");
    });
}

#[test]
fn open_url_does_not_start_a_program_planted_in_the_current_folder() {
    in_planted_folder(|name| {
        // As the browser to open a link in.
        let result = run(Action::OpenUrl { url: "https://example.invalid/".into(), browser: Some(name.into()) });
        assert!(result.is_err(), "`{name}` was started as a browser");
    });
}

#[test]
fn a_link_is_never_taken_for_a_file_in_the_current_folder() {
    // As the "link" itself: what is handed to Windows is a web address, not
    // a name it would look up in the current folder. (`.com` files run.)
    for name in ["pady-proof.exe", "example.com", "setup.bat", r"..\..\evil.cmd", "notes"] {
        let opened = pady_actions::link(name).unwrap();
        assert_eq!(opened, format!("https://{name}"), "{name:?}");
    }
    assert!(pady_actions::link("--gpu-launcher=calc.exe").is_err(), "an option, not a link");
    for kept in ["https://example.com/", "ms-settings:display", "spotify:track:1", r"C:\Users\me\page.html"] {
        assert_eq!(pady_actions::link(kept).unwrap(), kept);
    }
}

#[test]
fn a_program_named_by_its_full_path_still_starts() {
    let _turn = CWD.lock().unwrap_or_else(|e| e.into_inner());
    let dir = Scratch::new("full-path");
    let name = plant(&dir);
    let full = dir.path(&name).to_string_lossy().into_owned();
    run(Action::Launch { program: full, args: Vec::new() }).expect("a full path is the user's own choice");
    // Let it exit before the folder is removed.
    std::thread::sleep(Duration::from_millis(500));
}

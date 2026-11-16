//! PADY-04: `run` commands containing double quotes were mangled on Windows.
//!
//! The command went to `cmd /C` through Rust's argument quoting, which
//! escapes quotes with backslashes. `cmd` doesn't understand those, so
//! `"C:\Program Files\App\app.exe" --flag`, or any command with a quoted
//! path, never ran as written (and what did run was not what the user typed).
#![cfg(windows)]

use pady_core::Action;
use pady_security::Scratch;
use std::path::PathBuf;
use std::time::{Duration, Instant};

fn run(action: Action) -> Result<(), String> {
    pady_actions::executor().run(&action).map(|_| ()).map_err(|e| e.to_string())
}

fn wait_for(path: &PathBuf) -> Option<String> {
    let start = Instant::now();
    while start.elapsed() < Duration::from_secs(10) {
        if let Ok(text) = std::fs::read_to_string(path) {
            if !text.is_empty() {
                return Some(text);
            }
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    None
}

#[test]
fn run_commands_with_quotes_run_as_written() {
    let dir = Scratch::new("run quotes"); // a space in the path, on purpose
    let out = dir.path("proof file.txt");
    let command = format!("echo proof> \"{}\"", out.display());
    run(Action::Run { command }).expect("cmd starts");
    let text = wait_for(&out);
    assert_eq!(text.as_deref().map(str::trim), Some("proof"), "the quoted path was not understood");
}

#[test]
fn run_commands_keep_their_special_characters() {
    let dir = Scratch::new("run-special");
    let out = dir.path("out.txt");
    // Quotes, a caret, percent signs and an ampersand, as a user would type them.
    let command = format!("echo \"a b\" ^& 100% c> \"{}\" & echo second>> \"{}\"", out.display(), out.display());
    run(Action::Run { command }).expect("cmd starts");
    let start = Instant::now();
    let mut text = String::new();
    while start.elapsed() < Duration::from_secs(10) && !text.contains("second") {
        text = std::fs::read_to_string(&out).unwrap_or_default();
        std::thread::sleep(Duration::from_millis(50));
    }
    let lines: Vec<&str> = text.lines().map(str::trim).collect();
    assert_eq!(lines, vec!["\"a b\" & 100% c", "second"], "{text:?}");
}

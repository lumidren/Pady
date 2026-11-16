//! PADY-05: secrets from commands and links ended up in the log, and text
//! from outside could forge log lines.
//!
//! When an action failed, Pady logged the whole action: the full shell
//! command of a `run`, the full address of an `open_url` (webhook keys and
//! tokens live in both). `pady.log` is what people attach to bug reports.
//! Names that come from outside (a device's product string, a program's
//! name, a broken config's text) were written as they came, so a line break
//! in one of them started a new, made-up log line.

use crossbeam_channel::unbounded;
use pady_core::{Action, ActionError, Config, Contact, Engine, Executor, Level, SharedConfig, TouchFrame, Trigger};
use std::sync::{Arc, Mutex, OnceLock};

const KEY: &str = "sk-live-4242424242424242";

/// Collects everything logged in this test binary.
struct Capture(Mutex<Vec<String>>);

impl log::Log for Capture {
    fn enabled(&self, _: &log::Metadata) -> bool {
        true
    }
    fn log(&self, record: &log::Record) {
        self.0.lock().unwrap().push(record.args().to_string());
    }
    fn flush(&self) {}
}

fn capture() -> &'static Capture {
    static LOGGER: OnceLock<&'static Capture> = OnceLock::new();
    LOGGER.get_or_init(|| {
        let logger: &'static Capture = Box::leak(Box::new(Capture(Mutex::new(Vec::new()))));
        log::set_logger(logger).expect("the only logger in this test binary");
        log::set_max_level(log::LevelFilter::Trace);
        logger
    })
}

/// Fails every action the way the real executors do: naming what it was given.
struct Failing;

impl Executor for Failing {
    fn run(&mut self, action: &Action) -> Result<Option<Level>, ActionError> {
        match action {
            Action::Run { command } => Err(format!("cannot run `{command}`: No such file or directory").into()),
            Action::OpenUrl { url, .. } => Err(format!("Windows could not open `{url}` (error 2)").into()),
            Action::Launch { program, args } => {
                Err(format!("cannot launch {program} {}: not found", args.join(" ")).into())
            }
            _ => Ok(None),
        }
    }
    fn mouse_button(&mut self, _: bool) -> Result<(), ActionError> {
        Ok(())
    }
    fn mouse_move(&mut self, _: i32, _: i32) -> Result<(), ActionError> {
        Ok(())
    }
    fn foreground_app(&mut self) -> Option<String> {
        None
    }
}

fn tap(t: u64, fingers: u32) -> [TouchFrame; 2] {
    let contacts = (0..fingers).map(|i| Contact { id: i, x: 10.0 + 12.0 * i as f32, y: 30.0 }).collect();
    [
        TouchFrame { time_ms: t, contacts, width_mm: 100.0, height_mm: 60.0 },
        TouchFrame { time_ms: t + 60, contacts: Vec::new(), width_mm: 100.0, height_mm: 60.0 },
    ]
}

#[test]
fn a_failed_action_does_not_put_its_secrets_in_the_log() {
    let log = capture();
    let mut cfg = Config::default();
    cfg.bindings.clear();
    cfg.set_binding(
        Trigger::Tap(3),
        Action::Run { command: format!("curl -H \"Authorization: Bearer {KEY}\" https://api.example.com/lights/on") },
        false,
    );
    cfg.set_binding(
        Trigger::Tap(4),
        Action::OpenUrl {
            url: format!("https://hooks.example.com/trigger/door/with/key/{KEY}?user=me"),
            browser: None,
        },
        false,
    );
    cfg.set_binding(
        Trigger::Tap(5),
        Action::Launch { program: "deploy.exe".into(), args: vec!["--token".into(), KEY.into()] },
        false,
    );

    let (tx, rx) = unbounded();
    let engine = Engine::spawn(Arc::new(SharedConfig::new(cfg)), rx, || Box::new(Failing), Arc::new(|_| {}));
    for (i, fingers) in [3, 4, 5].into_iter().enumerate() {
        for frame in tap(1_000 + i as u64 * 1_000, fingers) {
            tx.send(frame).unwrap();
        }
    }
    drop(tx);
    engine.join();

    let lines = log.0.lock().unwrap().clone();
    let failures: Vec<&String> = lines.iter().filter(|l| l.contains("failed")).collect();
    assert_eq!(failures.len(), 3, "each failure is still logged: {lines:#?}");
    for line in &lines {
        assert!(!line.contains(KEY), "the key is in the log: {line}");
        assert!(!line.contains("Authorization"), "the command's arguments are in the log: {line}");
        assert!(!line.contains("user=me"), "the link's query is in the log: {line}");
    }
    // What failed is still clear enough to act on.
    assert!(failures.iter().any(|l| l.contains("curl")), "{failures:#?}");
    assert!(failures.iter().any(|l| l.contains("hooks.example.com")), "{failures:#?}");
    assert!(failures.iter().any(|l| l.contains("deploy.exe")), "{failures:#?}");
}

#[test]
fn text_from_outside_cannot_forge_a_log_line() {
    let name = "ELAN Touchpad\n[2026-01-01T00:00:00Z WARN  pady] update failed, run fix.exe\r\x1b[2K";
    let line = pady_core::log_line(name);
    assert_eq!(line.lines().count(), 1, "{line:?}");
    assert!(!line.contains('\x1b') && !line.contains('\r'), "{line:?}");
    assert!(line.contains(r"\n[2026-01-01"), "the break is shown, not followed: {line}");
}

/// Both programs that write a log pass every message through `log_line`.
#[test]
fn the_app_and_the_cli_write_one_event_per_line() {
    for source in ["crates/pady-app/src/main.rs", "crates/pady-cli/src/main.rs"] {
        let text = pady_security::read(source);
        assert!(text.contains(".format(|buf, record|"), "{source} sets its own log format");
        assert!(text.contains("pady_core::log_line(&message)"), "{source} escapes each message");
    }
}

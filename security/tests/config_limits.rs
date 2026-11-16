//! PADY-06: a config file had no size limits.
//!
//! Pady reads `config.toml` at start-up, re-reads it two seconds after every
//! change on disk, and reads every backup to list them. Nothing bounded the
//! file: a very large one (a shared "gesture pack", a sync tool gone wrong, a
//! script appending to it) was read whole into memory, copied into up to 50
//! backups and re-parsed on every reload, and a config with hundreds of
//! thousands of profiles made every single gesture slow, since each one
//! looks for the profile of the app in front.

use pady_core::{Action, Config, ConfigError, Direction, Profile, Trigger};
use pady_security::Scratch;
use std::time::Instant;

#[test]
fn an_oversized_config_file_is_refused_without_being_read() {
    let dir = Scratch::new("big-config");
    let path = dir.path("config.toml");
    // 64 MB of valid TOML: one long comment after a real config.
    let mut text = Config::default().to_toml().unwrap();
    text.push_str("# ");
    text.push_str(&"x".repeat(64 << 20));
    std::fs::write(&path, &text).unwrap();
    drop(text);

    let start = Instant::now();
    let result = Config::load(&path);
    let took = start.elapsed();
    match result {
        Err(ConfigError::TooLarge { bytes, .. }) => assert!(bytes > 64 << 20),
        other => panic!("a 64 MB config must be refused as too large, got {:?}", other.map(|_| "a config")),
    }
    assert!(took.as_millis() < 250, "refusing took {took:?}: the file was read");
    // Backups of an oversized config would fill the disk fifty times over.
    assert_eq!(pady_core::backups::snapshot(&path, true).unwrap(), None, "an oversized config is not copied");
}

#[test]
fn a_config_at_the_size_limit_still_loads() {
    let dir = Scratch::new("limit-config");
    let path = dir.path("config.toml");
    let mut text = Config::default().to_toml().unwrap();
    text.push_str("# ");
    let room = Config::MAX_BYTES as usize - text.len();
    text.push_str(&"x".repeat(room));
    assert_eq!(text.len() as u64, Config::MAX_BYTES);
    std::fs::write(&path, &text).unwrap();
    assert_eq!(Config::load(&path).unwrap(), Config::default());
}

#[test]
fn deeply_nested_toml_is_an_error_not_a_crash() {
    for (open, close) in [("[", "]"), ("{a=", "}")] {
        let text = format!("x = {}1{}\n", open.repeat(50_000), close.repeat(50_000));
        assert!(Config::from_toml(&text).is_err());
    }
    let dotted = format!("{}x = 1\n", "a.".repeat(50_000));
    assert!(Config::from_toml(&dotted).is_ok() || Config::from_toml(&dotted).is_err(), "no crash");
}

#[test]
fn counts_and_lengths_are_capped_when_a_config_is_normalised() {
    let mut cfg = Config::default();
    let long = "y".repeat(100_000);
    for i in 0..5_000 {
        cfg.profiles.push(Profile {
            name: format!("{long}{i}"),
            apps: (0..300).map(|a| format!("app{a}.exe")).chain([long.clone()]).collect(),
            bindings: Vec::new(),
        });
    }
    cfg.set_binding(Trigger::Swipe(4, Direction::Up), Action::Run { command: long.clone() }, false);
    cfg.set_binding(Trigger::Tap(4), Action::Keys { combo: long.clone() }, false);
    cfg.set_binding(
        Trigger::Tap(5),
        Action::Launch { program: "x".into(), args: (0..10_000).map(|i| i.to_string()).collect() },
        false,
    );
    cfg.set_binding(Trigger::Tap(3), Action::Mute, false);
    cfg.normalize();

    assert_eq!(cfg.profiles.len(), Config::MAX_PROFILES);
    for p in &cfg.profiles {
        assert!(p.apps.len() <= Config::MAX_APPS_PER_PROFILE, "{} apps", p.apps.len());
        assert!(p.name.chars().count() <= Config::MAX_NAME, "a {}-character name", p.name.len());
        assert!(p.apps.iter().all(|a| a.len() <= Config::MAX_TEXT));
    }
    // An action with an absurdly long command, shortcut or argument list is
    // dropped, not cut short: half a command is a different command.
    assert!(cfg.binding(Trigger::Swipe(4, Direction::Up)).is_none());
    assert!(cfg.binding(Trigger::Tap(4)).is_none());
    assert!(cfg.binding(Trigger::Tap(5)).is_none());
    assert_eq!(cfg.binding(Trigger::Tap(3)).unwrap().action, Action::Mute, "ordinary bindings are untouched");

    // What is left always fits in a file Pady will read back.
    let text = cfg.to_toml().unwrap();
    assert!((text.len() as u64) < Config::MAX_BYTES, "{} bytes", text.len());
    assert_eq!(Config::from_toml(&text).unwrap(), cfg);
}

#[test]
fn finding_the_profile_for_an_app_stays_fast_at_the_limits() {
    let mut cfg = Config::default();
    for i in 0..Config::MAX_PROFILES {
        cfg.profiles.push(Profile {
            name: format!("p{i}"),
            apps: (0..Config::MAX_APPS_PER_PROFILE).map(|a| format!("app-{i}-{a}.exe")).collect(),
            bindings: Vec::new(),
        });
    }
    cfg.normalize();
    let start = Instant::now();
    let rounds = 200;
    for _ in 0..rounds {
        assert!(cfg.resolve(Trigger::Tap(3), Some("C:\\Program Files\\Nowhere\\not-listed.exe")).is_some());
    }
    let each = start.elapsed() / rounds;
    // Debug builds are several times slower than what ships.
    assert!(each.as_millis() < 20, "a gesture waits {each:?} for its profile lookup");
}

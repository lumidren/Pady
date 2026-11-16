//! PADY-01: saving the config from two places at once could leave a torn
//! config file behind.
//!
//! `Config::save` wrote to one fixed temp file (`config.toml.tmp`) and renamed
//! it over the config. The app saves from several threads (the settings
//! window's commands, the tray's on/off switch, first-run setup), and
//! `pady-cli` saves too. Two saves at once shared that temp file: one could
//! rename it into place while the other was still writing it, so the config
//! on disk was cut short or mixed. Pady then failed to read it at the next
//! start, moved it aside as "broken" and fell back to defaults.

use pady_core::{Action, Config, Direction, Profile, Trigger};
use pady_security::Scratch;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

/// A config of roughly `kb` kilobytes, recognisable by its first profile's name.
fn config(tag: &str, kb: usize) -> Config {
    let mut cfg = Config::default();
    cfg.set_binding(Trigger::Swipe(4, Direction::Up), Action::Run { command: format!("echo {tag}") }, false);
    for i in 0..kb {
        cfg.profiles.push(Profile { name: format!("{tag}-{i}"), apps: vec!["x".repeat(1000)], bindings: Vec::new() });
    }
    cfg.normalize();
    cfg
}

#[test]
fn saves_from_several_threads_never_leave_a_torn_config() {
    let dir = Scratch::new("torn-config");
    let path = dir.path("config.toml");
    let configs: Vec<Config> = vec![config("small", 1), config("medium", 200), config("large", 900)];
    configs[0].save(&path).unwrap();

    let stop = Arc::new(AtomicBool::new(false));
    let failed_saves = Arc::new(AtomicUsize::new(0));
    let torn_reads = Arc::new(AtomicUsize::new(0));
    let good_reads = Arc::new(AtomicUsize::new(0));

    let writers: Vec<_> = (0..6)
        .map(|w| {
            let (path, stop, failed) = (path.clone(), stop.clone(), failed_saves.clone());
            let cfg = configs[w % configs.len()].clone();
            std::thread::spawn(move || {
                while !stop.load(Ordering::Relaxed) {
                    if cfg.save(&path).is_err() {
                        failed.fetch_add(1, Ordering::Relaxed);
                    }
                }
            })
        })
        .collect();

    let reader = {
        let (path, stop, torn, good, known) =
            (path.clone(), stop.clone(), torn_reads.clone(), good_reads.clone(), configs.clone());
        std::thread::spawn(move || {
            while !stop.load(Ordering::Relaxed) {
                // A read that fails to open is the OS being busy, not a torn
                // file; a file that opens must hold one whole config.
                let Ok(text) = std::fs::read_to_string(&path) else { continue };
                match Config::from_toml(&text) {
                    Ok(cfg) if known.contains(&cfg) => good.fetch_add(1, Ordering::Relaxed),
                    _ => torn.fetch_add(1, Ordering::Relaxed),
                };
            }
        })
    };

    let start = Instant::now();
    while start.elapsed() < Duration::from_secs(4) {
        std::thread::sleep(Duration::from_millis(50));
    }
    stop.store(true, Ordering::Relaxed);
    for w in writers {
        w.join().unwrap();
    }
    reader.join().unwrap();

    let (failed, torn, good) =
        (failed_saves.load(Ordering::Relaxed), torn_reads.load(Ordering::Relaxed), good_reads.load(Ordering::Relaxed));
    println!("{good} whole reads, {torn} torn reads, {failed} failed saves");
    assert!(good > 0, "the reader never saw the config");
    assert_eq!(torn, 0, "the config on disk was cut short or mixed {torn} times");
    assert_eq!(failed, 0, "{failed} saves failed because another save was in the way");

    // Whatever was saved last is whole, and no temp files are left behind.
    let last = Config::load(&path).expect("the final config is readable");
    assert!(configs.contains(&last));
    let leftovers: Vec<_> = std::fs::read_dir(&dir.0)
        .unwrap()
        .flatten()
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .filter(|n| n != "config.toml")
        .collect();
    assert!(leftovers.is_empty(), "temp files left behind: {leftovers:?}");
}

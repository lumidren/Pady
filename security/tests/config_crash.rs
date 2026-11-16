//! Failure while saving: the config on disk must stay whole whatever happens
//! to the process or the file while a save is under way.
//!
//! * A process killed at a random moment in a save (a crash, the task
//!   manager, the power button) leaves either the old config or the new
//!   one, never part of one, and its unfinished temp file is cleaned up by
//!   a later save.
//! * A save that can't finish (another program holds the config open and
//!   won't let go) reports the failure, keeps the old config and leaves no
//!   temp file.

use pady_core::{Config, Profile};
use pady_security::Scratch;
use std::time::{Duration, SystemTime};

fn config(tag: u64) -> Config {
    let mut cfg = Config::default();
    cfg.settings.hold_ms = 500 + tag % 100;
    // Large enough that a save takes a while to write.
    for i in 0..150 {
        cfg.profiles.push(Profile { name: format!("p{i}"), apps: vec!["a".repeat(2000)], bindings: Vec::new() });
    }
    cfg.normalize();
    cfg
}

/// Not a test of its own: the process that gets killed. Started by
/// `a_process_killed_mid_save_leaves_a_whole_config` with `PADY_SAVE_LOOP`
/// set to the config path; saves different configs until it dies.
#[test]
#[ignore]
fn child_saves_until_killed() {
    let Some(path) = std::env::var_os("PADY_SAVE_LOOP") else { return };
    let path = std::path::PathBuf::from(path);
    let mut tag = 0;
    loop {
        config(tag).save(&path).expect("save");
        tag += 1;
    }
}

fn temp_files(dir: &Scratch) -> Vec<std::path::PathBuf> {
    std::fs::read_dir(&dir.0)
        .unwrap()
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.extension().is_some_and(|x| x == "tmp"))
        .collect()
}

#[test]
fn a_process_killed_mid_save_leaves_a_whole_config() {
    let dir = Scratch::new("killed-save");
    let path = dir.path("config.toml");
    config(0).save(&path).unwrap();
    let me = std::env::current_exe().unwrap();
    let mut rng = pady_security::Rng(0xDEAD);

    for round in 0..25 {
        let mut child = std::process::Command::new(&me)
            .args(["child_saves_until_killed", "--exact", "--ignored", "--nocapture"])
            .env("PADY_SAVE_LOOP", &path)
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .spawn()
            .expect("start the saving process");
        // Let it get going, then kill it at a moment of no one's choosing.
        std::thread::sleep(Duration::from_millis(60 + rng.below(90)));
        child.kill().expect("kill it");
        child.wait().unwrap();

        let text = std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("round {round}: the config is gone: {e}"));
        let cfg = Config::from_toml(&text).unwrap_or_else(|e| panic!("round {round}: the config is torn: {e}"));
        assert_eq!(cfg.profiles.len(), 150, "round {round}: a whole config, old or new");
    }

    // The killed processes' unfinished temp files don't pile up: once they
    // are a minute old, the next save removes them.
    let orphans = temp_files(&dir);
    println!("{} unfinished temp files after 25 kills", orphans.len());
    for orphan in &orphans {
        let file = std::fs::OpenOptions::new().write(true).open(orphan).unwrap();
        file.set_modified(SystemTime::now() - Duration::from_secs(120)).unwrap();
    }
    config(7).save(&path).unwrap();
    assert_eq!(temp_files(&dir), Vec::<std::path::PathBuf>::new(), "stale temp files are swept");
    assert_eq!(Config::load(&path).unwrap(), config(7));
}

#[cfg(windows)]
#[test]
fn a_save_that_cannot_finish_keeps_the_old_config() {
    use std::os::windows::fs::OpenOptionsExt;
    let dir = Scratch::new("blocked-save");
    let path = dir.path("config.toml");
    let old = config(1);
    old.save(&path).unwrap();

    // Another program holds the config open and shares it with no one.
    let held = std::fs::OpenOptions::new().read(true).share_mode(0).open(&path).unwrap();
    let result = config(2).save(&path);
    assert!(result.is_err(), "the save cannot have replaced a file that is locked");
    drop(held);

    assert_eq!(Config::load(&path).unwrap(), old, "the old config is untouched");
    assert_eq!(temp_files(&dir), Vec::<std::path::PathBuf>::new(), "the failed save cleaned up after itself");
    // And once the other program lets go, saving works again.
    config(3).save(&path).unwrap();
    assert_eq!(Config::load(&path).unwrap(), config(3));
}

#[test]
fn a_fresh_temp_file_of_another_save_is_left_alone() {
    let dir = Scratch::new("live-temp");
    let path = dir.path("config.toml");
    config(1).save(&path).unwrap();
    // Another process is mid-save: its temp file is seconds old.
    let live = dir.path("config.toml.99999-0.tmp");
    std::fs::write(&live, "half a config").unwrap();
    config(2).save(&path).unwrap();
    assert!(live.exists(), "a save in progress elsewhere must not lose its temp file");
    // Files that merely look similar are never touched.
    let other = dir.path("config.toml.bak");
    std::fs::write(&other, "mine").unwrap();
    std::fs::OpenOptions::new()
        .write(true)
        .open(&other)
        .unwrap()
        .set_modified(SystemTime::now() - Duration::from_secs(3600))
        .unwrap();
    config(3).save(&path).unwrap();
    assert!(other.exists());
}

//! PADY-02: one backup dated in the future silently switched backups off.
//!
//! A backup's id is the time it was taken. Pady skips a new backup when the
//! newest one is less than two minutes old, measured as `now - newest`. With
//! the newest backup dated *after* now, that difference was clamped to zero,
//! so every ordinary save looked "too soon" until real time caught up with
//! the file's date: possibly years. That happens after the clock was wrong
//! for a while (a dead CMOS battery, a time zone mishap), and anything that
//! can drop a file in the backups folder can do it on purpose. The largest
//! possible id also overflowed the "never overwrite a copy" arithmetic.

use pady_core::{backups, Config};
use pady_security::Scratch;
use std::time::{SystemTime, UNIX_EPOCH};

fn now_ms() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_millis() as u64
}

fn plant(dir: &Scratch, id: u64) {
    let folder = backups::dir(&dir.path("config.toml"));
    std::fs::create_dir_all(&folder).unwrap();
    std::fs::write(folder.join(format!("config-{id}.toml")), "# planted\n").unwrap();
}

#[test]
fn a_backup_dated_in_the_future_does_not_stop_backups() {
    let dir = Scratch::new("future-backup");
    let path = dir.path("config.toml");
    Config::default().save(&path).unwrap();
    // A copy "taken" a year from now.
    plant(&dir, now_ms() + 365 * 24 * 3_600_000);

    let made = backups::snapshot(&path, false).expect("no error");
    assert!(made.is_some(), "an ordinary save must still be backed up");
    let text = std::fs::read_to_string(made.unwrap()).unwrap();
    assert_eq!(Config::from_toml(&text).unwrap(), Config::default());
}

#[test]
fn the_largest_possible_backup_id_is_survived() {
    let dir = Scratch::new("max-backup");
    let path = dir.path("config.toml");
    Config::default().save(&path).unwrap();
    plant(&dir, u64::MAX);

    // Used to overflow (`u64::MAX + 1`): a panic in debug builds.
    for force in [false, true] {
        let made = backups::snapshot(&path, force).expect("no error");
        if let Some(copy) = made {
            assert_ne!(copy, backups::dir(&path).join(format!("config-{}.toml", u64::MAX)), "never overwrite a copy");
        }
    }
    assert!(
        std::fs::read_to_string(backups::dir(&path).join(format!("config-{}.toml", u64::MAX))).unwrap()
            == "# planted\n",
        "the planted copy is untouched"
    );
    assert!(backups::list(&path).len() >= 2, "a real copy was made next to the planted one");
}

#[test]
fn restoring_never_reads_outside_the_backups_folder() {
    let dir = Scratch::new("backup-names");
    let path = dir.path("config.toml");
    Config::default().save(&path).unwrap();
    let folder = backups::dir(&path);
    std::fs::create_dir_all(&folder).unwrap();
    // Names that try to be something else than `config-<number>.toml`.
    for name in [
        "config-..toml",
        "config-1/../../x.toml",
        "config--1.toml",
        "config-1e3.toml",
        "config-0x10.toml",
        "config- 7.toml",
        "config-7.toml.bak",
    ] {
        let _ = std::fs::write(folder.join(name), "x");
    }
    assert!(backups::list(&path).is_empty(), "only config-<digits>.toml counts: {:?}", backups::list(&path));
    assert_eq!(backups::find(&path, 7), None);
}

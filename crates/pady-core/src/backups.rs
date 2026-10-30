//! Copies of the config kept in a `backups` folder next to it, so a slip in
//! the app, a bad import or a broken update can always be undone.
//!
//! A copy is taken before the config is overwritten. Identical copies are
//! skipped, and while someone is busy editing only the state from before
//! the burst is kept, so a dozen toggles don't push real history out.

use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

/// How many copies to keep; older ones are deleted.
pub const KEEP: usize = 50;
/// Saves this soon after the last copy don't take another (unless forced).
pub const QUIET: Duration = Duration::from_secs(120);

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Backup {
    /// When the copy was taken, in milliseconds since the Unix epoch. Also its id.
    pub id: u64,
    pub path: PathBuf,
}

/// The backups folder for the config at `config`.
pub fn dir(config: &Path) -> PathBuf {
    config.with_file_name("backups")
}

/// Every copy, newest first.
pub fn list(config: &Path) -> Vec<Backup> {
    let Ok(entries) = std::fs::read_dir(dir(config)) else { return Vec::new() };
    let mut out: Vec<Backup> = entries
        .filter_map(|e| e.ok())
        .filter_map(|e| {
            let name = e.file_name();
            let id = name.to_str()?.strip_prefix("config-")?.strip_suffix(".toml")?.parse().ok()?;
            Some(Backup { id, path: e.path() })
        })
        .collect();
    out.sort_by_key(|b| std::cmp::Reverse(b.id));
    out
}

/// The copy with this id.
pub fn find(config: &Path, id: u64) -> Option<Backup> {
    list(config).into_iter().find(|b| b.id == id)
}

/// Copy the config as it is on disk. `force` ignores [`QUIET`] (before a
/// reset, a preset or a restore); identical copies are always skipped.
/// Returns the new copy, if one was made.
pub fn snapshot(config: &Path, force: bool) -> std::io::Result<Option<PathBuf>> {
    let now = SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |d| d.as_millis() as u64);
    snapshot_at(config, force, now)
}

fn snapshot_at(config: &Path, force: bool, now: u64) -> std::io::Result<Option<PathBuf>> {
    let Ok(current) = std::fs::read(config) else { return Ok(None) };
    let existing = list(config);
    if let Some(newest) = existing.first() {
        if std::fs::read(&newest.path).is_ok_and(|b| b == current) {
            return Ok(None);
        }
        if !force && now.saturating_sub(newest.id) < QUIET.as_millis() as u64 {
            return Ok(None);
        }
    }
    let folder = dir(config);
    std::fs::create_dir_all(&folder)?;
    // Never overwrite a copy, even if two land in the same millisecond.
    let id = existing.first().map_or(now, |n| now.max(n.id + 1));
    let path = folder.join(format!("config-{id}.toml"));
    std::fs::write(&path, current)?;
    for old in existing.iter().skip(KEEP - 1) {
        let _ = std::fs::remove_file(&old.path);
    }
    Ok(Some(path))
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Folder(PathBuf);
    impl Folder {
        fn new(name: &str) -> Self {
            let p = std::env::temp_dir().join(format!("pady-backups-{name}-{}", std::process::id()));
            let _ = std::fs::remove_dir_all(&p);
            std::fs::create_dir_all(&p).unwrap();
            Folder(p)
        }
        fn config(&self) -> PathBuf {
            self.0.join("config.toml")
        }
    }
    impl Drop for Folder {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    const MIN: u64 = 60_000;

    #[test]
    fn copies_before_changes_and_skips_identical_ones() {
        let f = Folder::new("identical");
        let cfg = f.config();
        assert_eq!(snapshot_at(&cfg, false, 1).unwrap(), None, "no config yet, nothing to copy");
        std::fs::write(&cfg, "a").unwrap();
        assert!(snapshot_at(&cfg, false, 10 * MIN).unwrap().is_some());
        assert_eq!(snapshot_at(&cfg, true, 20 * MIN).unwrap(), None, "same content");
        std::fs::write(&cfg, "b").unwrap();
        assert!(snapshot_at(&cfg, false, 30 * MIN).unwrap().is_some());
        let all = list(&cfg);
        assert_eq!(all.iter().map(|b| b.id).collect::<Vec<_>>(), vec![30 * MIN, 10 * MIN], "newest first");
        assert_eq!(std::fs::read_to_string(&all[1].path).unwrap(), "a");
    }

    #[test]
    fn a_burst_of_edits_keeps_the_state_from_before_it() {
        let f = Folder::new("burst");
        let cfg = f.config();
        std::fs::write(&cfg, "before").unwrap();
        snapshot_at(&cfg, false, 10 * MIN).unwrap();
        std::fs::write(&cfg, "edit 1").unwrap();
        assert_eq!(snapshot_at(&cfg, false, 10 * MIN + 30_000).unwrap(), None, "still editing");
        assert!(snapshot_at(&cfg, true, 10 * MIN + 40_000).unwrap().is_some(), "a restore always copies");
        std::fs::write(&cfg, "edit 2").unwrap();
        assert!(snapshot_at(&cfg, false, 20 * MIN).unwrap().is_some(), "a later change copies again");
        assert_eq!(list(&cfg).len(), 3);
    }

    #[test]
    fn keeps_only_the_newest_copies() {
        let f = Folder::new("prune");
        let cfg = f.config();
        for i in 0..(KEEP as u64 + 7) {
            std::fs::write(&cfg, format!("v{i}")).unwrap();
            snapshot_at(&cfg, false, (i + 1) * 10 * MIN).unwrap();
        }
        let all = list(&cfg);
        assert_eq!(all.len(), KEEP);
        assert_eq!(std::fs::read_to_string(&all[0].path).unwrap(), format!("v{}", KEEP + 6));
        assert_eq!(find(&cfg, all[3].id), Some(all[3].clone()));
        assert_eq!(find(&cfg, 1), None);
    }

    #[test]
    fn ignores_other_files_in_the_folder() {
        let f = Folder::new("other");
        let cfg = f.config();
        std::fs::create_dir_all(dir(&cfg)).unwrap();
        std::fs::write(dir(&cfg).join("notes.txt"), "x").unwrap();
        std::fs::write(dir(&cfg).join("config-abc.toml"), "x").unwrap();
        assert!(list(&cfg).is_empty());
    }
}

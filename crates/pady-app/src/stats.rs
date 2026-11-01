//! The Stats page's numbers: how many gestures ran and how long you focused,
//! kept in `stats.json` next to the config. Counts and action names only,
//! never what was typed, copied or where, and never sent anywhere.
//!
//! Counts go into 15-minute slots of the system clock (UTC), and the page
//! adds them up into days of the local time zone: every zone's midnight
//! falls on a quarter hour, so Pady needs no time-zone code of its own.

use crate::lock;
use crate::notes::wall_ms;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Mutex;
use std::time::Duration;
use tauri::State;

const SLOT_MS: u64 = 15 * 60 * 1000;
/// Slots older than this are dropped (a little over two months).
const KEEP_MS: u64 = 64 * 24 * 60 * 60 * 1000;
/// Counts are written at most this often (and when Pady quits).
const SAVE_EVERY: Duration = Duration::from_secs(60);

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Slot {
    /// Gestures that ran an action.
    #[serde(default)]
    pub gestures: u32,
    /// Minutes of focus timers finished or stopped in this slot.
    #[serde(default)]
    pub focus_minutes: u32,
    /// How often each action ran ("Copy", "Volume +4%").
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub actions: BTreeMap<String, u32>,
    /// How often each gesture ran, by its config name (`swipe:3:up`).
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub triggers: BTreeMap<String, u32>,
}

#[derive(Default, Serialize, Deserialize)]
struct File {
    /// Keyed by slot start, in milliseconds since 1970 (UTC).
    slots: BTreeMap<u64, Slot>,
}

pub struct Stats {
    path: PathBuf,
    slots: Mutex<BTreeMap<u64, Slot>>,
    dirty: AtomicBool,
}

impl Stats {
    pub fn load(config: &Path) -> Stats {
        let path = config.with_file_name("stats.json");
        let slots = std::fs::read_to_string(&path)
            .ok()
            .and_then(|text| serde_json::from_str::<File>(&text).ok())
            .map(|f| f.slots)
            .unwrap_or_default();
        Stats { path, slots: Mutex::new(slots), dirty: AtomicBool::new(false) }
    }

    fn slot(&self, at: u64, change: impl FnOnce(&mut Slot)) {
        let mut slots = lock(&self.slots);
        change(slots.entry(at - at % SLOT_MS).or_default());
        let oldest = at.saturating_sub(KEEP_MS);
        slots.retain(|&start, _| start >= oldest);
        self.dirty.store(true, Ordering::Relaxed);
    }

    pub fn gesture(&self, action: &str, trigger: Option<&str>) {
        self.gesture_at(wall_ms(), action, trigger);
    }

    fn gesture_at(&self, at: u64, action: &str, trigger: Option<&str>) {
        self.slot(at, |s| {
            s.gestures += 1;
            *s.actions.entry(action.to_string()).or_default() += 1;
            if let Some(trigger) = trigger {
                *s.triggers.entry(trigger.to_string()).or_default() += 1;
            }
        });
    }

    pub fn focused(&self, minutes: u32) {
        if minutes > 0 {
            self.slot(wall_ms(), |s| s.focus_minutes += minutes);
        }
    }

    pub fn snapshot(&self) -> BTreeMap<u64, Slot> {
        lock(&self.slots).clone()
    }

    pub fn clear(&self) {
        lock(&self.slots).clear();
        self.dirty.store(true, Ordering::Relaxed);
        self.save();
    }

    /// Write the counts if they changed since the last write.
    pub fn save(&self) {
        if !self.dirty.swap(false, Ordering::Relaxed) {
            return;
        }
        let text = match serde_json::to_string(&File { slots: self.snapshot() }) {
            Ok(text) => text,
            Err(e) => return log::warn!("could not save stats: {e}"),
        };
        let tmp = self.path.with_extension("json.tmp");
        let written = self
            .path
            .parent()
            .map_or(Ok(()), std::fs::create_dir_all)
            .and_then(|()| std::fs::write(&tmp, text))
            .and_then(|()| std::fs::rename(&tmp, &self.path));
        if let Err(e) = written {
            log::warn!("could not save stats: {e}");
        }
    }

    /// Save now and then on a thread of its own.
    pub fn spawn_saver(app: tauri::AppHandle) {
        use tauri::Manager;
        let _ = std::thread::Builder::new().name("pady-stats".into()).spawn(move || loop {
            std::thread::sleep(SAVE_EVERY);
            app.state::<Stats>().save();
        });
    }
}

// ---------------------------------------------------------------------------
// Commands (the main window)

#[tauri::command]
pub fn stats_get(stats: State<Stats>) -> BTreeMap<u64, Slot> {
    stats.snapshot()
}

#[tauri::command]
pub fn stats_clear(stats: State<Stats>) {
    stats.clear();
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn counts_fall_into_quarter_hours_and_old_ones_go() {
        let dir = std::env::temp_dir().join(format!("pady-stats-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let stats = Stats::load(&dir.join("config.toml"));
        let t = 1_000 * SLOT_MS;
        stats.gesture_at(t + 1, "Copy", Some("tap:3"));
        stats.gesture_at(t + SLOT_MS - 1, "Copy", Some("tap:3"));
        stats.gesture_at(t + SLOT_MS, "Paste", None);
        let slots = stats.snapshot();
        assert_eq!(slots.len(), 2);
        assert_eq!(slots[&t].gestures, 2);
        assert_eq!(slots[&t].actions["Copy"], 2);
        assert_eq!(slots[&t].triggers["tap:3"], 2);
        assert!(slots[&(t + SLOT_MS)].triggers.is_empty());
        assert_eq!(slots[&(t + SLOT_MS)].actions["Paste"], 1);
        // Two months later the old slots are gone.
        stats.gesture_at(t + KEEP_MS + 2 * SLOT_MS, "Copy", None);
        assert_eq!(stats.snapshot().len(), 1);
        // Saved and read back.
        stats.save();
        assert_eq!(Stats::load(&dir.join("config.toml")).snapshot(), stats.snapshot());
        stats.clear();
        assert!(Stats::load(&dir.join("config.toml")).snapshot().is_empty());
    }
}

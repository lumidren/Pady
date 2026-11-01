//! Notes: what the Quick note gesture and the Notes tab write, kept in
//! `notes.json` next to the config. Plain text, on this computer only.

use crate::lock;
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use tauri::{AppHandle, Emitter, State};

/// A note longer than this is cut (a paste of a whole book, by mistake).
const MAX_CHARS: usize = 100_000;
/// The colours a note can have (the page draws them).
pub const COLORS: &[&str] = &["yellow", "orange", "red", "pink", "purple", "blue", "teal", "green", "gray"];

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Note {
    pub id: u64,
    pub text: String,
    /// Milliseconds since 1970.
    pub created: u64,
    pub updated: u64,
    /// The app the note was taken in, for a quick note (`chrome.exe`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source: Option<String>,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub pinned: bool,
    /// One of [`COLORS`], or none.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub color: Option<String>,
}

#[derive(Default, Serialize, Deserialize)]
struct File {
    notes: Vec<Note>,
}

pub struct Notes {
    path: PathBuf,
    notes: Mutex<Vec<Note>>,
}

impl Notes {
    pub fn load(config: &Path) -> Notes {
        let path = config.with_file_name("notes.json");
        let notes = match std::fs::read_to_string(&path) {
            Ok(text) => serde_json::from_str::<File>(&text).map(|f| f.notes).unwrap_or_else(|e| {
                log::warn!("notes.json is unreadable ({e}); keeping it as notes.broken.json");
                let _ = std::fs::rename(&path, path.with_file_name("notes.broken.json"));
                Vec::new()
            }),
            Err(_) => Vec::new(),
        };
        Notes { path, notes: Mutex::new(notes) }
    }

    /// Newest first, pinned notes before the rest.
    pub fn list(&self) -> Vec<Note> {
        let mut notes = lock(&self.notes).clone();
        notes.sort_by(|a, b| b.pinned.cmp(&a.pinned).then(b.updated.cmp(&a.updated)));
        notes
    }

    /// Add a note (`id: None`) or change one. Saving a note empty deletes it.
    pub fn save(&self, id: Option<u64>, text: &str, source: Option<String>) -> Result<Option<Note>, String> {
        let text = cut(text);
        let now = wall_ms();
        let mut notes = lock(&self.notes);
        let saved = match id.and_then(|id| notes.iter().position(|n| n.id == id)) {
            Some(i) if text.trim().is_empty() => {
                notes.remove(i);
                None
            }
            Some(i) => {
                let note = &mut notes[i];
                if note.text != text {
                    note.text = text;
                    note.updated = now;
                }
                Some(note.clone())
            }
            None if text.trim().is_empty() => None,
            None => {
                // Unique even for two notes in the same millisecond.
                let id = notes.iter().map(|n| n.id + 1).max().unwrap_or(0).max(now);
                let note = Note { id, text, created: now, updated: now, source, pinned: false, color: None };
                notes.push(note.clone());
                Some(note)
            }
        };
        self.write(&notes)?;
        Ok(saved)
    }

    pub fn delete(&self, id: u64) -> Result<(), String> {
        let mut notes = lock(&self.notes);
        notes.retain(|n| n.id != id);
        self.write(&notes)
    }

    pub fn pin(&self, id: u64, pinned: bool) -> Result<(), String> {
        let mut notes = lock(&self.notes);
        if let Some(note) = notes.iter_mut().find(|n| n.id == id) {
            note.pinned = pinned;
        }
        self.write(&notes)
    }

    pub fn color(&self, id: u64, color: Option<String>) -> Result<(), String> {
        let color = color.filter(|c| COLORS.contains(&c.as_str()));
        let mut notes = lock(&self.notes);
        if let Some(note) = notes.iter_mut().find(|n| n.id == id) {
            note.color = color;
        }
        self.write(&notes)
    }

    /// A temporary file, then a rename: a crash never leaves half a file.
    fn write(&self, notes: &[Note]) -> Result<(), String> {
        let text = serde_json::to_string_pretty(&File { notes: notes.to_vec() }).map_err(|e| e.to_string())?;
        if let Some(dir) = self.path.parent() {
            std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
        }
        let tmp = self.path.with_extension("json.tmp");
        std::fs::write(&tmp, text).and_then(|()| std::fs::rename(&tmp, &self.path)).map_err(|e| {
            log::warn!("could not save notes: {e}");
            format!("Couldn't save your notes: {e}")
        })
    }
}

/// Milliseconds since 1970, from the system clock.
pub fn wall_ms() -> u64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_millis() as u64)
}

fn cut(text: &str) -> String {
    match text.char_indices().nth(MAX_CHARS) {
        Some((end, _)) => text[..end].to_string(),
        None => text.to_string(),
    }
}

/// Tell the main window the notes changed (a quick note was saved).
pub fn changed(app: &AppHandle) {
    let _ = app.emit_to("main", "pady://notes", ());
}

// ---------------------------------------------------------------------------
// Commands (the main window)

#[tauri::command]
pub fn notes_list(notes: State<Notes>) -> Vec<Note> {
    notes.list()
}

#[tauri::command]
pub fn note_save(notes: State<Notes>, id: Option<u64>, text: String) -> Result<Option<Note>, String> {
    notes.save(id, &text, None)
}

#[tauri::command]
pub fn note_delete(notes: State<Notes>, id: u64) -> Result<(), String> {
    notes.delete(id)
}

#[tauri::command]
pub fn note_pin(notes: State<Notes>, id: u64, pinned: bool) -> Result<(), String> {
    notes.pin(id, pinned)
}

#[tauri::command]
pub fn note_color(notes: State<Notes>, id: u64, color: Option<String>) -> Result<(), String> {
    notes.color(id, color)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn store(name: &str) -> Notes {
        let dir = std::env::temp_dir().join(format!("pady-notes-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        Notes::load(&dir.join("config.toml"))
    }

    #[test]
    fn notes_save_change_pin_and_delete() {
        let notes = store("cycle");
        let a = notes.save(None, "milk", Some("chrome.exe".into())).unwrap().unwrap();
        let b = notes.save(None, "eggs", None).unwrap().unwrap();
        assert_ne!(a.id, b.id);
        notes.pin(a.id, true).unwrap();
        notes.color(a.id, Some("blue".into())).unwrap();
        notes.color(b.id, Some("plaid".into())).unwrap();
        assert_eq!(notes.list().iter().map(|n| n.text.as_str()).collect::<Vec<_>>(), ["milk", "eggs"]);
        notes.save(Some(b.id), "eggs and bread", None).unwrap();
        // It survives a restart.
        let again = Notes::load(&notes.path.with_file_name("config.toml"));
        let list = again.list();
        assert_eq!(list.len(), 2);
        assert_eq!(list[0].source.as_deref(), Some("chrome.exe"));
        assert_eq!(list[0].color.as_deref(), Some("blue"));
        assert_eq!(list[1].color, None);
        assert_eq!(list[1].text, "eggs and bread");
        // Emptied, a note goes away; an empty new note is never made.
        assert_eq!(again.save(Some(a.id), "  ", None).unwrap(), None);
        assert_eq!(again.save(None, "", None).unwrap(), None);
        again.delete(b.id).unwrap();
        assert!(again.list().is_empty());
    }

    #[test]
    fn a_broken_file_is_kept_aside_not_lost() {
        let notes = store("broken");
        std::fs::create_dir_all(notes.path.parent().unwrap()).unwrap();
        std::fs::write(&notes.path, "{ not json").unwrap();
        let loaded = Notes::load(&notes.path.with_file_name("config.toml"));
        assert!(loaded.list().is_empty());
        assert!(notes.path.with_file_name("notes.broken.json").exists());
    }
}

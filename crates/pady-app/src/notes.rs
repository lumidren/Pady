//! Notes: what the Quick note gesture and the Notes tab write, kept in
//! `notes.json` next to the config. Plain text, on this computer only.

use crate::lock;
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use tauri::{AppHandle, Emitter, Manager, State};

/// A note longer than this is cut (a paste of a whole book, by mistake).
const MAX_CHARS: usize = 100_000;
/// A title is one line, and a short one.
const MAX_TITLE_CHARS: usize = 120;
/// The colours a note can have (the page draws them).
pub const COLORS: &[&str] = &["yellow", "orange", "red", "pink", "purple", "blue", "teal", "green", "gray"];

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Note {
    pub id: u64,
    /// What the note is called. Empty for a note that was never named (a
    /// quick note): the page then shows its first line.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub title: String,
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

    /// Add a note (`id: None`) or change one. `title: None` leaves the title
    /// as it is. A note saved with neither a title nor text is deleted.
    pub fn save(
        &self,
        id: Option<u64>,
        title: Option<&str>,
        text: &str,
        source: Option<String>,
    ) -> Result<Option<Note>, String> {
        let text = cut(text, MAX_CHARS);
        let title = title.map(one_line);
        let now = wall_ms();
        let mut notes = lock(&self.notes);
        let saved = match id.and_then(|id| notes.iter().position(|n| n.id == id)) {
            Some(i) => {
                let title = title.unwrap_or_else(|| notes[i].title.clone());
                if text.trim().is_empty() && title.is_empty() {
                    notes.remove(i);
                    None
                } else {
                    let note = &mut notes[i];
                    if note.text != text || note.title != title {
                        note.text = text;
                        note.title = title;
                        note.updated = now;
                    }
                    Some(note.clone())
                }
            }
            None => {
                let title = title.unwrap_or_default();
                if text.trim().is_empty() && title.is_empty() {
                    return Ok(None);
                }
                // Unique even for two notes in the same millisecond.
                let id = notes.iter().map(|n| n.id + 1).max().unwrap_or(0).max(now);
                let note = Note { id, title, text, created: now, updated: now, source, pinned: false, color: None };
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

    /// Every note in one Markdown document, in the order of [`Notes::list`],
    /// with a rule between them. A note's title is its heading.
    pub fn markdown(&self) -> String {
        let one = |n: &Note| match (n.title.is_empty(), n.text.trim()) {
            (true, text) => text.to_string(),
            (false, "") => format!("# {}", n.title),
            (false, text) => format!("# {}\n\n{text}", n.title),
        };
        let notes: Vec<String> = self.list().iter().map(one).collect();
        notes.join("\n\n---\n\n") + "\n"
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

fn cut(text: &str, max: usize) -> String {
    match text.char_indices().nth(max) {
        Some((end, _)) => text[..end].to_string(),
        None => text.to_string(),
    }
}

/// A title as it is kept: one line, no blanks around it, not too long.
fn one_line(title: &str) -> String {
    let line = title.split_whitespace().collect::<Vec<_>>().join(" ");
    cut(&line, MAX_TITLE_CHARS)
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
pub fn note_save(
    notes: State<Notes>,
    id: Option<u64>,
    title: Option<String>,
    text: String,
) -> Result<Option<Note>, String> {
    notes.save(id, title.as_deref(), &text, None)
}

#[tauri::command]
pub fn note_delete(notes: State<Notes>, id: u64) -> Result<(), String> {
    notes.delete(id)
}

/// Save every note into one Markdown file the user picks; the file's path,
/// or `None` if they changed their mind.
#[tauri::command]
pub async fn notes_export(app: AppHandle) -> Result<Option<String>, String> {
    use tauri_plugin_dialog::DialogExt;
    let picked = app
        .dialog()
        .file()
        .set_title("Export your notes")
        .set_file_name("Pady notes.md")
        .add_filter("Markdown", &["md"])
        .add_filter("Plain text", &["txt"])
        .blocking_save_file();
    let Some(path) = picked.and_then(|p| p.into_path().ok()) else { return Ok(None) };
    std::fs::write(&path, app.state::<Notes>().markdown())
        .map_err(|e| format!("Couldn't write {}: {e}", path.display()))?;
    Ok(Some(path.display().to_string()))
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
        let a = notes.save(None, None, "milk", Some("chrome.exe".into())).unwrap().unwrap();
        let b = notes.save(None, None, "eggs", None).unwrap().unwrap();
        assert_ne!(a.id, b.id);
        notes.pin(a.id, true).unwrap();
        notes.color(a.id, Some("blue".into())).unwrap();
        notes.color(b.id, Some("plaid".into())).unwrap();
        assert_eq!(notes.list().iter().map(|n| n.text.as_str()).collect::<Vec<_>>(), ["milk", "eggs"]);
        notes.save(Some(b.id), None, "eggs and bread", None).unwrap();
        // It survives a restart.
        let again = Notes::load(&notes.path.with_file_name("config.toml"));
        let list = again.list();
        assert_eq!(list.len(), 2);
        assert_eq!(list[0].source.as_deref(), Some("chrome.exe"));
        assert_eq!(list[0].color.as_deref(), Some("blue"));
        assert_eq!(list[1].color, None);
        assert_eq!(list[1].text, "eggs and bread");
        assert_eq!(again.markdown(), "milk\n\n---\n\neggs and bread\n");
        // Emptied, a note goes away; an empty new note is never made.
        assert_eq!(again.save(Some(a.id), None, "  ", None).unwrap(), None);
        assert_eq!(again.save(None, None, "", None).unwrap(), None);
        again.delete(b.id).unwrap();
        assert!(again.list().is_empty());
    }

    #[test]
    fn a_note_can_have_a_title() {
        let notes = store("titles");
        // A title is kept as one tidy line.
        let a = notes.save(None, Some("  Shopping\n list  "), "milk", None).unwrap().unwrap();
        assert_eq!(a.title, "Shopping list");
        // Saving the text alone (a quick note does) leaves the title.
        let a2 = notes.save(Some(a.id), None, "milk, eggs", None).unwrap().unwrap();
        assert_eq!((a2.title.as_str(), a2.text.as_str()), ("Shopping list", "milk, eggs"));
        // A title alone is a note; so is text alone; neither is no note.
        let b = notes.save(None, Some("Call the bank"), "", None).unwrap().unwrap();
        assert_eq!(notes.save(None, Some("  "), " ", None).unwrap(), None);
        assert_eq!(notes.save(Some(b.id), Some(""), "", None).unwrap(), None);
        // Renaming counts as an edit, and the title is the heading in an export.
        let c = notes.save(Some(a.id), Some("Groceries"), "milk, eggs", None).unwrap().unwrap();
        assert!(c.updated >= a.updated);
        let only = notes.save(None, Some("Ideas"), "", None).unwrap().unwrap();
        notes.pin(only.id, true).unwrap();
        assert_eq!(notes.markdown(), "# Ideas\n\n---\n\n# Groceries\n\nmilk, eggs\n");
        // A very long title is cut.
        let long = notes.save(None, Some(&"x".repeat(500)), "", None).unwrap().unwrap();
        assert_eq!(long.title.chars().count(), MAX_TITLE_CHARS);
        // It survives a restart.
        let again = Notes::load(&notes.path.with_file_name("config.toml"));
        assert_eq!(again.list().len(), 3);
        assert_eq!(again.list()[0].title, "Ideas");
        // A file written before titles existed still loads, and a note
        // without a title is written without one.
        std::fs::write(&notes.path, r#"{"notes":[{"id":1,"text":"old","created":1,"updated":1}]}"#).unwrap();
        let old = Notes::load(&notes.path.with_file_name("config.toml"));
        assert_eq!(old.list()[0].title, "");
        old.pin(1, true).unwrap();
        assert!(!std::fs::read_to_string(&notes.path).unwrap().contains("title"));
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

//! A simple file picker: folders on the left ("places"), files on the right.

use std::path::{Path, PathBuf};
use std::time::SystemTime;

use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::widgets::TableState;

const JSON_EXTENSIONS: &[&str] = &["json", "jsonl", "ndjson", "bson.json"];

#[derive(Debug, Clone)]
pub struct Entry {
    pub name: String,
    pub path: PathBuf,
    pub is_dir: bool,
    pub is_parent: bool,
    pub size: u64,
    pub modified: Option<SystemTime>,
}

pub struct FileBrowser {
    pub dir: PathBuf,
    pub entries: Vec<Entry>,
    pub visible: Vec<usize>,
    pub filter: String,
    pub table: TableState,
    pub places: Vec<(String, PathBuf)>,
    pub places_table: TableState,
    pub focus_places: bool,
    pub show_all: bool,
    pub error: Option<String>,
    /// Count of files hidden because they are not JSON.
    pub hidden_files: usize,
}

pub enum BrowserOutcome {
    None,
    Close,
    Chosen(PathBuf),
}

fn is_json(name: &str) -> bool {
    let lower = name.to_ascii_lowercase();
    JSON_EXTENSIONS
        .iter()
        .any(|e| lower.ends_with(&format!(".{e}")))
}

fn places() -> Vec<(String, PathBuf)> {
    let mut out: Vec<(String, PathBuf)> = Vec::new();
    let mut add = |label: &str, p: Option<PathBuf>| {
        if let Some(p) = p
            && p.is_dir()
            && !out.iter().any(|(_, q)| *q == p)
        {
            out.push((label.to_string(), p));
        }
    };
    add("Current folder", std::env::current_dir().ok());
    add("Home", dirs::home_dir());
    add("Desktop", dirs::desktop_dir());
    add("Documents", dirs::document_dir());
    add("Downloads", dirs::download_dir());
    #[cfg(windows)]
    for letter in b'A'..=b'Z' {
        let p = PathBuf::from(format!("{}:\\", letter as char));
        if p.exists() {
            out.push((format!("Drive {}:", letter as char), p));
        }
    }
    #[cfg(not(windows))]
    out.push(("Computer (/)".into(), PathBuf::from("/")));
    out
}

impl FileBrowser {
    /// Opens the picker in `start` (or a sensible fallback), with `select`
    /// highlighted when it is in that folder.
    pub fn open(start: Option<PathBuf>, select: Option<&Path>) -> Self {
        let mut b = FileBrowser {
            dir: PathBuf::new(),
            entries: Vec::new(),
            visible: Vec::new(),
            filter: String::new(),
            table: TableState::default(),
            places: places(),
            places_table: TableState::default().with_selected(Some(0)),
            focus_places: false,
            show_all: false,
            error: None,
            hidden_files: 0,
        };
        let candidates = [start, std::env::current_dir().ok(), dirs::home_dir()];
        for dir in candidates.into_iter().flatten() {
            if b.load(&dir) {
                break;
            }
        }
        if let Some(sel) = select
            && let Some(pos) = b.visible.iter().position(|&i| b.entries[i].path == sel)
        {
            b.table.select(Some(pos));
        }
        b
    }

    /// Reads a folder. Returns false (keeping the old listing) on error.
    pub fn load(&mut self, dir: &Path) -> bool {
        let dir = std::fs::canonicalize(dir).unwrap_or_else(|_| dir.to_path_buf());
        let read = match std::fs::read_dir(&dir) {
            Ok(r) => r,
            Err(e) => {
                self.error = Some(format!("Cannot open {}: {e}", dir.display()));
                return false;
            }
        };
        let mut entries = Vec::new();
        for item in read.flatten() {
            let path = item.path();
            let name = item.file_name().to_string_lossy().into_owned();
            // Follow symlinks so linked folders behave like folders.
            let meta = std::fs::metadata(&path).or_else(|_| item.metadata());
            let Ok(meta) = meta else { continue };
            entries.push(Entry {
                name,
                path,
                is_dir: meta.is_dir(),
                is_parent: false,
                size: meta.len(),
                modified: meta.modified().ok(),
            });
        }
        entries.sort_by(|a, b| {
            b.is_dir
                .cmp(&a.is_dir)
                .then_with(|| a.name.to_lowercase().cmp(&b.name.to_lowercase()))
        });
        if let Some(parent) = dir.parent() {
            entries.insert(
                0,
                Entry {
                    name: "..  (up one folder)".into(),
                    path: parent.to_path_buf(),
                    is_dir: true,
                    is_parent: true,
                    size: 0,
                    modified: None,
                },
            );
        }
        self.dir = dir;
        self.entries = entries;
        self.filter.clear();
        self.error = None;
        self.refresh();
        self.table.select(Some(0));
        *self.table.offset_mut() = 0;
        true
    }

    pub fn refresh(&mut self) {
        let needle = self.filter.to_lowercase();
        self.hidden_files = 0;
        let mut visible = Vec::new();
        for (i, e) in self.entries.iter().enumerate() {
            if !needle.is_empty() && !e.name.to_lowercase().contains(&needle) {
                continue;
            }
            if !self.show_all && !e.is_parent {
                let hidden = e.name.starts_with('.');
                if hidden || (!e.is_dir && !is_json(&e.name)) {
                    if !e.is_dir {
                        self.hidden_files += 1;
                    }
                    continue;
                }
            }
            visible.push(i);
        }
        self.visible = visible;
        let sel = self
            .table
            .selected()
            .unwrap_or(0)
            .min(self.visible.len().saturating_sub(1));
        self.table.select(if self.visible.is_empty() {
            None
        } else {
            Some(sel)
        });
    }

    pub fn selected_entry(&self) -> Option<&Entry> {
        self.table
            .selected()
            .and_then(|i| self.visible.get(i))
            .map(|&i| &self.entries[i])
    }

    fn go_up(&mut self) {
        if let Some(parent) = self.dir.parent().map(Path::to_path_buf) {
            let old = self.dir.clone();
            if self.load(&parent)
                && let Some(pos) = self
                    .visible
                    .iter()
                    .position(|&i| self.entries[i].path == old)
            {
                self.table.select(Some(pos));
            }
        }
    }

    /// Opens the highlighted folder, or chooses the highlighted file.
    pub fn activate(&mut self) -> BrowserOutcome {
        if self.focus_places {
            if let Some((_, p)) = self
                .places_table
                .selected()
                .and_then(|i| self.places.get(i))
                .cloned()
                && self.load(&p)
            {
                self.focus_places = false;
            }
            return BrowserOutcome::None;
        }
        let Some(entry) = self.selected_entry().cloned() else {
            return BrowserOutcome::None;
        };
        if entry.is_parent {
            self.go_up();
        } else if entry.is_dir {
            self.load(&entry.path);
        } else {
            return BrowserOutcome::Chosen(entry.path);
        }
        BrowserOutcome::None
    }

    pub fn move_by(&mut self, delta: isize) {
        let (state, len) = if self.focus_places {
            (&mut self.places_table, self.places.len())
        } else {
            (&mut self.table, self.visible.len())
        };
        if len == 0 {
            return;
        }
        let cur = state.selected().unwrap_or(0) as isize;
        state.select(Some((cur + delta).clamp(0, len as isize - 1) as usize));
    }

    pub fn handle_key(&mut self, key: KeyEvent) -> BrowserOutcome {
        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
        match key.code {
            KeyCode::Esc if !self.filter.is_empty() => {
                self.filter.clear();
                self.refresh();
            }
            KeyCode::Esc => return BrowserOutcome::Close,
            KeyCode::Enter => return self.activate(),
            KeyCode::Right if !self.focus_places => {
                if self.selected_entry().is_some_and(|e| e.is_dir) {
                    return self.activate();
                }
            }
            KeyCode::Right | KeyCode::Tab | KeyCode::BackTab => {
                self.focus_places = !self.focus_places
            }
            KeyCode::Left if self.focus_places => self.focus_places = false,
            KeyCode::Left => self.go_up(),
            KeyCode::Backspace if !self.filter.is_empty() => {
                self.filter.pop();
                self.refresh();
            }
            KeyCode::Backspace => self.go_up(),
            KeyCode::Up => self.move_by(-1),
            KeyCode::Down => self.move_by(1),
            KeyCode::PageUp => self.move_by(-15),
            KeyCode::PageDown => self.move_by(15),
            KeyCode::Home => self.move_by(-1_000_000),
            KeyCode::End => self.move_by(1_000_000),
            KeyCode::Char('a') if ctrl => {
                self.show_all = !self.show_all;
                self.refresh();
            }
            KeyCode::Char('h') if ctrl => {
                if let Some(home) = dirs::home_dir() {
                    self.load(&home);
                }
            }
            KeyCode::Char(c) if !ctrl => {
                if self.focus_places {
                    self.focus_places = false;
                }
                self.filter.push(c);
                self.table.select(Some(0));
                self.refresh();
            }
            _ => {}
        }
        BrowserOutcome::None
    }
}

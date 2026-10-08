//! Top level application state and event handling.

use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::mpsc::{Receiver, TryRecvError, channel};
use std::time::{Duration, Instant};

use ratatui::crossterm::event::{
    Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers, MouseButton, MouseEvent, MouseEventKind,
};
use ratatui::layout::Rect;

use super::browser::{BrowserOutcome, FileBrowser};
use super::input::TextInput;
use super::results::{Action, DetailRow, Results, RowKind, Tab};
use crate::config::{Settings, clean_path, parse_list};
use crate::diff::DiffOptions;
use crate::engine::{self, CompareRequest, Progress};
use crate::export;
use crate::report::Report;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Field {
    Source,
    Dest,
    Created,
    Updated,
    Ignore,
    LooseNumbers,
    ArrayOrder,
    Start,
}

impl Field {
    pub const ALL: [Field; 8] = [
        Field::Source,
        Field::Dest,
        Field::Created,
        Field::Updated,
        Field::Ignore,
        Field::LooseNumbers,
        Field::ArrayOrder,
        Field::Start,
    ];

    fn step(self, delta: isize) -> Field {
        let i = Field::ALL.iter().position(|f| *f == self).unwrap() as isize;
        Field::ALL[(i + delta).rem_euclid(Field::ALL.len() as isize) as usize]
    }
}

pub struct Setup {
    pub source: TextInput,
    pub dest: TextInput,
    pub created: TextInput,
    pub updated: TextInput,
    pub ignore: TextInput,
    pub loose_numbers: bool,
    pub ignore_array_order: bool,
    pub focus: Field,
    pub error: Option<String>,
}

/// What we know about a path typed into one of the file boxes.
pub enum PathCheck {
    Empty,
    Missing,
    IsFolder,
    Ok {
        size: u64,
        modified: Option<std::time::SystemTime>,
    },
}

pub fn check_path(raw: &str) -> PathCheck {
    if raw.trim().is_empty() {
        return PathCheck::Empty;
    }
    match std::fs::metadata(clean_path(raw)) {
        Ok(m) if m.is_dir() => PathCheck::IsFolder,
        Ok(m) => PathCheck::Ok {
            size: m.len(),
            modified: m.modified().ok(),
        },
        Err(_) => PathCheck::Missing,
    }
}

impl Setup {
    fn from_settings(s: &Settings) -> Self {
        Setup {
            source: TextInput::new(s.last_source.clone()),
            dest: TextInput::new(s.last_dest.clone()),
            created: TextInput::new(s.created_field.clone()),
            updated: TextInput::new(s.updated_field.clone()),
            ignore: TextInput::new(s.ignore_fields.clone()),
            loose_numbers: s.loose_numbers,
            ignore_array_order: s.ignore_array_order,
            focus: Field::Source,
            error: None,
        }
    }

    pub fn input_mut(&mut self, f: Field) -> Option<&mut TextInput> {
        match f {
            Field::Source => Some(&mut self.source),
            Field::Dest => Some(&mut self.dest),
            Field::Created => Some(&mut self.created),
            Field::Updated => Some(&mut self.updated),
            Field::Ignore => Some(&mut self.ignore),
            _ => None,
        }
    }

    fn request(&self) -> Result<CompareRequest, String> {
        let source = clean_path(&self.source.value);
        let dest = clean_path(&self.dest.value);
        for (label, raw) in [
            ("source", &self.source.value),
            ("destination", &self.dest.value),
        ] {
            match check_path(raw) {
                PathCheck::Empty => return Err(format!("Please choose the {label} file first.")),
                PathCheck::Missing => {
                    return Err(format!(
                        "The {label} file could not be found. Check the path or press Enter on the box to browse."
                    ));
                }
                PathCheck::IsFolder => {
                    return Err(format!(
                        "The {label} path is a folder. Please pick a file inside it."
                    ));
                }
                PathCheck::Ok { .. } => {}
            }
        }
        let same = match (std::fs::canonicalize(&source), std::fs::canonicalize(&dest)) {
            (Ok(a), Ok(b)) => a == b,
            _ => source == dest,
        };
        if same {
            return Err(
                "Source and destination are the same file. Please pick two different files.".into(),
            );
        }
        Ok(CompareRequest {
            source,
            dest,
            created_field: self.created.value.trim().to_string(),
            updated_field: self.updated.value.trim().to_string(),
            options: DiffOptions {
                ignore: parse_list(&self.ignore.value),
                loose_numbers: self.loose_numbers,
                ignore_array_order: self.ignore_array_order,
                ..DiffOptions::default()
            },
        })
    }
}

pub struct Running {
    pub progress: Arc<Progress>,
    pub rx: Receiver<Result<Report, String>>,
    pub started: Instant,
    pub request: CompareRequest,
}

pub enum Screen {
    Setup,
    Running(Running),
    Results(Box<Results>),
}

#[derive(Debug, Clone)]
pub enum PopupKind {
    Info,
    Error,
    Exported { html: PathBuf, folder: PathBuf },
    ConfirmQuit,
    ConfirmNew,
}

pub struct Popup {
    pub title: String,
    pub body: Vec<String>,
    pub kind: PopupKind,
    pub scroll: u16,
}

/// Something on screen that reacts to the mouse. Registered while drawing.
#[derive(Debug, Clone, Copy)]
pub enum Clickable {
    Tab(Tab),
    Rows { offset: usize },
    SetupField(Field),
    BrowserFiles { offset: usize },
    BrowserPlaces { offset: usize },
    ClosePopup,
    Nothing,
}

pub struct App {
    pub settings: Settings,
    pub screen: Screen,
    pub setup: Setup,
    pub browser: Option<FileBrowser>,
    pub browser_target: Field,
    pub popup: Option<Popup>,
    pub show_help: bool,
    pub should_quit: bool,
    pub clickables: Vec<(Rect, Clickable)>,
    pub toast: Option<(String, Instant)>,
    last_click: Option<(Instant, u16, u16)>,
}

impl App {
    pub fn new(settings: Settings) -> Self {
        let setup = Setup::from_settings(&settings);
        App {
            settings,
            screen: Screen::Setup,
            setup,
            browser: None,
            browser_target: Field::Source,
            popup: None,
            show_help: false,
            should_quit: false,
            clickables: Vec::new(),
            toast: None,
            last_click: None,
        }
    }

    pub fn set_paths(&mut self, source: Option<&Path>, dest: Option<&Path>) {
        if let Some(s) = source {
            self.setup.source.set(s.display().to_string());
        }
        if let Some(d) = dest {
            self.setup.dest.set(d.display().to_string());
        }
        self.setup.focus = if self.setup.source.value.is_empty() {
            Field::Source
        } else if self.setup.dest.value.is_empty() {
            Field::Dest
        } else {
            Field::Start
        };
    }

    pub fn apply_overrides(
        &mut self,
        created: Option<String>,
        updated: Option<String>,
        ignore: Option<String>,
        loose: bool,
        array_order: bool,
    ) {
        if let Some(c) = created {
            self.setup.created.set(c);
        }
        if let Some(u) = updated {
            self.setup.updated.set(u);
        }
        if let Some(i) = ignore {
            self.setup.ignore.set(i);
        }
        self.setup.loose_numbers |= loose;
        self.setup.ignore_array_order |= array_order;
    }

    pub fn is_running(&self) -> bool {
        matches!(self.screen, Screen::Running(_))
    }

    fn toast(&mut self, msg: impl Into<String>) {
        self.toast = Some((msg.into(), Instant::now()));
    }

    fn info(&mut self, title: &str, body: Vec<String>, kind: PopupKind) {
        self.popup = Some(Popup {
            title: title.into(),
            body,
            kind,
            scroll: 0,
        });
    }

    // ------------------------------------------------------------------
    // Comparison lifecycle

    pub fn start(&mut self) {
        let request = match self.setup.request() {
            Ok(r) => r,
            Err(e) => {
                self.setup.error = Some(e);
                return;
            }
        };
        self.setup.error = None;
        self.settings.last_source = self.setup.source.value.clone();
        self.settings.last_dest = self.setup.dest.value.clone();
        self.settings.created_field = self.setup.created.value.clone();
        self.settings.updated_field = self.setup.updated.value.clone();
        self.settings.ignore_fields = self.setup.ignore.value.clone();
        self.settings.loose_numbers = self.setup.loose_numbers;
        self.settings.ignore_array_order = self.setup.ignore_array_order;
        self.settings.save();

        let progress = Arc::new(Progress::default());
        let (tx, rx) = channel();
        let worker_progress = progress.clone();
        let worker_request = request.clone();
        std::thread::spawn(move || {
            let result = engine::run(&worker_request, &worker_progress);
            let _ = tx.send(result);
        });
        self.screen = Screen::Running(Running {
            progress,
            rx,
            started: Instant::now(),
            request,
        });
    }

    /// Called regularly from the main loop.
    pub fn tick(&mut self) {
        if let Some((_, at)) = &self.toast
            && at.elapsed() > Duration::from_secs(3)
        {
            self.toast = None;
        }
        let Screen::Running(run) = &self.screen else {
            return;
        };
        match run.rx.try_recv() {
            Ok(Ok(report)) => self.screen = Screen::Results(Box::new(Results::new(report))),
            Ok(Err(msg)) => {
                let cancelled = run
                    .progress
                    .cancel
                    .load(std::sync::atomic::Ordering::Relaxed);
                self.screen = Screen::Setup;
                if !cancelled {
                    self.info("Could not compare the files", vec![msg], PopupKind::Error);
                } else {
                    self.toast("Comparison cancelled");
                }
            }
            Err(TryRecvError::Empty) => {}
            Err(TryRecvError::Disconnected) => {
                self.screen = Screen::Setup;
                self.info(
                    "Something went wrong",
                    vec!["The comparison stopped unexpectedly. Please try again, and report the problem if it keeps happening.".into()],
                    PopupKind::Error,
                );
            }
        }
    }

    fn export(&mut self) {
        let Screen::Results(res) = &self.screen else {
            return;
        };
        let parent = export::default_parent(&res.report);
        match export::export(&res.report, &parent) {
            Ok(out) => {
                let body = vec![
                    "The report was saved to this folder:".into(),
                    String::new(),
                    out.folder.display().to_string(),
                    String::new(),
                    "report.html          summary to read or share (opens in a web browser)".into(),
                    "differences.csv      every difference, one per row (opens in Excel)".into(),
                    "objects.csv          every object with its status and dates".into(),
                    "records-by-day.csv   counts per day".into(),
                ];
                self.info(
                    "Report saved",
                    body,
                    PopupKind::Exported {
                        html: out.html,
                        folder: out.folder,
                    },
                );
            }
            Err(e) => self.info(
                "Could not save the report",
                vec![format!("Writing to {} failed: {e}", parent.display())],
                PopupKind::Error,
            ),
        }
    }

    fn copy_to_clipboard(&mut self, text: &str) {
        // OSC 52 asks the terminal to put text on the clipboard; supported by
        // Windows Terminal, iTerm2, kitty, WezTerm, Alacritty, recent VS Code...
        let seq = format!("\x1b]52;c;{}\x07", base64(text.as_bytes()));
        let mut out = std::io::stdout();
        let _ = out.write_all(seq.as_bytes());
        let _ = out.flush();
        self.toast(format!("Copied {text}"));
    }

    // ------------------------------------------------------------------
    // Events

    pub fn handle_event(&mut self, ev: Event) {
        match ev {
            Event::Key(key) if key.kind != KeyEventKind::Release => self.handle_key(key),
            Event::Paste(text) => self.handle_paste(&text),
            Event::Mouse(m) => self.handle_mouse(m),
            _ => {}
        }
    }

    fn handle_paste(&mut self, text: &str) {
        if let Some(b) = &mut self.browser {
            // A path pasted into the picker: jump to it.
            let p = clean_path(text);
            if p.is_dir() {
                b.load(&p);
            } else if p.is_file() {
                let target = self.browser_target;
                self.browser = None;
                self.choose_file(target, p);
            }
            return;
        }
        if let Screen::Setup = self.screen {
            let focus = self.setup.focus;
            let is_path = matches!(focus, Field::Source | Field::Dest);
            if let Some(input) = self.setup.input_mut(focus) {
                if is_path {
                    let p = clean_path(text);
                    input.set(p.display().to_string());
                } else {
                    input.insert_str(text.trim());
                }
            }
        } else if let Screen::Results(res) = &mut self.screen
            && let super::results::View::Records(l) = res.current_mut()
            && l.searching
        {
            l.search.push_str(text.trim());
            let report = &res.report;
            if let Some(super::results::View::Records(l)) = res.stacks[res.tab.index()].last_mut() {
                l.refresh(report);
            }
        }
    }

    fn handle_key(&mut self, key: KeyEvent) {
        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
        if ctrl && key.code == KeyCode::Char('c') {
            self.should_quit = true;
            return;
        }
        if self.show_help {
            self.show_help = false;
            return;
        }
        if self.popup.is_some() {
            self.handle_popup_key(key);
            return;
        }
        if self.browser.is_some() {
            self.handle_browser_key(key);
            return;
        }
        match &self.screen {
            Screen::Setup => self.handle_setup_key(key),
            Screen::Running(run) => {
                if matches!(key.code, KeyCode::Esc | KeyCode::Char('q')) {
                    run.progress
                        .cancel
                        .store(true, std::sync::atomic::Ordering::Relaxed);
                }
            }
            Screen::Results(_) => self.handle_results_key(key),
        }
    }

    fn handle_popup_key(&mut self, key: KeyEvent) {
        let Some(popup) = &mut self.popup else { return };
        match (&popup.kind, key.code) {
            (PopupKind::ConfirmQuit, KeyCode::Char('y' | 'Y') | KeyCode::Enter) => {
                self.should_quit = true
            }
            (PopupKind::ConfirmNew, KeyCode::Char('y' | 'Y') | KeyCode::Enter) => {
                self.popup = None;
                self.screen = Screen::Setup;
                self.setup.focus = Field::Source;
            }
            (PopupKind::Exported { html, .. }, KeyCode::Char('o')) => {
                let html = html.clone();
                self.popup = None;
                match export::open_in_system(&html) {
                    Ok(()) => self.toast("Opening the report in your browser…"),
                    Err(e) => self.toast(format!("Could not open the report: {e}")),
                }
            }
            (PopupKind::Exported { folder, .. }, KeyCode::Char('f')) => {
                let folder = folder.clone();
                self.popup = None;
                if let Err(e) = export::open_in_system(&folder) {
                    self.toast(format!("Could not open the folder: {e}"));
                }
            }
            (_, KeyCode::Up) => popup.scroll = popup.scroll.saturating_sub(1),
            (_, KeyCode::Down) => popup.scroll = popup.scroll.saturating_add(1),
            (_, KeyCode::PageUp) => popup.scroll = popup.scroll.saturating_sub(10),
            (_, KeyCode::PageDown) => popup.scroll = popup.scroll.saturating_add(10),
            (_, KeyCode::Char('c')) if matches!(popup.kind, PopupKind::Info) => {
                // Copy the full value shown in a value popup.
                let text = popup.body.join("\n");
                self.copy_to_clipboard(&text);
            }
            (PopupKind::ConfirmQuit | PopupKind::ConfirmNew, _) => self.popup = None,
            (_, KeyCode::Esc | KeyCode::Enter | KeyCode::Char('q') | KeyCode::Char(' ')) => {
                self.popup = None
            }
            _ => {}
        }
    }

    fn open_browser(&mut self, target: Field) {
        let current = match target {
            Field::Dest => &self.setup.dest.value,
            _ => &self.setup.source.value,
        };
        let other = match target {
            Field::Dest => &self.setup.source.value,
            _ => &self.setup.dest.value,
        };
        let cur_path = (!current.trim().is_empty()).then(|| clean_path(current));
        let start = cur_path
            .as_ref()
            .filter(|p| p.exists())
            .and_then(|p| {
                if p.is_dir() {
                    Some(p.clone())
                } else {
                    p.parent().map(Path::to_path_buf)
                }
            })
            .or_else(|| self.settings.last_browse_dir.clone().filter(|p| p.is_dir()))
            .or_else(|| {
                let o = clean_path(other);
                o.parent().filter(|p| p.is_dir()).map(Path::to_path_buf)
            });
        self.browser = Some(FileBrowser::open(start, cur_path.as_deref()));
        self.browser_target = target;
    }

    fn choose_file(&mut self, target: Field, path: PathBuf) {
        if let Some(dir) = path.parent() {
            self.settings.last_browse_dir = Some(dir.to_path_buf());
        }
        let text = path.display().to_string();
        match target {
            Field::Dest => self.setup.dest.set(text),
            _ => self.setup.source.set(text),
        }
        self.setup.error = None;
        let source_ok = matches!(check_path(&self.setup.source.value), PathCheck::Ok { .. });
        let dest_ok = matches!(check_path(&self.setup.dest.value), PathCheck::Ok { .. });
        self.setup.focus = match (target, source_ok, dest_ok) {
            (Field::Source, _, false) => Field::Dest,
            (_, false, _) => Field::Source,
            _ => Field::Start,
        };
    }

    fn handle_browser_key(&mut self, key: KeyEvent) {
        let Some(b) = &mut self.browser else { return };
        if key.code == KeyCode::F(1) || (key.code == KeyCode::Char('?') && b.filter.is_empty()) {
            self.show_help = true;
            return;
        }
        match b.handle_key(key) {
            BrowserOutcome::None => {}
            BrowserOutcome::Close => self.browser = None,
            BrowserOutcome::Chosen(p) => {
                self.browser = None;
                let target = self.browser_target;
                self.choose_file(target, p);
            }
        }
    }

    fn handle_setup_key(&mut self, key: KeyEvent) {
        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
        let s = &mut self.setup;
        match key.code {
            KeyCode::F(1) => self.show_help = true,
            KeyCode::Char('?')
                if !matches!(
                    s.focus,
                    Field::Source | Field::Dest | Field::Created | Field::Updated | Field::Ignore
                ) =>
            {
                self.show_help = true
            }
            KeyCode::F(5) => self.start(),
            KeyCode::Char('r') if ctrl => self.start(),
            KeyCode::Char('d') if ctrl => self.load_demo(),
            KeyCode::F(2) | KeyCode::Char('o') if key.code == KeyCode::F(2) || ctrl => {
                let target = if s.focus == Field::Dest {
                    Field::Dest
                } else {
                    Field::Source
                };
                self.open_browser(target);
            }
            KeyCode::Esc => self.info(
                "Quit?",
                vec!["Do you want to close the application?".into()],
                PopupKind::ConfirmQuit,
            ),
            KeyCode::Tab | KeyCode::Down => s.focus = s.focus.step(1),
            KeyCode::BackTab | KeyCode::Up => s.focus = s.focus.step(-1),
            KeyCode::Enter => match s.focus {
                Field::Source | Field::Dest => {
                    let target = s.focus;
                    self.open_browser(target);
                }
                Field::LooseNumbers => s.loose_numbers = !s.loose_numbers,
                Field::ArrayOrder => s.ignore_array_order = !s.ignore_array_order,
                Field::Start => self.start(),
                _ => s.focus = s.focus.step(1),
            },
            KeyCode::Char(' ') if s.focus == Field::LooseNumbers => {
                s.loose_numbers = !s.loose_numbers
            }
            KeyCode::Char(' ') if s.focus == Field::ArrayOrder => {
                s.ignore_array_order = !s.ignore_array_order
            }
            _ => {
                let focus = s.focus;
                if let Some(input) = s.input_mut(focus)
                    && input.handle_key(key)
                {
                    s.error = None;
                }
            }
        }
    }

    pub fn load_demo(&mut self) {
        let dir = std::env::temp_dir().join("mongo-compare-demo");
        match crate::demo::generate(&dir, 2500) {
            Ok((src, dst)) => {
                self.set_paths(Some(&src), Some(&dst));
                self.toast("Sample files created. Press Enter on Start to compare them.");
            }
            Err(e) => self.info(
                "Could not create sample files",
                vec![e.to_string()],
                PopupKind::Error,
            ),
        }
    }

    fn handle_results_key(&mut self, key: KeyEvent) {
        let Screen::Results(res) = &mut self.screen else {
            return;
        };
        let searching = matches!(res.current(), super::results::View::Records(l) if l.searching);
        if !searching {
            match key.code {
                KeyCode::Char('q') => {
                    self.info(
                        "Quit?",
                        vec!["Do you want to close the application?".into()],
                        PopupKind::ConfirmQuit,
                    );
                    return;
                }
                KeyCode::Char('?') | KeyCode::F(1) => {
                    self.show_help = true;
                    return;
                }
                KeyCode::Char('e') => {
                    self.export();
                    return;
                }
                KeyCode::Char('n') => {
                    self.info(
                        "New comparison?",
                        vec!["Go back to choose other files? The current results will be closed (export them first with e if you want to keep them).".into()],
                        PopupKind::ConfirmNew,
                    );
                    return;
                }
                KeyCode::Tab => {
                    res.tab = Tab::ALL[(res.tab.index() + 1) % Tab::ALL.len()];
                    return;
                }
                KeyCode::BackTab => {
                    res.tab = Tab::ALL[(res.tab.index() + Tab::ALL.len() - 1) % Tab::ALL.len()];
                    return;
                }
                KeyCode::Char(c @ '1'..='5') => {
                    res.tab = Tab::ALL[(c as u8 - b'1') as usize];
                    return;
                }
                _ => {}
            }
        }
        match res.handle_key(key) {
            Action::None => {}
            Action::Copy(id) => self.copy_to_clipboard(&id),
            Action::ShowValue(row, id) => self.show_value(row, id),
        }
    }

    fn show_value(&mut self, row: DetailRow, id: String) {
        let what = match row.kind {
            RowKind::Same => "Same in both files".to_string(),
            RowKind::Ignored => "Ignored (as requested in the options)".to_string(),
            RowKind::Diff(k) => k.label().to_string(),
        };
        let fmt_side = |v: &Option<String>, t: Option<&str>| match v {
            Some(v) => format!("{v}    ({})", t.unwrap_or("?")),
            None => "(field not present)".to_string(),
        };
        let body = vec![
            format!("Object:  {id}"),
            format!("Field:   {}", row.path),
            format!("Result:  {what}"),
            String::new(),
            "SOURCE".into(),
            fmt_side(&row.source, row.source_type),
            String::new(),
            "DESTINATION".into(),
            fmt_side(&row.dest, row.dest_type),
        ];
        self.info("Field value", body, PopupKind::Info);
    }

    // ------------------------------------------------------------------
    // Mouse

    fn handle_mouse(&mut self, m: MouseEvent) {
        match m.kind {
            MouseEventKind::ScrollUp | MouseEventKind::ScrollDown => {
                let code = if m.kind == MouseEventKind::ScrollUp {
                    KeyCode::Up
                } else {
                    KeyCode::Down
                };
                if let Some(p) = &mut self.popup {
                    p.scroll = if code == KeyCode::Up {
                        p.scroll.saturating_sub(1)
                    } else {
                        p.scroll.saturating_add(1)
                    };
                } else if let Some(b) = &mut self.browser {
                    // Scroll the pane under the pointer.
                    let over_places = self.clickables.iter().any(|(r, c)| {
                        matches!(c, Clickable::BrowserPlaces { .. }) && contains(r, m.column, m.row)
                    });
                    b.focus_places = over_places;
                    b.move_by(if code == KeyCode::Up { -1 } else { 1 });
                } else if let Screen::Results(res) = &mut self.screen {
                    res.move_by(if code == KeyCode::Up { -1 } else { 1 });
                }
            }
            MouseEventKind::Down(MouseButton::Left) => self.handle_click(m.column, m.row),
            _ => {}
        }
    }

    fn handle_click(&mut self, x: u16, y: u16) {
        let now = Instant::now();
        let double = self.last_click.is_some_and(|(t, lx, ly)| {
            lx == x && ly == y && now.duration_since(t) < Duration::from_millis(450)
        });
        self.last_click = Some((now, x, y));

        let Some((rect, target)) = self
            .clickables
            .iter()
            .rev()
            .find(|(r, _)| contains(r, x, y))
            .copied()
        else {
            return;
        };
        let row = (y - rect.y) as usize;
        match target {
            Clickable::Nothing => {}
            Clickable::ClosePopup => {
                self.popup = None;
                self.show_help = false;
            }
            Clickable::Tab(tab) => {
                if let Screen::Results(res) = &mut self.screen {
                    res.tab = tab;
                }
            }
            Clickable::Rows { offset } => {
                if let Screen::Results(res) = &mut self.screen {
                    let idx = offset + row;
                    let already = res.selected_row() == Some(idx);
                    res.select_row(idx);
                    if (already || double)
                        && let Action::ShowValue(r, id) = res.activate()
                    {
                        self.show_value(r, id);
                    }
                }
            }
            Clickable::SetupField(f) => {
                let already = self.setup.focus == f;
                self.setup.focus = f;
                match f {
                    Field::Source | Field::Dest if already || double => self.open_browser(f),
                    Field::LooseNumbers => self.setup.loose_numbers = !self.setup.loose_numbers,
                    Field::ArrayOrder => {
                        self.setup.ignore_array_order = !self.setup.ignore_array_order
                    }
                    Field::Start => self.start(),
                    _ => {}
                }
            }
            Clickable::BrowserFiles { offset } | Clickable::BrowserPlaces { offset } => {
                let Some(b) = &mut self.browser else { return };
                let places = matches!(target, Clickable::BrowserPlaces { .. });
                let idx = offset + row;
                let (state, len) = if places {
                    (&mut b.places_table, b.places.len())
                } else {
                    (&mut b.table, b.visible.len())
                };
                if idx >= len {
                    return;
                }
                let already = state.selected() == Some(idx) && b.focus_places == places;
                state.select(Some(idx));
                b.focus_places = places;
                // Places open with one click, files and folders with a second click.
                if (places || already || double)
                    && let BrowserOutcome::Chosen(p) = b.activate()
                {
                    self.browser = None;
                    let target = self.browser_target;
                    self.choose_file(target, p);
                }
            }
        }
    }
}

fn contains(r: &Rect, x: u16, y: u16) -> bool {
    x >= r.x && x < r.x + r.width && y >= r.y && y < r.y + r.height
}

fn base64(data: &[u8]) -> String {
    const T: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(data.len().div_ceil(3) * 4);
    for chunk in data.chunks(3) {
        let b = [
            chunk[0],
            *chunk.get(1).unwrap_or(&0),
            *chunk.get(2).unwrap_or(&0),
        ];
        let n = (b[0] as u32) << 16 | (b[1] as u32) << 8 | b[2] as u32;
        out.push(T[(n >> 18) as usize & 63] as char);
        out.push(T[(n >> 12) as usize & 63] as char);
        out.push(if chunk.len() > 1 {
            T[(n >> 6) as usize & 63] as char
        } else {
            '='
        });
        out.push(if chunk.len() > 2 {
            T[n as usize & 63] as char
        } else {
            '='
        });
    }
    out
}

#[cfg(test)]
mod tests {
    #[test]
    fn base64_encodes() {
        assert_eq!(super::base64(b"Man"), "TWFu");
        assert_eq!(super::base64(b"Ma"), "TWE=");
        assert_eq!(super::base64(b"M"), "TQ==");
    }
}

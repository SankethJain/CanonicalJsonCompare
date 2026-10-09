//! State and navigation of the results screen.

use std::collections::HashMap;

use ratatui::crossterm::event::{KeyCode, KeyEvent};
use ratatui::widgets::TableState;

use crate::diff::{self, DiffKind};
use crate::extjson;
use crate::report::{DateBasis, MonthBucket, Report, Status, build_date_index};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Tab {
    Summary,
    Mismatches,
    Dates,
    Fields,
    Issues,
}

impl Tab {
    pub const ALL: [Tab; 5] = [
        Tab::Summary,
        Tab::Mismatches,
        Tab::Dates,
        Tab::Fields,
        Tab::Issues,
    ];

    pub fn index(self) -> usize {
        Tab::ALL.iter().position(|t| *t == self).unwrap()
    }
}

/// A filterable, searchable list of objects.
pub struct RecordList {
    pub title: String,
    pub all: Vec<usize>,
    pub filter: Option<Status>,
    pub search: String,
    pub searching: bool,
    pub visible: Vec<usize>,
    pub table: TableState,
    /// Status filters worth offering for this list, with their counts.
    pub filters: Vec<(Option<Status>, usize)>,
}

impl RecordList {
    pub fn new(title: impl Into<String>, all: Vec<usize>, report: &Report) -> Self {
        let mut counts: HashMap<Status, usize> = HashMap::new();
        for &i in &all {
            *counts.entry(report.records[i].status).or_default() += 1;
        }
        let mut filters = vec![(None, all.len())];
        for s in [
            Status::Different,
            Status::OnlyInSource,
            Status::OnlyInDest,
            Status::Matched,
        ] {
            if let Some(&n) = counts.get(&s)
                && n != all.len()
            {
                filters.push((Some(s), n));
            }
        }
        let mut list = RecordList {
            title: title.into(),
            all,
            filter: None,
            search: String::new(),
            searching: false,
            visible: Vec::new(),
            table: TableState::default(),
            filters,
        };
        list.refresh(report);
        list
    }

    pub fn refresh(&mut self, report: &Report) {
        let needle = self.search.trim().to_lowercase();
        self.visible = self
            .all
            .iter()
            .copied()
            .filter(|&i| {
                let r = &report.records[i];
                self.filter.is_none_or(|f| r.status == f)
                    && (needle.is_empty() || r.id.to_lowercase().contains(&needle))
            })
            .collect();
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

    pub fn cycle_filter(&mut self, report: &Report) {
        let pos = self
            .filters
            .iter()
            .position(|(f, _)| *f == self.filter)
            .unwrap_or(0);
        self.filter = self.filters[(pos + 1) % self.filters.len()].0;
        self.table.select(Some(0));
        self.refresh(report);
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RowKind {
    Same,
    Ignored,
    Diff(DiffKind),
}

#[derive(Debug, Clone)]
pub struct DetailRow {
    pub path: String,
    pub source: Option<String>,
    pub dest: Option<String>,
    pub source_type: Option<&'static str>,
    pub dest_type: Option<&'static str>,
    pub kind: RowKind,
}

/// One object, its differences or all its fields side by side.
pub struct Detail {
    pub siblings: Vec<usize>,
    pub pos: usize,
    pub all_fields: bool,
    pub rows: Vec<DetailRow>,
    pub table: TableState,
}

impl Detail {
    pub fn new(siblings: Vec<usize>, pos: usize, report: &Report) -> Self {
        let mut d = Detail {
            siblings,
            pos,
            all_fields: false,
            rows: Vec::new(),
            table: TableState::default(),
        };
        d.rebuild(report);
        d
    }

    pub fn record(&self) -> usize {
        self.siblings[self.pos]
    }

    pub fn rebuild(&mut self, report: &Report) {
        let r = &report.records[self.record()];
        self.rows = match r.status {
            Status::Matched => Vec::new(),
            Status::Different if !self.all_fields => r
                .diffs
                .iter()
                .map(|d| DetailRow {
                    path: d.path.clone(),
                    source: d.source.clone(),
                    dest: d.dest.clone(),
                    source_type: d.source_type,
                    dest_type: d.dest_type,
                    kind: RowKind::Diff(d.kind),
                })
                .collect(),
            _ => all_field_rows(r.src_doc().as_ref(), r.dst_doc().as_ref(), report),
        };
        self.table
            .select(if self.rows.is_empty() { None } else { Some(0) });
        *self.table.offset_mut() = 0;
    }
}

fn all_field_rows(
    src: Option<&serde_json::Value>,
    dst: Option<&serde_json::Value>,
    report: &Report,
) -> Vec<DetailRow> {
    let opts = &report.options;
    let a = src.map(diff::flatten).unwrap_or_default();
    let b = dst.map(diff::flatten).unwrap_or_default();
    let b_map: HashMap<&str, &serde_json::Value> =
        b.iter().map(|(p, v)| (p.as_str(), *v)).collect();
    let a_keys: std::collections::HashSet<&str> = a.iter().map(|(p, _)| p.as_str()).collect();
    let show = |v: &serde_json::Value| extjson::display(v);
    let mut rows = Vec::new();
    for (path, va) in &a {
        let vb = b_map.get(path.as_str()).copied();
        let kind = if diff::path_ignored(opts, path) {
            RowKind::Ignored
        } else {
            match vb {
                None => RowKind::Diff(DiffKind::OnlyInSource),
                Some(vb) => match diff::diff_documents(va, vb, opts).diffs.first() {
                    Some(d) => RowKind::Diff(d.kind),
                    None => RowKind::Same,
                },
            }
        };
        rows.push(DetailRow {
            path: path.clone(),
            source: Some(show(va)),
            dest: vb.map(show),
            source_type: Some(extjson::btype(va).name()),
            dest_type: vb.map(|v| extjson::btype(v).name()),
            kind,
        });
    }
    for (path, vb) in &b {
        if a_keys.contains(path.as_str()) {
            continue;
        }
        let kind = if diff::path_ignored(opts, path) {
            RowKind::Ignored
        } else {
            RowKind::Diff(DiffKind::OnlyInDest)
        };
        rows.push(DetailRow {
            path: path.clone(),
            source: None,
            dest: Some(show(vb)),
            source_type: None,
            dest_type: Some(extjson::btype(vb).name()),
            kind,
        });
    }
    rows
}

pub enum View {
    Summary(TableState),
    Records(RecordList),
    Months(TableState),
    Days { month: usize, table: TableState },
    Fields(TableState),
    Issues { scroll: u16 },
    Detail(Detail),
}

impl View {
    pub fn crumb(&self, results: &Results) -> String {
        match self {
            View::Summary(_) => "Summary".into(),
            View::Records(l) => l.title.clone(),
            View::Months(_) => "By date".into(),
            View::Days { month, .. } => results.months[*month].bucket.label.clone(),
            View::Fields(_) => "Fields".into(),
            View::Issues { .. } => "Issues".into(),
            View::Detail(d) => format!("Object {}", results.report.records[d.record()].id),
        }
    }
}

pub struct Results {
    pub report: Report,
    pub tab: Tab,
    pub stacks: [Vec<View>; 5],
    pub basis: DateBasis,
    pub months: Vec<MonthBucket>,
    pub problems_only: bool,
}

/// What the app should do after a key was handled by the results screen.
pub enum Action {
    None,
    Copy(String),
    ShowValue(DetailRow, String),
}

impl Results {
    pub fn new(report: Report) -> Self {
        let mismatched = report.mismatched();
        let list = RecordList::new("Mismatches", mismatched, &report);
        let basis = if report.created_field.name.is_some() {
            DateBasis::Created
        } else if report.updated_field.name.is_some() {
            DateBasis::Updated
        } else {
            DateBasis::IdTime
        };
        let months = build_date_index(&report.records, basis);
        let first =
            |n: usize| TableState::default().with_selected(if n > 0 { Some(0) } else { None });
        let stacks = [
            vec![View::Summary(first(4))],
            vec![View::Records(list)],
            vec![View::Months(first(months.len()))],
            vec![View::Fields(first(report.fields.len()))],
            vec![View::Issues { scroll: 0 }],
        ];
        Results {
            report,
            tab: Tab::Summary,
            stacks,
            basis,
            months,
            problems_only: false,
        }
    }

    pub fn stack(&self) -> &Vec<View> {
        &self.stacks[self.tab.index()]
    }

    pub fn current(&self) -> &View {
        self.stack().last().unwrap()
    }

    pub fn current_mut(&mut self) -> &mut View {
        self.stacks[self.tab.index()].last_mut().unwrap()
    }

    pub fn breadcrumb(&self) -> Vec<String> {
        self.stack().iter().map(|v| v.crumb(self)).collect()
    }

    pub fn issue_count(&self) -> u64 {
        let s = &self.report.source;
        let d = &self.report.dest;
        s.error_count
            + d.error_count
            + s.duplicate_count
            + d.duplicate_count
            + s.without_id
            + d.without_id
    }

    /// Months shown in the date view (all, or only those with problems).
    pub fn visible_months(&self) -> Vec<usize> {
        (0..self.months.len())
            .filter(|&i| !self.problems_only || has_problem(&self.months[i].bucket))
            .collect()
    }

    pub fn visible_days(&self, month: usize) -> Vec<usize> {
        (0..self.months[month].days.len())
            .filter(|&i| !self.problems_only || has_problem(&self.months[month].days[i]))
            .collect()
    }

    pub fn set_basis(&mut self, basis: DateBasis) {
        self.basis = basis;
        self.months = build_date_index(&self.report.records, basis);
        self.reset_dates();
    }

    fn reset_dates(&mut self) {
        let n = self.visible_months().len();
        self.stacks[Tab::Dates.index()] = vec![View::Months(
            TableState::default().with_selected(if n > 0 { Some(0) } else { None }),
        )];
    }

    pub fn back(&mut self) -> bool {
        let stack = &mut self.stacks[self.tab.index()];
        if stack.len() > 1 {
            stack.pop();
            true
        } else {
            false
        }
    }

    fn push(&mut self, view: View) {
        self.stacks[self.tab.index()].push(view);
    }

    /// Number of selectable rows in the current view.
    pub fn row_count(&self) -> usize {
        match self.current() {
            View::Summary(_) => 4,
            View::Records(l) => l.visible.len(),
            View::Months(_) => self.visible_months().len(),
            View::Days { month, .. } => self.visible_days(*month).len(),
            View::Fields(_) => self.report.fields.len(),
            View::Issues { .. } => 0,
            View::Detail(d) => d.rows.len(),
        }
    }

    pub fn move_by(&mut self, delta: isize) {
        let len = self.row_count();
        let view = self.current_mut();
        let state = match view {
            View::Summary(t) | View::Months(t) | View::Fields(t) | View::Days { table: t, .. } => t,
            View::Records(l) => &mut l.table,
            View::Detail(d) => &mut d.table,
            View::Issues { scroll } => {
                *scroll = (*scroll as isize + delta).clamp(0, u16::MAX as isize) as u16;
                return;
            }
        };
        if len == 0 {
            state.select(None);
            return;
        }
        let cur = state.selected().unwrap_or(0) as isize;
        state.select(Some((cur + delta).clamp(0, len as isize - 1) as usize));
    }

    pub fn select_row(&mut self, row: usize) {
        if row >= self.row_count() {
            return;
        }
        match self.current_mut() {
            View::Summary(t) | View::Months(t) | View::Fields(t) | View::Days { table: t, .. } => {
                t.select(Some(row))
            }
            View::Records(l) => l.table.select(Some(row)),
            View::Detail(d) => d.table.select(Some(row)),
            View::Issues { .. } => {}
        }
    }

    pub fn selected_row(&self) -> Option<usize> {
        match self.current() {
            View::Summary(t) | View::Months(t) | View::Fields(t) | View::Days { table: t, .. } => {
                t.selected()
            }
            View::Records(l) => l.table.selected(),
            View::Detail(d) => d.table.selected(),
            View::Issues { .. } => None,
        }
    }

    /// The object id under the cursor, if any (for copying).
    pub fn current_id(&self) -> Option<String> {
        let idx = match self.current() {
            View::Records(l) => l.table.selected().and_then(|i| l.visible.get(i)).copied(),
            View::Detail(d) => Some(d.record()),
            _ => None,
        }?;
        Some(self.report.records[idx].id.to_string())
    }

    /// Enter / → on the current row: drill down one level.
    pub fn activate(&mut self) -> Action {
        let Some(row) = self.selected_row() else {
            return Action::None;
        };
        match self.current() {
            View::Summary(_) => {
                let status = [
                    Status::Different,
                    Status::OnlyInSource,
                    Status::OnlyInDest,
                    Status::Matched,
                ][row];
                let all: Vec<usize> = (0..self.report.records.len())
                    .filter(|&i| self.report.records[i].status == status)
                    .collect();
                if !all.is_empty() {
                    let list = RecordList::new(status_title(status), all, &self.report);
                    self.push(View::Records(list));
                }
            }
            View::Records(l) => {
                if !l.visible.is_empty() {
                    let detail = Detail::new(l.visible.clone(), row, &self.report);
                    self.push(View::Detail(detail));
                }
            }
            View::Months(_) => {
                let month = self.visible_months()[row];
                let days = self.months[month].days.len();
                if days == 1 && self.months[month].days[0].label == "(no date)" {
                    let list = self.day_list(month, 0);
                    self.push(View::Records(list));
                } else {
                    let n = self.visible_days(month).len();
                    self.push(View::Days {
                        month,
                        table: TableState::default().with_selected(if n > 0 {
                            Some(0)
                        } else {
                            None
                        }),
                    });
                }
            }
            View::Days { month, .. } => {
                let month = *month;
                let day = self.visible_days(month)[row];
                let list = self.day_list(month, day);
                self.push(View::Records(list));
            }
            View::Fields(_) => {
                let f = &self.report.fields[row];
                let list = RecordList::new(
                    format!("Objects where {} differs", f.path),
                    f.records.clone(),
                    &self.report,
                );
                self.push(View::Records(list));
            }
            View::Issues { .. } => {}
            View::Detail(d) => {
                let r = d.rows[row].clone();
                let id = self.report.records[d.record()].id.to_string();
                return Action::ShowValue(r, id);
            }
        }
        Action::None
    }

    fn day_list(&self, month: usize, day: usize) -> RecordList {
        let b = &self.months[month].days[day];
        // Problems first, then identical objects, each in file order.
        let mut records = b.records.clone();
        records.sort_by_key(|&i| self.report.records[i].status);
        RecordList::new(b.label.clone(), records, &self.report)
    }

    pub fn handle_key(&mut self, key: KeyEvent) -> Action {
        // Search box of a record list takes all typing while open.
        if let View::Records(l) = self.current_mut()
            && l.searching
        {
            match key.code {
                KeyCode::Esc => {
                    l.search.clear();
                    l.searching = false;
                }
                KeyCode::Enter | KeyCode::Down | KeyCode::Tab => l.searching = false,
                KeyCode::Backspace => {
                    l.search.pop();
                }
                KeyCode::Char(c) => {
                    l.search.push(c);
                    l.table.select(Some(0));
                }
                _ => {}
            }
            // Borrow juggling: refresh needs the report.
            let report = &self.report;
            if let Some(View::Records(l)) = self.stacks[self.tab.index()].last_mut() {
                l.refresh(report);
            }
            return Action::None;
        }

        let page = 15;
        match key.code {
            KeyCode::Up => self.move_by(-1),
            KeyCode::Down => self.move_by(1),
            KeyCode::PageUp => self.move_by(-page),
            KeyCode::PageDown => self.move_by(page),
            KeyCode::Home => self.move_by(-10_000_000),
            KeyCode::End => self.move_by(10_000_000),
            KeyCode::Enter => return self.activate(),
            KeyCode::Right => {
                if let View::Detail(_) = self.current() {
                    self.step_object(1);
                } else {
                    return self.activate();
                }
            }
            KeyCode::Left => {
                if let View::Detail(_) = self.current() {
                    self.step_object(-1);
                } else {
                    self.back();
                }
            }
            KeyCode::Esc | KeyCode::Backspace => {
                self.back();
            }
            KeyCode::Char('/') => {
                if let View::Records(l) = self.current_mut() {
                    l.searching = true;
                }
            }
            KeyCode::Char('f') => {
                let report = &self.report;
                if let Some(View::Records(l)) = self.stacks[self.tab.index()].last_mut() {
                    l.cycle_filter(report);
                }
            }
            KeyCode::Char('a') => {
                let report = &self.report;
                if let Some(View::Detail(d)) = self.stacks[self.tab.index()].last_mut() {
                    d.all_fields = !d.all_fields;
                    d.rebuild(report);
                }
            }
            KeyCode::Char('d') if self.tab == Tab::Dates => self.set_basis(self.basis.next()),
            KeyCode::Char('m') if self.tab == Tab::Dates => {
                self.problems_only = !self.problems_only;
                self.reset_dates();
            }
            KeyCode::Char('c') => {
                if let Some(id) = self.current_id() {
                    return Action::Copy(id);
                }
            }
            _ => {}
        }
        Action::None
    }

    fn step_object(&mut self, delta: isize) {
        let report = &self.report;
        if let Some(View::Detail(d)) = self.stacks[self.tab.index()].last_mut() {
            let new = (d.pos as isize + delta).clamp(0, d.siblings.len() as isize - 1) as usize;
            if new != d.pos {
                d.pos = new;
                d.rebuild(report);
            }
        }
        // Keep the list underneath in sync so Esc returns to the same object.
        let stack = &mut self.stacks[self.tab.index()];
        let n = stack.len();
        if n >= 2 {
            let pos = match &stack[n - 1] {
                View::Detail(d) => d.pos,
                _ => return,
            };
            if let View::Records(l) = &mut stack[n - 2] {
                l.table.select(Some(pos));
            }
        }
    }
}

fn has_problem(b: &crate::report::Bucket) -> bool {
    b.counts.mismatched() > 0 || b.source != b.dest
}

fn status_title(s: Status) -> &'static str {
    match s {
        Status::Matched => "Identical objects",
        Status::Different => "Different objects",
        Status::OnlyInSource => "Only in source",
        Status::OnlyInDest => "Only in destination",
    }
}

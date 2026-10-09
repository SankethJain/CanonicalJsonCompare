//! All screen drawing.

use std::sync::atomic::Ordering;
use std::time::SystemTime;

use chrono::{DateTime, Local};
use ratatui::Frame;
use ratatui::layout::{Alignment, Constraint, Layout, Margin, Position, Rect};
use ratatui::style::{Color, Modifier, Style, Stylize};
use ratatui::text::{Line, Span, Text};
use ratatui::widgets::{
    Block, BorderType, Borders, Cell, Clear, Gauge, Paragraph, Row, Table, TableState, Wrap,
};

use super::app::{App, Clickable, Field, PathCheck, PopupKind, Screen, check_path};
use super::browser::FileBrowser;
use super::input::TextInput;
use super::results::{Detail, RecordList, Results, RowKind, Tab, View};
use crate::diff::DiffKind;
use crate::engine::Stage;
use crate::extjson::{fmt_datetime_short, truncate};
use crate::report::{Bucket, DateBasis, Status};
use crate::ui_text::{fmt_bytes, fmt_num, pct};

const ACCENT: Color = Color::Cyan;
const OK: Color = Color::Green;
const BAD: Color = Color::Red;
const WARN: Color = Color::Yellow;
const EXTRA: Color = Color::Magenta;
const MUTED: Color = Color::DarkGray;

type Clicks = Vec<(Rect, Clickable)>;

fn status_color(s: Status) -> Color {
    match s {
        Status::Matched => OK,
        Status::Different => BAD,
        Status::OnlyInSource => WARN,
        Status::OnlyInDest => EXTRA,
    }
}

fn kind_color(k: DiffKind) -> Color {
    match k {
        DiffKind::Changed => BAD,
        DiffKind::TypeChanged => BAD,
        DiffKind::OnlyInSource => WARN,
        DiffKind::OnlyInDest => EXTRA,
    }
}

fn selected_style() -> Style {
    Style::new().add_modifier(Modifier::REVERSED | Modifier::BOLD)
}

fn panel(title: impl Into<String>) -> Block<'static> {
    Block::new()
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(Style::new().fg(MUTED))
        .title(Span::styled(
            format!(" {} ", title.into()),
            Style::new().fg(ACCENT).bold(),
        ))
}

fn centered(area: Rect, width: u16, height: u16) -> Rect {
    let w = width.min(area.width);
    let h = height.min(area.height);
    Rect {
        x: area.x + (area.width - w) / 2,
        y: area.y + (area.height - h) / 2,
        width: w,
        height: h,
    }
}

fn fmt_systime(t: Option<SystemTime>) -> String {
    t.map(|t| {
        DateTime::<Local>::from(t)
            .format("%Y-%m-%d %H:%M")
            .to_string()
    })
    .unwrap_or_default()
}

/// A table whose rows can be clicked; registers the clickable row area.
fn render_table(
    f: &mut Frame,
    area: Rect,
    table: Table,
    state: &mut TableState,
    clicks: &mut Clicks,
    block_inset: u16,
) {
    f.render_stateful_widget(table, area, state);
    let inner = Rect {
        x: area.x + block_inset,
        y: area.y + block_inset + 1,
        width: area.width.saturating_sub(block_inset * 2),
        height: area.height.saturating_sub(block_inset * 2 + 1),
    };
    clicks.push((
        inner,
        Clickable::Rows {
            offset: state.offset(),
        },
    ));
}

fn header_row(cells: &[&str]) -> Row<'static> {
    Row::new(
        cells
            .iter()
            .map(|c| Cell::from(c.to_string()))
            .collect::<Vec<_>>(),
    )
    .style(Style::new().fg(ACCENT).bold())
}

fn num_cell(n: u64, color: Option<Color>) -> Cell<'static> {
    let text = Line::from(fmt_num(n)).alignment(Alignment::Right);
    match color {
        Some(c) if n > 0 => Cell::from(text).style(Style::new().fg(c)),
        _ if n == 0 => Cell::from(text).style(Style::new().fg(MUTED)),
        _ => Cell::from(text),
    }
}

// ---------------------------------------------------------------------------

pub fn draw(f: &mut Frame, app: &mut App) {
    app.clickables.clear();
    let [header, body, footer] = Layout::vertical([
        Constraint::Length(1),
        Constraint::Fill(1),
        Constraint::Length(1),
    ])
    .areas(f.area());

    draw_header(f, header);
    match &mut app.screen {
        Screen::Setup => draw_setup(f, body, app),
        Screen::Running(_) => draw_running(f, body, app),
        Screen::Results(res) => draw_results(f, body, res, &mut app.clickables),
    }
    draw_footer(f, footer, app);

    if app.browser.is_some() {
        let target = app.browser_target;
        if let Some(b) = &mut app.browser {
            draw_browser(f, body, b, target, &mut app.clickables);
        }
    }
    if app.popup.is_some() {
        draw_popup(f, body, app);
    }
    if app.show_help {
        draw_help(f, body, app);
    }
    if let Some((msg, _)) = &app.toast {
        let w = (msg.chars().count() as u16 + 4).min(body.width);
        let r = Rect {
            x: body.right().saturating_sub(w + 1),
            y: body.bottom().saturating_sub(3),
            width: w,
            height: 3,
        };
        f.render_widget(Clear, r);
        f.render_widget(
            Paragraph::new(msg.as_str()).block(
                Block::bordered()
                    .border_type(BorderType::Rounded)
                    .border_style(Style::new().fg(OK)),
            ),
            r,
        );
    }
}

fn draw_header(f: &mut Frame, area: Rect) {
    let left = Line::from(vec![Span::styled(
        " ◆ MongoDB Export Compare ",
        Style::new().fg(Color::Black).bg(ACCENT).bold(),
    )]);
    let right = Line::from(Span::styled(
        format!("v{}  ·  F1 help ", crate::VERSION),
        Style::new().fg(MUTED),
    ))
    .alignment(Alignment::Right);
    f.render_widget(Paragraph::new(left), area);
    f.render_widget(Paragraph::new(right), area);
}

fn draw_footer(f: &mut Frame, area: Rect, app: &App) {
    let hints: Vec<(&str, &str)> = if app.show_help {
        vec![("any key", "close help")]
    } else if let Some(p) = &app.popup {
        match p.kind {
            PopupKind::Exported { .. } => {
                vec![("o", "open report"), ("f", "open folder"), ("Esc", "close")]
            }
            PopupKind::ConfirmQuit | PopupKind::ConfirmNew => vec![("y", "yes"), ("n", "no")],
            PopupKind::Info => vec![("↑↓", "scroll"), ("c", "copy"), ("Esc", "close")],
            PopupKind::Error => vec![("Enter", "close")],
        }
    } else if let Some(b) = &app.browser {
        let mut h = vec![
            ("↑↓", "move"),
            ("Enter", "open / choose"),
            ("←", "up a folder"),
            ("Tab", "places"),
        ];
        h.push((
            "Ctrl+A",
            if b.show_all {
                "only JSON files"
            } else {
                "all files"
            },
        ));
        h.push((
            "Esc",
            if b.filter.is_empty() {
                "cancel"
            } else {
                "clear filter"
            },
        ));
        h
    } else {
        match &app.screen {
            Screen::Setup => {
                let mut h = vec![("Tab/↑↓", "next/prev")];
                match app.setup.focus {
                    Field::Source | Field::Dest => h.push(("Enter", "browse files")),
                    Field::LooseNumbers | Field::ArrayOrder => h.push(("Space", "toggle")),
                    Field::Start => h.push(("Enter", "start")),
                    _ => {}
                }
                h.extend([
                    ("F5", "start"),
                    ("Ctrl+D", "sample data"),
                    ("F1", "help"),
                    ("Esc", "quit"),
                ]);
                h
            }
            Screen::Running(_) => vec![("Esc", "cancel")],
            Screen::Results(res) => results_hints(res),
        }
    };
    let mut spans = Vec::new();
    for (k, label) in hints {
        spans.push(Span::styled(
            format!(" {k} "),
            Style::new().fg(Color::Black).bg(ACCENT),
        ));
        spans.push(Span::raw(format!(" {label}  ")));
    }
    f.render_widget(Paragraph::new(Line::from(spans)), area);
}

fn results_hints(res: &Results) -> Vec<(&'static str, &'static str)> {
    let mut h: Vec<(&str, &str)> = Vec::new();
    match res.current() {
        View::Records(l) if l.searching => {
            return vec![("type", "search id"), ("Enter", "done"), ("Esc", "clear")];
        }
        View::Records(_) => h.extend([
            ("Enter", "details"),
            ("f", "filter"),
            ("/", "search"),
            ("c", "copy id"),
        ]),
        View::Detail(_) => h.extend([
            ("←→", "prev/next object"),
            ("a", "all fields"),
            ("Enter", "full value"),
            ("c", "copy id"),
        ]),
        View::Months(_) | View::Days { .. } => h.extend([
            ("Enter", "drill down"),
            ("d", "date type"),
            ("m", "problems only"),
        ]),
        View::Summary(_) | View::Fields(_) => h.push(("Enter", "list objects")),
        View::Issues { .. } => h.push(("↑↓", "scroll")),
    }
    if res.stack().len() > 1 {
        h.push(("Esc", "back"));
    }
    h.extend([
        ("Tab", "tabs"),
        ("e", "export"),
        ("?", "help"),
        ("q", "quit"),
    ]);
    h
}

// ---------------------------------------------------------------------------
// Setup screen

fn draw_setup(f: &mut Frame, body: Rect, app: &mut App) {
    let width = body.width.min(104);
    let area = Rect {
        x: body.x + (body.width - width) / 2,
        width,
        ..body
    }
    .inner(Margin::new(1, 0));
    let rows = Layout::vertical([
        Constraint::Length(2), // intro
        Constraint::Length(1), // source label
        Constraint::Length(3), // source box
        Constraint::Length(1), // source status
        Constraint::Length(1),
        Constraint::Length(1), // dest label
        Constraint::Length(3),
        Constraint::Length(1),
        Constraint::Length(1),
        Constraint::Length(1), // options title
        Constraint::Length(1), // created
        Constraint::Length(1), // updated
        Constraint::Length(1), // ignore
        Constraint::Length(1), // loose
        Constraint::Length(1), // array
        Constraint::Length(1),
        Constraint::Length(3), // start
        Constraint::Fill(1),   // messages
    ])
    .split(area);

    f.render_widget(
        Paragraph::new(vec![
            Line::from("Compare two MongoDB exports (mongoexport, canonical or relaxed JSON) object by object, using _id.").bold(),
            Line::from(Span::styled("Pick the two files below, then start the comparison.", Style::new().fg(MUTED))),
        ]),
        rows[0],
    );

    let s = &app.setup;
    let clicks = &mut app.clickables;
    path_box(
        f,
        rows[1],
        rows[2],
        rows[3],
        "1",
        "Source file",
        "the original / expected data",
        &s.source,
        s.focus == Field::Source,
    );
    clicks.push((
        Rect {
            height: 5,
            ..rows[1]
        },
        Clickable::SetupField(Field::Source),
    ));
    path_box(
        f,
        rows[5],
        rows[6],
        rows[7],
        "2",
        "Destination file",
        "the copy you want to check",
        &s.dest,
        s.focus == Field::Dest,
    );
    clicks.push((
        Rect {
            height: 5,
            ..rows[5]
        },
        Clickable::SetupField(Field::Dest),
    ));

    f.render_widget(
        Paragraph::new(Line::from(vec![
            Span::styled("Options ", Style::new().fg(ACCENT).bold()),
            Span::styled(
                "(optional - the defaults work for most exports)",
                Style::new().fg(MUTED),
            ),
        ])),
        rows[9],
    );
    let opt_fields = [
        (
            Field::Created,
            "Created date field",
            &s.created,
            "auto-detect (e.g. createdAt)",
        ),
        (
            Field::Updated,
            "Updated date field",
            &s.updated,
            "auto-detect (e.g. updatedAt)",
        ),
        (
            Field::Ignore,
            "Ignore these fields",
            &s.ignore,
            "none (e.g. __v, lastSyncedAt)",
        ),
    ];
    for (i, (field, label, input, placeholder)) in opt_fields.into_iter().enumerate() {
        let r = rows[10 + i];
        text_option(f, r, label, input, placeholder, s.focus == field);
        clicks.push((r, Clickable::SetupField(field)));
    }
    checkbox(
        f,
        rows[13],
        s.loose_numbers,
        "Numbers with the same value are equal, whatever their type (5 = 5.0, Int32 = Int64)",
        s.focus == Field::LooseNumbers,
    );
    clicks.push((rows[13], Clickable::SetupField(Field::LooseNumbers)));
    checkbox(
        f,
        rows[14],
        s.ignore_array_order,
        "Ignore the order of items inside lists",
        s.focus == Field::ArrayOrder,
    );
    clicks.push((rows[14], Clickable::SetupField(Field::ArrayOrder)));

    let focused = s.focus == Field::Start;
    let btn = centered(rows[16], 32, 3);
    let style = if focused {
        Style::new().fg(Color::Black).bg(OK).bold()
    } else {
        Style::new().fg(OK).bold()
    };
    f.render_widget(
        Paragraph::new("▶  Start comparison")
            .alignment(Alignment::Center)
            .style(style)
            .block(
                Block::bordered()
                    .border_type(BorderType::Rounded)
                    .border_style(Style::new().fg(OK)),
            ),
        btn,
    );
    clicks.push((btn, Clickable::SetupField(Field::Start)));

    let mut msg = Vec::new();
    if let Some(e) = &s.error {
        msg.push(Line::from(Span::styled(
            format!("✘ {e}"),
            Style::new().fg(BAD).bold(),
        )));
    }
    msg.push(Line::from(Span::styled(
        "Tip: you can also drag a file from your file manager into a box, or paste its path.",
        Style::new().fg(MUTED),
    )));
    msg.push(Line::from(Span::styled(
        "No files yet? Press Ctrl+D to try it with sample data.",
        Style::new().fg(MUTED),
    )));
    f.render_widget(Paragraph::new(msg).wrap(Wrap { trim: true }), rows[17]);

    // Cursor for the focused text field.
    if let Some((x, y)) = setup_cursor(&rows, s) {
        f.set_cursor_position(Position { x, y });
    }
}

fn setup_cursor(rows: &[Rect], s: &super::app::Setup) -> Option<(u16, u16)> {
    let (input, rect, offset) = match s.focus {
        Field::Source => (&s.source, rows[2].inner(Margin::new(1, 1)), 0u16),
        Field::Dest => (&s.dest, rows[6].inner(Margin::new(1, 1)), 0),
        Field::Created => (&s.created, rows[10], OPTION_LABEL_W),
        Field::Updated => (&s.updated, rows[11], OPTION_LABEL_W),
        Field::Ignore => (&s.ignore, rows[12], OPTION_LABEL_W),
        _ => return None,
    };
    let width = rect.width.saturating_sub(offset + 1) as usize;
    let (_, cur) = input.visible(width);
    Some((rect.x + offset + cur as u16, rect.y))
}

#[allow(clippy::too_many_arguments)]
fn path_box(
    f: &mut Frame,
    label_r: Rect,
    box_r: Rect,
    status_r: Rect,
    num: &str,
    title: &str,
    sub: &str,
    input: &TextInput,
    focused: bool,
) {
    let marker = if focused { "› " } else { "  " };
    f.render_widget(
        Paragraph::new(Line::from(vec![
            Span::styled(marker, Style::new().fg(ACCENT).bold()),
            Span::styled(format!("{num}. {title}"), Style::new().bold()),
            Span::styled(format!("  - {sub}"), Style::new().fg(MUTED)),
        ])),
        label_r,
    );
    let border = if focused {
        Style::new().fg(ACCENT).bold()
    } else {
        Style::new().fg(MUTED)
    };
    let block = Block::bordered()
        .border_type(BorderType::Rounded)
        .border_style(border);
    let inner_w = box_r.width.saturating_sub(2) as usize;
    let content = if input.value.is_empty() {
        Line::from(Span::styled(
            if focused {
                "Press Enter to browse, or type / paste / drag a file here"
            } else {
                "(not chosen yet)"
            },
            Style::new().fg(MUTED),
        ))
    } else if !focused && input.value.chars().count() > inner_w {
        // Show the end of long paths: the file name matters most.
        let tail: String = {
            let chars: Vec<char> = input.value.chars().collect();
            chars[chars.len() + 1 - inner_w.max(1)..].iter().collect()
        };
        Line::from(vec![
            Span::styled("…", Style::new().fg(MUTED)),
            Span::raw(tail),
        ])
    } else {
        Line::from(input.visible(inner_w).0)
    };
    f.render_widget(Paragraph::new(content).block(block), box_r);

    let status = match check_path(&input.value) {
        PathCheck::Empty => Line::from(""),
        PathCheck::Missing => {
            Line::from(Span::styled("    ✘ File not found", Style::new().fg(BAD)))
        }
        PathCheck::IsFolder => Line::from(Span::styled(
            "    ✘ This is a folder - press Enter to pick a file in it",
            Style::new().fg(WARN),
        )),
        PathCheck::Ok { size, modified } => Line::from(vec![
            Span::styled("    ✔ Ready", Style::new().fg(OK)),
            Span::styled(
                format!(
                    "  ·  {}  ·  modified {}",
                    fmt_bytes(size),
                    fmt_systime(modified)
                ),
                Style::new().fg(MUTED),
            ),
        ]),
    };
    f.render_widget(Paragraph::new(status), status_r);
}

const OPTION_LABEL_W: u16 = 24;

fn text_option(
    f: &mut Frame,
    r: Rect,
    label: &str,
    input: &TextInput,
    placeholder: &str,
    focused: bool,
) {
    let marker = if focused { "› " } else { "  " };
    let label_style = if focused {
        Style::new().fg(ACCENT).bold()
    } else {
        Style::new()
    };
    let w = r.width.saturating_sub(OPTION_LABEL_W + 1) as usize;
    let value = if input.value.is_empty() {
        Span::styled(
            format!("{placeholder:<w$}"),
            Style::new().fg(MUTED).add_modifier(Modifier::UNDERLINED),
        )
    } else {
        let (shown, _) = input.visible(w);
        Span::styled(
            format!("{shown:<w$}"),
            Style::new().add_modifier(Modifier::UNDERLINED),
        )
    };
    let label_text = format!("{marker}{label}");
    f.render_widget(
        Paragraph::new(Line::from(vec![
            Span::styled(
                format!("{label_text:<width$}", width = OPTION_LABEL_W as usize),
                label_style,
            ),
            value,
        ])),
        r,
    );
}

fn checkbox(f: &mut Frame, r: Rect, checked: bool, label: &str, focused: bool) {
    let marker = if focused { "› " } else { "  " };
    let style = if focused {
        Style::new().fg(ACCENT).bold()
    } else {
        Style::new()
    };
    let mark = if checked { "[x] " } else { "[ ] " };
    f.render_widget(
        Paragraph::new(Line::from(vec![
            Span::styled(marker, style),
            Span::styled(mark, style.bold()),
            Span::styled(label, style),
        ])),
        r,
    );
}

// ---------------------------------------------------------------------------
// File browser

fn draw_browser(
    f: &mut Frame,
    body: Rect,
    b: &mut FileBrowser,
    target: Field,
    clicks: &mut Clicks,
) {
    let area = body.inner(Margin::new(2, 1));
    f.render_widget(Clear, area);
    let title = if target == Field::Dest {
        "Choose the destination file"
    } else {
        "Choose the source file"
    };
    let block = panel(title).border_style(Style::new().fg(ACCENT));
    let inner = block.inner(area);
    f.render_widget(block, area);
    let [path_r, filter_r, lists_r] = Layout::vertical([
        Constraint::Length(1),
        Constraint::Length(1),
        Constraint::Fill(1),
    ])
    .areas(inner.inner(Margin::new(1, 0)));

    f.render_widget(
        Paragraph::new(Line::from(vec![
            Span::styled("Folder: ", Style::new().fg(MUTED)),
            Span::styled(b.dir.display().to_string(), Style::new().bold()),
        ])),
        path_r,
    );
    let filter_line = if let Some(e) = &b.error {
        Line::from(Span::styled(e.clone(), Style::new().fg(BAD)))
    } else if b.filter.is_empty() {
        let mut spans = vec![Span::styled(
            "Type to filter by name.",
            Style::new().fg(MUTED),
        )];
        if !b.show_all && b.hidden_files > 0 {
            spans.push(Span::styled(
                format!(
                    "  {} non-JSON files hidden (Ctrl+A shows all).",
                    b.hidden_files
                ),
                Style::new().fg(MUTED),
            ));
        }
        Line::from(spans)
    } else {
        Line::from(vec![
            Span::styled("Filter: ", Style::new().fg(ACCENT)),
            Span::styled(format!("{}▏", b.filter), Style::new().bold()),
        ])
    };
    f.render_widget(Paragraph::new(filter_line), filter_r);

    let [places_r, files_r] = Layout::horizontal([Constraint::Length(24), Constraint::Fill(1)])
        .spacing(1)
        .areas(lists_r);

    let place_rows: Vec<Row> = b
        .places
        .iter()
        .map(|(label, _)| Row::new(vec![label.clone()]))
        .collect();
    let pb = Block::bordered()
        .border_type(BorderType::Rounded)
        .title(" Places ")
        .border_style(Style::new().fg(if b.focus_places { ACCENT } else { MUTED }));
    let places_table = Table::new(place_rows, [Constraint::Fill(1)])
        .block(pb)
        .row_highlight_style(if b.focus_places {
            selected_style()
        } else {
            Style::new().bold()
        });
    f.render_stateful_widget(places_table, places_r, &mut b.places_table);
    clicks.push((
        places_r.inner(Margin::new(1, 1)),
        Clickable::BrowserPlaces {
            offset: b.places_table.offset(),
        },
    ));

    let rows: Vec<Row> = b
        .visible
        .iter()
        .map(|&i| {
            let e = &b.entries[i];
            if e.is_dir {
                let style = if e.is_parent {
                    Style::new().fg(MUTED)
                } else {
                    Style::new().fg(Color::Blue).bold()
                };
                let name = if e.is_parent {
                    e.name.clone()
                } else {
                    format!("▸ {}/", e.name)
                };
                Row::new(vec![
                    Cell::from(name).style(style),
                    Cell::from(""),
                    Cell::from(fmt_systime(e.modified)),
                ])
            } else {
                Row::new(vec![
                    Cell::from(format!("  {}", e.name)),
                    Cell::from(Line::from(fmt_bytes(e.size)).alignment(Alignment::Right)),
                    Cell::from(fmt_systime(e.modified)).style(Style::new().fg(MUTED)),
                ])
            }
        })
        .collect();
    let empty = rows.is_empty();
    let fb = Block::bordered()
        .border_type(BorderType::Rounded)
        .border_style(Style::new().fg(if b.focus_places { MUTED } else { ACCENT }));
    let table = Table::new(
        rows,
        [
            Constraint::Fill(1),
            Constraint::Length(11),
            Constraint::Length(17),
        ],
    )
    .header(header_row(&["Name", "Size", "Modified"]))
    .block(fb)
    .column_spacing(2)
    .row_highlight_style(if b.focus_places {
        Style::new()
    } else {
        selected_style()
    })
    .highlight_symbol("▶");
    f.render_stateful_widget(table, files_r, &mut b.table);
    clicks.push((
        Rect {
            y: files_r.y + 2,
            height: files_r.height.saturating_sub(3),
            ..files_r.inner(Margin::new(1, 0))
        },
        Clickable::BrowserFiles {
            offset: b.table.offset(),
        },
    ));
    if empty {
        let msg = if b.filter.is_empty() {
            "No JSON files in this folder. Open a folder, or press Ctrl+A to show all files."
        } else {
            "Nothing matches the filter. Press Esc to clear it."
        };
        f.render_widget(
            Paragraph::new(msg)
                .style(Style::new().fg(MUTED))
                .alignment(Alignment::Center)
                .wrap(Wrap { trim: true }),
            centered(files_r, files_r.width.saturating_sub(4), 3),
        );
    }
}

// ---------------------------------------------------------------------------
// Progress

fn draw_running(f: &mut Frame, body: Rect, app: &App) {
    let Screen::Running(run) = &app.screen else {
        return;
    };
    let p = &run.progress;
    let area = centered(body, 76, 13);
    let block = panel("Comparing…").border_style(Style::new().fg(ACCENT));
    let inner = block.inner(area).inner(Margin::new(2, 1));
    f.render_widget(block, area);
    let [files_r, stage_r, gauge_r, stats_r, hint_r] = Layout::vertical([
        Constraint::Length(3),
        Constraint::Length(1),
        Constraint::Length(1),
        Constraint::Length(2),
        Constraint::Length(2),
    ])
    .areas(inner);

    let name = |path: &std::path::Path| {
        path.file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default()
    };
    f.render_widget(
        Paragraph::new(vec![
            Line::from(vec![
                Span::styled("Source:      ", Style::new().fg(MUTED)),
                Span::raw(name(&run.request.source)),
            ]),
            Line::from(vec![
                Span::styled("Destination: ", Style::new().fg(MUTED)),
                Span::raw(name(&run.request.dest)),
            ]),
        ]),
        files_r,
    );
    let stage = match p.stage() {
        Stage::ReadingSource => "Step 1 of 2 · Reading the source file",
        Stage::ComparingDest => "Step 2 of 2 · Comparing the destination with the source",
        Stage::Finishing => "Almost done · Building the report",
    };
    f.render_widget(Paragraph::new(stage).bold(), stage_r);
    let ratio = p.ratio();
    f.render_widget(
        Gauge::default()
            .ratio(ratio)
            .gauge_style(Style::new().fg(ACCENT).bg(Color::Reset))
            .label(format!("{:.0}%", ratio * 100.0)),
        gauge_r,
    );
    let elapsed = run.started.elapsed().as_secs_f64();
    let eta = if ratio > 0.02 {
        format!(" · about {} left", fmt_secs(elapsed / ratio - elapsed))
    } else {
        String::new()
    };
    f.render_widget(
        Paragraph::new(format!(
            "{} objects read · {} elapsed{eta}",
            fmt_num(p.documents.load(Ordering::Relaxed)),
            fmt_secs(elapsed)
        ))
        .style(Style::new().fg(MUTED)),
        stats_r,
    );
    f.render_widget(
        Paragraph::new("Press Esc to cancel.").style(Style::new().fg(MUTED)),
        hint_r,
    );
}

fn fmt_secs(s: f64) -> String {
    let s = s.max(0.0) as u64;
    if s >= 3600 {
        format!("{}h {:02}m", s / 3600, s / 60 % 60)
    } else {
        format!("{:02}:{:02}", s / 60, s % 60)
    }
}

// ---------------------------------------------------------------------------
// Results

fn draw_results(f: &mut Frame, body: Rect, res: &mut Results, clicks: &mut Clicks) {
    let [verdict_r, tabs_r, crumb_r, content_r] = Layout::vertical([
        Constraint::Length(3),
        Constraint::Length(1),
        Constraint::Length(1),
        Constraint::Fill(1),
    ])
    .areas(body);
    draw_verdict(f, verdict_r, res);
    draw_tabs(f, tabs_r, res, clicks);

    let crumbs = res.breadcrumb();
    let mut spans = vec![Span::raw(" ")];
    for (i, c) in crumbs.iter().enumerate() {
        if i > 0 {
            spans.push(Span::styled("  ›  ", Style::new().fg(MUTED)));
        }
        let style = if i + 1 == crumbs.len() {
            Style::new().bold()
        } else {
            Style::new().fg(MUTED)
        };
        spans.push(Span::styled(c.clone(), style));
    }
    if crumbs.len() > 1 {
        spans.push(Span::styled(
            "      (Esc goes back)",
            Style::new().fg(MUTED),
        ));
    }
    f.render_widget(Paragraph::new(Line::from(spans)), crumb_r);

    let report = &res.report;
    let tab = res.tab.index();
    let depth = res.stacks[tab].len();
    // Split borrows: the view being drawn is mutable, the rest read-only.
    let (rest, last) = res.stacks[tab].split_at_mut(depth - 1);
    let _ = rest;
    let view = &mut last[0];
    match view {
        View::Summary(t) => draw_summary(f, content_r, report, t, clicks),
        View::Records(l) => draw_records(f, content_r, report, l, clicks),
        View::Months(t) => {
            let visible: Vec<usize> = (0..res.months.len())
                .filter(|&i| !res.problems_only || bucket_problem(&res.months[i].bucket))
                .collect();
            let buckets: Vec<&Bucket> = visible.iter().map(|&i| &res.months[i].bucket).collect();
            draw_buckets(
                f,
                content_r,
                report,
                res.basis,
                res.problems_only,
                "Month",
                &buckets,
                t,
                clicks,
            );
        }
        View::Days { month, table } => {
            let m = &res.months[*month];
            let buckets: Vec<&Bucket> = m
                .days
                .iter()
                .filter(|d| !res.problems_only || bucket_problem(d))
                .collect();
            draw_buckets(
                f,
                content_r,
                report,
                res.basis,
                res.problems_only,
                "Day",
                &buckets,
                table,
                clicks,
            );
        }
        View::Fields(t) => draw_fields(f, content_r, report, t, clicks),
        View::Issues { scroll } => draw_issues(f, content_r, report, *scroll),
        View::Detail(d) => draw_detail(f, content_r, report, d, clicks),
    }
}

fn bucket_problem(b: &Bucket) -> bool {
    b.counts.mismatched() > 0 || b.source != b.dest
}

fn draw_verdict(f: &mut Frame, area: Rect, res: &Results) {
    let r = &res.report;
    let (color, text) = if r.files_match() {
        (
            OK,
            format!(
                "✔  THE FILES MATCH  -  all {} objects are identical",
                fmt_num(r.counts.total())
            ),
        )
    } else {
        (
            BAD,
            format!(
                "✘  THE FILES DO NOT MATCH  -  {} of {} objects are different, missing or extra",
                fmt_num(r.counts.mismatched()),
                fmt_num(r.counts.total())
            ),
        )
    };
    let mut spans = vec![Span::styled(text, Style::new().fg(color).bold())];
    let issues = res.issue_count();
    if issues > 0 {
        spans.push(Span::styled(
            format!("     ⚠ {} warnings (tab 5)", fmt_num(issues)),
            Style::new().fg(WARN).bold(),
        ));
    }
    f.render_widget(
        Paragraph::new(Line::from(spans))
            .alignment(Alignment::Center)
            .block(
                Block::bordered()
                    .border_type(BorderType::Thick)
                    .border_style(Style::new().fg(color)),
            ),
        area,
    );
}

fn draw_tabs(f: &mut Frame, area: Rect, res: &Results, clicks: &mut Clicks) {
    let r = &res.report;
    let labels = [
        "1 Summary".to_string(),
        format!("2 Mismatches ({})", fmt_num(r.counts.mismatched())),
        "3 By date".to_string(),
        format!("4 Fields ({})", fmt_num(r.fields.len() as u64)),
        format!("5 Issues ({})", fmt_num(res.issue_count())),
    ];
    let mut x = area.x + 1;
    let mut spans = vec![Span::raw(" ")];
    for (i, label) in labels.iter().enumerate() {
        let tab = Tab::ALL[i];
        let text = format!(" {label} ");
        let w = text.chars().count() as u16;
        let style = if tab == res.tab {
            Style::new().fg(Color::Black).bg(ACCENT).bold()
        } else {
            Style::new().fg(ACCENT)
        };
        spans.push(Span::styled(text, style));
        spans.push(Span::raw(" "));
        clicks.push((
            Rect {
                x,
                y: area.y,
                width: w,
                height: 1,
            },
            Clickable::Tab(tab),
        ));
        x += w + 1;
    }
    f.render_widget(Paragraph::new(Line::from(spans)), area);
}

fn draw_summary(
    f: &mut Frame,
    area: Rect,
    r: &crate::report::Report,
    state: &mut TableState,
    clicks: &mut Clicks,
) {
    let [files_r, cmp_r, notes_r] = Layout::vertical([
        Constraint::Length(12),
        Constraint::Length(8),
        Constraint::Fill(1),
    ])
    .areas(area);

    let (s, d) = (&r.source, &r.dest);
    let folder = |p: &std::path::Path| {
        p.parent()
            .map(|x| x.display().to_string())
            .unwrap_or_default()
    };
    let warn_num = |n: u64| {
        if n > 0 {
            Cell::from(fmt_num(n)).style(Style::new().fg(WARN))
        } else {
            Cell::from("0").style(Style::new().fg(MUTED))
        }
    };
    let docs_style = if s.documents == d.documents {
        Style::new().bold()
    } else {
        Style::new().fg(WARN).bold()
    };
    let rows = vec![
        Row::new(vec![
            Cell::from("File"),
            Cell::from(s.name()).bold(),
            Cell::from(d.name()).bold(),
        ]),
        Row::new(vec![
            Cell::from("Folder"),
            Cell::from(folder(&s.path)),
            Cell::from(folder(&d.path)),
        ])
        .style(Style::new().fg(MUTED)),
        Row::new(vec!["Size".into(), fmt_bytes(s.size), fmt_bytes(d.size)]),
        Row::new(vec![
            "Format".into(),
            s.format.map(|x| x.describe()).unwrap_or("").to_string(),
            d.format.map(|x| x.describe()).unwrap_or("").to_string(),
        ]),
        Row::new(vec![
            Cell::from("Objects"),
            Cell::from(fmt_num(s.documents)).style(docs_style),
            Cell::from(match d.documents as i64 - s.documents as i64 {
                0 => fmt_num(d.documents),
                n if n > 0 => format!(
                    "{}   ({} more than source)",
                    fmt_num(d.documents),
                    fmt_num(n as u64)
                ),
                n => format!(
                    "{}   ({} fewer than source)",
                    fmt_num(d.documents),
                    fmt_num(n.unsigned_abs())
                ),
            })
            .style(docs_style),
        ]),
        Row::new(vec![
            Cell::from("Unreadable entries"),
            warn_num(s.error_count),
            warn_num(d.error_count),
        ]),
        Row::new(vec![
            Cell::from("Duplicate _id"),
            warn_num(s.duplicate_count),
            warn_num(d.duplicate_count),
        ]),
        Row::new(vec![
            Cell::from("Without _id"),
            warn_num(s.without_id),
            warn_num(d.without_id),
        ]),
        Row::new(vec![
            "Created between".into(),
            format!(
                "{}  to  {}",
                fmt_datetime_short(s.first_created),
                fmt_datetime_short(s.last_created)
            ),
            format!(
                "{}  to  {}",
                fmt_datetime_short(d.first_created),
                fmt_datetime_short(d.last_created)
            ),
        ]),
    ];
    let t = Table::new(
        rows,
        [
            Constraint::Length(20),
            Constraint::Fill(1),
            Constraint::Fill(1),
        ],
    )
    .header(header_row(&["", "Source", "Destination"]))
    .column_spacing(2)
    .block(panel("Files"));
    f.render_widget(t, files_r);

    let total = r.counts.total();
    let bar_w = 24usize;
    let rows: Vec<Row> = [
        (Status::Different, "Different (same _id, other content)"),
        (
            Status::OnlyInSource,
            "Only in source (missing in destination)",
        ),
        (Status::OnlyInDest, "Only in destination (extra)"),
        (Status::Matched, "Identical"),
    ]
    .iter()
    .map(|&(st, label)| {
        let n = r.counts.get(st);
        let filled = if total == 0 {
            0
        } else {
            ((n as f64 / total as f64) * bar_w as f64).ceil() as usize
        };
        let color = status_color(st);
        Row::new(vec![
            Cell::from(Span::styled(format!("● {label}"), Style::new().fg(color))),
            num_cell(n, Some(color)),
            Cell::from(Line::from(pct(n, total)).alignment(Alignment::Right)),
            Cell::from(Line::from(vec![
                Span::styled("█".repeat(filled), Style::new().fg(color)),
                Span::styled(
                    "░".repeat(bar_w - filled.min(bar_w)),
                    Style::new().fg(MUTED),
                ),
            ])),
        ])
    })
    .collect();
    let t = Table::new(
        rows,
        [
            Constraint::Length(42),
            Constraint::Length(12),
            Constraint::Length(8),
            Constraint::Fill(1),
        ],
    )
    .header(header_row(&["Result", "     Objects", "   Share", ""]))
    .column_spacing(2)
    .block(panel("Comparison by _id  (Enter lists the objects)"))
    .row_highlight_style(selected_style())
    .highlight_symbol("▶ ");
    render_table(f, cmp_r, t, state, clicks, 1);

    let o = &r.options;
    let notes = vec![
        Line::from(vec![
            Span::styled("Created date field: ", Style::new().fg(MUTED)),
            Span::raw(r.created_field.describe()),
        ]),
        Line::from(vec![
            Span::styled("Updated date field: ", Style::new().fg(MUTED)),
            Span::raw(r.updated_field.describe()),
        ]),
        Line::from(vec![
            Span::styled("Ignored fields: ", Style::new().fg(MUTED)),
            Span::raw(if o.ignore.is_empty() {
                "none".to_string()
            } else {
                o.ignore.join(", ")
            }),
            Span::styled("   Numbers: ", Style::new().fg(MUTED)),
            Span::raw(if o.loose_numbers {
                "compared by value"
            } else {
                "type must match"
            }),
            Span::styled("   Lists: ", Style::new().fg(MUTED)),
            Span::raw(if o.ignore_array_order {
                "order ignored"
            } else {
                "order matters"
            }),
        ]),
        Line::from(Span::styled(
            format!(
                "Finished {} in {:.1}s. Press e to save the report (HTML + Excel/CSV).",
                r.finished_at.format("%Y-%m-%d %H:%M"),
                r.duration.as_secs_f64()
            ),
            Style::new().fg(MUTED),
        )),
    ];
    f.render_widget(
        Paragraph::new(notes)
            .wrap(Wrap { trim: true })
            .block(Block::new().padding(ratatui::widgets::Padding::horizontal(1))),
        notes_r,
    );
}

fn draw_records(
    f: &mut Frame,
    area: Rect,
    r: &crate::report::Report,
    l: &mut RecordList,
    clicks: &mut Clicks,
) {
    let [bar_r, table_r] =
        Layout::vertical([Constraint::Length(1), Constraint::Fill(1)]).areas(area);

    let mut spans = vec![Span::styled(" Show (f): ", Style::new().fg(MUTED))];
    for (flt, n) in &l.filters {
        let label = match flt {
            None => format!("All {}", fmt_num(*n as u64)),
            Some(s) => format!("{} {}", s.label(), fmt_num(*n as u64)),
        };
        let style = if *flt == l.filter {
            Style::new()
                .fg(Color::Black)
                .bg(flt.map(status_color).unwrap_or(ACCENT))
                .bold()
        } else {
            Style::new().fg(flt.map(status_color).unwrap_or(ACCENT))
        };
        spans.push(Span::styled(format!(" {label} "), style));
        spans.push(Span::raw(" "));
    }
    spans.push(Span::styled("   Search (/): ", Style::new().fg(MUTED)));
    if l.searching {
        spans.push(Span::styled(
            format!("{}▏", l.search),
            Style::new().fg(ACCENT).bold(),
        ));
    } else if l.search.is_empty() {
        spans.push(Span::styled("-", Style::new().fg(MUTED)));
    } else {
        spans.push(Span::styled(l.search.clone(), Style::new().bold()));
    }
    f.render_widget(Paragraph::new(Line::from(spans)), bar_r);

    // Only build rows near the visible window: lists can hold millions.
    let height = table_r.height.saturating_sub(3) as usize;
    let sel = l.table.selected().unwrap_or(0);
    let mut offset = l.table.offset().min(l.visible.len().saturating_sub(1));
    if sel < offset {
        offset = sel;
    } else if height > 0 && sel >= offset + height {
        offset = sel + 1 - height;
    }
    let end = (offset + height).min(l.visible.len());
    let rows: Vec<Row> = l.visible[offset..end]
        .iter()
        .enumerate()
        .map(|(k, &i)| {
            let rec = &r.records[i];
            let fields = match rec.status {
                Status::Different => {
                    let mut names: Vec<&str> = Vec::new();
                    for d in &rec.diffs {
                        if !names.contains(&d.path.as_str()) {
                            names.push(&d.path);
                        }
                        if names.len() >= 6 {
                            break;
                        }
                    }
                    let mut s = names.join(", ");
                    if rec.diffs.len() > names.len() {
                        s.push_str(", …");
                    }
                    s
                }
                Status::OnlyInSource => "whole object missing in destination".into(),
                Status::OnlyInDest => "whole object not in source".into(),
                Status::Matched => String::new(),
            };
            Row::new(vec![
                Cell::from(
                    Line::from(fmt_num((offset + k + 1) as u64)).alignment(Alignment::Right),
                )
                .style(Style::new().fg(MUTED)),
                Cell::from(rec.id.to_string()).bold(),
                Cell::from(Span::styled(
                    rec.status.label(),
                    Style::new().fg(status_color(rec.status)),
                )),
                Cell::from(
                    Line::from(if rec.diffs.is_empty() {
                        "-".into()
                    } else {
                        fmt_num(rec.diffs.len() as u64)
                    })
                    .alignment(Alignment::Right),
                ),
                Cell::from(fmt_datetime_short(rec.created())).style(Style::new().fg(MUTED)),
                Cell::from(fields),
            ])
        })
        .collect();
    let title = format!("{}  ·  {} shown", l.title, fmt_num(l.visible.len() as u64));
    let t = Table::new(
        rows,
        [
            Constraint::Length(7),
            Constraint::Length(26),
            Constraint::Length(20),
            Constraint::Length(6),
            Constraint::Length(16),
            Constraint::Fill(1),
        ],
    )
    .header(header_row(&[
        "      #",
        "Object id",
        "Status",
        " Diffs",
        "Created",
        "Fields that differ",
    ]))
    .column_spacing(2)
    .block(panel(title))
    .row_highlight_style(selected_style())
    .highlight_symbol("▶ ");
    // Render a window of rows with a local state, then map back.
    let mut local = TableState::default().with_selected(l.table.selected().map(|s| s - offset));
    f.render_stateful_widget(t, table_r, &mut local);
    *l.table.offset_mut() = offset;
    clicks.push((
        Rect {
            y: table_r.y + 2,
            height: table_r.height.saturating_sub(3),
            ..table_r.inner(Margin::new(1, 0))
        },
        Clickable::Rows { offset },
    ));
    if l.visible.is_empty() {
        let msg = if !l.search.is_empty() {
            format!(
                "No object id contains \"{}\". Press / and Esc to clear the search.",
                l.search
            )
        } else {
            "Nothing to show here.".into()
        };
        f.render_widget(
            Paragraph::new(msg)
                .style(Style::new().fg(MUTED))
                .alignment(Alignment::Center),
            centered(table_r, table_r.width.saturating_sub(4), 1),
        );
    }
}

#[allow(clippy::too_many_arguments)]
fn draw_buckets(
    f: &mut Frame,
    area: Rect,
    r: &crate::report::Report,
    basis: DateBasis,
    problems_only: bool,
    unit: &str,
    buckets: &[&Bucket],
    state: &mut TableState,
    clicks: &mut Clicks,
) {
    let [info_r, table_r] =
        Layout::vertical([Constraint::Length(2), Constraint::Fill(1)]).areas(area);
    let field = match basis {
        DateBasis::Created => format!(
            "field {}",
            r.created_field.name.as_deref().unwrap_or("not found")
        ),
        DateBasis::Updated => format!(
            "field {}",
            r.updated_field.name.as_deref().unwrap_or("not found")
        ),
        DateBasis::IdTime => "time stored inside the ObjectId".into(),
    };
    let info = vec![
        Line::from(vec![
            Span::styled(" Grouped by (d): ", Style::new().fg(MUTED)),
            Span::styled(
                format!(" {} ", basis.label()),
                Style::new().fg(Color::Black).bg(ACCENT).bold(),
            ),
            Span::styled(format!("  {field}"), Style::new().fg(MUTED)),
            Span::styled("     Only problems (m): ", Style::new().fg(MUTED)),
            Span::styled(
                if problems_only { " ON " } else { " off " },
                if problems_only {
                    Style::new().fg(Color::Black).bg(WARN).bold()
                } else {
                    Style::new().fg(MUTED)
                },
            ),
        ]),
        Line::from(Span::styled(
            " Each file is counted by its own date. Dates are in UTC. Enter drills down: month › day › objects.",
            Style::new().fg(MUTED),
        )),
    ];
    f.render_widget(Paragraph::new(info), info_r);

    let max = buckets
        .iter()
        .map(|b| b.source.max(b.dest))
        .max()
        .unwrap_or(0)
        .max(1);
    let bar_w = 16usize;
    let rows: Vec<Row> = buckets
        .iter()
        .map(|b| {
            let delta = b.dest as i64 - b.source as i64;
            let delta_cell = if delta == 0 {
                Cell::from(Line::from("=").alignment(Alignment::Right))
                    .style(Style::new().fg(MUTED))
            } else {
                Cell::from(Line::from(format!("{delta:+}")).alignment(Alignment::Right))
                    .style(Style::new().fg(WARN).bold())
            };
            let filled =
                ((b.source.max(b.dest) as f64 / max as f64) * bar_w as f64).round() as usize;
            let bar_color = if bucket_problem(b) { BAD } else { OK };
            let marker = if bucket_problem(b) {
                Span::styled("● ", Style::new().fg(BAD))
            } else {
                Span::styled("  ", Style::new())
            };
            Row::new(vec![
                Cell::from(Line::from(vec![
                    marker,
                    Span::styled(b.label.clone(), Style::new().bold()),
                ])),
                num_cell(b.source, None),
                num_cell(b.dest, None),
                delta_cell,
                num_cell(b.counts.matched, Some(OK)),
                num_cell(b.counts.different, Some(BAD)),
                num_cell(b.counts.only_source, Some(WARN)),
                num_cell(b.counts.only_dest, Some(EXTRA)),
                Cell::from(Span::styled(
                    "▇".repeat(filled.max(1)),
                    Style::new().fg(bar_color),
                )),
            ])
        })
        .collect();
    let title = format!(
        "Records per {}  ·  {} rows",
        unit.to_lowercase(),
        buckets.len()
    );
    let t = Table::new(
        rows,
        [
            Constraint::Length(14),
            Constraint::Length(10),
            Constraint::Length(11),
            Constraint::Length(8),
            Constraint::Length(10),
            Constraint::Length(10),
            Constraint::Length(10),
            Constraint::Length(10),
            Constraint::Fill(1),
        ],
    )
    .header(header_row(&[
        unit,
        "    Source",
        "Destination",
        "  Change",
        " Identical",
        " Different",
        "  Only src",
        "  Only dst",
        "",
    ]))
    .column_spacing(1)
    .block(panel(title))
    .row_highlight_style(selected_style())
    .highlight_symbol("▶ ");
    render_table(f, table_r, t, state, clicks, 1);
    if buckets.is_empty() {
        let msg = if problems_only {
            "No problems in any period."
        } else {
            "No objects."
        };
        f.render_widget(
            Paragraph::new(msg)
                .style(Style::new().fg(OK))
                .alignment(Alignment::Center),
            centered(table_r, 40, 1),
        );
    }
}

fn draw_fields(
    f: &mut Frame,
    area: Rect,
    r: &crate::report::Report,
    state: &mut TableState,
    clicks: &mut Clicks,
) {
    let [info_r, table_r] =
        Layout::vertical([Constraint::Length(1), Constraint::Fill(1)]).areas(area);
    f.render_widget(
        Paragraph::new(Span::styled(
            " Which properties differ and in how many objects. Positions in lists are grouped as []. Enter lists the objects.",
            Style::new().fg(MUTED),
        )),
        info_r,
    );
    let rows: Vec<Row> = r
        .fields
        .iter()
        .map(|fs| {
            Row::new(vec![
                Cell::from(fs.path.clone()).bold(),
                num_cell(fs.objects() as u64, None),
                num_cell(fs.changed, Some(BAD)),
                num_cell(fs.type_changed, Some(BAD)),
                num_cell(fs.only_source, Some(WARN)),
                num_cell(fs.only_dest, Some(EXTRA)),
            ])
        })
        .collect();
    let t = Table::new(
        rows,
        [
            Constraint::Fill(1),
            Constraint::Length(10),
            Constraint::Length(14),
            Constraint::Length(13),
            Constraint::Length(17),
            Constraint::Length(17),
        ],
    )
    .header(header_row(&[
        "Field",
        "   Objects",
        " Value changed",
        " Type changed",
        " Missing in dest.",
        "  Extra in dest.",
    ]))
    .column_spacing(1)
    .block(panel("Differences by field"))
    .row_highlight_style(selected_style())
    .highlight_symbol("▶ ");
    render_table(f, table_r, t, state, clicks, 1);
    if r.fields.is_empty() {
        f.render_widget(
            Paragraph::new(
                "No property differences: every object found in both files has identical content.",
            )
            .style(Style::new().fg(OK))
            .alignment(Alignment::Center),
            centered(table_r, table_r.width.saturating_sub(4), 1),
        );
    }
}

fn draw_issues(f: &mut Frame, area: Rect, r: &crate::report::Report, scroll: u16) {
    let mut lines: Vec<Line> = Vec::new();
    let mut any = false;
    for (label, s) in [("Source", &r.source), ("Destination", &r.dest)] {
        lines.push(Line::from(Span::styled(
            format!("{label} file: {}", s.name()),
            Style::new().fg(ACCENT).bold(),
        )));
        let mut file_ok = true;
        if s.stopped_early {
            file_ok = false;
            lines.push(Line::from(Span::styled(
                "  ⚠ Reading stopped before the end of the file, so some objects may be missing from the comparison.",
                Style::new().fg(WARN),
            )));
        }
        if s.error_count > 0 {
            file_ok = false;
            lines.push(Line::from(Span::styled(
                format!(
                    "  ⚠ {} entries could not be read (they were skipped):",
                    fmt_num(s.error_count)
                ),
                Style::new().fg(WARN),
            )));
            for e in &s.errors {
                lines.push(Line::from(format!("      {}: {}", e.location, e.message)));
            }
            if s.error_count as usize > s.errors.len() {
                lines.push(Line::from(Span::styled(
                    format!(
                        "      … and {} more",
                        fmt_num(s.error_count - s.errors.len() as u64)
                    ),
                    Style::new().fg(MUTED),
                )));
            }
        }
        if s.duplicate_count > 0 {
            file_ok = false;
            lines.push(Line::from(Span::styled(
                format!(
                    "  ⚠ {} documents repeat an _id seen earlier; only the first one was compared:",
                    fmt_num(s.duplicate_count)
                ),
                Style::new().fg(WARN),
            )));
            let shown: Vec<&str> = s
                .duplicate_ids
                .iter()
                .take(50)
                .map(String::as_str)
                .collect();
            lines.push(Line::from(format!("      {}", shown.join(", "))));
        }
        if s.without_id > 0 {
            file_ok = false;
            lines.push(Line::from(Span::styled(
                format!(
                    "  ⚠ {} documents have no _id field and were skipped.",
                    fmt_num(s.without_id)
                ),
                Style::new().fg(WARN),
            )));
        }
        if r.created_field.name.is_some() && s.missing_created > 0 {
            lines.push(Line::from(Span::styled(
                format!("  ℹ {} documents have no usable created date; they are grouped under \"(no date)\".", fmt_num(s.missing_created)),
                Style::new().fg(MUTED),
            )));
        }
        if r.updated_field.name.is_some() && s.missing_updated > 0 {
            lines.push(Line::from(Span::styled(
                format!(
                    "  ℹ {} documents have no usable updated date.",
                    fmt_num(s.missing_updated)
                ),
                Style::new().fg(MUTED),
            )));
        }
        if file_ok {
            lines.push(Line::from(Span::styled(
                "  ✔ No problems reading this file.",
                Style::new().fg(OK),
            )));
        } else {
            any = true;
        }
        lines.push(Line::from(""));
    }
    if r.created_field.name.is_none() {
        lines.push(Line::from(Span::styled(
            "ℹ No created date field was found. Enter its name in the options (e.g. meta.createdAt) to group by created date.",
            Style::new().fg(MUTED),
        )));
    }
    if r.updated_field.name.is_none() {
        lines.push(Line::from(Span::styled(
            "ℹ No updated date field was found. Enter its name in the options to group by updated date.",
            Style::new().fg(MUTED),
        )));
    }
    let title = if any {
        "Warnings while reading the files"
    } else {
        "Reading the files"
    };
    f.render_widget(
        Paragraph::new(lines)
            .block(panel(title))
            .wrap(Wrap { trim: false })
            .scroll((scroll, 0)),
        area,
    );
}

fn draw_detail(
    f: &mut Frame,
    area: Rect,
    r: &crate::report::Report,
    d: &mut Detail,
    clicks: &mut Clicks,
) {
    let rec = &r.records[d.record()];
    let [head_r, table_r] =
        Layout::vertical([Constraint::Length(6), Constraint::Fill(1)]).areas(area);
    let date_pair = |a: Option<i64>, b: Option<i64>| {
        let same = a == b;
        vec![
            Span::styled("source ", Style::new().fg(MUTED)),
            Span::raw(fmt_datetime_short(a)),
            Span::styled("   destination ", Style::new().fg(MUTED)),
            Span::styled(
                fmt_datetime_short(b),
                if same || a.is_none() || b.is_none() {
                    Style::new()
                } else {
                    Style::new().fg(WARN)
                },
            ),
        ]
    };
    let mut created = vec![Span::styled("Created  ", Style::new().fg(MUTED))];
    created.extend(date_pair(rec.src_created, rec.dst_created));
    let mut updated = vec![Span::styled("Updated  ", Style::new().fg(MUTED))];
    updated.extend(date_pair(rec.src_updated, rec.dst_updated));
    let mode = match rec.status {
        Status::Different if d.all_fields => {
            "Showing all fields of both documents (a: only the differences)"
        }
        Status::Different => "Showing only the differences (a: all fields side by side)",
        Status::Matched => "",
        _ => "Showing all fields of the object",
    };
    let head = vec![
        Line::from(vec![
            Span::styled("Object  ", Style::new().fg(MUTED)),
            Span::styled(rec.id.to_string(), Style::new().bold()),
            Span::raw("   "),
            Span::styled(
                format!(" {} ", rec.status.label()),
                Style::new()
                    .fg(Color::Black)
                    .bg(status_color(rec.status))
                    .bold(),
            ),
            Span::styled(
                format!(
                    "   {} of {}  (← → to move)",
                    fmt_num(d.pos as u64 + 1),
                    fmt_num(d.siblings.len() as u64)
                ),
                Style::new().fg(MUTED),
            ),
        ]),
        Line::from(created),
        Line::from(updated),
        Line::from(Span::styled(mode, Style::new().fg(MUTED))),
    ];
    f.render_widget(Paragraph::new(head).block(panel("Object details")), head_r);

    if rec.status == Status::Matched {
        f.render_widget(
            Paragraph::new(vec![
                Line::from(Span::styled(
                    "✔ This object is identical in both files.",
                    Style::new().fg(OK).bold(),
                )),
                Line::from(""),
                Line::from(Span::styled(
                    "(Full contents are only kept for objects that differ.)",
                    Style::new().fg(MUTED),
                )),
            ])
            .alignment(Alignment::Center)
            .block(panel("Fields")),
            table_r,
        );
        return;
    }

    let rows: Vec<Row> = d
        .rows
        .iter()
        .map(|row| {
            let (label, color) = match row.kind {
                RowKind::Same => ("same", MUTED),
                RowKind::Ignored => ("ignored", MUTED),
                RowKind::Diff(k) => (k.label(), kind_color(k)),
            };
            let value = |v: &Option<String>, t: Option<&str>| match v {
                Some(v) => {
                    let ty = t.map(|t| format!("  ({t})")).unwrap_or_default();
                    let show_type = matches!(row.kind, RowKind::Diff(DiffKind::TypeChanged));
                    Cell::from(format!(
                        "{}{}",
                        truncate(v, 300),
                        if show_type { ty } else { String::new() }
                    ))
                }
                None => Cell::from("(not present)").style(Style::new().fg(MUTED)),
            };
            let base = if matches!(row.kind, RowKind::Same | RowKind::Ignored) {
                Style::new().fg(MUTED)
            } else {
                Style::new()
            };
            Row::new(vec![
                Cell::from(row.path.clone()).style(base.bold()),
                value(&row.source, row.source_type),
                value(&row.dest, row.dest_type),
                Cell::from(Span::styled(label, Style::new().fg(color))),
            ])
            .style(base)
        })
        .collect();
    let mut title = format!("Fields  ·  {} rows", fmt_num(d.rows.len() as u64));
    if rec.diffs_truncated && !d.all_fields {
        title.push_str(&format!(
            "  ·  only the first {} differences are listed",
            rec.diffs.len()
        ));
    }
    let t = Table::new(
        rows,
        [
            Constraint::Percentage(30),
            Constraint::Percentage(30),
            Constraint::Percentage(30),
            Constraint::Length(22),
        ],
    )
    .header(header_row(&["Field", "Source", "Destination", "Result"]))
    .column_spacing(2)
    .block(panel(title))
    .row_highlight_style(selected_style())
    .highlight_symbol("▶ ");
    render_table(f, table_r, t, &mut d.table, clicks, 1);
}

// ---------------------------------------------------------------------------
// Overlays

fn draw_popup(f: &mut Frame, body: Rect, app: &mut App) {
    let Some(p) = &app.popup else { return };
    let width = (body.width.saturating_sub(6)).min(100);
    let inner_w = width.saturating_sub(4).max(10) as usize;
    let wrapped: u16 = p
        .body
        .iter()
        .map(|l| (l.chars().count().max(1)).div_ceil(inner_w) as u16)
        .sum();
    let height = (wrapped + 5).min(body.height.saturating_sub(2));
    let area = centered(body, width, height);
    let color = match p.kind {
        PopupKind::Error => BAD,
        PopupKind::Exported { .. } => OK,
        PopupKind::ConfirmQuit | PopupKind::ConfirmNew => WARN,
        PopupKind::Info => ACCENT,
    };
    let hint = match p.kind {
        PopupKind::Exported { .. } => " o: open report   f: open folder   Esc: close ",
        PopupKind::ConfirmQuit | PopupKind::ConfirmNew => " y: yes    n: no ",
        PopupKind::Info => " c: copy   Esc: close ",
        PopupKind::Error => " Enter: close ",
    };
    let block = Block::bordered()
        .border_type(BorderType::Rounded)
        .border_style(Style::new().fg(color))
        .title(Span::styled(
            format!(" {} ", p.title),
            Style::new().fg(color).bold(),
        ))
        .title_bottom(
            Line::from(Span::styled(hint, Style::new().fg(MUTED))).alignment(Alignment::Right),
        );
    let lines: Vec<Line> = p
        .body
        .iter()
        .map(|l| {
            if l.chars().all(|c| c.is_ascii_uppercase() || c == ' ') && !l.trim().is_empty() {
                Line::from(Span::styled(l.clone(), Style::new().fg(ACCENT).bold()))
            } else {
                Line::from(l.clone())
            }
        })
        .collect();
    f.render_widget(Clear, area);
    f.render_widget(
        Paragraph::new(Text::from(lines))
            .wrap(Wrap { trim: false })
            .scroll((p.scroll, 0))
            .block(block.padding(ratatui::widgets::Padding::new(1, 1, 1, 0))),
        area,
    );
    // Clicking outside closes the popup, clicking inside does nothing.
    app.clickables.push((body, Clickable::ClosePopup));
    app.clickables.push((area, Clickable::Nothing));
}

fn draw_help(f: &mut Frame, body: Rect, app: &mut App) {
    let area = centered(body, 96, 37);
    let key = |k: &str, d: &str| {
        Line::from(vec![
            Span::styled(format!("  {k:<16}"), Style::new().fg(ACCENT).bold()),
            Span::raw(d.to_string()),
        ])
    };
    let head = |t: &str| {
        Line::from(Span::styled(
            t.to_string(),
            Style::new().bold().underlined(),
        ))
    };
    let lines = vec![
        head("How it works"),
        Line::from(
            "  Every object in the source file is matched with the object that has the same _id in the",
        ),
        Line::from(
            "  destination file, and all their properties (including nested ones) are compared.",
        ),
        Line::from(vec![
            Span::raw("  "),
            Span::styled("Identical", Style::new().fg(OK)),
            Span::raw(" same content  ·  "),
            Span::styled("Different", Style::new().fg(BAD)),
            Span::raw(" same _id, other content  ·  "),
            Span::styled("Only in source", Style::new().fg(WARN)),
            Span::raw(" missing  ·  "),
            Span::styled("Only in destination", Style::new().fg(EXTRA)),
            Span::raw(" extra"),
        ]),
        Line::from(""),
        head("Everywhere"),
        key(
            "↑ ↓  PgUp PgDn",
            "move through lists (the mouse wheel and clicks work too)",
        ),
        key("Enter  or  →", "open / drill down into the selected row"),
        key("Esc  or  ←", "go back one level"),
        key(
            "Tab  or  1-5",
            "switch between Summary, Mismatches, By date, Fields and Issues",
        ),
        key("e", "save the report as HTML and Excel-friendly CSV files"),
        key("n", "start a new comparison"),
        key("q  or  Ctrl+C", "quit"),
        Line::from(""),
        head("Lists of objects"),
        key("f", "filter by result (Different, Only in source, ...)"),
        key("/", "search for an object id (paste works)"),
        key("c", "copy the selected object id to the clipboard"),
        Line::from(""),
        head("Object details"),
        key("← →", "previous / next object in the list"),
        key(
            "a",
            "switch between \"only differences\" and \"all fields side by side\"",
        ),
        key("Enter", "show the full value of a field"),
        Line::from(""),
        head("By date"),
        key(
            "d",
            "group by created date, updated date or the time inside the ObjectId",
        ),
        key("m", "show only months / days with problems"),
        Line::from(""),
        head("Choosing files"),
        key(
            "Enter",
            "on a file box opens the file picker; you can also type, paste or drag a path",
        ),
        key("Ctrl+D", "create two sample files to try the tool"),
        Line::from(""),
        Line::from(Span::styled(
            "  Tip: to select text with the mouse, hold Shift (Option on macOS) while dragging.",
            Style::new().fg(MUTED),
        )),
    ];
    f.render_widget(Clear, area);
    f.render_widget(
        Paragraph::new(lines).block(
            panel("Help")
                .border_style(Style::new().fg(ACCENT))
                .title_bottom(
                    Line::from(Span::styled(
                        " press any key to close ",
                        Style::new().fg(MUTED),
                    ))
                    .alignment(Alignment::Right),
                ),
        ),
        area,
    );
    app.clickables.push((body, Clickable::ClosePopup));
}

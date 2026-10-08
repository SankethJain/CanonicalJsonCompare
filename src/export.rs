//! Writes the comparison report to files people can open and share:
//! an HTML page for reading and CSV files for Excel.

use std::fmt::Write as _;
use std::fs;
use std::io::{self, BufWriter, Write};
use std::path::{Path, PathBuf};

use crate::extjson::{fmt_datetime, fmt_datetime_short};
use crate::report::{DateBasis, Report, Status, build_date_index};
use crate::ui_text::{fmt_bytes, fmt_num};

/// Objects shown with full details in the HTML page; the CSV has all of them.
const HTML_OBJECT_LIMIT: usize = 2000;

pub struct ExportResult {
    pub folder: PathBuf,
    pub html: PathBuf,
}

/// Picks a writable folder: next to the source file, else the current
/// directory, else the home directory.
pub fn default_parent(report: &Report) -> PathBuf {
    let candidates = [
        report.source.path.parent().map(Path::to_path_buf),
        std::env::current_dir().ok(),
        dirs::home_dir(),
    ];
    for c in candidates.into_iter().flatten() {
        let c = if c.as_os_str().is_empty() {
            PathBuf::from(".")
        } else {
            c
        };
        let probe = c.join(".mongo-compare-write-test");
        if fs::write(&probe, b"").is_ok() {
            let _ = fs::remove_file(&probe);
            return c;
        }
    }
    PathBuf::from(".")
}

pub fn export(report: &Report, parent: &Path) -> io::Result<ExportResult> {
    let stamp = report.finished_at.format("%Y-%m-%d_%H%M%S");
    let folder = parent.join(format!("compare-report-{stamp}"));
    fs::create_dir_all(&folder)?;
    write_differences(report, &folder.join("differences.csv"))?;
    write_objects(report, &folder.join("objects.csv"))?;
    write_by_day(report, &folder.join("records-by-day.csv"))?;
    let html = folder.join("report.html");
    fs::write(&html, render_html(report))?;
    Ok(ExportResult { folder, html })
}

fn csv(s: &str) -> String {
    if s.contains([',', '"', '\n', '\r']) {
        format!("\"{}\"", s.replace('"', "\"\""))
    } else {
        s.to_string()
    }
}

fn csv_row(w: &mut impl Write, cells: &[&str]) -> io::Result<()> {
    let line: Vec<String> = cells.iter().map(|c| csv(c)).collect();
    writeln!(w, "{}", line.join(","))
}

fn write_differences(report: &Report, path: &Path) -> io::Result<()> {
    let mut w = BufWriter::new(fs::File::create(path)?);
    w.write_all("\u{feff}".as_bytes())?; // lets Excel detect UTF-8
    csv_row(
        &mut w,
        &[
            "object_id",
            "status",
            "field",
            "difference",
            "source_value",
            "destination_value",
            "source_type",
            "destination_type",
        ],
    )?;
    for r in &report.records {
        match r.status {
            Status::Matched => {}
            Status::OnlyInSource | Status::OnlyInDest => {
                csv_row(
                    &mut w,
                    &[
                        &r.id,
                        r.status.label(),
                        "(whole object)",
                        r.status.label(),
                        "",
                        "",
                        "",
                        "",
                    ],
                )?;
            }
            Status::Different => {
                for d in &r.diffs {
                    csv_row(
                        &mut w,
                        &[
                            &r.id,
                            r.status.label(),
                            &d.path,
                            d.kind.label(),
                            d.source.as_deref().unwrap_or(""),
                            d.dest.as_deref().unwrap_or(""),
                            d.source_type.unwrap_or(""),
                            d.dest_type.unwrap_or(""),
                        ],
                    )?;
                }
                if r.diffs_truncated {
                    csv_row(
                        &mut w,
                        &[
                            &r.id,
                            r.status.label(),
                            "(more differences not listed)",
                            "",
                            "",
                            "",
                            "",
                            "",
                        ],
                    )?;
                }
            }
        }
    }
    w.flush()
}

fn write_objects(report: &Report, path: &Path) -> io::Result<()> {
    let mut w = BufWriter::new(fs::File::create(path)?);
    w.write_all("\u{feff}".as_bytes())?;
    csv_row(
        &mut w,
        &[
            "object_id",
            "status",
            "differences",
            "created_source",
            "created_destination",
            "updated_source",
            "updated_destination",
        ],
    )?;
    for r in &report.records {
        csv_row(
            &mut w,
            &[
                &r.id,
                r.status.label(),
                &r.diffs.len().to_string(),
                &opt_date(r.src_created),
                &opt_date(r.dst_created),
                &opt_date(r.src_updated),
                &opt_date(r.dst_updated),
            ],
        )?;
    }
    w.flush()
}

fn opt_date(ms: Option<i64>) -> String {
    ms.map(fmt_datetime).unwrap_or_default()
}

fn write_by_day(report: &Report, path: &Path) -> io::Result<()> {
    let mut w = BufWriter::new(fs::File::create(path)?);
    w.write_all("\u{feff}".as_bytes())?;
    for basis in [DateBasis::Created, DateBasis::Updated] {
        let field = match basis {
            DateBasis::Created => &report.created_field,
            _ => &report.updated_field,
        };
        let Some(name) = &field.name else { continue };
        csv_row(
            &mut w,
            &[
                "date_field",
                "month",
                "day",
                "source",
                "destination",
                "matched",
                "different",
                "only_in_source",
                "only_in_destination",
            ],
        )?;
        for m in build_date_index(&report.records, basis) {
            for d in &m.days {
                csv_row(
                    &mut w,
                    &[
                        name,
                        &m.bucket.label,
                        &d.label,
                        &d.source.to_string(),
                        &d.dest.to_string(),
                        &d.counts.matched.to_string(),
                        &d.counts.different.to_string(),
                        &d.counts.only_source.to_string(),
                        &d.counts.only_dest.to_string(),
                    ],
                )?;
            }
        }
        writeln!(w)?;
    }
    w.flush()
}

fn esc(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}

fn render_html(r: &Report) -> String {
    let mut h = String::new();
    let verdict = if r.files_match() {
        "<div class=\"verdict ok\">&#10004; The files match</div>".to_string()
    } else {
        format!(
            "<div class=\"verdict bad\">&#10008; The files do not match &mdash; {} of {} objects differ</div>",
            fmt_num(r.counts.mismatched()),
            fmt_num(r.counts.total())
        )
    };
    let _ = write!(
        h,
        r#"<!doctype html><html lang="en"><head><meta charset="utf-8"><meta name="viewport" content="width=device-width,initial-scale=1">
<title>MongoDB export comparison</title><style>
:root{{--bg:#fff;--fg:#1d2433;--muted:#5d6678;--line:#dde2ea;--ok:#167a3e;--bad:#b42318;--warn:#a15c07;--card:#f6f8fb}}
@media (prefers-color-scheme:dark){{:root{{--bg:#13161c;--fg:#e6e9ef;--muted:#9aa3b2;--line:#2c323d;--ok:#4cc27a;--bad:#ff7a6e;--warn:#e5a54b;--card:#1b1f27}}}}
body{{font-family:system-ui,-apple-system,Segoe UI,Roboto,sans-serif;background:var(--bg);color:var(--fg);margin:0 auto;max-width:1200px;padding:24px 16px;line-height:1.45}}
h1{{font-size:1.6rem;margin:0 0 4px}} h2{{margin-top:2rem;font-size:1.2rem;border-bottom:1px solid var(--line);padding-bottom:4px}}
.muted{{color:var(--muted)}} .verdict{{font-size:1.3rem;font-weight:600;padding:14px 18px;border-radius:8px;margin:18px 0;background:var(--card)}}
.ok{{color:var(--ok)}} .bad{{color:var(--bad)}} .warn{{color:var(--warn)}}
.cards{{display:grid;grid-template-columns:repeat(auto-fit,minmax(170px,1fr));gap:12px}}
.card{{background:var(--card);border-radius:8px;padding:12px 14px}} .card b{{display:block;font-size:1.5rem}}
table{{border-collapse:collapse;width:100%;font-size:.92rem;margin:8px 0}} th,td{{text-align:left;padding:6px 8px;border-bottom:1px solid var(--line);vertical-align:top}}
th{{color:var(--muted);font-weight:600}} td.n,th.n{{text-align:right;font-variant-numeric:tabular-nums}}
code,.mono{{font-family:ui-monospace,Consolas,monospace;font-size:.88rem;word-break:break-all}}
details{{border-bottom:1px solid var(--line);padding:6px 0}} summary{{cursor:pointer}}
.wrap{{overflow-x:auto}}
</style></head><body>
<h1>MongoDB export comparison</h1>
<div class="muted">Generated {when} &middot; took {took:.1}s</div>
{verdict}
"#,
        when = r.finished_at.format("%Y-%m-%d %H:%M"),
        took = r.duration.as_secs_f64(),
    );

    let c = &r.counts;
    let _ = write!(
        h,
        r#"<div class="cards">
<div class="card">Objects in source<b>{}</b></div><div class="card">Objects in destination<b>{}</b></div>
<div class="card ok">Identical<b>{}</b></div><div class="card bad">Different<b>{}</b></div>
<div class="card warn">Only in source<b>{}</b></div><div class="card warn">Only in destination<b>{}</b></div></div>
"#,
        fmt_num(r.source.documents),
        fmt_num(r.dest.documents),
        fmt_num(c.matched),
        fmt_num(c.different),
        fmt_num(c.only_source),
        fmt_num(c.only_dest)
    );

    h.push_str("<h2>Files</h2><div class=\"wrap\"><table><tr><th></th><th>Source</th><th>Destination</th></tr>");
    let rows: Vec<(&str, String, String)> = vec![
        (
            "File",
            esc(&r.source.path.display().to_string()),
            esc(&r.dest.path.display().to_string()),
        ),
        ("Size", fmt_bytes(r.source.size), fmt_bytes(r.dest.size)),
        (
            "Documents",
            fmt_num(r.source.documents),
            fmt_num(r.dest.documents),
        ),
        (
            "Unreadable entries",
            fmt_num(r.source.error_count),
            fmt_num(r.dest.error_count),
        ),
        (
            "Duplicate _id",
            fmt_num(r.source.duplicate_count),
            fmt_num(r.dest.duplicate_count),
        ),
        (
            "Without _id",
            fmt_num(r.source.without_id),
            fmt_num(r.dest.without_id),
        ),
        (
            "Created between",
            format!(
                "{} &ndash; {}",
                fmt_datetime_short(r.source.first_created),
                fmt_datetime_short(r.source.last_created)
            ),
            format!(
                "{} &ndash; {}",
                fmt_datetime_short(r.dest.first_created),
                fmt_datetime_short(r.dest.last_created)
            ),
        ),
    ];
    for (label, a, b) in rows {
        let _ = write!(
            h,
            "<tr><th>{label}</th><td class=\"mono\">{a}</td><td class=\"mono\">{b}</td></tr>"
        );
    }
    h.push_str("</table></div>");
    let _ = write!(
        h,
        "<p class=\"muted\">Created date field: {} &middot; Updated date field: {} &middot; Ignored fields: {} &middot; Numbers: {} &middot; Lists: {} &middot; Dates grouped in UTC.</p>",
        esc(&r.created_field.describe()),
        esc(&r.updated_field.describe()),
        if r.options.ignore.is_empty() {
            "none".to_string()
        } else {
            esc(&r.options.ignore.join(", "))
        },
        if r.options.loose_numbers {
            "compared by value"
        } else {
            "type must match"
        },
        if r.options.ignore_array_order {
            "order ignored"
        } else {
            "order matters"
        },
    );

    if r.created_field.name.is_some() {
        h.push_str("<h2>Records per month (created date)</h2><div class=\"wrap\"><table><tr><th>Month</th><th class=n>Source</th><th class=n>Destination</th><th class=n>Identical</th><th class=n>Different</th><th class=n>Only in source</th><th class=n>Only in destination</th></tr>");
        for m in build_date_index(&r.records, DateBasis::Created) {
            let b = &m.bucket;
            let flag = |n: u64, cls: &str| {
                if n > 0 {
                    format!("<td class=\"n {cls}\">{}</td>", fmt_num(n))
                } else {
                    "<td class=\"n muted\">0</td>".to_string()
                }
            };
            let month = if b.counts.mismatched() > 0 || b.source != b.dest {
                format!("<td class=\"bad\">&#9679; {}</td>", b.label)
            } else {
                format!("<td>{}</td>", b.label)
            };
            let dest_cls = if b.source != b.dest { " warn" } else { "" };
            let _ = write!(
                h,
                "<tr>{month}<td class=n>{}</td><td class=\"n{dest_cls}\">{}</td>{}{}{}{}</tr>",
                fmt_num(b.source),
                fmt_num(b.dest),
                flag(b.counts.matched, "ok"),
                flag(b.counts.different, "bad"),
                flag(b.counts.only_source, "warn"),
                flag(b.counts.only_dest, "warn"),
            );
        }
        h.push_str(
            "</table></div><p class=\"muted\">Day by day figures are in records-by-day.csv.</p>",
        );
    }

    if !r.fields.is_empty() {
        h.push_str("<h2>Fields that differ most</h2><div class=\"wrap\"><table><tr><th>Field</th><th class=n>Objects</th><th class=n>Value changed</th><th class=n>Type changed</th><th class=n>Missing in destination</th><th class=n>Extra in destination</th></tr>");
        for f in r.fields.iter().take(100) {
            let _ = write!(
                h,
                "<tr><td class=mono>{}</td><td class=n>{}</td><td class=n>{}</td><td class=n>{}</td><td class=n>{}</td><td class=n>{}</td></tr>",
                esc(&f.path),
                fmt_num(f.objects() as u64),
                fmt_num(f.changed),
                fmt_num(f.type_changed),
                fmt_num(f.only_source),
                fmt_num(f.only_dest)
            );
        }
        h.push_str("</table></div>");
    }

    let mismatched = r.mismatched();
    if !mismatched.is_empty() {
        let _ = write!(
            h,
            "<h2>Objects that do not match ({})</h2>",
            fmt_num(mismatched.len() as u64)
        );
        if mismatched.len() > HTML_OBJECT_LIMIT {
            let _ = write!(
                h,
                "<p class=\"warn\">Showing the first {} objects. The complete list is in differences.csv.</p>",
                fmt_num(HTML_OBJECT_LIMIT as u64)
            );
        }
        for &i in mismatched.iter().take(HTML_OBJECT_LIMIT) {
            let rec = &r.records[i];
            let note = match rec.status {
                Status::Different => format!("{} differences", rec.diffs.len()),
                s => s.label().to_string(),
            };
            let _ = write!(
                h,
                "<details><summary><code>{}</code> &mdash; <span class=\"{}\">{}</span></summary>",
                esc(&rec.id),
                if rec.status == Status::Different {
                    "bad"
                } else {
                    "warn"
                },
                esc(&note)
            );
            if rec.status == Status::Different {
                h.push_str("<div class=\"wrap\"><table><tr><th>Field</th><th>Difference</th><th>Source</th><th>Destination</th></tr>");
                for d in &rec.diffs {
                    let _ = write!(
                        h,
                        "<tr><td class=mono>{}</td><td>{}</td><td class=mono>{}</td><td class=mono>{}</td></tr>",
                        esc(&d.path),
                        d.kind.label(),
                        esc(d.source.as_deref().unwrap_or("—")),
                        esc(d.dest.as_deref().unwrap_or("—"))
                    );
                }
                h.push_str("</table></div>");
            }
            h.push_str("</details>");
        }
    }

    if r.has_warnings() {
        h.push_str("<h2 class=\"warn\">Warnings</h2><ul>");
        for (name, s) in [("Source", &r.source), ("Destination", &r.dest)] {
            for e in s.errors.iter().take(200) {
                let _ = write!(
                    h,
                    "<li>{name}, {}: {}</li>",
                    esc(&e.location),
                    esc(&e.message)
                );
            }
            if s.duplicate_count > 0 {
                let _ = write!(
                    h,
                    "<li>{name}: {} documents share an _id with an earlier document; only the first was compared.</li>",
                    fmt_num(s.duplicate_count)
                );
            }
            if s.without_id > 0 {
                let _ = write!(
                    h,
                    "<li>{name}: {} documents have no _id and were skipped.</li>",
                    fmt_num(s.without_id)
                );
            }
        }
        h.push_str("</ul>");
    }
    h.push_str("</body></html>");
    h
}

/// Opens a file or folder with the system's default application.
pub fn open_in_system(path: &Path) -> io::Result<()> {
    #[cfg(target_os = "windows")]
    let mut cmd = {
        let mut c = std::process::Command::new("cmd");
        c.args(["/C", "start", ""]).arg(path);
        c
    };
    #[cfg(target_os = "macos")]
    let mut cmd = {
        let mut c = std::process::Command::new("open");
        c.arg(path);
        c
    };
    #[cfg(not(any(target_os = "windows", target_os = "macos")))]
    let mut cmd = {
        let mut c = std::process::Command::new("xdg-open");
        c.arg(path);
        c
    };
    cmd.stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
        .map(|_| ())
}

//! mongo-compare: compare two MongoDB exports object by object.

mod config;
mod demo;
mod diff;
mod engine;
mod export;
mod extjson;
mod loader;
mod report;
mod tui;
mod ui_text;

use std::path::PathBuf;
use std::process::ExitCode;

use clap::Parser;

use crate::config::{Settings, clean_path, parse_list};
use crate::diff::DiffOptions;
use crate::engine::{CompareRequest, Progress};
use crate::report::Status;
use crate::ui_text::fmt_num;

/// The program version. Release builds take it from the git tag (the
/// release workflow sets `MONGO_COMPARE_VERSION`), so a tag such as `v0.2.0`
/// is always the version people see; other builds use `Cargo.toml`.
pub const VERSION: &str = match option_env!("MONGO_COMPARE_VERSION") {
    Some(v) => v,
    None => env!("CARGO_PKG_VERSION"),
};

/// Compare two MongoDB exports (mongoexport, canonical or relaxed Extended
/// JSON) object by object using _id.
///
/// Run without arguments to open the interactive screen and pick the files.
#[derive(Parser, Debug)]
#[command(version = VERSION, about, long_about = None)]
struct Cli {
    /// The source file (the original / expected data).
    source: Option<PathBuf>,
    /// The destination file (the copy to check).
    destination: Option<PathBuf>,

    /// Field holding the created date (default: detected automatically).
    #[arg(long, value_name = "FIELD")]
    created_field: Option<String>,
    /// Field holding the updated date (default: detected automatically).
    #[arg(long, value_name = "FIELD")]
    updated_field: Option<String>,
    /// Comma separated fields to ignore, e.g. "__v,meta.syncedAt".
    #[arg(long, value_name = "FIELDS")]
    ignore: Option<String>,
    /// Treat numbers with the same value as equal regardless of type.
    #[arg(long)]
    loose_numbers: bool,
    /// Ignore the order of items inside lists.
    #[arg(long)]
    ignore_array_order: bool,

    /// Compare without the interactive screen and print a summary.
    /// Exit code: 0 = files match, 1 = files differ, 2 = error.
    #[arg(long)]
    check: bool,
    /// With --check: also save the report (HTML + CSV) in this folder.
    #[arg(long, value_name = "FOLDER", requires = "check")]
    export: Option<PathBuf>,

    /// Create two sample export files and open them, to try the tool.
    #[arg(long)]
    demo: bool,
}

fn main() -> ExitCode {
    let mut cli = Cli::parse();
    if cli.demo && cli.check {
        match demo::generate(&std::env::temp_dir().join("mongo-compare-demo"), 2500) {
            Ok((src, dst)) => {
                cli.source = Some(src);
                cli.destination = Some(dst);
            }
            Err(e) => {
                eprintln!("Could not create sample files: {e}");
                return ExitCode::from(2);
            }
        }
    }
    if cli.check {
        return run_check(&cli);
    }

    let mut app = tui::App::new(Settings::load());
    app.apply_overrides(
        cli.created_field.clone(),
        cli.updated_field.clone(),
        cli.ignore.clone(),
        cli.loose_numbers,
        cli.ignore_array_order,
    );
    let mut start_now = false;
    if cli.demo {
        app.load_demo();
    } else if cli.source.is_some() || cli.destination.is_some() {
        let src = cli
            .source
            .as_deref()
            .map(|p| clean_path(&p.to_string_lossy()));
        let dst = cli
            .destination
            .as_deref()
            .map(|p| clean_path(&p.to_string_lossy()));
        start_now = src.is_some() && dst.is_some();
        app.set_paths(src.as_deref(), dst.as_deref());
    }
    match tui::run(app, start_now) {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("mongo-compare: {e}");
            ExitCode::from(2)
        }
    }
}

fn run_check(cli: &Cli) -> ExitCode {
    let (Some(source), Some(dest)) = (&cli.source, &cli.destination) else {
        eprintln!("--check needs both a SOURCE and a DESTINATION file.");
        return ExitCode::from(2);
    };
    let request = CompareRequest {
        source: source.clone(),
        dest: dest.clone(),
        created_field: cli.created_field.clone().unwrap_or_default(),
        updated_field: cli.updated_field.clone().unwrap_or_default(),
        options: DiffOptions {
            ignore: cli.ignore.as_deref().map(parse_list).unwrap_or_default(),
            loose_numbers: cli.loose_numbers,
            ignore_array_order: cli.ignore_array_order,
            ..DiffOptions::default()
        },
    };
    let report = match engine::run(&request, &Progress::default()) {
        Ok(r) => r,
        Err(e) => {
            eprintln!("{e}");
            return ExitCode::from(2);
        }
    };
    let c = &report.counts;
    println!(
        "Source:       {}  ({} objects)",
        report.source.path.display(),
        fmt_num(report.source.documents)
    );
    println!(
        "Destination:  {}  ({} objects)",
        report.dest.path.display(),
        fmt_num(report.dest.documents)
    );
    println!();
    println!("Identical:            {:>12}", fmt_num(c.matched));
    println!("Different:            {:>12}", fmt_num(c.different));
    println!("Only in source:       {:>12}", fmt_num(c.only_source));
    println!("Only in destination:  {:>12}", fmt_num(c.only_dest));
    println!();
    if report.files_match() {
        println!("RESULT: the files match.");
    } else {
        println!(
            "RESULT: the files do NOT match ({} objects).",
            fmt_num(c.mismatched())
        );
        for r in report
            .records
            .iter()
            .filter(|r| r.status != Status::Matched)
            .take(20)
        {
            let detail = match r.status {
                Status::Different => {
                    let fields: Vec<&str> =
                        r.diffs.iter().take(5).map(|d| d.path.as_str()).collect();
                    format!("differs in {}", fields.join(", "))
                }
                s => s.label().to_lowercase(),
            };
            println!("  {}  {detail}", r.id);
        }
        if c.mismatched() > 20 {
            println!("  … and {} more", fmt_num(c.mismatched() - 20));
        }
    }
    for (name, s) in [("source", &report.source), ("destination", &report.dest)] {
        if s.has_warnings() {
            println!(
                "Warning ({name}): {} unreadable entries, {} duplicate _id, {} without _id.",
                fmt_num(s.error_count),
                fmt_num(s.duplicate_count),
                fmt_num(s.without_id)
            );
        }
    }
    if let Some(dir) = &cli.export {
        match export::export(&report, dir) {
            Ok(out) => println!("Report saved to {}", out.folder.display()),
            Err(e) => {
                eprintln!("Could not save the report: {e}");
                return ExitCode::from(2);
            }
        }
    }
    if report.files_match() {
        ExitCode::SUCCESS
    } else {
        ExitCode::from(1)
    }
}

//! Runs a full comparison of two export files.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU8, AtomicU64, Ordering};
use std::time::Instant;

use chrono::Local;
use serde::Deserialize;
use serde_json::Value;

use crate::diff::{DiffOptions, diff_documents};
use crate::extjson;
use crate::loader::{self, Cancelled, Location};
use crate::report::{Counts, DateField, FileStats, Record, Report, Status};

const CREATED_CANDIDATES: &[&str] = &[
    "createdat",
    "created_at",
    "createddate",
    "created_date",
    "createdon",
    "created_on",
    "datecreated",
    "date_created",
    "creationdate",
    "creation_date",
    "createdtime",
    "created_time",
    "createdatetime",
    "createdate",
    "create_date",
    "insertedat",
    "inserted_at",
    "created",
];

const UPDATED_CANDIDATES: &[&str] = &[
    "updatedat",
    "updated_at",
    "updateddate",
    "updated_date",
    "updatedon",
    "updated_on",
    "modifiedat",
    "modified_at",
    "modifieddate",
    "modified_date",
    "modifiedon",
    "lastmodified",
    "last_modified",
    "lastmodifieddate",
    "last_modified_date",
    "lastupdated",
    "last_updated",
    "lastupdatedat",
    "datemodified",
    "date_modified",
    "updatedate",
    "update_date",
    "updatedtime",
    "updated_time",
    "modified",
    "updated",
];

/// How many documents are inspected to guess the date fields.
const SAMPLE_SIZE: usize = 500;
/// Duplicate ids kept for display (all are counted).
const MAX_KEPT_DUPLICATES: usize = 1000;

#[derive(Debug, Clone)]
pub struct CompareRequest {
    pub source: PathBuf,
    pub dest: PathBuf,
    /// Empty means "detect automatically".
    pub created_field: String,
    pub updated_field: String,
    pub options: DiffOptions,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Stage {
    ReadingSource = 0,
    ComparingDest = 1,
    Finishing = 2,
}

/// Shared progress counters, updated by the worker thread.
#[derive(Default)]
pub struct Progress {
    stage: AtomicU8,
    pub source_bytes: Arc<AtomicU64>,
    pub dest_bytes: Arc<AtomicU64>,
    pub source_total: AtomicU64,
    pub dest_total: AtomicU64,
    pub documents: AtomicU64,
    pub cancel: AtomicBool,
}

impl Progress {
    pub fn stage(&self) -> Stage {
        match self.stage.load(Ordering::Relaxed) {
            0 => Stage::ReadingSource,
            1 => Stage::ComparingDest,
            _ => Stage::Finishing,
        }
    }
    fn set_stage(&self, s: Stage) {
        self.stage.store(s as u8, Ordering::Relaxed);
    }
    /// Overall completion between 0 and 1 (bytes read of both files).
    pub fn ratio(&self) -> f64 {
        let total =
            self.source_total.load(Ordering::Relaxed) + self.dest_total.load(Ordering::Relaxed);
        if total == 0 {
            return 0.0;
        }
        let done =
            self.source_bytes.load(Ordering::Relaxed) + self.dest_bytes.load(Ordering::Relaxed);
        (done as f64 / total as f64).clamp(0.0, 1.0)
    }
}

/// Counts how often each candidate field holds a date in the sampled documents.
#[derive(Default)]
struct FieldSniffer {
    seen: usize,
    created: HashMap<String, usize>,
    updated: HashMap<String, usize>,
}

impl FieldSniffer {
    fn observe(&mut self, doc: &Value) {
        if self.seen >= SAMPLE_SIZE {
            return;
        }
        self.seen += 1;
        let Some(map) = doc.as_object() else { return };
        for (key, v) in map {
            let lower = key.to_ascii_lowercase();
            let is_created = CREATED_CANDIDATES.contains(&lower.as_str());
            let is_updated = UPDATED_CANDIDATES.contains(&lower.as_str());
            if (is_created || is_updated) && extjson::date_like_millis(v).is_some() {
                let target = if is_created {
                    &mut self.created
                } else {
                    &mut self.updated
                };
                *target.entry(key.clone()).or_default() += 1;
            }
        }
    }

    fn best(counts: &HashMap<String, usize>, candidates: &[&str]) -> Option<String> {
        counts
            .iter()
            .max_by(|(ka, ca), (kb, cb)| {
                ca.cmp(cb).then_with(|| {
                    // Prefer names earlier in the candidate list on a tie.
                    let pos = |k: &String| {
                        candidates
                            .iter()
                            .position(|c| *c == k.to_ascii_lowercase())
                            .unwrap_or(usize::MAX)
                    };
                    pos(kb).cmp(&pos(ka))
                })
            })
            .map(|(k, _)| k.clone())
    }
}

fn choose_field(configured: &str, detected: Option<String>) -> DateField {
    let configured = configured.trim();
    if configured.is_empty() || configured.eq_ignore_ascii_case("auto") {
        DateField {
            name: detected,
            auto: true,
        }
    } else {
        DateField {
            name: Some(configured.to_string()),
            auto: false,
        }
    }
}

fn read_date(doc: &Value, field: &DateField) -> Option<i64> {
    extjson::date_like_millis(extjson::get_path(doc, field.name.as_deref()?)?)
}

/// Only the `_id` of a document; everything else is skipped while parsing.
#[derive(Deserialize)]
struct IdOnly {
    #[serde(rename = "_id")]
    id: Option<Value>,
}

struct SourceEntry {
    order: u64,
    raw: Box<str>,
}

/// Documents are handed to the worker threads in batches of this size.
const BATCH: usize = 4096;

/// Runs `f` over `items` on all CPU cores, keeping the order.
fn par_map<T: Send, R: Send>(items: Vec<T>, f: impl Fn(T) -> R + Sync) -> Vec<R> {
    let threads = std::thread::available_parallelism()
        .map_or(1, |n| n.get())
        .min(16);
    if threads <= 1 || items.len() < 64 {
        return items.into_iter().map(f).collect();
    }
    let size = items.len().div_ceil(threads);
    let mut chunks: Vec<Vec<T>> = Vec::with_capacity(threads);
    let mut iter = items.into_iter();
    loop {
        let chunk: Vec<T> = iter.by_ref().take(size).collect();
        if chunk.is_empty() {
            break;
        }
        chunks.push(chunk);
    }
    let f = &f;
    std::thread::scope(|scope| {
        let handles: Vec<_> = chunks
            .into_iter()
            .map(|chunk| scope.spawn(move || chunk.into_iter().map(f).collect::<Vec<R>>()))
            .collect();
        handles
            .into_iter()
            .flat_map(|h| h.join().expect("comparison worker failed"))
            .collect()
    })
}

fn not_an_object(raw: &str) -> Option<String> {
    (!raw.trim_start().starts_with('{'))
        .then(|| "not a JSON object (each document must start with '{')".to_string())
}

fn parse_error(e: &serde_json::Error) -> String {
    loader::friendly_json_error(e, false)
}

/// Reads just the `_id` key of a document.
fn parse_id(raw: &str) -> Result<Option<String>, String> {
    if let Some(msg) = not_an_object(raw) {
        return Err(msg);
    }
    let doc: IdOnly = serde_json::from_str(raw).map_err(|e| parse_error(&e))?;
    Ok(doc.id.as_ref().map(extjson::id_key))
}

fn parse_doc(raw: &str) -> Result<Value, String> {
    if let Some(msg) = not_an_object(raw) {
        return Err(msg);
    }
    serde_json::from_str(raw).map_err(|e| parse_error(&e))
}

fn file_size(p: &Path) -> u64 {
    std::fs::metadata(p).map(|m| m.len()).unwrap_or(0)
}

fn open_error(which: &str, path: &Path, e: std::io::Error) -> String {
    let reason = match e.kind() {
        std::io::ErrorKind::NotFound => "the file does not exist".to_string(),
        std::io::ErrorKind::PermissionDenied => "permission denied".to_string(),
        _ => e.to_string(),
    };
    format!(
        "Could not read the {which} file {}: {reason}",
        path.display()
    )
}

fn note_duplicate(stats: &mut FileStats, key: String) {
    stats.duplicate_count += 1;
    if stats.duplicate_ids.len() < MAX_KEPT_DUPLICATES {
        stats.duplicate_ids.push(key);
    }
}

/// Stage 1: the source file, kept as text and keyed by `_id`.
struct SourceSide {
    stats: FileStats,
    map: HashMap<String, SourceEntry>,
    sniffer: FieldSniffer,
    order: u64,
}

impl SourceSide {
    fn flush(&mut self, batch: Vec<(Box<str>, Location)>) {
        let parsed = par_map(batch, |(raw, loc)| {
            let id = parse_id(&raw);
            (raw, loc, id)
        });
        for (raw, loc, id) in parsed {
            match id {
                Err(msg) => {
                    let s = &mut self.stats;
                    loader::push_error(&mut s.errors, &mut s.error_count, loc.to_string(), msg);
                }
                Ok(id) => {
                    self.stats.documents += 1;
                    if self.sniffer.seen < SAMPLE_SIZE
                        && let Ok(v) = serde_json::from_str::<Value>(&raw)
                    {
                        self.sniffer.observe(&v);
                    }
                    match id {
                        None => self.stats.without_id += 1,
                        Some(key) if self.map.contains_key(&key) => {
                            note_duplicate(&mut self.stats, key)
                        }
                        Some(key) => {
                            self.map.insert(
                                key,
                                SourceEntry {
                                    order: self.order,
                                    raw,
                                },
                            );
                            self.order += 1;
                        }
                    }
                }
            }
        }
    }
}

/// One destination document waiting to be compared with its source.
struct Job {
    key: String,
    raw: Box<str>,
    doc: Value,
    source: Option<SourceEntry>,
}

/// Stage 2: the destination file, streamed and compared batch by batch.
struct DestSide {
    stats: FileStats,
    seen: HashSet<String>,
    records: Vec<(u64, Record)>,
    dest_order: u64,
    created: DateField,
    updated: DateField,
    detect_fields: bool,
    configured: (String, String),
}

impl DestSide {
    fn flush(
        &mut self,
        batch: Vec<(Box<str>, Location)>,
        source: &mut HashMap<String, SourceEntry>,
        opts: &DiffOptions,
    ) {
        let parsed = par_map(batch, |(raw, loc)| {
            let doc = parse_doc(&raw);
            (raw, loc, doc)
        });
        let mut jobs = Vec::with_capacity(parsed.len());
        for (raw, loc, doc) in parsed {
            let doc = match doc {
                Ok(d) => d,
                Err(msg) => {
                    let s = &mut self.stats;
                    loader::push_error(&mut s.errors, &mut s.error_count, loc.to_string(), msg);
                    continue;
                }
            };
            self.stats.documents += 1;
            if self.detect_fields {
                // The source had no documents: guess the date fields from here.
                self.detect_fields = false;
                let mut sniff = FieldSniffer::default();
                sniff.observe(&doc);
                self.created = choose_field(
                    &self.configured.0,
                    FieldSniffer::best(&sniff.created, CREATED_CANDIDATES),
                );
                self.updated = choose_field(
                    &self.configured.1,
                    FieldSniffer::best(&sniff.updated, UPDATED_CANDIDATES),
                );
            }
            let Some(id) = doc.get("_id") else {
                self.stats.without_id += 1;
                continue;
            };
            let key = extjson::id_key(id);
            if !self.seen.insert(key.clone()) {
                note_duplicate(&mut self.stats, key);
                continue;
            }
            let source = source.remove(&key);
            jobs.push(Job {
                key,
                raw,
                doc,
                source,
            });
        }
        let (created, updated) = (&self.created, &self.updated);
        let results = par_map(jobs, |job| compare_job(job, opts, created, updated));
        for (order, record) in results {
            let order = order.unwrap_or_else(|| {
                self.dest_order += 1;
                u64::MAX / 2 + self.dest_order
            });
            self.records.push((order, record));
        }
    }
}

fn compare_job(
    job: Job,
    opts: &DiffOptions,
    created: &DateField,
    updated: &DateField,
) -> (Option<u64>, Record) {
    let id_time = job
        .doc
        .get("_id")
        .and_then(extjson::oid_hex)
        .and_then(extjson::oid_timestamp_millis);
    let dst_created = read_date(&job.doc, created);
    let dst_updated = read_date(&job.doc, updated);
    match job.source {
        Some(src) => {
            let src_doc: Value = serde_json::from_str(&src.raw).unwrap_or(Value::Null);
            let result = diff_documents(&src_doc, &job.doc, opts);
            let matched = result.diffs.is_empty();
            let record = Record {
                id: job.key.into(),
                status: if matched {
                    Status::Matched
                } else {
                    Status::Different
                },
                src_created: read_date(&src_doc, created),
                src_updated: read_date(&src_doc, updated),
                dst_created,
                dst_updated,
                id_time,
                diffs: result.diffs,
                diffs_truncated: result.truncated,
                src_raw: (!matched).then_some(src.raw),
                dst_raw: (!matched).then_some(job.raw),
            };
            (Some(src.order), record)
        }
        None => (
            None,
            Record {
                id: job.key.into(),
                status: Status::OnlyInDest,
                src_created: None,
                src_updated: None,
                dst_created,
                dst_updated,
                id_time,
                diffs: Vec::new(),
                diffs_truncated: false,
                src_raw: None,
                dst_raw: Some(job.raw),
            },
        ),
    }
}

fn only_in_source(
    key: String,
    src: SourceEntry,
    created: &DateField,
    updated: &DateField,
) -> (u64, Record) {
    let doc: Value = serde_json::from_str(&src.raw).unwrap_or(Value::Null);
    let id_time = doc
        .get("_id")
        .and_then(extjson::oid_hex)
        .and_then(extjson::oid_timestamp_millis);
    let record = Record {
        id: key.into(),
        status: Status::OnlyInSource,
        src_created: read_date(&doc, created),
        src_updated: read_date(&doc, updated),
        dst_created: None,
        dst_updated: None,
        id_time,
        diffs: Vec::new(),
        diffs_truncated: false,
        src_raw: Some(src.raw),
        dst_raw: None,
    };
    (src.order, record)
}

fn copy_summary(stats: &mut FileStats, summary: loader::ReadSummary) {
    stats.format = Some(summary.format);
    // Structural problems found by the reader come after per-document ones.
    for e in summary.errors {
        loader::push_error(
            &mut stats.errors,
            &mut stats.error_count,
            e.location,
            e.message,
        );
    }
    stats.stopped_early = summary.stopped_early;
}

pub fn run(req: &CompareRequest, progress: &Progress) -> Result<Report, String> {
    let started = Instant::now();
    let opts = &req.options;
    let cancelled = || progress.cancel.load(Ordering::Relaxed);
    progress
        .source_total
        .store(file_size(&req.source), Ordering::Relaxed);
    progress
        .dest_total
        .store(file_size(&req.dest), Ordering::Relaxed);

    // --- Stage 1: load the source file, keyed by _id -----------------------
    progress.set_stage(Stage::ReadingSource);
    let mut src = SourceSide {
        stats: FileStats {
            path: req.source.clone(),
            size: file_size(&req.source),
            ..Default::default()
        },
        map: HashMap::new(),
        sniffer: FieldSniffer::default(),
        order: 0,
    };
    let mut batch = Vec::with_capacity(BATCH);
    let summary = loader::read_documents(
        &req.source,
        progress.source_bytes.clone(),
        &progress.cancel,
        |raw, loc| {
            progress.documents.fetch_add(1, Ordering::Relaxed);
            batch.push((raw, loc));
            if batch.len() >= BATCH {
                src.flush(std::mem::replace(&mut batch, Vec::with_capacity(BATCH)));
                if cancelled() {
                    return Err(Cancelled);
                }
            }
            Ok(())
        },
    )
    .map_err(|e| open_error("source", &req.source, e))?;
    src.flush(batch);
    if cancelled() {
        return Err("Comparison cancelled.".into());
    }
    copy_summary(&mut src.stats, summary);

    // --- Stage 2: stream the destination file and compare ------------------
    progress.set_stage(Stage::ComparingDest);
    let mut dst = DestSide {
        stats: FileStats {
            path: req.dest.clone(),
            size: file_size(&req.dest),
            ..Default::default()
        },
        seen: HashSet::new(),
        records: Vec::with_capacity(src.map.len()),
        dest_order: 0,
        created: choose_field(
            &req.created_field,
            FieldSniffer::best(&src.sniffer.created, CREATED_CANDIDATES),
        ),
        updated: choose_field(
            &req.updated_field,
            FieldSniffer::best(&src.sniffer.updated, UPDATED_CANDIDATES),
        ),
        detect_fields: src.sniffer.seen == 0,
        configured: (req.created_field.clone(), req.updated_field.clone()),
    };
    let mut source = std::mem::take(&mut src.map);
    let mut batch = Vec::with_capacity(BATCH);
    let summary = loader::read_documents(
        &req.dest,
        progress.dest_bytes.clone(),
        &progress.cancel,
        |raw, loc| {
            progress.documents.fetch_add(1, Ordering::Relaxed);
            batch.push((raw, loc));
            if batch.len() >= BATCH {
                dst.flush(
                    std::mem::replace(&mut batch, Vec::with_capacity(BATCH)),
                    &mut source,
                    opts,
                );
                if cancelled() {
                    return Err(Cancelled);
                }
            }
            Ok(())
        },
    )
    .map_err(|e| open_error("destination", &req.dest, e))?;
    dst.flush(batch, &mut source, opts);
    if cancelled() {
        return Err("Comparison cancelled.".into());
    }
    copy_summary(&mut dst.stats, summary);

    // --- Stage 3: whatever is left in the source was not in the destination
    progress.set_stage(Stage::Finishing);
    let (created, updated) = (dst.created.clone(), dst.updated.clone());
    let leftovers: Vec<(String, SourceEntry)> = source.drain().collect();
    let mut records = dst.records;
    records.extend(par_map(leftovers, |(key, entry)| {
        only_in_source(key, entry, &created, &updated)
    }));
    records.sort_unstable_by_key(|(o, _)| *o);
    let records: Vec<Record> = records.into_iter().map(|(_, r)| r).collect();
    let (mut src_stats, mut dst_stats) = (src.stats, dst.stats);

    let mut counts = Counts::default();
    for r in &records {
        counts.add(r.status);
        if r.in_source() {
            track_dates(
                &mut src_stats,
                r.src_created,
                r.src_updated,
                created.name.is_some(),
                updated.name.is_some(),
            );
        }
        if r.in_dest() {
            track_dates(
                &mut dst_stats,
                r.dst_created,
                r.dst_updated,
                created.name.is_some(),
                updated.name.is_some(),
            );
        }
    }
    let fields = Report::compute_field_stats(&records);

    Ok(Report {
        source: src_stats,
        dest: dst_stats,
        records,
        counts,
        created_field: created,
        updated_field: updated,
        options: opts.clone(),
        duration: started.elapsed(),
        finished_at: Local::now(),
        fields,
    })
}

fn track_dates(
    stats: &mut FileStats,
    created: Option<i64>,
    updated: Option<i64>,
    has_c: bool,
    has_u: bool,
) {
    match created {
        Some(c) => {
            stats.first_created = Some(stats.first_created.map_or(c, |f| f.min(c)));
            stats.last_created = Some(stats.last_created.map_or(c, |l| l.max(c)));
        }
        None if has_c => stats.missing_created += 1,
        None => {}
    }
    if updated.is_none() && has_u {
        stats.missing_updated += 1;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    fn write(dir: &Path, name: &str, lines: &[&str]) -> PathBuf {
        let p = dir.join(name);
        let mut f = std::fs::File::create(&p).unwrap();
        for l in lines {
            writeln!(f, "{l}").unwrap();
        }
        p
    }

    #[test]
    fn end_to_end() {
        let dir = std::env::temp_dir().join(format!("mc-engine-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let src = write(
            &dir,
            "src.json",
            &[
                r#"{"_id":{"$oid":"65f0a1b2c3d4e5f601234561"},"name":"a","createdAt":{"$date":{"$numberLong":"1710499327000"}}}"#,
                r#"{"_id":{"$oid":"65f0a1b2c3d4e5f601234562"},"name":"b","createdAt":{"$date":{"$numberLong":"1710499327000"}}}"#,
                r#"{"_id":{"$oid":"65f0a1b2c3d4e5f601234563"},"name":"c","createdAt":{"$date":{"$numberLong":"1710499327000"}}}"#,
                r#"{"_id":{"$oid":"65f0a1b2c3d4e5f601234563"},"name":"dup"}"#,
                r#"{"_id": broken"#,
                r#"[1, 2]"#,
            ],
        );
        let dst = write(
            &dir,
            "dst.json",
            &[
                r#"{"_id":{"$oid":"65f0a1b2c3d4e5f601234562"},"name":"B","createdAt":{"$date":"2024-03-15T10:42:07Z"}}"#,
                r#"{"_id":{"$oid":"65f0a1b2c3d4e5f601234561"},"name":"a","createdAt":{"$date":"2024-03-15T10:42:07Z"}}"#,
                r#"{"_id":{"$oid":"65f0a1b2c3d4e5f601234564"},"name":"d"}"#,
            ],
        );
        let req = CompareRequest {
            source: src,
            dest: dst,
            created_field: String::new(),
            updated_field: String::new(),
            options: DiffOptions::default(),
        };
        let report = run(&req, &Progress::default()).unwrap();
        assert_eq!(report.source.documents, 4);
        assert_eq!(report.source.duplicate_count, 1);
        assert_eq!(report.source.error_count, 2);
        assert_eq!(report.source.errors[0].location, "line 5");
        assert_eq!(report.dest.documents, 3);
        assert_eq!(report.counts.matched, 1);
        assert_eq!(report.counts.different, 1);
        assert_eq!(report.counts.only_source, 1);
        assert_eq!(report.counts.only_dest, 1);
        assert_eq!(report.created_field.name.as_deref(), Some("createdAt"));
        // Records keep source order, destination-only records come last.
        let ids: Vec<&str> = report.records.iter().map(|r| &*r.id).collect();
        assert_eq!(
            ids,
            [
                "65f0a1b2c3d4e5f601234561",
                "65f0a1b2c3d4e5f601234562",
                "65f0a1b2c3d4e5f601234563",
                "65f0a1b2c3d4e5f601234564"
            ]
        );
        assert_eq!(report.fields[0].path, "name");
        let months =
            crate::report::build_date_index(&report.records, crate::report::DateBasis::Created);
        assert_eq!(months[0].bucket.label, "2024-03");
        assert_eq!(months[0].bucket.source, 3);
        assert_eq!(months[0].days[0].label, "2024-03-15");
        assert_eq!(months.last().unwrap().bucket.label, "(no date)");
        let _ = std::fs::remove_dir_all(&dir);
    }
}

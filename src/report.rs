//! The comparison result and the derived views (date buckets, field stats).

use std::collections::{BTreeMap, HashMap};
use std::path::PathBuf;
use std::time::Duration;

use chrono::{DateTime, Datelike, Local};
use serde_json::Value;

use crate::diff::{Diff, DiffKind, DiffOptions, generic_path};
use crate::extjson;
use crate::loader::{FileFormat, ReadError};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Status {
    Different,
    OnlyInSource,
    OnlyInDest,
    Matched,
}

impl Status {
    pub fn label(self) -> &'static str {
        match self {
            Status::Matched => "Identical",
            Status::Different => "Different",
            Status::OnlyInSource => "Only in source",
            Status::OnlyInDest => "Only in destination",
        }
    }
}

#[derive(Debug)]
pub struct Record {
    pub id: Box<str>,
    pub status: Status,
    pub src_created: Option<i64>,
    pub src_updated: Option<i64>,
    pub dst_created: Option<i64>,
    pub dst_updated: Option<i64>,
    /// Creation time embedded in the ObjectId, when `_id` is an ObjectId.
    pub id_time: Option<i64>,
    pub diffs: Vec<Diff>,
    pub diffs_truncated: bool,
    /// The original JSON text is kept for objects that do not match, so the
    /// details screen can show every field. Text is much smaller in memory
    /// than parsed documents.
    pub src_raw: Option<Box<str>>,
    pub dst_raw: Option<Box<str>>,
}

impl Record {
    pub fn in_source(&self) -> bool {
        self.status != Status::OnlyInDest
    }
    pub fn in_dest(&self) -> bool {
        self.status != Status::OnlyInSource
    }
    pub fn src_doc(&self) -> Option<Value> {
        self.src_raw
            .as_deref()
            .and_then(|t| serde_json::from_str(t).ok())
    }
    pub fn dst_doc(&self) -> Option<Value> {
        self.dst_raw
            .as_deref()
            .and_then(|t| serde_json::from_str(t).ok())
    }
    pub fn created(&self) -> Option<i64> {
        self.src_created.or(self.dst_created)
    }
}

#[derive(Debug, Default)]
pub struct FileStats {
    pub path: PathBuf,
    pub size: u64,
    pub format: Option<FileFormat>,
    pub documents: u64,
    pub errors: Vec<ReadError>,
    pub error_count: u64,
    pub stopped_early: bool,
    pub without_id: u64,
    pub duplicate_ids: Vec<String>,
    pub duplicate_count: u64,
    pub missing_created: u64,
    pub missing_updated: u64,
    pub first_created: Option<i64>,
    pub last_created: Option<i64>,
}

impl FileStats {
    pub fn name(&self) -> String {
        self.path
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_else(|| self.path.display().to_string())
    }
    pub fn has_warnings(&self) -> bool {
        self.error_count > 0
            || self.duplicate_count > 0
            || self.without_id > 0
            || self.stopped_early
    }
}

/// Which field holds a date, and whether it was found automatically.
#[derive(Debug, Clone, Default)]
pub struct DateField {
    pub name: Option<String>,
    pub auto: bool,
}

impl DateField {
    pub fn describe(&self) -> String {
        match (&self.name, self.auto) {
            (Some(n), true) => format!("{n} (detected automatically)"),
            (Some(n), false) => n.clone(),
            (None, _) => "not found".into(),
        }
    }
}

#[derive(Debug, Default, Clone, Copy)]
pub struct Counts {
    pub matched: u64,
    pub different: u64,
    pub only_source: u64,
    pub only_dest: u64,
}

impl Counts {
    pub fn add(&mut self, s: Status) {
        match s {
            Status::Matched => self.matched += 1,
            Status::Different => self.different += 1,
            Status::OnlyInSource => self.only_source += 1,
            Status::OnlyInDest => self.only_dest += 1,
        }
    }
    pub fn get(&self, s: Status) -> u64 {
        match s {
            Status::Matched => self.matched,
            Status::Different => self.different,
            Status::OnlyInSource => self.only_source,
            Status::OnlyInDest => self.only_dest,
        }
    }
    pub fn mismatched(&self) -> u64 {
        self.different + self.only_source + self.only_dest
    }
    pub fn total(&self) -> u64 {
        self.matched + self.mismatched()
    }
}

#[derive(Debug, Clone)]
pub struct FieldStat {
    /// Field path with list positions removed, e.g. `items[].price`.
    pub path: String,
    pub changed: u64,
    pub type_changed: u64,
    pub only_source: u64,
    pub only_dest: u64,
    /// Indices of the records that have this difference.
    pub records: Vec<usize>,
}

impl FieldStat {
    pub fn objects(&self) -> usize {
        self.records.len()
    }
}

pub struct Report {
    pub source: FileStats,
    pub dest: FileStats,
    pub records: Vec<Record>,
    pub counts: Counts,
    pub created_field: DateField,
    pub updated_field: DateField,
    pub options: DiffOptions,
    pub duration: Duration,
    pub finished_at: DateTime<Local>,
    pub fields: Vec<FieldStat>,
}

impl Report {
    pub fn files_match(&self) -> bool {
        self.counts.mismatched() == 0
    }

    pub fn has_warnings(&self) -> bool {
        self.source.has_warnings() || self.dest.has_warnings()
    }

    /// Indices of all records that are not identical, in file order.
    pub fn mismatched(&self) -> Vec<usize> {
        (0..self.records.len())
            .filter(|&i| self.records[i].status != Status::Matched)
            .collect()
    }

    pub fn compute_field_stats(records: &[Record]) -> Vec<FieldStat> {
        let mut map: HashMap<String, FieldStat> = HashMap::new();
        for (i, r) in records.iter().enumerate() {
            for d in &r.diffs {
                let key = generic_path(&d.path);
                let entry = map.entry(key.clone()).or_insert_with(|| FieldStat {
                    path: key,
                    changed: 0,
                    type_changed: 0,
                    only_source: 0,
                    only_dest: 0,
                    records: Vec::new(),
                });
                match d.kind {
                    DiffKind::Changed => entry.changed += 1,
                    DiffKind::TypeChanged => entry.type_changed += 1,
                    DiffKind::OnlyInSource => entry.only_source += 1,
                    DiffKind::OnlyInDest => entry.only_dest += 1,
                }
                if entry.records.last() != Some(&i) {
                    entry.records.push(i);
                }
            }
        }
        let mut list: Vec<FieldStat> = map.into_values().collect();
        list.sort_by(|a, b| {
            b.objects()
                .cmp(&a.objects())
                .then_with(|| a.path.cmp(&b.path))
        });
        list
    }
}

// ---------------------------------------------------------------------------
// Date buckets

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DateBasis {
    Created,
    Updated,
    IdTime,
}

impl DateBasis {
    pub fn next(self) -> Self {
        match self {
            DateBasis::Created => DateBasis::Updated,
            DateBasis::Updated => DateBasis::IdTime,
            DateBasis::IdTime => DateBasis::Created,
        }
    }
    pub fn label(self) -> &'static str {
        match self {
            DateBasis::Created => "Created date",
            DateBasis::Updated => "Updated date",
            DateBasis::IdTime => "ObjectId time",
        }
    }
}

#[derive(Debug, Clone, Default)]
pub struct Bucket {
    pub label: String,
    /// Documents in the source file whose date falls in this bucket.
    pub source: u64,
    /// Documents in the destination file whose date falls in this bucket.
    pub dest: u64,
    pub counts: Counts,
    /// Records shown when drilling into this bucket (day buckets only).
    pub records: Vec<usize>,
}

impl Bucket {
    fn absorb(&mut self, other: &Bucket) {
        self.source += other.source;
        self.dest += other.dest;
        self.counts.matched += other.counts.matched;
        self.counts.different += other.counts.different;
        self.counts.only_source += other.counts.only_source;
        self.counts.only_dest += other.counts.only_dest;
    }
}

#[derive(Debug, Clone)]
pub struct MonthBucket {
    pub bucket: Bucket,
    pub days: Vec<Bucket>,
}

const NO_DATE: i64 = i64::MAX;

fn day_key(ms: Option<i64>) -> i64 {
    match ms.and_then(extjson::to_utc) {
        Some(d) => d.year() as i64 * 10_000 + d.month() as i64 * 100 + d.day() as i64,
        None => NO_DATE,
    }
}

fn day_label(key: i64) -> String {
    if key == NO_DATE {
        return "(no date)".into();
    }
    format!(
        "{:04}-{:02}-{:02}",
        key / 10_000,
        key / 100 % 100,
        key % 100
    )
}

fn month_label(key: i64) -> String {
    if key == NO_DATE {
        return "(no date)".into();
    }
    format!("{:04}-{:02}", key / 100, key % 100)
}

/// Groups records by year-month and then by day (all in UTC).
///
/// Each file is counted with its own date, so a record whose updated date
/// differs between the files can land in different buckets. Match status is
/// counted once, using the source date (or the destination date when the
/// record exists only in the destination).
pub fn build_date_index(records: &[Record], basis: DateBasis) -> Vec<MonthBucket> {
    let mut days: BTreeMap<i64, Bucket> = BTreeMap::new();
    for (i, r) in records.iter().enumerate() {
        let (sd, dd) = match basis {
            DateBasis::Created => (r.src_created, r.dst_created),
            DateBasis::Updated => (r.src_updated, r.dst_updated),
            DateBasis::IdTime => (r.id_time, r.id_time),
        };
        let sk = day_key(sd);
        let dk = day_key(dd);
        if r.in_source() {
            let b = days.entry(sk).or_default();
            b.source += 1;
            b.records.push(i);
        }
        if r.in_dest() {
            let b = days.entry(dk).or_default();
            b.dest += 1;
            if !(r.in_source() && sk == dk) {
                b.records.push(i);
            }
        }
        let primary = if r.in_source() { sk } else { dk };
        days.entry(primary).or_default().counts.add(r.status);
    }

    let mut months: Vec<(i64, MonthBucket)> = Vec::new();
    for (key, mut bucket) in days {
        bucket.label = day_label(key);
        let mkey = if key == NO_DATE { NO_DATE } else { key / 100 };
        if months.last().map(|(k, _)| *k) != Some(mkey) {
            months.push((
                mkey,
                MonthBucket {
                    bucket: Bucket {
                        label: month_label(mkey),
                        ..Default::default()
                    },
                    days: Vec::new(),
                },
            ));
        }
        let month = &mut months.last_mut().unwrap().1;
        month.bucket.absorb(&bucket);
        month.days.push(bucket);
    }
    months.into_iter().map(|(_, m)| m).collect()
}

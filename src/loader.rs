//! Streaming reader for MongoDB export files.
//!
//! Supports the three shapes `mongoexport` can produce:
//! * one document per line (the default),
//! * a JSON array (`--jsonArray`),
//! * pretty printed documents one after another (`--pretty`).

use std::fs::File;
use std::io::{self, BufRead, BufReader, Read, Seek, SeekFrom};
use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};

use serde::de::{Deserializer as _, SeqAccess, Visitor};
use serde_json::Value;
use serde_json::value::RawValue;

/// How many individual read errors are kept for display (all are counted).
pub const MAX_KEPT_ERRORS: usize = 1000;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FileFormat {
    Empty,
    JsonLines,
    JsonArray,
    Concatenated,
}

impl FileFormat {
    pub fn describe(self) -> &'static str {
        match self {
            FileFormat::Empty => "empty file",
            FileFormat::JsonLines => "one document per line",
            FileFormat::JsonArray => "JSON array",
            FileFormat::Concatenated => "pretty printed documents",
        }
    }
}

#[derive(Debug, Clone)]
pub struct ReadError {
    /// Line number (1-based) or document number when lines are unknown.
    pub location: String,
    pub message: String,
}

#[derive(Debug)]
pub struct ReadSummary {
    pub format: FileFormat,
    pub errors: Vec<ReadError>,
    pub error_count: u64,
    /// True when reading stopped before the end of the file.
    pub stopped_early: bool,
}

/// Counts bytes as they are read so the UI can show progress.
struct CountingReader<R> {
    inner: R,
    counter: Arc<AtomicU64>,
}

impl<R: Read> Read for CountingReader<R> {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        let n = self.inner.read(buf)?;
        self.counter.fetch_add(n as u64, Ordering::Relaxed);
        Ok(n)
    }
}

pub struct Cancelled;

/// Opens a file positioned after an optional UTF-8 byte order mark.
fn open_skipping_bom(path: &Path) -> io::Result<(File, u64)> {
    let mut f = File::open(path)?;
    let mut head = [0u8; 3];
    let n = f.read(&mut head)?;
    if n >= 2 && head[0] == 0x1f && head[1] == 0x8b {
        return Err(io::Error::other(
            "this file is compressed (gzip). Please unzip it first and select the .json file",
        ));
    }
    if n >= 2 && (head[..2] == [0xff, 0xfe] || head[..2] == [0xfe, 0xff]) {
        return Err(io::Error::other(
            "this file is saved as UTF-16 text. Please re-export it as UTF-8",
        ));
    }
    let start = if n == 3 && head == [0xef, 0xbb, 0xbf] {
        3
    } else {
        0
    };
    f.seek(SeekFrom::Start(start))?;
    Ok((f, start))
}

fn detect_format(path: &Path) -> io::Result<FileFormat> {
    let (f, _) = open_skipping_bom(path)?;
    let mut reader = BufReader::new(f);
    // Find the first non-whitespace byte.
    let first = loop {
        let buf = reader.fill_buf()?;
        if buf.is_empty() {
            return Ok(FileFormat::Empty);
        }
        if let Some(pos) = buf.iter().position(|b| !b.is_ascii_whitespace()) {
            let b = buf[pos];
            reader.consume(pos);
            break b;
        }
        let len = buf.len();
        reader.consume(len);
    };
    if first == b'[' {
        return Ok(FileFormat::JsonArray);
    }
    let mut line = Vec::new();
    reader.read_until(b'\n', &mut line)?;
    Ok(match serde_json::from_slice::<Value>(trim_ascii(&line)) {
        Ok(_) => FileFormat::JsonLines,
        Err(_) => FileFormat::Concatenated,
    })
}

fn trim_ascii(b: &[u8]) -> &[u8] {
    let start = b
        .iter()
        .position(|c| !c.is_ascii_whitespace())
        .unwrap_or(b.len());
    let end = b
        .iter()
        .rposition(|c| !c.is_ascii_whitespace())
        .map_or(start, |p| p + 1);
    &b[start..end]
}

/// Where a document was found, for error messages.
#[derive(Debug, Clone, Copy)]
pub enum Location {
    Line(u64),
    Document(u64),
}

impl std::fmt::Display for Location {
    fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        match self {
            Location::Line(n) => write!(f, "line {n}"),
            Location::Document(n) => write!(f, "document {n}"),
        }
    }
}

/// Records a problem, keeping at most [`MAX_KEPT_ERRORS`] messages.
pub fn push_error(errors: &mut Vec<ReadError>, count: &mut u64, location: String, message: String) {
    *count += 1;
    if errors.len() < MAX_KEPT_ERRORS {
        errors.push(ReadError { location, message });
    }
}

/// Reads every document of `path` and calls `on_doc(json_text, location)`
/// with the raw text of each one. Parsing the text is left to the caller so
/// it can be done in parallel and only kept as text while waiting.
/// `on_doc` returns `Err(Cancelled)` to stop early.
pub fn read_documents(
    path: &Path,
    bytes_read: Arc<AtomicU64>,
    cancel: &AtomicBool,
    mut on_doc: impl FnMut(Box<str>, Location) -> Result<(), Cancelled>,
) -> io::Result<ReadSummary> {
    let format = detect_format(path)?;
    let (file, skipped) = open_skipping_bom(path)?;
    bytes_read.store(skipped, Ordering::Relaxed);
    let reader = BufReader::with_capacity(
        1 << 20,
        CountingReader {
            inner: file,
            counter: bytes_read,
        },
    );
    let mut summary = ReadSummary {
        format,
        errors: Vec::new(),
        error_count: 0,
        stopped_early: false,
    };
    let add_error = |summary: &mut ReadSummary, location: String, message: String| {
        push_error(
            &mut summary.errors,
            &mut summary.error_count,
            location,
            message,
        );
    };

    match format {
        FileFormat::Empty => {}
        FileFormat::JsonLines => {
            let mut reader = reader;
            let mut line = Vec::new();
            let mut line_no = 0u64;
            loop {
                line.clear();
                if reader.read_until(b'\n', &mut line)? == 0 {
                    break;
                }
                line_no += 1;
                let text = trim_ascii(&line);
                if text.is_empty() {
                    continue;
                }
                if line_no.is_multiple_of(1024) && cancel.load(Ordering::Relaxed) {
                    summary.stopped_early = true;
                    break;
                }
                match std::str::from_utf8(text) {
                    Ok(t) => {
                        if on_doc(t.into(), Location::Line(line_no)).is_err() {
                            summary.stopped_early = true;
                            break;
                        }
                    }
                    Err(_) => add_error(
                        &mut summary,
                        format!("line {line_no}"),
                        "the text is not valid UTF-8".into(),
                    ),
                }
            }
        }
        FileFormat::Concatenated => {
            let stream = serde_json::Deserializer::from_reader(reader).into_iter::<Box<RawValue>>();
            for (i, item) in stream.enumerate() {
                if cancel.load(Ordering::Relaxed) {
                    summary.stopped_early = true;
                    break;
                }
                match item {
                    Ok(raw) => {
                        if on_doc(raw.into(), Location::Document(i as u64 + 1)).is_err() {
                            summary.stopped_early = true;
                            break;
                        }
                    }
                    Err(e) => {
                        // The stream cannot resynchronise after a syntax error.
                        add_error(
                            &mut summary,
                            format!("line {}", e.line()),
                            friendly_json_error(&e, true),
                        );
                        summary.stopped_early = true;
                        break;
                    }
                }
            }
        }
        FileFormat::JsonArray => {
            let mut de = serde_json::Deserializer::from_reader(reader);
            let mut visitor = ArrayVisitor {
                on_doc: &mut on_doc,
                cancel,
                count: 0,
                cancelled: false,
            };
            let result = (&mut de).deserialize_seq(&mut visitor);
            match result {
                Ok(()) => {}
                Err(_) if visitor.cancelled => summary.stopped_early = true,
                Err(e) => {
                    add_error(
                        &mut summary,
                        format!("line {}", e.line()),
                        friendly_json_error(&e, true),
                    );
                    summary.stopped_early = true;
                }
            }
        }
    }
    Ok(summary)
}

struct ArrayVisitor<'a, F> {
    on_doc: &'a mut F,
    cancel: &'a AtomicBool,
    count: u64,
    cancelled: bool,
}

impl<'de, F> Visitor<'de> for &mut ArrayVisitor<'_, F>
where
    F: FnMut(Box<str>, Location) -> Result<(), Cancelled>,
{
    type Value = ();

    fn expecting(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        f.write_str("an array of documents")
    }

    fn visit_seq<A: SeqAccess<'de>>(self, mut seq: A) -> Result<(), A::Error> {
        while let Some(raw) = seq.next_element::<Box<RawValue>>()? {
            self.count += 1;
            if self.cancel.load(Ordering::Relaxed)
                || (self.on_doc)(raw.into(), Location::Document(self.count)).is_err()
            {
                self.cancelled = true;
                return Err(serde::de::Error::custom("cancelled"));
            }
        }
        Ok(())
    }
}

pub fn friendly_json_error(e: &serde_json::Error, with_position: bool) -> String {
    use serde_json::error::Category;
    let what = match e.classify() {
        Category::Eof => "the text ends unexpectedly (file may be cut off)".to_string(),
        Category::Syntax => format!("invalid JSON ({e})"),
        Category::Data => format!("unexpected data ({e})"),
        Category::Io => format!("could not read file ({e})"),
    };
    if with_position {
        format!("{what}; reading stopped here")
    } else {
        what
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    fn read_all(content: &str) -> (Vec<Value>, ReadSummary) {
        let dir = std::env::temp_dir().join(format!("mc-loader-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join(format!("t{}.json", content.len()));
        std::fs::File::create(&path)
            .unwrap()
            .write_all(content.as_bytes())
            .unwrap();
        let mut docs = Vec::new();
        let summary = read_documents(
            &path,
            Arc::new(AtomicU64::new(0)),
            &AtomicBool::new(false),
            |raw, _| {
                if let Ok(v) = serde_json::from_str::<Value>(&raw) {
                    docs.push(v);
                }
                Ok(())
            },
        )
        .unwrap();
        let _ = std::fs::remove_file(&path);
        (docs, summary)
    }

    #[test]
    fn reads_lines_and_skips_bad_ones() {
        let (docs, s) = read_all("{\"a\":1}\n\n{bad\n{\"a\":2}\n");
        assert_eq!(s.format, FileFormat::JsonLines);
        // Invalid lines are handed over too; the caller reports them.
        assert_eq!(docs.len(), 2);
        assert_eq!(s.error_count, 0);
    }

    #[test]
    fn reads_arrays_and_pretty_output() {
        let (docs, s) = read_all("\u{feff}[\n {\"a\":1},\n {\"a\":2}\n]");
        assert_eq!(s.format, FileFormat::JsonArray);
        assert_eq!(docs.len(), 2);
        let (docs, s) = read_all("{\n \"a\": 1\n}\n{\n \"a\": 2\n}\n");
        assert_eq!(s.format, FileFormat::Concatenated);
        assert_eq!(docs.len(), 2);
    }
}

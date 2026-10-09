//! The worker progress file (spec 57 §5.5): `<data_dir>/sessions/<id>.progress.jsonl`.
//!
//! One JSON object per line — `seq`, `timestampMs`, `kind` and `payload` — so
//! the Workers board can show a per-session list without joining audit events
//! on `taskId`. The audit log stays the authoritative record of tool calls.

use crate::error::Result;
use crate::hash::now_millis;
use crate::secret_scanner::SecretScanner;
use serde_json::{Map, Value, json};
use std::fs::{self, File};
use std::io::Write;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProgressKind {
    Status,
    Phase,
    Plan,
    Text,
    Tool,
    Approval,
    Note,
}

impl ProgressKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Status => "status",
            Self::Phase => "phase",
            Self::Plan => "plan",
            Self::Text => "text",
            Self::Tool => "tool",
            Self::Approval => "approval",
            Self::Note => "note",
        }
    }
}

/// Appends progress lines for one worker session. Only the worker process
/// writes the file, so `seq` is kept in memory rather than read back.
pub struct ProgressWriter {
    file: File,
    last_seq: u64,
    scanner: SecretScanner,
}

impl ProgressWriter {
    /// Truncates any previous file: a restarted worker starts again from `seq` 1.
    pub fn create(data_dir: &Path, session_id: &str, scanner: SecretScanner) -> Result<Self> {
        let path = progress_path(data_dir, session_id);
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }
        Ok(Self {
            file: File::create(path)?,
            last_seq: 0,
            scanner,
        })
    }

    /// Redacts every string in `payload`, then writes one full line. Returns
    /// the line's `seq`.
    pub fn append(&mut self, kind: ProgressKind, payload: Value) -> Result<u64> {
        // The session log is unredacted on write (spec 57 C3), so this file
        // cannot rely on it: whatever the caller passes is redacted here.
        let payload = redact_value(&self.scanner, payload);
        let seq = self.last_seq + 1;
        let line = json!({
            "seq": seq,
            "timestampMs": now_millis() as u64,
            "kind": kind.as_str(),
            "payload": payload,
        });
        let mut text = line.to_string();
        text.push('\n');
        // One write per line, so a concurrent reader sees at most one torn
        // line, and only at the end.
        self.file.write_all(text.as_bytes())?;
        self.last_seq = seq;
        Ok(seq)
    }
}

fn redact_value(scanner: &SecretScanner, value: Value) -> Value {
    match value {
        Value::String(text) => Value::String(scanner.redact(&text).text),
        Value::Array(items) => Value::Array(
            items
                .into_iter()
                .map(|item| redact_value(scanner, item))
                .collect(),
        ),
        Value::Object(fields) => Value::Object(
            fields
                .into_iter()
                .map(|(key, field)| (key, redact_value(scanner, field)))
                .collect::<Map<_, _>>(),
        ),
        other => other,
    }
}

/// Every line with `seq > after`, in file order. A missing file reads as empty.
///
/// A line that does not parse is skipped. The reader runs while the worker
/// appends, so the last line can be a write in progress; the next poll reads
/// it whole. Each line is one object ending in `}`, so a tear always leaves
/// invalid JSON rather than a shorter valid object.
pub fn read_progress_after(data_dir: &Path, session_id: &str, after: u64) -> Result<Vec<Value>> {
    let bytes = match fs::read(progress_path(data_dir, session_id)) {
        Ok(bytes) => bytes,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(error) => return Err(error.into()),
    };
    Ok(bytes
        .split(|byte| *byte == b'\n')
        .filter_map(|line| serde_json::from_slice::<Value>(line).ok())
        .filter(|event| event["seq"].as_u64().is_some_and(|seq| seq > after))
        .collect())
}

/// `<data_dir>/sessions/<id>.progress.jsonl`, beside the session log.
pub fn progress_path(data_dir: &Path, session_id: &str) -> PathBuf {
    data_dir
        .join("sessions")
        .join(format!("{session_id}.progress.jsonl"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use std::fs::{self, OpenOptions};
    use std::io::Write;
    use std::sync::atomic::{AtomicU64, Ordering};
    use std::time::{SystemTime, UNIX_EPOCH};

    static COUNTER: AtomicU64 = AtomicU64::new(1);

    // A token shape `SecretScanner` detects (its generic `ghp_` token rule).
    const SECRET: &str = "ghp_abcdefghijklmnopqrstuvwxyz0123456789";

    fn temp_data_dir(name: &str) -> PathBuf {
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("clock should work")
            .as_nanos();
        std::env::temp_dir().join(format!(
            "damaian-worker-progress-{name}-{now}-{}",
            COUNTER.fetch_add(1, Ordering::Relaxed)
        ))
    }

    fn writer(data_dir: &Path) -> ProgressWriter {
        ProgressWriter::create(data_dir, "ses_1", SecretScanner::default()).unwrap()
    }

    fn seqs(events: &[Value]) -> Vec<u64> {
        events
            .iter()
            .map(|event| event["seq"].as_u64().unwrap())
            .collect()
    }

    #[test]
    fn seq_strictly_increases_from_one() {
        let data_dir = temp_data_dir("seq");
        let mut progress = writer(&data_dir);
        let assigned: Vec<u64> = (0..4)
            .map(|n| {
                progress
                    .append(ProgressKind::Text, json!({"text": n}))
                    .unwrap()
            })
            .collect();
        assert_eq!(assigned, vec![1, 2, 3, 4]);

        let events = read_progress_after(&data_dir, "ses_1", 0).unwrap();
        assert_eq!(seqs(&events), vec![1, 2, 3, 4]);
        for event in &events {
            assert_eq!(event["kind"], "text");
            assert!(event["timestampMs"].as_u64().unwrap() > 0);
        }
        assert_eq!(events[2]["payload"], json!({"text": 2}));
        let _ = fs::remove_dir_all(data_dir);
    }

    #[test]
    fn creating_a_writer_truncates_the_previous_file() {
        let data_dir = temp_data_dir("truncate");
        let mut first = writer(&data_dir);
        for _ in 0..3 {
            first
                .append(ProgressKind::Status, json!({"status": "running"}))
                .unwrap();
        }
        drop(first);

        // A restarted worker starts again from 1, and the old lines are gone.
        let mut restarted = writer(&data_dir);
        assert!(
            read_progress_after(&data_dir, "ses_1", 0)
                .unwrap()
                .is_empty()
        );
        assert_eq!(restarted.append(ProgressKind::Note, json!({})).unwrap(), 1);
        let events = read_progress_after(&data_dir, "ses_1", 0).unwrap();
        assert_eq!(seqs(&events), vec![1]);
        assert_eq!(events[0]["kind"], "note");
        let _ = fs::remove_dir_all(data_dir);
    }

    #[test]
    fn reading_after_n_returns_exactly_the_later_lines() {
        let data_dir = temp_data_dir("after");
        let mut progress = writer(&data_dir);
        for n in 0..5 {
            progress
                .append(ProgressKind::Phase, json!({"round": n}))
                .unwrap();
        }
        let read = |after| seqs(&read_progress_after(&data_dir, "ses_1", after).unwrap());
        assert_eq!(read(0), vec![1, 2, 3, 4, 5]);
        assert_eq!(read(2), vec![3, 4, 5]);
        assert_eq!(read(5), Vec::<u64>::new());
        assert_eq!(read(9), Vec::<u64>::new());
        let _ = fs::remove_dir_all(data_dir);
    }

    #[test]
    fn a_session_without_a_progress_file_reads_as_empty() {
        let data_dir = temp_data_dir("missing");
        assert!(
            read_progress_after(&data_dir, "ses_none", 0)
                .unwrap()
                .is_empty()
        );
    }

    #[test]
    fn a_seeded_secret_never_reaches_the_file() {
        let data_dir = temp_data_dir("secret");
        let mut progress = writer(&data_dir);
        progress
            .append(
                ProgressKind::Text,
                json!({"text": format!("the token is {SECRET}")}),
            )
            .unwrap();
        progress
            .append(
                ProgressKind::Tool,
                json!({"label": format!("curl -H 'x: {SECRET}'")}),
            )
            .unwrap();
        progress
            .append(
                ProgressKind::Approval,
                json!({"proposal": {"id": "p1", "commands": [format!("echo {SECRET}")]}}),
            )
            .unwrap();

        let raw = fs::read(progress_path(&data_dir, "ses_1")).unwrap();
        let raw = String::from_utf8(raw).unwrap();
        assert!(!raw.contains(SECRET), "secret reached the file: {raw}");
        assert_eq!(raw.matches("[REDACTED_").count(), 3, "{raw}");
        let _ = fs::remove_dir_all(data_dir);
    }

    #[test]
    fn a_torn_last_line_is_skipped_not_fatal() {
        let data_dir = temp_data_dir("torn");
        let mut progress = writer(&data_dir);
        progress
            .append(ProgressKind::Text, json!({"text": "a"}))
            .unwrap();
        progress
            .append(ProgressKind::Text, json!({"text": "b"}))
            .unwrap();

        // The reader runs while the writer appends, so it can see half a line.
        let mut file = OpenOptions::new()
            .append(true)
            .open(progress_path(&data_dir, "ses_1"))
            .unwrap();
        file.write_all(br#"{"seq":3,"timestampMs":1,"kind":"te"#)
            .unwrap();

        let events = read_progress_after(&data_dir, "ses_1", 0).unwrap();
        assert_eq!(seqs(&events), vec![1, 2]);
        let _ = fs::remove_dir_all(data_dir);
    }

    #[test]
    fn the_progress_file_sits_beside_the_session_log() {
        assert_eq!(
            progress_path(Path::new("/data"), "ses_1"),
            PathBuf::from("/data/sessions/ses_1.progress.jsonl")
        );
    }
}

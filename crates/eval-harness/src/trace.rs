use std::path::Path;

use workspace_engine::{ClientError, Result};

/// One audit event. `AuditLog::record` writes a flat JSON object per line with
/// `eventId`, `timestampMs`, `userId`, `eventType` plus caller fields, every
/// value already redacted (`crates/workspace-engine/src/audit.rs:42`).
#[derive(Debug, Clone)]
pub struct Event {
    pub event_type: String,
    pub fields: serde_json::Map<String, serde_json::Value>,
}

impl Event {
    pub fn field(&self, key: &str) -> Option<&str> {
        self.fields.get(key).and_then(|value| value.as_str())
    }
}

/// The engine's own record of what a run did. Read rather than inferred from the
/// scenario script, so an assertion tests the engine instead of the script.
///
/// Field names differ per event and are not interchangeable — `file_modified`
/// carries a single `resourcePath`, while `patch_applied` and `patch_proposed`
/// carry a comma-joined `files` list. Note also that `stored_command_rejected`
/// and `command_allowlisted` carry a `resourcePath` naming the *config file*
/// they wrote, not a repository file, so a broad sweep for `resourcePath` would
/// wrongly report config writes as changed files. Read specific fields from
/// specific events.
#[derive(Debug, Clone, Default)]
pub struct Trace {
    pub events: Vec<Event>,
}

impl Trace {
    pub fn read(data_dir: &Path) -> Result<Trace> {
        let path = data_dir.join("audit").join("events.jsonl");
        if !path.exists() {
            // A run that produced no auditable action is a legitimate outcome —
            // the refusal scenarios are exactly that — so this is not an error.
            return Ok(Trace::default());
        }
        let text = std::fs::read_to_string(&path)
            .map_err(|error| ClientError::Io(format!("{}: {error}", path.display())))?;

        let mut events = Vec::new();
        for line in text.lines() {
            let line = line.trim();
            if line.is_empty() {
                continue;
            }
            // A single malformed line must not discard the rest of the trace:
            // the log is append-only and a crash mid-write is plausible.
            let Ok(serde_json::Value::Object(fields)) = serde_json::from_str(line) else {
                continue;
            };
            let Some(event_type) = fields.get("eventType").and_then(|value| value.as_str()) else {
                continue;
            };
            events.push(Event {
                event_type: event_type.to_string(),
                fields,
            });
        }
        Ok(Trace { events })
    }

    pub fn of_type(&self, event_type: &str) -> Vec<&Event> {
        self.events
            .iter()
            .filter(|event| event.event_type == event_type)
            .collect()
    }

    pub fn count(&self, event_type: &str) -> u64 {
        self.of_type(event_type).len() as u64
    }

    /// Distinct, order-preserving field values across every event of a type,
    /// for events whose field holds exactly one path (`file_modified`'s
    /// `resourcePath`). Used for `filesChanged`, where the same path can be
    /// touched twice.
    pub fn paths_from(&self, event_type: &str, field: &str) -> Vec<String> {
        let mut seen = Vec::new();
        for event in self.of_type(event_type) {
            if let Some(value) = event.field(field)
                && !seen.iter().any(|existing| existing == value)
            {
                seen.push(value.to_string());
            }
        }
        seen
    }

    /// Like [`Self::paths_from`], but for a field holding a comma-joined list —
    /// which is how `patch_applied` and `patch_proposed` report their `files`.
    /// Empty entries are dropped, so a trailing comma cannot produce a blank
    /// path.
    pub fn csv_from(&self, event_type: &str, field: &str) -> Vec<String> {
        let mut seen = Vec::new();
        for event in self.of_type(event_type) {
            let Some(value) = event.field(field) else {
                continue;
            };
            for part in value.split(',') {
                let part = part.trim();
                if !part.is_empty() && !seen.iter().any(|existing| existing == part) {
                    seen.push(part.to_string());
                }
            }
        }
        seen
    }
}

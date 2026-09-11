use std::path::Path;

use workspace_engine::{ClientError, Result};

use crate::record::RecordedToolCall;

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

/// The tool calls a run actually dispatched, read from the engine's session
/// log rather than from the scenario script.
///
/// The script is the wrong source for two reasons. The live tier ignores the
/// `[[turn]]` blocks entirely, so a live record built from them lists calls
/// that never happened; and even in the deterministic tier the script cannot
/// say how a call *ended*, which left every entry stamped with whether the
/// turn as a whole succeeded.
///
/// Each tool dispatch is bracketed by `action_started` / `action_finished`
/// markers carrying the tool name and the engine's own outcome
/// (`crates/workspace-engine/src/chat.rs`, `tool_action_marker`). An action
/// that started and never finished is reported `unknown` rather than dropped —
/// that is the crash signature spec 17 exists to preserve, and silently
/// omitting it would hide a call that may well have been billed.
///
/// `model_call` markers are excluded: they are not tool calls, and
/// `model_calls` counts them separately.
pub fn tool_actions(data_dir: &Path) -> Result<Vec<RecordedToolCall>> {
    let sessions = data_dir.join("sessions");
    let Ok(entries) = std::fs::read_dir(&sessions) else {
        // No session log means no turn ran — a refusal scenario is exactly
        // that, so this is not an error.
        return Ok(Vec::new());
    };

    let mut paths = Vec::new();
    for entry in entries {
        let path = entry
            .map_err(|error| ClientError::Io(format!("{}: {error}", sessions.display())))?
            .path();
        if path
            .extension()
            .is_some_and(|extension| extension == "jsonl")
        {
            paths.push(path);
        }
    }
    // Sorted so a run with more than one session log produces a stable record
    // rather than one that depends on directory order.
    paths.sort();

    let mut started = Vec::new();
    let mut outcomes: Vec<(String, String)> = Vec::new();
    for path in &paths {
        let text = std::fs::read_to_string(path)
            .map_err(|error| ClientError::Io(format!("{}: {error}", path.display())))?;
        for line in text.lines() {
            // A torn final line is plausible in an append-only log a crash
            // scenario deliberately interrupts; skip it rather than failing.
            let Ok(serde_json::Value::Object(event)) = serde_json::from_str(line.trim()) else {
                continue;
            };
            let event_type = event.get("eventType").and_then(|value| value.as_str());
            let Some(payload) = event.get("payload").and_then(|value| value.as_object()) else {
                continue;
            };
            let text_field = |key: &str| {
                payload
                    .get(key)
                    .and_then(|value| value.as_str())
                    .unwrap_or_default()
                    .to_string()
            };
            match event_type {
                Some("action_started") => {
                    let action = text_field("action");
                    if action == "model_call" {
                        continue;
                    }
                    started.push((text_field("markerId"), action, text_field("ref")));
                }
                Some("action_finished") => {
                    outcomes.push((text_field("markerId"), text_field("outcome")));
                }
                _ => {}
            }
        }
    }

    Ok(started
        .into_iter()
        .map(|(marker_id, name, reference)| RecordedToolCall {
            name,
            // The marker's reference is what the engine recorded of the call:
            // the command, the path, the query, the patch summary. It is not
            // the full argument object the script carries, and deliberately
            // so — this is what the run can testify to.
            arguments: serde_json::json!({ "ref": reference }),
            outcome: outcomes
                .iter()
                .find(|(id, _)| *id == marker_id)
                .map(|(_, outcome)| outcome.clone())
                .unwrap_or_else(|| "unknown".to_string()),
        })
        .collect())
}

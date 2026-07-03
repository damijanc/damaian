use crate::error::Result;
use crate::hash::{create_id, now_millis};
use crate::secret_scanner::SecretScanner;
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone)]
pub struct AuditLog {
    data_dir: PathBuf,
    enabled: bool,
    scanner: SecretScanner,
    local_profile_id: String,
}

impl AuditLog {
    pub fn new(data_dir: impl AsRef<Path>, enabled: bool, scanner: SecretScanner) -> Self {
        Self {
            data_dir: data_dir.as_ref().to_path_buf(),
            enabled,
            scanner,
            local_profile_id: "local_user".to_string(),
        }
    }

    pub fn disabled(scanner: SecretScanner) -> Self {
        Self::new(".", false, scanner)
    }

    pub fn record(&self, event_type: &str, fields: &[(&str, String)]) -> Result<String> {
        let mut event = vec![
            ("eventId".to_string(), create_id("evt")),
            ("timestampMs".to_string(), now_millis().to_string()),
            ("userId".to_string(), self.local_profile_id.clone()),
            ("eventType".to_string(), event_type.to_string()),
        ];
        for (key, value) in fields {
            event.push(((*key).to_string(), self.scanner.redact(value).text));
        }

        let json = object_json(&event);
        if !self.enabled {
            return Ok(json);
        }

        let audit_dir = self.data_dir.join("audit");
        fs::create_dir_all(&audit_dir)?;
        let log_path = audit_dir.join("events.jsonl");
        let mut file = OpenOptions::new()
            .create(true)
            .append(true)
            .open(log_path)?;
        writeln!(file, "{json}")?;
        Ok(json)
    }
}

fn object_json(fields: &[(String, String)]) -> String {
    let body = fields
        .iter()
        .map(|(key, value)| format!("\"{}\":\"{}\"", escape_json(key), escape_json(value)))
        .collect::<Vec<_>>()
        .join(",");
    format!("{{{body}}}")
}

pub fn escape_json(value: &str) -> String {
    let mut escaped = String::new();
    for character in value.chars() {
        match character {
            '"' => escaped.push_str("\\\""),
            '\\' => escaped.push_str("\\\\"),
            '\n' => escaped.push_str("\\n"),
            '\r' => escaped.push_str("\\r"),
            '\t' => escaped.push_str("\\t"),
            character if character.is_control() => {
                escaped.push_str(&format!("\\u{:04x}", character as u32));
            }
            character => escaped.push(character),
        }
    }
    escaped
}

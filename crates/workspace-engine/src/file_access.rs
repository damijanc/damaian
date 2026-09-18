use crate::audit::AuditLog;
use crate::config::Config;
use crate::error::{ClientError, Result};
use crate::hash::file_hash;
use crate::path_policy::PathPolicy;
use crate::secret_scanner::SecretScanner;
use std::fs;
use std::path::Path;

/// A 1-based, inclusive line range.
///
/// Spec 47 §5.6: this is the representation spec 26 adopts for `ContextItem`
/// rather than defining a second one, because a ranged read is what produces
/// the ranges that spec deduplicates.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LineRange {
    pub start: usize,
    pub end: usize,
}

/// How much of a file a caller wants.
///
/// This is an enum rather than an `Option<LineRange>` because the two callers
/// of `read_file` want genuinely different things, and `None` cannot say which.
/// Context assembly wants the whole file and applies its own token budget to it
/// (`context_manager.rs`); the `read_file` *tool* wants a bounded window,
/// because one 4420-line file exhausts the default context budget in a single
/// call. Encoding that as "no range" would have silently truncated context
/// assembly the day the cap was added.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReadWindow {
    /// Every line, however many. For callers that bound the result themselves.
    Whole,
    /// The model asked for no range: the first `max_read_lines` lines.
    Default,
    /// The model asked for this range, clamped to `max_read_lines` so a large
    /// range cannot be used to step around the cap.
    Range(LineRange),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FileRead {
    pub path: String,
    pub absolute_path: String,
    pub hash: String,
    pub content: String,
    pub redaction_status: String,
    pub finding_count: usize,
    /// The lines actually returned, which is not always what was asked for.
    pub line_range: LineRange,
    /// The file's real length, so a truncated read can say "of 4420" instead
    /// of reading as the whole file.
    pub total_lines: usize,
    /// `"lines"` or `"bytes"` when the window was cut short, `None` when the
    /// caller got exactly what it asked for. Spec 47 §5.5: a truncated result
    /// that reads as complete is the failure this design exists to prevent.
    pub truncated_by: Option<String>,
}

#[derive(Debug, Clone)]
pub struct FileAccessController {
    config: Config,
    audit_log: AuditLog,
    scanner: SecretScanner,
    path_policy: PathPolicy,
}

impl FileAccessController {
    pub fn new(
        config: Config,
        audit_log: AuditLog,
        scanner: SecretScanner,
        path_policy: PathPolicy,
    ) -> Self {
        Self {
            config,
            audit_log,
            scanner,
            path_policy,
        }
    }

    // Each argument is an independent input to one read: where, what, who for,
    // and the two policy opt-outs. A params struct would be built at each of the
    // three call sites and destructured straight back here, which is the same
    // reasoning as the `#[allow]` on `ContextManager::add_file`.
    #[allow(clippy::too_many_arguments)]
    pub fn read_file(
        &self,
        root_path: impl AsRef<Path>,
        requested_path: impl AsRef<Path>,
        task_id: Option<&str>,
        repository_id: Option<&str>,
        allow_restricted: bool,
        allow_outside_root: bool,
        window: ReadWindow,
    ) -> Result<FileRead> {
        let target =
            self.path_policy
                .resolve_existing(root_path, requested_path, allow_outside_root)?;
        self.path_policy
            .assert_not_restricted(&target.relative_path, allow_restricted)?;
        let metadata = fs::metadata(&target.absolute_path)?;
        if !metadata.is_file() {
            return Err(ClientError::AccessDenied(
                "Path is not a regular file".to_string(),
            ));
        }

        // For a bounded window, `max_file_bytes` caps what is *returned* rather
        // than what may be inspected (spec 47 §5.2): the limit exists to keep a
        // huge file out of the model's context, not to stop the engine reading
        // one, so an oversized file is readable by range instead of refused.
        //
        // `Whole` keeps the original refusal, because its callers asked for
        // every byte and nothing downstream would bound the result.
        if matches!(window, ReadWindow::Whole) && metadata.len() > self.config.max_file_bytes {
            return Err(ClientError::AccessDenied(
                "File exceeds configured size limit".to_string(),
            ));
        }

        let bytes = fs::read(&target.absolute_path)?;
        if bytes.iter().take(8000).any(|byte| *byte == 0) {
            return Err(ClientError::AccessDenied(
                "Binary file reads are denied by default".to_string(),
            ));
        }
        let raw_content = String::from_utf8_lossy(&bytes).to_string();
        let lines = raw_content.lines().collect::<Vec<_>>();
        let total_lines = lines.len();

        // An empty or inverted range is refused naming the total, so the model
        // can correct it in the same round rather than receiving an empty
        // string it might read as "the file is empty".
        if let ReadWindow::Range(asked) = window
            && (asked.start == 0 || asked.start > asked.end || asked.start > total_lines)
        {
            return Err(ClientError::InvalidInput(format!(
                "Requested lines {}-{} of {}, which has {total_lines} lines",
                asked.start, asked.end, target.relative_path
            )));
        }

        let max_bytes = self.config.max_file_bytes as usize;
        let start = match window {
            ReadWindow::Range(asked) => asked.start,
            _ => 1,
        };
        let requested_end = match window {
            ReadWindow::Range(asked) => asked.end,
            _ => usize::MAX,
        };
        // The line cap applies to an explicit range too, or a model could step
        // around it by asking for a range larger than the cap. `Whole` opts out
        // entirely: its callers bound the result themselves.
        let capped_end = match window {
            ReadWindow::Whole => usize::MAX,
            _ => start
                .saturating_add(self.config.max_read_lines)
                .saturating_sub(1),
        };
        // Not clamped up to 1: an empty file has no lines, and `lines[0..1]`
        // would panic on one.
        let mut end = requested_end.min(capped_end).min(total_lines);
        let mut truncated_by = (end < requested_end.min(total_lines)).then(|| "lines".to_string());

        // The byte cap sits beside the line cap rather than under it: one
        // window of a minified or generated file can exceed any budget a line
        // count implies.
        let mut selected_lines = lines[start - 1..end].to_vec();
        while selected_lines.len() > 1 && window_bytes(&selected_lines) > max_bytes {
            selected_lines.pop();
            end -= 1;
            truncated_by = Some("bytes".to_string());
        }

        let mut selected = selected_lines.join("\n");
        if !selected.is_empty() {
            selected.push('\n');
        }
        if selected.len() > max_bytes {
            // One line longer than the whole budget. Cut it on a char boundary
            // rather than returning the payload the cap exists to prevent.
            let mut cut = max_bytes;
            while cut > 0 && !selected.is_char_boundary(cut) {
                cut -= 1;
            }
            selected.truncate(cut);
            truncated_by = Some("bytes".to_string());
        }
        let redaction = self.scanner.redact(&selected);
        let result = FileRead {
            path: target.relative_path.clone(),
            absolute_path: target.absolute_path.to_string_lossy().to_string(),
            hash: file_hash(&target.absolute_path)?,
            content: redaction.text,
            redaction_status: if redaction.findings.is_empty() {
                "clean".to_string()
            } else {
                "redacted".to_string()
            },
            finding_count: redaction.findings.len(),
            line_range: LineRange { start, end },
            total_lines,
            truncated_by,
        };

        self.audit_log.record(
            "file_read",
            &[
                ("actor", "assistant".to_string()),
                (
                    "repositoryId",
                    repository_id.unwrap_or_default().to_string(),
                ),
                ("taskId", task_id.unwrap_or_default().to_string()),
                ("resourcePath", result.path.clone()),
                ("status", "allowed".to_string()),
                ("redactionStatus", result.redaction_status.clone()),
                ("findingCount", result.finding_count.to_string()),
            ],
        )?;

        Ok(result)
    }
}

/// Bytes a joined window would occupy, counting the newline each line carries.
fn window_bytes(window: &[&str]) -> usize {
    window.iter().map(|line| line.len() + 1).sum()
}

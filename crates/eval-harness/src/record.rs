use serde::Serialize;
use workspace_engine::SecretScanner;

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Tokens {
    pub input: u64,
    pub output: u64,
    /// Whether these came from the provider or from an estimate. The harness
    /// never presents an estimate as measured (proposal §5.5).
    ///
    /// False throughout the deterministic tier: the mock adapter is not a
    /// provider and reports nothing, so every figure there is spec 19's
    /// `len / 4` estimate. Only a live-tier run against a provider that
    /// reports usage sets this true.
    pub measured: bool,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RecordedToolCall {
    pub name: String,
    pub arguments: serde_json::Value,
    pub outcome: String,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RecordedApproval {
    pub kind: String,
    pub decision: String,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RecordedCheck {
    pub command: String,
    pub passed: bool,
}

/// What a restart made of a task the scenario left mid-action.
///
/// §5.6's recovery row asks two things of a session killed mid-task: that it
/// *classifies*, and that it is *not auto-retried*. Both are recorded here, per
/// run, for the same reason `approval_policy_violations` is: a metric derived
/// from fields that cannot express a failure would satisfy its assertion while
/// detecting nothing.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RecordedRecovery {
    /// The action left dangling — the evidence the classifier read.
    pub interrupted_action: String,
    /// What the classifier concluded on restart.
    pub classification: String,
    /// Whether Damaian would continue this task without asking.
    pub auto_resume_permitted: bool,
    /// Whether the engine refused an explicit `resume`. With an
    /// `unknown_external_outcome` classification, `false` here is a
    /// requirement-5 violation: the guarantee is enforced in the engine, so a
    /// resume that went through means it is not enforced anywhere.
    pub resume_refused: bool,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AssertionOutcome {
    pub name: String,
    pub passed: bool,
    pub skipped: bool,
    pub expected: String,
    pub actual: String,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RunRecord {
    pub scenario: String,
    pub fixture_version: String,
    pub tier: String,
    pub provider: String,
    pub model: String,
    pub started_at_ms: u128,
    pub duration_ms: u128,
    pub tool_calls: Vec<RecordedToolCall>,
    pub approvals: Vec<RecordedApproval>,
    /// Paths only. File contents never enter a record (proposal §5.5).
    pub files_changed: Vec<String>,
    pub checks: Vec<RecordedCheck>,
    pub final_status: String,
    pub tokens: Tokens,
    pub cost: Option<f64>,
    pub assertions: Vec<AssertionOutcome>,
    /// Set when the scenario was skipped because the capability it measures does
    /// not exist yet — carries the blocking spec, e.g. `"spec-17"`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub not_applicable: Option<String>,
    pub tool_rounds: u64,
    pub model_calls: u64,
    /// Commands the engine executed despite a human declining them, counted by
    /// matching `proposalId` across `stored_command_rejected` and
    /// `stored_command_executed`. Computed by the runner rather than by
    /// `metrics.rs`, because the linkage lives in the audit trace and a
    /// `RunRecord`'s `approvals` list has already discarded the ids.
    ///
    /// §5.6 asserts this is zero. It is recorded per run so the assertion has
    /// something real to read: a metric derived from fields that cannot express
    /// a violation would satisfy the assertion while detecting nothing.
    pub approval_policy_violations: u64,
    /// Present only for a scenario that declared `crash_mid_action`. `None`
    /// means the scenario measured nothing about recovery, which is different
    /// from recovering badly — `metrics.rs` counts only the records that carry
    /// one.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub recovery: Option<RecordedRecovery>,
}

impl RunRecord {
    pub fn new(
        scenario: &str,
        fixture_version: &str,
        tier: &str,
        provider: &str,
        model: &str,
    ) -> Self {
        Self {
            scenario: scenario.to_string(),
            fixture_version: fixture_version.to_string(),
            tier: tier.to_string(),
            provider: provider.to_string(),
            model: model.to_string(),
            started_at_ms: 0,
            duration_ms: 0,
            tool_calls: Vec::new(),
            approvals: Vec::new(),
            files_changed: Vec::new(),
            checks: Vec::new(),
            final_status: "not_run".to_string(),
            tokens: Tokens {
                input: 0,
                output: 0,
                measured: false,
            },
            cost: None,
            assertions: Vec::new(),
            not_applicable: None,
            tool_rounds: 0,
            model_calls: 0,
            approval_policy_violations: 0,
            recovery: None,
        }
    }

    /// Redacts every free-text field a secret could reach before the record is
    /// written. Requirement 9 is asserted by the seeded-secret scenario, which
    /// greps the emitted records; this is what makes that assertion pass.
    pub fn sanitize(&mut self, scanner: &SecretScanner) {
        for call in &mut self.tool_calls {
            let rendered = call.arguments.to_string();
            let redacted = scanner.redact(&rendered);
            // Re-parsed when it still parses so the record keeps its shape;
            // falls back to the redacted string when redaction broke the JSON.
            call.arguments = serde_json::from_str(&redacted.text)
                .unwrap_or_else(|_| serde_json::Value::String(redacted.text.clone()));
        }
        for check in &mut self.checks {
            check.command = scanner.redact(&check.command).text;
        }
        for assertion in &mut self.assertions {
            assertion.expected = scanner.redact(&assertion.expected).text;
            assertion.actual = scanner.redact(&assertion.actual).text;
        }
    }
}

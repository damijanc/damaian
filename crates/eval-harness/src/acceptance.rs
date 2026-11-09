//! Spec 106: the single-worker acceptance gate.
//!
//! The acceptance data for a fixture lives in `acceptance/<fixture>/`, beside
//! `fixtures/` rather than inside it. A fixture is copied into the repository
//! the worker works in, so anything the worker must not read — the scripted
//! answer to the material clarification, the expected values, the protected
//! paths the run is scored on — stays out of it. The fixture test asserts that
//! no answer marker leaks into the fixture.

pub mod criteria;
pub mod driver;
pub mod policy;
pub mod report;
pub mod worker;

use std::fs;
use std::path::{Path, PathBuf};

use serde::Deserialize;
use workspace_engine::{ClientError, Result};

/// Where acceptance data lives: `crates/eval-harness/acceptance/`.
pub fn acceptance_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("acceptance")
}

/// Everything a run needs to drive and score one fixture.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AcceptanceFixture {
    pub fixture: String,
    /// The worker prompt, read from `prompt_file`. Both tiers send this exact
    /// text.
    pub prompt: String,
    pub protected_paths: Vec<String>,
    pub expected_command_families: Vec<String>,
    pub clarification: Clarification,
    pub expected: ExpectedValues,
    pub scoped_check: CheckSpec,
    pub pre_existing_failure: KnownFailure,
    pub regression: Regression,
    pub progress_record: ProgressRecord,
}

/// The material clarification (spec 106 requirement 4).
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct Clarification {
    /// A question containing any of these (case-insensitive) is the material
    /// clarification and gets `answer`.
    pub keywords: Vec<String>,
    pub answer: String,
    /// Phrases that would reveal the answer if they appeared in the fixture.
    pub answer_markers: Vec<String>,
}

/// The acceptance detail the clarification changes.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct ExpectedValues {
    pub tie_input_mills: i64,
    pub tie_cents: i64,
    /// What a worker that guessed instead of asking would produce.
    pub tie_cents_if_half_up: i64,
}

/// A check by its identity: repository-relative directory (`""` is the root)
/// and command.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct CheckSpec {
    pub working_directory: String,
    pub command: String,
}

/// The failure that exists before the task starts (spec 106 requirement 2).
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct KnownFailure {
    pub working_directory: String,
    pub command: String,
    /// The failing test's name as `cargo test` prints it.
    pub test: String,
}

/// The test a correct change breaks: the regression the worker must repair.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct Regression {
    pub crate_name: String,
    pub test: String,
}

/// The task-record row the worker must update (spec 106 requirement 8).
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct ProgressRecord {
    pub path: String,
    pub row_prefix: String,
    pub done_state: String,
}

#[derive(Debug, Deserialize)]
struct RawAcceptance {
    fixture: String,
    prompt_file: String,
    protected_paths: Vec<String>,
    expected_command_families: Vec<String>,
    clarification: Clarification,
    expected: ExpectedValues,
    scoped_check: CheckSpec,
    pre_existing_failure: KnownFailure,
    regression: Regression,
    progress_record: ProgressRecord,
}

impl AcceptanceFixture {
    /// Loads `acceptance/<name>/acceptance.toml` and its prompt file.
    pub fn load(name: &str) -> Result<Self> {
        Self::load_from(&acceptance_dir().join(name))
    }

    /// Loads acceptance data from an explicit directory.
    pub fn load_from(dir: &Path) -> Result<Self> {
        let path = dir.join("acceptance.toml");
        let text = fs::read_to_string(&path)
            .map_err(|error| ClientError::InvalidInput(format!("{}: {error}", path.display())))?;
        let raw: RawAcceptance = toml::from_str(&text)
            .map_err(|error| ClientError::InvalidInput(format!("{}: {error}", path.display())))?;
        let prompt_path = dir.join(&raw.prompt_file);
        let prompt = fs::read_to_string(&prompt_path).map_err(|error| {
            ClientError::InvalidInput(format!("{}: {error}", prompt_path.display()))
        })?;
        if raw.clarification.keywords.is_empty() || raw.clarification.answer.trim().is_empty() {
            return Err(ClientError::InvalidInput(format!(
                "{}: the clarification needs keywords and an answer",
                path.display()
            )));
        }
        Ok(Self {
            fixture: raw.fixture,
            prompt,
            protected_paths: raw.protected_paths,
            expected_command_families: raw.expected_command_families,
            clarification: raw.clarification,
            expected: raw.expected,
            scoped_check: raw.scoped_check,
            pre_existing_failure: raw.pre_existing_failure,
            regression: raw.regression,
            progress_record: raw.progress_record,
        })
    }
}

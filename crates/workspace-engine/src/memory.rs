//! Persistent memory: the record, its project identity, and the store
//! (`docs/specs/28_memory_model_and_storage/proposal.md`).
//!
//! Memory is the one feature that gives the agent cross-session influence, so
//! the shape of the record carries the safety properties the later specs rely
//! on. Three deliberate choices live in this module and should not be relaxed
//! without changing the specification:
//!
//! * [`MemoryProvenance`] has no model-inference variant. An unconfirmed
//!   suggestion is not a `MemoryEntry` at all; spec 29 owns proposals.
//! * [`Sensitivity`] is derived from the confirmed entry by rule, never read
//!   from a caller, so a statement cannot be labelled `Ordinary` to reach the
//!   retained audit log.
//! * The store refuses a detected secret rather than storing a redaction. A
//!   redacted memory is permanently useless and records that a credential
//!   exists.

use crate::error::Result;
use crate::git_service::GitService;
use crate::hash::{create_id, now_millis, repository_id_for_root, sha256_hex};
use serde::{Deserialize, Serialize};
use std::path::Path;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MemoryScope {
    Session,
    Project,
    User,
}

impl MemoryScope {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Session => "session",
            Self::Project => "project",
            Self::User => "user",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MemoryCategory {
    Preference,
    Decision,
    Convention,
    Environment,
    Command,
    Fact,
}

impl MemoryCategory {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Preference => "preference",
            Self::Decision => "decision",
            Self::Convention => "convention",
            Self::Environment => "environment",
            Self::Command => "command",
            Self::Fact => "fact",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MemoryState {
    Active,
    Superseded,
    Expired,
    Deleted,
}

impl MemoryState {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Active => "active",
            Self::Superseded => "superseded",
            Self::Expired => "expired",
            Self::Deleted => "deleted",
        }
    }
}

/// How much to trust the statement, and why.
///
/// A stale evidence-derived memory and a user-confirmed one are categorically
/// different, not different by degree, which is why this is an enum rather
/// than a float plus a flag. Spec 30 labels the first as uncertain.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Verification {
    /// The user stated it and confirmed it.
    UserConfirmed,
    /// Derived from repository evidence that still holds.
    EvidenceCurrent,
    /// Derived from repository evidence that has since changed.
    EvidenceStale,
}

impl Verification {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::UserConfirmed => "user_confirmed",
            Self::EvidenceCurrent => "evidence_current",
            Self::EvidenceStale => "evidence_stale",
        }
    }
}

/// What the statement is about, for handling and display.
///
/// Declaration order is the restrictiveness order, so `Ord` is the direction a
/// user may raise a value and the direction a write must never lower it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum Sensitivity {
    /// Safe to show anywhere, including a shared diagnostic report.
    #[default]
    Ordinary,
    /// Local paths, machine names, environment details. Local display only.
    Environmental,
    /// Personal preference. Never leaves the machine, never in a report.
    Personal,
}

impl Sensitivity {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Ordinary => "ordinary",
            Self::Environmental => "environmental",
            Self::Personal => "personal",
        }
    }
}

/// Where a memory came from.
///
/// There is deliberately **no** variant for a model inference with no user
/// action: spec 29's consent requirement is enforced by this type having
/// nowhere to put an unconfirmed entry. Do not add one.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "camelCase")]
pub enum MemoryProvenance {
    /// A user message asked for this.
    UserRequest {
        #[serde(rename = "sessionId")]
        session_id: String,
        #[serde(rename = "messageId")]
        message_id: String,
    },
    /// Suggested after a task and confirmed by the user.
    SuggestedAndConfirmed {
        #[serde(rename = "sessionId")]
        session_id: String,
        #[serde(rename = "taskId")]
        task_id: String,
    },
    /// Observed from repository content. Carries the evidence, per §5.5.
    RepositoryEvidence {
        #[serde(rename = "paths")]
        paths: Vec<EvidenceRef>,
    },
}

impl MemoryProvenance {
    /// The provenance kind as an audit field. Never the identifiers: an audit
    /// line should say *why* the record was allowed to exist, not repeat the
    /// session and message it came from.
    pub fn kind(&self) -> &'static str {
        match self {
            Self::UserRequest { .. } => "user_request",
            Self::SuggestedAndConfirmed { .. } => "suggested_and_confirmed",
            Self::RepositoryEvidence { .. } => "repository_evidence",
        }
    }
}

/// A file and the content hash it had when the memory was created.
///
/// The hash is what makes staleness detectable by comparison (spec 30 §5.5).
/// It holds a path and a hash, never file content.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct EvidenceRef {
    pub path: String,
    /// Content hash when the memory was created.
    pub content_hash: String,
}

impl EvidenceRef {
    pub fn new(path: impl Into<String>, content_hash: impl Into<String>) -> Self {
        Self {
            path: path.into(),
            content_hash: content_hash.into(),
        }
    }
}

/// When and how an entry stops being current.
///
/// Both halves are optional and independent: a staging URL may carry a date,
/// while a repository observation is revalidated when its evidence changes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct MemoryExpiry {
    /// Absolute expiry the user set, in milliseconds since the Unix epoch.
    pub expires_at_ms: Option<u128>,
    /// Re-derive [`Verification`] when an evidence file's hash changes.
    pub revalidate_on_evidence_change: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MemoryEntry {
    pub id: String,
    pub scope: MemoryScope,
    /// Set for Project scope; None for Session and User.
    pub project_key: Option<String>,
    /// Set for Session scope only.
    pub session_id: Option<String>,
    pub category: MemoryCategory,
    /// One sentence, normalized. Never a file excerpt.
    pub statement: String,
    pub provenance: MemoryProvenance,
    pub verification: Verification,
    pub sensitivity: Sensitivity,
    pub state: MemoryState,
    pub created_at_ms: u128,
    pub updated_at_ms: u128,
    /// Set when this entry replaced another.
    pub supersedes: Option<String>,
    pub expiry: Option<MemoryExpiry>,
}

/// A statement is one sentence, not an excerpt. The ceiling is what bounds
/// what a memory store can accumulate, and a longer candidate is refused
/// rather than truncated (spec 29 §5.5): a cut sentence is a misleading one.
pub const STATEMENT_MAX_CHARS: usize = 280;

impl MemoryEntry {
    /// Builds an entry for a new statement. `id`, timestamps and the initial
    /// state are Damaian's, not a caller's, so two creations cannot collide and
    /// a record cannot claim to have been born `Deleted`.
    pub fn new(
        scope: MemoryScope,
        category: MemoryCategory,
        statement: impl Into<String>,
        provenance: MemoryProvenance,
        verification: Verification,
    ) -> Self {
        let now = now_millis();
        Self {
            id: create_id("mem"),
            scope,
            project_key: None,
            session_id: None,
            category,
            statement: statement.into(),
            provenance,
            verification,
            sensitivity: Sensitivity::Ordinary,
            state: MemoryState::Active,
            created_at_ms: now,
            updated_at_ms: now,
            supersedes: None,
            expiry: None,
        }
    }

    pub fn in_project(mut self, project_key: impl Into<String>) -> Self {
        self.project_key = Some(project_key.into());
        self
    }

    pub fn in_session(mut self, session_id: impl Into<String>) -> Self {
        self.session_id = Some(session_id.into());
        self
    }

    pub fn with_expiry(mut self, expiry: MemoryExpiry) -> Self {
        self.expiry = Some(expiry);
        self
    }

    /// The sensitivity the rule assigns, ignoring what the entry currently
    /// claims.
    ///
    /// A statement naming a path, hostname, port or the local user is
    /// `Environmental`; a `Preference` or a user-scope entry is `Personal`;
    /// anything else is `Ordinary`. The most restrictive match wins, so an
    /// uncertain case resolves away from disclosure.
    pub fn derived_sensitivity(&self) -> Sensitivity {
        let environmental = statement_has_environmental_marker(&self.statement, local_names());
        let personal =
            self.category == MemoryCategory::Preference || self.scope == MemoryScope::User;
        match (environmental, personal) {
            (_, true) => Sensitivity::Personal,
            (true, false) => Sensitivity::Environmental,
            (false, false) => Sensitivity::Ordinary,
        }
    }

    /// What the store should persist: the maximum of the caller's claim and
    /// the rule. A user may raise an entry's sensitivity, never lower it below
    /// what the rule derives.
    pub fn effective_sensitivity(&self) -> Sensitivity {
        self.sensitivity.max(self.derived_sensitivity())
    }

    pub fn with_derived_sensitivity(mut self) -> Self {
        self.sensitivity = self.effective_sensitivity();
        self
    }
}

fn local_names() -> Vec<String> {
    let mut names = Vec::new();
    for key in ["USER", "LOGNAME"] {
        if let Ok(value) = std::env::var(key) {
            names.push(value);
        }
    }
    if let Ok(home) = std::env::var("HOME")
        && let Some(name) = std::path::Path::new(&home)
            .file_name()
            .map(|name| name.to_string_lossy().to_string())
    {
        names.push(name);
    }
    names
}

/// True when a statement names something that ties it to this machine: an
/// absolute path, a hostname, a `host:port`, an IPv4 address, or the local user
/// name.
///
/// The check over-classifies rather than under-classifies. A false
/// `Environmental` costs a user one audit line; a false `Ordinary` writes a
/// personal statement into a retained log.
fn statement_has_environmental_marker(statement: &str, names: Vec<String>) -> bool {
    let lowered = statement.to_lowercase();
    let words: Vec<String> = lowered
        .split(|character: char| character.is_whitespace() || ",;()[]\"'`".contains(character))
        .map(|word| word.trim_matches('.').to_string())
        .filter(|word| !word.is_empty())
        .collect();

    if words.iter().any(|word| looks_like_path(word)) {
        return true;
    }
    if words.iter().any(|word| looks_like_host_or_port(word)) {
        return true;
    }
    names.iter().any(|name| {
        let name = name.trim().to_lowercase();
        name.len() >= 3 && words.iter().any(|word| word == &name)
    })
}

fn looks_like_path(word: &str) -> bool {
    // `https://…` is a URL, not an absolute path; its host is caught by the
    // hostname rule instead.
    if word.contains("://") {
        return false;
    }
    let path = word.trim_start_matches(['(', '[', '{']);
    (path.starts_with('/') && path.len() > 1) || path.starts_with("~/")
}

fn looks_like_host_or_port(word: &str) -> bool {
    if word.contains("://") {
        // Treat the URL's host as a hostname even though it is not a bare
        // token; a URL in a memory is environmental either way.
        return true;
    }
    if word.contains("localhost") {
        return true;
    }
    // `host:port` or a bare `:port`.
    if let Some((host, port)) = word.rsplit_once(':')
        && !port.is_empty()
        && port.chars().all(|character| character.is_ascii_digit())
        && host.chars().all(|character| {
            character.is_ascii_alphanumeric() || matches!(character, '.' | '-' | '_')
        })
    {
        return true;
    }
    if is_ipv4(word) {
        return true;
    }
    // A dotted hostname: at least two dots, a label-safe body and an alphabetic
    // top-level label. This catches `api.example.com` and `build.internal`;
    // a file name with a single dot does not match.
    let dot_count = word.chars().filter(|character| *character == '.').count();
    if dot_count >= 2 {
        let labels: Vec<&str> = word.split('.').collect();
        let labels_are_host_like = labels.iter().all(|label| {
            !label.is_empty()
                && label
                    .chars()
                    .all(|character| character.is_ascii_alphanumeric() || character == '-')
        });
        let tld_is_alphabetic = labels.last().is_some_and(|label| {
            label
                .chars()
                .all(|character| character.is_ascii_alphabetic())
        });
        return labels_are_host_like && tld_is_alphabetic;
    }
    false
}

fn is_ipv4(word: &str) -> bool {
    let parts: Vec<&str> = word.split('.').collect();
    parts.len() == 4
        && parts
            .iter()
            .all(|part| !part.is_empty() && part.parse::<u8>().is_ok())
}

/// Which resolution tier produced a [`ProjectIdentity`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProjectKeySource {
    /// The repository's first root commit. Survives clones, moves, renames,
    /// remote changes and forks.
    RootCommit,
    /// The normalized `origin` remote. Survives clones and moves, not a remote
    /// rename.
    RemoteUrl,
    /// The existing path-derived `repository_id`. Nothing survives; the
    /// per-checkout fallback.
    RepositoryId,
}

impl ProjectKeySource {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::RootCommit => "root_commit",
            Self::RemoteUrl => "remote_url",
            Self::RepositoryId => "repository_id",
        }
    }
}

/// Memory's own repository identity, separate from `repository_id`.
///
/// `repository_id` is a hash of the checkout's path, which is the right
/// identity for permissions and the wrong one for memory: two clones of one
/// repository must share memory, and a moved checkout must keep it. Changing
/// `repository_id` would invalidate the keyword index, the vector index,
/// sessions, checkpoints, the repository map and file access, so memory gets a
/// second key instead (spec 28 §5.2).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProjectIdentity {
    /// `root_…`, `remote_…` or `path_…`. The prefix names the tier, which is
    /// how the UI can report the source without a second stored field, and the
    /// suffix is already filesystem-safe.
    pub key: String,
    pub source: ProjectKeySource,
    pub repository_id: String,
}

impl ProjectIdentity {
    /// Resolves in the proposal's order: root commit, then normalized origin,
    /// then the path hash. Each tier is attempted only when the previous one
    /// produced nothing.
    pub fn resolve(
        root: impl AsRef<Path>,
        repository_id: impl Into<String>,
        git: &GitService,
    ) -> Result<Self> {
        let root = root.as_ref();
        let repository_id = repository_id.into();

        if let Some(commit) = git.root_commit(root)?
            && !commit.is_empty()
        {
            return Ok(Self {
                key: format!("root_{}", commit.to_lowercase()),
                source: ProjectKeySource::RootCommit,
                repository_id,
            });
        }

        if let Some(url) = git.origin_url(root)?
            && let Some(normalized) = normalize_remote_url(&url)
        {
            return Ok(Self {
                key: format!("remote_{}", &sha256_hex(normalized.as_bytes())[..16]),
                source: ProjectKeySource::RemoteUrl,
                repository_id,
            });
        }

        Ok(Self {
            key: format!("path_{repository_id}"),
            source: ProjectKeySource::RepositoryId,
            repository_id,
        })
    }

    /// [`Self::resolve`] with the path-derived id computed from `root`.
    pub fn resolve_for_root(root: impl AsRef<Path>, git: &GitService) -> Result<Self> {
        let root = root.as_ref();
        Self::resolve(root, repository_id_for_root(root), git)
    }

    /// Reads the tier back out of a stored key, for a UI or a troubleshooting
    /// reader. `None` for a key no build of Damaian wrote.
    pub fn source_of_key(key: &str) -> Option<ProjectKeySource> {
        if key.starts_with("root_") {
            Some(ProjectKeySource::RootCommit)
        } else if key.starts_with("remote_") {
            Some(ProjectKeySource::RemoteUrl)
        } else if key.starts_with("path_repo_") {
            Some(ProjectKeySource::RepositoryId)
        } else {
            None
        }
    }
}

/// Normalizes an `origin` URL to `host/path`, stripping the scheme, any
/// credentials, a port and a trailing `.git`.
///
/// Handles both `https://user:token@host:443/owner/repo.git` and the scp-style
/// `git@host:owner/repo.git`. A remote that is a local filesystem path has no
/// host to share and returns `None`, so identity falls through to the path
/// tier rather than pretending a machine-local path is portable.
pub fn normalize_remote_url(url: &str) -> Option<String> {
    let url = url.trim();
    if url.is_empty() {
        return None;
    }

    let (host, path) = if let Some((_scheme, rest)) = url.split_once("://") {
        let rest = rest.trim_start_matches('/');
        let (authority, path) = rest.split_once('/').unwrap_or((rest, ""));
        (authority, path)
    } else if let Some((user_host, path)) = url.split_once(':')
        && !url.starts_with('/')
        && !url.starts_with('.')
    {
        // scp-style `user@host:path`, or a bare `host:path`. A local absolute
        // path or a relative `./` path is not this shape and falls through.
        let host = user_host
            .rsplit_once('@')
            .map_or(user_host, |(_, host)| host);
        (host, path)
    } else {
        return None;
    };

    // Credentials may appear without a scheme in the scp form; the caller
    // already stripped them above, and the authority form is handled here.
    let authority = host.rsplit_once('@').map_or(host, |(_, host)| host);
    let authority = match authority.rsplit_once(':') {
        Some((host, port)) if !port.is_empty() && port.chars().all(|c| c.is_ascii_digit()) => host,
        _ => authority,
    };
    let host = authority.trim().to_lowercase();
    if host.is_empty() {
        return None;
    }

    let path = path
        .trim()
        .trim_matches('/')
        .trim_end_matches(".git")
        .trim_end_matches('/');
    if path.is_empty() {
        return None;
    }
    Some(format!("{host}/{path}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(category: MemoryCategory, scope: MemoryScope, statement: &str) -> MemoryEntry {
        MemoryEntry::new(
            scope,
            category,
            statement,
            MemoryProvenance::UserRequest {
                session_id: "session_1".to_string(),
                message_id: "message_1".to_string(),
            },
            Verification::UserConfirmed,
        )
    }

    #[test]
    fn a_plain_project_convention_is_ordinary() {
        let entry = entry(
            MemoryCategory::Convention,
            MemoryScope::Project,
            "Integration tests name their fixture repository explicitly.",
        );
        assert_eq!(entry.derived_sensitivity(), Sensitivity::Ordinary);
    }

    #[test]
    fn a_statement_naming_a_path_is_environmental() {
        for statement in [
            "Builds write into /tmp/damaian-build.",
            "The helper lives in ~/bin/check-test.",
        ] {
            let entry = entry(MemoryCategory::Fact, MemoryScope::Project, statement);
            assert_eq!(
                entry.derived_sensitivity(),
                Sensitivity::Environmental,
                "{statement}"
            );
        }
    }

    #[test]
    fn a_statement_naming_a_host_or_port_is_environmental() {
        for statement in [
            "The staging API is api.staging.example.com.",
            "Postgres listens on localhost:5433.",
            "The dashboard runs at 10.0.0.12.",
            "The debug port is :9229.",
        ] {
            let entry = entry(MemoryCategory::Environment, MemoryScope::Project, statement);
            assert_eq!(
                entry.derived_sensitivity(),
                Sensitivity::Environmental,
                "{statement}"
            );
        }
    }

    #[test]
    fn a_statement_naming_the_local_user_is_environmental() {
        let user = std::env::var("USER").unwrap_or_default();
        if user.len() < 3 {
            return;
        }
        let entry = entry(
            MemoryCategory::Fact,
            MemoryScope::Project,
            &format!("Only {user} runs the release check."),
        );
        assert_eq!(entry.derived_sensitivity(), Sensitivity::Environmental);
    }

    #[test]
    fn a_preference_or_a_user_scope_entry_is_personal() {
        let preference = entry(
            MemoryCategory::Preference,
            MemoryScope::Project,
            "Prefer short commit subjects.",
        );
        assert_eq!(preference.derived_sensitivity(), Sensitivity::Personal);

        let user = entry(
            MemoryCategory::Fact,
            MemoryScope::User,
            "This person reviews Rust on Tuesday.",
        );
        assert_eq!(user.derived_sensitivity(), Sensitivity::Personal);
    }

    #[test]
    fn the_more_restrictive_rule_wins_when_both_match() {
        let entry = entry(
            MemoryCategory::Preference,
            MemoryScope::Project,
            "Prefer building in ~/work/build.",
        );
        assert_eq!(entry.derived_sensitivity(), Sensitivity::Personal);
    }

    #[test]
    fn a_supplied_lower_value_is_raised_but_a_higher_one_is_kept() {
        let lowered = MemoryEntry {
            sensitivity: Sensitivity::Ordinary,
            ..entry(
                MemoryCategory::Fact,
                MemoryScope::Project,
                "The logs are under /var/log/damaian.",
            )
        };
        assert_eq!(
            lowered.clone().with_derived_sensitivity().sensitivity,
            Sensitivity::Environmental
        );

        let raised = MemoryEntry {
            sensitivity: Sensitivity::Personal,
            ..entry(
                MemoryCategory::Fact,
                MemoryScope::Project,
                "A plain statement.",
            )
        };
        assert_eq!(
            raised.with_derived_sensitivity().sensitivity,
            Sensitivity::Personal
        );
    }

    #[test]
    fn sensitivity_order_is_restrictiveness_order() {
        assert!(Sensitivity::Ordinary < Sensitivity::Environmental);
        assert!(Sensitivity::Environmental < Sensitivity::Personal);
    }

    #[test]
    fn a_remote_normalizes_to_host_and_path() {
        assert_eq!(
            normalize_remote_url("https://user:token@GitHub.com:443/Owner/Repo.git").as_deref(),
            Some("github.com/Owner/Repo")
        );
        assert_eq!(
            normalize_remote_url("git@github.com:owner/repo.git").as_deref(),
            Some("github.com/owner/repo")
        );
        assert_eq!(
            normalize_remote_url("ssh://git@github.com/owner/repo.git/").as_deref(),
            Some("github.com/owner/repo")
        );
    }

    #[test]
    fn a_remote_with_no_portable_host_does_not_become_an_identity() {
        assert_eq!(normalize_remote_url("/srv/git/repo.git"), None);
        assert_eq!(normalize_remote_url("../repo.git"), None);
        assert_eq!(normalize_remote_url("https://github.com"), None);
        assert_eq!(normalize_remote_url(""), None);
    }

    #[test]
    fn a_project_key_names_its_tier() {
        assert_eq!(
            ProjectIdentity::source_of_key("root_0f1e2d"),
            Some(ProjectKeySource::RootCommit)
        );
        assert_eq!(
            ProjectIdentity::source_of_key("remote_0123456789abcdef"),
            Some(ProjectKeySource::RemoteUrl)
        );
        assert_eq!(
            ProjectIdentity::source_of_key("path_repo_0123456789abcdef"),
            Some(ProjectKeySource::RepositoryId)
        );
        assert_eq!(ProjectIdentity::source_of_key("mem_1"), None);
    }

    #[test]
    fn provenance_serialises_camel_case_fields_under_a_type_tag() {
        let value = serde_json::to_value(MemoryProvenance::SuggestedAndConfirmed {
            session_id: "s".to_string(),
            task_id: "t".to_string(),
        })
        .unwrap();
        assert_eq!(value["type"], "suggestedAndConfirmed");
        assert_eq!(value["sessionId"], "s");
        assert_eq!(value["taskId"], "t");
    }

    #[test]
    fn there_is_no_provenance_variant_for_an_unconfirmed_inference() {
        // The test is the match: adding a fourth variant fails to compile here
        // and must be a deliberate specification change.
        fn kind(provenance: &MemoryProvenance) -> &'static str {
            match provenance {
                MemoryProvenance::UserRequest { .. } => "user_request",
                MemoryProvenance::SuggestedAndConfirmed { .. } => "suggested_and_confirmed",
                MemoryProvenance::RepositoryEvidence { .. } => "repository_evidence",
            }
        }
        let all = [
            MemoryProvenance::UserRequest {
                session_id: String::new(),
                message_id: String::new(),
            },
            MemoryProvenance::SuggestedAndConfirmed {
                session_id: String::new(),
                task_id: String::new(),
            },
            MemoryProvenance::RepositoryEvidence { paths: vec![] },
        ];
        assert_eq!(
            all.iter().map(kind).collect::<Vec<_>>(),
            [
                "user_request",
                "suggested_and_confirmed",
                "repository_evidence"
            ]
        );
    }
}

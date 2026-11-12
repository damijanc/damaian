//! The shell's route table and each route's class (spec 110 D2).
//!
//! Every route is a [`Route`] variant, and both the dispatch in
//! `handle_connection` and [`Route::is_session_work`] are exhaustive matches
//! with no wildcard arm, so a new route does not compile until it has chosen
//! a handler and a class.

/// One route the shell serves. Paths that serve the same thing (`/app.js` and
/// `/assets/app.js`) share a variant.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) enum Route {
    Index,
    StyleCss,
    AppJs,
    XtermJs,
    XtermAddonFitJs,
    XtermCss,
    Bootstrap,
    WebDiagnosticArtifact,
    Config,
    GetConfigFile,
    EffectivePolicy,
    ProviderTokenDefaults,
    PermissionProfile,
    PermissionProfiles,
    PermissionProfileExport,
    PermissionProfileImport,
    ModelKeyStatus,
    GitStatus,
    CommitMessage,
    CommitStatus,
    CommitPreparation,
    Commit,
    BranchInfo,
    BranchSuggestion,
    BranchPrepare,
    BranchChange,
    MergePreparation,
    TerminalCwd,
    TerminalRun,
    Sessions,
    SessionSearch,
    SessionExport,
    WorkingSessions,
    CancelSessionWork,
    Recovery,
    RecoveryDecision,
    RecoveryApprovalResolved,
    Checkpoints,
    Rewind,
    Session,
    SessionCreate,
    SessionRename,
    SessionArchive,
    SessionUnarchive,
    SessionMode,
    ContextPin,
    ContextRestriction,
    ContextRemove,
    ContextRemoveSummary,
    ContextMemory,
    ResumeContextStream,
    MemoryProposals,
    MemoryConfirm,
    MemoryDismiss,
    Findings,
    FindingStatus,
    FindingsRepair,
    SessionDelete,
    Compact,
    OpenVscode,
    RevealInFinder,
    OpenGithub,
    RevealWebDiagnosticArtifact,
    ContextFile,
    OpenVscodeFile,
    RenderMarkdown,
    AskStream,
    ResumeCommandStream,
    ResumeFetchStream,
    FetchDecision,
    VerifyStream,
    ContinueRepairStream,
    ContinueTaskStream,
    ContinueWorkerResultStream,
    ResumeChecksStream,
    ResumePlanStream,
    PlanWorkerPrompt,
    PlanToWorker,
    AnswerQuestionStream,
    ResumeFallbackStream,
    ResumeRoundBudgetStream,
    Ask,
    ProposeEdit,
    ApplyPatch,
    RollbackPatch,
    RejectPatchFiles,
    RejectPatch,
    ProposeCommand,
    RunCommand,
    RejectCommand,
    RepositoryMap,
    RepositoryRoots,
    Symbols,
    SymbolReferences,
    LanguageServers,
    CommandProposal,
    RepositoryConfigReview,
    RepositoryConfigAllowlist,
    Hooks,
    HookReview,
    HookEnabled,
    HookTest,
    ConfigSet,
    PostConfigFile,
    ModelKey,
    ProviderKey,
    ModelKeyDelete,
    McpTest,
    WorkersEnabled,
    WorkerSourceStatus,
    StartWorker,
    ListWorkers,
    WorkerProgress,
    WorkerControl,
    WorkerRestart,
    WorkerRemoveWorktree,
}

/// Method, path, route. The only place a path is spelled.
pub(crate) const ROUTES: &[(&str, &str, Route)] = &[
    ("GET", "/", Route::Index),
    ("GET", "/style.css", Route::StyleCss),
    ("GET", "/assets/style.css", Route::StyleCss),
    ("GET", "/app.js", Route::AppJs),
    ("GET", "/assets/app.js", Route::AppJs),
    ("GET", "/xterm.js", Route::XtermJs),
    ("GET", "/assets/xterm.js", Route::XtermJs),
    ("GET", "/xterm-addon-fit.js", Route::XtermAddonFitJs),
    ("GET", "/assets/xterm-addon-fit.js", Route::XtermAddonFitJs),
    ("GET", "/xterm.css", Route::XtermCss),
    ("GET", "/assets/xterm.css", Route::XtermCss),
    ("GET", "/api/bootstrap", Route::Bootstrap),
    (
        "GET",
        "/api/web-diagnostic-artifact",
        Route::WebDiagnosticArtifact,
    ),
    ("GET", "/api/config", Route::Config),
    ("GET", "/api/config-file", Route::GetConfigFile),
    ("GET", "/api/effective-policy", Route::EffectivePolicy),
    (
        "GET",
        "/api/provider-token-defaults",
        Route::ProviderTokenDefaults,
    ),
    ("POST", "/api/permission-profile", Route::PermissionProfile),
    ("GET", "/api/permission-profiles", Route::PermissionProfiles),
    (
        "GET",
        "/api/permission-profile-export",
        Route::PermissionProfileExport,
    ),
    (
        "POST",
        "/api/permission-profile-import",
        Route::PermissionProfileImport,
    ),
    ("GET", "/api/model-key-status", Route::ModelKeyStatus),
    ("GET", "/api/git-status", Route::GitStatus),
    ("POST", "/api/commit-message", Route::CommitMessage),
    ("GET", "/api/commit-status", Route::CommitStatus),
    ("POST", "/api/commit-preparation", Route::CommitPreparation),
    ("POST", "/api/commit", Route::Commit),
    ("GET", "/api/branch-info", Route::BranchInfo),
    ("POST", "/api/branch-suggestion", Route::BranchSuggestion),
    ("POST", "/api/branch-prepare", Route::BranchPrepare),
    ("POST", "/api/branch", Route::BranchChange),
    ("GET", "/api/merge-preparation", Route::MergePreparation),
    ("GET", "/api/terminal-cwd", Route::TerminalCwd),
    ("POST", "/api/terminal-run", Route::TerminalRun),
    ("GET", "/api/sessions", Route::Sessions),
    ("GET", "/api/session-search", Route::SessionSearch),
    ("GET", "/api/session-export", Route::SessionExport),
    ("GET", "/api/working-sessions", Route::WorkingSessions),
    ("POST", "/api/session-work-cancel", Route::CancelSessionWork),
    ("GET", "/api/recovery", Route::Recovery),
    ("POST", "/api/recovery-decision", Route::RecoveryDecision),
    (
        "POST",
        "/api/recovery-approval-resolved",
        Route::RecoveryApprovalResolved,
    ),
    ("GET", "/api/checkpoints", Route::Checkpoints),
    ("POST", "/api/rewind", Route::Rewind),
    ("GET", "/api/session", Route::Session),
    ("POST", "/api/session-create", Route::SessionCreate),
    ("POST", "/api/session-rename", Route::SessionRename),
    ("POST", "/api/session-archive", Route::SessionArchive),
    ("POST", "/api/session-unarchive", Route::SessionUnarchive),
    ("POST", "/api/session-mode", Route::SessionMode),
    ("POST", "/api/context-pin", Route::ContextPin),
    (
        "POST",
        "/api/context-restriction",
        Route::ContextRestriction,
    ),
    ("POST", "/api/context-remove", Route::ContextRemove),
    (
        "POST",
        "/api/context-remove-summary",
        Route::ContextRemoveSummary,
    ),
    ("POST", "/api/context-memory", Route::ContextMemory),
    (
        "POST",
        "/api/resume-context-stream",
        Route::ResumeContextStream,
    ),
    ("GET", "/api/memory-proposals", Route::MemoryProposals),
    ("POST", "/api/memory-confirm", Route::MemoryConfirm),
    ("POST", "/api/memory-dismiss", Route::MemoryDismiss),
    ("GET", "/api/findings", Route::Findings),
    ("POST", "/api/finding-status", Route::FindingStatus),
    ("POST", "/api/findings-repair", Route::FindingsRepair),
    ("POST", "/api/session-delete", Route::SessionDelete),
    ("POST", "/api/compact", Route::Compact),
    ("POST", "/api/open-vscode", Route::OpenVscode),
    ("POST", "/api/reveal-in-finder", Route::RevealInFinder),
    ("POST", "/api/open-github", Route::OpenGithub),
    (
        "POST",
        "/api/reveal-web-diagnostic-artifact",
        Route::RevealWebDiagnosticArtifact,
    ),
    ("POST", "/api/context-file", Route::ContextFile),
    ("POST", "/api/open-vscode-file", Route::OpenVscodeFile),
    ("POST", "/api/render-markdown", Route::RenderMarkdown),
    ("POST", "/api/ask-stream", Route::AskStream),
    (
        "POST",
        "/api/resume-command-stream",
        Route::ResumeCommandStream,
    ),
    ("POST", "/api/resume-fetch-stream", Route::ResumeFetchStream),
    ("POST", "/api/fetch-decision", Route::FetchDecision),
    ("POST", "/api/verify-stream", Route::VerifyStream),
    (
        "POST",
        "/api/continue-repair-stream",
        Route::ContinueRepairStream,
    ),
    (
        "POST",
        "/api/continue-task-stream",
        Route::ContinueTaskStream,
    ),
    (
        "POST",
        "/api/continue-worker-result-stream",
        Route::ContinueWorkerResultStream,
    ),
    (
        "POST",
        "/api/resume-checks-stream",
        Route::ResumeChecksStream,
    ),
    ("POST", "/api/resume-plan-stream", Route::ResumePlanStream),
    ("POST", "/api/plan-worker-prompt", Route::PlanWorkerPrompt),
    ("POST", "/api/plan-to-worker", Route::PlanToWorker),
    (
        "POST",
        "/api/answer-question-stream",
        Route::AnswerQuestionStream,
    ),
    (
        "POST",
        "/api/resume-fallback-stream",
        Route::ResumeFallbackStream,
    ),
    (
        "POST",
        "/api/resume-round-budget-stream",
        Route::ResumeRoundBudgetStream,
    ),
    ("POST", "/api/ask", Route::Ask),
    ("POST", "/api/propose-edit", Route::ProposeEdit),
    ("POST", "/api/apply-patch", Route::ApplyPatch),
    ("POST", "/api/rollback-patch", Route::RollbackPatch),
    ("POST", "/api/reject-patch-files", Route::RejectPatchFiles),
    ("POST", "/api/reject-patch", Route::RejectPatch),
    ("POST", "/api/propose-command", Route::ProposeCommand),
    ("POST", "/api/run-command", Route::RunCommand),
    ("POST", "/api/reject-command", Route::RejectCommand),
    ("GET", "/api/repository-map", Route::RepositoryMap),
    ("POST", "/api/repository-roots", Route::RepositoryRoots),
    ("GET", "/api/symbols", Route::Symbols),
    ("GET", "/api/symbol-references", Route::SymbolReferences),
    ("GET", "/api/language-servers", Route::LanguageServers),
    ("GET", "/api/command-proposal", Route::CommandProposal),
    (
        "GET",
        "/api/repository-config-review",
        Route::RepositoryConfigReview,
    ),
    (
        "POST",
        "/api/repository-config-allowlist",
        Route::RepositoryConfigAllowlist,
    ),
    ("GET", "/api/hooks", Route::Hooks),
    ("POST", "/api/hook-review", Route::HookReview),
    ("POST", "/api/hook-enabled", Route::HookEnabled),
    ("POST", "/api/hook-test", Route::HookTest),
    ("POST", "/api/config-set", Route::ConfigSet),
    ("POST", "/api/config-file", Route::PostConfigFile),
    ("POST", "/api/model-key", Route::ModelKey),
    ("POST", "/api/provider-key", Route::ProviderKey),
    ("POST", "/api/model-key-delete", Route::ModelKeyDelete),
    ("POST", "/api/mcp-test", Route::McpTest),
    ("GET", "/api/workers-enabled", Route::WorkersEnabled),
    (
        "GET",
        "/api/worker-source-status",
        Route::WorkerSourceStatus,
    ),
    ("POST", "/api/workers", Route::StartWorker),
    ("GET", "/api/workers", Route::ListWorkers),
    ("GET", "/api/worker-progress", Route::WorkerProgress),
    ("POST", "/api/worker-control", Route::WorkerControl),
    ("POST", "/api/worker-restart", Route::WorkerRestart),
    (
        "POST",
        "/api/worker-remove-worktree",
        Route::WorkerRemoveWorktree,
    ),
];

/// The route `method path` names, or `None` for a 404.
pub(crate) fn route_for(method: &str, path: &str) -> Option<Route> {
    ROUTES
        .iter()
        .find(|(route_method, route_path, _)| *route_method == method && *route_path == path)
        .map(|(_, _, route)| *route)
}

/// What a request is to the shell (D2): a session's work, which holds that
/// session's lane while it runs, or UI, which is answered at once whatever
/// any session is doing.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum RouteClass {
    /// `None` when the request does not name its session: the first message
    /// of a new session (which claims the new session's lane as soon as its id
    /// exists), or a request that names only a patch, batch, checkpoint or
    /// command proposal. Spec 110 Task 1b resolves those to a session, because
    /// it must route each to that session's host.
    SessionWork(Option<String>),
    Ui,
}

impl Route {
    /// Whether this route is a session's work. No wildcard: a new route must
    /// choose.
    pub(crate) fn is_session_work(self) -> bool {
        match self {
            // Turns and every resume route: they run or continue a turn.
            Route::AskStream
            | Route::Ask
            | Route::ResumeCommandStream
            | Route::ResumeFetchStream
            | Route::FetchDecision
            | Route::VerifyStream
            | Route::ContinueRepairStream
            | Route::ContinueTaskStream
            | Route::ContinueWorkerResultStream
            | Route::ResumeChecksStream
            | Route::ResumePlanStream
            | Route::ResumeContextStream
            | Route::AnswerQuestionStream
            | Route::ResumeFallbackStream
            | Route::ResumeRoundBudgetStream
            // Command and check runs.
            | Route::ProposeCommand
            | Route::RunCommand
            | Route::RejectCommand
            // Patches and checkpoints.
            | Route::ProposeEdit
            | Route::ApplyPatch
            | Route::RollbackPatch
            | Route::RejectPatchFiles
            | Route::RejectPatch
            | Route::Rewind
            // A model call that rewrites the conversation.
            | Route::Compact
            // Recovery changes a task's state; on a working session it would
            // be deciding about a turn that is still running.
            | Route::RecoveryDecision
            | Route::RecoveryApprovalResolved
            // Starting a worker from a session (D2).
            | Route::PlanToWorker
            | Route::StartWorker => true,
            Route::Index
            | Route::StyleCss
            | Route::AppJs
            | Route::XtermJs
            | Route::XtermAddonFitJs
            | Route::XtermCss
            | Route::Bootstrap
            | Route::WebDiagnosticArtifact
            | Route::Config
            | Route::GetConfigFile
            | Route::EffectivePolicy
            | Route::ProviderTokenDefaults
            | Route::PermissionProfile
            | Route::PermissionProfiles
            | Route::PermissionProfileExport
            | Route::PermissionProfileImport
            | Route::ModelKeyStatus
            | Route::GitStatus
            | Route::CommitMessage
            | Route::CommitStatus
            | Route::CommitPreparation
            | Route::Commit
            | Route::BranchInfo
            | Route::BranchSuggestion
            | Route::BranchPrepare
            | Route::BranchChange
            | Route::MergePreparation
            | Route::TerminalCwd
            | Route::TerminalRun
            | Route::Sessions
            | Route::SessionSearch
            | Route::SessionExport
            | Route::WorkingSessions
            | Route::CancelSessionWork
            | Route::Recovery
            | Route::Checkpoints
            | Route::Session
            | Route::SessionCreate
            | Route::SessionRename
            // UI routes that refuse a working session themselves (D2, req. 3).
            | Route::SessionArchive
            | Route::SessionDelete
            | Route::SessionUnarchive
            | Route::SessionMode
            | Route::ContextPin
            | Route::ContextRestriction
            | Route::ContextRemove
            | Route::ContextRemoveSummary
            | Route::ContextMemory
            | Route::MemoryProposals
            | Route::MemoryConfirm
            | Route::MemoryDismiss
            | Route::Findings
            | Route::FindingStatus
            // Builds a repair prompt; the repair itself is an ask.
            | Route::FindingsRepair
            | Route::OpenVscode
            | Route::RevealInFinder
            | Route::OpenGithub
            | Route::RevealWebDiagnosticArtifact
            | Route::ContextFile
            | Route::OpenVscodeFile
            | Route::RenderMarkdown
            // Read-only: the pause is untouched.
            | Route::PlanWorkerPrompt
            | Route::RepositoryMap
            | Route::RepositoryRoots
            | Route::Symbols
            | Route::SymbolReferences
            | Route::LanguageServers
            | Route::CommandProposal
            | Route::RepositoryConfigReview
            | Route::RepositoryConfigAllowlist
            | Route::Hooks
            | Route::HookReview
            | Route::HookEnabled
            | Route::HookTest
            | Route::ConfigSet
            | Route::PostConfigFile
            | Route::ModelKey
            | Route::ProviderKey
            | Route::ModelKeyDelete
            | Route::McpTest
            | Route::WorkersEnabled
            | Route::WorkerSourceStatus
            | Route::ListWorkers
            | Route::WorkerProgress
            // A worker's own work runs in its own process and lane (spec 57).
            | Route::WorkerControl
            | Route::WorkerRestart
            | Route::WorkerRemoveWorktree => false,
        }
    }

    /// What the sidebar and a refusal call this work until the turn says more.
    pub(crate) fn work_label(self) -> &'static str {
        match self {
            Route::AskStream | Route::Ask => "Answering a message",
            Route::VerifyStream | Route::ResumeChecksStream => "Running checks",
            Route::ContinueRepairStream => "Repairing after verification",
            Route::ContinueTaskStream | Route::ContinueWorkerResultStream => "Continuing the task",
            Route::RunCommand | Route::ResumeCommandStream => "Running a command",
            Route::ApplyPatch | Route::RollbackPatch => "Applying a patch",
            Route::Rewind => "Rewinding",
            Route::Compact => "Compacting the conversation",
            Route::StartWorker | Route::PlanToWorker => "Starting a worker",
            _ => "Working",
        }
    }
}

/// The form field naming the session that a session-work route acts for, if
/// it names one directly.
pub(crate) fn session_field(route: Route) -> &'static str {
    match route {
        Route::StartWorker => "parent_session_id",
        _ => "session_id",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;

    #[test]
    fn every_path_is_spelled_once_and_resolves_to_its_route() {
        let mut seen = HashSet::new();
        for (method, path, route) in ROUTES {
            assert!(
                seen.insert((*method, *path)),
                "{method} {path} is listed twice"
            );
            assert_eq!(route_for(method, path), Some(*route));
        }
        assert_eq!(route_for("GET", "/api/nope"), None);
        assert_eq!(route_for("DELETE", "/api/sessions"), None);
    }

    /// The session-work set is exactly D2's list; everything else is UI. A
    /// change here is a change to what the UI can do during a turn, so it is
    /// spelled out rather than derived.
    #[test]
    fn every_route_is_classified_and_session_work_is_exactly_d2s_list() {
        let session_work: HashSet<&str> = ROUTES
            .iter()
            .filter(|(_, _, route)| route.is_session_work())
            .map(|(_, path, _)| *path)
            .collect();
        let expected: HashSet<&str> = [
            "/api/ask-stream",
            "/api/ask",
            "/api/resume-command-stream",
            "/api/resume-fetch-stream",
            "/api/fetch-decision",
            "/api/verify-stream",
            "/api/continue-repair-stream",
            "/api/continue-task-stream",
            "/api/continue-worker-result-stream",
            "/api/resume-checks-stream",
            "/api/resume-plan-stream",
            "/api/resume-context-stream",
            "/api/answer-question-stream",
            "/api/resume-fallback-stream",
            "/api/resume-round-budget-stream",
            "/api/propose-command",
            "/api/run-command",
            "/api/reject-command",
            "/api/propose-edit",
            "/api/apply-patch",
            "/api/rollback-patch",
            "/api/reject-patch-files",
            "/api/reject-patch",
            "/api/rewind",
            "/api/compact",
            "/api/recovery-decision",
            "/api/recovery-approval-resolved",
            "/api/plan-to-worker",
            "/api/workers",
        ]
        .into_iter()
        .collect();
        // `/api/workers` is listed for its POST only; the GET lists workers.
        assert!(!Route::ListWorkers.is_session_work());
        assert_eq!(session_work, expected);
        for ui in [
            "/api/sessions",
            "/api/session",
            "/api/session-archive",
            "/api/session-delete",
            "/api/session-rename",
            "/api/config-set",
            "/api/working-sessions",
            "/api/session-work-cancel",
        ] {
            assert!(!session_work.contains(ui), "{ui} must stay a UI route");
        }
    }
}

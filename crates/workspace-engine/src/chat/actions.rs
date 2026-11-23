use super::*;

impl ChatOrchestrator {
    /// The directory a requested command will run in, used to classify it
    /// before dispatch. A request the policy refuses resolves to the
    /// repository root here; the dispatch arm refuses it, with the reason,
    /// before anything is stored (spec 24 Task 7).
    pub(super) fn command_working_directory(
        &self,
        repository_root: &Path,
        request: &CommandRequest,
    ) -> PathBuf {
        resolve_command_directory(
            &self.path_policy,
            repository_root,
            request.working_directory.as_deref().unwrap_or_default(),
        )
        .unwrap_or_else(|_| repository_root.to_path_buf())
    }

    /// Asks the permission matrix about one dispatched action, supplying the
    /// context only two classes need (`permits`'s own contract): a
    /// command's classification — the same classifier `propose_command` runs,
    /// without storing a proposal — and an MCP tool's read-only hint, from the
    /// tool lists Layer 1 already fetched this turn.
    pub(super) fn action_permission(
        &self,
        mode: SessionMode,
        repository_root: &Path,
        action: &ToolAction,
        mcp: &McpRuntime,
    ) -> Permission {
        let classification = match action {
            ToolAction::Command(request) => {
                let working_directory = self.command_working_directory(repository_root, request);
                Some(self.validation_orchestrator.classify_command_at(
                    repository_root,
                    &working_directory,
                    &request.command,
                ))
            }
            _ => None,
        };
        let read_only_hint = match action {
            ToolAction::McpCall {
                server_id,
                tool_name,
                ..
            } => mcp.tool_read_only_hint(server_id, tool_name),
            _ => None,
        };
        permits(
            mode,
            &self.config.profile_capabilities(),
            action,
            classification.as_ref(),
            read_only_hint,
        )
    }

    /// The canonical identity of a dispatched call for repeat detection and
    /// the elision notice (spec 59 §5.3, spec 65 §5.3). `None` means the tool
    /// has no row in the proposal's table and is therefore never a repeat —
    /// the err-toward-exploration direction §5.3 requires, so a new read tool
    /// cannot silently start being policed before its arguments are
    /// considered. `turn_notes` has a row so an identical append is a repeat.
    pub(super) fn call_key(
        &self,
        repository_root: &Path,
        action: &ToolAction,
    ) -> Option<Vec<String>> {
        match action {
            ToolAction::ReadFile {
                path,
                range,
                worktree,
            } => {
                let (checkout, root) =
                    self.read_call_checkout(repository_root, worktree.as_deref());
                let mut key = vec![
                    "read_file".to_string(),
                    checkout,
                    self.read_call_path(&root, path),
                ];
                match range {
                    Some(range) => key.extend([
                        "range".to_string(),
                        range.start.to_string(),
                        range.end.to_string(),
                    ]),
                    None => key.push("no-range".to_string()),
                }
                Some(key)
            }
            ToolAction::ListDirectory {
                dir,
                depth,
                worktree,
            } => {
                let (checkout, root) =
                    self.read_call_checkout(repository_root, worktree.as_deref());
                // An omitted `dir` is the checkout root, so it keys the same as
                // an explicit `.` (§5.3).
                let mut key = vec![
                    "list_directory".to_string(),
                    checkout,
                    self.read_call_path(&root, dir.as_deref().unwrap_or(".")),
                ];
                match depth {
                    Some(depth) => key.extend(["depth".to_string(), depth.to_string()]),
                    None => key.push("no-depth".to_string()),
                }
                Some(key)
            }
            ToolAction::SearchContent {
                pattern,
                path_glob,
                max_matches,
                worktree,
            } => {
                let (checkout, root) =
                    self.read_call_checkout(repository_root, worktree.as_deref());
                let mut key = vec!["search_content".to_string(), checkout, pattern.clone()];
                match path_glob {
                    Some(glob) => {
                        key.extend(["glob".to_string(), self.read_call_path(&root, glob)]);
                    }
                    None => key.push("no-glob".to_string()),
                }
                match max_matches {
                    Some(cap) => key.extend(["max".to_string(), cap.to_string()]),
                    None => key.push("no-max".to_string()),
                }
                Some(key)
            }
            ToolAction::SearchCodebase {
                query,
                semantic,
                limit,
            } => Some(vec![
                "search_codebase".to_string(),
                query.clone(),
                semantic.to_string(),
                limit.to_string(),
            ]),
            ToolAction::ReadGitStatus { worktree } => {
                let (checkout, _) = self.read_call_checkout(repository_root, worktree.as_deref());
                Some(vec!["read_git_status".to_string(), checkout])
            }
            ToolAction::ReadGitDiff {
                staged,
                worktree,
                paths,
                stat,
                base,
            } => {
                let (checkout, root) =
                    self.read_call_checkout(repository_root, worktree.as_deref());
                // Path order does not change the diff, so the key sorts the
                // resolved paths; every other argument changes what it returns.
                let mut paths: Vec<String> = paths
                    .iter()
                    .map(|path| self.read_call_path(&root, path))
                    .collect();
                paths.sort();
                let mut key = vec![
                    "read_git_diff".to_string(),
                    checkout,
                    format!("staged:{staged}"),
                    format!("stat:{stat}"),
                ];
                match base {
                    Some(base) => key.extend(["base".to_string(), base.clone()]),
                    None => key.push("no-base".to_string()),
                }
                key.push("paths".to_string());
                key.extend(paths);
                Some(key)
            }
            ToolAction::ListWorktrees { base } => {
                let mut key = vec!["list_worktrees".to_string()];
                match base {
                    Some(base) => key.extend(["base".to_string(), base.clone()]),
                    None => key.push("no-base".to_string()),
                }
                Some(key)
            }
            ToolAction::ReadGitLog {
                worktree,
                base,
                max_count,
            } => {
                let (checkout, _) = self.read_call_checkout(repository_root, worktree.as_deref());
                let mut key = vec!["read_git_log".to_string(), checkout];
                match base {
                    Some(base) => key.extend(["base".to_string(), base.clone()]),
                    None => key.push("no-base".to_string()),
                }
                match max_count {
                    Some(count) => key.extend(["max".to_string(), count.to_string()]),
                    None => key.push("no-max".to_string()),
                }
                Some(key)
            }
            // A fetch's identity is the exact URL asked for: two pages are two
            // calls, and asking for the same page again is a repeat even when
            // the session cache serves it without a request.
            ToolAction::FetchUrl { url } => Some(vec!["fetch_url".to_string(), url.clone()]),
            // An identical append is a repeat (requirement 8), so the key
            // carries the redacted text's stable hash. A replace is never a
            // repeat: replacing the whole record is how a full one is made to
            // fit, and charging that as a repeated call would punish the only
            // way out.
            ToolAction::TurnNotes { text, replace } => (!*replace).then(|| {
                vec![
                    "turn_notes".to_string(),
                    "append".to_string(),
                    stable_text_hash(&self.scanner.redact(text).text),
                ]
            }),
            _ => None,
        }
    }

    /// A human-readable line for one dispatched call, over the same resolved
    /// identity [`Self::call_key`] computes (spec 65 §5.3). Displaying a second
    /// resolution would let the notice and the repeat rule disagree; calls
    /// without a canonical row (a command, a patch, a plan, `turn_notes`) are
    /// printed from [`tool_action_marker`] instead.
    pub(super) fn render_call_line(
        &self,
        repository_root: &Path,
        action: &ToolAction,
    ) -> Option<String> {
        if let Some(key) = self.call_key(repository_root, action)
            && let Some(line) = render_call_key(&key, repository_root)
        {
            return Some(line);
        }
        let (name, reference, _) = tool_action_marker(action);
        let reference = single_line(&reference);
        Some(if reference.is_empty() {
            name.to_string()
        } else {
            format!("{name} {reference}")
        })
    }

    /// The checkout a read call names, as spec 60's resolver canonicalizes it,
    /// together with the root its paths are resolved against. `None` is the
    /// session's checkout. A refused `worktree` is kept as given, so a model
    /// retrying the same refused argument is still caught (§5.3).
    fn read_call_checkout(
        &self,
        repository_root: &Path,
        worktree: Option<&str>,
    ) -> (String, PathBuf) {
        match worktree {
            None => (
                "checkout:session".to_string(),
                repository_root.to_path_buf(),
            ),
            Some(requested) => {
                match resolve_worktree(&self.git, &self.path_policy, repository_root, requested) {
                    Ok(worktree) => match worktree.canonical {
                        Some(canonical) => (format!("checkout:{}", canonical.display()), canonical),
                        None => (
                            format!("checkout-as-given:{requested}"),
                            repository_root.to_path_buf(),
                        ),
                    },
                    Err(_) => (
                        format!("checkout-as-given:{requested}"),
                        repository_root.to_path_buf(),
                    ),
                }
            }
        }
    }

    /// Refuses an edit whose path lies in another git worktree. Edits apply to
    /// this session's checkout only (spec 60 §4, spec 76): a path that reaches
    /// a worktree through this checkout, such as `.claude/worktrees/x/src`,
    /// would write into a checkout the user never opened, without that
    /// worktree's verification or recovery. A repository git cannot list
    /// worktrees for has none to protect.
    pub(super) fn refuse_edit_in_worktree<'a>(
        &self,
        repository_root: &Path,
        paths: impl Iterator<Item = &'a str>,
    ) -> Result<()> {
        let worktrees = match foreign_worktrees(&self.git, &self.path_policy, repository_root) {
            Ok(worktrees) if !worktrees.is_empty() => worktrees,
            _ => return Ok(()),
        };
        for path in paths {
            let Ok(target) = self.path_policy.resolve_for_write(repository_root, path) else {
                continue;
            };
            if let Some((_, display)) = worktrees
                .iter()
                .find(|(canonical, _)| target.absolute_path.starts_with(canonical))
            {
                let next = if self.config.enable_worker_sessions {
                    "To change a worktree, hand the task to a worker session in a \
                     ```worker-prompt block; it runs in a worktree of its own."
                } else {
                    "Tell the user that working in a separate worktree needs Worker \
                     sessions (Settings › General), or ask whether to make the change \
                     in this checkout."
                };
                return Err(ClientError::AccessDenied(format!(
                    "`{path}` is inside the git worktree `{display}`. Edits apply only to \
                     this session's checkout, never to another worktree. {next}"
                )));
            }
        }
        Ok(())
    }

    /// A read path as `PathPolicy` resolves it relative to its checkout. A path
    /// the policy refuses is kept as given: a retry of the same refused path is
    /// still the same call.
    fn read_call_path(&self, checkout: &Path, requested: &str) -> String {
        self.path_policy
            .resolve_existing(checkout, requested, false)
            .map(|resolved| resolved.relative_path)
            .unwrap_or_else(|_| requested.to_string())
    }

    /// Dispatch one of the read-only tools. Shared by the sequential path (a
    /// lone call in a round) and [`Self::run_read_only_batch`], so the two
    /// cannot drift. The returned triple is what the turn records.
    pub(super) fn dispatch_read_only_action(
        &self,
        repository_root: &Path,
        session: &Session,
        task: &Task,
        action: &ToolAction,
    ) -> (String, String, ActionOutcome) {
        // Spec 60 §5.3: a `worktree` argument becomes a root here, once, and
        // the same read functions then run on it under the session's policy.
        // A refusal is an ordinary failed call naming the reachable worktrees.
        let resolved = match read_tool_worktree(action) {
            None => None,
            Some(requested) => {
                match resolve_worktree(&self.git, &self.path_policy, repository_root, requested) {
                    Ok(worktree) => Some(worktree),
                    Err(error) => {
                        return (
                            format!("Attempted to read worktree `{requested}`"),
                            format!("Cannot use worktree `{requested}`: {error}"),
                            ActionOutcome::Failed,
                        );
                    }
                }
            }
        };
        let worktree = resolved.as_ref().map(|worktree| worktree.display.as_str());
        let repository_root = resolved
            .as_ref()
            .and_then(|worktree| worktree.canonical.as_deref())
            .unwrap_or(repository_root);
        let scope = in_worktree(worktree);
        // Spec 27 §5.4: the session's path restriction is part of capability,
        // not only assembly. A failure to read it fails the call rather than
        // falling back to an unrestricted read.
        let context_restriction = match self.session_store.context_restriction(&session.id) {
            Ok(restriction) => restriction,
            Err(error) => {
                return (
                    "Could not check the session's path restriction.".to_string(),
                    format!("Cannot run this read-only tool: {error}"),
                    ActionOutcome::Failed,
                );
            }
        };
        let outside_restriction = |path: Option<&str>| -> bool {
            context_restriction.as_deref().is_some_and(|paths| {
                !self
                    .context_manager
                    .restriction_admits_path(repository_root, paths, path)
            })
        };
        match action {
            ToolAction::ReadFile { path, range, .. } => {
                if outside_restriction(Some(path)) {
                    return (
                        format!("Read `{path}`{scope}"),
                        format!(
                            "Cannot read {path}{scope}: the path is outside this session's path restriction."
                        ),
                        ActionOutcome::Failed,
                    );
                }
                let window = match range {
                    Some(range) => ReadWindow::Range(*range),
                    None => ReadWindow::Default,
                };
                let (content, outcome) = match self.file_access.read_file_in(
                    repository_root,
                    worktree,
                    path,
                    Some(&task.id),
                    Some(&session.repository_id),
                    false,
                    false,
                    window,
                ) {
                    Ok(file_read) => {
                        let mut content = format!(
                            "Content of {}{scope}, lines {}–{} of {}:",
                            file_read.path,
                            file_read.line_range.start,
                            file_read.line_range.end,
                            file_read.total_lines,
                        );
                        if let Some(reason) = &file_read.truncated_by {
                            content.push_str(&format!(
                                " (truncated by {reason}; ask for a narrower range)"
                            ));
                        }
                        content.push_str(&format!("\n{}", file_read.content));
                        // Evidence names the worktree too, so a read there is
                        // never taken for a read of the same path here.
                        let evidence_path = match worktree {
                            Some(worktree) => format!("{worktree}:{}", file_read.path),
                            None => file_read.path.clone(),
                        };
                        (
                            content,
                            ActionOutcome::FileRead {
                                path: evidence_path,
                                hash: file_read.hash.clone(),
                            },
                        )
                    }
                    Err(error) => (
                        format!("Cannot read {path}{scope}: {error}"),
                        ActionOutcome::Failed,
                    ),
                };
                (format!("Read `{path}`{scope}"), content, outcome)
            }
            ToolAction::ListDirectory { dir, depth, .. } => {
                if outside_restriction(dir.as_deref()) {
                    let listed = dir.as_deref().unwrap_or(".");
                    return (
                        format!("Listed {listed}{scope}"),
                        format!(
                            "Cannot list {listed}{scope}: the path is outside this session's path restriction."
                        ),
                        ActionOutcome::Failed,
                    );
                }
                match self.navigation.list_directory_in(
                    repository_root,
                    worktree,
                    dir.as_deref(),
                    *depth,
                    Some(&task.id),
                    Some(&session.repository_id),
                ) {
                    Ok(listing) => {
                        let dir_name = dir.as_deref().unwrap_or(".");
                        let content = if listing.truncated {
                            format!(
                                "Listing of {dir_name}{scope} ({} of {} paths):\n{}",
                                listing.paths.len(),
                                listing.total_found,
                                listing.paths.join("\n")
                            )
                        } else {
                            format!(
                                "Listing of {dir_name}{scope} ({} paths):\n{}",
                                listing.total_found,
                                listing.paths.join("\n")
                            )
                        };
                        (
                            format!("Listed {dir_name}{scope}"),
                            content,
                            ActionOutcome::Ok,
                        )
                    }
                    Err(error) => (
                        "Attempted to list the directory.".to_string(),
                        format!("Cannot list that directory: {error}"),
                        ActionOutcome::Failed,
                    ),
                }
            }
            ToolAction::SearchContent {
                pattern,
                path_glob,
                max_matches,
                ..
            } => {
                // A literal glob is a path request, so it can be refused
                // outright. A pattern with wildcards is filtered while walking,
                // below, because no single path can answer it.
                // The glob is part of what was searched, so the session log
                // names it: without it, "0 matches" cannot be told apart from
                // a search that covered nothing.
                let in_glob = path_glob
                    .as_deref()
                    .map(|glob| format!(" in `{glob}`"))
                    .unwrap_or_default();
                if let Some(glob) = path_glob
                    && !glob.contains(['*', '?', '[', '{'])
                    && outside_restriction(Some(glob))
                {
                    return (
                        format!("Searched for \"{pattern}\"{in_glob}{scope}"),
                        format!(
                            "Cannot search {glob}{scope}: the path is outside this session's path restriction."
                        ),
                        ActionOutcome::Failed,
                    );
                }
                let allowed = |path: &str| !outside_restriction(Some(path));
                match self.navigation.search_content_in(
                    repository_root,
                    worktree,
                    pattern,
                    path_glob.as_deref(),
                    *max_matches,
                    Some(&task.id),
                    Some(&session.repository_id),
                    Some(&allowed),
                ) {
                    Ok(found) => {
                        let mut lines = found
                            .matches
                            .iter()
                            .map(|m| format!("{}:{}: {}", m.path, m.line, m.text))
                            .collect::<Vec<_>>()
                            .join("\n");
                        if !lines.is_empty() {
                            lines.push('\n');
                        }
                        let note = if context_restriction.is_some() {
                            " (limited to this session's path restriction)"
                        } else {
                            ""
                        };
                        // A search that read no file found nothing because it
                        // looked nowhere. Saying "0 matches" would read as
                        // evidence of absence, so it says what happened instead.
                        let content = if found.files_searched == 0 {
                            match path_glob {
                                Some(glob) => format!(
                                    "No file was searched for \"{pattern}\"{scope}{note}: the glob \
                                     `{glob}` matched no readable file. Only `*` (within one \
                                     directory), `**` (across directories) and `?` are \
                                     wildcards, and `{{a,b}}` matches any one of its \
                                     comma-separated alternatives; `[...]` is matched literally. \
                                     This is not evidence that the text is absent: search again \
                                     with one glob per directory, or without a glob.\n"
                                ),
                                None if context_restriction.is_some() => format!(
                                    "No file was searched for \"{pattern}\"{scope}: this \
                                     session's path restriction admits no file to search. \
                                     This is not evidence that the text is absent.\n"
                                ),
                                None => format!(
                                    "No file was searched for \"{pattern}\"{scope}: there is \
                                     no readable text file to search. This is not evidence \
                                     that the text is absent.\n"
                                ),
                            }
                        } else {
                            format!(
                                "{} matches for \"{}\" across {} files{scope}{note} (showing {}):\n{}",
                                found.total_found,
                                pattern,
                                found.files_searched,
                                found.matches.len(),
                                lines,
                            )
                        };
                        (
                            format!("Searched for \"{pattern}\"{in_glob}{scope}"),
                            content,
                            ActionOutcome::Ok,
                        )
                    }
                    Err(error) => (
                        "Attempted to search the repository.".to_string(),
                        format!("Cannot search: {error}"),
                        ActionOutcome::Failed,
                    ),
                }
            }
            ToolAction::SearchCodebase {
                query,
                semantic,
                limit,
            } => {
                let index = match crate::index_cache::IndexCache::get_or_build(
                    &self.indexer,
                    repository_root,
                ) {
                    Ok(index) => index,
                    Err(error) => {
                        return (
                            format!("Searched codebase for \"{query}\""),
                            format!("Cannot search the codebase: {error}"),
                            ActionOutcome::Failed,
                        );
                    }
                };
                let allowed = |path: &str| !outside_restriction(Some(path));
                let results = if *semantic {
                    if self.config.enable_semantic_search {
                        VectorIndexCache::semantic_search_allowed(
                            &self.config.data_dir,
                            &index,
                            query,
                            *limit,
                            crate::vector_index::semantic_search_max_chunks(&self.config),
                            &allowed,
                        )
                    } else {
                        index.semantic_search_allowed(query, *limit, &allowed)
                    }
                } else {
                    index.keyword_search_allowed(query, *limit, &allowed)
                };
                let note = if context_restriction.is_some() {
                    "\n\n(Results are limited to this session's path restriction.)"
                } else {
                    ""
                };
                (
                    format!("Searched codebase for \"{query}\""),
                    format!("{}{note}", format_search_results(&results)),
                    // A search that matched nothing still ran. "No results"
                    // is an answer, not a failure.
                    ActionOutcome::Ok,
                )
            }
            ToolAction::ReadGitStatus { .. } => {
                let (content, outcome) = match self.git.status_in(repository_root, worktree) {
                    Ok(status) if worktree.is_some() => (
                        format!("Git status{scope}:\n{}", format_git_status(&status)),
                        ActionOutcome::Ok,
                    ),
                    Ok(status) => (format_git_status(&status), ActionOutcome::Ok),
                    Err(error) => (
                        format!("Cannot read git status: {error}"),
                        ActionOutcome::Failed,
                    ),
                };
                (format!("Checked git status{scope}"), content, outcome)
            }
            ToolAction::ReadGitDiff {
                staged,
                paths,
                stat,
                base,
                ..
            } => {
                // Spec 27 §5.4: a diff is file content, so it is bounded by the
                // session restriction like `read_file`. Refusing is clearer
                // than silently dropping hunks: the model can name an admitted
                // path instead of believing it saw every change.
                if context_restriction.is_some() {
                    if paths.is_empty() {
                        return (
                            format!(
                                "Read git diff{}{scope}",
                                if *staged { " (staged)" } else { "" }
                            ),
                            "Cannot read the whole repository's diff: this session is restricted \
                             to selected paths. Name paths inside the restriction."
                                .to_string(),
                            ActionOutcome::Failed,
                        );
                    }
                    if let Some(path) = paths
                        .iter()
                        .find(|path| outside_restriction(Some(path.as_str())))
                    {
                        return (
                            format!(
                                "Read git diff{}{scope}",
                                if *staged { " (staged)" } else { "" }
                            ),
                            format!(
                                "Cannot read git diff for {path}{scope}: the path is outside this session's path restriction."
                            ),
                            ActionOutcome::Failed,
                        );
                    }
                }
                let options = DiffOptions {
                    staged: *staged,
                    paths: paths.clone(),
                    stat: *stat,
                    base: base.clone(),
                };
                let (content, outcome) = match self.git.diff_in(repository_root, worktree, &options)
                {
                    Ok(diff) if diff.trim().is_empty() => {
                        (format!("No differences{scope}."), ActionOutcome::Ok)
                    }
                    Ok(diff) => (diff, ActionOutcome::Ok),
                    Err(error) => (
                        format!("Cannot read git diff: {error}"),
                        ActionOutcome::Failed,
                    ),
                };
                (
                    format!(
                        "Read git diff{}{scope}",
                        if *staged { " (staged)" } else { "" }
                    ),
                    content,
                    outcome,
                )
            }
            ToolAction::ListWorktrees { base } => {
                let (content, outcome) =
                    match self.git.list_worktrees(repository_root, base.as_deref()) {
                        Ok(listing) => (format_worktree_listing(&listing), ActionOutcome::Ok),
                        Err(error) => (
                            format!("Cannot list worktrees: {error}"),
                            ActionOutcome::Failed,
                        ),
                    };
                ("Listed worktrees".to_string(), content, outcome)
            }
            ToolAction::ReadGitLog {
                base, max_count, ..
            } => {
                let (content, outcome) =
                    match self
                        .git
                        .log_in(repository_root, worktree, base.as_deref(), *max_count)
                    {
                        Ok(log) if log.trim().is_empty() => {
                            (format!("No commits{scope}."), ActionOutcome::Ok)
                        }
                        Ok(log) => (log, ActionOutcome::Ok),
                        Err(error) => (
                            format!("Cannot read git log: {error}"),
                            ActionOutcome::Failed,
                        ),
                    };
                (format!("Read git log{scope}"), content, outcome)
            }
            other => unreachable!("not a read-only action: {other:?}"),
        }
    }

    /// Run an all-read-only round's calls concurrently, collecting results in
    /// the model's call order. `thread::scope` plus joining in order is the
    /// determinism: whichever thread finishes first, result `i` is the one the
    /// model asked for at position `i`, so the recorded log is identical to the
    /// sequential one (`context.md` §3 on why this is the primitive).
    pub(super) fn run_read_only_batch(
        &self,
        repository_root: &Path,
        session: &Session,
        task: &Task,
        actions: &[DecodedCall],
        cancel: &CancelToken,
    ) -> Vec<(String, String, ActionOutcome)> {
        std::thread::scope(|scope| {
            let handles: Vec<_> = actions
                .iter()
                .map(|(_, action)| {
                    scope.spawn(move || {
                        // Requirement 8: a stop cancels every call that has not
                        // started. A read already in flight is not interrupted
                        // — it is a bounded local read and takes no cancel token
                        // — but no call begins after this check.
                        if cancel.is_cancelled() {
                            return (String::new(), String::new(), ActionOutcome::Cancelled);
                        }
                        self.dispatch_read_only_action(repository_root, session, task, action)
                    })
                })
                .collect();
            handles
                .into_iter()
                .map(|handle| handle.join().expect("read-only tool thread panicked"))
                .collect()
        })
    }
}

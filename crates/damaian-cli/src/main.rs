use std::env;
use std::io::IsTerminal;
use std::path::Path;
use workspace_engine::hooks::{HookReviewDecision, list_hooks, review_hook, test_run};
use workspace_engine::{
    CURRENT_DATA_SCHEMA_VERSION, CancelToken, CommandProposal, CommandRisk, Config, ConfigOverlay,
    ConfigScope, CurlModelTransport, DataSchemaOutcome, EffectivePolicy, ExclusionReason,
    HookEvent, MapLoad, MockModelAdapter, ModelAdapter, OpenAICompatibleAdapter, ProcessRegistry,
    ProfileId, ReadWindow, RebuildReason, RelationTarget, RepositoryMap, RootEvidence,
    RootOverride, RootOverrideEdit, SearchResult, SymbolLoad, SymbolMatch, SymbolRef,
    SymbolReferences, TurnProgress, TurnSink, WorkspaceEngine, command_approval_prompt,
    edit_root_overrides, ensure_data_dir_schema, export_profile, hook_definition_hash,
    import_profile, parse_hunk_selection, patch_diff_text, patch_hunk_summary, profile_import_base,
    render_markdown_to_ansi, review_profile_import, review_profile_rejections, select_profile,
    symbol_matches_json, symbol_references_json,
};

fn usage() -> &'static str {
    "Usage:
  damaian [--no-color] <command> [args...]

  damaian index <repo>
  damaian search <repo> <query>
  damaian read <repo> <path>
  damaian git-status <repo>
  damaian git-diff <repo>
  damaian detect-commands <repo>
  damaian repo-map <repo> [--json]
  damaian symbols <repo> <query> [--json]
  damaian symbol-references <repo> <path>:<line> <name> [--json]
  damaian classify-command <command>
  damaian config-show [--sources] [repo]
  damaian config-review <repo>
  damaian config-allowlist-keep <repo> [command...]
  damaian config-set user <key> <value>
  damaian config-set repo <repo> <key> <value>
  damaian config-set admin <key> <value>
  damaian hooks <repo>
  damaian hook-review <repo> <id> approve|decline
  damaian hook-run <repo> <id> <event>
  damaian profile-set <repo> <read_only|safe_local|full|offline_private|custom-name>
  damaian profile-export <profile> [file]
  damaian profile-import <custom-name> <file> [--repo <repo>] [--replace] [--review]
  damaian repo-root <repo> add|remove|clear <path>
  damaian propose-command <repo> <command>
  damaian propose-validations <repo>     (each root's commands, run in that root)
  damaian run-command <proposal-id> --approve [--always]
  damaian reject-command <proposal-id>
  damaian ask <repo> <prompt>
  damaian verify <repo> <session> [--decline-checks]
  damaian propose-edit <repo> <prompt>
  damaian show-patch <repo> <patch-id>
  damaian apply-patch <repo> <patch-id> [file...] [--hunk-selection <json>]
                      [--allow-generated-secrets]
  damaian reject-patch <patch-id>
"
}

fn main() {
    // Before any command touches the data directory: a directory written by a
    // layout this build does not understand is refused, not written over.
    if let Err(error) = verify_data_dir_schema() {
        eprintln!("{error}");
        std::process::exit(1);
    }
    if let Err(error) = start_process_cleanup() {
        eprintln!("{error}");
        std::process::exit(1);
    }
    if let Err(error) = run() {
        eprintln!("{error}");
        std::process::exit(1);
    }
}

fn verify_data_dir_schema() -> std::result::Result<(), String> {
    let config = Config::load_for_repository(None).map_err(|error| error.to_string())?;
    match ensure_data_dir_schema(&config.data_dir).map_err(|error| error.to_string())? {
        DataSchemaOutcome::Adopted => eprintln!(
            "Data directory {} adopted at schema version {CURRENT_DATA_SCHEMA_VERSION}",
            config.data_dir.display()
        ),
        DataSchemaOutcome::Upgraded { from } => eprintln!(
            "Data directory {} migrated from schema version {from} to {CURRENT_DATA_SCHEMA_VERSION}",
            config.data_dir.display()
        ),
        DataSchemaOutcome::Initialized | DataSchemaOutcome::Current => {}
    }
    Ok(())
}

/// Sweeps what a crashed instance left running and arms the shutdown handler,
/// per `docs/specs/46_process_registry_and_orphan_sweep/proposal.md` §5.6–§5.7.
///
/// The CLI spawns `curl` and MCP stdio servers of its own and may be the only
/// front end a user ever runs, so it cannot rely on the desktop shell having
/// swept. Running both is harmless: an entry whose owner is still alive is
/// skipped, so the second sweep of a launch is a no-op.
fn start_process_cleanup() -> std::result::Result<(), String> {
    let config = Config::load_for_repository(None).map_err(|error| error.to_string())?;
    let (registry, audit) =
        ProcessRegistry::open_with_audit(&config).map_err(|error| error.to_string())?;
    let report = registry.sweep(&audit).map_err(|error| error.to_string())?;
    if report.killed() > 0 || report.refused() > 0 {
        eprintln!(
            "Orphan sweep: killed {}, refused {} on a start-time mismatch",
            report.killed(),
            report.refused()
        );
    }
    registry
        .install_shutdown_handler(audit)
        .map_err(|error| error.to_string())
}

fn run() -> workspace_engine::Result<()> {
    let mut args = env::args().skip(1).collect::<Vec<_>>();
    let no_color_flag = take_flag(&mut args, "--no-color");
    // Global so positional parsing below never sees the flag or its value;
    // only `apply-patch` reads it.
    let hunk_selection_arg = take_option(&mut args, "--hunk-selection");
    // Also global for the same reason: stripped before positional parsing so
    // it can sit anywhere among `apply-patch`'s optional `[file...]` args.
    let allow_generated_secrets = take_flag(&mut args, "--allow-generated-secrets");
    let Some(command) = args.first().map(String::as_str) else {
        print!("{}", usage());
        return Ok(());
    };
    if command == "--help" || command == "-h" {
        print!("{}", usage());
        return Ok(());
    }

    match command {
        "index" => {
            let repo = require_arg(&args, 1, "<repo>")?;
            let engine = engine_for_repo(repo)?;
            let index = engine.indexer.index_repository(repo)?;
            println!(
                "{{\"repositoryId\":\"{}\",\"rootPath\":\"{}\",\"fileCount\":{},\"skippedCount\":{}}}",
                escape(&index.repository_id),
                escape(&index.root_path.to_string_lossy()),
                index.files.len(),
                index.skipped.len()
            );
        }
        "search" => {
            let repo = require_arg(&args, 1, "<repo>")?;
            let engine = engine_for_repo(repo)?;
            if args.len() < 3 {
                return Err(workspace_engine::ClientError::InvalidInput(
                    "Missing <query>".to_string(),
                ));
            }
            let query = args[2..].join(" ");
            let index = engine.indexer.index_repository(repo)?;
            let results = index.keyword_search(&query, 10);
            println!("{}", search_results_json(&results));
        }
        "read" => {
            let repo = require_arg(&args, 1, "<repo>")?;
            let engine = engine_for_repo(repo)?;
            let path = require_arg(&args, 2, "<path>")?;
            let file = engine.file_access.read_file(
                repo,
                path,
                Some("cli"),
                Some(repo),
                false,
                false,
                ReadWindow::Whole,
            )?;
            print!("{}", file.content);
            if !file.content.ends_with('\n') {
                println!();
            }
        }
        "git-status" => {
            let repo = require_arg(&args, 1, "<repo>")?;
            let engine = engine_for_repo(repo)?;
            let status = engine.git.status(repo)?;
            println!(
                "{{\"clean\":{},\"exitCode\":{},\"fileCount\":{}}}",
                status.clean,
                status.exit_code,
                status.files.len()
            );
        }
        "git-diff" => {
            let repo = require_arg(&args, 1, "<repo>")?;
            let engine = engine_for_repo(repo)?;
            print!("{}", engine.git.diff(repo, false)?);
        }
        "detect-commands" => {
            let repo = require_arg(&args, 1, "<repo>")?;
            let engine = engine_for_repo(repo)?;
            let commands = engine
                .command_policy
                .detect_project_commands(Path::new(repo))?;
            let body = commands
                .iter()
                .map(|command| {
                    format!(
                        "{{\"name\":\"{}\",\"command\":\"{}\",\"risk\":\"{}\"}}",
                        escape(&command.name),
                        escape(&command.command),
                        risk_json(&command.risk)
                    )
                })
                .collect::<Vec<_>>()
                .join(",");
            println!("[{body}]");
        }
        "classify-command" => {
            let engine = default_engine()?;
            if args.len() < 2 {
                return Err(workspace_engine::ClientError::InvalidInput(
                    "Missing <command>".to_string(),
                ));
            }
            let cwd = std::env::current_dir()?;
            let classification = engine.command_policy.classify(&args[1..].join(" "), &cwd);
            println!(
                "{{\"command\":\"{}\",\"risk\":\"{}\",\"blocked\":{},\"requiresApproval\":{},\"mayUseNetwork\":{}}}",
                escape(&classification.command),
                risk_json(&classification.risk),
                classification.blocked,
                classification.requires_approval,
                classification.may_use_network
            );
        }
        "config-show" => {
            // `--sources` attributes every rule (spec 31, proposal §5.7). The
            // CLI has no session, so the header names the profile alone.
            let sources = args.get(1).is_some_and(|arg| arg == "--sources");
            let repo = args.get(if sources { 2 } else { 1 }).map(Path::new);
            if sources {
                print!("{}", EffectivePolicy::resolve(repo, None)?.to_text());
            } else {
                print!("{}", Config::load_for_repository(repo)?.to_policy_text());
            }
        }
        "config-review" => {
            let repo = require_arg(&args, 1, "<repo>")?;
            print!("{}", repository_config_review(repo)?);
        }
        "profile-set" => {
            let repo = require_arg(&args, 1, "<repo>")?;
            let profile = require_arg(&args, 2, "<profile>")?;
            print!("{}", set_permission_profile(repo, profile)?);
        }
        "profile-export" => {
            let profile = require_arg(&args, 1, "<profile>")?;
            let config = Config::load_for_repository(None)?;
            let text = export_profile(&ProfileId::parse(profile)?, &config.data_dir)?;
            match args.get(2) {
                Some(file) => {
                    std::fs::write(file, &text)?;
                    println!("exported permission profile {profile} to {file}");
                }
                None => print!("{text}"),
            }
        }
        "profile-import" => {
            print!("{}", import_permission_profile(&args[1..])?);
        }
        "config-allowlist-keep" => {
            let repo = require_arg(&args, 1, "<repo>")?;
            print!("{}", keep_repository_allowlist(repo, &args[2..])?);
        }
        "hooks" => {
            let repo = require_arg(&args, 1, "<repo>")?;
            print!("{}", hooks_listing(repo)?);
        }
        "hook-review" => {
            let repo = require_arg(&args, 1, "<repo>")?;
            let id = require_arg(&args, 2, "<id>")?;
            let decision = require_arg(&args, 3, "approve|decline")?;
            print!("{}", review_hook_command(repo, id, decision)?);
        }
        "hook-run" => {
            let repo = require_arg(&args, 1, "<repo>")?;
            let id = require_arg(&args, 2, "<id>")?;
            let event = require_arg(&args, 3, "<event>")?;
            print!("{}", run_hook_command(repo, id, event)?);
        }
        "config-set" => {
            set_config_value(&args)?;
        }
        "repo-root" => {
            print!("{}", edit_repository_roots(&args)?);
        }
        "propose-command" => {
            let repo = require_arg(&args, 1, "<repo>")?;
            let engine = engine_for_repo(repo)?;
            if args.len() < 3 {
                return Err(workspace_engine::ClientError::InvalidInput(
                    "Missing <command>".to_string(),
                ));
            }
            let command = args[2..].join(" ");
            let proposal = engine.validation_orchestrator.propose_command(
                repo,
                &command,
                "User requested command proposal",
            )?;
            print!("{}", command_approval_prompt(&proposal));
            println!("{}", command_proposal_json(&proposal));
        }
        "repo-map" => {
            let repo = require_arg(&args, 1, "<repo>")?;
            let json = args.iter().skip(2).any(|arg| arg == "--json");
            let engine = engine_for_repo(repo)?;
            let (map, load) = engine.repository_map(repo)?;
            if json {
                println!(
                    "{{\"load\":{},\"map\":{}}}",
                    map_load_json(&load),
                    map.to_json()
                );
            } else {
                print!("{}", repository_map_summary(&map, &load));
            }
        }
        "symbols" => {
            let repo = require_arg(&args, 1, "<repo>")?;
            let json = args.iter().skip(2).any(|arg| arg == "--json");
            let query = args
                .iter()
                .skip(2)
                .find(|arg| *arg != "--json")
                .ok_or_else(|| {
                    workspace_engine::ClientError::InvalidInput("Missing <query>".to_string())
                })?;
            let engine = engine_for_repo(repo)?;
            let (index, load) = engine.symbol_index(repo)?;
            let matches = index.find(query, SYMBOL_LIMIT);
            if json {
                println!(
                    "{{\"load\":{},\"matches\":{}}}",
                    symbol_load_json(&load),
                    symbol_matches_json(&matches)
                );
            } else {
                print!("{}", symbol_matches_text(&matches, &load));
            }
        }
        "symbol-references" => {
            let repo = require_arg(&args, 1, "<repo>")?;
            let location = require_arg(&args, 2, "<path>:<line>")?;
            let name = require_arg(&args, 3, "<name>")?;
            let json = args.iter().skip(4).any(|arg| arg == "--json");
            let (path, line) = parse_symbol_location(location)?;
            let symbol = SymbolRef {
                path,
                name: name.to_string(),
                line,
            };
            let engine = engine_for_repo(repo)?;
            let result = engine.symbol_references(repo, &symbol)?;
            if json {
                println!("{}", symbol_references_json(&result));
            } else {
                print!("{}", symbol_references_text(&result));
            }
        }
        "propose-validations" => {
            let repo = require_arg(&args, 1, "<repo>")?;
            let engine = engine_for_repo(repo)?;
            let (map, _) = engine.repository_map(repo)?;
            let proposals = engine
                .validation_orchestrator
                .propose_detected_validations(repo, &map)?;
            println!(
                "[{}]",
                proposals
                    .iter()
                    .map(command_proposal_json)
                    .collect::<Vec<_>>()
                    .join(",")
            );
        }
        "run-command" => {
            let engine = default_engine()?;
            let proposal_id = require_arg(&args, 1, "<proposal-id>")?;
            let approved = args.iter().any(|arg| arg == "--approve");
            let always = args.iter().any(|arg| arg == "--always");
            if always && !approved {
                return Err(workspace_engine::ClientError::InvalidInput(
                    "--always requires --approve".to_string(),
                ));
            }
            // Persisted before the run: if the allowlist write fails, the
            // command must not run either, so the user isn't left thinking a
            // permanent allowance was granted when it wasn't.
            //
            // `default_engine` has no repository overlay applied, so its
            // approval settings can differ from the ones that classified this
            // proposal. Re-scope to the proposal's own repository first, or
            // the eligibility check would be answered against the wrong
            // policy.
            let allowlist_path = if always {
                let proposal = engine.validation_orchestrator.load_proposal(proposal_id)?;
                let repo_engine = engine_for_repo(&proposal.working_directory)?;
                Some(
                    repo_engine
                        .validation_orchestrator
                        .allow_command_always(proposal_id, "local_user")?,
                )
            } else {
                None
            };
            let record = {
                // The CLI's command path is not a turn: nothing can stop it and
                // there is no progress stream to feed.
                let cancel = CancelToken::new();
                let mut on_output = |_line: &str| {};
                engine.validation_orchestrator.run_proposal(
                    proposal_id,
                    approved,
                    "local_user",
                    None,
                    &cancel,
                    &mut on_output,
                )?
            };
            println!(
                "{{\"proposalId\":\"{}\",\"commandId\":\"{}\",\"exitCode\":{},\"stdoutRef\":\"{}\",\"stderrRef\":\"{}\",\"allowlistPath\":{}}}",
                escape(&record.proposal_id),
                escape(&record.execution.id),
                record.execution.exit_code.unwrap_or(-1),
                escape(&record.stdout_ref.to_string_lossy()),
                escape(&record.stderr_ref.to_string_lossy()),
                match &allowlist_path {
                    Some(path) => format!("\"{}\"", escape(&path.to_string_lossy())),
                    None => "null".to_string(),
                }
            );
        }
        "reject-command" => {
            let engine = default_engine()?;
            let proposal_id = require_arg(&args, 1, "<proposal-id>")?;
            let path = engine
                .validation_orchestrator
                .reject_proposal(proposal_id, "local_user")?;
            println!(
                "{{\"proposalId\":\"{}\",\"status\":\"rejected\",\"path\":\"{}\"}}",
                escape(proposal_id),
                escape(&path.to_string_lossy())
            );
        }
        "ask" => {
            let repo = require_arg(&args, 1, "<repo>")?;
            let engine = engine_for_repo(repo)?;
            if args.len() < 3 {
                return Err(workspace_engine::ClientError::InvalidInput(
                    "Missing <prompt>".to_string(),
                ));
            }
            let prompt = args[2..].join(" ");
            // Formatting (colors, syntax highlighting) requires the whole
            // response, so tokens are collected silently instead of printed
            // live; the rendered text is printed once streaming completes.
            let mut on_token = |_token: &str| {};
            let result = if let Ok(mock_response) = env::var("DAMAIAN_MOCK_MODEL_RESPONSE") {
                let mut adapter = MockModelAdapter::new(mock_response);
                engine
                    .chat_orchestrator
                    .ask(repo, &prompt, &[], &mut adapter, &mut on_token)?
            } else {
                let api_key = live_model_api_key(&engine.config)?;
                let transport = CurlModelTransport::new(
                    &engine.config.model_base_url,
                    api_key,
                    ProcessRegistry::open(&engine.config.data_dir)?,
                    &engine.config.data_dir,
                );
                let mut adapter = OpenAICompatibleAdapter::with_provider(
                    &engine.config.model_provider,
                    &engine.config.model_name,
                    transport,
                );
                engine
                    .chat_orchestrator
                    .ask(repo, &prompt, &[], &mut adapter, &mut on_token)?
            };
            if use_color(no_color_flag) {
                print!("{}", render_markdown_to_ansi(&result.response));
            } else {
                print!("{}", result.response);
            }
            if !result.response.ends_with('\n') {
                println!();
            }
            eprintln!(
                "context_files={}",
                result
                    .context_files
                    .iter()
                    .map(|path| escape(path))
                    .collect::<Vec<_>>()
                    .join(",")
            );
        }
        "verify" => {
            let repo = require_arg(&args, 1, "<repo>")?;
            let session_id = require_arg(&args, 2, "<session>")?;
            let decline_all = args[3..].iter().any(|arg| arg == "--decline-checks");
            let engine = engine_for_repo(repo)?;
            let mut adapter = cli_model_adapter(&engine)?;
            let cancel = CancelToken::new();
            let mut on_token = |_token: &str| {};
            let mut on_progress = |_progress: TurnProgress| {};
            let mut sink = TurnSink {
                on_token: &mut on_token,
                on_progress: &mut on_progress,
                cancel: &cancel,
                steering: None,
            };
            let mut result = engine.chat_orchestrator.verify_due(
                repo,
                session_id,
                adapter.as_mut(),
                &mut sink,
            )?;
            // One check card at a time. The engine refuses a missing decision and
            // puts the pause back, so a mistyped answer cannot half-run a batch.
            while let Some(batch) = result.check_batch.clone() {
                let mut decisions = Vec::new();
                for check in &batch.checks {
                    let approved = if decline_all {
                        false
                    } else {
                        confirm_check(check)?
                    };
                    decisions.push((check.id.clone(), approved));
                }
                result = engine.chat_orchestrator.resume_after_check_decision(
                    &batch.id,
                    &decisions,
                    "local_user",
                    adapter.as_mut(),
                    &mut sink,
                )?;
            }
            if use_color(no_color_flag) {
                print!("{}", render_markdown_to_ansi(&result.response));
            } else {
                print!("{}", result.response);
            }
            if !result.response.ends_with('\n') {
                println!();
            }
            if let Some(patch) = &result.patch_proposal {
                eprintln!("repair_patch={}", escape(&patch.patch_id));
            }
        }
        "propose-edit" => {
            let repo = require_arg(&args, 1, "<repo>")?;
            let engine = engine_for_repo(repo)?;
            if args.len() < 3 {
                return Err(workspace_engine::ClientError::InvalidInput(
                    "Missing <prompt>".to_string(),
                ));
            }
            let prompt = args[2..].join(" ");
            let result = if let Ok(mock_response) = env::var("DAMAIAN_MOCK_MODEL_RESPONSE") {
                let mut adapter = MockModelAdapter::new(mock_response);
                engine
                    .edit_orchestrator
                    .propose_edit(repo, &prompt, &[], None, &mut adapter)?
            } else {
                let api_key = live_model_api_key(&engine.config)?;
                let transport = CurlModelTransport::new(
                    &engine.config.model_base_url,
                    api_key,
                    ProcessRegistry::open(&engine.config.data_dir)?,
                    &engine.config.data_dir,
                );
                let mut adapter = OpenAICompatibleAdapter::with_provider(
                    &engine.config.model_provider,
                    &engine.config.model_name,
                    transport,
                );
                engine
                    .edit_orchestrator
                    .propose_edit(repo, &prompt, &[], None, &mut adapter)?
            };
            print!("{}", patch_diff_text(&result.patch));
            eprintln!("patch_id={}", result.patch.id);
            eprintln!(
                "context_files={}",
                result
                    .context_files
                    .iter()
                    .map(|path| escape(path))
                    .collect::<Vec<_>>()
                    .join(",")
            );
        }
        "show-patch" => {
            let repo = require_arg(&args, 1, "<repo>")?;
            let engine = engine_for_repo(repo)?;
            let patch_id = require_arg(&args, 2, "<patch-id>")?;
            let patch = engine.patch_store.load(patch_id)?;
            print!("{}", patch_diff_text(&patch));
            print!("{}", patch_hunk_summary(&patch));
        }
        "apply-patch" => {
            let repo = require_arg(&args, 1, "<repo>")?;
            let engine = engine_for_repo(repo)?;
            let patch_id = require_arg(&args, 2, "<patch-id>")?;
            let approved_paths = if args.len() > 3 {
                Some(args[3..].to_vec())
            } else {
                None
            };
            let hunk_selection = hunk_selection_arg
                .as_deref()
                .map(parse_hunk_selection)
                .transpose()?;
            // Show what the scanner found before applying, so the operator can
            // see which file tripped the check and re-run with the override
            // instead of only being told "blocked".
            if !allow_generated_secrets {
                let flagged = engine.edit_orchestrator.preview_stored_patch_secrets(
                    repo,
                    patch_id,
                    approved_paths.as_deref(),
                    hunk_selection.as_ref(),
                )?;
                for warning in &flagged {
                    eprintln!(
                        "warning: {} may contain a hardcoded secret ({} finding(s): {})",
                        warning.path,
                        warning.count,
                        warning.categories.join(", ")
                    );
                }
                if !flagged.is_empty() {
                    eprintln!(
                        "Re-run with --allow-generated-secrets to apply anyway after reviewing the diff."
                    );
                }
            }
            let result = engine.edit_orchestrator.apply_stored_patch(
                repo,
                patch_id,
                approved_paths.as_deref(),
                hunk_selection.as_ref(),
                "local_user",
                allow_generated_secrets,
            )?;
            println!(
                "{{\"patchId\":\"{}\",\"appliedFiles\":[{}],\"warningCount\":{}}}",
                escape(&result.patch_id),
                result
                    .applied_files
                    .iter()
                    .map(|path| format!("\"{}\"", escape(path)))
                    .collect::<Vec<_>>()
                    .join(","),
                result.warnings.len()
            );
        }
        "reject-patch" => {
            let engine = default_engine()?;
            let patch_id = require_arg(&args, 1, "<patch-id>")?;
            let path = engine
                .edit_orchestrator
                .reject_stored_patch(patch_id, "local_user")?;
            println!(
                "{{\"patchId\":\"{}\",\"status\":\"rejected\",\"path\":\"{}\"}}",
                escape(patch_id),
                escape(&path.to_string_lossy())
            );
        }
        _ => {
            return Err(workspace_engine::ClientError::InvalidInput(format!(
                "Unknown command: {command}\n\n{}",
                usage()
            )));
        }
    }
    Ok(())
}

fn require_arg<'a>(
    args: &'a [String],
    index: usize,
    name: &str,
) -> workspace_engine::Result<&'a str> {
    args.get(index)
        .map(String::as_str)
        .ok_or_else(|| workspace_engine::ClientError::InvalidInput(format!("Missing {name}")))
}

/// Removes `flag` from `args` if present, returning whether it was found.
/// Used for global flags (e.g. `--no-color`) that can appear anywhere and
/// shouldn't be treated as a positional argument by subcommands.
fn take_flag(args: &mut Vec<String>, flag: &str) -> bool {
    match args.iter().position(|arg| arg == flag) {
        Some(index) => {
            args.remove(index);
            true
        }
        None => false,
    }
}

/// Removes `flag` and its following value from `args` if present, returning
/// the value. Like [`take_flag`] but for `--name <value>` options, so
/// positional-argument parsing never sees either token.
fn take_option(args: &mut Vec<String>, flag: &str) -> Option<String> {
    let index = args.iter().position(|arg| arg == flag)?;
    if index + 1 < args.len() {
        let value = args.remove(index + 1);
        args.remove(index);
        Some(value)
    } else {
        // Flag with no value: drop it and behave as if it wasn't passed.
        args.remove(index);
        None
    }
}

/// Whether ANSI-formatted output should be printed: respects an explicit
/// `--no-color` flag and the `NO_COLOR` convention (https://no-color.org/),
/// and otherwise only colors output when stdout is an interactive terminal
/// so piped/redirected output stays plain text.
fn use_color(no_color_flag: bool) -> bool {
    !no_color_flag && env::var_os("NO_COLOR").is_none() && std::io::stdout().is_terminal()
}

fn default_engine() -> workspace_engine::Result<WorkspaceEngine> {
    let (config, report) = Config::load_for_repository_reporting(None)?;
    Ok(WorkspaceEngine::new_with_report(config, &report))
}

fn engine_for_repo(repo: &str) -> workspace_engine::Result<WorkspaceEngine> {
    let (config, report) = Config::load_for_repository_reporting(Some(Path::new(repo)))?;
    Ok(WorkspaceEngine::new_with_report(config, &report))
}

/// The key for a live model call. A loopback endpoint such as local Ollama
/// serves without credentials, so a missing reference becomes an empty key
/// and the transport sends no `Authorization` header. A remote endpoint still
/// fails loudly rather than sending an unauthenticated request.
/// A model adapter for a CLI command, mocked when `DAMAIAN_MOCK_MODEL_RESPONSE`
/// is set — the same convention every model-dependent CLI path uses.
fn cli_model_adapter(engine: &WorkspaceEngine) -> workspace_engine::Result<Box<dyn ModelAdapter>> {
    if let Ok(mock_response) = env::var("DAMAIAN_MOCK_MODEL_RESPONSE") {
        return Ok(Box::new(MockModelAdapter::new(mock_response)));
    }
    let api_key = live_model_api_key(&engine.config)?;
    let transport = CurlModelTransport::new(
        &engine.config.model_base_url,
        api_key,
        ProcessRegistry::open(&engine.config.data_dir)?,
        &engine.config.data_dir,
    );
    Ok(Box::new(OpenAICompatibleAdapter::with_provider(
        &engine.config.model_provider,
        &engine.config.model_name,
        transport,
    )))
}

/// Asks whether one check may run. Declining is the default and the safe
/// answer: a pipe or a non-terminal stdin declines every check rather than
/// running one nobody approved.
fn confirm_check(check: &workspace_engine::AgentCommandProposal) -> workspace_engine::Result<bool> {
    if !std::io::stdin().is_terminal() {
        return Ok(false);
    }
    eprint!("Run `{}` (risk: {})? [y/N] ", check.command, check.risk);
    std::io::Write::flush(&mut std::io::stderr()).ok();
    let mut line = String::new();
    std::io::stdin()
        .read_line(&mut line)
        .map_err(|error| workspace_engine::ClientError::Io(error.to_string()))?;
    Ok(matches!(
        line.trim().to_ascii_lowercase().as_str(),
        "y" | "yes"
    ))
}

fn live_model_api_key(config: &Config) -> workspace_engine::Result<String> {
    match env::var(&config.model_api_key_env) {
        Ok(api_key) => Ok(api_key),
        Err(_) if config.model_endpoint_is_loopback() => Ok(String::new()),
        Err(_) => Err(workspace_engine::ClientError::InvalidInput(format!(
            "{} is required for live model calls. Set DAMAIAN_MOCK_MODEL_RESPONSE for local smoke tests.",
            config.model_api_key_env
        ))),
    }
}

/// What this repository's config asked for and did not get, and any
/// `command_allowlist` entries still awaiting a keep-or-discard decision.
/// Reading the review is what records the refusals in the audit log, so it
/// reports each key once per repository — the same behaviour the app has.
fn repository_config_review(repo: &str) -> workspace_engine::Result<String> {
    let (config, report) = Config::load_for_repository_reporting(Some(Path::new(repo)))?;
    let engine = WorkspaceEngine::new(config);
    let mut output = String::new();
    match engine.repository_trust.review(&report, &engine.audit_log)? {
        Some(notice) => {
            for rejected in &notice.rejected {
                output.push_str(&format!(
                    "rejected {} ({})\n",
                    rejected.key,
                    rejected.class.as_str()
                ));
            }
        }
        None => output.push_str("no unreported repository config keys were refused\n"),
    }
    match engine
        .repository_trust
        .pending_allowlist_migration(&report)?
    {
        Some(migration) => {
            for entry in &migration.entries {
                output.push_str(&format!("awaiting decision: {entry}\n"));
            }
            output.push_str(
                "run `damaian config-allowlist-keep <repo> [command...]` to keep or discard\n",
            );
        }
        None => output.push_str("no repository allowlist entries are awaiting a decision\n"),
    }
    // Kept apart from the repository's refusals above: these are keys the
    // user's own selected profile carried and could not apply.
    if let Some(profile) = &report.permission_profile {
        let fresh = review_profile_rejections(&engine.config.data_dir, &report, &engine.audit_log)?;
        for rejected in &fresh {
            output.push_str(&format!(
                "profile {} could not apply {} ({})\n",
                profile.as_str(),
                rejected.key,
                rejected.class.as_str()
            ));
        }
    }
    Ok(output)
}

/// Every configured hook, whether it can run yet, and the command it runs.
/// Spec 32 Task 9; the shell's Settings page shows the same listing.
fn hooks_listing(repo: &str) -> workspace_engine::Result<String> {
    let config = Config::load_for_repository(Some(Path::new(repo)))?;
    let listings = list_hooks(&config, Path::new(repo));
    if listings.is_empty() {
        return Ok("no hooks configured\n".to_string());
    }
    let mut output = String::new();
    for hook in listings {
        let failure = if hook.mode == "mandatory" {
            "if it fails: blocks"
        } else {
            "if it fails: warns"
        };
        output.push_str(&format!(
            "{}  {}  {} ({failure})  {}\n",
            hook.id,
            hook.source,
            hook.mode,
            hook.activation.as_str()
        ));
        let events = hook.events.join(", ");
        output.push_str(&format!("  events: {events}\n"));
        let mut command = hook.command;
        if !hook.args.is_empty() {
            command.push(' ');
            command.push_str(&hook.args.join(" "));
        }
        output.push_str(&format!("  command: {command}\n"));
        if let Some(hash) = hook.hash {
            output.push_str(&format!("  review hash: {hash}\n"));
        }
    }
    Ok(output)
}

/// `hook-review <repo> <id> approve|decline`. The review is written to user
/// config, never to the repository's own `.damaian/config.conf`, and an
/// approval records the definition hash that is current now.
fn review_hook_command(
    repo: &str,
    hook_id: &str,
    decision: &str,
) -> workspace_engine::Result<String> {
    let config = Config::load_for_repository(Some(Path::new(repo)))?;
    let hook = config.hook(hook_id).cloned().ok_or_else(|| {
        workspace_engine::ClientError::InvalidInput(format!("Hook `{hook_id}` is not configured"))
    })?;
    let decision = match decision {
        "approve" => HookReviewDecision::Approve {
            hash: hook_definition_hash(&hook, Path::new(repo))?,
        },
        "decline" => HookReviewDecision::Decline,
        _ => {
            return Err(workspace_engine::ClientError::InvalidInput(
                "hook-review takes approve or decline".to_string(),
            ));
        }
    };
    let approved = matches!(decision, HookReviewDecision::Approve { .. });
    let path = review_hook(&config, Path::new(repo), hook_id, decision)?;
    Ok(format!(
        "{} {}\nwrote {}\n",
        hook_id,
        if approved { "approved" } else { "declined" },
        path.to_string_lossy()
    ))
}

/// `hook-run <repo> <id> <event>`: one hook against a sample envelope, with
/// its redacted stderr, for debugging a failing hook (proposal §5.11).
fn run_hook_command(repo: &str, hook_id: &str, event: &str) -> workspace_engine::Result<String> {
    let event = HookEvent::parse(event).ok_or_else(|| {
        workspace_engine::ClientError::InvalidInput(format!("Unknown hook event: {event}"))
    })?;
    let config = Config::load_for_repository(Some(Path::new(repo)))?;
    let hook = config.hook(hook_id).cloned().ok_or_else(|| {
        workspace_engine::ClientError::InvalidInput(format!("Hook `{hook_id}` is not configured"))
    })?;
    let run = test_run(&config, &hook, event, Path::new(repo))?;
    let mut output = format!(
        "{hook_id} at {}: {}\nfailure: {}\nduration: {} ms\n",
        event.as_str(),
        run.verdict.as_str(),
        run.failure.map_or("none", |kind| kind.as_str()),
        run.duration_ms
    );
    if run.stderr.trim().is_empty() {
        output.push_str("stderr: (empty)\n");
    } else {
        output.push_str("stderr:\n");
        output.push_str(&run.stderr);
        if !run.stderr.ends_with('\n') {
            output.push('\n');
        }
    }
    Ok(output)
}

/// Selects the permission profile for one checkout, in user config, and audits
/// the change. Config is loaded without the repository so a checkout whose
/// selected custom profile file has gone missing can still be switched.
fn set_permission_profile(repo: &str, profile: &str) -> workspace_engine::Result<String> {
    let id = ProfileId::parse(profile)?;
    let config = Config::load_for_repository(None)?;
    let engine = WorkspaceEngine::new(config.clone());
    let selection = select_profile(&config, Path::new(repo), id, &engine.audit_log)?;
    Ok(format!(
        "permission profile for {} ({}) is now {} (was {})\n",
        repo,
        selection.repository_id,
        selection.selected.as_str(),
        selection
            .previous
            .as_ref()
            .map_or("none", ProfileId::as_str)
    ))
}

/// `profile-import <custom-name> <file> [--repo <repo>] [--replace] [--review]`.
/// The review is printed before anything is written (spec 31, `context.md`
/// §9): what a profile cannot carry, and what would have no effect because it
/// loosens this checkout's config. `--review` stops there.
fn import_permission_profile(args: &[String]) -> workspace_engine::Result<String> {
    let mut positional = Vec::new();
    let mut repo = None;
    let mut replace = false;
    let mut review_only = false;
    let mut rest = args.iter();
    while let Some(arg) = rest.next() {
        match arg.as_str() {
            "--repo" => {
                repo = Some(rest.next().ok_or_else(|| {
                    workspace_engine::ClientError::InvalidInput("Missing <repo>".to_string())
                })?)
            }
            "--replace" => replace = true,
            "--review" => review_only = true,
            _ => positional.push(arg.as_str()),
        }
    }
    let name = require_arg_str(&positional, 0, "<custom-name>")?;
    let file = require_arg_str(&positional, 1, "<file>")?;
    let text = std::fs::read_to_string(file)?;
    let base = profile_import_base(repo.map(Path::new))?;

    let review = review_profile_import(&base, &text);
    let mut output = String::new();
    let listed = |title: &str, keys: &[workspace_engine::RejectedConfigKey]| {
        let mut section = format!("{title}: {}\n", keys.len());
        for rejected in keys {
            section.push_str(&format!(
                "  {} ({})\n",
                rejected.key,
                rejected.class.as_str()
            ));
        }
        section
    };
    output.push_str(&format!("carried: {}\n", review.carried.join(", ")));
    output.push_str(&listed(
        "not carried, a profile cannot set these (not written)",
        &review.not_carried,
    ));
    output.push_str(&listed(
        "no effect, these would loosen this config and a profile only narrows",
        &review.loosening,
    ));
    print!("{output}");
    if review_only {
        return Ok("review only: nothing was written\n".to_string());
    }
    let engine = WorkspaceEngine::new(base.clone());
    let imported = import_profile(&base, &text, name, replace, &engine.audit_log)?;
    Ok(format!(
        "{} permission profile {} at {}\nselect it with: damaian profile-set <repo> {}\n",
        if imported.replaced {
            "replaced"
        } else {
            "imported"
        },
        imported.id.as_str(),
        imported.path.display(),
        imported.id.as_str()
    ))
}

fn require_arg_str<'a>(
    args: &[&'a str],
    index: usize,
    name: &str,
) -> workspace_engine::Result<&'a str> {
    args.get(index)
        .copied()
        .ok_or_else(|| workspace_engine::ClientError::InvalidInput(format!("Missing {name}")))
}

/// Answers the migration question: the named commands move to user config
/// under this repository's key, everything else the repository listed is
/// discarded. No commands means discard all. The repository's own file is
/// never modified.
fn keep_repository_allowlist(repo: &str, keep: &[String]) -> workspace_engine::Result<String> {
    let (config, report) = Config::load_for_repository_reporting(Some(Path::new(repo)))?;
    let engine = WorkspaceEngine::new(config);
    let path =
        engine
            .repository_trust
            .resolve_allowlist_migration(&report, keep, &engine.audit_log)?;
    Ok(format!(
        "kept {} of {} entries in {}\n",
        keep.len(),
        report.repository_allowlist_entries.len(),
        path.to_string_lossy()
    ))
}

/// Applies one key at repository scope over this repository's real effective
/// policy and reports what the boundary refused. Used to warn immediately
/// after writing a repository config key that will be ignored.
fn repository_scope_refusals(
    repo: &str,
    key: &str,
    value: &str,
) -> workspace_engine::Result<Vec<workspace_engine::RejectedConfigKey>> {
    let mut probe = Config::load_for_repository(Some(Path::new(repo)))?;
    let mut single = ConfigOverlay::default();
    single.set(key, value)?;
    Ok(probe
        .apply_overlay_scoped(single, ConfigScope::Repository)
        .rejected)
}

/// Corrects root detection in `<repo>/.damaian/config.conf` (spec 24 §5.7).
/// The strict parse refuses a file with a line it cannot read, so a
/// hand-edited file is never rewritten without that line. The path is
/// validated when the map is built, where a bad entry shows as
/// `invalidOverride`.
fn edit_repository_roots(args: &[String]) -> workspace_engine::Result<String> {
    let repo = require_arg(args, 1, "<repo>")?;
    let edit = match require_arg(args, 2, "add|remove|clear")? {
        "add" => RootOverrideEdit::Add,
        "remove" => RootOverrideEdit::Remove,
        "clear" => RootOverrideEdit::Clear,
        _ => {
            return Err(workspace_engine::ClientError::InvalidInput(
                "repo-root takes add, remove or clear".to_string(),
            ));
        }
    };
    let root = require_arg(args, 3, "<path>")?;
    let path = Config::repository_config_path(repo);
    let mut overlay = ConfigOverlay::load_or_default(&path)?;
    edit_root_overrides(&mut overlay, edit, root);
    overlay.save(&path)?;
    Ok(format!(
        "wrote {} (in your working tree, so git status shows it)\n",
        path.to_string_lossy()
    ))
}

fn set_config_value(args: &[String]) -> workspace_engine::Result<()> {
    let scope = require_arg(args, 1, "<scope>")?;
    match scope {
        "user" => {
            let key = require_arg(args, 2, "<key>")?;
            let value = require_arg(args, 3, "<value>")?;
            let config = Config::load_for_repository(None)?;
            let path = config.user_config_path();
            let mut overlay = ConfigOverlay::load_or_default(&path)?;
            overlay.set(key, value)?;
            overlay.save(&path)?;
            println!("wrote {}", path.to_string_lossy());
        }
        "repo" => {
            let repo = require_arg(args, 2, "<repo>")?;
            let key = require_arg(args, 3, "<key>")?;
            let value = require_arg(args, 4, "<value>")?;
            let path = Config::repository_config_path(repo);
            let mut overlay = ConfigOverlay::load_or_default(&path)?;
            overlay.set(key, value)?;
            overlay.save(&path)?;
            println!("wrote {}", path.to_string_lossy());
            // Say now if repository scope will refuse this key, rather than
            // leaving the user to wonder why the setting had no effect.
            for rejected in repository_scope_refusals(repo, key, value)? {
                println!(
                    "note: {} is {} in repository config and was ignored",
                    rejected.key,
                    rejected.class.as_str()
                );
            }
        }
        "admin" => {
            let key = require_arg(args, 2, "<key>")?;
            let value = require_arg(args, 3, "<value>")?;
            let config = Config::load_for_repository(None)?;
            let path = config.admin_config_path();
            let mut overlay = ConfigOverlay::load_or_default(&path)?;
            overlay.set(key, value)?;
            overlay.save(&path)?;
            println!("wrote {}", path.to_string_lossy());
        }
        _ => {
            return Err(workspace_engine::ClientError::InvalidInput(
                "config-set scope must be user, repo, or admin".to_string(),
            ));
        }
    }
    Ok(())
}

fn search_results_json(results: &[SearchResult]) -> String {
    let body = results
        .iter()
        .map(|result| {
            format!(
                "{{\"path\":\"{}\",\"language\":\"{}\",\"score\":{},\"snippet\":\"{}\"}}",
                escape(&result.path),
                escape(&result.language),
                result.score,
                escape(&result.snippet)
            )
        })
        .collect::<Vec<_>>()
        .join(",");
    format!("[{body}]")
}

/// How many matches `damaian symbols` prints.
const SYMBOL_LIMIT: usize = 50;

fn symbol_load_json(load: &SymbolLoad) -> String {
    match load {
        SymbolLoad::Reused => map_load_json(&MapLoad::Reused),
        SymbolLoad::Built => map_load_json(&MapLoad::Built),
        SymbolLoad::Rebuilt { reason } => map_load_json(&MapLoad::Rebuilt { reason: *reason }),
    }
}

/// `name  kind  path:line  root  provenance`, one match per line, after a
/// line saying how the index was loaded. Provenance is always printed: a
/// heuristic is never shown as fact (spec 25 §5.7).
fn symbol_matches_text(matches: &[SymbolMatch], load: &SymbolLoad) -> String {
    let mut output = match load {
        SymbolLoad::Reused => "symbol index reused (files unchanged)\n".to_string(),
        SymbolLoad::Built => "symbol index built (none stored)\n".to_string(),
        SymbolLoad::Rebuilt { reason } => match reason {
            RebuildReason::Corrupt => {
                "symbol index rebuilt: the stored index was unreadable\n".to_string()
            }
            RebuildReason::SchemaMismatch { found } => format!(
                "symbol index rebuilt: the stored index was schema version {found}, this build reads {}\n",
                workspace_engine::SYMBOL_INDEX_SCHEMA_VERSION
            ),
            RebuildReason::Stale => "symbol index updated: files changed\n".to_string(),
        },
    };
    if matches.is_empty() {
        output.push_str("no matching symbols\n");
    }
    for found in matches {
        let symbol = &found.symbol;
        let root = if symbol.root_path.is_empty() {
            "."
        } else {
            &symbol.root_path
        };
        output.push_str(&format!(
            "{}  {}  {}:{}  {root}  {}\n",
            symbol.name,
            symbol.kind.as_str(),
            symbol.path,
            symbol.line,
            symbol.provenance.as_str()
        ));
    }
    output
}

/// `<path>:<line>` for `damaian symbol-references`. The last colon separates
/// the line, so a path may contain one.
fn parse_symbol_location(location: &str) -> workspace_engine::Result<(String, u32)> {
    let (path, line) = location.rsplit_once(':').ok_or_else(|| {
        workspace_engine::ClientError::InvalidInput(format!(
            "Expected <path>:<line>, not {location}"
        ))
    })?;
    if path.is_empty() {
        return Err(workspace_engine::ClientError::InvalidInput(format!(
            "{location} has no path"
        )));
    }
    let line = line.parse::<u32>().map_err(|_| {
        workspace_engine::ClientError::InvalidInput(format!(
            "{location} does not end in a line number"
        ))
    })?;
    if line == 0 {
        return Err(workspace_engine::ClientError::InvalidInput(format!(
            "{location} has line 0; lines are 1-based"
        )));
    }
    Ok((path.to_string(), line))
}

/// One line per relation, after a line saying whether a language server
/// answered. Provenance is always printed: a language-server answer is the
/// only kind this command reports, and it says so (spec 25 §5.7).
fn symbol_references_text(result: &SymbolReferences) -> String {
    let location = format!("{}:{}", result.symbol.path, result.symbol.line);
    let mut output = if result.available {
        format!(
            "{} for {} at {location}\n",
            result.kind.as_str(),
            result.symbol.name
        )
    } else {
        format!(
            "{} for {} at {location} unavailable: {}\n",
            result.kind.as_str(),
            result.symbol.name,
            result
                .reason
                .as_deref()
                .unwrap_or("no language server answered")
        )
    };
    if result.available && result.relations.is_empty() {
        output.push_str("none\n");
    }
    for relation in &result.relations {
        let (path, name, line) = match &relation.to {
            RelationTarget::Symbol(target) => {
                (target.path.as_str(), target.name.as_str(), target.line)
            }
            RelationTarget::File { path } => (path.as_str(), "", 0),
            RelationTarget::External { specifier } => (specifier.as_str(), "", 0),
        };
        let name = if name.is_empty() {
            result.symbol.name.as_str()
        } else {
            name
        };
        output.push_str(&format!(
            "{path}:{line}  {name}  {}  {}\n",
            relation.kind.as_str(),
            relation.provenance.as_str()
        ));
    }
    output
}

fn map_load_json(load: &MapLoad) -> String {
    match load {
        MapLoad::Reused => "{\"outcome\":\"reused\"}".to_string(),
        MapLoad::Built => "{\"outcome\":\"built\"}".to_string(),
        MapLoad::Rebuilt { reason } => match reason {
            RebuildReason::SchemaMismatch { found } => format!(
                "{{\"outcome\":\"rebuilt\",\"reason\":\"{}\",\"foundSchemaVersion\":{found}}}",
                reason.as_str()
            ),
            _ => format!(
                "{{\"outcome\":\"rebuilt\",\"reason\":\"{}\"}}",
                reason.as_str()
            ),
        },
    }
}

/// One line per root and per exclusion, after a line saying whether the
/// stored map was reused or why it was rebuilt.
fn repository_map_summary(map: &RepositoryMap, load: &MapLoad) -> String {
    let mut output = match load {
        MapLoad::Reused => "map reused (inputs unchanged)\n".to_string(),
        MapLoad::Built => "map built (none stored)\n".to_string(),
        MapLoad::Rebuilt { reason } => match reason {
            RebuildReason::Corrupt => "map rebuilt: the stored map was unreadable\n".to_string(),
            RebuildReason::SchemaMismatch { found } => format!(
                "map rebuilt: the stored map was schema version {found}, this build reads {}\n",
                map.schema_version
            ),
            RebuildReason::Stale => {
                "map rebuilt: the repository or its config changed\n".to_string()
            }
        },
    };
    output.push_str(&format!("{} roots\n", map.roots.len()));
    for root in &map.roots {
        let path = if root.path.is_empty() {
            "."
        } else {
            &root.path
        };
        let evidence = match &root.detected_by {
            RootEvidence::RepositoryRoot => "repository root".to_string(),
            RootEvidence::Manifest { path } => path.clone(),
            RootEvidence::UserOverride => "added by you".to_string(),
        };
        if root.user_override == Some(RootOverride::Removed) {
            output.push_str(&format!("  {path}  ({evidence}, removed by you)\n"));
            continue;
        }
        let commands = root
            .commands
            .iter()
            .map(|command| command.command.as_str())
            .collect::<Vec<_>>()
            .join(", ");
        output.push_str(&format!(
            "  {path}  ({evidence})  languages: {}  commands: {}\n",
            or_none(&root.languages.join(", ")),
            or_none(&commands)
        ));
    }
    for excluded in &map.excluded {
        let reason = match excluded.reason {
            ExclusionReason::Vendor => "vendor or build output",
            ExclusionReason::BelowDepthCeiling => "below the depth ceiling",
            ExclusionReason::InvalidOverride => "override not applied",
        };
        output.push_str(&format!("  excluded {}  ({reason})\n", excluded.path));
    }
    output
}

fn or_none(value: &str) -> &str {
    if value.is_empty() { "none" } else { value }
}

fn risk_json(risk: &CommandRisk) -> &'static str {
    risk.as_str()
}

fn command_proposal_json(proposal: &CommandProposal) -> String {
    format!(
        "{{\"proposalId\":\"{}\",\"command\":\"{}\",\"workingDirectory\":\"{}\",\"risk\":\"{}\",\"requiresApproval\":{},\"blocked\":{},\"mayUseNetwork\":{},\"expectedEffects\":\"{}\"}}",
        escape(&proposal.id),
        escape(&proposal.command),
        escape(&proposal.working_directory),
        risk_json(&proposal.risk),
        proposal.requires_approval,
        proposal.blocked,
        proposal.may_use_network,
        escape(&proposal.expected_effects)
    )
}

fn escape(value: &str) -> String {
    value
        .replace('\\', "\\\\")
        .replace('"', "\\\"")
        .replace('\n', "\\n")
        .replace('\r', "\\r")
        .replace('\t', "\\t")
}

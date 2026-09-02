use std::env;
use std::io::IsTerminal;
use std::path::Path;
use workspace_engine::{
    CommandProposal, CommandRisk, Config, ConfigOverlay, ConfigScope, CurlModelTransport,
    MockModelAdapter, OpenAICompatibleAdapter, SearchResult, WorkspaceEngine,
    command_approval_prompt, parse_hunk_selection, patch_diff_text, patch_hunk_summary,
    render_markdown_to_ansi,
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
  damaian classify-command <command>
  damaian config-show [repo]
  damaian config-review <repo>
  damaian config-allowlist-keep <repo> [command...]
  damaian config-set user <key> <value>
  damaian config-set repo <repo> <key> <value>
  damaian config-set admin <key> <value>
  damaian propose-command <repo> <command>
  damaian propose-validations <repo>
  damaian run-command <proposal-id> --approve [--always]
  damaian reject-command <proposal-id>
  damaian ask <repo> <prompt>
  damaian propose-edit <repo> <prompt>
  damaian show-patch <repo> <patch-id>
  damaian apply-patch <repo> <patch-id> [file...] [--hunk-selection <json>]
                      [--allow-generated-secrets]
  damaian reject-patch <patch-id>
"
}

fn main() {
    if let Err(error) = run() {
        eprintln!("{error}");
        std::process::exit(1);
    }
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
            let file =
                engine
                    .file_access
                    .read_file(repo, path, Some("cli"), Some(repo), false, false)?;
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
            let config = if let Some(repo) = args.get(1) {
                Config::load_for_repository(Some(Path::new(repo)))?
            } else {
                Config::load_for_repository(None)?
            };
            print!("{}", config.to_policy_text());
        }
        "config-review" => {
            let repo = require_arg(&args, 1, "<repo>")?;
            print!("{}", repository_config_review(repo)?);
        }
        "config-allowlist-keep" => {
            let repo = require_arg(&args, 1, "<repo>")?;
            print!("{}", keep_repository_allowlist(repo, &args[2..])?);
        }
        "config-set" => {
            set_config_value(&args)?;
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
        "propose-validations" => {
            let repo = require_arg(&args, 1, "<repo>")?;
            let engine = engine_for_repo(repo)?;
            let proposals = engine
                .validation_orchestrator
                .propose_detected_validations(repo)?;
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
            let record =
                engine
                    .validation_orchestrator
                    .run_proposal(proposal_id, approved, "local_user")?;
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
                let api_key = env::var(&engine.config.model_api_key_env).map_err(|_| {
                    workspace_engine::ClientError::InvalidInput(format!(
                        "{} is required for live model calls. Set DAMAIAN_MOCK_MODEL_RESPONSE for local smoke tests.",
                        engine.config.model_api_key_env
                    ))
                })?;
                let transport = CurlModelTransport::new(&engine.config.model_base_url, api_key);
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
                    .propose_edit(repo, &prompt, &[], &mut adapter)?
            } else {
                let api_key = env::var(&engine.config.model_api_key_env).map_err(|_| {
                    workspace_engine::ClientError::InvalidInput(format!(
                        "{} is required for live model calls. Set DAMAIAN_MOCK_MODEL_RESPONSE for local smoke tests.",
                        engine.config.model_api_key_env
                    ))
                })?;
                let transport = CurlModelTransport::new(&engine.config.model_base_url, api_key);
                let mut adapter = OpenAICompatibleAdapter::with_provider(
                    &engine.config.model_provider,
                    &engine.config.model_name,
                    transport,
                );
                engine
                    .edit_orchestrator
                    .propose_edit(repo, &prompt, &[], &mut adapter)?
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
    Ok(WorkspaceEngine::new(Config::load_for_repository(None)?))
}

fn engine_for_repo(repo: &str) -> workspace_engine::Result<WorkspaceEngine> {
    Ok(WorkspaceEngine::new(Config::load_for_repository(Some(
        Path::new(repo),
    ))?))
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
    Ok(output)
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
    Ok(probe.apply_overlay_scoped(single, ConfigScope::Repository))
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

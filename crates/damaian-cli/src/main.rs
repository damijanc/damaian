use std::env;
use std::path::Path;
use workspace_engine::{
    CommandProposal, CommandRisk, Config, ConfigOverlay, CurlModelTransport, MockModelAdapter,
    OpenAICompatibleAdapter, SearchResult, WorkspaceEngine, command_approval_prompt,
    patch_diff_text,
};

fn usage() -> &'static str {
    "Usage:
  damaian index <repo>
  damaian search <repo> <query>
  damaian read <repo> <path>
  damaian git-status <repo>
  damaian git-diff <repo>
  damaian detect-commands <repo>
  damaian classify-command <command>
  damaian config-show [repo]
  damaian config-set user <key> <value>
  damaian config-set repo <repo> <key> <value>
  damaian config-set admin <key> <value>
  damaian propose-command <repo> <command>
  damaian propose-validations <repo>
  damaian run-command <proposal-id> --approve
  damaian reject-command <proposal-id>
  damaian ask <repo> <prompt>
  damaian propose-edit <repo> <prompt>
  damaian apply-patch <repo> <patch-id> [file...]
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
    let args = env::args().skip(1).collect::<Vec<_>>();
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
            let file = engine
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
            let classification = engine
                .command_policy
                .classify(&args[1..].join(" "), &cwd);
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
            let record =
                engine
                    .validation_orchestrator
                    .run_proposal(proposal_id, approved, "local_user")?;
            println!(
                "{{\"proposalId\":\"{}\",\"commandId\":\"{}\",\"exitCode\":{},\"stdoutRef\":\"{}\",\"stderrRef\":\"{}\"}}",
                escape(&record.proposal_id),
                escape(&record.execution.id),
                record.execution.exit_code.unwrap_or(-1),
                escape(&record.stdout_ref.to_string_lossy()),
                escape(&record.stderr_ref.to_string_lossy())
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
            let mut stdout_token = |token: &str| {
                print!("{token}");
            };
            let result = if let Ok(mock_response) = env::var("DAMAIAN_MOCK_MODEL_RESPONSE") {
                let mut adapter = MockModelAdapter::new(mock_response);
                engine
                    .chat_orchestrator
                    .ask(repo, &prompt, &[], &mut adapter, &mut stdout_token)?
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
                    .ask(repo, &prompt, &[], &mut adapter, &mut stdout_token)?
            };
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
        "apply-patch" => {
            let repo = require_arg(&args, 1, "<repo>")?;
            let engine = engine_for_repo(repo)?;
            let patch_id = require_arg(&args, 2, "<patch-id>")?;
            let approved_paths = if args.len() > 3 {
                Some(args[3..].to_vec())
            } else {
                None
            };
            let result = engine.edit_orchestrator.apply_stored_patch(
                repo,
                patch_id,
                approved_paths.as_deref(),
                None,
                "local_user",
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

fn default_engine() -> workspace_engine::Result<WorkspaceEngine> {
    Ok(WorkspaceEngine::new(Config::load_for_repository(None)?))
}

fn engine_for_repo(repo: &str) -> workspace_engine::Result<WorkspaceEngine> {
    Ok(WorkspaceEngine::new(Config::load_for_repository(Some(
        Path::new(repo),
    ))?))
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

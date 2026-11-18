use super::*;

/// Recognizes any of the native tools above by name and extracts a
/// [`ToolAction`] from its arguments. Returns `None` for an unrecognized
/// tool name or malformed/empty arguments — the caller treats that the same
/// as the model not having requested a tool at all, matching the existing
/// (and equally permissive) behavior of `command_request_from_tool_call`.
pub(super) fn tool_action_from_call(call: &ToolCall) -> Result<Option<ToolAction>> {
    match call.name.as_str() {
        "run_command" => Ok(command_request_from_tool_call(call).map(ToolAction::Command)),
        "propose_patch" => Ok(generated_edit_from_tool_call(call).map(ToolAction::ProposePatch)),
        "propose_plan" => {
            let Ok(arguments) = serde_json::from_str::<serde_json::Value>(&call.arguments_json)
            else {
                return Ok(None);
            };
            let Some(entries) = arguments.get("steps").and_then(|value| value.as_array()) else {
                return Ok(None);
            };
            let steps: Vec<ProposedStep> = entries
                .iter()
                .filter_map(|entry| {
                    let title = entry
                        .get("title")
                        .and_then(|value| value.as_str())
                        .map(str::trim)
                        .filter(|value| !value.is_empty())?;
                    // A check is optional, and a proposed check authorises
                    // nothing on its own (spec 105 §2). It only becomes
                    // authoritative once the user approves the plan. A check
                    // without a command is not a check, so it is dropped the
                    // same way a step with no title is.
                    let check = entry
                        .get("check")
                        .and_then(|value| planned_check_from_json(value, true));
                    // A build check (spec 109 D10) has the same shape and is
                    // feedback only, so it is never required.
                    let build = entry
                        .get("build")
                        .and_then(|value| planned_check_from_json(value, false));
                    Some(ProposedStep {
                        title: title.to_string(),
                        detail: entry
                            .get("detail")
                            .and_then(|value| value.as_str())
                            .map(str::trim)
                            .filter(|value| !value.is_empty())
                            .map(str::to_string),
                        check,
                        build,
                    })
                })
                .collect();
            // A plan of one step is not a plan (§5.1). Decoding it to `None`
            // rather than accepting it keeps the ceremony out of trivial turns
            // at the one place that decides.
            if steps.len() < 2 {
                return Ok(None);
            }
            Ok(Some(ToolAction::ProposePlan(steps)))
        }
        "complete_step" => Ok(Some(ToolAction::CompleteStep)),
        // A malformed or out-of-bounds question is an error naming what was
        // wrong, never `None`: the model must learn the bound it broke rather
        // than read "undecodable" and guess (spec 50 §5.1).
        "ask_user" => {
            let input: AskUserInput = serde_json::from_str(&call.arguments_json)
                .map_err(|error| ClientError::InvalidInput(error.to_string()))?;
            input.validate().map_err(ClientError::InvalidInput)?;
            Ok(Some(ToolAction::AskUser(input)))
        }
        // The URL is not parsed here: an unparsable one is a refused *call* the
        // model sees, with the reason, not a call that silently disappears
        // (spec 51 §5.3, context.md §5).
        "fetch_url" => {
            let Ok(arguments) = serde_json::from_str::<serde_json::Value>(&call.arguments_json)
            else {
                return Err(ClientError::InvalidInput(
                    "fetch_url arguments must be a JSON object".to_string(),
                ));
            };
            let Some(url) = string_argument(&arguments, "url") else {
                return Err(ClientError::InvalidInput(
                    "fetch_url requires a non-empty url".to_string(),
                ));
            };
            Ok(Some(ToolAction::FetchUrl { url }))
        }
        "propose_memory" => {
            let Ok(arguments) = serde_json::from_str::<serde_json::Value>(&call.arguments_json)
            else {
                return Ok(None);
            };
            let Some(statement) = string_argument(&arguments, "statement") else {
                return Err(ClientError::InvalidInput(
                    "propose_memory requires a non-empty statement".to_string(),
                ));
            };
            let category = match string_argument(&arguments, "category").as_deref() {
                Some("preference") => MemoryCategory::Preference,
                Some("decision") => MemoryCategory::Decision,
                Some("convention") => MemoryCategory::Convention,
                Some("environment") => MemoryCategory::Environment,
                Some("command") => MemoryCategory::Command,
                Some("fact") => MemoryCategory::Fact,
                Some(other) => {
                    return Err(ClientError::InvalidInput(format!(
                        "propose_memory: unknown category {other}"
                    )));
                }
                None => {
                    return Err(ClientError::InvalidInput(
                        "propose_memory requires a category".to_string(),
                    ));
                }
            };
            let scope = match string_argument(&arguments, "scope").as_deref() {
                Some("session") => Some(MemoryScope::Session),
                Some("project") => Some(MemoryScope::Project),
                Some("user") => Some(MemoryScope::User),
                Some(other) => {
                    return Err(ClientError::InvalidInput(format!(
                        "propose_memory: unknown scope {other}"
                    )));
                }
                None => None,
            };
            Ok(Some(ToolAction::ProposeMemory {
                statement,
                category,
                scope,
                source: string_argument(&arguments, "source"),
                evidence_path: string_argument(&arguments, "evidence_path"),
            }))
        }
        // Empty notes are a model error worth naming, like a malformed
        // question: an empty append records nothing, and silently accepting it
        // would waste a round (spec 65 §5.1).
        "turn_notes" => {
            let Ok(arguments) = serde_json::from_str::<serde_json::Value>(&call.arguments_json)
            else {
                return Ok(None);
            };
            let Some(text) = arguments
                .get("text")
                .and_then(|value| value.as_str())
                .map(str::trim)
                .filter(|value| !value.is_empty())
            else {
                return Err(ClientError::InvalidInput(
                    "turn_notes requires non-empty text".to_string(),
                ));
            };
            let replace = arguments
                .get("replace")
                .and_then(|value| value.as_bool())
                .unwrap_or(false);
            Ok(Some(ToolAction::TurnNotes {
                text: text.to_string(),
                replace,
            }))
        }
        "read_file" => {
            let Ok(arguments) = serde_json::from_str::<serde_json::Value>(&call.arguments_json)
            else {
                return Ok(None);
            };
            let Some(path) = arguments
                .get("path")
                .and_then(|value| value.as_str())
                .map(str::trim)
                .filter(|value| !value.is_empty())
            else {
                return Ok(None);
            };
            let start_line = arguments
                .get("start_line")
                .and_then(|value| value.as_u64())
                .map(|value| value as usize);
            let end_line = arguments
                .get("end_line")
                .and_then(|value| value.as_u64())
                .map(|value| value as usize);
            let range = match (start_line, end_line) {
                (Some(start), Some(end)) => Some(LineRange { start, end }),
                _ => None,
            };
            Ok(Some(ToolAction::ReadFile {
                path: path.to_string(),
                range,
                worktree: string_argument(&arguments, "worktree"),
            }))
        }
        "list_directory" => {
            let Ok(arguments) = serde_json::from_str::<serde_json::Value>(&call.arguments_json)
            else {
                return Ok(None);
            };
            let dir = arguments
                .get("dir")
                .and_then(|value| value.as_str())
                .map(str::trim)
                .filter(|value| !value.is_empty())
                .map(str::to_string);
            let depth = arguments
                .get("depth")
                .and_then(|value| value.as_u64())
                .map(|value| value as usize);
            Ok(Some(ToolAction::ListDirectory {
                dir,
                depth,
                worktree: string_argument(&arguments, "worktree"),
            }))
        }
        "search_content" => {
            let Ok(arguments) = serde_json::from_str::<serde_json::Value>(&call.arguments_json)
            else {
                return Ok(None);
            };
            let Some(pattern) = arguments
                .get("pattern")
                .and_then(|value| value.as_str())
                .map(str::trim)
                .filter(|value| !value.is_empty())
            else {
                return Ok(None);
            };
            let path_glob = arguments
                .get("path_glob")
                .and_then(|value| value.as_str())
                .map(str::trim)
                .filter(|value| !value.is_empty())
                .map(str::to_string);
            let max_matches = arguments
                .get("max_matches")
                .and_then(|value| value.as_u64())
                .map(|value| value as usize);
            Ok(Some(ToolAction::SearchContent {
                pattern: pattern.to_string(),
                path_glob,
                max_matches,
                worktree: string_argument(&arguments, "worktree"),
            }))
        }
        "edit_file" => {
            let Ok(arguments) = serde_json::from_str::<serde_json::Value>(&call.arguments_json)
            else {
                return Ok(None);
            };
            let Some(summary) = arguments
                .get("summary")
                .and_then(|value| value.as_str())
                .map(str::trim)
                .filter(|value| !value.is_empty())
            else {
                return Ok(None);
            };
            let Some(entries) = arguments.get("edits").and_then(|value| value.as_array()) else {
                return Ok(None);
            };
            let edits: Vec<RegionEdit> = entries
                .iter()
                .filter_map(|entry| {
                    let path = entry
                        .get("path")
                        .and_then(|value| value.as_str())
                        .map(str::trim)
                        .filter(|value| !value.is_empty())?;
                    let old_text = entry.get("old_text").and_then(|value| value.as_str())?;
                    let new_text = entry.get("new_text").and_then(|value| value.as_str())?;
                    Some(RegionEdit {
                        path: path.to_string(),
                        old_text: old_text.to_string(),
                        new_text: new_text.to_string(),
                    })
                })
                .collect();
            if edits.is_empty() {
                return Ok(None);
            }
            Ok(Some(ToolAction::EditFile {
                summary: summary.to_string(),
                edits,
            }))
        }
        "search_codebase" => {
            let Ok(arguments) = serde_json::from_str::<serde_json::Value>(&call.arguments_json)
            else {
                return Ok(None);
            };
            let Some(query) = arguments
                .get("query")
                .and_then(|value| value.as_str())
                .map(str::trim)
                .filter(|value| !value.is_empty())
            else {
                return Ok(None);
            };
            let query = query.to_string();
            if query.is_empty() {
                return Ok(None);
            }
            let semantic =
                arguments.get("mode").and_then(|value| value.as_str()) == Some("semantic");
            let limit = arguments
                .get("limit")
                .and_then(|value| value.as_u64())
                .map(|value| value as usize)
                .filter(|value| *value > 0)
                .unwrap_or(8)
                .min(20);
            Ok(Some(ToolAction::SearchCodebase {
                query,
                semantic,
                limit,
            }))
        }
        // Both stay as lenient as they always were: malformed arguments read
        // the session's checkout rather than refusing the call.
        "read_git_status" => {
            let arguments = serde_json::from_str::<serde_json::Value>(&call.arguments_json)
                .unwrap_or(serde_json::Value::Null);
            Ok(Some(ToolAction::ReadGitStatus {
                worktree: string_argument(&arguments, "worktree"),
            }))
        }
        "read_git_diff" => {
            let arguments = serde_json::from_str::<serde_json::Value>(&call.arguments_json)
                .unwrap_or(serde_json::Value::Null);
            let flag = |key: &str| {
                arguments
                    .get(key)
                    .and_then(|value| value.as_bool())
                    .unwrap_or(false)
            };
            let paths = arguments
                .get("paths")
                .and_then(|value| value.as_array())
                .map(|entries| {
                    entries
                        .iter()
                        .filter_map(|entry| entry.as_str())
                        .map(str::trim)
                        .filter(|entry| !entry.is_empty())
                        .map(str::to_string)
                        .collect()
                })
                .unwrap_or_default();
            Ok(Some(ToolAction::ReadGitDiff {
                staged: flag("staged"),
                worktree: string_argument(&arguments, "worktree"),
                paths,
                stat: flag("stat"),
                base: string_argument(&arguments, "base"),
            }))
        }
        "list_worktrees" => {
            let arguments = serde_json::from_str::<serde_json::Value>(&call.arguments_json)
                .unwrap_or(serde_json::Value::Null);
            Ok(Some(ToolAction::ListWorktrees {
                base: string_argument(&arguments, "base"),
            }))
        }
        "read_git_log" => {
            let arguments = serde_json::from_str::<serde_json::Value>(&call.arguments_json)
                .unwrap_or(serde_json::Value::Null);
            let max_count = arguments
                .get("max_count")
                .and_then(|value| value.as_u64())
                .map(|value| value as usize);
            Ok(Some(ToolAction::ReadGitLog {
                worktree: string_argument(&arguments, "worktree"),
                base: string_argument(&arguments, "base"),
                max_count,
            }))
        }
        "inspect_web_page" | "run_web_scenario" => {
            WebDiagnosticCall::from_tool_call(&call.name, &call.arguments_json)
                .map(|call| call.map(ToolAction::WebDiagnostic))
        }
        name => Ok(
            parse_namespaced_tool_name(name).map(|(server_id, tool_name)| ToolAction::McpCall {
                server_id,
                tool_name,
                arguments_json: call.arguments_json.clone(),
            }),
        ),
    }
}

/// A trimmed, non-empty string argument, or `None`.
fn string_argument(arguments: &serde_json::Value, key: &str) -> Option<String> {
    arguments
        .get(key)
        .and_then(|value| value.as_str())
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_string)
}

/// A decoded call and what it asks for; `None` is the text-envelope fallback.
pub(super) type DecodedCall = (Option<ToolCall>, ToolAction);

/// A call that could not be decoded, with the note to feed back to the model.
type UndecodableCall = (ToolCall, String);

/// Every call in a round that decodes, in the order the model made them, plus
/// the ones that did not and the note to feed back for each.
///
/// A first-only walk used to silently drop the rest of a round, so the
/// multi-call shape spec 47 requirement 8 batches never actually reached
/// dispatch. An `Err` is a rejected call; an `Ok(None)` is a recognized tool
/// whose required arguments were missing or empty, and is reported through
/// [`undecodable_tool_call_note`] so a truncated `arguments` string still says
/// so.
pub(super) fn decodable_tool_actions(
    calls: &[ToolCall],
    truncated: bool,
) -> (Vec<DecodedCall>, Vec<UndecodableCall>) {
    let mut actions = Vec::new();
    let mut undecodable = Vec::new();
    for call in calls {
        match tool_action_from_call(call) {
            Ok(Some(action)) => actions.push((Some(call.clone()), action)),
            Ok(None) => undecodable.push((call.clone(), undecodable_tool_call_note(call, truncated))),
            Err(error) => undecodable.push((
                call.clone(),
                format!(
                    "Your `{}` call was rejected before execution: {error}. Retry with well-formed arguments matching the tool schema.",
                    call.name
                ),
            )),
        }
    }
    (actions, undecodable)
}

/// Feedback for a tool call whose arguments couldn't be decoded. When the
/// provider truncated the response, says so explicitly and asks for a smaller
/// call — retrying the same oversized patch would just truncate again.
fn undecodable_tool_call_note(call: &ToolCall, truncated: bool) -> String {
    let name = &call.name;
    if truncated {
        format!(
            "Your `{name}` call was cut off because the response reached the model's maximum output length, so its arguments were incomplete and could not be used. Nothing was changed. Retry with a smaller call: for a patch, propose fewer files at a time — a single file per call if needed — and send the rest in follow-up calls."
        )
    } else {
        format!(
            "Your `{name}` call could not be decoded: the arguments were not valid JSON matching the tool's schema. Nothing was changed. Retry with well-formed arguments."
        )
    }
}

fn generated_edit_from_tool_call(call: &ToolCall) -> Option<GeneratedEdit> {
    if call.name != "propose_patch" {
        return None;
    }
    let arguments: serde_json::Value = serde_json::from_str(&call.arguments_json).ok()?;
    let summary = arguments
        .get("summary")
        .and_then(|value| value.as_str())
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .unwrap_or("Proposed model edit")
        .to_string();
    let files = arguments.get("files")?.as_array()?;
    if files.is_empty() {
        return None;
    }
    let mut changes = Vec::new();
    for file in files {
        let path = file.get("path")?.as_str()?.trim().to_string();
        if path.is_empty() {
            return None;
        }
        let content = file
            .get("content")
            .and_then(|value| value.as_str())
            .unwrap_or_default()
            .to_string();
        let status = file
            .get("status")
            .and_then(|value| value.as_str())
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(str::to_string);
        changes.push(ProposedChange {
            path,
            new_content: content,
            status,
            allow_restricted: false,
        });
    }
    Some(GeneratedEdit { summary, changes })
}

fn command_request_from_tool_call(call: &ToolCall) -> Option<CommandRequest> {
    if call.name != "run_command" {
        return None;
    }
    let arguments: serde_json::Value = serde_json::from_str(&call.arguments_json).ok()?;
    let command = arguments.get("command")?.as_str()?.trim().to_string();
    if command.is_empty() {
        return None;
    }
    let reason = arguments
        .get("reason")
        .and_then(|value| value.as_str())
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .unwrap_or("Assistant requested a local command")
        .to_string();
    let working_directory = arguments
        .get("working_directory")
        .and_then(|value| value.as_str())
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_string);
    Some(CommandRequest {
        command,
        reason,
        working_directory,
    })
}

pub(super) fn parse_command_request(value: &str) -> Option<CommandRequest> {
    let marker_start = value.find("DAMAIAN_COMMAND_V1")?;
    let envelope = &value[marker_start..];
    let envelope = if let Some(end_start) = envelope.find("END_COMMAND") {
        &envelope[..end_start + "END_COMMAND".len()]
    } else {
        envelope
    };
    let mut command = String::new();
    let mut reason = String::new();
    let mut working_directory: Option<String> = None;
    for raw_line in envelope.lines() {
        let line = raw_line.trim();
        if let Some(value) = line.strip_prefix("COMMAND:") {
            command = value.trim().to_string();
        } else if let Some(value) = line.strip_prefix("REASON:") {
            reason = value.trim().to_string();
        } else if let Some(value) = line.strip_prefix("WORKING_DIRECTORY:") {
            let value = value.trim();
            if !value.is_empty() {
                working_directory = Some(value.to_string());
            }
        } else if line.trim() == "END_COMMAND" {
            break;
        }
    }
    if command.is_empty() {
        return None;
    }
    if reason.is_empty() {
        reason = "Assistant requested a local command".to_string();
    }
    Some(CommandRequest {
        command,
        reason,
        working_directory,
    })
}

# Working Modes Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Implements:** [`proposal.md`](proposal.md) in full · corrections and the
extended tool-class mapping in [`context.md`](context.md)
**Started:** 2026-09-23 — **Done:** —

## Progress

| Task | State | Notes |
|---|---|---|
| 1 · `SessionMode`, `ToolAction` extended with the two new mutation/planning arms it needs, and the permission-matrix function | Done | `mode.rs` added, registered in `lib.rs` between `mcp` and `model`. Signature decision (Step 3): `fn mode_permits(mode: SessionMode, action: &ToolAction, command: Option<&CommandClassification>) -> Permission` — the single-function-over-the-whole-enum option, per context.md §6's preference; `Command` gets no separate function. `Permission` is `Allowed \| Refused { blocked_by: SessionMode, allowed_in: SessionMode }`, matching the sketch. `ToolAction`, `CommandRequest`, and `ProposedStep` were module-private to `chat.rs` and had to be widened to `pub(crate)` (with `CommandRequest`'s fields also `pub(crate)`) for `mode.rs` to see them — no other file referenced them before this change. Two field-name corrections against tasks.md's own sketch, confirmed by reading the real types before writing the test: `GeneratedEdit` has `changes: Vec<ProposedChange>`, not `files`; `WebDiagnosticCall` is a struct (`kind: WebDiagnosticKind, url, arguments_json, session_id, task_id`), not an enum with an `InspectPage` variant. `SessionMode`, `Permission`, `mode_permits`, and `Permission::is_allowed` carry `#[allow(dead_code)]` with a comment pointing at Task 3, since this task is self-contained by design and nothing outside its own tests calls them yet — `cargo clippy -D warnings` fails without it. Mutation test: flipped the `ProposePatch`/`EditFile` arm to also allow `Ask`, confirmed `the_permission_matrix_matches_the_spec_table` fails (`assertion failed: !mode_permits(Ask, action, None).is_allowed()`), reverted. All 9 `mode::tests` pass; `cargo fmt --all -- --check` and `cargo clippy -p workspace-engine --all-targets --locked -- -D warnings` both clean. |
| 2 · MCP read-only capability | Done | `McpTool.read_only_hint: Option<bool>` added; `list_tools`'s inline loop extracted into a private pure `parse_mcp_tool(item: &Value) -> Option<McpTool>` (4 unit tests, no subprocess). `McpRuntime::tool_read_only_hint(server_id, tool_name) -> Option<bool>` added near `requires_approval`; unused by any caller until Task 3/6 wire it in, so no `#[allow(dead_code)]` was needed since it's `pub`. `mode_permits` widened to a 4th `mcp_tool_read_only: Option<bool>` parameter (Task 2's own decision, per its row's Interfaces note); `McpCall` arm now `mcp_tool_read_only == Some(true) \|\| mode == Code`. Every existing `mode.rs` test call site updated to pass the new parameter (compiler-named, all `None` except the new MCP cases). Mutation test: made the arm also treat `None` as permitting outside Code, confirmed `an_mcp_call_with_no_read_only_signal_is_treated_as_mutation_class` fails with `assertion failed: !mode_permits(SessionMode::Ask, &action, None, None).is_allowed()`, reverted. All 22 `mode::tests` + `mcp::tests` pass; `cargo fmt --all -- --check` (after one `cargo fmt --all` pass) and `cargo clippy -p workspace-engine --all-targets --locked -- -D warnings` both clean. `chat.rs` untouched, as scoped. |
| 3 · Layer 1 — tool-list construction filters by mode | Not started | |
| 4 · Persistence — `SessionStore::set_session_mode` / `session_mode` | Not started | |
| 5 · Layer 2 — the non-native fallback's system-prompt envelopes | Not started | |
| 6 · Layer 3 — the orchestrator refuses at every action path | Not started | |
| 7 · Command-allowlist does not widen a mode | Not started | |
| 8 · UI — mode control, refusal messaging, Plan→Code continuity | Not started | |
| 9 · Migration and eval-harness guard | Not started | |
| 10 · Docs, acceptance criteria, close the slice | Not started | |

**Goal:** Give every session one of four modes (Ask, Plan, Code, Review) that
structurally bounds what the model can do — not what it is told to do — and
enforce that boundary at tool-list construction, the non-native text-envelope
fallback, and the orchestrator itself, so no single missed layer is a hole.

**Architecture:** One `SessionMode` enum and one pure function
(`mode_permits(mode, &ToolAction) -> Permission` or equivalent — Task 1
names it) are the single source of truth the other nine tasks all call
through. Layer 1 (chat.rs's tool-list construction) and Layer 3 (every
action-dispatch arm) both call it; nothing re-implements the matrix. Mode is
persisted the way `browser_diagnostics_allowed_for_session` already is —
append an event, replay the newest.

**Tech Stack:** Rust 2024 (workspace edition), no new dependencies. `app.js`
gains a mode control; no new web dependency.

## Global Constraints

Every task's requirements implicitly include this section.

- **Read [`context.md`](context.md) in full before Task 1.** It corrects
  `proposal.md` §2's tool inventory and §5.1's matrix against the current
  code, and records four decisions (§1's tool-class mapping, §2's MCP
  read-only field, §3's two-refusal-point reading of Layer 3, §5's open
  question about the matrix function's signature) that every later task
  depends on. Do not re-derive them from the flat spec alone.
- **One matrix, one function.** `proposal.md` §5.1: "expressed once, in code,
  as a function of mode and tool class — not duplicated across call sites."
  If a task finds itself writing a second `match mode { ... }` over tool
  identity anywhere outside Task 1's function, it has duplicated the matrix —
  stop and call the existing function instead.
- **Withholding is not enforcement.** `proposal.md` §5.2: Layer 1 (tool-list
  filtering) and Layer 2 (envelope omission) are UX and token-budget
  concerns. Layer 3 (the orchestrator refusing at the action-dispatch site)
  is what acceptance criteria are asserted against. A task that implements
  Layer 1 or 2 without Layer 3 has not enforced anything yet.
- **Nothing the model emits changes mode.** Requirement 4, asserted directly
  by a test in Task 6 where a model output requests a mode change and
  nothing happens.
- **`AGENTS.md` is data with respect to capability.** Requirement 5,
  `proposal.md` §5.5. No task reads repository-scoped instruction content
  when building a tool list or refusing an action.
- **Mode and approval are different axes.** Non-goals, `proposal.md` §4. No
  task touches `require_approval_for_file_edits`,
  `require_approval_for_risky_commands`, `require_approval_for_all_commands`,
  or `CommandPolicy`'s classification, blocklist, or allowlist semantics.
  Code mode with all-commands-approval on is a valid, unchanged combination.
- **A turn captures its mode at start.** `proposal.md` §5.4: a mode change
  mid-turn does not take effect until the next turn. Task 4 persists the
  event; Task 3/Task 6 read it once per turn, not per action.
- **Falsify every load-bearing test.** Break what it guards and confirm it
  fails, per this repository's history of tests that passed without testing
  anything (spec 47 §7.2, spec 21's error rate, spec 49's mutation-tested
  invariants). The matrix crossing-test in Task 1 is the work package's
  primary artifact per `proposal.md` §6 — it earns the most scrutiny.
- **Scope the per-task checks; run the full seven-command gate from
  `AGENTS.md` once, at the end** (Task 10). `cargo nextest run --workspace`
  takes about 5 minutes locally and `cargo clippy --workspace --all-targets`
  up to 18 minutes cold — see `AGENTS.md`'s own measured figures.
- **`chat.rs` is contended.** `docs/specs/README.md`'s "What to build next"
  → Parallel work: this spec is the one currently holding `chat.rs`. Do not
  start a second spec that also touches it (`#56`, `#26`, `#55`) in a
  parallel worktree while this is in flight, per that section's "Not
  together" / "Never together" rules.
- **Never `git commit` unasked.** Each task ends by showing the change and
  the scoped check result and asking. One subject line when asked, no body.

## File Structure

| File | Change |
|---|---|
| `crates/workspace-engine/src/mode.rs` | New: `SessionMode`, the permission function, its crossing test |
| `crates/workspace-engine/src/mcp.rs` | `McpTool.read_only_hint: Option<bool>`, parsed in `list_tools` |
| `crates/workspace-engine/src/chat.rs` | Layer 1 (tool-list filtering, `run_agentic_turn`), Layer 2 (system-prompt envelope omission), Layer 3 (every `ToolAction` dispatch arm), mode-change-request test fixture |
| `crates/workspace-engine/src/session.rs` | `set_session_mode`, `session_mode`, the `session_mode_set` event |
| `crates/workspace-engine/src/edit.rs` | Layer 3 for `PatchEngine::apply_patch`'s caller (the second of context.md §3's two refusal points) |
| `crates/workspace-engine/src/command_policy.rs` | No change — Task 7 only reads `risk`/`requires_approval`, per Global Constraints |
| `crates/desktop-shell/src/lib.rs` | Session-mode read/write endpoints, mode in the session JSON, refusal messages carrying blocking/allowing mode |
| `crates/desktop-shell/static/app.js` | Mode control in the conversation header, refusal messaging, Plan→Code continuity |
| `crates/eval-harness/src/{metrics,record}.rs` | Acceptance criterion: "no increase in approval-policy violations" — Task 9 |
| `docs/USER_GUIDE.md`, `docs/TROUBLESHOOTING.md` | `proposal.md` §5.8's documentation obligation — Task 10 |

## Interface reference

Read before starting; each is load-bearing for a task below.

- `enum ToolAction` (`chat.rs:3237-3278`) — the exhaustive current tool
  inventory; Task 1 matches on it directly (`context.md` §1).
- `native_tools` construction (`chat.rs:1267-1293`, inside
  `run_agentic_turn`) — Layer 1's edit site.
- `CommandClassification { risk: CommandRisk, requires_approval: bool, .. }`
  (`command_policy.rs:24-32`), `CommandPolicy::classify`
  (`command_policy.rs:60`) — Task 7 reads these two fields only.
- `McpTool { name, description, input_schema_json }`
  (`mcp.rs:102-108`), `McpClient::list_tools` (`mcp.rs:178-203`) — Task 2's
  edit site.
- `SessionStore::allow_browser_diagnostics_for_session` /
  `browser_diagnostics_allowed_for_session` (`session.rs:664-695`) — the
  persistence pattern Task 4 follows exactly (append an event, replay by
  parsed `eventType`, newest wins).
- `PatchEngine::apply_patch` caller (`edit.rs:538`) and the `ProposePatch` /
  `EditFile` dispatch arms that create a proposal before any approval exists
  (`chat.rs:2019-2058`, `chat.rs:2195-2210`) — Task 6's two refusal points
  for the mutation-proposal class, per `context.md` §3.
- `ValidationOrchestrator::run_proposal` (`validation.rs:206`) — Task 6's
  refusal point for command execution.
- `web_diagnostics_runner` call sites (`chat.rs:497`, `chat.rs:1281`) —
  Task 6's refusal point for browser diagnostics.
- `mcp.call_tool` dispatch (`chat.rs:2400`, and the resumed-call path at
  `chat.rs:816`) — Task 6's refusal point for MCP tools.
- `parsed_events` / `active_events` (`session.rs:1904`, `session.rs:1931`) —
  what Task 4's reader calls; confirmed already free of the substring-match
  pattern `proposal.md` §5.4 warns against (`context.md` §4).
- `system_prompt()` and the `DAMAIAN_EDIT_V1` / `DAMAIAN_COMMAND_V1`
  instruction blocks it assembles — Task 5's edit site; grep `chat.rs` for
  `DAMAIAN_EDIT_V1` to find the current assembly point before editing it.
  **Spec 49 Task 8 already pinned this function's output as a byte-stable
  prefix** (`crates/workspace-engine/tests/prompt_cache.rs`); Task 5 must
  keep the mode-dependent parts *outside* that stable prefix or explain why
  varying it there is acceptable, since those guards will start failing
  otherwise and that is a real signal, not a nuisance.

---

## Task 1: `SessionMode` and the permission matrix

**Requirements:** 1, 2, 6. **Files:** new `crates/workspace-engine/src/mode.rs`.

This task is entirely self-contained — no wiring into `chat.rs`, `session.rs`,
or anywhere else. It produces one pure function and proves the matrix against
every current tool. Nothing calls it yet; that starts at Task 3.

**Interfaces:**
- Consumes: `ToolAction` (`chat.rs:3237-3278`, read-only — this task does not
  modify it), `CommandClassification` (`command_policy.rs:24-32`, read-only).
- Produces: `SessionMode` (`Ask`/`Plan`/`Code`/`Review`, `Serialize` +
  `Deserialize` with `#[serde(rename_all = "snake_case")]`, per `proposal.md`
  §5.1's exact definition) and a permission function every later task calls
  through. This task decides and records the function's exact name and
  signature — see Step 3 — so later tasks can cite it precisely instead of
  re-deriving it.

- [x] **Step 1: Write the failing tests**

  Create `crates/workspace-engine/src/mode.rs` with `#[cfg(test)] mod tests`
  below the (not-yet-existing) implementation. Write these first so they fail
  to compile:

  ```rust
  #[cfg(test)]
  mod tests {
      use super::*;
      use crate::chat::ToolAction; // adjust visibility if ToolAction is
                                    // private to chat.rs — see Step 3's note.
      use crate::command_policy::{CommandClassification, CommandRisk};

      fn read_only_command() -> CommandClassification {
          CommandClassification {
              command: "git status".to_string(),
              risk: CommandRisk::Low,
              blocked: false,
              requires_approval: false,
              reasons: vec![],
              expected_effects: String::new(),
              may_use_network: false,
          }
      }

      fn approval_required_command() -> CommandClassification {
          CommandClassification {
              command: "npm install".to_string(),
              risk: CommandRisk::Medium,
              blocked: false,
              requires_approval: true,
              reasons: vec![],
              expected_effects: String::new(),
              may_use_network: true,
          }
      }

      /// The work package's primary artifact per `proposal.md` §6: every
      /// mode crossed with every current tool class, asserting allowed or
      /// refused. Table source: `proposal.md` §5.1 plus `context.md` §1's
      /// extension for the five tools added since the flat spec was written.
      #[test]
      fn the_permission_matrix_matches_the_spec_table() {
          use SessionMode::*;
          let read_actions = [
              ToolAction::ReadFile { path: "x".into(), range: None },
              ToolAction::ListDirectory { dir: None, depth: None },
              ToolAction::SearchContent {
                  pattern: "x".into(),
                  path_glob: None,
                  max_matches: None,
              },
              ToolAction::SearchCodebase {
                  query: "x".into(),
                  semantic: false,
                  limit: 8,
              },
              ToolAction::ReadGitStatus,
              ToolAction::ReadGitDiff { staged: false },
          ];
          for action in &read_actions {
              for mode in [Ask, Plan, Code, Review] {
                  assert!(
                      mode_permits(mode, action, None).is_allowed(),
                      "{mode:?} should permit {action:?}"
                  );
              }
          }

          let mutation_actions = [
              ToolAction::ProposePatch(GeneratedEdit {
                  summary: "x".into(),
                  files: vec![],
              }),
              ToolAction::EditFile { summary: "x".into(), edits: vec![] },
          ];
          for action in &mutation_actions {
              assert!(!mode_permits(Ask, action, None).is_allowed());
              assert!(!mode_permits(Plan, action, None).is_allowed());
              assert!(mode_permits(Code, action, None).is_allowed());
              assert!(!mode_permits(Review, action, None).is_allowed());
          }

          let planning_actions = [
              ToolAction::ProposePlan(vec![]),
              ToolAction::CompleteStep,
          ];
          for action in &planning_actions {
              assert!(!mode_permits(Ask, action, None).is_allowed());
              assert!(mode_permits(Plan, action, None).is_allowed());
              assert!(mode_permits(Code, action, None).is_allowed());
              assert!(!mode_permits(Review, action, None).is_allowed());
          }

          let web_action = ToolAction::WebDiagnostic(WebDiagnosticCall::InspectPage {
              url: "http://localhost".into(),
              viewport: None,
              wait_ms: None,
              capture: Default::default(),
          });
          assert!(!mode_permits(Ask, &web_action, None).is_allowed());
          assert!(!mode_permits(Plan, &web_action, None).is_allowed());
          assert!(mode_permits(Code, &web_action, None).is_allowed());
          assert!(mode_permits(Review, &web_action, None).is_allowed());
      }

      #[test]
      fn ask_offers_no_commands_at_all_not_even_read_only_ones() {
          let action = ToolAction::Command(CommandRequest {
              command: "git status".into(),
              reason: String::new(),
          });
          assert!(!mode_permits(SessionMode::Ask, &action, Some(&read_only_command()))
              .is_allowed());
      }

      #[test]
      fn plan_permits_a_read_only_command_that_needs_no_approval() {
          let action = ToolAction::Command(CommandRequest {
              command: "git status".into(),
              reason: String::new(),
          });
          assert!(mode_permits(SessionMode::Plan, &action, Some(&read_only_command()))
              .is_allowed());
      }

      /// §5.3's sharpest case: a command that would need approval is refused
      /// outright in Plan, never turned into an approval card.
      #[test]
      fn plan_refuses_a_command_that_would_require_approval_even_if_low_risk() {
          let mut classification = read_only_command();
          classification.requires_approval = true;
          let action = ToolAction::Command(CommandRequest {
              command: "git status".into(),
              reason: String::new(),
          });
          assert!(!mode_permits(SessionMode::Plan, &action, Some(&classification))
              .is_allowed());
      }

      #[test]
      fn plan_refuses_a_mutating_command() {
          let action = ToolAction::Command(CommandRequest {
              command: "npm install".into(),
              reason: String::new(),
          });
          assert!(
              !mode_permits(SessionMode::Plan, &action, Some(&approval_required_command()))
                  .is_allowed()
          );
      }

      #[test]
      fn code_permits_any_command_classification() {
          let action = ToolAction::Command(CommandRequest {
              command: "npm install".into(),
              reason: String::new(),
          });
          assert!(
              mode_permits(SessionMode::Code, &action, Some(&approval_required_command()))
                  .is_allowed()
          );
      }

      #[test]
      fn review_permits_a_read_only_command_but_not_a_mutating_one() {
          let read = ToolAction::Command(CommandRequest {
              command: "git status".into(),
              reason: String::new(),
          });
          let mutate = ToolAction::Command(CommandRequest {
              command: "npm install".into(),
              reason: String::new(),
          });
          assert!(mode_permits(SessionMode::Review, &read, Some(&read_only_command()))
              .is_allowed());
          assert!(!mode_permits(
              SessionMode::Review,
              &mutate,
              Some(&approval_required_command())
          )
          .is_allowed());
      }

      /// A refusal names both the mode that blocked the action and the mode
      /// that would allow it (`proposal.md` §5.6), so the UI never has to
      /// re-derive that pairing from the matrix itself.
      #[test]
      fn a_refusal_names_the_blocking_mode_and_the_permitting_mode() {
          let action = ToolAction::ProposePatch(GeneratedEdit {
              summary: "x".into(),
              files: vec![],
          });
          let Permission::Refused { blocked_by, allowed_in } =
              mode_permits(SessionMode::Ask, &action, None)
          else {
              panic!("expected a refusal");
          };
          assert_eq!(blocked_by, SessionMode::Ask);
          assert_eq!(allowed_in, SessionMode::Code);
      }

      /// `run_command` needs the command's own classification to decide,
      /// unlike every other tool — the compile-time proof that a caller
      /// cannot omit it for a `Command` action and get a silent default.
      #[test]
      fn a_command_action_without_a_classification_panics_rather_than_defaulting() {
          let action = ToolAction::Command(CommandRequest {
              command: "git status".into(),
              reason: String::new(),
          });
          let result = std::panic::catch_unwind(|| {
              mode_permits(SessionMode::Plan, &action, None)
          });
          assert!(result.is_err());
      }
  }
  ```

  Adjust the exact `ToolAction`/`WebDiagnosticCall`/`CommandRequest`/
  `GeneratedEdit` construction to match their real field names — read
  `chat.rs:3237-3400` for the authoritative shapes before writing this;
  the sketch above may not match field-for-field.

- [x] **Step 2: Run to verify they fail to compile**

  `cargo nextest run -p workspace-engine -E 'test(mode)'` — expect a compile
  error (`mode` module and `mode_permits` do not exist yet).

- [x] **Step 3: Decide and record the function's shape**

  Options, per `context.md` §6:
  - `fn mode_permits(mode: SessionMode, action: &ToolAction, command: Option<&CommandClassification>) -> Permission`
    — matches on `ToolAction` directly. `command` is `Some` only for
    `ToolAction::Command`, and Step 1's last test pins that a `Command`
    action with `None` panics rather than silently defaulting — the
    signature *could* make this impossible by giving `Command` its own
    function, but a single function matching the whole enum is what keeps
    "one matrix, one function" true without a second dispatch layer at every
    call site. Prefer this.
  - Returning `Permission { Allowed, Refused { blocked_by: SessionMode,
    allowed_in: SessionMode } }` rather than `bool`, because Layer 3's
    refusal message (`proposal.md` §5.6, acceptance criteria) needs
    `allowed_in` and computing it after the fact at every call site would be
    the exact duplication this task exists to prevent. `allowed_in` is the
    *most permissive* mode that allows the action — for the `run_command`
    row this still depends on the command's classification, so `allowed_in`
    for a mutating command is `Code` even though Plan would allow it *if*
    the command were read-only; do not compute a wrong answer to avoid this
    nuance, name it in a doc comment instead.
  - `ToolAction` currently lives in `chat.rs` and may be private to that
    module (`enum ToolAction` at `chat.rs:3237` has no visibility modifier
    shown in the grep — check `pub(crate)` or bare `enum` before assuming
    `mode.rs` can see it as a separate module; make it `pub(crate)` if it
    is not already, rather than duplicating its variants).

  Record the decision taken in this Progress table's row for Task 1, the way
  spec 49 Task 2 recorded its `ReportedUsage` decision — a future task
  reads this row instead of re-deriving the signature from the test file.

- [x] **Step 4: Implement `SessionMode` and `mode_permits`**

  ```rust
  use crate::chat::ToolAction;
  use crate::command_policy::{CommandClassification, CommandRisk};
  use serde::{Deserialize, Serialize};

  #[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
  #[serde(rename_all = "snake_case")]
  pub enum SessionMode {
      Ask,
      Plan,
      Code,
      Review,
  }

  #[derive(Debug, Clone, Copy, PartialEq, Eq)]
  pub enum Permission {
      Allowed,
      Refused {
          blocked_by: SessionMode,
          allowed_in: SessionMode,
      },
  }

  impl Permission {
      pub fn is_allowed(&self) -> bool {
          matches!(self, Permission::Allowed)
      }
  }

  /// The permission matrix from `proposal.md` §5.1, extended per
  /// `context.md` §1. The single place mode and tool identity are crossed —
  /// every other layer calls this rather than re-implementing any part of
  /// it.
  pub fn mode_permits(
      mode: SessionMode,
      action: &ToolAction,
      command: Option<&CommandClassification>,
  ) -> Permission {
      use SessionMode::*;

      let allow_in = |allowed_in: SessionMode| {
          if mode == Code || allowed_in == mode {
              Permission::Allowed
          } else {
              Permission::Refused { blocked_by: mode, allowed_in }
          }
      };

      match action {
          ToolAction::ReadFile { .. }
          | ToolAction::ListDirectory { .. }
          | ToolAction::SearchContent { .. }
          | ToolAction::SearchCodebase { .. }
          | ToolAction::ReadGitStatus
          | ToolAction::ReadGitDiff { .. } => Permission::Allowed,

          ToolAction::ProposePatch(_) | ToolAction::EditFile { .. } => {
              if mode == Code {
                  Permission::Allowed
              } else {
                  Permission::Refused { blocked_by: mode, allowed_in: Code }
              }
          }

          ToolAction::ProposePlan(_) | ToolAction::CompleteStep => match mode {
              Plan | Code => Permission::Allowed,
              Ask | Review => {
                  Permission::Refused { blocked_by: mode, allowed_in: Plan }
              }
          },

          ToolAction::Command(_) => {
              let classification = command.expect(
                  "mode_permits called with ToolAction::Command and no \
                   CommandClassification — every Command call site must \
                   classify the command before asking whether the mode \
                   permits it",
              );
              let read_only_no_approval =
                  classification.risk == CommandRisk::Low
                      && !classification.requires_approval;
              match mode {
                  Ask => {
                      Permission::Refused { blocked_by: Ask, allowed_in: Plan }
                  }
                  Code => Permission::Allowed,
                  Plan | Review if read_only_no_approval => Permission::Allowed,
                  Plan | Review => {
                      Permission::Refused { blocked_by: mode, allowed_in: Code }
                  }
              }
          }

          ToolAction::WebDiagnostic(_) => match mode {
              Code | Review => Permission::Allowed,
              Ask | Plan => {
                  Permission::Refused { blocked_by: mode, allowed_in: Code }
              }
          },

          // Task 2 replaces this arm with the read-only-hint check once
          // `McpTool.read_only_hint` exists; until then every MCP call is
          // treated as mutation-class, the conservative default `context.md`
          // §2 chose.
          ToolAction::McpCall { .. } => allow_in(Code),
      }
  }
  ```

  This implementation is a starting point, not a transcription to follow
  blindly — make it satisfy Step 1's tests, and if a test and this sketch
  disagree, the test (derived from `proposal.md` §5.1 and `context.md` §1)
  is correct and the sketch is not. `allow_in`'s closure is unused by most
  arms above; remove it if the final implementation does not need it, or use
  it consistently — do not leave a half-used helper.

- [x] **Step 5: Run tests to verify they pass**

  `cargo nextest run -p workspace-engine -E 'test(mode)'`

- [x] **Step 6: Mutation-test the matrix**

  Temporarily flip one arm (e.g. make `Ask` permit `ProposePatch`) and
  confirm `the_permission_matrix_matches_the_spec_table` fails. Revert.
  This is the work package's primary artifact per `proposal.md` §6 — it must
  be provably capable of failing.

- [x] **Step 7: Scoped checks**

  `cargo nextest run -p workspace-engine -E 'test(mode)'`, `cargo fmt`,
  `cargo clippy -p workspace-engine --all-targets --locked -- -D warnings`.

- [x] **Step 8: Show the change and the check result, and ask before committing**

---

## Task 2: MCP read-only capability

**Requirements:** 2, 6. **Files:** `mcp.rs`, `mode.rs`.

Adds `read_only_hint: Option<bool>` to `McpTool`, parsed from the MCP
protocol's `annotations.readOnlyHint` in `list_tools`, per `context.md` §2's
decision. Updates `mode_permits`'s `McpCall` arm to read it instead of the
conservative `allow_in(Code)` placeholder Task 1 left there. A server that
omits the annotation or sets it `false` stays mutation-class — silence is
not read-only, the same posture spec 49 used for cache reporting.

This task does **not** touch `chat.rs`. `McpRuntime` already caches the raw
`McpTool` list per server in `tools: HashMap<String, Vec<McpTool>>`
(`mcp.rs:612`) — separately from `tool_definitions()`'s `Vec<ToolDefinition>`
output, which is the generic `{name, description, parameters_json}` shape
every tool kind flattens to and has no room for a hint. This task adds a
lookup over that existing cache; wiring the lookup into `chat.rs`'s
`McpCall` dispatch is Task 3/Task 6's job, once Layer 1 and Layer 3 exist to
call it from. `mode_permits`'s signature changes regardless, since Task 1
already established that a `ToolAction` variant needing extra context to
decide gets that context as a parameter (`command: Option<&CommandClassification>`
for `Command`) rather than the matrix growing a special case.

**Interfaces:**
- Consumes: `McpRuntime.tools: HashMap<String, Vec<McpTool>>` (`mcp.rs:612`,
  private field — add a method rather than widening its visibility),
  `McpClient::list_tools` (`mcp.rs:178-203`).
- Produces: `McpTool.read_only_hint: Option<bool>`, a new
  `McpRuntime::tool_read_only_hint(&self, server_id: &str, tool_name: &str) -> Option<bool>`
  method Task 3/6 will call, and `mode_permits`'s widened signature —
  `mcp_tool_read_only: Option<bool>` alongside `command`. Record in this
  row if the parameter list grows differently.

- [x] **Step 1: Write the failing tests**

  `list_tools` (`mcp.rs:178-203`) has no dedicated unit tests today —
  it is exercised only through
  `mcp_stdio_client_handshakes_lists_and_calls_tools`
  (`crates/workspace-engine/tests/foundation.rs:4729`), a fake stdio
  subprocess (a shell script that replies to `tools/list` with canned JSON)
  driven through the real `McpClient`. Spinning up a subprocess per parsing
  variant is the wrong shape for four small JSON-shape cases. Extract the
  per-item parsing `list_tools` already does into a private pure function
  first — `fn parse_mcp_tool(item: &serde_json::Value) -> Option<McpTool>`,
  called once per array element inside `list_tools` — so these tests can be
  ordinary unit tests in `mcp.rs`'s own `mod tests` (`mcp.rs:780`) against a
  literal `serde_json::json!({...})` value, no subprocess required:
  - `parse_mcp_tool_reads_a_true_read_only_hint` — an item with
    `"annotations":{"readOnlyHint":true}` parses to
    `read_only_hint == Some(true)`.
  - `parse_mcp_tool_reads_a_false_read_only_hint` — `readOnlyHint:false`
    parses to `Some(false)`, not dropped or defaulted.
  - `parse_mcp_tool_with_no_annotations_object_is_none` — an item with no
    `annotations` key at all parses to `None`. This is the
    silence-is-not-read-only case; the test that would fail first if a
    future edit defaulted it to `Some(false)` or `Some(true)` instead of
    leaving it unknown.
  - `parse_mcp_tool_with_annotations_but_no_read_only_hint_is_none` — an
    `annotations` object present but without a `readOnlyHint` key (a server
    that sets some other annotation) also parses to `None` — distinguishing
    "no annotations sent" from "annotations sent, this one absent" is not
    required by anything downstream, so both collapse to `None`; this test
    pins that they do, rather than leaving it to be discovered as a gap
    later.

  Leave `mcp_stdio_client_handshakes_lists_and_calls_tools` as the one
  existing integration test that still exercises `list_tools` end to end
  through the real subprocess-and-transport path — it doesn't need a
  read-only-hint case added, since the unit tests above now own that
  question at the parsing layer, and duplicating it there would test the
  transport twice for no new information.

  In `mode.rs`'s test module:
  - Extend `the_permission_matrix_matches_the_spec_table`'s (currently
    absent) MCP coverage: a `ToolAction::McpCall` with
    `mcp_tool_read_only: Some(true)` is allowed in all four modes; with
    `Some(false)` or `None`, it follows the same allowed-in-Code-only shape
    the placeholder already has.
  - `an_mcp_call_with_no_read_only_signal_is_treated_as_mutation_class` —
    `mcp_tool_read_only: None` refused in Ask/Plan/Review, the explicit
    silence-is-not-a-green-light regression guard, named separately from
    the crossing test so a future reader finds the reasoning attached to
    the case that most needs it.

- [x] **Step 2: Run to verify they fail**

  `cargo nextest run -p workspace-engine -E 'test(read_only) + test(mcp_call)'`

- [x] **Step 3: Implement `mcp.rs`**

  Add the field:

  ```rust
  pub struct McpTool {
      pub name: String,
      pub description: String,
      pub input_schema_json: String,
      /// The server's own claim about whether calling this tool has side
      /// effects, from `tools/list`'s optional `annotations.readOnlyHint`.
      /// `None` means the server made no claim either way — not "not
      /// read-only" and not "read-only". A hint, per the MCP spec, not a
      /// guarantee; `mode_permits` (`mode.rs`) is what turns it into an
      /// enforced boundary, and only a `Some(true)` widens what a
      /// capability-restricted mode offers.
      pub read_only_hint: Option<bool>,
  }
  ```

  Extract the per-item parsing into a private function and call it from
  `list_tools` (`mcp.rs:178-203`) in place of the inline loop body:

  ```rust
  fn parse_mcp_tool(item: &Value) -> Option<McpTool> {
      let name = item.get("name").and_then(Value::as_str)?.to_string();
      let description = item
          .get("description")
          .and_then(Value::as_str)
          .unwrap_or("")
          .to_string();
      let input_schema = item
          .get("inputSchema")
          .cloned()
          .unwrap_or_else(|| json!({ "type": "object" }));
      let read_only_hint = item
          .get("annotations")
          .and_then(|a| a.get("readOnlyHint"))
          .and_then(Value::as_bool);
      Some(McpTool {
          name,
          description,
          input_schema_json: input_schema.to_string(),
          read_only_hint,
      })
  }
  ```

  `list_tools` becomes `items.iter().filter_map(parse_mcp_tool).collect()`
  (its existing `continue`-on-missing-name behavior is exactly what
  `filter_map` over an `Option`-returning function gives for free — confirm
  this before assuming it, `list_tools`'s current loop may do something
  slightly different worth preserving).

  Add the lookup method near `McpRuntime`'s other accessors
  (`requires_approval`, `mcp.rs:662`):

  ```rust
  /// The server's own read-only claim for one of its tools, from the
  /// cached `tools/list` response — `None` both when the server made no
  /// claim and when the tool or server is unknown to this runtime. Callers
  /// that need to distinguish "unknown" from "known but unclaimed" should
  /// not use this method; nothing downstream needs that distinction today.
  pub fn tool_read_only_hint(&self, server_id: &str, tool_name: &str) -> Option<bool> {
      self.tools
          .get(server_id)?
          .iter()
          .find(|tool| tool.name == tool_name)?
          .read_only_hint
  }
  ```

  Fix every other `McpTool { .. }` construction site the compiler names
  (test fixtures almost certainly construct it by struct literal) to add
  `read_only_hint: None` unless that fixture is specifically testing the
  hint.

- [x] **Step 4: Widen `mode_permits` and implement the real `McpCall` arm**

  ```rust
  pub(crate) fn mode_permits(
      mode: SessionMode,
      action: &ToolAction,
      command: Option<&CommandClassification>,
      mcp_tool_read_only: Option<bool>,
  ) -> Permission {
      // ...
      ToolAction::McpCall { .. } => {
          if mcp_tool_read_only == Some(true) {
              Permission::Allowed
          } else if mode == Code {
              Permission::Allowed
          } else {
              Permission::Refused { blocked_by: mode, allowed_in: Code }
          }
      }
  }
  ```

  Update every existing call site of `mode_permits` in `mode.rs`'s own
  tests to pass `None` for the new parameter where the action under test
  is not `McpCall` — the compiler will name each one. Remove the
  `#[allow(dead_code)]`-adjacent "Task 2 replaces this arm" comment Task 1
  left, since this task is what replaces it.

- [x] **Step 5: Run tests to verify they pass**

  `cargo nextest run -p workspace-engine -E 'test(mode) + test(mcp)'`

- [x] **Step 6: Mutation-test the silence case**

  Temporarily change the `McpCall` arm to treat `None` the same as
  `Some(true)` (i.e. delete the `mcp_tool_read_only == Some(true)`
  distinction and allow whenever `mode != Code` is the only gate removed —
  concretely, make `None` also permit outside Code) and confirm
  `an_mcp_call_with_no_read_only_signal_is_treated_as_mutation_class` fails.
  Revert.

- [x] **Step 7: Scoped checks**

  `cargo nextest run -p workspace-engine -E 'test(mode) + test(mcp)'`,
  `cargo fmt`, `cargo clippy -p workspace-engine --all-targets --locked --
  -D warnings`.

- [x] **Step 8: Show the change and the check result, and ask before committing**

## Task 3: Layer 1 — tool-list construction filters by mode

**Requirements:** 2, 6. **Files:** `chat.rs`.

Filters `native_tools` (`chat.rs:1267-1293`) by `mode_permits`, reading the
turn's captured mode (Task 4 supplies the read; this task consumes it, does
not yet persist it). For `run_command`, whether the tool definition itself is
offered cannot depend on a specific command's classification — the decision
here is coarser: offer `run_command_tool_definition()` whenever *some*
command could pass in this mode (i.e., not in Ask), and let Layer 3 refuse
individual commands. Acceptance criterion: "no tool capable of mutation
appears in the tool list sent to the model — asserted against the constructed
list, not the prompt," in Ask and Plan.

## Task 4: Persistence

**Requirements:** 3, 4, 7. **Files:** `session.rs`.

`SessionStore::set_session_mode(session_id, mode, set_by)` and
`session_mode(session_id) -> SessionMode`, following
`allow_browser_diagnostics_for_session` / `browser_diagnostics_allowed_for_session`
(`session.rs:664-695`) exactly: append a `session_mode_set` event
(`{"sessionId":...,"mode":"code","setBy":"user"}`), read by newest-event-wins
over parsed `eventType`. Default for no event is `Code` (requirement 7).
`set_by` is always `"user"` and is asserted as such, per `proposal.md` §5.4.

## Task 5: Layer 2 — the non-native fallback's envelopes

**Requirements:** 2, 6. **Files:** `chat.rs` (`system_prompt()` and its
callers).

Omits `DAMAIAN_EDIT_V1` from the system prompt in Ask, Plan, Review. Omits
`DAMAIAN_COMMAND_V1` in Ask; restricts its wording to read-only commands in
Plan and Review. **Must not break spec 49 Task 8's prefix-stability guards**
(`crates/workspace-engine/tests/prompt_cache.rs`) — those assert the prefix
is identical between two turns of an *unchanged* mode; a mode change between
turns is expected to change the prefix, and the guard tests do not exercise
that case, so this task does not need to preserve stability across a mode
change, only within one.

## Task 6: Layer 3 — the orchestrator refuses

**Requirements:** 2, 4, 5, 6. **Files:** `chat.rs`, `edit.rs`, `validation.rs`.

The five refusal points from `proposal.md` §5.2's table, corrected to six
call sites by `context.md` §3 (the mutation-proposal class needs both its
creation site in `chat.rs` and its apply site in `edit.rs`). Every path
checks `mode_permits` immediately before acting and refuses with the
`blocked_by`/`allowed_in` message from Task 1's `Permission::Refused`. This
is the task the acceptance criteria are mostly asserted against, including
the `DAMAIAN_EDIT_V1`-emitted-without-being-offered test via
`MockModelAdapter`, and the mid-turn mode-change-has-no-effect test.

## Task 7: Command-allowlist does not widen a mode

**Requirements:** 6. **Files:** `chat.rs` or wherever `command_allowlist` /
`Allow Always` is currently consulted.

Asserts directly: an allowlisted `npm run build` is still refused in Ask and
Plan. `context.md` §5 confirms no change to `command_policy.rs` itself is
needed — this task only confirms the allowlist check and the mode check are
independent gates, both required, per `proposal.md` §5.3.

## Task 8: UI — mode control and refusal messaging

**Requirements:** 3, 6. **Files:** `desktop-shell/src/lib.rs`,
`static/app.js`.

Mode control in the conversation header (always visible, not a settings
panel). Switching to a more permissive mode is explicit; switching to a more
restrictive one needs no confirmation. A refusal surfaces `blocked_by` and
`allowed_in`. A plan made in Plan mode stays intact across a Plan→Code
switch (spec 21 owns the plan's persistence; this task only confirms the
switch does not clear it).

## Task 9: Migration and eval-harness guard

**Requirements:** 7, plus the harness half of acceptance. **Files:**
`session.rs` (covered by Task 4's default), `eval-harness`.

Confirms existing sessions with no `session_mode_set` event load in Code.
Confirms the spec 18 baseline shows no increase in approval-policy
violations with modes active (acceptance criteria, last bullet) — likely a
new deterministic-tier scenario or an assertion added to an existing one;
decide which during this task and record why.

## Task 10: Docs, acceptance criteria, close the slice

Same shape as spec 49 Task 10: `docs/USER_GUIDE.md` (the four modes, the
matrix in user-facing terms, why an allowlisted command is still refused in
Ask/Plan), `docs/TROUBLESHOOTING.md` (mode refusal vs. policy refusal, where
the mode event lives in the session log), walk `proposal.md` §6's acceptance
list naming the test for each, update the four status records (`proposal.md`
Status:, `docs/specs/README.md` row, this file's Progress table and
Started/Done header), run the full seven-command gate, and follow
`AGENTS.md`'s "When a spec becomes Done" checklist — `grep -rn
"20_working_modes" docs/specs/*.md docs/specs/*/*.md` to find every
`Depends on:` line this unblocks (spec 31 permission profiles and others
found during this planning pass already name it) and re-evaluate
`docs/specs/README.md`'s "What to build next" section.

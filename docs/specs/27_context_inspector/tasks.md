# Context Inspector — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Implements:** [`proposal.md`](proposal.md) in full · corrections, decisions and
the resolved §7 open question in [`context.md`](context.md)
**Started:** 2026-10-26 — **Done:** 2026-10-26

## Progress

| Task | State | Notes |
|---|---|---|
| 1 · Folder split, context map and links | Done 2026-10-26 | `git mv` to `27_context_inspector/proposal.md`; added this plan and `context.md`; every link to `27_context_inspector.md` repointed (28, 29, 30, 32 ×2, 24, 54, 51, 42 ×2, 26 ×4, 55 ×2, README); Status moved to In progress and README current state/row updated. No code. `npm run specs:check` passes (66 specs; it initially flagged the Progress rows until their State cells used the checker's three words). |
| 2 · Session-scoped pins and restrictions | Done 2026-10-26 | `ContextPin` and the seven methods in `session.rs` append/replay `context_pin_added` / `context_pin_removed` / `context_pins_cleared` / `context_restriction_set` / `context_restriction_cleared`. Readers use `parsed_events` (a rewind does not clear the controls); pins match on `(path, range)`; an empty restriction is `Some([])`. **Deviation:** the five tests are inline in `session.rs` rather than the new integration file, because they need only a `SessionStore` and a temp dir; the integration file starts at Task 3. **Falsified:** dropping the `context_pin_removed` arm left the removed range in the list and the test failed. Scoped checks: fast five passed; scoped clippy passed; the filtered lib nextest run passed 7 tests. |
| 3 · Manifest extensions and collection controls | Done 2026-10-26 | `ContextManifest` gained `conversationSummary` (from the #55 summary, measured redacted) and `symbolProvenance` (`total`/`languageServer`/`heuristic`, counted over the lines the renderer showed); `ExclusionReason` gained `UserRemoved`, `PathNotSelected`, `Missing`. `ContextRequest` gained `pinned`, `restriction` and `conversationSummary`; pins are collected through `add_file` (policy applies, a deleted pin is `Missing`), the restriction filters explicit/mention/search/project-rule/pin sources and exempts `AgentInstruction`. New `tests/context_inspector.rs` (13 tests at this point). **Falsified:** filtering instructions too made `a_restriction_does_not_remove_agent_instructions` fail. Scoped: fast five passed, scoped clippy passed, `context_inspector` 13/13, the five affected suites (context_assembly, prompt_cache, hooks, repository_map, symbol_index) 277 passed / 5 skipped. **Review 2026-10-26 (finding 6):** the symbols block's exemption from the session restriction is now explicit instead of implicit — the smaller coherent change, because C10 defines the restriction as a filter on file-bearing collection and the block is a path-less lookup keyed by the prompt. `a_restriction_narrows_file_content_but_not_the_symbols_block` pins both halves: `src/other.rs` is `PathNotSelected` while its symbol line still appears. `context.md` C10/§5, proposal §5.4 and the user guide say so. |
| 4 · `PreparedContext` and removal | Done 2026-10-26 | `PreparedContext` (`from_plan`/`into_plan`/`files`/`sections`) and `remove_optional`/`remove_summary` landed with Task 3's commit; this task added `removing_by_index_twice_uses_the_new_indices`. `Required` is refused before any mutation; category use, `items`, `total_tokens` and `included` are recomputed, `allocated` is left alone. **Falsified:** moving the `Required` check after the removal made the test see one fewer item and fail. Scoped: `context_inspector` 14/14, scoped clippy passed. |
| 5 · `pause_for_context_review` config | Done 2026-10-26 | `Config.pause_for_context_review` (default `false`) and its `ConfigOverlay` field are classified `Capability`, applied through `scoped(..., forbidden, ...)`, printed by both `to_policy_text` functions, labelled in `effective_policy`, and refused by `split_profile_keys`. Added a `weakening_cases` entry and `pause_for_context_review_is_off_by_default_and_user_only`. Scoped: fast five passed, scoped clippy passed, `permission_profiles` 79 passed / 1 skipped. |
| 6 · The pre-send pause and send gate | Done 2026-10-26 | `TurnProgress::Context` is emitted before the first model call (and, since the review, before the send-time `context_assembled` record); `build_model_prompt` is split into `build_conversation` + `build_model_prompt_with_conversation` with byte-identical output (the `prompt_cache` guard passes). `PausedFor::ContextReview` and `PendingContextReview` persist the prepared plan, the conversation block and the summary; `resume_after_context_decision` refuses every other pause and every other resume refuses a review; `remove_context_item`/`remove_context_summary` amend the pending review without taking the pause; Task 10 moved the `context_assembled` record to the send. Decline sends nothing and marks the task `Cancelled`. **Review 2026-10-26 (finding 2):** the record is now written by the send itself, not at assembly — a paused review records 0, a decline records 0, and an unchanged or amended `Send` records exactly 1 (the amended manifest when changed). `PendingContextReview.changed` was removed because nothing branches on it any more, and the resume emits the sent manifest on every send. `an_unchanged_review_is_recorded_once_and_a_changed_one_twice` became `a_turn_records_its_context_only_when_it_is_sent` (0 before send, 0 after decline, 1 after each send). **Falsified:** restoring the assembly-time record made the first assertion read 1 instead of 0. Tests: 10 new (pause no-call, default ordering, send, decline, instruction removal in the request, Required refusal, both cross-resume directions, removal across a reload, stripped-field load). **Falsified:** not saving the amended review left the removed instruction in the request and the test failed. Scoped: `context_inspector` 23/23, `prompt_cache`/`foundation`/`round_budget`/`compaction` 206 passed, scoped clippy passed. |
| 7 · Post-send record and past turns | Done 2026-10-26 | `SessionStore::read_session_context_manifests` reads `<data_dir>/audit/events.jsonl` (the manifest is an audit record, not a session event), filters by `sessionId`, and keeps the newest `context_assembled` per task. The edit flow reads the origin session's pins/restriction; the repair continuation now reads them too, reads the conversation before assembly, and emits `TurnProgress::Context`. Tests: the amended manifest is the one the reader returns, two turns' manifests in one session, an old manifest without the new fields, and an edit from a session with a pin and a restriction. **Review 2026-10-26 (finding 2):** because the send is now the only writer, `the_newest_manifest_per_task_is_what_was_sent` no longer creates two records; `the_newest_record_for_a_task_is_what_the_reader_returns` writes two records for one task directly and pins newest-wins. **Falsified:** changing `insert` to `entry(..).or_insert(..)` made the reader return the first (117-token) manifest. **Falsified:** `or_insert` (first-wins) made the removal test read the pre-removal manifest. Scoped: `context_inspector` 27/27, `prompt_cache` + `verification_loop` 84 passed, scoped clippy passed. |
| 8 · Shell routes, JSON and SSE | Done 2026-10-26 | Added `POST /api/context-pin`, `/api/context-restriction`, `/api/context-remove`, `/api/context-remove-summary`, `/api/resume-context-stream`; `chat_result_json` carries `contextReview` (manifest only); `/api/session` carries `contextState` plus a `context` manifest on each task; `TurnProgress::Context` maps to `TurnEvent::Context` and the SSE `context` event. Unknown pin actions and review decisions are refused, not defaulted. A decline path uses a no-op adapter, so it needs no provider key. Tests: pin round-trip through `/api/session`, restriction set/clear, unknown action, unknown decision, task JSON carries the manifest and no item content, and the progress mapping. Scoped: `desktop-shell --lib` 204 passed / 5 skipped, scoped clippy passed, fast five passed (typos caught an intentionally misspelled action value; changed to `unknown_action`). |
| 9 · The web panel and review card | Done 2026-10-26 | Header control shows `total / limit`; `#context-panel` renders category details (Required locked, symbols heuristic count, summary row), included paths/ranges, a session-pin list with unpin, an excluded `<details>` with every reason, a pin form, restriction textarea, and remove controls only on an open review. Every path opens through `/api/open-vscode-file`. `createContextReview` offers Review context / Send / Decline and streams `/api/resume-context-stream`; `context` SSE events and done payloads update the panel; past turns get a Context link from each task's manifest. Added the settings checkbox and `refreshPauseForContextReview`. Recovery re-presents a paused review (`a_reattached_context_review_carries_its_manifest`). Scoped: `desktop-shell --lib` 205 passed / 5 skipped, clippy on both crates passed, `node --check` and `npm run lint:web` passed, fast five passed after fixing one typo in this record. **Manual walk is Task 10.** **Review 2026-10-26 (findings 1, 3, 4):** the `context` SSE event is handled centrally in `processSseEvent`, so every stream path gets it (plain prompt, ask-again, resume, verification), not only `streamResumeContextRequest`; the redundant per-card handler was removed. The restriction textarea is populated from `contextRestriction` on panel render and session load, guarded by a dirty flag so typed text is not clobbered; an empty Apply is refused with a message pointing at Clear paths; the label and hint say empty means no file content and name the symbols exemption. The two debug `eprintln!`s in `recovery.rs` were removed. **Pause-off browser walk (2026-10-26):** shell on port 4899 against a throwaway data dir and the inspection fixture, `DAMAIAN_MOCK_MODEL_RESPONSE`, pause off. The `context` SSE event arrived before any `token`/`done` with `chatSubmitting` true; the header read `123 / 16.0k` and the panel listed Required 6, Repository map 63, Symbols 43 and File ranges 11 while the answer streamed. A window marker set before the send was still there afterwards, so this was the live event, not a reload. After Apply `retry.rs` and `loadSession`, the field read `retry.rs`; an empty Apply toasted the refusal and the restriction stayed; Clear paths emptied both. |
| 10 · Docs, manual walk, close-out | Done 2026-10-26 | User guide gained **Inspecting and controlling context**; troubleshooting gained the panel-first diagnosis section and the three new exclusion reasons; the `pinned` and `conversation_summary` category rows were corrected. §7 records the resolved open question, the walk and honest "not measured" answers. #32 §7.1's inspector criterion is closed. **Manual walk:** shell on port 4899 against a throwaway data dir and the inspection fixture with `DAMAIAN_MOCK_MODEL_RESPONSE`; `apiToken` + `setRepository()` from the console. First turn paused at 74 tokens (Required 4, map 63, file ranges 7); `Required` refused at the API; removing the file range took it to 67 and recorded `user_removed`; Send completed; after reload a **Context · 67 tokens** link opened the amended read-only manifest with the exclusion; a ranged pin, a restriction, and their clears were applied and read back from `/api/session`. Not walked: the Tauri window and an actual editor launch. **Deviations:** recovery reattachment for a paused review was added (not in the original plan) because the launch sweep would otherwise call the pause an unknown approval kind and fail the task; #26's "no producer yields a line range" note and test name were corrected because session pins are now the first producer; summary removal is API/manifest-tested but not walked through a live #55 summary; the record-count test was rewritten in the review to pin that a paused or declined review records nothing and a send records exactly one. **Checks:** `scripts/quality-gate.sh fmt node lint typos deny` passed; scoped clippy on `workspace-engine` and `desktop-shell` passed; `nextest` on context_inspector/context_assembly/permission_profiles/prompt_cache/round_budget/foundation **340 passed, 3 skipped**; `desktop-shell --lib` **205 passed, 5 skipped**; the deterministic eval tier **30/30 scenarios pass**, completion 1.000, 0 approval-policy violations, 0 restricted-path/secret violations, 0 unrelated files changed, recovery 1.000 (check pass 0.686 against the 30-scenario tier; the figure is not comparable to #26's 23-scenario run); `npm run specs:check` passed (66 specs). **Review 2026-10-26 (findings 5, 6, 7):** proposal §5.3 now says the record is written at send, not at assembly; §5.4 and `context.md` C10/§5 state the symbols exemption; `context.md`'s `remove_context_item` sentence no longer claims it appends an event; USER_GUIDE's restriction paragraph covers the populated field, the refused empty Apply and the symbols exemption; §7's Known gaps records that `/api/session` parses the whole global audit log and attaches a manifest to every task with no bounded reader; spec 26 §5.5 and the README's #26 row no longer say the event is `once per assembly`. **Review checks:** `scripts/quality-gate.sh fmt node lint typos deny` passed; scoped clippy on `workspace-engine` and `desktop-shell` passed; `context_inspector` 30 passed; `desktop-shell --lib` 205 passed, 5 skipped; `npm run specs:check` passed. **CI:** the full seven-check gate passed in CI on 2026-10-26 (Quality run 40340511717). |

| Review fixes · 2026-10-28 | Done 2026-10-28 | Post-merge review findings 5a–5d. **5a confirmed at the source:** `createContextReview` reopened the panel from the review object it was created with, so after one removal the next removal computed its index in the pre-removal manifest and hit the wrong item. The card now owns one live review, refreshed from every removal route's returned manifest, and `two_removals_across_a_reopen_use_the_manifest_each_call_returns` removes an item, reopens through `PausedTurns::context_review_manifest` and removes a second from the reopened manifest. **5b documented, not enforced:** the restriction filters only assembled context; `read_file`, `list_directory` and the search tools ignore it. Enforcing it belongs in `chat.rs`, which spec 51 owns until it merges, so the panel hint, `index.html` and the user guide now state the coverage and the gap exactly. **5c documented:** `run_repair_turn` and the one-shot edit flow do not pause, although the guide said "every turn"; the guide and the settings hint name both exemptions. **5d:** directory entries (`src/`, or a resolved `src`) prefix-match (`a_restriction_to_a_directory_includes_everything_under_it`), the repository map is listed as exempt beside the symbols row, the finding-repair dialog counts `kept.length` rather than every requested id, and the file-bearing-source test pins a fourth file so the keyword/semantic hit on `src/other.rs` is no longer masked by a pin on that path. Checks: fast five passed, scoped clippy on both crates passed, `context_inspector` 32/32, `desktop-shell --lib` 210 passed / 6 skipped, `node --check` and `npm run lint:web` passed. CI outstanding. **Superseded:** 5b and 5c were fixed on `chat-review-fixes` after spec 51 merged; see the Deferred review fixes row below. |
| Deferred review fixes · 2026-10-28 | Done 2026-10-28 (follow-up 2026-10-28) | **5b confirmed and fixed.** Tool dispatch ignored the session restriction. `dispatch_read_only_action` now reads it and refuses `read_file`/`list_directory` outside it with a plain failed tool result; a literal `search_content` path outside it is refused; wildcard content searches and codebase searches return only admitted matches with a note. The check uses the same normalization as assembly (`ContextManager::restriction_admits_path`). **Follow-up review:** `read_git_diff` now refuses a whole-repository request and any path outside the selection (`read_git_diff_refuses_paths_outside_the_session_restriction`). The search admission check moved into the walk/index search, before the match cap, for `search_content`, keyword and semantic codebase search (`the_search_cap_counts_only_matches_inside_the_restriction` builds 20 outside matches against a two-hit cap). An auto-approved `ls` (including `-R`) and the path lists of `read_git_status`, `read_git_log` and `list_worktrees` remain unfiltered and are named as exemptions in the panel, `index.html`, the user guide and §5.4. **5c confirmed and fixed for repair turns, edit flow recorded as a limit.** `run_repair_turn` now calls `pause_for_context_review`, saves `verification_turn` on the pending turn, and `resume_after_context_decision` closes a resumed repair through `close_verification_resume`. Test: `a_repair_turn_pauses_for_context_review_and_resumes_as_a_repair`. The one-shot edit flow (`edit.rs`) stays exempt because `/api/propose-edit` is a plain request with no pending-turn record or resume route; a pause would leave it waiting with nothing to send or decline. The guide and settings hint name only that exemption and explain it. Checks: fast five, scoped clippy for `workspace-engine`, `--test context_inspector --test verification_loop --test compaction` 114/114 before the follow-up; the follow-up reran fast five, scoped clippy on both crates, `context_inspector`/`compaction`/`verification_loop` 117/117, `workspace-engine --lib` 601 passed / 5 skipped, the related integration suites 377 passed / 2 skipped, `desktop-shell --lib` 224 passed / 6 skipped, and the deterministic eval tier (completion 1.000). Workspace gate left to CI. |

**Goal:** Make "the user can see what context the agent uses" true. The panel
renders #26's sanitized manifest pre- and post-send, and the user can pin a file
or range, remove optional context for a paused turn, restrict the session to
selected paths, clear pins and open any referenced path. Required context and the
user's own messages cannot be removed, enforced at the API.

**Architecture:** `ContextManifest` crosses the wire; `ContextItem` content never
does. Assembly gains pins, a session restriction, the #55 summary row and symbol
provenance counts. A new `PausedFor::ContextReview` in the existing typed-pause
machinery is the only place a review can be edited and sent. Pins and restrictions
are appended session events replayed newest-wins.

**Tech Stack:** Rust 2024, the existing serde/serde_json dependencies, the
existing session log, vanilla JavaScript and CSS. No new dependencies.

## Global Constraints

Every task's requirements implicitly include this section.

- **Read [`context.md`](context.md) in full before Task 1.** Its corrections
  (C1–C12) override the proposal's file references and several of its statements
  about the code. Do not re-derive any of them from `proposal.md` alone.
- **The pre-send pause is the only place removal is possible.** `remove_context_item`
  and `remove_context_summary` load the pending turn and refuse anything that is
  not `PausedFor::ContextReview`. `Required` items are refused **at the API**,
  not merely absent from the UI. A completed turn's panel is read-only.
- **A restriction narrows; it never widens.** `PathPolicy` and
  `restricted_patterns` apply to every collected path before the restriction
  does. A path in the restriction list that policy refuses is a
  `RestrictedPath` exclusion, never an inclusion. The restriction filters
  file-bearing collection only; `Instructions` and `Required` are not filtered
  (`context.md` §5, C10).
- **The manifest is the only context structure the shell sees.** It holds no
  content. The paused result, the SSE event, `/api/session` and every route
  return `ContextManifest`, never `ContextItem`. No route may serialise
  `PreparedContext.items`.
- **`chat.rs` is a solo track.** Tasks 6–7 edit `build_model_prompt`, the turn
  path and `PendingChatTurn`; no other spec may run beside them. #55, #65 and
  #66 are Done; #49's reuse slice is not in the ready set.
- **`PendingChatTurn` is shared with #61, #50, #23, #56, #65 and #66.** Every
  literal is exhaustive (no `..`), so the compiler finds all of them. A new
  field is `#[serde(default)]` and old pending turns must still load.
- **A new pause kind must be refused by every other resume.** Add the variant to
  `PausedFor`, `description`, `resume_method` and the exhaustive tuple in
  `paused_for`, and add the refusal to the new resume. A review must never be
  resumable through `resume_after_command_decision`, and a command approval must
  never be resumable through the review.
- **Every new `ConfigOverlay` field is classified exactly once.** Update the
  exhaustive destructure, `classify_overlay_fields!`, `ConfigOverlay::set`,
  `apply_overlay_scoped`, `to_policy_text`, `Config::default`, `rule_label` and
  `split_profile_keys`. `pause_for_context_review` is forbidden to repository and
  profile scope.
- **Redaction is untouched.** Pins carry paths and ranges, not content. Every
  item still goes through `add_text`, which redacts before measuring. The summary
  row is measured from the already-redacted rendering. No new path to the
  provider is added.
- **Tests first, and falsify every load-bearing test.** Write the named failing
  test, confirm it fails, implement, then break what the test guards, confirm it
  fails, restore, and record the mutation in the Progress row. This repository
  has a history of tests that passed without testing anything.
- **Fixtures:** engines set `enable_index_watcher: false` and a `data_dir` inside
  the temporary repository. Model-dependent paths use `MockModelAdapter` and its
  `requests`, or `DAMAIAN_MOCK_MODEL_RESPONSE`. Never touch the user's real
  Damaian data.
- **A private Cargo target directory per worktree.** Prefix every `cargo`
  command with `CARGO_TARGET_DIR="$PWD/target"` from the worktree root.
- **Scoped checks per task; the full gate runs on CI only** (AGENTS.md, since
  2026-10-15). Each task runs `scripts/quality-gate.sh fmt node lint typos deny`,
  `cargo clippy -p <crate> --all-targets --locked -- -D warnings` for every
  crate it touches, and the named test files. Do not run
  `cargo clippy --workspace` or `cargo nextest run --workspace` locally. Run
  `cargo nextest run -p desktop-shell --lib` when the shell is touched, and the
  deterministic eval tier when assembly or prompts change.
- **Never push.** Local commits per task are part of this work; the user pushes.
- **Never kill a process by name and never take port 4765.** The manual walk
  uses its own port, its own `DAMAIAN_DATA_DIR` and its own PID.

## File Structure

| File | Change |
|---|---|
| `crates/workspace-engine/src/session.rs` | `ContextPin`; pin and restriction events/readers (Task 2); context-manifest reader (Task 7) |
| `crates/workspace-engine/src/context_manager.rs` | `ContextRequest` fields; `Pinned` source; restriction filter; `ExclusionReason` additions; summary usage; symbol provenance counts; `PreparedContext` + removal (Tasks 3–4) |
| `crates/workspace-engine/src/config.rs`, `profile.rs`, `effective_policy.rs` | `pause_for_context_review` and its exhaustive classification (Task 5) |
| `crates/workspace-engine/src/chat.rs` | Manifest progress event; pause kind and pending review; `resume_after_context_decision`; removal API; prompt split; post-send record; edit-flow pins/restriction (Tasks 6–7) |
| `crates/workspace-engine/src/edit.rs` | Reads the origin session's pins/restriction; passes the new request fields (Task 7) |
| `crates/workspace-engine/tests/context_inspector.rs` | The engine behaviour, one test per acceptance criterion (Tasks 2–7) |
| `crates/workspace-engine/tests/permission_profiles.rs` | The new key's forbidden scopes (Task 5) |
| `crates/desktop-shell/src/lib.rs` | Routes, result JSON, session JSON, SSE event, tests (Task 8) |
| `crates/desktop-shell/static/index.html`, `app.js`, `style.css` | Header control, panel, review card, settings toggle (Task 9) |
| `docs/USER_GUIDE.md`, `docs/TROUBLESHOOTING.md`, `docs/specs/README.md`, `CHANGELOG.md`, `proposal.md`, this file | Task 10 |

## Interface reference

Checked on 2026-10-26. Re-check a line before editing near it.

```rust
// session.rs
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ContextPin {
    pub path: String,
    pub range: Option<LineRange>,
}

impl SessionStore {
    pub fn add_context_pin(
        &self, session_id: &str, path: &str, range: Option<LineRange>,
    ) -> Result<()>;
    pub fn remove_context_pin(
        &self, session_id: &str, path: &str, range: Option<LineRange>,
    ) -> Result<()>;
    pub fn clear_context_pins(&self, session_id: &str) -> Result<()>;
    pub fn context_pins(&self, session_id: &str) -> Result<Vec<ContextPin>>;
    pub fn set_context_restriction(&self, session_id: &str, paths: &[String]) -> Result<()>;
    pub fn clear_context_restriction(&self, session_id: &str) -> Result<()>;
    /// Newest restriction_set / restriction_cleared wins. `None` means no
    /// restriction; `Some([])` is an empty restriction, which excludes every
    /// file-bearing source.
    pub fn context_restriction(&self, session_id: &str) -> Result<Option<Vec<String>>>;
    pub fn read_session_context_manifests(
        &self, session_id: &str,
    ) -> Result<HashMap<String, ContextManifest>>; // newest context_assembled per task
}

// context_manager.rs
pub enum ExclusionReason { /* existing six */ UserRemoved, PathNotSelected, Missing }

pub struct ConversationSummaryUsage {
    pub tokens: usize,
    pub messages: usize,
    pub through_seq: u64,
    pub facts_only: bool,
}
pub struct SymbolProvenanceCounts {
    pub total: usize,
    pub language_server: usize,
    pub heuristic: usize,
}
pub struct ContextManifest {
    /* existing fields */,
    pub conversation_summary: Option<ConversationSummaryUsage>,
    pub symbol_provenance: Option<SymbolProvenanceCounts>,
}

pub struct PreparedContext {
    pub manifest: ContextManifest,
    pub items: Vec<ContextItem>,
}
impl PreparedContext {
    pub fn from_plan(plan: &ContextPlan) -> Self;
    pub fn into_plan(self) -> ContextPlan;
    pub fn files(&self) -> Vec<String>;
    pub fn sections(&self) -> Vec<PromptSection>;
    /// Removes `items[index]`; `Required` is an error. Adds `UserRemoved`,
    /// recomputes category use, `total_tokens` and the summary row is untouched.
    pub fn remove_optional(&mut self, index: usize) -> Result<ExclusionRecord>;
    /// Removes the summary row and records `UserRemoved` in its category.
    pub fn remove_summary(&mut self) -> Result<ExclusionRecord>;
}

pub struct ContextRequest<'a> {
    /* existing fields */,
    pub pinned: &'a [ContextPin],
    pub restriction: Option<&'a [String]>,
    pub conversation_summary: Option<&'a ConversationSummary>,
}

// chat.rs
pub struct AgentContextReview {
    pub id: String,
    pub manifest: ContextManifest,
}
pub struct ChatTurnResult {
    /* existing fields */,
    pub context_review: Option<AgentContextReview>,
}
pub enum ContextDecision { Send, Decline }

enum PausedFor { /* existing */, ContextReview }

struct PendingContextReview {
    prepared: PreparedContext,
    conversation: PromptConversation,
    summary: Option<ConversationSummary>,
    summary_removed: bool,
}

impl ChatOrchestrator {
    pub fn resume_after_context_decision(
        &self, proposal_id: &str, decision: ContextDecision, approved_by: &str,
        model_adapter: &mut dyn ModelAdapter, sink: &mut TurnSink<'_>,
    ) -> Result<ChatTurnResult>;
    pub fn remove_context_item(
        &self, proposal_id: &str, index: usize,
    ) -> Result<ContextManifest>;
    pub fn remove_context_summary(&self, proposal_id: &str) -> Result<ContextManifest>;
}

enum TurnProgress { /* existing */, Context(Box<ContextManifest>) }
```

Key current lines: `ask_with_session_with_options` `chat.rs:943`; assembly
`:1157`; `context_assembled` `:1168`; `build_model_prompt` `:8670`;
`ChatTurnResult` `:516`; `PendingChatTurn` `:8125`; `PausedFor` `:8002`;
`paused_for` `:8055`; `refuse_unless_paused_for` `:3804`;
`run_agentic_turn` `:4436`; `build_context` `context_manager.rs:1057`;
`ChatResult` JSON `lib.rs:4842`; `task_states_json` `lib.rs:5447`;
`handle_session` `lib.rs:551`; SSE switch `lib.rs:4758`; `renderMessages`
`app.js:5076`; SSE dispatch `app.js:11627`.

## Order and parallelism

```
1 ──> 2 ──> 3 ──> 4 ──> 5 ──> 6 ──> 7 ──> 8 ──> 9 ──> 10
```

Sequential. Tasks 2 and 3 can be built in either order (session state is
independent of assembly) but Task 4 needs both. Task 5 is independent of 2–4 and
may land earlier if a rebase demands it; Task 6 needs 3–5; Task 7 needs 6; Tasks
8 and 9 need the engine surface fixed by 4–7. Tasks 6 and 7 are the solo
`chat.rs` window.

---

## Task 1: Folder split, context map and links

**Requirements:** none (planning). **Proposal:** header. **Context:** all.

**Files:** `docs/specs/27_context_inspector/`; every document linking to the
flat file; `docs/specs/README.md`.

- [x] **Step 1: Move the flat spec** with `git mv
  docs/specs/27_context_inspector.md
  docs/specs/27_context_inspector/proposal.md`, so history follows it.
- [x] **Step 2: Repoint every link** that names `27_context_inspector.md`:
  same-directory specs use `27_context_inspector/proposal.md`; folder specs use
  `../27_context_inspector/proposal.md`; the README implementation-order row and
  the proposal's own links use the folder form.
- [x] **Step 3: Write [`context.md`](context.md)** with the verified pipeline
  map, C1–C12 corrections, the resolved §7 open question, the decisions, what
  other specs rely on, the existing tests and the manual-walk recipe.
- [x] **Step 4: Write this file** with Global Constraints, ten tasks, the
  progress table and per-task verification.
- [x] **Step 5: Set Status In progress** in the proposal header, the README
  current-state paragraph (move #27 from ready to in progress) and the README
  implementation-order row; add `**Started:** 2026-10-26 — **Done:** 2026-10-26` to this header.
- [x] **Step 6: Verify links and statuses.** `npm run specs:check`; read its
  output. `grep -rn "27_context_inspector\\.md" docs` must return nothing but
  this plan's historical note, if any.
- [x] **Step 7: Update the Progress row** and commit.
  Subject: `Plan the context inspector as a folder spec`.

---

## Task 2: Session-scoped pins and restrictions

**Requirements:** 2, §5.4. **Context:** §3 C4, §5.

**Files:** `session.rs`; new `tests/context_inspector.rs`.

- [x] **Step 1: Write the failing tests** in `tests/context_inspector.rs`
  (build a `SessionStore` with `test_config`-style `data_dir` and a real
  session; the session helpers live in `tests/foundation.rs`):
  - `pins_are_appended_events_and_replay_newest_wins`: add a whole-file pin and
    a ranged pin, remove the ranged one, add another range; `context_pins`
    returns the whole file and the second range in insertion order, and the log
    contains `context_pin_added`/`context_pin_removed` events with the parsed
    `eventType`.
  - `clearing_pins_removes_every_earlier_pin`: add two, clear, assert empty, add
    one after the clear and assert only it remains.
  - `a_restriction_replaces_and_clears`: set `["src/a.rs"]`, set
    `["src/b.rs"]`, assert only `b`; clear, assert `None`; set `[]`, assert
    `Some([])`.
  - `a_pin_round_trips_through_a_second_store`: write with one
    `SessionStore`, read with a second built on the same `data_dir` (the restart
    proof).
  - `a_line_containing_a_pin_event_name_is_not_a_pin`: write a `chat_message`
    event with the text `context_pin_added` and assert it is ignored — the
    reader matches parsed `eventType`, not substrings.
- [x] **Step 2: Run them and confirm they fail to compile** on the missing
  methods: `CARGO_TARGET_DIR="$PWD/target" cargo nextest run -p workspace-engine
  --test context_inspector`.
- [x] **Step 3: Implement.** Add `ContextPin` and the seven methods beside
  `allow_browser_diagnostics_for_session` (`session.rs:1063`). Every write uses
  `append_session_event`; every read uses `parsed_events`, never
  `active_events`: a rewind discards conversation, not the user's control of
  what context may be sent (same reasoning as `session_mode`). Pin removal
  matches `(path, range)`; a missing pin is a no-op, not an error. A restriction
  path is stored trimmed and non-empty; an empty entry is dropped. Do not
  normalise paths here — the assembler owns path normalisation, and doing it in
  two places is how the two disagree.
- [x] **Step 4: Falsify.** Make `context_pins` use `active_events` after a
  rewind → the round-trip test still passes but a new rewind test fails; add one
  assert to the round-trip test that a rewind does not clear pins. Make pin
  removal match only the path → the ranged-pin test's second range disappears.
  Restore both.
- [x] **Step 5: Scoped checks.**

  ```bash
  scripts/quality-gate.sh fmt node lint typos deny
  CARGO_TARGET_DIR="$PWD/target" cargo clippy -p workspace-engine --all-targets --locked -- -D warnings
  CARGO_TARGET_DIR="$PWD/target" cargo nextest run -p workspace-engine --test context_inspector
  ```

- [x] **Step 6: Update the Progress row** (what landed, checks, mutations) and
  commit. Subject: `Store context pins and path restrictions per session`.

---

## Task 3: Manifest extensions, pins and the restriction filter

**Requirements:** 1, 2, §5.2, §5.4. **Context:** §3 C2, C6, C7, C10; §5.

**Files:** `context_manager.rs`; `tests/context_assembly.rs` and
`tests/context_inspector.rs`.

- [x] **Step 1: Write the failing tests.**
  - `the_manifest_has_a_summary_row_with_its_message_count`
    (`tests/context_inspector.rs`): assemble with a `ConversationSummary`
    fixture (build one through `crate::compaction` or the test helper if one
    exists); assert `manifest.conversation_summary` has non-zero tokens,
    `messages`, `through_seq`, and `facts_only` matching the fixture.
  - `the_symbols_row_counts_language_server_and_heuristic_symbols`
    (`tests/context_inspector.rs`): a fixture whose symbol index has both;
    assert `manifest.symbol_provenance` has `total == ls + heuristic` and both
    counts match the symbols the prompt names.
  - `a_pinned_file_enters_the_pinned_category_on_every_assembly`
    (`tests/context_inspector.rs`): pass one pin; assert a `Pinned` item with
    the path, its tokens in the category, and it in `files`.
  - `a_pinned_range_carries_its_line_range` (same): pin `10..20`; the item's
    `range` is `Some(LineRange { 10, 20 })` and its content is those lines.
  - `a_pinned_file_the_policy_refuses_is_an_exclusion_not_an_item`: pin
    `.env`; `excluded` has `RestrictedPath` with category `Pinned`; `included`
    has no `.env`.
  - `a_pinned_file_that_no_longer_reads_is_missing`: pin a path, delete the
    file, assemble; `excluded` has `Missing` with category `Pinned`.
  - `a_restriction_filters_each_file_bearing_source`: with a restriction to
    `src/a.rs`, a prompt mentioning `src/b.rs`, a search hit on `src/c.rs`, a
    project rule `README.md` and a pin on `src/d.rs` all appear in `excluded`
    as `PathNotSelected`; `src/a.rs` is included.
  - `a_restriction_cannot_include_a_path_the_policy_refuses`: restrict to
    `.env`; excluded is `RestrictedPath`, included is empty.
  - `a_restriction_does_not_remove_agent_instructions`: restrict to
    `src/a.rs`; `AGENTS.md` is still included.
  - `every_new_exclusion_reason_reaches_the_manifest`: extend
    `every_exclusion_reason_reaches_the_manifest` in the inline
    `context_manager.rs` tests with `UserRemoved`, `PathNotSelected`, `Missing`
    (the serialised names `user_removed`, `path_not_selected`, `missing`).
- [x] **Step 2: Run them and confirm they fail** on the missing fields,
  reasons and request fields.
- [x] **Step 3: Implement.**
  - Add `ConversationSummaryUsage`, `SymbolProvenanceCounts`,
    `ContextManifest.conversation_summary` and `.symbol_provenance`. Defaults
    are `None`; do not add `#[serde(default)]` unless an old `context_assembled`
    event must still parse (it must — Task 7 reads old logs; add it).
  - Add `ExclusionReason::{UserRemoved, PathNotSelected, Missing}` and their
    `as_str`/serde names through the enum's existing `snake_case` derive.
  - Add `ContextRequest.pinned`, `.restriction`, `.conversation_summary`.
    Compute the summary row in `build_context` from
    `summary.render_for_model()` through `scanner.redact` and
    `TokenEstimate::Prose`; never from unredacted text.
  - Collect pins after instructions/project rules and before the map, through
    `add_file` with `CollectionSource::Pinned`, `ContextKind::Pinned`, the pin's
    range and `allow_outside_root: false`. Add `Pinned` to `CollectionSource`
    and its `collection_exclusion` arm (apply the policy).
  - Normalise each restriction entry once in `build_context` with
    `normalise_candidate(root, path, false)`. Filter in `add_file` for
    `ContextKind::{ExplicitFile, RetrievedFile, ProjectRule, Pinned}` only: if
    the normalised path is not in the (non-empty) restriction, record
    `PathNotSelected` with the item's category and return before the read.
    `AgentInstruction` is exempt by kind, not by a special source check.
  - On a read error, if `record_read_failure` returns no reason and the kind is
    `Pinned`, record `Missing`. Do the same in `flush_file_ranges`.
  - Thread symbol provenance counts through `render_block` (return a small
    `RenderedBlock` with text, provenance and counts) and set
    `manifest.symbol_provenance` when the symbols block is placed. Count only
    the lines `render_for_model` says it showed.
  - Update every `ContextRequest` construction (`chat.rs` twice, `edit.rs`
    once) with `pinned: &[]`, `restriction: None`, `conversation_summary: None`
    for now; Task 6/7 wire the real values.
- [x] **Step 4: Falsify.** (1) Count all matches instead of `shown` → the symbol
  count test fails on a truncated block. (2) Filter `AgentInstruction` too → the
  instructions test fails. (3) Apply the restriction before the policy check →
  the `.env` restriction test reports `PathNotSelected` instead of
  `RestrictedPath`. (4) Drop `Missing` on read failure → the deleted-pin test
  loses its record. Restore each.
- [x] **Step 5: Scoped checks.**

  ```bash
  scripts/quality-gate.sh fmt node lint typos deny
  CARGO_TARGET_DIR="$PWD/target" cargo clippy -p workspace-engine --all-targets --locked -- -D warnings
  CARGO_TARGET_DIR="$PWD/target" cargo nextest run -p workspace-engine --test context_inspector --test context_assembly
  ```

- [x] **Step 6: Update the Progress row** and commit.
  Subject: `Show pins, restrictions and summary cost in the context manifest`.

---

## Task 4: `PreparedContext` and per-turn removal

**Requirements:** 2, 3, §5.5. **Context:** §3 C8; §5.

**Files:** `context_manager.rs`; `tests/context_inspector.rs`.

- [x] **Step 1: Write the failing tests.**
  - `removing_an_optional_item_reduces_the_tokens_and_records_user_removed`:
    build a plan, `PreparedContext::from_plan`, remove an `Instructions` item;
    `total_tokens` falls by the item's tokens, its category's `used` and
    `items` fall, `manifest.included` loses it and `manifest.excluded` gains
    `UserRemoved` with the same path.
  - `a_required_item_cannot_be_removed`: remove the `Required` item's index;
    the call is `Err` and the context is unchanged. Assert the error names
    `required`.
  - `removing_a_whole_file_removes_it_from_files`: remove a file item; its path
    is absent from `files()`.
  - `removing_the_summary_records_its_category_and_hides_the_row`: remove the
    summary; `manifest.conversation_summary` is `None`, and the exclusion is
    `UserRemoved` in category `ConversationSummary`.
  - `removing_by_index_twice_uses_the_new_indices`: two removals, each by the
    index in the manifest returned by the previous call; both are gone.
- [x] **Step 2: Run them and confirm they fail to compile.**
- [x] **Step 3: Implement.** `PreparedContext` beside `ContextPlan`; derive
  `Serialize`/`Deserialize`/`Eq` on it and on `ContextPlan` if not already
  (the pause persists it). `from_plan` clones items and manifest; `into_plan`
  reconstructs `token_estimate` and `files`. `remove_optional`:
  - bounds-check the index against `items`; `Err(InvalidInput)` otherwise;
  - refuse `ContextCategory::Required` **before** mutating;
  - remove the item, push `ExclusionRecord { UserRemoved, category, path,
    range, detail: Some("removed for this turn by the user") }`;
  - recompute the category's `used` and `items` from the remaining items;
    recompute `total_tokens`; leave `allocated` alone (it is what the category
    was allowed, not what it used).
  `remove_summary` sets the row to `None` and pushes a `UserRemoved` record in
  `ConversationSummary` with a detail naming the message count.
- [x] **Step 4: Falsify.** (1) Check `Required` after mutating → the unchanged
  assertion fails. (2) Leave `used` alone → the category assertion fails.
  (3) Do not rebuild `files` → the whole-file assertion fails. Restore each.
- [x] **Step 5: Scoped checks** (same three commands as Task 3).
- [x] **Step 6: Update the Progress row** and commit.
  Subject: `Remove optional context from a prepared turn`.

---

## Task 5: `pause_for_context_review` config

**Requirements:** §5.1, §5.6, §6. **Context:** §6.

**Files:** `config.rs`, `profile.rs`, `effective_policy.rs`,
`tests/permission_profiles.rs`.

- [x] **Step 1: Write the failing tests** in `tests/permission_profiles.rs`:
  - `pause_for_context_review_is_off_by_default_and_user_only`: default
    `Config` is false; a user overlay setting `true` applies; a repository
    overlay in either direction is `Forbidden` and the default holds; a profile
    overlay is refused by `split_profile_keys` with class `forbidden`.
  - Add the key to `conversation_keys_have_human_names_and_capability_kinds`
    (or a sibling) asserting kind `Capability` and a `rule_label`.
- [x] **Step 2: Run them and confirm they fail.**
- [x] **Step 3: Implement.**
  - `Config.pause_for_context_review: bool` with a doc comment saying why it is
    user-only: it decides whether every turn waits for a human, which is neither
    a restriction nor a repository's business.
  - `ConfigOverlay.pause_for_context_review: Option<bool>`;
    exhaustive destructure; `classify_overlay_fields!` entry `Capability`;
    `ConfigOverlay::set`; `apply_overlay_scoped` via
    `scoped(..., trusted, forbidden, rejected)`; `Config::default` `false`;
    `to_policy_text` (always printed, so an off value is visible);
    `rule_label`; `split_profile_keys`'s forbidden list.
  - Do not add a `context_budget.*` key; the restriction is session state.
- [x] **Step 4: Falsify.** Remove the key from `split_profile_keys` → the
  agreement test fails. Remove it from the exhaustive destructure → the crate
  does not compile, which is the guard. Restore.
- [x] **Step 5: Scoped checks.**

  ```bash
  scripts/quality-gate.sh fmt node lint typos deny
  CARGO_TARGET_DIR="$PWD/target" cargo clippy -p workspace-engine --all-targets --locked -- -D warnings
  CARGO_TARGET_DIR="$PWD/target" cargo nextest run -p workspace-engine --test permission_profiles
  ```

- [x] **Step 6: Update the Progress row** and commit.
  Subject: `Add the pause-before-send context setting`.

---

## Task 6: The pre-send pause and the send gate

**Requirements:** 1, 2, 3, §5.1, §5.5. **Context:** §4, C8, C12.

**Files:** `chat.rs`; `tests/context_inspector.rs`; `tests/round_budget.rs` if
the pause matrix test lives there.

- [x] **Step 1: Write the failing tests.**
  - `a_turn_pauses_before_its_first_model_call_when_the_setting_is_on`: config
    with `pause_for_context_review: true`; ask; `result.context_review` is
    `Some`, `adapter.requests` is empty, the pending file exists, and
    `paused_for` is `ContextReview`.
  - `the_default_turn_emits_its_manifest_and_does_not_pause`: setting off; ask
    with a Mock; a `TurnProgress::Context` event is observed before the first
    request, `context_review` is `None`, and the model was called.
  - `sending_a_reviewed_context_makes_exactly_one_request`: pause, resume with
    `Send`; `adapter.requests` has one request whose user message contains the
    prompt, and the task ends in a normal state.
  - `declining_a_review_makes_no_model_call_and_does_not_send`: resume with
    `Decline`; zero requests, the task is `Cancelled`, and the response says no
    request was sent.
  - `removing_an_instruction_changes_the_request_that_is_sent`: pause, remove
    the `Instructions` item by manifest index, resume; the request user message
    lacks the instruction text; the amended manifest excludes it as
    `UserRemoved`; a second `context_assembled` event exists for the task.
  - `removing_a_required_item_is_refused_at_the_api`: `remove_context_item` on
    the `Required` index returns `Err`; the pending turn still exists and the
    review is still resumable.
  - `a_command_approval_cannot_resume_a_context_review`: call
    `resume_after_command_decision` with the review's id; it is refused and the
    pending file remains.
  - `a_context_review_cannot_resume_a_command_approval`: the reverse, with a
    normal command pause.
  - `an_old_pending_turn_without_the_review_field_still_loads`: strip
    `context_review` from a pending file written by a command pause, resume it;
    it loads (the field is `#[serde(default)]`).
- [x] **Step 2: Run them and confirm they fail.**
- [x] **Step 3: Implement.**
  - Add `TurnProgress::Context(Box<ContextManifest>)` and emit it in
    `ask_with_session_with_options` after the audit record and before the pause
    decision. Emit it again on a resume whose manifest changed.
  - Split `build_model_prompt` without changing its output: a
    `PromptConversation { summary: Option<String>, body: Option<String> }`
    builder (redacting the summary and each message as today), and a
    `build_model_prompt_with_conversation(plan, conversation, command_context)`
    that writes the two `plan.sections()` blocks around it. `build_model_prompt`
    keeps its signature and composes the two. Add the `without_summary` helper.
  - Add `context_review: Option<AgentContextReview>` to `ChatTurnResult` and set
    it to `None` in every existing constructor.
  - Add `PendingContextReview { prepared, conversation, summary,
    summary_removed }`, `PendingChatTurn.context_review` with
    `#[serde(default)]`, `PausedFor::ContextReview` (description
    "a context review", resume method `resume_after_context_decision`), and its
    tuple position in `paused_for`. Update all seven `PendingChatTurn` literals
    and `refuse_unless_paused_for` callers that construct one.
  - In `ask_with_session_with_options`, read `session_store.context_pins` and
    `context_restriction` before assembly and pass them plus
    `conversation.summary.as_ref()` in the request. After `messages` are built,
    if `config.pause_for_context_review`, call `pause_for_context_review`
    (which saves the pending turn, notes the pending approval, marks the task
    `AwaitingApproval` with kind `context_review`, audits
    `context_review_paused`, and returns a result with a response such as
    "Review the context before this turn is sent." and `context_review: Some`).
    Otherwise call `run_agentic_turn` exactly as today.
  - `remove_context_item` and `remove_context_summary`: load (do not take) the
    pending turn, refuse the wrong pause kind, call
    `PreparedContext::remove_optional`/`remove_summary`, save the pending turn,
    audit `context_item_removed`/`context_summary_removed` (path, range,
    category, tokens; never content), append a new `context_assembled` event
    with the amended manifest, and return the manifest.
  - `resume_after_context_decision`: take the pending turn, refuse the wrong
    kind, audit `context_review_decision`. `Decline` finishes the task
    `Cancelled`, appends an assistant message saying nothing was sent, and
    returns a result with `cancelled: true` and zero model calls. `Send` renders
    the amended user message through `build_model_prompt_with_conversation`,
    replaces `messages[1]`, emits the amended `Context` event, clears the
    pending approvals, sets the task to `PreparingContext`, and calls
    `run_agentic_turn` with the carried fields.
  - The post-send manifest for the task is the newest `context_assembled`
    event; do not record a second event when nothing changed.
- [x] **Step 4: Falsify.** (1) Resume with the unamended plan → the removed
  instruction test sees it in the request. (2) Do not refuse the wrong pause
  kind → the cross-resume tests resume something they should not. (3) Emit
  `Context` after the first call → the ordering test fails. (4) Drop the
  `#[serde(default)]` → the old-file test fails. Restore each.
- [x] **Step 5: Scoped checks.**

  ```bash
  scripts/quality-gate.sh fmt node lint typos deny
  CARGO_TARGET_DIR="$PWD/target" cargo clippy -p workspace-engine --all-targets --locked -- -D warnings
  CARGO_TARGET_DIR="$PWD/target" cargo nextest run -p workspace-engine --test context_inspector --test foundation --test round_budget --test prompt_cache --test context_assembly
  ```

- [x] **Step 6: Update the Progress row** and commit.
  Subject: `Pause a turn before sending so its context can be reviewed`.

---

## Task 7: Post-send record, past turns and the edit flow

**Requirements:** 1, §5.3, §5.4. **Context:** §3 C7, §4, §7.

**Files:** `session.rs`, `chat.rs`, `edit.rs`, `tests/context_inspector.rs`.

- [x] **Step 1: Write the failing tests.**
  - `the_newest_context_assembled_event_per_task_is_what_was_sent`: a paused
    turn with one removal then send; `read_session_context_manifests` returns
    the amended manifest for that task, not the first one.
  - `a_past_turns_context_is_readable_after_another_turn`: two turns in one
    session; the map has both task ids, each with its own included list.
  - `an_old_context_assembled_event_without_the_new_fields_still_parses`:
    remove `conversationSummary`/`symbolProvenance` from the JSON and parse; the
    manifest loads with `None`.
  - `an_edit_from_a_session_honours_its_pins_and_restriction`: pin `src/a.rs`,
    restrict to it, call `propose_edit` with the origin session id; its
    `context_assembled` manifest has the pin and no path outside the
    restriction.
- [x] **Step 2: Run them and confirm they fail.**
- [x] **Step 3: Implement.**
  - `SessionStore::read_session_context_manifests`: iterate parsed events, keep
    `event_type == "context_assembled"`, read `taskId` and the `manifest` field
    (a JSON string, because `audit_fields` stores it as one), parse into
    `ContextManifest`, and overwrite per task so the newest wins. A malformed
    record is skipped, not fatal.
  - In `edit.rs`, read pins/restriction from `origin_session_id` when it is
    non-empty and pass them to `ContextRequest`. Leave `conversation_summary`
    `None` (the one-shot edit has no conversation block).
  - In `chat.rs`'s verification repair turn, pass `pinned`/`restriction`/summary
    from the session (read the conversation before assembly so the summary is
    available) and emit `TurnProgress::Context` after its audit record.
- [x] **Step 4: Falsify.** (1) Keep the first event per task instead of the
  newest → the removal test sees the pre-removal manifest. (2) Read the
  `manifest` field as an object → parse fails and the test catches it.
  (3) Skip the edit wiring → its manifest has no pin. Restore each.
- [x] **Step 5: Scoped checks.**

  ```bash
  scripts/quality-gate.sh fmt node lint typos deny
  CARGO_TARGET_DIR="$PWD/target" cargo clippy -p workspace-engine --all-targets --locked -- -D warnings
  CARGO_TARGET_DIR="$PWD/target" cargo nextest run -p workspace-engine --test context_inspector --test prompt_cache
  ```

- [x] **Step 6: Update the Progress row** and commit.
  Subject: `Keep each turn's sent context readable after it ends`.

---

## Task 8: Shell routes, JSON and SSE

**Requirements:** 1, 2, §5.3, §5.6. **Context:** §5.

**Files:** `crates/desktop-shell/src/lib.rs`; its inline tests.

- [x] **Step 1: Write the failing tests** (beside the existing handler tests,
  using `post_form_for_test`, `get_for_test`, `json_of`, and the SSE harness):
  - a `context` SSE event reaches the stream before `done` and carries a
    manifest with no item content;
  - `POST /api/context-pin` with `action=add` then `GET /api/session` shows the
    pin in `contextState.pins`; `action=remove` removes it; `action=clear`
    empties it;
  - `POST /api/context-restriction` with `paths=src/a.rs\nsrc/b.rs` sets two,
    and `clear=true` clears;
  - `POST /api/context-remove` returns the amended manifest and a `Required`
    index is a 400/error;
  - `POST /api/resume-context-stream` with `decision=send` streams a turn, and
    `decision=decline` returns without a model call (the mock adapter is reached
    through the same harness as the existing resume tests);
  - `GET /api/session` puts a `context` object on each task that has one and
    omits it otherwise.
- [x] **Step 2: Run them and confirm they fail.**
- [x] **Step 3: Implement.**
  - Add `TurnProgress::Context` mapping in `progress_event` (`lib.rs:2866`),
    `TurnEvent::Context(Box<ContextManifest>)`, and the SSE arm writing
    `serde_json::to_string(&manifest)` (the manifest is `Serialize`; no manual
    escaping).
  - `chat_result_json` gains `"contextReview"` via a helper
    `context_review_json(Option<&AgentContextReview>)` shaped
    `{"proposalId":"…","manifest":{…}}` or `null`.
  - `handle_session` reads `read_session_context_manifests` and
    `context_pins`/`context_restriction`; `task_states_json` gains a `contexts`
    parameter and appends `,"context":<manifest>` for a task that has one; the
    response gains `"contextState":{"pins":[…],"restrictedPaths":[…]|null}`.
  - Routes in the dispatch table: `POST /api/context-pin`,
    `POST /api/context-restriction`, `POST /api/context-remove`,
    `POST /api/context-remove-summary`, `POST /api/resume-context-stream`.
    Field names: `session_id`, `action`, `path`, `start`, `end`, `paths`,
    `clear`, `proposal_id`, `index`, `decision`, `repo`. Validate the session
    exists; reject an unknown `action`/`decision` rather than defaulting.
  - `run_resume_context_request` follows `run_resume_plan_request`'s shape and
    calls `resume_after_context_decision` with a dedicated adapter.
- [x] **Step 4: Falsify.** (1) Serialise the prepared items by mistake → the
  no-content assertion fails. (2) Default an unknown decision to `send` → the
  reject test fails. (3) Take the review in `context-remove` → the subsequent
  send fails. Restore each.
- [x] **Step 5: Scoped checks.**

  ```bash
  scripts/quality-gate.sh fmt node lint typos deny
  CARGO_TARGET_DIR="$PWD/target" cargo clippy -p desktop-shell --all-targets --locked -- -D warnings
  CARGO_TARGET_DIR="$PWD/target" cargo nextest run -p desktop-shell --lib
  ```

- [x] **Step 6: Update the Progress row** and commit.
  Subject: `Serve the context manifest and its controls to the shell`.

---

## Task 9: The web panel and review card

**Requirements:** 1, 2, 3, §5.2, §5.6. **Context:** §5, §10.

**Files:** `static/index.html`, `static/app.js`, `static/style.css`.

- [x] **Step 1: Add the markup.** A `#context-inspector-btn` in
  `.thread-actions` (label shows `tokens / limit`, `aria-haspopup="dialog"`,
  hidden until a manifest exists), a `#context-panel` section inside
  `.conversation` (header with total/limit/source, close button, body, controls),
  and a `pause-for-context-review-input` checkbox in the Conversation settings
  section with a hint naming the default and that it is user-only.
- [x] **Step 2: Implement the state and rendering.**
  - Globals: `lastContextManifest`, `contextPanelTaskId`, `contextPins`,
    `contextRestriction`, `currentContextReview`.
  - `formatContextTokens`, `renderContextButton`, `renderContextPanel`,
    `openContextPanel`, `closeContextPanel`.
  - Category rows from `manifest.categories` in order, with `required` locked,
    `items` expandable to `manifest.included` entries for that category (path,
    `:start-end`, tokens, provenance). The summary row comes from
    `manifest.conversationSummary`. Excluded is a collapsible `<details>` with
    the count; every path is a button calling `/api/open-vscode-file` (not
    `/api/context-file`).
  - Pin buttons on included/excluded file rows call `/api/context-pin`
    (`action=add`, the row's path/range), plus a small path/range form and
    "Clear pins". "Pin applies to later turns" is stated.
  - Restriction controls: a textarea (one path per line), Apply and Clear, and
    the current list as chips. The list comes from `payload.contextState`.
  - Remove buttons render only when `currentContextReview` matches the displayed
    task and the row is not `required`; they call `/api/context-remove` (or
    `/api/context-remove-summary`) and re-render from the returned manifest.
- [x] **Step 3: Wire the events.**
  - `processSseEvent` handles `context` → `lastContextManifest = payload;
    renderContextButton(); if open, renderContextPanel()`.
  - Every `done` handler calls `noteContext(payload)`; `renderMessages` builds
    `contextByTask` from `tasks` and adds a *Context* disclosure on each
    assistant message that has one, opening the panel for that task.
  - `appendProposals` renders `createContextReview(payload.contextReview,
    repo)`; the card shows the token/limit summary, *Review context* (opens the
    panel), *Send* (primary), *Decline* (quiet), and streams the resume through
    `streamResumeContextRequest`. On send it renders the result like
    `createPlanReview`'s approve path; on decline it writes "No request was
    sent." and refreshes the session.
  - `loadConfigFile` and the settings open path refresh the pause checkbox; its
    change handler writes `pause_for_context_review` through `/api/config-set`
    and reloads the editor so the next Save cannot write the old value back.
  - Relabel the composer chips' remove aria to "Remove … from this request" and
    add a short title saying they are attachments, not session pins.
- [x] **Step 4: Style** with existing variables: a right-hand drawer
  (`position: absolute; right: 16px; top: 48px; width: min(520px, …);
  max-height: calc(100% - 120px); overflow: auto`), `.context-row`,
  `.context-row[data-locked]`, `.context-excluded`, `.context-controls`. Keep
  it 2-space indented and under ~100 columns where practical.
- [x] **Step 5: Scoped checks.**

  ```bash
  node --check crates/desktop-shell/static/app.js
  npm run lint:web
  CARGO_TARGET_DIR="$PWD/target" cargo nextest run -p desktop-shell --lib
  ```

- [x] **Step 6: Manual smoke** against a throwaway shell (port not 4765) before
  the formal walk in Task 10: open the panel, see the category rows.
- [x] **Step 7: Update the Progress row** and commit.
  Subject: `Add the context inspector panel and review card`.

---

## Task 10: Docs, acceptance walk and close-out

**Requirements:** §5.7, all acceptance criteria. **Context:** §10.

**Files:** `docs/USER_GUIDE.md`, `docs/TROUBLESHOOTING.md`, `CHANGELOG.md`,
`docs/specs/README.md`, `docs/specs/28_memory_model_and_storage/proposal.md`,
`docs/specs/32_hooks/proposal.md`, `proposal.md`, this file.

- [x] **Step 1: Documentation.**
  - `USER_GUIDE.md`: how to read the panel (categories, tokens, limit source,
    excluded), what each category is, what a pin is and how long it lasts, what
    a session restriction does, why `Required` is locked, why instructions can
    be removed but not by a restriction, and how to turn on pause-before-send.
  - `TROUBLESHOOTING.md`: the excluded list as the first diagnostic, what every
    exclusion reason means (`restricted_path`, `secret_bearing` if present,
    `category_budget_exhausted`, `total_budget_exhausted` if present,
    `deduplicated_into_range`, `truncated`, `too_large`, `binary`,
    `user_removed`, `path_not_selected`, `missing`), how to inspect a past
    turn, and what to do when a pin is not included.
- [x] **Step 2: Manual browser walk** from `context.md` §10: a throwaway
  `DAMAIAN_DATA_DIR`, a fixture repo, a port other than 4765, `apiToken = '…'`
  then `setRepository('<repo>', false)` in devtools. Walk and record in
  `proposal.md` §7: the header control's numbers, a category expanded, one
  excluded path opened, a pin added/cleared, a restriction applied/cleared, and
  (setting on) a paused review with one item removed and sent. Record what was
  seen and anything that was not reachable.
- [x] **Step 3: Fill `proposal.md` §7** with the resolved open question (from
  `context.md` §4), what the walk showed, and honest answers to the two
  product questions the spec asks. Do not invent usage data; say "not measured"
  where it is not.
- [x] **Step 4: Close #32's criterion.** In `32_hooks/proposal.md` §7.1 change
  "The inspector criterion is not met ... until #27" to met, naming the
  `HookContext` category row.
- [x] **Step 5: Close-out.** Mark Done in all four places (proposal header,
  README implementation-order row, this Progress table, this header's
  `**Done:**`). Re-derive the README ready set: #27 leaves it, **#28 is
  promoted** (its only dependency); flip #28's `Depends on:` line to built and
  add a "Re-derived on 2026-10-26" history entry. Remove the
  "Show exactly what went into each request, with the ability to pin and
  exclude." line from `CHANGELOG.md`'s Unreleased section; add no release row.
- [x] **Step 6: Checks.**

  ```bash
  scripts/quality-gate.sh fmt node lint typos deny
  CARGO_TARGET_DIR="$PWD/target" cargo clippy -p workspace-engine -p desktop-shell --all-targets --locked -- -D warnings
  CARGO_TARGET_DIR="$PWD/target" cargo nextest run -p workspace-engine --test context_inspector --test context_assembly --test permission_profiles --test prompt_cache --test round_budget --test foundation
  CARGO_TARGET_DIR="$PWD/target" cargo nextest run -p desktop-shell --lib
  CARGO_TARGET_DIR="$PWD/target" cargo run -p eval-harness -- run --tier deterministic
  npm run specs:check
  ```

  Do **not** run the workspace clippy or the workspace test suite; CI runs
  them. Record every result in the Progress row and say "CI outstanding".
- [x] **Step 7: Update the Progress row** and commit the close-out.
  Subject: `Close out the context inspector`.

---

## Review fixes (2026-10-28)

Post-merge review of the shipped inspector. Each finding is treated as a claim:
confirmed by reading the code or by a failing test, or recorded as disproved.
None was disproved.

- [x] **5a · The review card reopened the original manifest.** Confirmed at
  `app.js`'s `createContextReview`: the *Review context* button assigned the
  `review` object the card was created with, while removal updates
  `currentContextReview` and is by index. After one removal, reopening restored
  the removed item and the next removal addressed the wrong entry. The card now
  registers in `contextReviewCards` and owns one live review; removals apply the
  manifest the route returned to both the card and the open panel, and the
  card's token line and excluded count re-render. `contextReviewCards` is
  cleared on a repository change and the entry removed when a decision settles.
  Test: `two_removals_across_a_reopen_use_the_manifest_each_call_returns`
  removes one item, reopens through `PausedTurns::context_review_manifest`, and
  removes a second by its index in the reopened manifest. **Falsified:** making
  `save_context_review` a no-op made the reopen read 4 items instead of 3 and the
  test fail. The web UI has no JavaScript test runner, so the JS state bug is
  confirmed by reading and the regression test pins the engine contract the card
  now uses.
- [x] **5b · The restriction filters assembly only.** Confirmed:
  `context_restriction` was read at the three assembly sites (`chat.rs` twice,
  `edit.rs` once) and nowhere in tool dispatch, so `read_file`,
  `list_directory` and the search tools ignored it. The first pass documented
  the gap because `chat.rs` belonged to spec 51. **Fixed after that spec
  merged:** `dispatch_read_only_action` now refuses reads and listings outside
  the restriction, refuses a literal search path outside it, and filters
  wildcard and codebase search results, using
  `ContextManager::restriction_admits_path` so tools and assembly agree on
  normalisation. `the_session_restriction_refuses_reads_lists_and_searches_outside_it`
  proves the outside read, directory listing, root listing, literal search and
  both search tools; `an_empty_restriction_refuses_every_read_but_not_a_plain_answer`
  proves the no-path case. The panel hint, `index.html` label, `USER_GUIDE.md`,
  proposal §5.4 and `context.md` now describe the enforced behaviour.
- [x] **5c · Two send paths skip the pause.** Confirmed: `run_repair_turn`
  (`chat.rs:2563`) and the edit flow (`edit.rs:526`) built and sent without
  consulting `pause_for_context_review`; both still pass the session's pins and
  restriction. **Fixed for the repair turn after spec 51 merged:**
  `run_repair_turn` now assembles through the same `pause_for_context_review`
  route, the pending turn carries `verification_turn`, and
  `resume_after_context_decision` closes a resumed repair that answers without
  a patch through `close_verification_resume`.
  `a_repair_turn_pauses_for_context_review_and_resumes_as_a_repair` asserts no
  model call before Send and `NoRepairApplied` after. **The one-shot edit flow
  stays exempt:** it is a plain `POST /api/propose-edit` with no pending-turn
  record and no resume route, so pausing it would leave the request with
  nothing that could send or decline it. The guide and settings hint name only
  that path and say why.
- [x] **5d · Four nits.**
  - Directory entries now prefix-match. `build_context` marks an entry that
    resolves to a directory with a trailing `/`; `add_file` admits either an
    exact path or anything under a marked directory. Test:
    `a_restriction_to_a_directory_includes_everything_under_it` covers `src/`
    and a resolved `src`. **Falsified:** restoring exact equality made `src/`
    admit nothing and the test fail.
  - The **repository map** was exempt but unlisted. It is now named beside the
    symbols row in the panel hint, the user guide, proposal §5.4 and
    `context.md` §5.
  - The finding-repair dialog counted `ids.length`; it now counts `kept.length`,
    the findings the server actually put in the prompt. The server's own
    exclusion toast still reports what was left out.
  - The file-bearing-source restriction test pinned `src/other.rs`, the same
    path the keyword/semantic hit reaches, so the pin's `PathNotSelected`
    satisfied the assertion even if search were unfiltered. The pin is now on
    `src/pinned_only.rs` and the test asserts the hit on `src/other.rs` too.
    **Falsified:** exempting `RetrievedFile` from the restriction made
    `src/other.rs` disappear from the excluded list and the test fail. (Exempting
    only `KeywordHit` did not, because the same file is also a semantic hit;
    both reach `RetrievedFile`.)

**Checks** (scoped, 2026-10-28):

```bash
scripts/quality-gate.sh fmt node lint typos deny
cargo clippy -p workspace-engine -p desktop-shell --all-targets --locked -- -D warnings
cargo nextest run -p workspace-engine --test context_inspector --test context_assembly
cargo nextest run -p desktop-shell --lib
node --check crates/desktop-shell/static/app.js
npm run lint:web
```

Results: fast five passed; scoped clippy passed; `context_inspector` 32 passed
and `context_assembly` 48 passed (80 run, 2 skipped); `desktop-shell --lib` 210
passed / 6 skipped; `node --check` and `npm run lint:web` passed;
`npm run specs:check` passed (67 specs). The workspace clippy and test suite were
left to CI; CI is outstanding.

**Deferred work closed:** enforcing the path restriction in tool dispatch (5b)
and giving the verification repair turn the pre-send pause (5c) are implemented
on `chat-review-fixes` after spec 51 merged. The one-shot edit flow remains
exempt from the pause and is recorded as a limit above: it has no pending-turn
resume route. There is no remaining deferred item from this review.

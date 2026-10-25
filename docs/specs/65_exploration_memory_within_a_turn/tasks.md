# Exploration Memory — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Implements:** [`proposal.md`](proposal.md) · code map and collision notes in [`context.md`](context.md)
**Started:** 2026-10-25 — **Done:** 2026-10-25
**Status:** Done. Tasks 1–11 implemented and the live acceptance run on 2026-10-25 (results in [`proposal.md`](proposal.md) §6).

## Progress

| Task | State | Notes |
|---|---|---|
| 1 · `agent_max_turn_notes_chars` and its scope classification | Done | Field, default 8,000, `ABSOLUTE_TURN_NOTES_CHARS` (32,000), accept-0-and-clamp parser, restrict-only apply, `Capability` partition, profile carry and `equals_base_limit`, label "Notes size". Added the four `foundation.rs` tests, the repository-trust lowering/raising test with the audit assertion, and the permission-profile weakening case. `cargo nextest run -p workspace-engine --test foundation --test repository_config_trust --test permission_profiles`: 293 passed, 1 skipped. Mutation checks not run. |
| 2 · The per-turn call list and canonical call lines (extends #59) | Done | `read_call_key` became `call_key` (identical for the eight read tools, plus `turn_notes`), and `render_call_line` formats the already-resolved key rather than re-resolving paths. `RecordedCall { round, message_index, line, key, writes }` is appended after each dispatch from the same flags; denied calls are not recorded. Covered by the notice/warning and repeat tests in `tests/turn_notes.rs`; no separate call-list accessor is exposed. |
| 3 · The `turn_notes` tool, `ActionEffect::Notes`, and its bound | Done | `ToolAction::TurnNotes`, `ActionEffect::Notes`, both permission matrices, definition offered only when the key is above 0, decode (empty text is a named error), redaction before measuring, bound check, `turn_notes` session event and `turn_notes_recorded` audit with sizes only. Refusal wording recorded here: `notes: {current} / {limit} characters. Adding {chars} more characters to {current} would exceed the {limit}-character limit; rewrite the notes shorter with replace.` A replace over the limit says `The replacement is {chars} characters, over the {limit}-character limit; shorten it.` Tests in `tests/turn_notes.rs` (offered in every mode, refusal, inclusive boundary, replace clears carried, exploration round, identical append is a repeat, redaction). Mutation checks not run. |
| 4 · Pinning notes in `bounded_messages`, with the key=0 baseline | Done | `NotesWindow` (`none()` for key 0), notes block after the seed, notice in its old place, warning last. `budget` remains `max_messages - 3`. Inline tests compare the function against a verbatim pre-change copy across rounds 0–13 and windows 3/4/5/10/24/40, assert the notes block does not shrink the retained rounds, and place notes before the notice. `tests/turn_notes.rs` covers the 20-round `Answer now` path and windows 3/4/10/24. |
| 5 · The elision notice lists elided calls; the pre-cut warning | Done | The key-on notice keeps its first sentence and adds `Calls already made, oldest first:` (cap 40, `…and N more`); the warning is `The window is full. With your next request, round N leaves it:` (cap 10), then the `turn_notes` instruction and `Notes: n / limit characters.` Lines are redacted and truncated when recorded, and rendered from the same key the repeat rule uses. Key 0 keeps today's literal. Tests in `tests/turn_notes.rs`; the live acceptance may still revise the warning wording. |
| 6 · Persistence, resume paths, crash recovery and the seen-call rebuild | Done | `PendingChatTurn` gained `turn_notes`, `turn_notes_carried` and `calls` with `#[serde(default)]`; all seven save sites and six resume sites carry them. The seen set is rebuilt after the last `writes: true` entry. `SessionStore::record_turn_notes`/`read_turn_notes` over `active_events`; `notesChars` added to `model_request_prepared`, `turn_continued` and `round_budget_paused`. Deviation: the current recovery code has no resume path with only the log and no pending file, so the rebuild-from-messages path is exercised by a pending file carrying notes with an empty call list rather than by a log-only resume. Tests in `tests/turn_notes.rs` (approval resume, pre-change pending file, pre-Continue repeat, latest event). Mutation checks not run. |
| 7 · Carrying notes into the next turn, including a rewind | Done | Re-checked after #26 landed (main `e7e5258`, spec 26 Done 2026-10-25). The seed is still exactly two messages (`system` from `system_prompt_for_session`, `user` from the context plan), so the notes block stays at `messages[2]` and `HEAD_MESSAGES = 2` is unchanged; `build_model_prompt` now takes a `ContextPlan` and a scanner but is not called by this task. `carry_notes_from_the_previous_turn` uses the same ordering as `carry_plan_from_the_previous_turn`, writes a `carried: true` event before the first request, and audits its size. A replace clears carried; an append keeps it. Tests cover the second/third turn, a rewind, a new session, and a second turn after a pause; the whole scoped set passes on main (979 tests). |
| 8 · Redaction across every boundary | Done | A note is redacted before it is measured, stored, logged, sent or counted; a carried note is redacted again on read-back; notice/warning lines are redacted when recorded; the export redacts through its existing closure. Test seeds an AWS key and asserts it is absent from the request, session log, audit, result and Markdown export. Because the stored text is already redacted, the export's redaction count for notes is normally zero; the guarantee tested is absence of the secret. |
| 9 · UI disclosure, session export and the progress label | Done | `ChatTurnResult.turn_notes`/`TurnNotesView`; `chat_result_json` emits `turnNotes`; `task_states_json` joins `notes` by task id; `phase_json` and the worker progress file carry `notesChars`; `app.js` renders the collapsed **Notes** disclosure (carried label when applicable), the reload path joins task notes, and the progress label appends `· notes 2.1k`. `session.rs` exports Markdown and JSON notes. Shell tests pass (177). There is no JS test runner, so the `app.js` changes are checked by inspection, `node --check` and `npm run lint:web` — the same deviation #59 Task 4 recorded. |
| 10 · Eval harness: notes, repeated reads, `long_exploration_memory` | Done | `Scenario.follow_up_prompt` runs a second turn on the same session id; `trace::notes_calls`, `trace::repeated_elided_reads` and `trace::carried_notes` read the session log; `RunRecord` records all three and the text report has a `Notes and repeated reads` section. The scenario uses `agent_max_turn_messages=6`, batches a read with `turn_notes` in round 1, answers, then runs the follow-up. Deterministic run: `pass`, notes 1, carried 1, repeated reads 0; `cargo nextest run -p eval-harness`: 92 passed, 2 skipped. Deviation: the log cannot tell whether a repeat was caused by a cut, so `repeated_elided_reads` counts any repeated read in a task — an upper bound. Mutation checks not run. |
| 11 · Docs, live acceptance and close-out | Done | Docs and status records updated. **Live acceptance run on 2026-10-25** against `deepseek-v4-flash` with a temporary in-process driver and isolated data directories; results are in `proposal.md` §6. Within a turn: prompt 1 default 9 model calls / 1 note / 7 calls elided in the final request / answer used the elided status table and script correctly; prompt 1 at `0` 11 calls / 3 repeats; prompt 2 default 10 calls / 1 repeat; prompt 2 at `0` 33 calls / 1 *Continue* / 14 repeats. Across turns: the second task carried 2,809 characters before its first request and grew to 7,715/8,000; the answer named the first turn's places. Caveats recorded: prompt 1's `0` control had fewer repeats (3 vs 5), prompt 2's default cut was shallow, and `list_worktrees` returned no worktrees although `git worktree list` shows them (a spec-60/tool observation). The full seven-check gate is left to CI. |

**Goal:** Keep what the agent learned in front of it. A model-written, bounded
`turn_notes` record is pinned into every later request of a turn — never
elided, never a message in `messages` — the elision notice names the calls
that left the window, a warning names the round about to leave, and the record
carries into the next turn of the session. `agent_max_turn_notes_chars = 0`
restores today's requests byte for byte.

**Architecture:** The notes are turn state locals of `run_agentic_turn`
(`chat.rs:2505`), beside `messages`: `turn_notes`, a `carried` flag, and a
per-turn call list. `bounded_messages` (`chat.rs:6161`) gains a `NotesWindow`
parameter and inserts at most three extra system messages around the unchanged
bounded tail. The call list extends #59's `read_call_key` (`chat.rs:5116`) so
identity and display come from one resolution. Durability is a `turn_notes`
session event plus `PendingChatTurn` fields, with the seen-call set rebuilt
from the list on resume. Configuration is one restrict-only Capability key.

**Tech Stack:** Rust 2024, `serde`/`serde_json` (already dependencies),
vanilla JS in `app.js`, the existing session log and audit log. No new
dependencies.

## Global Constraints

Every task's requirements implicitly include this section.

- **Read [`context.md`](context.md) in full before Task 1.** It carries the
  current line references, the exact `bounded_messages` algorithm, every
  exhaustive `match` a new tool enters, and the collision notes. Do not
  re-derive any of that from `proposal.md` alone.
- **`agent_max_turn_notes_chars = 0` restores today's requests byte for byte**
  (requirement 3, acceptance criterion 2). No tool definition, no notes block,
  no warning, today's notice literal, and the same messages in the turn after.
  Task 4 pins this with a test-local copy of today's `bounded_messages` and
  `ModelRequest` equality (`tests/prompt_cache.rs:67` is the precedent).
- **Restrict-only, lower wins** (requirement 3). A repository may lower the
  bound or set it to `0`; a higher value is refused and audited as a
  restrict-only weakening attempt. The ceiling is
  `ABSOLUTE_TURN_NOTES_CHARS = 32_000`, defined once in `config.rs`.
- **Secret redaction is never bypassed** (requirement 9, `AGENTS.md`). A note
  goes through `SecretScanner::redact` before it is measured, stored, logged
  or sent. The audit record carries sizes, never text. The elision notice and
  the warning are model-facing too, so their lines are redacted.
- **Notes are never a message in `messages`** (proposal §5.2). A replace must
  not rewrite history, and the persisted array must stay what was dispatched.
- **Nothing here widens anything.** No mode, profile, permission, command
  policy, allow-list or approval decision changes. `turn_notes` is allowed in
  every mode and mapped to no capability key; that is the only permission
  change.
- **`chat.rs` is shared and one track runs at a time** (`docs/specs/README.md`
  → "Parallel work"). #26 landed on 2026-10-25 and rewrote
  `build_model_prompt`/context assembly; #66 edits the turn end and
  `PendingChatTurn`. Tasks 1–6 and 8–10 stay inside `bounded_messages`, the
  tool contract, config, the call list and the persistence sites. **Task 7 was
  re-checked after #26 landed**; its seed split and notes placement did not
  move. Do not run #65 beside #66.
- **#59's and #61's suites pass unchanged.** `tests/exploration_rounds.rs`,
  `tests/round_budget.rs`, `tests/token_ceiling.rs` and
  `tests/crash_recovery.rs` are the floor. If one fails, treat it as a
  regression in classification or persistence, not a test to update.
- **No command-policy change.** Nothing in this spec edits
  `command_policy.rs`, `command_allowlist`, `is_low_risk_read_only` or the
  shell-control gate.
- **Tests:** engine fixtures set `enable_index_watcher: false` and their own
  `data_dir` inside a temp directory. Model-dependent paths use
  `MockModelAdapter` or `DAMAIAN_MOCK_MODEL_RESPONSE`. Never touch the user's
  real `~/Library/Application Support`.
- **Falsify every load-bearing test.** Break what it guards, confirm it fails,
  revert, and record the mutation in the Progress row.
- **A private Cargo target directory per worktree.** Prefix every `cargo`
  command with `CARGO_TARGET_DIR="$PWD/target"` from the worktree root.
- **Scoped checks per task; the full seven-check gate runs on CI only**
  (`AGENTS.md`). Each task runs `scripts/quality-gate.sh fmt node lint typos
  deny`, `cargo clippy -p <crate> --all-targets --locked -- -D warnings` for
  every crate it touches, and the test files it names. The workspace clippy
  and test suite are left to CI.
- **Never `git commit` or `git push` unasked.** Each task ends by showing the
  change and the check results. One-line commit subject, no body.

## Corrections to the proposal found while planning

Each one names the task that owns it. Where a correction contradicts the
proposal's prose, the correction wins and the owning task updates the proposal
in the same change (C1, C3 and C9 were folded in during planning).

| # | Proposal said | Code says | Resolution |
|---|---|---|---|
| C1 | §2's `chat.rs`, `config.rs` and `effective_policy.rs` line references (`bounded_messages` 4802, call site 2331, `build_model_prompt` 4748, `PendingChatTurn` 4396, `action_effect` 4960, restrict loop 1602, partition 2812, label 161) | Drifted after #59–#62 merged: `bounded_messages` `chat.rs:6161`, call site `:2911`, `build_model_prompt` `:6101`, `PendingChatTurn` `:5696`, `action_effect` `:6322`, restrict loop `config.rs:1604–1612`, partition `:2843`, label `effective_policy.rs:168` | §2 updated on 2026-10-24. Every task re-checks a line before editing near it. **Planning; all tasks** |
| C2 | "`#59`'s seen-call set (its §5.3, not yet built) lives in the loop and is not persisted" | #59 is Done 2026-10-23. `seen_read_calls: HashSet<Vec<String>>` is at `chat.rs:2760`, keyed by `read_call_key` (`:5116`), computed once per round (`:3348`), cleared on a write (`:4601`), and starts empty on resume. It exists only for the eight read tools and is not display text | §2 rewritten. This spec extends the key to `turn_notes`, adds a rendered line over the same resolution, and persists the list. **Tasks 2, 6** |
| C3 | "#61 (Tasks 1–5 on main since 2026-10-19)" | #61 is Done 2026-10-23 with Tasks 1–7: the pause, both resume decisions, crash reattachment, `ABSOLUTE_TURN_ROUND_CAP` (64) at `chat.rs:5970` | §1 and §2 updated. Nothing to build. **Planning** |
| C4 | The deterministic harness "can check which calls a run made and how many rounds it used, but not what a request contained" | It records every request (`MockModelAdapter.requests`, `model.rs:372`) and asserts substrings already (`continuation_request_contains`, `answer_request_contains`; `runner.rs:117–152`). It cannot compare a whole request across two runs, and it runs exactly one user turn per scenario (`runner.rs:368`) | §2 corrected. The key=0 baseline uses `ModelRequest` equality in an engine test, not the harness. **Tasks 4, 10** |
| C5 | §5.9's `long_exploration_memory` runs "then a second turn in the same session" | A scenario has one user turn; `Scenario.turns` (`scenario.rs:164`) is the list of **model responses**, not user turns. `ask_with_session_with_options` accepts a `session_id`, so a second turn is possible but not wired | Task 10 adds `follow_up_prompt` to `Scenario`/the runner and runs it against the same session id. The across-turn engine tests do not depend on it. **Task 10** |
| C6 | Requirement 11: "the audit records of the forced-final and exhausted paths include the notes size" | There is no forced-final or exhausted audit event. The forced final is a `model_request_prepared` (`chat.rs:2925`); exhaustion is `tool_budget_exhausted_response` plus `StopReason::ToolBudget` and the task status. The other round records are `turn_continued` (`:3197`) and `round_budget_paused` (`:4920`) | Add `notesChars` to `model_request_prepared`, `turn_continued` and `round_budget_paused`; the notes-change record carries `chars`/`totalChars`. Requirement 11 reworded to "paused" paths. **Task 6** |
| C7 | §5.3: "Whichever of #59 and this spec is built first adds the list; the other reuses it" | #59 was built first and added the **identity** only, for read tools only (`read_call_key` returns `None` otherwise). There is no display rendering, no record for a notes or command call, no write marker and no persistence | §5.3 rewritten: extend the key function, render from the same resolution, add the record and the persistence. **Task 2** |
| C8 | §5.2: "None of the three blocks counts toward `max_messages`" | Today `budget = max_messages.saturating_sub(3)` accounts for head (2) and one notice (`chat.rs:6169`). Adding two blocks to that subtraction would shrink the round budget | Keep `max_messages - 3`; insert the extra blocks beside the bounded tail, so the request may exceed `max_messages` by their size and the kept rounds are unchanged. §5.2 and `context.md` §3 say so. **Task 4** |
| C9 | §1: "Nothing else from an earlier turn reaches it" | The plan does: `carry_plan_from_the_previous_turn` (`chat.rs:2410`, called at `:949`) carries it from a stopped or superseded task. Context assembly also adds repository context | §1 and §2 corrected; the plan is the precedent Task 7 reuses for finding the previous turn. **Task 7** |
| C10 | §5.1: "`mode_permits` allows it in every mode" | Permission is two exhaustive matrices: `mode_permits` (`mode.rs:233`) and `profile_permits` (`mode.rs:155`), combined by `permits` (`:139`). Placing the variant in one only does not compile | The tool is `Permission::Allowed` in `mode_permits` and maps to no capability key in `profile_permits`, like `AskUser`. **Task 3** |
| C11 | §5.6: a task resumed from the log "rebuilds its call list from the tool calls in its saved `messages`" | A task resumed **from the log** does not necessarily have `messages`: the durable forms are the session log and the `PendingChatTurn` file, which is what `recovery::reattach_pending_approvals` (`recovery.rs:339`) reads | Notes come from the latest `turn_notes` event; the call list comes from the pending file when present and is rebuilt from `messages` otherwise. Task 6 tests both sources. **Task 6** |
| C12 | §2: "`ShapesTurn` tools … cannot be batched with reads" implied the three effects are the whole story | `AskUser` is also `ShapesTurn` (spec 50), and `profile_permits` maps reads, `propose_plan`, `complete_step` and `ask_user` to no capability key | §2's wording widened without changing the design: a fourth variant `Notes` is added, batchable with reads but not `ReadOnly`. **Task 3** |

## File Structure

| File | Change |
|---|---|
| `crates/workspace-engine/src/config.rs` | `agent_max_turn_notes_chars`: field, overlay field, destructure, restrict-only apply, parser with clamp, `config_show`, overlay serializer, default, partition, `ABSOLUTE_TURN_NOTES_CHARS` (Task 1) |
| `crates/workspace-engine/src/profile.rs` | Carried capability key and `equals_base_limit` arm (Task 1) |
| `crates/workspace-engine/src/effective_policy.rs` | Label "Notes size" (Task 1) |
| `crates/workspace-engine/src/mode.rs` | `TurnNotes` in `mode_permits` and `profile_permits` (Task 3) |
| `crates/workspace-engine/src/chat.rs` | Call list and renderer (Task 2); the tool variant, effect, definition, decode, dispatch, bound, log and audit (Task 3); `NotesWindow` and pinning (Task 4); notice and warning (Task 5); `PendingChatTurn` fields, save/resume, rebuild, audit fields (Task 6); carry seed (Task 7); redaction points (Task 8); `ChatTurnResult.turn_notes`, `TurnPhase.notes_chars` (Task 9) |
| `crates/workspace-engine/src/session.rs` | `turn_notes` event writer, `read_turn_notes`, export rendering (Tasks 3, 6, 9) |
| `crates/workspace-engine/src/recovery.rs` | Reattachment already carries `PendingChatTurn`; only the "from the log" path needs notes (Task 6) |
| `crates/workspace-engine/tests/foundation.rs`, `repository_config_trust.rs`, `permission_profiles.rs` | The config key (Task 1) |
| `crates/workspace-engine/tests/exploration_rounds.rs` | Call list, notes-vs-budget, notice/warning, persistence and rebuild (Tasks 2–6) |
| `crates/workspace-engine/tests/turn_notes.rs` | **New (Task 7).** Cross-turn carry, rewind, *Ask again*, redaction across boundaries (Tasks 7, 8) |
| `crates/desktop-shell/src/lib.rs` | `chat_result_json`, `task_states_json`, `phase_json` (Task 9) |
| `crates/desktop-shell/src/workers.rs` | Progress-file notes count (Task 9) |
| `crates/desktop-shell/static/app.js`, `index.html`, `style.css`, `docs/ui-style-guide.html` | Notes disclosure, carried marker, progress label (Task 9) |
| `crates/eval-harness/src/{scenario,runner,trace,record}.rs`, `scenarios/long_exploration_memory.toml`, `tests/harness.rs` | Follow-up turn, notes and repeated-read metrics, the new scenario (Task 10) |
| `docs/USER_GUIDE.md`, `docs/TROUBLESHOOTING.md`, `SECURITY.md` (only if it lists budget classes), `docs/specs/README.md`, `CHANGELOG.md` | Task 11 |

## Order and parallelism

```
1 ──> 2 ──> 3 ──> 4 ──> 5 ──> 6 ──> 7 ──> 8 ──> 9 ──> 10 ──> 11
                                    ▲
          #26 must land before ─────┘
```

Sequential. Tasks 2–9 all edit `chat.rs`, so **one at a time**, and no task may
run beside #26 or #66 (`docs/specs/README.md` → "Parallel work"). Task 7 is
the only task that edits the new-turn seed and is blocked on #26; if #26 has
not landed when Task 6 finishes, stop and wait rather than implementing the
carry against the pre-#26 seed.

---

## Task 1: `agent_max_turn_notes_chars` and its scope classification

**Requirements:** 3 (the key, its default, clamp and scope) and acceptance
criterion 3. **Files:** `config.rs`, `profile.rs`, `effective_policy.rs`,
`tests/foundation.rs`, `tests/repository_config_trust.rs`,
`tests/permission_profiles.rs`.

Nothing reads the key yet. This task makes it exist, parse, clamp, merge per
scope and show in the policy view.

- [x] **Step 1: Write the failing tests.** Model them on spec 59 Task 1's
  four `foundation.rs` tests and its two trust/profile tests.
  - `turn_notes_chars_accept_zero_where_the_window_refuses_it`: `0` parses and
    applies; a value of `0` means "feature off" (requirement 3's last
    paragraph).
  - `turn_notes_chars_clamps_values_above_its_ceiling`: `999_999` parses and
    reads back as `32_000`.
  - `turn_notes_chars_defaults_to_8000`.
  - `turn_notes_chars_round_trips_through_config_show_and_the_overlay_serializer`.
  - `repository_config_may_lower_the_turn_notes_bound_but_not_raise_it`, with
    the weakening attempt asserted in the audit trail (the shape of
    `repository_config_may_lower_the_exploration_allowance_but_not_raise_it`).
  - `agent_max_turn_notes_chars_is_a_capability_key_with_a_human_name`
    ("Notes size").
  - In `tests/permission_profiles.rs`, add a `weakening_cases` entry for a
    repository raising the key. Do not change an existing case.

- [x] **Step 2: Run the tests and confirm they fail to compile** on the missing
  field:
  `CARGO_TARGET_DIR="$PWD/target" cargo nextest run -p workspace-engine --test foundation --test repository_config_trust --test permission_profiles`.

- [x] **Step 3: Implement**, following spec 59's `agent_max_exploration_rounds`
  end to end (`context.md` §7 lists every site):
  - `pub agent_max_turn_notes_chars: usize` in `Config`, default `8_000`;
  - `Option<usize>` in `ConfigOverlay`, plus the exhaustive destructure and
    both `to_policy_text`s;
  - `=> Capability` in the key partition;
  - a restrict-only apply block. It is `usize`, so use the existing
    `restrict_only_limit` loop if it fits, or a dedicated block as spec 59 did
    for its `u32`; do not change the loop's type;
  - `parse_turn_notes_chars`: accept `0..=32_000` and clamp larger values to
    `ABSOLUTE_TURN_NOTES_CHARS`; `parse_read_lines` refuses `0`, so it cannot
    be reused unchanged;
  - `pub const ABSOLUTE_TURN_NOTES_CHARS: usize = 32_000;` in `config.rs` with
    a doc comment in the style of `ABSOLUTE_TOOL_ROUND_CAP`: raising it past
    the constant requires explicit work, not configuration;
  - `profile.rs`: the carried key in `split_profile_keys` and
    `equals_base_limit`;
  - `effective_policy.rs`: `"agent_max_turn_notes_chars" => "Notes size"`.

- [x] **Step 4: Run the tests and confirm they pass**, then re-run `--test
  permission_profiles --test repository_config_trust --test foundation`
  together. `repository_config_trust.rs`'s existing tests must be unchanged
  apart from the new one.

- [ ] **Step 5: Mutation-test.** Record each in the progress row:
  1. Classify it `Preference` → the capability-name test fails.
  2. Apply it as a Free key → the repository trust test fails (the raise
     applies instead of being refused).
  3. Refuse `0` → the zero test fails.
  4. Remove the clamp → the clamp test fails.

- [x] **Step 6: Scoped checks.**
  ```bash
  scripts/quality-gate.sh fmt node lint typos deny
  CARGO_TARGET_DIR="$PWD/target" cargo clippy -p workspace-engine --all-targets --locked -- -D warnings
  CARGO_TARGET_DIR="$PWD/target" cargo nextest run -p workspace-engine --test foundation --test repository_config_trust --test permission_profiles
  ```

- [x] **Step 7: Update this tasks.md row** with what landed, the test result,
  the mutations, and any deviation.

## Task 2: The per-turn call list and canonical call lines (extends #59)

**Requirements:** the data half of 4 and 5, and 8's rebuild source. **Files:**
`chat.rs`, `tests/exploration_rounds.rs`.

This task produces no model-facing text. It creates the record the notice, the
warning and the resume rebuild all read, and it must not change #59's repeat
classification.

- [x] **Step 1: Write the failing tests** in `tests/exploration_rounds.rs`:
  - `the_call_list_records_each_dispatched_call_oldest_first`: a scripted
    three-round turn yields one record per dispatched call, with the round
    number, the assistant message index, the tool name and the line.
  - `a_rendered_call_line_names_the_checkout_and_the_range`: `read_file
    src/lib.rs 10–20` in the session checkout.
  - `the_same_path_in_two_checkouts_renders_two_lines`: the spec-60 worktree
    fixture already in this file.
  - `a_non_read_call_renders_from_its_action_marker`: `run_command cargo test`
    (redacted, truncated) and `turn_notes 812`.
  - `the_call_list_marks_the_round_that_wrote`.
  - `a_denied_call_is_not_recorded`: a mode-refused read never dispatched.
  - `the_repeat_rule_is_unchanged`: re-run #59's existing suite; no assertion
    changes.

- [x] **Step 2: Run them and confirm they fail** on the missing type:
  `CARGO_TARGET_DIR="$PWD/target" cargo nextest run -p workspace-engine --test exploration_rounds`.

- [x] **Step 3: Implement.**
  - Rename `read_call_key` to `call_key` (update the two call sites,
    `chat.rs:3348` and `:3682`) and keep its behaviour identical for the eight
    read tools. Add a `turn_notes` arm in Task 3.
  - `render_call_line(&self, repository_root, action) -> Option<String>` over
    the same resolved checkout/path/argument values. **Do not re-resolve**
    paths in the renderer; a second resolution is how display and identity
    drift.
  - `struct RecordedCall { round: u32, message_index: usize, line: String,
    key: Option<Vec<String>>, writes: bool }` with `Serialize`/`Deserialize`
    and `#[serde(default)]` on the container that holds it (Task 6).
  - `CallList` as a local of `run_agentic_turn`, appended after each marker
    closes (both the batch path and the sequential path), from the same
    `dispatched_read_key`/write flags already computed. A denied call is not
    appended.
  - Cap nothing here; the caps are Task 5's.

- [x] **Step 4: Run and confirm they pass**, plus the unchanged #59 tests:
  `--test exploration_rounds --test round_budget --test token_ceiling`.

- [ ] **Step 5: Mutation-test.** Record each in the progress row:
  1. Render the raw `worktree` argument instead of the resolved checkout →
     the two-checkout test fails.
  2. Record only read calls → the non-read test fails.
  3. Append before permission is checked → the denied-call test fails.
  4. Store the message index as 0 for every call → Task 5's boundary test
     fails (record it now, run it after Task 5).

- [x] **Step 6: Scoped checks.**
  ```bash
  scripts/quality-gate.sh fmt node lint typos deny
  CARGO_TARGET_DIR="$PWD/target" cargo clippy -p workspace-engine --all-targets --locked -- -D warnings
  CARGO_TARGET_DIR="$PWD/target" cargo nextest run -p workspace-engine --test exploration_rounds --test round_budget --test token_ceiling
  ```

- [x] **Step 7: Update this tasks.md row.**

## Task 3: The `turn_notes` tool, `ActionEffect::Notes`, and its bound

**Requirements:** 1, 3 (the bound and refusal), 8 (a notes round is
exploration; an identical append is a repeat), 9 (redaction on write), 11
(the notes-change audit and log). **Files:** `chat.rs`, `mode.rs`,
`session.rs`, `tests/exploration_rounds.rs`, `tests/foundation.rs`.

The tool exists and changes `turn_notes`; nothing pins it into a request yet
(Task 4).

- [x] **Step 1: Write the failing tests.**
  - `a_notes_call_is_offered_in_every_mode`: the definition list contains
    `turn_notes` in Ask, Plan, Code and Review.
  - `a_notes_call_is_not_offered_when_the_bound_is_zero`: no definition and a
    hand-written call is undecodable/refused.
  - `a_notes_append_is_refused_over_the_bound_and_the_record_is_unchanged`:
    the refusal names the current size and the limit and suggests `replace`;
    the following success is still within the bound.
  - `the_boundary_is_inclusive`: a write landing exactly on the limit is
    accepted and one character over is refused.
  - `a_notes_replace_shortens_the_record_and_clears_carried`.
  - `a_round_of_reads_and_notes_is_an_exploration_round`: one read plus one
    notes call charges `exploration_rounds`, not `action_rounds`.
  - `an_identical_append_is_a_repeat_and_is_charged_as_an_action_round`: the
    second identical append costs an action round, while a different append
    does not.
  - `a_notes_call_writes_one_turn_notes_event_and_one_audit_record_without_text`:
    the event carries `taskId`, `replace`, `carried`, `text`, `chars`,
    `totalChars`; the audit carries sizes only and the text is absent from it.
  - `a_secret_in_a_note_is_redacted_before_it_is_stored`.

- [x] **Step 2: Run them and confirm they fail to compile** on the missing
  variant: `--test exploration_rounds`, `--test foundation`.

- [x] **Step 3: Implement** (every `match` is listed in `context.md` §4):
  - `ToolAction::TurnNotes { text: String, replace: bool }`; decode through
    `tool_action_from_call` with an error for empty `text` (the `ask_user`
    precedent) and `replace` defaulting to false.
  - `turn_notes_tool_definition()` beside `read_file_tool_definition`
    (`chat.rs:6824`), offered only when
    `self.config.agent_max_turn_notes_chars > 0` and `permits` allows a
    placeholder `TurnNotes`.
  - `ActionEffect::Notes`; `action_is_batchable_read_only` and the round
    classification both become `ReadOnly | Notes`; the `action_effect` doc
    comment names the budget decision.
  - `mode.rs`: `TurnNotes` is `Permission::Allowed` in `mode_permits` and maps
    to `None` in `profile_permits`.
  - `tool_action_marker` → `("turn_notes", chars, false)`;
    `tool_event_payload` → `("turn_notes", Vec::new())`;
    `tool_action_label` → "Writing notes" / "Updating notes".
  - Dispatch: redact `text` first, then append/replace on the loop thread
    after a batch returns. Refuse an append whose result would exceed the
    limit and a replace whose own text exceeds it; return
    `notes: {total} / {limit} characters`.
  - `SessionStore::record_turn_notes(task, replace, carried, text)` writing the
    `turn_notes` event; `AuditLog::record("turn_notes_recorded", …)` with
    `taskId`, `replace`, `carried`, `chars`, `totalChars` and no text.
  - `call_key` gains the `turn_notes` arm: `["turn_notes", replace,
    text_hash]` (a stable hash of the redacted text), so an identical append
    is a repeat and a replace is never a repeat.

- [x] **Step 4: Run and confirm they pass**, and re-run #59's and #61's suites:
  `--test exploration_rounds --test round_budget --test token_ceiling --test foundation`.

- [ ] **Step 5: Mutation-test.** Record each:
  1. `Notes` as `ShapesTurn` → the read+notes batch exploration test fails.
  2. `Notes` charged as an action round → the same test fails.
  3. Redact after storing instead of before → the secret-in-note test fails.
  4. Put the text in the audit record → the audit test fails.
  5. Refuse at `>=` rather than `>` → the boundary test fails.

- [x] **Step 6: Scoped checks.**
  ```bash
  scripts/quality-gate.sh fmt node lint typos deny
  CARGO_TARGET_DIR="$PWD/target" cargo clippy -p workspace-engine --all-targets --locked -- -D warnings
  CARGO_TARGET_DIR="$PWD/target" cargo nextest run -p workspace-engine --test exploration_rounds --test round_budget --test token_ceiling --test foundation
  ```

- [x] **Step 7: Update this tasks.md row.** Record the exact refusal wording,
  because Task 5's notice and Task 9's UI must not restate it.

## Task 4: Pinning notes in `bounded_messages`, with the key=0 baseline

**Requirements:** 2, 3's last paragraph, and the first two acceptance
criteria. **Files:** `chat.rs`, `tests/exploration_rounds.rs`.

- [x] **Step 1: Write the failing tests.**
  - `a_note_from_round_two_is_in_every_later_request`: a 20-round scripted
    turn that passes through #61's *Continue*; the note appears unchanged in
    every later `MockModelAdapter.requests` entry, including the forced final
    call *Answer now* makes.
  - `notes_survive_the_narrowest_window`: for `agent_max_turn_messages` 3..24
    the notes are still `messages[2]` (the first block after the seed) and are
    never dropped.
  - `the_notes_block_does_not_shrink_the_round_budget`: with notes on, the
    number of round messages kept at a given `max_messages` equals the number
    today's algorithm keeps.
  - `the_notes_block_sits_before_the_notice`: order is seed, notes, notice,
    tail.
  - `the_carried_header_is_used_while_carried_text_remains` and
    `a_replace_switches_back_to_the_plain_header` (the latter needs Task 3's
    replace and Task 7's carry; mark it `#[ignore]` with the Task 7 command if
    it cannot pass yet, or move it to Task 7).
  - `with_notes_off_every_request_is_byte_identical_to_todays`: a test-local
    `bounded_messages_before_65` copy of today's algorithm
    (`chat.rs:6161–6200`, copied verbatim, not called), compared for a matrix
    of lengths and `max_messages` including 3, 4, 10, 24 and larger than the
    array; plus a recorded-request assertion that the whole `ModelRequest` of
    a cut turn equals the one built by the copy.

- [x] **Step 2: Run them and confirm they fail** on the signature change.

- [x] **Step 3: Implement.**
  - `struct NotesWindow<'a> { notes: &'a str, carried: bool, calls: &'a CallList, notice_cap: usize, warning_cap: usize }`
    with `NotesWindow::none()`.
  - `bounded_messages(messages, max_messages, window)`; compute `head`, `tail`,
    `budget`, `drop_count`, `results` exactly as today.
  - Insert the notes block after `head` when `notes` is non-empty, with the
    header from proposal §5.2. Keep `budget = max_messages.saturating_sub(3)`.
  - Leave the notice text as today's literal for now (Task 5 replaces it when
    notes are on).
  - Thread `turn_notes` and `carried` from `run_agentic_turn` into the request
    at `chat.rs:2911`.

- [x] **Step 4: Run and confirm they pass**, plus `tests/prompt_cache.rs`
  (prefix stability) and `tests/round_budget.rs`.

- [ ] **Step 5: Mutation-test.** Record each:
  1. Subtract the extra blocks from `budget` → the round-budget test fails.
  2. Put the notes after the tail → the round-2 test fails.
  3. Emit the block when the key is `0` → the byte-identical test fails.
  4. Drop the notes when `notes.len()` is small → the narrow-window test
     fails.

- [x] **Step 6: Scoped checks.**
  ```bash
  scripts/quality-gate.sh fmt node lint typos deny
  CARGO_TARGET_DIR="$PWD/target" cargo clippy -p workspace-engine --all-targets --locked -- -D warnings
  CARGO_TARGET_DIR="$PWD/target" cargo nextest run -p workspace-engine --test exploration_rounds --test round_budget --test prompt_cache
  ```

- [x] **Step 7: Update this tasks.md row.**

## Task 5: The elision notice lists elided calls; the pre-cut warning

**Requirements:** 4 and 5, and their acceptance criteria. **Files:**
`chat.rs`, `tests/exploration_rounds.rs`.

- [x] **Step 1: Write the failing tests.**
  - `the_elision_notice_lists_every_elided_call_oldest_first`: the lines match
    the call list entries whose `message_index` precedes the cut.
  - `the_elision_notice_caps_the_list_with_a_remainder`: 45 elided calls print
    40 lines and "…and 5 more".
  - `a_read_in_two_checkouts_appears_as_two_lines`.
  - `the_notice_lists_a_notes_call_too` (the Task 3 identity rendered).
  - `the_warning_names_the_oldest_round_in_view_once_the_window_is_full`: when
    `tail.len() >= budget`, the last message is a system warning naming the
    round at `tail[0]` and its calls; capped at 10.
  - `no_warning_appears_before_the_window_is_full`.
  - `the_next_notice_lists_the_round_the_warning_named`: capture the warning
    of request *n*, then assert request *n+1*'s notice contains those calls.
  - `with_notes_off_the_notice_and_the_tail_are_todays`: with the key at `0`
    the notice literal is unchanged and no warning appears.

- [x] **Step 2: Run them and confirm they fail.** They fail on the notice
  being today's literal and the warning being absent.

- [x] **Step 3: Implement.**
  - When notes are on: compute the elided entries as
    `call.message_index < 2 + drop_count` (the cut is in the original array;
    `drop_count` counts `tail` messages, so the boundary is
    `2 + drop_count`), render with `render_call_line`, cap at 40, append
    "…and N more".
  - The notice keeps its first sentence (`{drop_count} messages ({results}
    call results) omitted`) and says the notes are above.
  - The warning: when `tail.len() >= budget`, name the oldest round in view by
    its position in the turn, list its calls (cap 10), then "Record in
    turn_notes anything from these results you still need." and
    `Notes: {n} / {limit} characters.` Push it after the newest round.
  - Both blocks redact their lines through `SecretScanner::redact`.
  - With notes off, the function must take exactly today's path (Task 4's
    byte-identical test is the gate).

- [x] **Step 4: Run and confirm they pass.**

- [ ] **Step 5: Mutation-test.** Record each:
  1. List the retained calls instead of the elided ones → the oldest-first
     test fails.
  2. Use `tail.len() > budget` for the warning → the no-warning test (or the
     next-notice test) fails.
  3. Cap at 10 instead of 40 for the notice → the remainder test fails.
  4. Skip redaction of the line → Task 8's notice test fails (record it now).

- [x] **Step 6: Scoped checks.**
  ```bash
  scripts/quality-gate.sh fmt node lint typos deny
  CARGO_TARGET_DIR="$PWD/target" cargo clippy -p workspace-engine --all-targets --locked -- -D warnings
  CARGO_TARGET_DIR="$PWD/target" cargo nextest run -p workspace-engine --test exploration_rounds --test round_budget
  ```

- [x] **Step 7: Update this tasks.md row.** Record both exact block texts; the
  live acceptance in Task 11 may revise the warning, and the revision must be
  recorded here.

## Task 6: Persistence, resume paths, crash recovery and the seen-call rebuild

**Requirements:** 7, 8's last paragraph, the second half of 11. **Files:**
`chat.rs`, `session.rs`, `recovery.rs`, `tests/round_budget.rs`,
`tests/crash_recovery.rs`, `tests/exploration_rounds.rs`.

- [x] **Step 1: Write the failing tests.**
  - `a_continued_turn_keeps_its_notes_and_its_call_list`: a note written
    before the #61 pause is in the first request after *Continue*, unchanged.
  - `answer_now_carries_the_notes_block`: the forced final request has the
    notes and no tools.
  - `a_command_approval_resume_keeps_the_notes`.
  - `a_pending_turn_written_before_notes_loads_with_none`: hand-write a
    pre-change pending file; it resumes with empty notes and no call list.
  - `a_repeat_made_before_a_continue_is_still_a_repeat_after_it`: the same
    read before and after the pause makes the post-resume round an action
    round.
  - `the_seen_call_set_is_rebuilt_only_since_the_last_write`: read, write,
    read; after a resume only the post-write read is in the set.
  - `notes_are_rebuilt_from_the_latest_turn_notes_event_after_a_crash`.
  - `the_call_list_is_rebuilt_from_the_saved_messages_when_only_the_log_is_available`.
  - `model_request_prepared_turn_continued_and_round_budget_paused_record_the_notes_size`.

- [x] **Step 2: Run them and confirm they fail.**

- [x] **Step 3: Implement.**
  - `PendingChatTurn` gains `#[serde(default)] turn_notes: String`,
    `#[serde(default)] turn_notes_carried: bool` and
    `#[serde(default)] calls: Vec<RecordedCall>`; every one of the seven
    construction sites (`chat.rs:3496`, `:3765`, `:4141`, `:4253`, `:4376`,
    `:4760`, `:4881`) fills them, and all six resume calls to
    `run_agentic_turn` (`:1430`, `:1555`, `:1707`, `:1847`, `:1949`, `:1983`)
    pass them back.
  - On resume, `seen_read_calls` is rebuilt from `calls` after the last
    `writes: true` entry (`None` keys skipped). Keep the clear-on-write rule.
  - `SessionStore::read_turn_notes(session_id) -> BTreeMap<String, TurnNotes>`
    over `active_events`, latest event per task.
  - The crash path: notes from `read_turn_notes`; calls from the pending file
    when present, else rebuilt from `messages` with `tool_action_from_call`
    (`chat.rs:6924`) and `call_key`. Test both sources explicitly (C11).
  - `notesChars` on `model_request_prepared`, `turn_continued` and
    `round_budget_paused`. Update proposal requirement 11 to name the paused
    paths (C6).

- [x] **Step 4: Run and confirm they pass**, plus the untouched suites:
  `--test round_budget --test token_ceiling --test crash_recovery --test exploration_rounds`.

- [ ] **Step 5: Mutation-test.** Record each:
  1. Omit a field from one save site → the corresponding resume test fails.
  2. Rebuild the set from all entries rather than since the last write → the
     since-last-write test fails.
  3. Read the earliest `turn_notes` event instead of the latest → the crash
     test fails.
  4. Skip `#[serde(default)]` → the pre-change pending test fails to load.

- [x] **Step 6: Scoped checks.**
  ```bash
  scripts/quality-gate.sh fmt node lint typos deny
  CARGO_TARGET_DIR="$PWD/target" cargo clippy -p workspace-engine --all-targets --locked -- -D warnings
  CARGO_TARGET_DIR="$PWD/target" cargo nextest run -p workspace-engine --test round_budget --test token_ceiling --test crash_recovery --test exploration_rounds
  ```

- [x] **Step 7: Update this tasks.md row.** State plainly whether the crash
  path had a stored pending file or only the log, for each test.

## Task 7: Carrying notes into the next turn, including a rewind

**Requirements:** 6 and its acceptance criterion. **Files:** `chat.rs`,
`session.rs`, `tests/turn_notes.rs` (new).

**#26 landed on 2026-10-25** (main `e7e5258`). Re-read `context.md` §10 and
re-check every `chat.rs` reference it names before editing. The seed is still
two messages and `bounded_messages` is unchanged, so this task's split point
and notes block did not move; `build_model_prompt` now takes a `ContextPlan`
and a scanner, and this task does not call it. The reference re-check is
recorded in the progress row.

- [x] **Step 1: Write the failing tests** in a new
  `crates/workspace-engine/tests/turn_notes.rs`, `enable_index_watcher:
  false`:
  - `the_next_turn_starts_with_the_previous_turns_final_notes`: the second
    turn's first recorded request contains the notes block under the carried
    header, and a `turn_notes` event with `carried: true` names the first
    task.
  - `a_third_turn_starts_with_the_second_turns_replacement`.
  - `a_rewind_carries_from_the_turn_before_the_rewound_one`: drive
    `rewind_conversation` (`session.rs:2252`) past the second turn, then start
    a new turn; it carries the first turn's notes.
  - `a_new_session_starts_with_no_notes`.
  - `ask_again_carries_the_stopped_turns_notes`: a
    `ToolBudgetExhausted` task → `tool_budget_ask_again_prompt`
    (`chat.rs:2004`) → the new turn's first request carries them.
  - `a_carry_is_audited_with_its_size_and_marked_carried`.
  - `with_notes_off_no_carry_event_is_written`.

- [x] **Step 2: Run and confirm they fail.**

- [x] **Step 3: Implement.**
  - In `ask_with_session_with_options`, before `run_agentic_turn`, find the
    previous task with the same ordering `carry_plan_from_the_previous_turn`
    uses (`read_tasks`, exclude the current task), read its final notes with
    `read_turn_notes`, and seed `turn_notes`/`carried`.
  - Write the `turn_notes` carry event (redacted text, `carried: true`,
    `sourceTaskId`) before the first request, plus the size-only audit record.
  - Pass the seed into `run_agentic_turn` as new parameters beside
    `exploration_rounds`.
  - `carried` stays true until a `replace` (Task 3), so an append keeps the
    carried header.

- [x] **Step 4: Run and confirm they pass**, plus `tests/round_budget.rs` and
  `tests/prompt_cache.rs`.

- [ ] **Step 5: Mutation-test.** Record each:
  1. Pick the newest task instead of the one before this turn → the rewind
     test fails.
  2. Write the carry event after the first request → the first-request test
     fails.
  3. Set `carried: false` on the seed → the header and audit tests fail.

- [x] **Step 6: Scoped checks.**
  ```bash
  scripts/quality-gate.sh fmt node lint typos deny
  CARGO_TARGET_DIR="$PWD/target" cargo clippy -p workspace-engine --all-targets --locked -- -D warnings
  CARGO_TARGET_DIR="$PWD/target" cargo nextest run -p workspace-engine --test turn_notes --test round_budget --test prompt_cache
  ```

- [x] **Step 7: Update this tasks.md row.** Record the #26 commit/merge the
  task was started against.

## Task 8: Redaction across every boundary

**Requirements:** 9 and its acceptance criterion. **Files:** `chat.rs`,
`session.rs`, `tests/turn_notes.rs`, `tests/exploration_rounds.rs`.

Task 3 redacts on write; this task proves every crossing and fixes what is
still missing.

- [x] **Step 1: Write the failing tests** with a seeded fake credential (the
  repository's existing pattern):
  - `a_secret_in_a_note_never_reaches_the_request`.
  - `a_secret_in_a_note_is_redacted_in_the_session_log_and_the_audit`.
  - `a_secret_in_carried_notes_is_redacted_in_the_next_turns_request`.
  - `a_secret_in_a_note_is_redacted_in_the_export_and_counted`: the export's
    redaction count includes it.
  - `the_notice_line_for_a_command_carrying_a_secret_is_redacted`.
  - `a_secret_in_a_note_does_not_reach_the_notes_size_audit`: the audit has
    only sizes.

- [x] **Step 2: Run them and confirm which fail.** At least the export and the
  notice line are expected to fail until wired.

- [x] **Step 3: Implement** the missing redaction points only. Do not add a
  redaction pass that re-redacts already-redacted text in a way that changes
  the size accounting; measure the redacted text (the `redact` result's
  `.text.len()`).

- [x] **Step 4: Run and confirm they pass.**

- [ ] **Step 5: Mutation-test.** Record each:
  1. Skip redaction on the carried read-back → the carried test fails.
  2. Export the raw stored text (bypass the export's `redact` closure) → the
     export test fails.
  3. Print the raw command reference in the notice → the notice test fails.

- [x] **Step 6: Scoped checks.**
  ```bash
  scripts/quality-gate.sh fmt node lint typos deny
  CARGO_TARGET_DIR="$PWD/target" cargo clippy -p workspace-engine --all-targets --locked -- -D warnings
  CARGO_TARGET_DIR="$PWD/target" cargo nextest run -p workspace-engine --test turn_notes --test exploration_rounds
  ```

- [x] **Step 7: Update this tasks.md row.**

## Task 9: UI disclosure, session export and the progress label

**Requirements:** 10 and its acceptance criterion, plus §5.8. **Files:**
`chat.rs`, `session.rs`, `desktop-shell/src/lib.rs`,
`desktop-shell/src/workers.rs`, `desktop-shell/static/app.js`,
`index.html`, `style.css`, `docs/ui-style-guide.html`.

- [x] **Step 1: Write the failing tests.**
  - `the_turn_result_carries_the_final_notes_and_whether_they_were_carried`
    (engine).
  - `a_reloaded_session_joins_each_turns_notes_by_task_id`
    (`task_states_json` includes `notes` for a task that has it and omits it
    otherwise).
  - `the_export_includes_each_turns_notes_marked_carried` (Markdown and JSON).
  - `phases_carry_the_notes_size` and `progress_lines_mirror_the_turn`
    extended for the new field.
  - `workers_list_groups_by_source_repository` extended for the mirrored
    field.
  - There is no JS test runner, so the `app.js` disclosure and label are
    checked by inspection, `node --check` and `npm run lint:web`; record that
    as a deviation like #59 Task 4 did.

- [x] **Step 2: Run them and confirm they fail.**

- [x] **Step 3: Implement.**
  - `ChatTurnResult.turn_notes: Option<TurnNotesView>` (text + carried), and
    `chat_result_json` (`desktop-shell/src/lib.rs:4141`) emits `turnNotes`.
  - `handle_session` (`lib.rs:543`) reads `read_turn_notes` and
    `task_states_json` (`:4703`) adds `notes` beside `plan`.
  - `app.js`: under the assistant answer, `createDisclosure("Notes", panel)`
    with the final text; the trigger label says "Notes (carried from the
    previous turn)" when carried; nothing when empty. Append the panel the way
    `appendContextDisclosure` (`:7441`) does.
  - `session.rs` `export_session` (`:2094`) renders each task's notes, through
    the existing `redact` closure.
  - `TurnPhase` (`chat.rs:168`) gains `notes_chars`, `TurnSink::phase`
    (`chat.rs:262`) gains the argument at its three call sites
    (`:965`, `:2946`, `:3618`), `phase_json` (`lib.rs:3904`) and
    `workers.rs:1101` mirror it, and `app.js:4481` appends
    `· notes {n.n}k` when non-zero.
  - Add the pattern to `docs/ui-style-guide.html` and the class to
    `style.css`; follow the existing 2-space style and Biome rules.

- [x] **Step 4: Run and confirm they pass**, plus `cargo nextest run -p
  desktop-shell`, `node --check crates/desktop-shell/static/app.js` and
  `npm run lint:web`.

- [ ] **Step 5: Mutation-test.** Record each:
  1. Omit `notes` from `task_states_json` → the reload test fails.
  2. Emit `turnNotes` with the wrong carried flag → the engine/shell test
     fails.
  3. Render the label from `explorationRound` → the phase-label test fails.

- [x] **Step 6: Scoped checks.**
  ```bash
  scripts/quality-gate.sh fmt node lint typos deny
  CARGO_TARGET_DIR="$PWD/target" cargo clippy -p workspace-engine --all-targets --locked -- -D warnings
  CARGO_TARGET_DIR="$PWD/target" cargo clippy -p desktop-shell --all-targets --locked -- -D warnings
  CARGO_TARGET_DIR="$PWD/target" cargo nextest run -p workspace-engine --test turn_notes --test exploration_rounds
  CARGO_TARGET_DIR="$PWD/target" cargo nextest run -p desktop-shell
  ```

- [x] **Step 7: Update this tasks.md row.**

## Task 10: Eval harness: notes, repeated reads, `long_exploration_memory`

**Requirements:** 11's last sentence and the scenario in §5.9. **Files:**
`crates/eval-harness/src/{scenario,runner,trace,record}.rs`,
`crates/eval-harness/scenarios/long_exploration_memory.toml`,
`crates/eval-harness/tests/harness.rs`.

- [x] **Step 1: Write the failing tests.**
  - `a_scenario_can_run_a_follow_up_turn_in_the_same_session`: a scenario with
    `follow_up_prompt` makes a second `ask_with_session_with_options` call with
    the first turn's session id and reports two tasks.
  - `notes_calls_come_from_the_session_log`: `trace::notes_calls` counts
    `turn_notes` action markers, not scripted turns.
  - `a_read_that_repeats_an_elided_call_is_counted`: a synthetic log with a
    duplicate read marker after a cut reports one repeated read.
  - `the_long_exploration_memory_scenario_completes_and_carries_its_notes`:
    completes, one notes call, zero repeated reads, and the follow-up turn's
    first request contains the notes; all `deterministic_only`.
  - The pinned scenario counts in `tests/harness.rs` move by one.

- [x] **Step 2: Run them and confirm they fail.**

- [x] **Step 3: Implement.**
  - `Scenario.follow_up_prompt: Option<String>` (wire shape
    `follow_up_prompt`), parsed at the boundary and run after the first turn
    finishes, reusing `session_id`.
  - `Trace::notes_calls` over the existing `action_started` walker (the
    `READ_ONLY_ACTION_MARKERS` precedent, spec 59 Task 5).
  - `RunRecord.notes_calls` and `repeated_elided_reads`; report them in the
    text report beside `exploration_rounds`.
  - `scenarios/long_exploration_memory.toml`: `rust-workspace` fixture, 20
    scripted responses; round 2 batches `read_file` with `turn_notes`;
    `agent_max_turn_messages` low enough to cut (via `config_overlay`), the
    answer at the end, then `follow_up_prompt` with a second script segment.
  - Do not assert on the live model. The script decides the calls, so the
    assertions are `deterministic_only`.

- [x] **Step 4: Run and confirm they pass**, and run the whole deterministic
  tier: `cargo run -p eval-harness -- run --tier deterministic`.

- [ ] **Step 5: Mutation-test.** Record each:
  1. Count notes from scripted turns → the trace test fails.
  2. Start the follow-up as a new session → the carry assertion fails.
  3. Count every repeated read rather than only those after a cut → the
     scenario fails (or the synthetic test fails).

- [x] **Step 6: Scoped checks.**
  ```bash
  scripts/quality-gate.sh fmt node lint typos deny
  CARGO_TARGET_DIR="$PWD/target" cargo clippy -p eval-harness --all-targets --locked -- -D warnings
  CARGO_TARGET_DIR="$PWD/target" cargo nextest run -p eval-harness
  CARGO_TARGET_DIR="$PWD/target" cargo run -p eval-harness -- run --tier deterministic
  ```

- [x] **Step 7: Update this tasks.md row.** Record the scenario's observed
  notes-call count, repeated-read count and model-call count.

## Task 11: Docs, live acceptance and close-out

**Requirements:** proposal §5.10 and §6 in full. **Files:**
`docs/USER_GUIDE.md`, `docs/TROUBLESHOOTING.md`, `SECURITY.md` (only if it
lists per-key budget classes by then), `docs/specs/README.md`,
`CHANGELOG.md`, this file, `proposal.md`.

- [x] **Step 1: Docs.**
  - `USER_GUIDE.md`: what notes are, that they stay in view after earlier
    rounds are dropped, that they carry into the next turn of the session,
    where they appear (the collapsed **Notes** disclosure and the export), the
    `agent_max_turn_notes_chars` row (default 8,000, lower only, `0` off), and
    the elision notice and warning in plain words.
  - `TROUBLESHOOTING.md`: why an answer may rely on notes, why a later turn
    knows something it did not re-read, what the notice and warning mean, and
    how to read `turn_notes`/`turn_notes_recorded` events in a session log and
    the audit. Separate "notes were off" from "the model took none".
  - `SECURITY.md`: only if it lists per-key budget classes; notes are
    redacted before storage and sending, and carry no authority.

- [x] **Step 2: Live acceptance, within a turn.** Against `deepseek-v4-flash`
  on this repository, with an isolated `DAMAIAN_DATA_DIR` in this worktree:
  run #59's motivating prompt and "Where is the command approval decision
  made, what feeds it, and which specs changed it?", each at the default and
  at `0`, continuing through #61 until the model answers or the hard cap.
  Record rounds, *Continue*s, notes calls, repeated elided reads, and every
  claim that depends on an elided result, checked against the session log.
  **Accepted only if the default run's answer uses findings from elided rounds
  correctly.** If it does not, revise the tool description or the warning and
  repeat the run; do not enlarge the window. Record the revision in Task 3's
  or Task 5's progress row.

- [x] **Step 3: Live acceptance, across turns.** In the same session as the
  second prompt's default run, ask "Add a new approval cause for commands that
  write outside the repository: where does it go and what has to change?".
  Accepted if the answer uses the carried notes without re-reading everything,
  and the repeated-read count from the first turn is recorded.

- [x] **Step 4: Status and indexes.** In the same change:
  - `proposal.md` `Status:` → `Done YYYY-MM-DD` with the live results in §6;
  - the README implementation-order row for #65 and the "What to build next"
    ready set;
  - this file's header → `**Started:** YYYY-MM-DD — **Done:** YYYY-MM-DD` and
    every Progress row Done;
  - remove the notes bullet from `CHANGELOG.md`'s `Unreleased` section (the
    line beginning "Let the agent keep short notes while it investigates").
    Do not add a release row.
  - Re-run `grep -rn "65_exploration_memory_within_a_turn" docs/ CHANGELOG.md`
    and confirm every link resolves to a file that exists. `npm run
    specs:check -- --strict` must be clean.

- [x] **Step 5: Close-out checks.**
  ```bash
  scripts/quality-gate.sh fmt node lint typos deny
  typos docs/specs
  npm run specs:check -- --strict
  CARGO_TARGET_DIR="$PWD/target" cargo clippy -p workspace-engine --all-targets --locked -- -D warnings
  CARGO_TARGET_DIR="$PWD/target" cargo clippy -p desktop-shell --all-targets --locked -- -D warnings
  CARGO_TARGET_DIR="$PWD/target" cargo clippy -p eval-harness --all-targets --locked -- -D warnings
  CARGO_TARGET_DIR="$PWD/target" cargo nextest run -p workspace-engine --test turn_notes --test exploration_rounds --test round_budget --test crash_recovery
  CARGO_TARGET_DIR="$PWD/target" cargo nextest run -p desktop-shell
  CARGO_TARGET_DIR="$PWD/target" cargo nextest run -p eval-harness
  ```
  The workspace clippy and the workspace test suite are **left to CI**
  (`AGENTS.md`); do not run them locally. Report which local checks ran and
  that CI's Quality workflow is the remaining gate. The user pushes.

- [x] **Step 6: Update this tasks.md row** and the header with the real dates
  and the CI run number once it is green.

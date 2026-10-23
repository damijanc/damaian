# Exploration Round Allowance Implementation Plan

**Implements:** [`proposal.md`](proposal.md) · background in [`context.md`](context.md)
**Started:** 2026-10-18
**Status:** In progress. Task 1 done 2026-10-18, Tasks 2–5 done 2026-10-23; task 6 remains.

## Progress

| Task | State | Notes |
|---|---|---|
| 1 · The `agent_max_exploration_rounds` config key | Done 2026-10-18 | **Landed:** the `Config` field `agent_max_exploration_rounds: u32` (default 24) and the `ConfigOverlay` field `Option<u32>`, the exhaustive destructure, a `u32` restrict-only apply block via a new `restrict_only_u32` helper (C3), the dedicated `parse_exploration_round_count` parser (accepts `0..=32`, clamps above 32 to 32 — C2), `config_show`/`to_policy_text` for both `Config` and `ConfigOverlay`, `=> Capability` in `classify_overlay_fields!`, the carried profile key (`split_profile_keys` destructure + `carried` overlay + `equals_base_limit`), and the `effective_policy` label "Read-only rounds per turn". **Tests:** 4 new in `tests/foundation.rs` (`exploration_round_allowance_accepts_zero_where_the_action_budget_refuses_it`, `…_clamps_values_above_its_ceiling_to_32`, `…_defaults_to_24`, `…_round_trips_through_config_show_and_the_overlay_serializer`), 1 new in `tests/repository_config_trust.rs` (`repository_config_may_lower_the_exploration_allowance_but_not_raise_it`, including the audit assertion), 1 new in `tests/permission_profiles.rs` (`agent_max_exploration_rounds_is_a_capability_key_with_a_human_name`), plus a `weakening_cases` entry so `every_capability_key_resists_weakening_from_repository_scope` and the profile-carry/import tests hold. `cargo nextest run -p workspace-engine --test permission_profiles --test repository_config_trust --test foundation`: 275 passed, 1 skipped. Scoped `cargo clippy -p workspace-engine --all-targets --locked -- -D warnings` clean; `scripts/quality-gate.sh fmt node lint typos deny` clean. **Falsified:** classified `Preference` → `agent_max_exploration_rounds_is_a_capability_key_with_a_human_name` fails; applied as a Free preference → `repository_config_may_lower_the_exploration_allowance_but_not_raise_it` fails (32 applied instead of refused); parse refused 0 → `…_accepts_zero_…` fails; clamp removed → `…_clamps_values_above_its_ceiling_to_32` fails. All reverted. **Deviations:** none of design; the ceiling constant is not yet defined — the parser clamps to the literal 32 with a comment naming `ABSOLUTE_EXPLORATION_ROUND_CAP`, which Task 2 defines beside `ABSOLUTE_TOOL_ROUND_CAP` in `chat.rs` (the `parse_round_count` precedent also hardcodes 16 rather than referencing the constant). |
| 2 · Round classification and two counters | Done 2026-10-23 | **Landed:** in `chat.rs`, `run_agentic_turn`'s `round` is renamed `action_rounds` and a `mut exploration_rounds: u32` parameter sits beside it (C8: no `model_calls`); the loop classifies each round **before dispatch** as exploration when every decoded call is `ActionEffect::ReadOnly`, `undecodable_calls.is_empty()`, and `exploration_rounds < max_exploration_rounds` (repeat detection stubbed to "never a repeat", Task 3), charging the end-of-loop increment to the chosen counter while the undecodable-only path and #47's continuation increment `action_rounds`; `force_final` is `action_rounds >= max_rounds`, the pause condition and continuation guard compare `action_rounds` with `ABSOLUTE_TOOL_ROUND_CAP`, and `pause_for_round_budget`'s `rounds_used`/`at_hard_cap` and both audit round markers (`toolRound`, `turn_continued`'s `round`) are action counts; `ABSOLUTE_EXPLORATION_ROUND_CAP = 32` beside the other caps plus `default_exploration_round_limit` (`.min(32)`); `PendingChatTurn.exploration_rounds` (`#[serde(default)]`) is set at all six pending-turn sites and passed back as a new `run_agentic_turn` parameter at all five resume sites, so *Continue* and every approval carry it; the `action_effect` doc comment names the budget decision as a consumer (C1). **Tests:** 9 new in `tests/exploration_rounds.rs` (ten reads complete past the action budget; eight mixed rounds pause at `rounds_used == 8`; one command + three reads = one action round; zero allowance reproduces the action budget; a spent allowance charges reads as action and does not pause; the allowance survives *Continue*; it survives a command approval; a pending turn without `exploration_rounds` loads; `TurnPhase.round` stays at 1 for an all-read turn). `cargo nextest run -p workspace-engine --test exploration_rounds --test round_budget --test token_ceiling --test crash_recovery --test foundation`: 233 passed, 2 skipped. Scoped `cargo clippy -p workspace-engine --all-targets --locked -- -D warnings` clean; `scripts/quality-gate.sh fmt node lint typos` clean. **Deviation:** `foundation.rs::tool_round_exhaustion_pauses_when_model_still_requests_tools` reaches its first round with `read_git_status`, which spec 59 correctly classifies as exploration, so one round no longer reaches the action cap; its subject is spec 61's action-budget pause, so it now pins `agent_max_exploration_rounds = 0` (requirement 6 / the "same rounds, stop reason and audit sequence" acceptance criterion) and its assertions are unchanged. **Falsified:** charging every round to `action_rounds` → `ten_read_only_rounds_complete_past_the_action_budget` fails; `force_final = action_rounds + exploration_rounds >= max_rounds` → same test fails; `default_exploration_round_limit` permitting 1 at config 0 → `zero_exploration_allowance_reproduces_the_action_budget` fails (9 calls vs 8); resuming with `0` → both `the_allowance_survives_*` tests fail. All reverted. |
| 3 · Repeat detection | Done 2026-10-23 | **Landed:** `chat.rs` keeps a turn-local `HashSet<Vec<String>>` of already-dispatched read calls, and `read_call_key` builds §5.3's canonical row per read tool: the checkout from spec 60's `resolve_worktree` canonical path (a session marker when `worktree` is omitted, a refused argument kept as given); paths through `PathPolicy::resolve_existing` relative to that checkout (a refused path kept as given); `read_git_diff` paths resolved and sorted; optional arguments tagged so omitted differs from present; every other argument as given. `list_worktrees` keys on `base` with no checkout, `read_git_status` on its checkout, and `search_codebase` on query/semantic/limit. Keys are computed once per round before dispatch; a round is a repeat when any key is already in the set or appears twice in the same round, and `round_is_exploration` now includes `!repeats_a_seen_call`. A dispatched read enters the set after its marker closes (a permission-denied call does not; a read that failed on a missing or refused path does); a round that ran a `WritesRepository` call clears the set before charging; every resume starts empty. A read tool without a §5.3 row returns `None` and is never a repeat. **Tests:** 10 new in `tests/exploration_rounds.rs` (`identical_reads_spend_the_action_budget_after_one_exploration_round`, `a_continued_line_range_is_not_a_repeat`, `two_identical_calls_in_one_round_make_it_an_action_round`, `the_same_path_in_two_checkouts_is_not_a_repeat`, `two_spellings_of_the_same_worktree_are_a_repeat`, `read_git_diff_with_the_same_paths_in_a_different_order_is_a_repeat`, `read_git_diff_with_a_different_option_is_not_a_repeat`, `read_git_status_after_a_command_is_not_a_repeat`, `read_git_status_repeated_with_only_reads_between_is_a_repeat`, `different_read_arguments_are_not_repeats`). Three Task 2 tests now walk distinct reads so their subject (the allowance and its survival) is not masked by repeat detection: `ten_read_only_rounds_complete_past_the_action_budget`, `a_spent_allowance_charges_reads_as_action_rounds_without_pausing`, `the_allowance_survives_a_round_budget_continue`. `cargo nextest run -p workspace-engine --test exploration_rounds --test round_budget --test token_ceiling --test crash_recovery`: 82 passed, 2 skipped; `--test foundation`: 161 passed. Scoped `cargo clippy -p workspace-engine --all-targets --locked -- -D warnings` clean. `scripts/quality-gate.sh fmt node lint typos` clean; `deny` fails on the pre-existing advisory RUSTSEC-2026-0382 in `rustls-webpki`, unrelated. **Falsified:** removed `!repeats_a_seen_call` (repeated reads stay exploration) → six named tests fail, including `identical_reads_spend_the_action_budget_after_one_exploration_round`, `two_identical_calls_in_one_round_make_it_an_action_round`, `two_spellings_of_the_same_worktree_are_a_repeat`, `read_git_diff_with_the_same_paths_in_a_different_order_is_a_repeat`, `read_git_status_after_a_command_is_not_a_repeat` and `read_git_status_repeated_with_only_reads_between_is_a_repeat`; dropped the line range from the `read_file` key → `a_continued_line_range_is_not_a_repeat` fails; used the raw `worktree` spelling instead of the canonical path → `two_spellings_of_the_same_worktree_are_a_repeat` fails; ignored the checkout in the `read_file` key → `the_same_path_in_two_checkouts_is_not_a_repeat` fails; unsorted `read_git_diff` paths → `read_git_diff_with_the_same_paths_in_a_different_order_is_a_repeat` fails; ignored `base`, `stat` and `staged` → `read_git_diff_with_a_different_option_is_not_a_repeat` fails; removed the clear-on-write → `read_git_status_after_a_command_is_not_a_repeat` fails; ignored `read_file`'s path in its key → `different_read_arguments_are_not_repeats` fails. All reverted. **Deviations:** the first test reaches `ToolBudgetExhausted` through the pause and *Answer now* rather than in one call, because spec 61 pauses at the action budget when no token ceiling is set; the call count is still `1 + agent_max_tool_rounds` plus the forced final call (10 with the defaults). The set is cleared on a `WritesRepository` round only; an edit or applied patch is terminal, and the resume that applies a patch starts with an empty set, so no separate applied-edit branch is reachable. |
| 4 · What the user sees | Done 2026-10-23 | **Landed:** `TurnPhase` gains `exploration_round` (the raw count, not 1-based) and `max_exploration_rounds`; `TurnPhase::new` and `TurnSink::phase` take both, threaded through all six emitters (the context phase, the per-round model phase, the tool phase, the provider-retry model phase, and both command-output phases — the approved-command resume uses `pending.round`/`pending.exploration_rounds`, the others the loop counters). `phase_json` sends `explorationRound`/`maxExplorationRounds` beside `round`/`maxRounds`, and the workers row mirrors both additively (`null` for a phase file written before them). `app.js` appends ` · N reads` (singular ` · 1 read`) when the count is non-zero, keeping the unchanged `round N/M` part. **Wording settled (proposal §5.5):** pause = `Used {N} of {M} action rounds and {R} read-only round(s).`; stop = `This turn reached its budget of {M} action rounds and had used {R} read-only round(s) while the model was still trying to call another tool. I stopped instead of treating the raw tool request as a completed answer. Narrow the next request to finish within the budget.` A small `read_only_rounds` helper pluralises the count. Per C9 no button changed; the pause row still renders *Continue — N more* / *Answer now* and its note still fits the longer message. **Tests:** 3 new in `tests/exploration_rounds.rs` (`phases_carry_the_exploration_count_and_limit`, `the_pause_message_names_the_action_and_read_counts`, `the_stop_message_names_both_counts_without_mentioning_debugging`); `progress_lines_mirror_the_turn` now asserts both new SSE fields, and `workers_list_groups_by_source_repository` asserts the row mirrors them. `cargo nextest run -p workspace-engine --test exploration_rounds --test round_budget`: 34 passed. `cargo nextest run -p desktop-shell`: 160 passed, 17 skipped. Scoped clippy for both crates clean; `node --check` and `npm run lint:web` clean; `scripts/quality-gate.sh fmt node lint typos deny` passes fmt/node/lint/typos and fails `deny` only on the pre-existing RUSTSEC-2026-0382 (`rustls-webpki` 0.103.15, via `tauri-plugin-updater`). **Falsified:** forced `TurnPhase::new`'s `exploration_round` to 0 → `phases_carry_…` fails (`[0,0,0,0,0,0]`); dropped `explorationRound` from `phase_json` → `progress_lines_mirror_the_turn` fails; dropped the two row fields → `workers_list_groups_by_source_repository` fails (`Null` vs `3`); re-added "Continue debugging" to the stop text → `the_stop_message_…` fails; dropped the read-only clause from the pause text → `the_pause_message_…` fails. **Deviation:** the `app.js` label has no automated test (the repo has no JS test runner), so its read-count append was checked by inspection and lint only, not falsified by a failing test. |
| 5 · Audit, eval harness and the two scenarios | Done 2026-10-23 | **Landed:** `model_request_prepared` (beside `toolRound`/`toolRoundLimit`), `turn_continued` (beside `round`/`roundLimit`) and — C10's decision — `round_budget_paused` (beside `roundsUsed`/`roundsTotal`) all carry `explorationRounds`/`explorationLimit`. `trace::tool_rounds` was refactored onto a shared `rounds()` walker over `action_started` markers between `model_call` markers; the new `trace::exploration_rounds` counts rounds whose every dispatched action is one of the eight read marker names (`READ_ONLY_ACTION_MARKERS`, with a comment tying it to `tool_action_marker`). `RunRecord.exploration_rounds` (serialized `explorationRounds`) is set in `runner.rs` from the log; a new `exploration_rounds` scenario assertion compares it exactly. `Scenario` gained `config_overlay` (a `key=value` string parsed to `ConfigOverlay` at load, applied at user scope by both runners, with `data_dir` restored afterwards so a scenario cannot redirect a run's state). Two new scenarios reuse `rust-workspace`: `long_discovery` (twelve distinct file reads, one listing, one search, then an answer) and `read_loop` (the same file and range every round, `round_budget_decision = "answer_now"`). C10 settled: the pause record names both budgets, and both the zero-allowance case and `read_loop` answer *Answer now* to reach `ToolBudgetExhausted`, because the deterministic harness sets no token ceiling and without one the engine pauses. **Tests:** 3 new in `tests/exploration_rounds.rs` (`model_request_prepared_records_the_exploration_count_and_limit`, `turn_continued_records_the_exploration_count_and_limit`, `round_budget_paused_records_the_exploration_count_and_limit`); in `eval-harness/tests/harness.rs` a synthetic-session-log test (`exploration_rounds_come_from_the_session_log`), `the_long_discovery_scenario_completes_past_the_action_budget`, `without_an_exploration_allowance_long_discovery_reaches_the_action_budget`, `the_read_loop_scenario_stops_after_one_exploration_round_and_the_action_budget`, a guard that a scenario's `config_overlay` cannot move the run's `data_dir`, plus `toolRounds`/`explorationRounds` in the record-serialization test and the scenario counts 19 → 21. `cargo nextest run -p workspace-engine --test exploration_rounds`: 25 passed; `cargo nextest run -p eval-harness`: 79 passed, 1 skipped. **Falsified:** removing the three audit field pairs fails all three named audit tests; deriving `exploration_rounds` from the script (`read_*` turns) fails `long_discovery` (12 vs 14) and `read_loop` (10 vs 9); removing `!repeats_a_seen_call` from classification fails `read_loop` with 33 model calls against `model_calls_at_most = 10` (§5.7's "not thirty-three"). All reverted. **Deviation:** the session-log figure counts *all-read dispatched rounds*, not rounds the engine charged to the allowance — repeat detection and a spent allowance are invisible in tool names. `read_loop` therefore asserts `exploration_rounds = 9` and the test asserts the charged count (`explorationRounds = 1`) from the audit trail; proposal §5.6 now states that distinction. `scenario::Scenario` gained a small generic config overlay rather than a third scenario file for the zero-allowance case. |
| 6 · Docs, live re-run and close-out | Not started | |

## Global Constraints

Every task's requirements implicitly include this section.

- **Read [`proposal.md`](proposal.md) §2, §3 and §5 and the corrections below
  before starting.** Where a correction contradicts the proposal's prose, the
  correction wins, and the task that acts on it updates the proposal in the
  same change (most were already folded in while planning).
- **Classification is by `action_effect`, and nothing else** (§5.1). Do not
  hard-code a tool-name list. The allowance counts whatever `action_effect`
  classifies `ReadOnly`, which today is spec 60's now-merged read tools —
  `ReadFile`, `ListDirectory`, `SearchContent`, `SearchCodebase`,
  `ReadGitStatus` and `ReadGitDiff`, all carrying spec 60's `worktree` argument
  — and will include its `list_worktrees` and `read_git_log` automatically when
  spec 60's Task 4 lands. Adding a `ToolAction` variant already forces a
  decision in that match; that decision is now also the budget decision, and
  the doc comment on `action_effect` must say so.
- **Spec 61 Tasks 1–5 have landed on `main`** (the `round_budget` pause,
  `resume_round_budget` with *Continue* / *Answer now*,
  `ABSOLUTE_TURN_ROUND_CAP`, crash recovery and the pause row in the UI), and
  this plan was re-checked against them on 2026-10-23 (C8–C10). Build on that
  code; do not undo or route around it. Spec 61's remaining Tasks 6–7 (an eval
  scenario and docs) do not edit `chat.rs`, but Task 6 edits the eval
  harness's `runner.rs`, which this spec's Task 5 also edits, so those two
  must not run at the same time. `docs/specs/README.md` "Parallel work" still
  rules out #62 beside #59.
- **The existing round-budget tests pass unchanged.** `round_budget.rs`,
  `token_ceiling.rs` and the `foundation.rs` budget tests walk the budget with
  command rounds (`ls src`, `pwd`), which are action rounds, so this spec must
  not change their outcome. If one fails, treat it as a classification bug,
  not a test to update.
- **`chat.rs` is shared.** Spec 60 edits the tool definitions, `parse_tool_call`
  and read-only dispatch; spec 63 edits `record_command_findings` and the
  `run_command` description. Keep this spec's `chat.rs` edits to the round loop,
  `TurnPhase`, the `action_effect` doc comment, `tool_budget_exhausted_response`,
  the audit round fields, and the progress labels.
- **No command-policy change** (non-goal 2). Nothing in this spec edits
  `command_policy.rs`, `command_allowlist`, `is_low_risk_read_only`, or the
  shell-control gate.
- **`agent_max_tool_rounds` and its scope class do not change** (non-goal 3).
  It stays a Free Preference at repository scope, default 8, cap
  `ABSOLUTE_TOOL_ROUND_CAP`.
- **The token ceiling is untouched.** Continuation (spec 47 requirement 6) still
  extends only the action budget; the exploration allowance is per turn and
  never extended.
- **Tests:** fixture repositories are fresh `git init` directories under the
  temp dir with `user.name`, `user.email` and `commit.gpgsign=false` set
  locally. Engines built in tests set `enable_index_watcher: false` and their
  own data directory. Model-dependent paths use `MockModelAdapter` or
  `DAMAIAN_MOCK_MODEL_RESPONSE`.
- **Falsify every load-bearing test.** Break what it guards, confirm it fails,
  revert, and record that in the Progress row.
- **A private Cargo target directory per worktree.** Prefix every `cargo`
  command with `CARGO_TARGET_DIR="$PWD/target"` from the worktree root.
- **Scoped checks per task; the full gate runs on CI only** (AGENTS.md, since
  2026-10-15). Each task runs `scripts/quality-gate.sh fmt node lint typos
  deny`, `cargo clippy -p <crate> --all-targets --locked -- -D warnings` for
  every crate it touches, and the test files it names. The workspace clippy and
  test suite are left to CI.
- **Never `git commit` or `git push` unasked.** Each task ends by showing the
  change and the check results. One-line commit subject, no body.

## Corrections to the proposal found while planning

Each one names the task that owns it.

| # | Proposal said | Code says | Resolution |
|---|---|---|---|
| C1 | §2 lists the read tools as `ReadFile`, `ListDirectory`, `SearchContent`, `SearchCodebase`, `ReadGitStatus`, `ReadGitDiff` | Spec 60 has merged its `worktree` argument onto those five (and `ReadGitDiff`'s `paths`/`stat`/`base`), and its Task 4 will add `ListWorktrees`/`ReadGitLog`, all `ReadOnly` | Classification is by `action_effect`, so the allowance counts spec 60's tools automatically; no redesign. §2 updated. **Tasks 2, 3** |
| C2 | §5.4 says the key is clamped "as `agent_max_tool_rounds` is" | `parse_round_count` (`config.rs:4368`) *refuses* 0 and anything over 16, but this key must *accept* 0 (requirement 6) and clamp over 32 to the ceiling (requirement 5) | A dedicated parser accepting `0..=32`, clamping larger values to `ABSOLUTE_EXPLORATION_ROUND_CAP`, plus a runtime `default_exploration_round_limit`. §5.4 updated. **Task 1** |
| C3 | §5.4 says "in the same loop as `agent_max_turn_messages` and `max_read_lines` (`config.rs:1570`)" | That loop's `restrict_only_limit` is `usize`-typed (`config.rs:4423`); the new key is `u32` | A `u32` restrict-only apply block (a small `restrict_only_u32` helper or a dedicated block, as `agent_max_task_tokens` has via `restrict_only_ceiling`). §5.4 updated. **Task 1** |
| C4 | §5.6 says the audit records gain fields "beside `roundLimit`" on "forced-final, continued and exhausted paths" | The round fields are `toolRound`/`toolRoundLimit` in `model_request_prepared` (`chat.rs:2108`) and `round`/`roundLimit` in `turn_continued` (`chat.rs:2358`); there is no separate forced-final or exhausted audit event | Name the two records precisely; exhaustion is visible through `model_request_prepared` at `toolRound == toolRoundLimit` plus the stop reason. §5.6 updated. **Task 5** |
| C5 | §5.2 lists "the progress phase's position in the turn" as a `model_calls` role | §5.5 says `TurnPhase.round`/`max_rounds` "now mean action rounds" | §5.5 wins: `TurnPhase.round` = action rounds; `model_calls` feeds spec 57's pause/steer boundary and the audit round markers only. §5.2 updated; `model_calls` itself dropped by C8. **Tasks 2, 4** |
| C6 | §2's `chat.rs` line references (523, 535, 2053, 2324, 2334, 4380, 5238, 162) | Drifted after spec 60/63/64 merged into `chat.rs` (now 530, 542, 2072, 2340, 2350, 4465, 5443, 168) | Re-checked and corrected on 2026-10-18; will drift again when spec 61 lands. Not a design change. **all** |
| C7 | §5.3's canonical form named only a repository-relative path, a line range and search arguments | Spec 60 gave five read tools a `worktree` argument and added `read_git_diff`'s `paths`/`stat`/`base`, `list_worktrees` and `read_git_log`; keyed by path alone, `README.md` in a worktree repeats `README.md` in the main checkout, and a `read_git_status` after a command repeats the one before it | §5.3 now keys every call by its checkout, gives a canonical row per read tool, and clears the seen-call set after a round that writes. Found reviewing #65 on 2026-10-19, whose elision notice prints this form. **Task 3** |
| C8 | §5.2 assumed the round loop of 2026-10-18: *Continue debugging* (`options.continue_debugging`) raising the limit to `ABSOLUTE_TOOL_ROUND_CAP` in a new turn, a forced final call at the budget, and a third `model_calls` counter for spec 57's steer boundary and the audit markers | Spec 61 Tasks 1–5 removed `continue_debugging`; the budget now pauses (`pause_for_round_budget`) and *Continue* resumes the **same** turn with `round_budget_extension` raised, up to `ABSOLUTE_TURN_ROUND_CAP` (64). `round` is a parameter of `run_agentic_turn`, persisted in `PendingChatTurn.round` across every pause. Nothing reads a per-call count | §2 and §5.2 rewritten 2026-10-23: `round` becomes `action_rounds` everywhere it is used; no `model_calls`; the pause and both caps compare action rounds; `PendingChatTurn` gains `exploration_rounds` (`#[serde(default)]`), carried through every pause and resume so neither *Continue* nor an approval refreshes the allowance. **Task 2** |
| C9 | §5.5 asked for the stop button to become *Continue* and `tool_budget_exhausted_response` to name both counts | Spec 61 already shows *Continue — N more* / *Answer now* beside "Used N of M tool rounds." (`round_budget_pause_response`); `tool_budget_exhausted_response` is now reached only through *Answer now* and the token-ceiling path, and still says "Continue debugging" | §5.5 updated: no button change; both messages name both counts and drop "debugging". **Task 4** |
| C10 | §5.6 and §5.7 assumed a budget stop ends the turn `ToolBudgetExhausted` and that the only round audit records are `model_request_prepared` and `turn_continued` | With no token ceiling the turn now pauses at the budget (the deterministic harness has no ceiling), and spec 61 added `round_budget_paused` and `turn_budget_extended` records | Not settled here. Task 5 decides, when it starts, whether `round_budget_paused` also carries `explorationRounds`, and whether `long_discovery` with the allowance at 0 and `read_loop` assert the pause or answer *Answer now* — spec 61 Task 6's `RoundBudgetDecision` scenario option, if it has landed by then — and updates §5.6, §5.7 and §6 to match. **Task 5** |

## File Structure

| File | Change |
|---|---|
| `crates/workspace-engine/src/config.rs` | New key: field, overlay field, destructure, `u32` restrict-only apply, parser, `config_show`, overlay serializer, `Default` (24), key partition `Capability` (Task 1) |
| `crates/workspace-engine/src/profile.rs` | Carried capability key (Task 1) |
| `crates/workspace-engine/src/effective_policy.rs` | Label "Read-only rounds per turn" (Task 1) |
| `crates/workspace-engine/src/chat.rs` | `ABSOLUTE_EXPLORATION_ROUND_CAP` + `default_exploration_round_limit`, round split and classification (Task 2); repeat detection (Task 3); `TurnPhase` fields and both budget messages (Task 4); audit fields (Task 5) |
| `crates/workspace-engine/tests/exploration_rounds.rs` | **New (Task 2).** Grows with every task |
| `crates/workspace-engine/tests/permission_profiles.rs`, `repository_config_trust.rs`, `foundation.rs` | Config key tests (Task 1) |
| `crates/desktop-shell/src/lib.rs` | SSE `phase` serialization (Task 4) |
| `crates/desktop-shell/src/workers.rs` | Progress-file phase fields (Task 4) |
| `crates/desktop-shell/static/app.js` | Progress label (Task 4); the stop button is unchanged (C9) |
| `crates/eval-harness/src/trace.rs`, `record.rs`, `runner.rs` | `exploration_rounds` (Task 5) |
| `crates/eval-harness/scenarios/long_discovery.toml`, `read_loop.toml`, fixtures | **New (Task 5)** |
| `docs/USER_GUIDE.md`, `docs/TROUBLESHOOTING.md` | Task 6 |

## Order and parallelism

```
1 ──> 2 ──> 3 ──> 4 ──> 5 ──> 6
```

Sequential. Tasks 2–5 all edit `chat.rs`. Task 1 touches only `config.rs`,
`profile.rs` and `effective_policy.rs`, so it could run beside another spec's
`config.rs` work as a rebase, but no `chat.rs` spec. Tasks 2–5 must not run
beside #61 Tasks 1–3 (Global Constraints) or another spec's `chat.rs` task
(`docs/specs/README.md`, "Parallel work").

---

## Task 1: The `agent_max_exploration_rounds` config key

**Requirements:** 5, 6, 7. **Proposal:** §5.4. **Corrections:** C2, C3.
**Files:** `config.rs`, `profile.rs`, `effective_policy.rs`,
`tests/permission_profiles.rs`, `tests/repository_config_trust.rs`,
`tests/foundation.rs`.

- [x] **Step 1: Write the failing tests.**
  - Parse accepts `0` (`ConfigOverlay::parse("agent_max_exploration_rounds=0\n")`
    is `Ok`), unlike `agent_max_tool_rounds=0`, which errs (`foundation.rs:5196`).
  - Parse accepts up to 32, and clamps a value above 32 to 32 (acceptance
    "A value above 32 is clamped to 32").
  - `Config::default().agent_max_exploration_rounds == 24`.
  - `config_show` emits `agent_max_exploration_rounds=24`, and the overlay
    serializer round-trips it.
  - The key partition lists `agent_max_exploration_rounds` as `Capability`
    (`overlay_field_kinds`; the `permission_profiles.rs:109` pattern).
  - Restrict-only at repository scope: a repo config with a higher value than
    the user's is rejected as a `RestrictOnly` weakening and audited; a lower
    one applies (`repository_config_trust.rs:497` pattern).
  - The effective-policy label is "Read-only rounds per turn", and `profile.rs`
    carries it as a capability key like the other `agent_*` Capability keys.
- [x] **Step 2: Run them and confirm they fail** (the field does not exist, so
      they fail to compile).
- [x] **Step 3: Add the field** to `Config` (`agent_max_exploration_rounds: u32`)
      and `ConfigOverlay` (`Option<u32>`), the destructure, `Default` (24), the
      key parser, `config_show`, and the overlay serializer.
- [x] **Step 4: The apply block and partition.** A `u32` restrict-only apply
      (C3) so a lower value wins and a higher one is rejected; `=> Capability`
      in `classify_overlay_fields!`; the carried capability key in `profile.rs`
      and the label in `effective_policy.rs`.
- [x] **Step 5: The parser (C2).** Accept `0..=32`, clamping larger values to
      32. Reuse the existing "refuse a nonsense value" reasoning but not
      `parse_round_count`, which rejects 0 and >16.
- [x] **Step 6: Falsify** — classify it as `Preference`, apply it as a Free
      preference instead of restrict-only, refuse 0 at parse, and leave the
      clamp off. Confirm a named test fails for each, and revert.
- [x] **Step 7: Scoped checks.** `cargo nextest run -p workspace-engine --test
      permission_profiles --test repository_config_trust --test foundation`,
      scoped clippy for `workspace-engine`, and the fast checks.

## Task 2: Round classification and two counters

**Requirements:** 1, 2, 3, 5, 6. **Proposal:** §5.1, §5.2. **Corrections:** C1,
C5, C8. **Files:** `chat.rs`, `tests/exploration_rounds.rs` (new).

Re-planned on 2026-10-23 against spec 61's round loop (C8). Read §5.2 as
rewritten, `run_agentic_turn`, `pause_for_round_budget`,
`resume_round_budget` and `PendingChatTurn` before starting. `TurnPhase`'s new
fields and every message wording stay in Task 4.

- [x] **Step 1: Write the failing tests** in `tests/exploration_rounds.rs`,
      each through `MockModelAdapter` with native tool calls
      (`new_sequence_with_tool_calls`), no token ceiling unless stated, over
      a fixture repository (the `round_budget.rs` helpers are the pattern).
      For action rounds use a command the policy runs without approval
      (`ls src`, as `round_budget.rs`'s `read_only_round` does), so a test
      walks the budget without stopping for a command approval:
  - a turn of ten read-only rounds (`read_file`/`list_directory`/`search_content`)
    then an answer completes (`TaskStatus::Complete`, no
    `round_budget_proposal`) past the eight-round action budget;
  - a turn of eight command rounds with reads between them pauses at the
    action budget (`round_budget_proposal` with `rounds_used == 8`), counting
    only its action rounds;
  - a round containing one command and three reads charges one action round and
    no exploration round (visible as the pause's `rounds_used`, or the phase
    events' `round`);
  - `agent_max_exploration_rounds = 0` reproduces today's behaviour: a
    read-only turn pauses with `rounds_used == agent_max_tool_rounds`;
  - once the allowance is spent (set it to, say, 2), further read-only rounds
    are charged as action rounds and the turn is not paused at that moment,
    only when the action budget is reached;
  - **the allowance survives a resume:** a turn that spends its allowance,
    then pauses at the action budget and is resumed with
    `resume_round_budget(.., RoundBudgetDecision::Continue, ..)`, charges its
    next read-only round as an action round; and a turn resumed after a
    command approval likewise keeps its exploration count;
  - a pending turn file written without `exploration_rounds` still loads (the
    field defaults to 0);
  - `TurnPhase.round`/`max_rounds` in the progress events are action counts:
    an all-read turn's phases never show `round` above 0 (C5).
- [x] **Step 2: Run them and confirm they fail.**
- [x] **Step 3: Split the counter.** In `run_agentic_turn`, `round` becomes
      `action_rounds` in every use (§5.2: the budget checks, `sink.phase`,
      the `toolRound`/`round` audit fields, the pending turn's `round`,
      `RoundBudgetPause.rounds_used`), and a new `exploration_rounds` counter
      sits beside it. No `model_calls`. Add `ABSOLUTE_EXPLORATION_ROUND_CAP = 32`
      beside `ABSOLUTE_TOOL_ROUND_CAP` and `ABSOLUTE_TURN_ROUND_CAP`, with
      their style of doc comment, and `default_exploration_round_limit`
      clamping the config value to `0..=32`. Point Task 1's literal 32 in
      `parse_exploration_round_count` at the constant if it is visible there;
      otherwise leave the comment.
- [x] **Step 4: Persist it.** `PendingChatTurn` gains
      `exploration_rounds: u32` with `#[serde(default)]`; every site that
      builds a `PendingChatTurn` sets it from the loop, and every resume
      (`resume_round_budget` both arms, command/MCP/web-diagnostic/plan/
      fallback resumes — wherever `pending.round` is passed into
      `run_agentic_turn`) passes it back, as a new parameter beside `round`.
- [x] **Step 5: Classify before dispatch** (§5.1). After the calls decode and
      before any dispatches, a round is exploration when every decoded call is
      `ActionEffect::ReadOnly`, there is no undecodable call in the round,
      none is a repeat (§5.3 — stubbed to "never a repeat" in this task), and
      `exploration_rounds < max_exploration_rounds`; action otherwise. The
      round's increment at the end of the loop body goes to the counter
      classification chose; the undecodable-only path and #47's continuation
      increment `action_rounds`.
- [x] **Step 6: Re-point the budget** (§5.2). `force_final` is
      `action_rounds >= max_action_rounds`, where `max_action_rounds` is the
      existing `max_rounds` expression; the pause condition and #47's
      continuation guard compare `action_rounds` with
      `ABSOLUTE_TOOL_ROUND_CAP`; `pause_for_round_budget`'s `rounds_used` and
      `at_hard_cap` use `action_rounds`; `resume_round_budget`'s *Continue*
      grants action rounds only and never resets `exploration_rounds`;
      web-debug mode keeps its action limit.
- [x] **Step 7: Update the `action_effect` doc comment** so the budget decision
      is documented as one of its consumers (C1).
- [x] **Step 8: Falsify** — charge an all-read round to the action budget, let
      exploration rounds count toward the action cap, let
      `agent_max_exploration_rounds = 0` still allow exploration, and reset
      `exploration_rounds` to 0 on resume. Confirm a named test fails for
      each, and revert.
- [x] **Step 9: Scoped checks.** `cargo nextest run -p workspace-engine --test
      exploration_rounds --test round_budget --test token_ceiling --test
      crash_recovery --test foundation`, scoped clippy for `workspace-engine`,
      and the fast checks. `round_budget`, `token_ceiling` and
      `crash_recovery` must pass without edits (Global Constraints).

## Task 3: Repeat detection

**Requirement:** 4. **Proposal:** §5.3. **Corrections:** C1.
**Files:** `chat.rs`, `tests/exploration_rounds.rs`.

- [x] **Step 1: Write the failing tests:**
  - a turn that reads the same file with the same range every round stops with
    `ToolBudgetExhausted` after one exploration round plus the action budget
    (`1 + agent_max_tool_rounds` rounds plus the forced final call);
  - a continued range read (lines 400–468 after 1–400) is **not** a repeat;
  - two identical calls in the same round make it an action round;
  - the same path read in the main checkout and in a registered worktree is
    **not** a repeat, and two `worktree` spellings that resolve to the same
    registered worktree **are**;
  - `read_git_diff` with the same `paths` in a different order is a repeat,
    and one with a different `base`, `stat` or `staged` is not;
  - `read_git_status` repeated after a round that ran a command is **not** a
    repeat, and repeated with only reads between is;
  - a read of a different path (or a different search pattern/glob/match cap)
    is not a repeat.
- [x] **Step 2: Run them and confirm they fail.**
- [x] **Step 3: The seen-call set.** A turn-local set keyed by canonical call
      as §5.3's table defines it per tool — the checkout (#60's resolved
      `worktree`, or the session's checkout), paths resolved through
      `PathPolicy` relative to that checkout, `read_git_diff`'s `paths` sorted,
      every other argument as given. Populated as each call dispatches;
      cleared when a round with a `WritesRepository` call or an applied
      edit/patch completes; empty for a resumed turn.
- [x] **Step 4: Wire it into classification** (§5.3, via Task 2's Step 4): a
      read-only call that matches a seen call makes its round an action round.
- [x] **Step 5: Falsify** — treat a continued range read as a repeat, and let a
      repeated read stay exploration. Confirm a named test fails for each, and
      revert.
- [x] **Step 6: Scoped checks** (`exploration_rounds`, scoped clippy, fast
      checks).

## Task 4: What the user sees

**Requirement:** 8. **Proposal:** §5.5. **Corrections:** C5, C9.
**Files:** `chat.rs`, `desktop-shell/src/lib.rs`,
`desktop-shell/src/workers.rs`, `desktop-shell/static/app.js`,
`tests/exploration_rounds.rs`.

- [x] **Step 1: Write the failing tests.** `TurnPhase` gains
      `exploration_round` and `max_exploration_rounds`; the SSE `phase` payload
      (`lib.rs:3533`) and the worker progress file (`workers.rs:1097`) carry
      them additively; the pause message and the stop message name both counts
      and do not mention debugging (C9).
- [x] **Step 2: Run them and confirm they fail.**
- [x] **Step 3: `TurnPhase` and its emitters.** Add the two fields to
      `TurnPhase`, `TurnPhase::new`, `sink.phase` and every `TurnPhase::new`
      call site; serialize them in `lib.rs:3533` and `workers.rs:1097` (older
      readers ignore the new fields).
- [x] **Step 4: The label.** `app.js:4172` appends `· {n} reads` when the
      exploration count is non-zero, keeping `round N/M` (now action rounds)
      unchanged.
- [x] **Step 5: The messages** (C9). `round_budget_pause_response` and
      `tool_budget_exhausted_response` name both counts and assume nothing
      about the task; the latter drops "Continue debugging". Spec 61's pause
      row already offers *Continue*; no button changes. Check that the pause
      row in `app.js` (`createRoundBudgetOffer`) still reads correctly with
      the longer message.
- [x] **Step 6: Falsify** — drop the reads from the label, leave the stop
      message mentioning "debugging", and drop the read-only count from the
      pause message. Confirm a named test fails for each, and revert.
- [x] **Step 7: Scoped checks.** `node --check crates/desktop-shell/static/app.js`,
      `npm run lint:web`, scoped clippy for `workspace-engine` and
      `desktop-shell`, `cargo nextest run -p workspace-engine --test
      exploration_rounds` and `-p desktop-shell`, and the fast checks.

## Task 5: Audit, eval harness and the two scenarios

**Requirement:** 9, and the acceptance rows in §6. **Proposal:** §5.6, §5.7.
**Corrections:** C4, C10. **Files:** `chat.rs`, `eval-harness/src/trace.rs`,
`record.rs`, `runner.rs`, `scenarios/long_discovery.toml`, `read_loop.toml`,
fixtures.

- [x] **Step 1: Write the failing tests and scenarios.**
  - `model_request_prepared` carries `explorationRounds` and
    `explorationLimit`, and `turn_continued` carries them beside `roundLimit`
    (C4).
  - `trace::exploration_rounds(data_dir)` counts rounds whose every dispatched
    tool is a read, and `record.exploration_rounds` is populated for every run.
  - `long_discovery` (fourteen read-only rounds, no repeats, then an answer):
    completes with the defaults, stops `ToolBudgetExhausted` with
    `agent_max_exploration_rounds = 0`.
  - `read_loop` (same file/range every round): stops `ToolBudgetExhausted`
    after one exploration round plus the action budget.
- [x] **Step 2: Run them and confirm they fail.**
- [x] **Step 3: Audit fields.** Add `explorationRounds`/`explorationLimit` to
      `model_request_prepared` and `turn_continued` (C4).
- [x] **Step 4: Harness.** `trace::exploration_rounds` derived the same way as
      `tool_rounds`, classifying by read-tool action-marker names; a sibling
      `exploration_rounds` field on `record` and set in `runner.rs`.
- [x] **Step 5: The two scenarios** and their fixtures per §5.7.
- [x] **Step 6: Falsify** — drop the audit fields, derive exploration rounds
      from the scenario script rather than the log, and let `read_loop` run on.
      Confirm a named test fails for each, and revert.
- [x] **Step 7: Scoped checks.** `cargo nextest run -p workspace-engine --test
      exploration_rounds`, `cargo nextest run -p eval-harness`, scoped clippy
      for `workspace-engine` and `eval-harness`, and the fast checks.

## Task 6: Docs, live re-run and close-out

- [ ] **Step 1: Live re-run** of the motivating prompt against
      `deepseek-v4-flash` on this repository (§5.7). If it completes, record
      that; if it still stops, record how many exploration rounds it used before
      stopping — the evidence for revisiting the default (§6, last row).
- [ ] **Step 2: Docs** (§5.8): `USER_GUIDE.md` (the budget section explaining
      *Tool budget exhausted*, and the config reference for
      `agent_max_exploration_rounds`) and `TROUBLESHOOTING.md` (the
      `tool_budget_exhausted` entry, which today says "default 8, plus one
      closing model call"). `SECURITY.md` needs no change unless it has gained
      budget keys by then.
- [ ] **Step 3: Close the spec** per AGENTS.md "When a spec becomes Done":
      status in the proposal, README row and this file; the `Depends on:` search;
      What to build next; remove the item from `CHANGELOG.md` Unreleased.
- [ ] **Step 4: Checks.** Fast checks and scoped clippy/tests locally; the full
      gate on CI. Record the CI result here when it is green.

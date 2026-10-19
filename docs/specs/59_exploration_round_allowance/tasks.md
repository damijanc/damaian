# Exploration Round Allowance Implementation Plan

**Implements:** [`proposal.md`](proposal.md) · background in [`context.md`](context.md)
**Status:** In progress. Task 1 done 2026-10-18; tasks 2–6 remain.

## Progress

| Task | State | Notes |
|---|---|---|
| 1 · The `agent_max_exploration_rounds` config key | Done 2026-10-18 | **Landed:** the `Config` field `agent_max_exploration_rounds: u32` (default 24) and the `ConfigOverlay` field `Option<u32>`, the exhaustive destructure, a `u32` restrict-only apply block via a new `restrict_only_u32` helper (C3), the dedicated `parse_exploration_round_count` parser (accepts `0..=32`, clamps above 32 to 32 — C2), `config_show`/`to_policy_text` for both `Config` and `ConfigOverlay`, `=> Capability` in `classify_overlay_fields!`, the carried profile key (`split_profile_keys` destructure + `carried` overlay + `equals_base_limit`), and the `effective_policy` label "Read-only rounds per turn". **Tests:** 4 new in `tests/foundation.rs` (`exploration_round_allowance_accepts_zero_where_the_action_budget_refuses_it`, `…_clamps_values_above_its_ceiling_to_32`, `…_defaults_to_24`, `…_round_trips_through_config_show_and_the_overlay_serializer`), 1 new in `tests/repository_config_trust.rs` (`repository_config_may_lower_the_exploration_allowance_but_not_raise_it`, including the audit assertion), 1 new in `tests/permission_profiles.rs` (`agent_max_exploration_rounds_is_a_capability_key_with_a_human_name`), plus a `weakening_cases` entry so `every_capability_key_resists_weakening_from_repository_scope` and the profile-carry/import tests hold. `cargo nextest run -p workspace-engine --test permission_profiles --test repository_config_trust --test foundation`: 275 passed, 1 skipped. Scoped `cargo clippy -p workspace-engine --all-targets --locked -- -D warnings` clean; `scripts/quality-gate.sh fmt node lint typos deny` clean. **Falsified:** classified `Preference` → `agent_max_exploration_rounds_is_a_capability_key_with_a_human_name` fails; applied as a Free preference → `repository_config_may_lower_the_exploration_allowance_but_not_raise_it` fails (32 applied instead of refused); parse refused 0 → `…_accepts_zero_…` fails; clamp removed → `…_clamps_values_above_its_ceiling_to_32` fails. All reverted. **Deviations:** none of design; the ceiling constant is not yet defined — the parser clamps to the literal 32 with a comment naming `ABSOLUTE_EXPLORATION_ROUND_CAP`, which Task 2 defines beside `ABSOLUTE_TOOL_ROUND_CAP` in `chat.rs` (the `parse_round_count` precedent also hardcodes 16 rather than referencing the constant). |
| 2 · Round classification and two counters | Not started | |
| 3 · Repeat detection | Not started | |
| 4 · What the user sees | Not started | |
| 5 · Audit, eval harness and the two scenarios | Not started | |
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
- **Do not run alongside spec 61 Tasks 1–3** (the `round_budget` pause, its
  resume path and `ABSOLUTE_TURN_ROUND_CAP`). Spec 61 is being implemented on
  another branch and rewrites the same round-limit code in `chat.rs` — the
  `force_final`/forced-final path and the `ABSOLUTE_TOOL_ROUND_CAP` constant
  that this spec also edits. If a task finds spec 61's changes present, stop
  and say so rather than reconciling them. `docs/specs/README.md` "Parallel
  work" already rules out #61 and #62 beside #59.
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
| C5 | §5.2 lists "the progress phase's position in the turn" as a `model_calls` role | §5.5 says `TurnPhase.round`/`max_rounds` "now mean action rounds" | §5.5 wins: `TurnPhase.round` = action rounds; `model_calls` feeds spec 57's pause/steer boundary and the audit round markers only. §5.2 updated. **Tasks 2, 4** |
| C6 | §2's `chat.rs` line references (523, 535, 2053, 2324, 2334, 4380, 5238, 162) | Drifted after spec 60/63/64 merged into `chat.rs` (now 530, 542, 2072, 2340, 2350, 4465, 5443, 168) | Re-checked and corrected on 2026-10-18; will drift again when spec 61 lands. Not a design change. **all** |
| C7 | §5.3's canonical form named only a repository-relative path, a line range and search arguments | Spec 60 gave five read tools a `worktree` argument and added `read_git_diff`'s `paths`/`stat`/`base`, `list_worktrees` and `read_git_log`; keyed by path alone, `README.md` in a worktree repeats `README.md` in the main checkout, and a `read_git_status` after a command repeats the one before it | §5.3 now keys every call by its checkout, gives a canonical row per read tool, and clears the seen-call set after a round that writes. Found reviewing #65 on 2026-10-19, whose elision notice prints this form. **Task 3** |

## File Structure

| File | Change |
|---|---|
| `crates/workspace-engine/src/config.rs` | New key: field, overlay field, destructure, `u32` restrict-only apply, parser, `config_show`, overlay serializer, `Default` (24), key partition `Capability` (Task 1) |
| `crates/workspace-engine/src/profile.rs` | Carried capability key (Task 1) |
| `crates/workspace-engine/src/effective_policy.rs` | Label "Read-only rounds per turn" (Task 1) |
| `crates/workspace-engine/src/chat.rs` | `ABSOLUTE_EXPLORATION_ROUND_CAP` + `default_exploration_round_limit`, round split and classification (Task 2); repeat detection (Task 3); `TurnPhase` fields and `tool_budget_exhausted_response` (Task 4); audit fields (Task 5) |
| `crates/workspace-engine/tests/exploration_rounds.rs` | **New (Task 2).** Grows with every task |
| `crates/workspace-engine/tests/permission_profiles.rs`, `repository_config_trust.rs`, `foundation.rs` | Config key tests (Task 1) |
| `crates/desktop-shell/src/lib.rs` | SSE `phase` serialization (Task 4) |
| `crates/desktop-shell/src/workers.rs` | Progress-file phase fields (Task 4) |
| `crates/desktop-shell/static/app.js` | Progress label, stop button (Task 4) |
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

**Requirements:** 1, 2, 3, 6. **Proposal:** §5.1, §5.2. **Corrections:** C1, C5.
**Files:** `chat.rs`, `tests/exploration_rounds.rs` (new).

- [ ] **Step 1: Write the failing tests** in `tests/exploration_rounds.rs`,
      each through `MockModelAdapter`:
  - a turn of ten read-only rounds (`read_file`/`list_directory`/`search_content`
    over a fixture) completes (`TaskStatus::Complete`) past the eight-round
    action budget;
  - a turn mixing reads and a command stops at the action budget
    (`TaskStatus::ToolBudgetExhausted`), counting only its action rounds;
  - a round containing one command and three reads charges one action round and
    no exploration round;
  - `agent_max_exploration_rounds = 0` reproduces today's behaviour (a read-only
    turn stops after `agent_max_tool_rounds` rounds);
  - once the allowance is spent, further read-only rounds are charged as action
    rounds and the turn is not stopped at that moment;
  - `TurnPhase` carries `exploration_round` and `max_exploration_rounds`, and
    `round`/`max_rounds` are the action counts (C5).
- [ ] **Step 2: Run them and confirm they fail.**
- [ ] **Step 3: Split the counter.** In `run_agentic_turn`, replace the single
      `round` with `action_rounds`, `exploration_rounds` and `model_calls` per
      §5.2. `ABSOLUTE_EXPLORATION_ROUND_CAP = 32` beside
      `ABSOLUTE_TOOL_ROUND_CAP`, and `default_exploration_round_limit` clamps
      `0..=32`.
- [ ] **Step 4: Classify before dispatch** (§5.1). After the calls decode and
      before any dispatches, a round is exploration when every decoded call is
      `ActionEffect::ReadOnly`, none is a repeat (§5.3 — stubbed empty in this
      task), and `exploration_rounds < max_exploration_rounds`; action
      otherwise. An undecodable call makes the round action.
- [ ] **Step 5: Re-point the budget.** `force_final` becomes
      `action_rounds >= max_action_rounds` (where `max_action_rounds` is
      today's `max_rounds`); continuation's guard becomes
      `action_rounds < ABSOLUTE_TOOL_ROUND_CAP`; *Continue debugging* still
      raises the action limit to the cap and never touches the exploration
      allowance; web-debug mode keeps its action limit.
- [ ] **Step 6: Update the `action_effect` doc comment** so the budget decision
      is documented as one of its consumers (C1).
- [ ] **Step 7: Falsify** — charge an all-read round to the action budget, let
      exploration rounds count toward the action cap, and let
      `agent_max_exploration_rounds = 0` still allow exploration. Confirm a
      named test fails for each, and revert.
- [ ] **Step 8: Scoped checks.** `cargo nextest run -p workspace-engine --test
      exploration_rounds --test foundation --test token_ceiling`, scoped clippy
      for `workspace-engine`, and the fast checks.

## Task 3: Repeat detection

**Requirement:** 4. **Proposal:** §5.3. **Corrections:** C1.
**Files:** `chat.rs`, `tests/exploration_rounds.rs`.

- [ ] **Step 1: Write the failing tests:**
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
- [ ] **Step 2: Run them and confirm they fail.**
- [ ] **Step 3: The seen-call set.** A turn-local set keyed by canonical call
      as §5.3's table defines it per tool — the checkout (#60's resolved
      `worktree`, or the session's checkout), paths resolved through
      `PathPolicy` relative to that checkout, `read_git_diff`'s `paths` sorted,
      every other argument as given. Populated as each call dispatches;
      cleared when a round with a `WritesRepository` call or an applied
      edit/patch completes; empty for a resumed turn.
- [ ] **Step 4: Wire it into classification** (§5.3, via Task 2's Step 4): a
      read-only call that matches a seen call makes its round an action round.
- [ ] **Step 5: Falsify** — treat a continued range read as a repeat, and let a
      repeated read stay exploration. Confirm a named test fails for each, and
      revert.
- [ ] **Step 6: Scoped checks** (`exploration_rounds`, scoped clippy, fast
      checks).

## Task 4: What the user sees

**Requirement:** 8. **Proposal:** §5.5. **Corrections:** C5.
**Files:** `chat.rs`, `desktop-shell/src/lib.rs`,
`desktop-shell/src/workers.rs`, `desktop-shell/static/app.js`,
`tests/exploration_rounds.rs`.

- [ ] **Step 1: Write the failing tests.** `TurnPhase` gains
      `exploration_round` and `max_exploration_rounds`; the SSE `phase` payload
      (`lib.rs:3533`) and the worker progress file (`workers.rs:1097`) carry
      them additively; the stop message names both counts and does not mention
      debugging; the button text is *Continue*.
- [ ] **Step 2: Run them and confirm they fail.**
- [ ] **Step 3: `TurnPhase` and its emitters.** Add the two fields to
      `TurnPhase`, `TurnPhase::new`, `sink.phase` and every `TurnPhase::new`
      call site; serialize them in `lib.rs:3533` and `workers.rs:1097` (older
      readers ignore the new fields).
- [ ] **Step 4: The label.** `app.js:4172` appends `· {n} reads` when the
      exploration count is non-zero, keeping `round N/M` (now action rounds)
      unchanged.
- [ ] **Step 5: The stop message and button.** `tool_budget_exhausted_response`
      names both counts and assumes nothing about the task; the button at
      `app.js:6978` becomes *Continue* (the `continue_debugging` option name
      may stay).
- [ ] **Step 6: Falsify** — drop the reads from the label, leave the stop
      message mentioning "debugging", and keep the button text. Confirm a named
      test fails for each, and revert.
- [ ] **Step 7: Scoped checks.** `node --check crates/desktop-shell/static/app.js`,
      `npm run lint:web`, scoped clippy for `workspace-engine` and
      `desktop-shell`, `cargo nextest run -p workspace-engine --test
      exploration_rounds` and `-p desktop-shell`, and the fast checks.

## Task 5: Audit, eval harness and the two scenarios

**Requirement:** 9, and the acceptance rows in §6. **Proposal:** §5.6, §5.7.
**Corrections:** C4. **Files:** `chat.rs`, `eval-harness/src/trace.rs`,
`record.rs`, `runner.rs`, `scenarios/long_discovery.toml`, `read_loop.toml`,
fixtures.

- [ ] **Step 1: Write the failing tests and scenarios.**
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
- [ ] **Step 2: Run them and confirm they fail.**
- [ ] **Step 3: Audit fields.** Add `explorationRounds`/`explorationLimit` to
      `model_request_prepared` and `turn_continued` (C4).
- [ ] **Step 4: Harness.** `trace::exploration_rounds` derived the same way as
      `tool_rounds`, classifying by read-tool action-marker names; a sibling
      `exploration_rounds` field on `record` and set in `runner.rs`.
- [ ] **Step 5: The two scenarios** and their fixtures per §5.7.
- [ ] **Step 6: Falsify** — drop the audit fields, derive exploration rounds
      from the scenario script rather than the log, and let `read_loop` run on.
      Confirm a named test fails for each, and revert.
- [ ] **Step 7: Scoped checks.** `cargo nextest run -p workspace-engine --test
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

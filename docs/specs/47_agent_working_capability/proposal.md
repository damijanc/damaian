# Feature Spec: Agent Working Capability

Status: In progress. The first slice is built and done: requirements 1–4 —
ranged reads, `list_directory` and `search_content`, and anchor-based region
edits — are implemented, tested, and closed out (see
[`tasks.md`](tasks.md)). Requirements 5, 6 and 8 are designed only to the extent
of the decisions recorded in §5.6, and follow as their own slices.
Order: 47 of 47
Plan: none. Like [`07`](../07_generated_secret_override.md),
[`08`](../08_stop_and_progress.md), [`13`](../13_docker_command_support.md) and
[`34`](../34_repository_config_trust_boundary.md), this spec is evidence-driven
rather than a roadmap graduation — a fourth exception to the rule in
[`README.md`](../README.md). It came from measuring Damaian against its own
backlog: a session asked what capabilities Damaian needs to implement the
remaining specs, and ran out of tool rounds before it could answer.
Also in this spec: [`context.md`](context.md) (what was measured against the
tree while designing, and the four corrections it forced), [`tasks.md`](tasks.md)
(execution order and progress).
Depends on: nothing in this directory.
Related implementation specs:
[`03_structured_tool_calling.md`](../03_structured_tool_calling.md) (owns the tool
surface this extends), [`04_hunk_level_patch_apply.md`](../04_hunk_level_patch_apply.md)
(hunk selection at review time, which this does not change),
[`08_stop_and_progress.md`](../08_stop_and_progress.md) (owns cancellation and the
progress channel), [`10_persistent_command_approval.md`](../10_persistent_command_approval.md)
and [`34_repository_config_trust_boundary.md`](../34_repository_config_trust_boundary.md)
(the command trust boundary this must not widen),
[`19_token_and_cost_accounting/proposal.md`](../19_token_and_cost_accounting/proposal.md) and
[`21_task_plan_progress_and_budget/proposal.md`](../21_task_plan_progress_and_budget/proposal.md)
(own the budget this defers to),
[`24_repository_map_and_monorepo_boundaries.md`](../24_repository_map_and_monorepo_boundaries.md),
[`25_symbol_and_relationship_index.md`](../25_symbol_and_relationship_index.md) and
[`26_context_assembly.md`](../26_context_assembly.md) (the durable versions of the
navigation and ranged-read floor this establishes).

## 1. Motivation

Every other spec describes what Damaian does for the user. This one asks a
different question: **can Damaian do a spec's worth of work in one sitting?**
Today it cannot, and the reason is not model quality — it is the shape of the
tool surface.

The evidence is in this repository. Task 2 of
[spec 17](../17_durable_task_state_and_crash_recovery/tasks.md) mutation-tested a
cache against fifteen construction sites and measured per-append cost at three
log sizes; Task 5 instrumented six action sites and migrated a stored patch
format from V1 to V2. Tasks 1–5 together moved the test count from 382 to 395
across six files. That is many dozens of read-search-edit-verify cycles per
task. Damaian's default is **eight tool rounds per turn**, hard-capped at
sixteen, after which the model is handed no tools and forced to answer with
whatever it has.

Twenty specs remain. Each is a module, its tests, and its wiring. If Damaian is
to build any of them, the floor beneath the agent has to hold.

This spec is that floor. It adds no autonomy: every capability here is either
read-only or produces the same reviewable patch the user already approves.

## 2. Current State

The tools offered to a native-tool provider are assembled in `chat.rs`, where
`native_tools` is built:
`run_command`, `propose_patch`, `read_file`, `search_codebase`,
`read_git_status`, `read_git_diff`, plus the two browser tools when a
diagnostics runner is attached, plus discovered MCP tools.

| Capability | Current state | Consequence |
|---|---|---|
| Sustained work | `agent_max_tool_rounds` 8, web-debug 12, `ABSOLUTE_TOOL_ROUND_CAP` 16 (`chat.rs`, `config.rs:1185`). At the limit `tools` is set to `None` (`chat.rs`, `force_final`) | Every turn ends in a forced summary. The only continuation is `ChatTurnOptions.continue_debugging` (`chat.rs`), a single boolean built for web debugging |
| Targeted edits | `propose_patch` takes `"Full replacement file content"` per file (`propose_patch_tool_definition`) | `desktop-shell/src/lib.rs` is 4160 lines. Changing ten of them means emitting all 4160. `undecodable_tool_call_note` already carries an apology path for a provider-truncated tool call — the failure is anticipated, not hypothetical |
| Ranged reads | `read_file` takes only `path`; `FileAccess::read_file` reads the whole file (`file_access.rs:69`) and **refuses** above `max_file_bytes`, default 1 MiB (`file_access.rs:63`) | `tests/foundation.rs` is 4420 lines — one read exhausts the 16k default context budget (`config.rs:11`; 64k for `deepseek-v4`). An oversized file is not truncated, it is unreadable |
| Navigation | No listing tool, no content-search tool. `search_codebase` is index-scored keyword overlap or embeddings (`indexer.rs:61`, `:92`) | It finds *files*, not call sites. "Where is `apply_overlay` wired" has no cheap answer |
| Navigation via shell | `is_low_risk_read_only` is exactly `pwd`, `ls`, `git status`, `git diff`, `git log`, `git show` (`command_policy.rs:293`) | `grep`, `find`, `cat`, `sed`, `head` all require approval. `command_allowlist` is **exact-match** (`command_policy.rs:107`), so each distinct invocation needs its own `Allow Always`; and the shell-control gate runs first, so anything containing `\|` or `>` can never be allowlisted at all (`command_policy.rs:245`) |
| Command execution | `Command::output()` — blocking, no timeout, no streaming (`command_runner.rs:89`). Output truncated at `max_command_output_bytes`, default 1 MiB | The `AGENTS.md` quality gate is seven commands; `cargo clippy --workspace --all-targets` alone runs for minutes with no incremental output. Spec 08's stop is checked *between* rounds (`chat.rs`, `sink.cancel.is_cancelled()`), so it cannot interrupt a running build |

Two of these gaps have durable successors already specified — ranged context
belongs to [#26](../26_context_assembly.md), which adds line ranges to
`ContextItem`, and structural navigation to [#24](../24_repository_map_and_monorepo_boundaries.md)
and [#25](../25_symbol_and_relationship_index.md). **This spec is the cheap
deterministic floor beneath them, not a substitute.** It is specified first
because those three cannot be built by an agent that lacks it.

## 3. Requirements

1. `read_file` accepts an optional line range and returns only those lines,
   stating the range returned and the file's total line count. A file above
   `max_file_bytes` becomes readable by range rather than refused outright.
2. Listing and content search exist as first-class tools, resolved through
   `PathPolicy` and redacted through `SecretScanner` on the same path as
   `FileAccess::read_file`.
3. Navigation requires no per-invocation approval, and achieves that **without
   widening the command trust boundary**: no new entry in `command_allowlist`,
   no change to `is_low_risk_read_only`, and no relaxation of the shell-control
   gate. A capability the agent has as a tool is not a capability it gains at
   the shell.
4. A change can be proposed as a bounded region replacement, so the size of an
   edit payload is proportional to the size of the change rather than the size
   of the file. It converts into the same `ProposedChange` that
   `PatchEngine::create_patch` already consumes, leaving diff review, hunk
   selection, redaction, checkpointing and audit untouched.
5. A running command can be cancelled, is subject to a timeout, and reports
   output before it exits.
6. A turn that exhausts its tool rounds can continue with its task state
   intact, under a budget that is explicit, bounded, and recorded.
7. Every new tool records through `AuditLog::record`, on the same terms as the
   tools it sits beside.
8. Where a model requests several read-only tool calls in one round, they are
   executed concurrently rather than one after another, with the order of
   results and of log entries unchanged from the sequential case.

## 4. Non-goals

- **Any widening of what the agent may do.** Every capability here is read-only
  or produces a patch the user still approves before anything is written.
- Changing the approval model of [#10](../10_persistent_command_approval.md) or the
  scope rules of [#34](../34_repository_config_trust_boundary.md). Requirement 3 is
  a constraint on the design, not an invitation to revisit them.
- Replacing [#24](../24_repository_map_and_monorepo_boundaries.md),
  [#25](../25_symbol_and_relationship_index.md) or
  [#26](../26_context_assembly.md). Those stay as specified.
- Removing the tool-round cap. Requirement 6 replaces a fixed count with a
  bounded budget; an unbounded turn is not the goal, and the enforced ceiling
  belongs to [#21](../21_task_plan_progress_and_budget/proposal.md).
- Changing hunk selection at review time — that is
  [#04](../04_hunk_level_patch_apply.md), and it is unaffected.
- Background or long-running processes as a feature (Phase 2 WP5), and
  subagents ([#38](../38_subagent_model.md),
  [#39](../39_coordination_and_conflict_handling.md)). Requirement 8 is
  *within-round* dispatch of read-only calls the model already made in one
  response: no second agent, no second conversation, no second context, and
  nothing that outlives the round. Multi-agent parallelism and its ownership
  and conflict rules remain #38 and #39 entirely.

## 5. Design

The five questions this section listed as open were settled against the tree on
2026-09-17. What was measured, and the four places the measurement contradicted
this spec's own §2, is in [`context.md`](context.md); what was decided is here.

### 5.1 What the first slice is

Requirements 1–4: ranged reads, `list_directory` and `search_content`, and
anchor-based region edits. They share one property that makes them one slice —
**none of them touches the round loop**. Requirements 5, 6 and 8 all do, and
each carries a dependency this one does not: 5 on spec 08's progress channel and
spec 17's markers, 6 on the budget specs, 8 on a batching change the round loop
does not yet have. Their decisions are recorded in §5.6 so the later slices
start from a position rather than a blank page.

### 5.2 The tool surface

`read_file` gains an optional line range; three tools are added.

| Tool | Arguments | Returns |
|---|---|---|
| `read_file` | `path`, `start_line?`, `end_line?` | The requested lines, always stating the range returned and the file's total line count |
| `list_directory` | `dir?`, `depth?` | Repository-relative paths under the ignore rules |
| `search_content` | `pattern`, `path_glob?`, `max_matches?` | `path:line:text`, capped, stating what was cut |
| `edit_file` | `path`, `old_text`, `new_text`, `summary` | A reviewable patch, or a refusal |

`list_directory` is named to match [#52](../52_mcp_server_mode.md) §5.2, which
already commits to exposing it and a ranged `read_file` over the MCP server. A
different name here would have made that spec wrong.

**`max_file_bytes` becomes a cap on what is returned, not on what may be
inspected.** That is the change requirement 1 asks for, stated precisely: the
limit exists to keep a huge file out of the model's context, not to stop the
engine streaming one. So a read counts the file's lines by streaming it — cheap
at any size, since nothing is retained — and then returns at most
`max_read_lines` lines and at most `max_file_bytes` bytes, whichever binds
first, saying which one did.

A read of `tests/foundation.rs` therefore reports "lines 1–400 of 4420" instead
of spending a turn's whole context budget on one call, and a 5 MiB file is
readable by range *and* readable unranged, because the returned payload is
bounded either way. The outright refusal at `file_access.rs:63` disappears; it
was protecting the wrong thing.

The byte cap has to sit beside the line cap rather than under it, because one
400-line region of a minified or generated file can be larger than the whole
budget a line count suggests.

### 5.3 A region edit anchors on a matched string

`edit_file` takes `old_text` and refuses unless it matches **exactly once**.
Zero matches and two matches are both refusals, and the two-match refusal names
the count so the model can widen its anchor in the same round.

This is the load-bearing decision and it went to the failure mode, as §5 said it
should. A line range is cheaper to express and silently wrong against a stale
view of the file; an anchor against a stale view stops matching, so staleness
lands in a refusal instead of a wrong-region write. That matters because a
wrong-region write is caught only by a human reading the diff, which is the one
review step this spec must not lean on harder than it already does.

The conversion keeps requirement 4's promise structurally. `ProposedChange`
already carries `new_content: String`, and `PatchEngine::create_patch` re-reads
the old file to diff against it. So `edit.rs` resolves the path, reads the file,
splices the single match, and hands over a `ProposedChange` **identical in shape
to one built from a whole-file proposal**. Diff review, hunk selection,
redaction, checkpointing and audit are untouched because nothing downstream can
tell the two apart — not because this spec promises to leave them alone.
`patch_engine.rs` needs no change at all.

### 5.4 One walker

`indexer.rs` already walks the tree accumulating per-directory `.gitignore`
rules and rejecting symlinks that resolve outside the root. A second walker
would be a second place for those rules to drift, and the symlink check is a
security property, not a convenience.

The traversal moves to `tree_walk.rs` as a free function taking a visitor, and
`Indexer` becomes its first caller with an index-building visitor. The two new
navigation tools are two more visitors. The existing index tests are the guard
that the extraction changed no behaviour.

`regex` becomes a direct dependency of `workspace-engine`. It is already in
`Cargo.lock` transitively at 1.12.4 through `tokenizers`, under the same
MIT/Apache-2.0 already on `deny.toml`'s allow-list, so this is the precedent
[#46](../46_process_registry_and_orphan_sweep/proposal.md) set with `libc` and
not a new supply-chain decision. The reason to take it rather than hand-roll a
matcher is that `regex` is linear-time by construction: a pattern written by a
model cannot make the tool hang, which no backtracking matcher can promise.

### 5.5 Caps, and saying what was cut

Four keys, all `RepositoryKeyClass::RestrictOnly` like
[#21](../21_task_plan_progress_and_budget/proposal.md)'s `agent_max_task_tokens`,
so a cloned repository may lower a cap but never raise one — the rule
[#34](../34_repository_config_trust_boundary.md) exists to hold:
`max_read_lines` (400), `max_list_entries` (200), `max_search_matches` (50),
`max_match_line_chars` (500).

**Every truncated result states what was cut** — "50 of 231 matches across 12
files", "lines 1–400 of 4420". This is deliberately unlike `truncate_output`
(`command_runner.rs`), which keeps the tail and leaves no marker, so a reader
cannot tell output was cut. A model that believes it saw every match stops
searching; one told it saw fifty of two hundred narrows the pattern. The
principle is the one [#19](../19_token_and_cost_accounting/proposal.md) settled
for cost figures: a number presented without its caveat is worse than no number.

### 5.6 What the later slices inherit

- **Requirement 6's sequencing is no longer open.** This section used to ask
  whether the continuation budget waits for #19 and #21 or ships a provisional
  one. Both are Done: `agent_max_task_tokens` and a `TokenBudget` stop reason
  are live in `chat.rs`. Requirement 6 uses the real budget and invents nothing.
- **Requirement 6 has a second half this spec did not know about.**
  `OBSERVATIONS.md` entry 6 records that the within-turn message array grows
  without bound — an assistant message and a tool result per round, where a tool
  result can be a whole file — and nominates this spec's continuation budget as
  one of its two possible homes. A continuation that carries an unbounded array
  forward continues the problem rather than the task. That slice must decide it.
- **Requirement 8 needs batching before it needs concurrency.**
  `first_decodable_tool_action` (`chat.rs`) executes only the *first* decodable
  call in a round and silently drops the rest, so the multi-call shape §2
  assumed the code already had, it does not have. Concurrency is the second step
  of that slice, not the first.
- **Concurrency safety is derived, not declared.** `tool_action_marker`
  (`chat.rs`) already answers, in exactly one place, whether a crash mid-tool
  could have left a side effect. A tool that cannot is a tool safe to batch. A
  second declaration would be a second thing to keep in sync, and this one
  cannot drift because spec 17's recovery already depends on it being right.
- **A ranged read and [#26](../26_context_assembly.md)'s `ContextItem` ranges
  are the same representation.** A ranged read is what produces one. #26 adopts
  the range type this slice introduces rather than defining a second.

### 5.7 The trust boundary, structurally

Requirement 3 is met by where the code sits, not by a policy decision. All four
tools are engine tools dispatched in `chat.rs`; none reaches `command_policy.rs`.
So `command_allowlist` gains no entry, `is_low_risk_read_only` keeps its exact
six commands, and the shell-control gate is untouched. `PathPolicy` and
`SecretScanner` sit on all four on the same path `FileAccess::read_file` uses,
so a restricted path is unreadable and a secret is redacted whichever tool
reaches for it. The agent gains navigation *as a tool* and gains nothing *at the
shell*, which is the distinction requirement 3 draws.

## 6. Acceptance Criteria

Split by slice, per §5.1. The first slice is done when the first group passes;
the spec is done when both do.

### First slice — requirements 1–4

- Changing ten lines of a 150 KB file produces an edit payload proportional to
  ten lines, asserted by test against the payload size.
- A targeted edit whose anchor matches zero times, or more than once, is
  refused rather than applied — asserted by test.
- A file larger than `max_file_bytes` is readable by range.
- A navigation tool cannot read a restricted path and cannot return an
  unredacted secret, asserted by test against `PathPolicy` and `SecretScanner`.
- Navigating the repository requires no `command_allowlist` entry and no change
  to `is_low_risk_read_only`, asserted by running a representative task with an
  empty allowlist.
- An edit proposed as a region replacement reaches disk through the same
  `PatchEngine` path as a whole-file proposal — same review, same hunks, same
  checkpoint, asserted by comparing the resulting patch record.
- Every capped result states what it cut, asserted by test on each of the three
  read tools: a truncated result that reads as complete is the failure mode
  §5.5 exists to prevent.
- `Indexer` produces a byte-identical index across the `tree_walk` extraction,
  asserted by the existing index tests rather than a new one.

### Later slices — requirements 5, 6 and 8

- A command exceeding its timeout is terminated, and a stop issued during a
  running command takes effect during that command rather than after it.
- A task exceeding its tool-round budget continues with its state intact, and
  the continuation is bounded and appears in the audit log.
- Four read-only calls in one round complete in materially less wall-clock time
  than the same four run sequentially, and produce byte-identical results in
  identical order — asserted by comparing a concurrent run against a sequential
  one, not by timing alone.
- A round mixing read-only and mutating calls runs the mutating ones
  sequentially, asserted by test.
- A stop issued during a concurrent batch cancels every call in it.
- Session-log `seq` ordering after a concurrent round is deterministic across
  repeated runs of the same scenario.

### Both

- Every quality-gate command from `AGENTS.md` passes.

## 7. Implementation Notes

What implementing the first slice found that contradicted §5 or the plan, in
order of consequence.

1. **`read_file`'s range argument is not an `Option<LineRange>`.** §5.2 said the
   tool takes an optional range. `None` cannot say which of two callers it
   serves: context assembly wants the *whole* file and applies its own token
   budget, while the tool wants a bounded window. Encoding both as "no range"
   would have silently capped context assembly the day the line cap landed. The
   signature takes a three-variant `ReadWindow` (`Whole` / `Default` / `Range`)
   so each call site states its intent, and a large requested range is clamped
   to the cap rather than allowed to step around it.

2. **The orchestrator needed two fields the plan did not list.** `tasks.md`'s
   file-structure table names `chat.rs` as "four tool definitions, four decode
   arms, four dispatch arms" and stops there. Wiring `list_directory`,
   `search_content` and `edit_file` in fact required `ChatOrchestrator` to hold a
   `NavigationController` (for the two navigation tools) and a `PathPolicy` (for
   `region_edits_to_changes`), both passed in from `workspace_engine.rs`.

3. **The audit log path in the plan's own test was wrong.** Task 7's audit test
   asserted on `.damaian/audit.log`; `AuditLog::record` writes
   `data_dir/audit/events.jsonl` (`audit.rs:61`). The test uses the real path.

4. **The 150 KB fixture as written never reached 150 KB.** Task 5's plan built it
   as 6000 lines of `fn f{n}() {}\n`, which is roughly 84 KB — the assertion
   would have passed a fixture that never exceeded the threshold it was supposed
   to exceed. It is built at 12 000 lines instead, with the length asserted.

5. **One `read_file` call site was missed.** `damaian-cli/src/main.rs` still
   passed six arguments after the ranged-read signature change and only failed to
   compile at the whole-workspace test run in Task 7. Fixed to `ReadWindow::Whole`,
   which preserves the CLI's read-the-whole-file behaviour.

6. **The `edit_file` tool shape differs from §5.2's table.** The table named a
   single `path`/`old_text`/`new_text` triple; the schema takes `summary` plus an
   `edits` array, so one call can propose several anchored edits and they compose
   against each other rather than each being spliced against the on-disk file
   independently — which would silently drop all but the last edit to one file.

### 7.1 Eval coverage, added after the slice closed

The slice shipped with no [#18](../18_local_evaluation_harness/proposal.md)
scenario, which meant the four tools could stop being offered and every eval
would still pass. `navigated_edit` closes that: the same task as
`one_file_patch` — give the upload client a retry — reached by listing,
searching and a ranged read instead of being handed the path, and answered with
an anchored region instead of whole-file content. It asserts the same four
things `one_file_patch` does, which is requirement 4's claim that a region edit
is indistinguishable downstream, not duplication. Fifteen scenarios now; two
count guards in `tests/harness.rs` assert it.

Its assertions were falsified before being trusted: replacing the anchor with a
string that occurs more than once refuses the edit and fails `patch_touches`
and `approval_required`, and the harness exits non-zero.

**What it does not measure.** `absent_everywhere` reads the response, the
context files, the run record and the audit trace; a tool result reaches none of
them, so an assertion that a `search_content` over `secrets/` returned nothing
would pass whatever the tool returned. The engine's own tests cover the
behaviour (the redaction one mutation-tested); making the harness see tool
results is its own change. Fewer rounds it cannot measure either, because the
deterministic tier scripts every call — which is what §7.2 went to the live tier
for.

### 7.2 The live-tier A/B, measured 2026-09-18

Run as an A/B rather than against an older baseline, because #21, #48 and #53
moved the engine over the same period and a single after-number could not be
attributed to this slice. The before arm is a worktree at the slice's parent
commit and the after arm is `main`, so **no product code differs between the
arms except the slice itself**; three runs each against `deepseek-v4-flash`,
medians over the 14 scenarios both arms can run (`navigated_edit` exists only on
the after side and is excluded).

| | before | after | |
|---|---|---|---|
| tool calls | 29 | 55 | +90% |
| model calls | 34 | 63 | +85% |
| tokens | 92,876 | 191,729 | +106% |
| tokens per model call | 2,850 | 3,043 | +7% |
| scenarios completed | 14/14 | 13/14 | |
| failed assertions | 4 | 4 | |

**The premise as written is not supported.** This spec opens on a session that
ran out of tool rounds and concludes the tool surface is the reason. On this
scenario set the slice makes the model do *more*, and the token increase is
almost entirely more calls rather than heavier ones — the +7% per call is the
four extra tool definitions riding in every payload, which is a permanent
structural cost paid by every turn whether or not it navigates.

**What the slice does buy is visible in the tool mix**, summed over three runs
each:

| | before | after |
|---|---|---|
| `run_command` | 28 | 10 |
| `search_codebase` | 31 | 1 |
| `read_file` | 26 | 70 |
| `list_directory` / `search_content` / `edit_file` | — | 34 / 31 / 8 |
| outcomes `awaiting_approval` | **17** | **4** |

The model stopped shelling out to navigate, and approval stops fell from 17 to
4. That is requirement 2's actual claim — navigating the repository needs no
`command_allowlist` entry — measured rather than argued. The slice trades tokens
for human interruptions.

**The measurement found a defect in the measurement.** `toolRounds` read
identically — 18 — in all six runs, because it was counted from
`scenario.turns`, and the live tier ignores the scripts. It described the
scenario directory, not a model. Fixed to read the session log
(`trace::tool_rounds`), delimited by the `model_call` marker that opens each
round, so a provider emitting several tool calls in one message adds calls and
not rounds — the distinction this A/B needed and could not make. Pinned by
`tool_rounds_come_from_the_run_and_not_from_the_scenario_script`, which uses
`failed_validation_retry`: one scripted turn, several real rounds, so the two
sources disagree without needing a provider. Read the numbers above as tool
calls and model calls; the rounds column of the original runs is not evidence.

**What this does not settle.** All 14 scenarios are short — 1 to 8 calls — and
the premise is about a long task that exhausts the round cap, which a short task
cannot exhibit. So the result is "no benefit on short tasks, at a real cost",
not "no benefit". Testing the premise needs one long-task scenario, deferred
until Damaian is closer to production. Requirement 6's justification is weaker
than when this spec was written, and should be re-argued from a long-task
measurement rather than from the opening paragraph.

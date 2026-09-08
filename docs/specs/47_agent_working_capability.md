# Feature Spec: Agent Working Capability

Status: Not started
Order: 47 of 47
Roadmap: none. Like [`07`](07_generated_secret_override.md),
[`08`](08_stop_and_progress.md), [`13`](13_docker_command_support.md) and
[`34`](34_repository_config_trust_boundary.md), this spec is evidence-driven
rather than a roadmap graduation — a fourth exception to the rule in
[`README.md`](README.md). It came from measuring Damaian against its own
backlog: a session asked what capabilities Damaian needs to implement the
remaining specs, and ran out of tool rounds before it could answer.
Related implementation specs:
[`03_structured_tool_calling.md`](03_structured_tool_calling.md) (owns the tool
surface this extends), [`04_hunk_level_patch_apply.md`](04_hunk_level_patch_apply.md)
(hunk selection at review time, which this does not change),
[`08_stop_and_progress.md`](08_stop_and_progress.md) (owns cancellation and the
progress channel), [`10_persistent_command_approval.md`](10_persistent_command_approval.md)
and [`34_repository_config_trust_boundary.md`](34_repository_config_trust_boundary.md)
(the command trust boundary this must not widen),
[`19_token_and_cost_accounting.md`](19_token_and_cost_accounting.md) and
[`21_task_plan_progress_and_budget.md`](21_task_plan_progress_and_budget.md)
(own the budget this defers to),
[`24_repository_map_and_monorepo_boundaries.md`](24_repository_map_and_monorepo_boundaries.md),
[`25_symbol_and_relationship_index.md`](25_symbol_and_relationship_index.md) and
[`26_context_assembly.md`](26_context_assembly.md) (the durable versions of the
navigation and ranged-read floor this establishes).

## 1. Motivation

Every other spec describes what Damaian does for the user. This one asks a
different question: **can Damaian do a spec's worth of work in one sitting?**
Today it cannot, and the reason is not model quality — it is the shape of the
tool surface.

The evidence is in this repository. Task 2 of
[spec 17](17_durable_task_state_and_crash_recovery/tasks.md) mutation-tested a
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
belongs to [#26](26_context_assembly.md), which adds line ranges to
`ContextItem`, and structural navigation to [#24](24_repository_map_and_monorepo_boundaries.md)
and [#25](25_symbol_and_relationship_index.md). **This spec is the cheap
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

## 4. Non-goals

- **Any widening of what the agent may do.** Every capability here is read-only
  or produces a patch the user still approves before anything is written.
- Changing the approval model of [#10](10_persistent_command_approval.md) or the
  scope rules of [#34](34_repository_config_trust_boundary.md). Requirement 3 is
  a constraint on the design, not an invitation to revisit them.
- Replacing [#24](24_repository_map_and_monorepo_boundaries.md),
  [#25](25_symbol_and_relationship_index.md) or
  [#26](26_context_assembly.md). Those stay as specified.
- Removing the tool-round cap. Requirement 6 replaces a fixed count with a
  bounded budget; an unbounded turn is not the goal, and the enforced ceiling
  belongs to [#21](21_task_plan_progress_and_budget.md).
- Changing hunk selection at review time — that is
  [#04](04_hunk_level_patch_apply.md), and it is unaffected.
- Background or long-running processes as a feature (Phase 2 WP5), and
  subagents or parallelism ([#38](38_subagent_model.md),
  [#39](39_coordination_and_conflict_handling.md)).

## 5. Design

To be written when the work starts. Five things to settle first, each verified
rather than assumed:

- **Whether a targeted edit anchors on a matched string or a line range.** This
  is the load-bearing decision. A line range is cheaper to express and trivially
  wrong against a stale view of the file; an anchor fails closed when it matches
  zero times or more than once. The failure mode matters more than the
  ergonomics: a wrong-region write is caught only by a human reading the diff,
  which is the one review step this spec must not lean on harder than it
  already does.
- **Whether content search reuses the indexer's walk.** `indexer.rs` already
  walks the tree under `ignore_patterns`. A second walker would be a second
  place for the ignore and restriction rules to drift. `AGENTS.md` forbids a
  Node runtime dependency, so a shelled-out matcher is not an option either.
- **How continuation interacts with [#17](17_durable_task_state_and_crash_recovery/proposal.md)
  and [#21](21_task_plan_progress_and_budget.md).** #21 owns the enforced
  ceiling and #19 the accounting beneath it; this spec must not invent a second
  budget that later has to be reconciled. The open question is sequencing —
  whether requirement 6 waits for #19 and #21, or ships a provisional round
  budget it hands over when they land. Requirements 1–5 do not depend on either
  and can proceed regardless.
- **Whether streaming command output can reuse spec 08's progress channel**, and
  what cancelling a running child does to spec 17's action markers. A killed
  command's outcome is unknown, which is exactly the classification #17 exists
  to record — so cancellation must finish its marker honestly rather than
  reporting a clean stop.
- **What a ranged read means to context accounting.** [#26](26_context_assembly.md)
  adds line ranges to `ContextItem` and range deduplication; a ranged read is
  the thing that produces them. The two should agree on one representation
  rather than each carrying its own.

## 6. Acceptance Criteria

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
- A command exceeding its timeout is terminated, and a stop issued during a
  running command takes effect during that command rather than after it.
- A task exceeding its tool-round budget continues with its state intact, and
  the continuation is bounded and appears in the audit log.
- An edit proposed as a region replacement reaches disk through the same
  `PatchEngine` path as a whole-file proposal — same review, same hunks, same
  checkpoint, asserted by comparing the resulting patch record.
- Every quality-gate command from `AGENTS.md` passes.

## 7. Implementation Notes

To be completed during implementation.

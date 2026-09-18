# Agent Working Capability — Background and Corrections

Background for [`proposal.md`](proposal.md). It records what was measured
against the tree on 2026-09-17 while settling §5's five open questions, and the
places that measurement contradicted the proposal's own §2. The proposal states
what was decided; this file states what it was decided against, so a later
reader can tell a judgement from a fact.

## 1. What was measured

Every claim in the proposal's §2 was re-checked against `main` at
`611f45a`, because §2 was written from a survey and surveys go stale. Five of
its six rows held exactly. The corrections are §2 of this file.

Confirmed unchanged:

- `agent_max_tool_rounds` is 8 (`config.rs`), `ABSOLUTE_TOOL_ROUND_CAP` is 16
  (`chat.rs`), and `continue_debugging` is still the only continuation.
- `propose_patch`'s schema still reads `"Full replacement file content"`.
- `read_file`'s schema is still `{"path"}` alone, and `FileAccess::read_file`
  still refuses above `max_file_bytes` rather than truncating.
- The native tool list is still eight tools: `run_command`, `propose_patch`,
  `propose_plan`, `complete_step`, `read_file`, `search_codebase`,
  `read_git_status`, `read_git_diff`.
- `is_low_risk_read_only` is still exactly six commands.

## 2. Corrections

### 2.1 §2's command-execution row is stale

§2 says `Command::output()` — "blocking, no timeout, no streaming". The
`output()` half is no longer true:
[#46](../46_process_registry_and_orphan_sweep/proposal.md) replaced it with
`spawn()` plus `process_group(0)` and a registry handle, precisely so a child's
pid is knowable. No timeout and no streaming are still accurate, and requirement
5 still has work to do — but it starts from a spawned child rather than a
hidden one, which is most of what it needed.

### 2.2 Requirement 8 is further away than §2 implies

§2 notes that `ModelResponse` already carries `tool_calls: Vec<ToolCall>`, and
concludes that several calls in one round is "a shape the code has". It is a
shape the *type* has. `first_decodable_tool_action` (`chat.rs`) walks the vector
and returns on the first call it can decode; the remaining calls are silently
discarded, and nothing tells the model they were.

So requirement 8 is two changes, not one: execute a batch at all, then execute
the read-only part of it concurrently. The proposal's §5.6 records this so the
later slice does not plan against the wrong starting point.

### 2.3 Requirement 4 is cheaper than §2 implies

§2 frames region edits as a payload problem, which they are. What it does not
say is that the receiving side already fits: `ProposedChange` carries
`new_content: String`, and `PatchEngine::create_patch` re-reads the old file
from disk to diff against it. A region edit therefore needs no new patch type,
no new engine path, and no change to `patch_engine.rs` — the splice happens in
`edit.rs` and hands over the same `ProposedChange` a whole-file proposal would.

This is why requirement 4's "leaving diff review, hunk selection, redaction,
checkpointing and audit untouched" can be a structural property rather than a
promise: nothing downstream can distinguish the two.

### 2.4 Requirement 6's sequencing question is closed

§5 asked whether requirement 6 waits for [#19](../19_token_and_cost_accounting/proposal.md)
and [#21](../21_task_plan_progress_and_budget/proposal.md) or ships a
provisional round budget. Both are Done. `agent_max_task_tokens` is a
restrict-only ceiling and a `TokenBudget` stop reason is live in the round loop.
The question does not need answering; requirement 6 uses what exists.

### 2.5 `OBSERVATIONS.md` entry 6 names this spec

Recorded 2026-09-15, found while writing [#55](../55_conversation_compaction.md):
within a turn the message array grows without bound — an assistant message and
a tool result per round, and a tool result can be a whole file. The entry says
this, not conversation length, is what reaches #21's per-turn ceiling, and
nominates "a section of spec 47's continuation budget" as one of its two
possible homes.

It matters to requirement 6 specifically: a continuation that carries an
unbounded array forward continues the problem rather than the task. The entry
also warns against folding it into #55, because it runs mid-turn against actions
#17 may classify as unknown. That slice has to decide it; this one does not
touch it.

## 3. What the tree does not have

Checked because three of §5's questions assumed one of these existed:

- **No `walkdir` or `ignore` crate.** `indexer.rs` hand-rolls its recursive walk
  over `fs::read_dir`, accumulating per-directory `.gitignore` rules and
  rejecting symlinks that canonicalize outside the root. Reusing it is not a
  preference, it is the only way the symlink check is not duplicated.
- **No `regex` crate as a direct dependency** — but it *is* in `Cargo.lock`
  transitively at 1.12.4, through `tokenizers`. (Not `syntect`: it is built with
  the `regex-fancy` feature and pulls `fancy-regex` instead. Checked with
  `cargo tree -i regex` while implementing.) Promoting it is the same
  move [#46](../46_process_registry_and_orphan_sweep/proposal.md) made with
  `libc`: already in the tree, already on `deny.toml`'s allow-list, no new
  license and no new download.
- **No async runtime and no `rayon`.** Concurrency in `workspace-engine` is
  `std::thread::spawn`, used in four modules. Requirement 8's concurrency is
  therefore scoped threads with results collected by index — which is also the
  cheapest way to get the deterministic ordering #17's `seq` replay needs.

## 4. Two things worth not copying

Found while reading the neighbouring code, and deliberately not imitated:

- **`truncate_output` (`command_runner.rs`) cuts silently.** It keeps the tail
  and leaves no marker, so nothing downstream can tell output was truncated.
  Every capped result in this spec states what it cut. The existing command path
  is out of scope here and is worth its own `OBSERVATIONS.md` entry.
- **`search_codebase` finds files, not call sites.** It is index-scored keyword
  overlap or embeddings. `search_content` is not a replacement for it and does
  not touch it: one answers "which files are about this", the other answers
  "where exactly is this string". Both stay.

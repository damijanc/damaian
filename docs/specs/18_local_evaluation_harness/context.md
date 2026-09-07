# Context: Local Evaluation Harness and Metric Baseline

Why this spec exists and what the code looks like today. The decision is in
[`proposal.md`](proposal.md); the execution order is in [`tasks.md`](tasks.md).

## 1. Motivation

Damaian has 232 passing tests and no way to tell whether it is getting better at
its job.

The tests assert that components behave: that `CommandPolicy` classifies a
Docker command as high-risk, that `patch_engine` refuses to overwrite a changed
file, that the secret scanner catches a seeded pattern. None of them assert that
Damaian, given a repository and a request, produces a useful answer, references
the right files, or stays inside its approval boundary end to end. A change that
makes the assistant noticeably worse — a context-assembly regression, a prompt
change that stops it citing files, a tool description that nudges it away from
requesting commands — passes the whole suite.

Every phase after this one is justified by a claim about improvement, and the
roadmap's own metric set is unevaluable until something emits it. Phase 6's
readiness gates are defined as thresholds on numbers that nothing currently
produces. This work package is the measuring instrument, and its output is the
baseline that makes a later regression a diff rather than a recollection.

## 2. Current State

- **232 tests pass, 2 are `#[ignore]`d.** Mostly inline `#[test]` modules, plus
  two integration files: `crates/workspace-engine/tests/foundation.rs` and
  `crates/workspace-engine/tests/semantic_search.rs`. They test components, not
  end-to-end behaviour against a repository.
- **`DAMAIAN_MOCK_MODEL_RESPONSE` is narrower than it looks.** It is read in
  exactly two places, both in the CLI (`crates/damaian-cli/src/main.rs:271` and
  `:320`, the `ask` and `propose-edit` paths), and it carries a **single** canned
  response string. It cannot express a multi-round tool-calling conversation, and
  nothing in `desktop-shell` or the chat loop consults it.
- **The real mock foundation is in-crate and better.** `MockModelAdapter`
  (`crates/workspace-engine/src/model.rs:215`) supports a *sequence* of
  responses, per-response tool calls, `finish_reason: "length"` truncation
  simulation, reasoning content, and it records every request it was handed so a
  test can assert on what was sent. `MockModelTransport` (`model.rs:541`) does
  the same at the transport layer, including a failing variant. Both are `pub`.
- **`DAMAIAN_DATA_DIR`** (`crates/workspace-engine/src/config.rs:192`) redirects
  all app data, which is how a run is isolated from the user's real data.
- **The audit log is the closest thing to a task trace.**
  `AuditLog::record(event_type, fields)`
  (`crates/workspace-engine/src/audit.rs:42`) writes redacted JSONL under
  `<data_dir>/audit`. It records events, not the structured per-task record a
  harness needs.
- **No token or cost data exists.** `ModelRun`
  (`crates/workspace-engine/src/model.rs:158-177`) carries no usage fields, and
  the only token figure anywhere is the `payload.len() / 4` estimate in
  `ModelAdapter::estimate_tokens` (`model.rs:209`).
  [Spec 19](../19_token_and_cost_accounting.md) supplies these.
- **Work is bounded by round count, not tokens**: `agent_max_tool_rounds` and
  `agent_tool_retry_limit` in `Config`.
- **No fixture repositories exist**, and none can be committed with a nested
  `.git`.


# Feature Spec: Task-Scoped Protected Patch Paths

Status: Done 2026-11-01 (Tasks 1–4 built 2026-10-31; final integration and
close-out on 2026-11-01. Scoped local checks are recorded in §7; the
workspace-wide Clippy and test suite remain CI-only).
Order: 98 of 98
Plan: none. A defect reported in use, like #73–#97.
Depends on: [#20](20_working_modes/proposal.md) (Code-mode patch approval),
[#21](21_task_plan_progress_and_budget/proposal.md) (durable task state),
[#23](23_verification_loop/proposal.md) (repair continuations), and
[#57](57_worker_sessions/proposal.md) (worker sessions), all built.

## 1. Motivation

A worker was told directly not to touch two unrelated files. After a failed
check, its repair turn proposed a one-file patch deleting one of them. The user
rejected it, so nothing was written, but the model had already violated an
explicit task constraint by proposing the edit.

Today a phrase such as “do not touch `crates/desktop-shell/src/lib.rs`” is
prompt text only. A rejection is recorded per patch file, but it neither
becomes a durable no-touch constraint nor prevents a later repair from
proposing the same path. That is the wrong boundary for a user who has named
unrelated work that must remain out of scope.

This spec makes an explicit user path exclusion a task-scoped patch capability:
the engine, not the model, refuses a patch that includes it. It must survive
the patch/verification/repair hand-off and a worker restart.

## 2. Requirements

1. A direct user request may establish protected repository-relative paths.
   The initial syntax is deliberately narrow and deterministic: `do not touch`
   or `do not modify`, followed by one or more backtick-delimited paths. The
   desktop and CLI also provide an explicit protected-path control for paths a
   user wants to add after the task starts. Model output, fetched content,
   repository files, and tool output cannot establish a protected path.
2. Each path is canonicalised and must be a file or directory inside the task's
   repository. A directory protects every descendant. Ambiguous prose and an
   invalid, missing, absolute, or outside-repository path do not silently
   create a constraint; the user sees the reason and can use the explicit
   control instead.
3. Protected paths are durable task state. They carry from a patch task to its
   verification, repair, and ordinary plan continuation, including a worker's
   own process. A new user-started task does not inherit them unless the user
   explicitly sets them again.
4. Every patch-producing route enforces the active task's paths before a patch
   is stored or shown: `edit_file`, `propose_patch`, and the legacy edit flow.
   A patch containing any protected file is refused as a whole, names the
   protected paths, and is returned to the model as a failed tool result so it
   can choose an in-scope alternative. It must not create a review card.
5. Applying a stored patch repeats the check against the task's current
   protected paths. This closes the race where a path becomes protected after a
   preview was made or a recovered card is applied after restart. A partial
   apply may still accept selected unprotected files, but it refuses any
   selected protected file without writing it.
6. A user's rejection of one proposed file is not itself a permanent protected
   path. Rejection can concern the patch's content, hunk, or timing rather than
   every future edit to that file. The rejected decision remains durable review
   evidence and the user can explicitly protect the path when that is intended.
7. The protected-path state and every refusal are visible in the session log,
   worker progress, and the desktop session view. The model receives the active
   exclusions in its task context as a reminder, but enforcement does not rely
   on that reminder.
8. Existing command policy remains unchanged. This is a fence for patch and
   edit-tool file targets, not a claim to sandbox arbitrary shell commands. A
   shell command remains separately approval-gated under the command policy;
   its explicit approval is not inferred from a protected-path setting.
9. Secret redaction, path policy, file-hash patch safety, patch approval,
   recovery, and the one-file rejection flow remain intact.

## 3. Non-goals

- Inferring a permanent write restriction from arbitrary natural language,
  model text, fetched pages, or repository instructions. The narrow direct-user
  syntax and explicit control avoid a false sense of protection.
- Turning a rejected patch file into an implicit ban on all future edits.
- Replacing command approval or providing an operating-system sandbox for
  arbitrary shell commands.
- Changing repository-level `restricted_patterns`, which remain a broader
  policy boundary rather than a task's user-owned scope.
- Retrofitting protected paths into historical tasks that did not record them.

## 4. Design

### 4.1 Durable source and scope

Add a `TaskProtectedPaths` session event keyed by the task that the direct user
request starts. It records canonical repository-relative paths, the user-facing
source (`prompt` or explicit control), and its timestamp, but never model or
repository-supplied text. `SessionStore` resolves the active exclusions for a
task by following its `plan_resumed` / verification continuation ancestry; it
does not inherit them merely because another task belongs to the same session.

The direct-request parser accepts only code spans after the exact phrase `do
not touch` or `do not modify`, splitting multiple spans in the same sentence.
The desktop's **Protect paths** affordance and `damaian protect-paths` use the
same canonicalisation and event writer. A successful set or change is rendered
with the active task and included in worker progress. Removing a protection is
an explicit user action, recorded as a new event; a model cannot remove it.

### 4.2 Patch proposal and application fences

`ChatOrchestrator` resolves the active paths before dispatching `edit_file` and
`propose_patch`; the legacy edit route does the same from its supplied task.
`EditOrchestrator` receives the resolved set and rejects any proposed target
that is the protected path itself or its descendant. The guard sits before
`PatchStore::save`, so no pending patch or preview can misleadingly offer the
file.

`apply_stored_patch` resolves the patch's task protections again before
checkpointing or writing. It checks the selected paths, so a partial review can
apply unrelated selected files while refusing an attempt to include a protected
one. The refusal is a known no-write outcome: it leaves no ambiguous action
marker, no patch decision for the protected path, and no verification due.

The error text is concise and path-specific: “Patch would modify protected task
path: …”. Agentic routes return it through the ordinary tool result. The
desktop card marks only the rejected selection and keeps other pending files
reviewable; the worker stays waiting for an answer rather than silently
continuing around the refusal.

### 4.3 Continuation and recovery

`resume_plan`, `verify_due`, repair turns, `continue_task_after_verification`,
and worker control all resolve the same source task's protections. Recovery
reattaches only a card whose remaining files are not now protected; a stale
card containing a newly protected selected file is shown as refused, not as an
actionable approval. The model-context reminder is generated from the durable
resolved set each turn, so compaction or a fresh worker cannot drop it.

## 5. Acceptance Criteria

Engine coverage proves all of the following:

- A direct user request with `do not touch` and backtick paths records the
  canonical file and directory exclusions; malformed or ambiguous wording
  records none and produces a user-visible reason.
- `edit_file`, `propose_patch`, and the legacy edit route refuse a patch that
  targets a protected file or a child of a protected directory, write no patch
  file, and return the path-specific tool result.
- A multi-file patch with one protected target is refused whole at proposal
  time; after an exclusion is added, a stored multi-file patch accepts selected
  unprotected files and refuses a selected protected file without writing it.
- A fresh `SessionStore` and a restarted worker preserve the exclusions through
  an applied patch, `verification_due`, a repair attempt, and the ordinary
  continuation. A newly typed task in the same session has none.
- The model context and worker progress show the exclusions, but a model cannot
  set, clear, or broaden them.
- Existing partial rejection tests still prove that rejecting a file does not
  itself protect that path, and existing patch hash, secret, path-policy, mode,
  and command-approval tests remain green.
- Desktop and CLI tests cover adding, listing, and explicitly removing paths;
  a reloaded desktop session shows the same protected paths and does not offer
  a patch card containing a newly protected selected file.

## 6. Verification

Implementation must run the fast checks, scoped `workspace-engine`,
`desktop-shell`, and `damaian-cli` Clippy and nextest suites, the deterministic
evaluation tier, and `npm run specs:check`. The workspace-wide Clippy and test
suite remain CI-only under `AGENTS.md`.

## 7. Implementation Notes

### Task 1: durable state foundation

The event kind is `task_protected_paths`. Its payload is:

```json
{
  "taskId": "task_…",
  "action": "add",
  "source": "prompt",
  "paths": [{ "path": "src/nested", "kind": "directory" }],
  "createdAtMs": 0
}
```

`action` is `add` or `remove`; at Task 1 `source` is `prompt` or
`explicit_control`. Task 3 adds `inherited_worker_handoff` for a worker's
first-task copy of its parent's durable record.

`crates/workspace-engine/src/protected_paths.rs` owns the typed record, the
parser and canonicalisation. `SessionStore` owns append, replay and ancestry:

- `record_task_protected_paths(session_id, task_id, action, paths, source)` is
  `pub(crate)`. It appends one event after validating the task and every stored
  path; desktop and CLI must go through `WorkspaceEngine`'s user-action methods,
  which verify the session's repository first.
- `read_task_protected_path_events(session_id)` returns parseable events in log
  order; malformed path entries are dropped.
- `task_protected_paths(session_id, task_id)` replays add/remove events for the
  task and its continuation ancestry, newest-wins, in one pass over active
  events.
- `task(session_id, task_id)` is the shared unknown-task error path.

`WorkspaceEngine` exposes `task_protected_paths`, `add_task_protected_paths`,
`remove_task_protected_paths` and
`record_task_protected_paths_from_prompt`. Every write operation first compares
`repository_id_for_root(repository_root)` with the session's recorded
`repository_id`; a mismatch is an access refusal before canonicalisation or any
event, so a path from repository A cannot land on a task in repository B. The
two explicit operations use `ProtectedPathSource::ExplicitControl`; only the
prompt operation uses `ProtectedPathSource::Prompt`. Canonicalisation
establishes containment through `PathPolicy::resolve_for_write` before requiring
existing file/directory metadata, so a lexically outside or symlink-escaping
path is `OutsideRepository` even when its target does not exist. It stores the
`/`-separated repository-relative path and the `file`/`directory` kind, and
returns a typed `ProtectedPathError` for empty, absolute, missing,
outside-repository, repository-root or non-file paths.
`TaskProtectedPaths::covering_path` is the later fence's single answer for
whether a target is the protected path itself or a descendant of a protected
directory.

Explicit removal does not canonicalise against the filesystem. It matches the
requested value against the task's active canonical path list, so a protection
can still be removed after its file or directory was deleted or renamed.
Non-canonical, absolute, outside-repository and non-active values are refused;
the event records the exact active canonical path and keeps the earlier add in
the log.

Ancestry has two edges. `plan_resumed.resumedFrom` covers verification, repair,
worker and typed-plan continuations. A verification created for a due that had
no plan is linked by its `verification_started` following the
`verification_due` for the patch or command task. Session membership alone is
never an edge, so a new user-started task starts empty unless it carried a plan.
The ancestry test keeps two separate due/started/finished chains and a started
event with no due, so adjacency cannot leak one chain's protection into another.

The parser accepts the exact phrases `do not touch` and `do not modify`
(ASCII-case-insensitive) followed by backtick spans in the same sentence.
Spans may be separated by whitespace, comma, semicolon, colon, `and`, `or`,
`nor` or `then`; anything else before or between spans is ambiguous and
refuses the whole phrase. A phrase inside a code span is quoted text, not a
request. Model, repository, fetched and tool text have no API call site: only
the user-prompt entry point reads prose, and the store records only canonical
paths.

Task 1 coverage is in `protected_paths::tests`: parser acceptance and refusal,
file and directory canonicalisation including descendant coverage, typed path
errors, missing-outside containment, cross-repository refusal with no event,
explicit add/remove with source and timestamps, add/remove replay through a
fresh `SessionStore`, deletion/rename removal through a restarted store and
engine, non-canonical/outside/absolute/non-active removal refusals, new-task
isolation, `plan_resumed` and two-chain verification ancestry, and proof that
message and repository text write no protection. Scoped checks passed locally:
`cargo fmt --all`, `cargo clippy -p workspace-engine --all-targets --locked -- -D warnings`,
`cargo nextest run -p workspace-engine protected_paths --locked` (12 passed) and
`cargo nextest run -p workspace-engine session::tests --locked` (40 passed).
The workspace-wide Clippy and test suite remain CI-only.

### Task 2: prompt boundary, reminder, proposal and apply fences

The narrow prompt grammar is now wired at the two original direct-user task
boundaries, after each task and its user message exist:

- `ChatOrchestrator::ask_with_session_inner` parses only an ordinary new task.
  A pre-created task (`ask_with_session_using_task`, spec 100) and a worker
  session are skipped, because their prompt is constructed or worker text, not
  the user's original request. Verification, repair, plan and worker
  continuations do not pass through this call site.
- `EditOrchestrator::propose_edit` parses its own one-shot task's prompt.

Both call the shared `protected_paths::record_task_protected_paths_from_prompt`
helper; `WorkspaceEngine::record_task_protected_paths_from_prompt` now delegates
to it, so the repository check, canonicalisation and event write have one
implementation. A refused phrase is appended as an assistant message with kind
`protected_path_refusal` and audited as `task_protected_path_refusal`; it writes
no protection event. A valid request with one invalid span records the valid
paths and reports each invalid one with its reason.

The model reminder is `protected_paths::protected_paths_reminder`, derived from
the durable resolved set and applied by `ChatOrchestrator::run_agentic_turn`
under the idempotent `[protected task paths]` marker. Because it runs in the
shared turn loop, ordinary turns, verification and repair continuations, plan
continuations, worker continuations and paused resumes all receive it, and a
resume replaces the previous reminder rather than stacking copies. The legacy
edit flow applies the same helper to its one-shot model messages. A durable-read
failure propagates and ends the turn before the model is called; it is not
treated as "no protections". The reminder is never the enforcement boundary.

Proposal fences resolve the task's typed `TaskProtectedPaths` and check the
complete resulting `ProposedPatch.files` after `PatchEngine::create_patch` and
before `PatchStore::save`:

- `propose_patch` and `edit_file` do this in their tool arms; a refusal becomes
  an ordinary `ActionOutcome::Failed` tool result, so the turn continues and the
  model can choose an in-scope alternative.
- `EditOrchestrator::propose_edit` does it before its store write and records
  the refusal as the edit task's failure.

`ensure_patch_paths_allowed` uses `TaskProtectedPaths::covering_path`, so a
protected directory covers descendants. If any file in a multi-file proposal is
covered, the whole proposal is refused with
`Patch would modify protected task path: …`; no review card and no PatchStore
record exist.

`apply_stored_patch` repeats the fence immediately after loading the patch,
before the mode and hook refusals, apply marker, checkpoint or any write. It
checks only the selected files: all patch files when `approved_paths` is `None`,
otherwise exactly the selected set. An unselected protected file therefore does
not block a selected unprotected partial apply, while any selected protected
file refuses the whole apply. A refusal appends a path-specific engine line to
the patch task's session log and the matching `task_protected_path_refusal`
audit event, then returns the same `AccessDenied`. It is not a patch-file
rejection decision and leaves no write, checkpoint, patch decision, action
marker or `verification_due`.

Deviations and discoveries:

- `EditOrchestrator` has no `PathPolicy` field, so prompt canonicalisation uses
  `PathPolicy::new(&self.config)`, the same config-derived policy the engine
  holds. The orchestrator resolves the durable set from the `SessionStore` it
  already owns instead of receiving a set from `WorkspaceEngine`; both fences
  still read only the typed record, never prompt text.
- The legacy edit flow receives the reminder as well, so "every turn" includes
  its one-shot model call, not only chat turns.
- `ChatMessage::kind` now documents `protected_path_refusal` alongside
  `plan_reminder` and `memory_refusal`.

Task 2 coverage added in `protected_paths::tests`:

- `a_direct_prompt_records_paths_and_a_malformed_one_refuses_without_protection`
  — file and directory prompt recording, malformed wording refused with no
  event, missing path refused visibly with no protection.
- `a_protected_directory_descendant_refuses_the_whole_proposal_before_storage`
  — a multi-file `propose_patch` with one descendant is refused whole and
  stored nowhere.
- `edit_file_refuses_a_protected_file_before_storage`.
- `the_legacy_edit_flow_refuses_a_protected_file_before_storage` — also proves
  the edit model request carries the reminder.
- `an_apply_race_is_refused_without_a_write_checkpoint_decision_or_verification_due`
  — protection added after the preview; checks file content, checkpoint count,
  patch decisions, verification events, the action marker, and that the
  path-specific refusal line was durably recorded.
- `a_selected_unprotected_partial_apply_ignores_an_unselected_protected_file` —
  partial apply succeeds for the selected unprotected file and refuses the
  selected protected one.

`chat::mode_refusal_tests::a_continuation_gets_the_inherited_reminder_without_reparsing_its_prompt`
covers a `plan_resumed` verification continuation: the constructed prompt's
`do not touch` text writes no event, while the inherited `src/a.rs` reminder is
present in the request.

`chat::mode_refusal_tests::a_protected_path_read_failure_stops_the_turn_before_the_model`
passes a task absent from the durable log to `run_agentic_turn`; the read error
ends the turn and the mock adapter records zero model requests.

Checks run locally for Task 2: `cargo fmt --all`; `cargo clippy -p
workspace-engine --all-targets --locked -- -D warnings`; `cargo nextest run -p
workspace-engine protected_paths --locked` (18 passed; one earlier parallel run
marked the pre-existing canonicalisation test leaky, and its isolated rerun
passed); the two `chat::mode_refusal_tests` above (2 passed); `cargo nextest run
-p workspace-engine --lib --locked` (670 passed, 5 skipped); `cargo nextest run
-p workspace-engine --test verification_loop --test crash_recovery --test
recovery_prompt --test plan --test agent_tools --locked` (232 passed, 2 skipped);
`npm run specs:check -- --strict` (105 specs agree); `typos` on every changed
source and spec file. The workspace-wide Clippy and test suite remain CI-only.

### Task 3: worker handoff, progress and recovery reattachment

A worker is a separate session, so `plan_resumed` ancestry cannot carry the
parent's paths across the spawn. The handoff is now explicit:

- `SpawnRequest` carries an optional `parent_task_id`, read from the
  `parent_task_id` spawn form field. The desktop fills it from the assistant
  message's task id for a `worker-prompt` block, and from the paused task id the
  `/api/plan-worker-prompt` response now returns beside the generated prompt.
- `Supervisor::spawn` refuses a parent task without a parent session, validates
  a supplied task with `SessionStore::task(parent_session_id, parent_task_id)`,
  and snapshots `task_protected_paths(parent_session_id, parent_task_id)`. It
  never parses the worker prompt, which may be model-generated.
- Whenever a parent session is supplied, the spawn also compares that session's
  `repository_id` with `repository_id_for_root(request.repo)`. A mismatch is
  refused before any worktree, branch, session or audit record exists, so a
  crafted request cannot transplant repository A's canonical relative paths
  into a worker for repository B. The check applies whether or not a parent
  task is named.
- `WorkerRecord` stores `parent_task_id` and `inherited_protected_paths`, both
  `#[serde(default)]` so a record written before spec 98 decodes with no
  handoff. The worker's spawn audit carries the same fields.
- `ChatOrchestrator` seeds the first worker task from that snapshot with one
  `task_protected_paths` add event whose source is the new
  `inherited_worker_handoff`. This is explicitly not `prompt` or
  `explicit_control`: the worker prompt was not read and no user action
  established the paths in the worker session. The write happens immediately
  after the first task is created, before context assembly or any model
  request.
- Seeding is limited to the earliest task in the worker session and checks for
  an existing `inherited_worker_handoff` event on it, so a restart or reload
  preserves the record without duplicating it. A later user-started task in the
  same worker session starts empty; verification, repair and plan
  continuations continue to inherit through the ordinary durable ancestry.

Worker progress now has a dedicated event. `TurnProgress::ProtectedPaths`
carries the resolved typed `TaskProtectedPaths` for the task at every agentic
turn start. The worker's `ProgressSink` writes it as
`ProgressKind::protected_paths` with `taskId` and the canonical `paths`
(`path`, `kind`), through the existing redacting `ProgressWriter`. The desktop
SSE relay ignores the variant until its visible controls land; a worker's
progress file records it now.

Recovery reattachment refuses a stale card instead of offering it. For a
pending patch, `recovery::reattach_patch` loads the patch, reads the task's
current protections, and checks only files the log has not already decided. If
a remaining file is covered, it appends the path-specific
`protected_path_refusal` message, writes the matching `task_protected_path_refusal`
audit event, and returns the ordinary `Unavailable` recovery result through
`fail_task`. It creates no patch-file decision, action marker, write,
checkpoint or `verification_due`. An unprotected pending card stays a `Patch`,
and a protected file that was already decided does not cause a false refusal.

Task 3 coverage:

- `protected_paths::tests::worker_handoff_seeds_the_first_task_before_the_model_and_never_parses_the_prompt`
  proves the first worker task receives the exact inherited snapshot before the
  first `model_call` marker, model-authored `do not touch` text adds nothing, a
  fresh engine reads the same task record with exactly one handoff event, a
  later task in the same worker session is empty, and progress reports the
  typed paths.
- `recovery_refuses_a_pending_patch_whose_remaining_file_is_now_protected`
  proves a newly protected remaining file becomes `Unavailable` with a
  path-specific reason and refusal record, while leaving no decision, action
  marker, write, checkpoint or verification due.
- `recovery_keeps_unprotected_cards_and_ignores_decided_protected_files` proves
  an unprotected card remains a `Patch` and a protected already-decided file
  does not cause a false refusal.
- `session::tests::a_worker_record_written_before_handoff_fields_still_decodes`
  proves backward-compatible `WorkerRecord` decoding.
- `desktop-shell worker_supervisor::a_parent_task_snapshot_crosses_the_handoff_and_prompt_text_cannot_broaden_it`
  proves spawn persists exactly the parent task's resolved paths, ignores the
  prompt text, gives a sidebar worker nothing, and refuses a task from another
  session.
- `desktop-shell worker_supervisor::a_parent_session_from_another_repository_is_refused_before_any_worker_state`
  proves a cross-repository parent handoff is refused with and without a parent
  task, before any session, child record, worktree or audit event exists, while
  the same-repository handoff still snapshots and spawns.
- `desktop-shell worker::protected_path_progress_is_typed_canonical_and_redacted`
  proves the progress payload carries the task id and canonical typed paths and
  that a secret-shaped path is redacted in the file.
- `plan_turn::a_plan_under_review_is_handed_to_a_worker` now asserts the
  plan-to-worker prompt carries the paused task id, not the proposal id.

Checks run locally for Task 3: `cargo fmt --all`; `cargo clippy -p
workspace-engine --all-targets --locked -- -D warnings`; `cargo clippy -p
desktop-shell --all-targets --locked -- -D warnings`; `cargo nextest run -p
workspace-engine protected_paths --locked` (21 passed); the two worker-record
tests (2 passed); the four engine integration binaries `crash_recovery`,
`recovery_prompt`, `plan_turn` and `verification_loop` (167 passed, 2 skipped);
`cargo nextest run -p desktop-shell worker --locked` (63 passed); `cargo
nextest run -p workspace-engine --lib --locked` (674 passed, 5 skipped);
`node --check crates/desktop-shell/static/app.js`; `npm run lint:web` (only the
pre-existing `style.css` descending-specificity warning); `npm run specs:check
-- --strict` (105 specs agree); `scripts/quality-gate.sh fmt node lint typos
deny` (deny needed an out-of-sandbox rerun only because the sandbox made
Cargo's advisory lock read-only); `DAMAIAN_DATA_DIR=.damaian cargo run -p
eval-harness -- run --tier deterministic` (all 46 scenarios passed; its first
sandboxed attempt could not bind the local stub, and the approved rerun
passed); and `typos` on every changed source and spec file. The workspace-wide
Clippy and test suite remain CI-only.

### Task 4: visible desktop and CLI controls

Reusable control behavior lives in `workspace-engine`, not in `desktop-shell` or
`app.js`. The two front ends consume the same typed model:

- `TaskProtectedPathsControl` is the complete view model: `available`,
  `taskId`, `taskLabel`, `note`, and `rows`. Each row carries the canonical
  path, the typed `kind`, the engine's `kindLabel` (`file` or `directory (and
  everything under it)`) and its `removeLabel`.
- `WorkspaceEngine::task_protected_paths_control(repository_root, session_id)`
  repository-binds the read and selects the newest task from durable creation
  order. A session with no task returns an explicit unavailable control instead
  of an empty actionable list.
- `WorkspaceEngine::task_protected_paths_control_for_task` is the same model for
  a named task, so the CLI can list an older task even after newer tasks exist.
- `TaskProtectedPathUpdate` now carries the refreshed `control` and the
  engine-written `outcome` sentence alongside accepted/refused paths. Both
  front ends render those fields; neither reconstructs kind labels, acceptance
  meaning or refusal prose.

The generic `add_task_protected_paths` / `remove_task_protected_paths` perform
explicit named-task mutations and therefore accept an older task. The desktop
uses separate `add_current_task_protected_paths` /
`remove_current_task_protected_paths` methods, which re-select the current
control target in the engine and refuse a stale supplied task before any event.
The shell does not read, sort or choose tasks. Blank-line trimming and empty
input are also engine semantics: the shell only splits the textarea on its
transport separators.

Desktop routes remain:

- `GET /api/protected-paths?repo=&session_id=&task_id=` for the typed active
  `TaskProtectedPaths` (API coverage; the UI does not call it).
- `POST /api/protected-paths-add` and `POST /api/protected-paths-remove`, whose
  `paths` field is the textarea value unchanged. Each returns
  `{ protectedPathsControl, outcome, accepted, refused }`.

`GET /api/session` now includes `protectedPathsControl` in the same payload.
When `repo` is supplied it is repository-bound: a mismatched repository is an
error, not a 200 with an unavailable control. A missing `repo` still returns an
unavailable control for legacy callers such as the worker note view.

The UI renders `protectedPathsControl` directly from `/api/session`; it does not
make a second decision-making read. `app.js` contains no task selection, id
sorting, availability decision, kind inference, accepted/refused interpretation
or outcome composition. Add and remove capture the server-provided repo,
session and task at request start and ignore a response after navigation, so
stale async work cannot repaint another session.

CLI syntax is unchanged and now uses the shared engine model:

```text
damaian protect-paths <repo> <session> <task> list
damaian protect-paths <repo> <session> <task> add <path...>
damaian protect-paths <repo> <session> <task> remove <path...>
```

`list` uses `task_protected_paths_control_for_task`; `add` and `remove` use the
generic named-task mutations. The CLI prints the engine's `outcome` and active
rows; it does not decide action meaning or kind labels itself.

Task 4 coverage:

- Engine `protected_paths::tests::the_control_selects_the_newest_created_task_and_reloads_durably`
  pins newest-task selection, labels, durable reload, stale current-control
  refusal without an event, and generic access to a named older task.
- Engine `the_control_reports_no_task_and_refuses_another_repository` pins the
  explicit no-task state and a repository mismatch refusal with no event.
- Engine `a_partial_update_carries_the_shared_outcome_and_control_rows` pins the
  shared outcome sentence, labels and blank-input semantics.
- Desktop route tests cover list/add/remove, file and directory kinds, partial
  invalid refusal, empty textarea, cross-repository and wrong-task refusal with
  no event, session-payload target selection and no-task state, stale desktop
  action refusal, repository-mismatch session refusal, and durable reload. The
  source test pins render-and-submit client wiring and the absence of client
  task/outcome/kind logic.
- CLI tests cover list/add/remove, an older named task with a newer task
  present, invalid path, repository mismatch, durable reload, help usage, and
  proof that the CLI prints the engine result rather than rebuilding it.

Checks run locally for Task 4: `cargo fmt --all`; `cargo clippy -p
workspace-engine -p desktop-shell -p damaian-cli --all-targets --locked -- -D
warnings`; `cargo nextest run -p workspace-engine protected_paths --locked` (24
passed); `cargo nextest run -p desktop-shell protected_paths --locked` (5
passed); `cargo nextest run -p damaian-cli --test protected_paths --locked` (6
passed); `node --check crates/desktop-shell/static/app.js`; `npm run lint:web`
(only the pre-existing `style.css` descending-specificity warning); `npm run
specs:check -- --strict` (105 specs agree); and `typos`. The workspace-wide
Clippy and test suite remain CI-only. Spec 98 stays **In progress**; final
close-out and user documentation are not part of Task 4.

### Task 5: final integration, verification and close-out

The close-out audited every requirement and acceptance criterion against the
implementation and tests. Three narrow gaps were closed; no selection,
validation, canonicalisation, state transition, kind/outcome meaning or
accepted/refused interpretation moved out of `workspace-engine`.

- **The model reminder is now redacted.** `apply_protected_paths_reminder`
  inserts the reminder into the system message, which leaves the machine, so it
  now passes the orchestrators' `SecretScanner` like the worker progress file
  already did. A credential-shaped protected path therefore cannot reach the
  provider through the reminder. Covered by
  `chat::mode_refusal_tests::a_protected_path_reminder_is_redacted_before_the_model`.
- **Requirement 6 has its own executable proof.**
  `protected_paths::tests::rejecting_a_proposed_file_does_not_protect_it`
  rejects one file of a two-file legacy edit, checks that no protection event or
  active protection exists, checks that the rejected decision remains durable
  review evidence, and proposes the same file again successfully.
- **Acceptance criterion 7's stale-card clause now has a desktop test.**
  `desktop-shell recovery::a_reloaded_patch_card_with_a_newly_protected_file_is_unavailable`
  drives the shell's recovery sweep and proves a pending patch whose remaining
  file became protected renders as `unavailable`, never as a `patch` card.

No other requirement or criterion was missing coverage; the scan found the
program-visible behavior, refusal paths, durable replay, worker handoff,
progress, recovery and control tests recorded for Tasks 1–4 already in place.
The audit also corrected two stale comments that still called the desktop
controls "later work", and recorded the worker handoff and progress event in
`docs/TROUBLESHOOTING.md`.

Close-out documentation in the same change: `docs/USER_GUIDE.md` gained
"Protecting paths from a task's patches" (the prompt grammar, the panel, the CLI,
directory meaning, refusal behavior, rejection semantics and the command-policy
limit); `docs/TROUBLESHOOTING.md` gained the matching common-failure section,
the session event and handoff fields; `SECURITY.md` gained the patch-boundary
paragraph; and `CHANGELOG.md` lost the spec's `Unreleased` clause with no release
row added. `docs/specs/README.md` now lists #98 as Done, promotes #102 and
updates the blocked and pairing notes, and #102 and #106 name #98 as built.

Verification on 2026-11-01:

- `scripts/quality-gate.sh fmt node lint typos deny` — all five steps passed.
- `cargo clippy -p workspace-engine --all-targets --locked -- -D warnings`,
  `cargo clippy -p desktop-shell --all-targets --locked -- -D warnings` and
  `cargo clippy -p damaian-cli --all-targets --locked -- -D warnings` — all
  passed.
- `cargo nextest run -p workspace-engine protected_paths --locked` — 25 passed.
- `cargo nextest run -p workspace-engine --lib --locked` — 679 passed, 5
  skipped.
- `cargo nextest run -p workspace-engine --test verification_loop --test
  crash_recovery --test recovery_prompt --test plan_turn --test agent_tools
  --locked` — 205 passed, 2 skipped.
- `cargo nextest run -p desktop-shell --lib --locked` — 255 passed, 6 skipped.
- `cargo nextest run -p desktop-shell --test workers --test worker_process
  --locked --run-ignored all` — 12 passed.
- `cargo nextest run -p damaian-cli --locked` — 13 passed.
- `npm run specs:check -- --strict` — 105 specs agree.
- `DAMAIAN_DATA_DIR=.damaian cargo run -p eval-harness -- run --tier
  deterministic` — all 46 scenarios passed, exit 0.

The workspace-wide Clippy and test suite remain CI-only under `AGENTS.md`, and
CI has not yet run for this change; the full seven-check gate is not claimed
until its Quality workflow is green.

**For later tasks:** enforcement reads the resolved `TaskProtectedPaths` (or its
`covering_path`) for the active task, never a re-parsed prompt. The turn loop
records prompt protections only at the two direct-user boundaries; constructed
verification, repair and worker prompts inherit through ancestry or, at a
worker spawn, through the explicit first-task handoff event. Proposal/apply
fences, the model reminder, worker progress, recovery reattachment, and the
visible engine-owned desktop and CLI controls are built, and final integrated
verification and close-out are complete.

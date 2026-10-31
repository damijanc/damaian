# Feature Spec: Verify After a Command Changes Files

Status: Done 2026-10-31 (specified and built the same day; local checks in §6,
the full gate is left to CI).
Order: 85 of 85
Plan: none. A defect reported in use, like #73–#84: an exception to the rule
in [`README.md`](README.md).
Depends on: [#23](23_verification_loop/proposal.md),
[#16](16_session_checkpoints_and_rewind.md) (the command census) and
[#77](77_continue_the_task_after_verification.md), all built.

## 1. Motivation

On 2026-10-31 a session asked the agent to fix one line in a test file. The
file sat in a worktree the session's edit tools may not write to (#76), so the
agent edited it with a `python3 - <<'PY'` heredoc. A command with side effects
needs approval, so it waited. The user asked why the checks were skipped.

None had been skipped: verification had never been due. Under #23 §5.1 only a
settled patch makes verification due. A file changed by an approved command (a
script, `sed -i`, `perl -i`, a formatter) was never checked, however much it
changed. #84's marker guard, which runs in `create_patch`, never sees such an
edit either.

## 2. Requirements

1. When an approved command in a Code-mode turn changes files, verification
   becomes due for those files: every path created, modified or deleted, as
   the command census of #16 sees it.
2. The turn that ran the command ends with that verification, once the turn
   has finished. A turn that pauses again keeps the run due, and the resume
   that finishes the turn runs it. If the turn proposes a patch, the patch
   makes verification due when it is settled, and that run covers the
   command's files too.
3. In a repair turn (#23 §5.5) a file-changing command is the repair. The next
   attempt runs instead of the attempt closing as `no_repair_applied`.
4. A command that changes nothing the census sees starts no verification. The
   census follows `git status`, so it doesn't see ignored files, such as a
   build's `target/`, or an empty directory. Nor does a command whose census
   could not be taken (not a git checkout, or over
   `checkpoint_census_max_paths`): its effects are unknown, and Damaian
   doesn't claim either way.
5. Verification stays Code-only (#23 §5.1).
6. The turn's own answer stays in front of the report in the turn's result,
   so the message that streamed it is not replaced by the report alone.

## 3. Non-goals

- Commands that run with no approval. The command policy only auto-runs
  read-only commands, so they don't change files.
- Commands the user runs from the terminal panel. Those are not the agent's.
- Moving shell edits through `create_patch`. A command's effects are known
  only after it has run, so #84's guard cannot refuse them beforehand.
- The check commands of a verification itself. They resume through the check
  route, not this one, so a test run cannot make its own verification due.

## 4. Design

- **`verification.rs`:**
  - `VerificationEvent::Due` gains `paths`, the files a command changed,
    which is empty and omitted for a patch.
  - `patch_id` is empty for a command, and `#[serde(default)]` keeps old logs
    readable.
  - `VerificationDue` carries `paths` too, and `from_command()` says which
    kind made the run due.
- **`checkpoint.rs`:**
  - `CommandCensus::changed_paths(after)` diffs two censuses. It returns
    `None` when either one is truncated.
  - `record_command_effects_between` takes the after census, so the
    checkpoint and verification read the working tree once.
    `record_command_effects` keeps its signature.
- **`chat.rs`:**
  - In `resume_after_command_decision_with_options`, after the approved
    command runs, `record_command_effects` returns the changed paths and
    `record_command_verification_due` appends the `Due`.
  - When the resumed turn has run, `verify_after_command_turn` checks
    whether the turn finished (no proposal, not cancelled, status
    `complete`) and a command's `Due` is pending. If so, it calls
    `verify_due` and puts the turn's answer in front of the response. If not,
    a repair turn closes through `close_verification_resume` as before.
  - `verification_changed_files` adds the paths of every `Due` in the
    session.
  - `verify_due_report` treats a command's `Due` task as the task that made
    the change, so a repair by command starts the next attempt.
- **Callers:** the shell, the worker (#80) and the CLI all resume through
  this one method. The result carries the report or the check card the way a
  verification turn's result always has, so none of them needed a change.
- **`app.js`:** the recovery card for a due verification no longer says "The
  patch was applied".

## 5. Acceptance

All in `tests/verification_loop.rs`:
- `an_approved_command_that_changes_a_file_is_verified_when_the_turn_ends`:
  `touch notes.txt` is approved, and the log holds a `Due` with
  `paths: ["notes.txt"]`. The resumed turn returns a report
  (`only_documentation_changed`) whose `files_changed` is `notes.txt`. Its
  response starts with the turn's own "Done.", and nothing is left due.
- `an_approved_command_that_changes_nothing_is_not_verified`: `mkdir -p
  empty` records no `Due` and returns no report.
- `an_unavailable_census_reports_no_changed_paths`.

## 6. Verification

Local checks, 2026-10-31:
- `scripts/quality-gate.sh fmt node lint typos deny`;
- `cargo clippy -p workspace-engine -p desktop-shell --all-targets --locked -- -D warnings`;
- `cargo nextest run -p workspace-engine --locked`;
- `cargo nextest run -p desktop-shell --locked`.

The shell test `a_command_approval_continues_in_process` now expects a second
task: its `touch approved.txt` changes a file, so the worker ends the resume
with the verification.

The workspace clippy and test suite are left to CI.

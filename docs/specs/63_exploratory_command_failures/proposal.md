# Feature Spec: Exploratory Command Failures Are Not Findings

Status: Done 2026-10-23; §5.1 amended 2026-10-24 (loops, `set`, `sed`, command
substitution — see §7). Tasks 1–6 of 6 done — see
[`tasks.md`](tasks.md). Specified on 2026-10-17 from the session behind
[#59](../59_exploration_round_allowance/proposal.md)–[#62](../62_approval_prompts_name_the_reason/proposal.md),
a reporting-only request that left two open **error** findings behind: an
unquoted glob that zsh refused, and an `ls -d` of a directory that did not
exist.
Order: 63 of 63
Plan: none. Evidence-driven, like #47 — an exception to the rule in
[`README.md`](../README.md).
Depends on: [#22](../22_findings_model_and_panel/proposal.md) (the finding model and
its generic fallback) — built; [#21](../21_task_plan_progress_and_budget/proposal.md)
(plan evidence and step status) — built. Everything else named below is a
cross-reference, not a prerequisite.
Related implementation specs:
[`23_verification_loop`](../23_verification_loop/proposal.md) (whose checks are
detected validations, and which this spec must not weaken),
[`58_header_repo_bar_and_menus`](../58_header_repo_bar_and_menus/proposal.md) (the
*Fix N errors* action that offered these two for repair),
[`62_approval_prompts_name_the_reason`](../62_approval_prompts_name_the_reason/proposal.md)
(also corrects what the model is told about the shell).

## 1. Motivation

A finding says "something in your code is wrong; here is where". Damaian turns
every failed agent command into one. When the command was a check — `cargo
test`, `npm run lint` — that is right, and the generic fallback that catches an
unparsed failure is, as #22 says, "the honest case". When the command was the
agent looking around, it is wrong:

- `cd .claude/worktrees/spec25-task8 && grep -rn 'ContextManager::new'
  --include=*.rs .` failed because zsh's `nomatch` option rejected the unquoted
  `*.rs` before `grep` ran. Finding: severity **error**, "zsh:1: no matches
  found: --include=*.rs".
- `ls -d target; du -sh target; ls -d ../../target && …` exited 1 because one
  directory did not exist. Finding: severity **error**, with the summary
  "…: target".

Neither says anything about the user's code. Both sat in the findings panel as
open errors, counted by the repo bar's *Fix 2 errors*, which would have sent
them to the model as things to repair. A `grep` that matches nothing exits 1 as
well, so every unsuccessful search becomes an error too.

The same misreading reaches the plan: a step whose evidence includes a non-zero
`CommandExit` is **Blocked**, so a `grep` that found nothing blocks the step that
was looking.

## 2. Current State

- `findings_from_execution` (`finding.rs:347`) runs the first parser that
  matches the command (Rust diagnostics, Rust tests, Biome). If none produced an
  error and the run failed — any non-zero exit or a timeout — it adds one
  `generic_draft` (`finding.rs:369`): source `Command`, severity `Error`, summary
  `"<command>: <first stderr line>"`.
- It is called from `record_command_findings` (`chat.rs:1527`) for both the
  auto-run path and the resume after an approved command. Nothing about the
  command's purpose is passed in: `CommandExecution` has no reason or kind.
- `status_from_evidence` (`plan.rs:158`) blocks a step on any
  `Evidence::CommandExit` whose exit code is not `Some(0)`.
- The classifier knows more than it keeps. `is_low_risk_read_only` and
  `is_validation_command` (`command_policy.rs`) decide risk and are then
  forgotten. `propose_detected_validations` (`validation.rs:273`) marks real
  checks only through its reason string.
- The repo bar's *Fix N errors* (`app.js:1276`) and the repair request
  (`finding/repair.rs`) take every open finding of that severity, whatever its
  source.
- Agent commands run as `<shell> -lc <command>` (`command_runner.rs:248`), and
  the shell defaults to `$SHELL`, which on macOS is zsh. Nothing tells the model
  that, and nothing handles zsh's `nomatch`.
- #22 never distinguished the two kinds of command. Its own Task 8 recorded an
  `ls no-such-directory` as a generic finding without remark
  (`22_findings_model_and_panel/proposal.md`, §7).

## 3. Requirements

1. Every agent command run has a **purpose**, decided when it is proposed and
   carried to where findings and evidence are recorded: *check*, *exploration*,
   or *other*.
2. A failed **exploration** command produces no finding. Its output still
   reaches the model as the tool result, unchanged.
3. A failed **check** produces findings exactly as today, parsers first and the
   generic fallback after.
4. A failed **other** command produces findings as today. Only commands known to
   be exploration change behaviour; an unrecognised command keeps today's
   treatment.
5. A non-zero exit from an exploration command is recorded as plan evidence but
   does not block the step.
6. Purpose never influences approval, risk or blocking. It is a description for
   findings and evidence, not a policy input, and it is computed after the
   policy has decided.
7. The model is told which shell runs its commands and that unquoted glob
   patterns fail in zsh when nothing matches.

## 4. Non-goals

- Changing the parsers, the finding model, or how check findings repair.
- Removing findings already recorded. Existing open findings from exploratory
  commands can be dismissed in the panel; no migration rewrites the log, which is
  append-only (#17).
- Changing the user's shell or its options. Setting `NO_NOMATCH` for agent
  commands would change what a command means relative to the same command typed
  by the user, and `shell` is the user's setting.
- Deciding approval from purpose. That is [#64](../64_read_only_command_boundary.md)'s
  subject, and it uses its own, stricter rules.

## 5. Design

### 5.1 Deciding the purpose

`CommandPurpose { Check, Exploration, Other }`, defined with its rules in
`command_purpose.rs` and computed by `proposal_from_classification`
(`validation.rs`) from the classification's command, after the policy has
decided. It is stored on `CommandProposal` — as an optional trailing `PURPOSE`
field of the stored proposal, so a proposal written before this spec reads as
`Other` — and copied onto `CommandExecution` through
`CommandRunOptions::purpose`, which `run_proposal` fills from the proposal. The
runner carries it and never computes or reads it. Every agent command goes
through `propose_command_at` and `run_proposal` (the auto-run path and the
resume after approval), so the proposal is the one place it is decided.
`command_policy.rs` is touched only to make `is_validation_command` visible to
the crate.

- **Check** when any of: the proposal came from `propose_detected_validations`
  (marked by a field, not by its reason string); `is_validation_command` matches;
  or a registered finding parser `matches` the command. The parser test means
  `cargo clippy` counts as a check even though it is not on the validation list,
  because its output is something #22 already knows how to read.
- **Exploration** when it is not a check and every simple command in it — split
  on `&&`, `||`, `;`, `|` and newlines, with a leading `cd <dir>` ignored —
  starts with a program from a closed list: `ls`, `pwd`, `cat`, `head`, `tail`,
  `wc`, `grep`, `rg`, `find`, `sed` (any form without `-i` or `--in-place`),
  `awk`, `sort`, `uniq`, `cut`, `du`, `stat`, `file`, `echo`, `true`, `read`,
  and `git` with `status`, `diff`, `log`, `show`, `rev-list`, `rev-parse`,
  `branch --list`, `worktree list`, `ls-files`, `grep`, optionally after
  `-C <dir>`. Programs are matched by their exact first word, so `./grep` and
  `FOO=1 grep` are `Other`. The split respects single and double quotes and
  backslashes, and does not treat the `&` of `2>&1` as a separator.

  Shell syntax that runs nothing by itself is looked through (amended
  2026-10-24, §7): a `set` of options only (`set -e`, `set +e`,
  `set -euo pipefail`; not `set --`); a `for` header, whose word list runs
  nothing; and the `do` and `done` keywords. `while` and `until` are judged by
  their condition, which is a command. So a loop is exploration exactly when
  every command it runs would be on its own: `for f in …; do sed -n … "$f";
  done` is, `for f in …; do rm "$f"; done` is not. A command that runs nothing
  from the list — only `cd`, only `set` — is not exploration.

  Any command substitution, `$(…)` or a backtick outside single quotes, makes
  the whole command `Other`, because it runs a command the split cannot see:
  `for f in $(cargo test …)` and `echo $(rm x)` alike.
- **Other** otherwise.

The list is generous on purpose, because requirement 6 makes it harmless: it
decides only whether a failure is reported as a finding. A command that is on it
and has side effects — `find -delete`, `awk` with `system()` — still went
through the policy's approval first, and a missed finding for it is a lost hint,
not a lost guard.

The split is a lexical split for description only. It is not the parser
[#64](../64_read_only_command_boundary.md) needs for a security decision, and must
not be reused as one.

### 5.2 Findings

`findings_from_execution` reads the purpose from `CommandExecution::purpose`,
which Task 1 already put there, so its signature and `record_command_findings`
are unchanged and both call paths (the auto-run and the resume after approval)
get the rule. For `Exploration` it returns no findings, before any parser runs,
whatever the exit or termination. For `Check` and `Other` it is unchanged.

### 5.3 Plan evidence

`Evidence::CommandExit` gains `purpose`, with `#[serde(default)]` reading old
events as `Other`, which keeps their recorded behaviour. `status_from_evidence`
blocks on a failed exit only when the purpose is not `Exploration`. The exit
is still recorded as the step's evidence.

The evidence is minted by `evidence_for` (`chat.rs`) from
`ActionOutcome::CommandExit`, so the outcome carries the execution's purpose
beside its exit code. `None` (killed, timed out, cancelled) from exploration
does not block either: it is still not a pass, but a look-around that learned
nothing says nothing about the step's own work. For `Check` and `Other`, `None`
blocks as before.

### 5.4 Telling the model about the shell

The `run_command` description and the command envelope paragraphs name the
configured shell — "commands run in `zsh -lc`" — and, when it is zsh, add:
"quote glob patterns such as `'*.rs'`; zsh fails a command whose unquoted glob
matches nothing." The shell's name is the basename of `config.shell`, which is
the user's own setting and safe to put in the prompt. Its full path is not
needed.

### 5.5 Recording

The audit record for a command execution gains `purpose`. The eval harness
reports findings per run grouped by purpose, so a regression that starts filing
exploration failures again shows as a non-zero count where there should be none.

As built (Task 5):

- Both execution audit events carry `purpose` (`check`, `exploration`,
  `other`): `command_executed` (`command_runner.rs`) and
  `stored_command_executed` (`validation.rs`). The stored execution summary
  (`summary.dcmd`) ends with a `PURPOSE` field after `STDERR_REF`, the way the
  proposal's does. Nothing in the tree reads summaries back, so a summary
  written before this spec differs only by lacking it.
- The eval harness had no findings at all (correction C8). Each run record
  now carries `findingsByPurpose`, a `{commands, findings}` count for each
  purpose. It is built by `trace::findings_by_purpose`, which joins the
  audit trace's `stored_command_executed` (`commandId`, `purpose`) with the
  session log's `finding_recorded` (`originRef`). Every agent command runs
  through a stored proposal, so the join covers them all. A finding whose
  origin is not a command, such as a browser diagnostic, has no purpose and
  is not counted.
- The command count sits beside the finding count, so a zero is evidence
  only when something explored. The new metric `exploration_findings`
  ("Findings from exploration commands") sums the exploration findings. It
  reports no data when no run executed an exploration command, as the other
  asserted-zero counts do. The text report gains a "Findings by command
  purpose" section that lists `findings/commands` per purpose for each run
  that executed a command.
- No new scenario was needed. `failed_validation_retry` already fails `ls
  no-such-directory` eight times. It now asserts `exploration_findings = 0`.
  That assertion fails when the run executed no exploration command, and it
  is `deterministic_only`, because a live model may never run one.

### 5.6 Documentation

On Done: `docs/USER_GUIDE.md` (findings: which commands produce them), and
`docs/TROUBLESHOOTING.md` (a failed command with no finding, and why).

## 6. Acceptance Criteria

- The two commands from §1, run by the agent and failing as they did, record no
  finding, and their output still reaches the model.
- `grep -rn nothing-matches .` exiting 1 records no finding and does not block
  its plan step.
- `cargo test` and `npm run lint` failing record findings exactly as before,
  through their parsers or the generic fallback.
- `cargo clippy` failing is a check and records findings.
- `python3 script.py` failing is `Other` and records a generic finding as today.
- A detected validation from `propose_detected_validations` is a check, whatever
  its command text.
- Approval, risk and blocking are identical for every command in the existing
  policy tests.
- An old `CommandExit` event without `purpose` replays as `Other`, and a plan
  replayed from a pre-spec log has the same step statuses as before.
- With `shell` set to zsh, the model-facing command description names zsh and
  the glob-quoting rule; with bash it names bash and omits the rule.
- #23's checks still record and repair findings unchanged.
- *(Amendment 2026-10-24.)* The two loops from the findings panel in §7's
  amendment note are exploration; a loop whose body runs anything off the list
  is `Other`, or `Check` when it runs a check; a command substitution anywhere
  is `Other`.

## 7. Implementation Notes

The execution order and per-task record are in [`tasks.md`](tasks.md), which
also lists the corrections found while planning (C1–C8). The ones that change
the design above have been folded into §5.1.

- **Task 1 (2026-10-17).** `CommandPurpose` and `command_purpose` are in
  `command_purpose.rs`, not `validation.rs`: #64 rewrites `command_policy.rs` at
  the same time, and a separate module keeps the description away from the
  policy code. Every command `detect_in` produces today is on the validation
  list, so the `detected` flag changes no current outcome; it is kept so a
  future detector cannot fall through to `Other`.
- **Task 2 (2026-10-17).** `findings_from_execution` reads the purpose from the
  execution, so its signature did not change (§5.2). Tests that used a failing
  `ls` to mean "a failed command" now use a repository script, which is `Other`.
- **Task 3 (2026-10-17).** The purpose reaches plan evidence through
  `ActionOutcome::CommandExit`, not only `Evidence::CommandExit` (C5). An
  exploration exit of `None` does not block (§5.3). The purpose is always
  written, as `"purpose":"exploration"` and so on, and an event without it
  reads as `Other`. The shell's plan panel still describes such a step's
  evidence as "a command ran and exited 1"; it does not read the purpose.
- **Task 4 (2026-10-18).** The shell name is threaded into
  `run_command_tool_definition`, both `command_envelope_paragraph_*`
  functions, `mode_system_prompt`, `system_prompt` and
  `system_prompt_for_session` as `&self.config.shell`, and `shell_basename`
  reduces it to its basename before it reaches any model-facing text. The
  exact wording is "Commands run in {name} -lc." plus, for zsh, " Quote glob
  patterns such as '*.rs'; zsh fails a command whose unquoted glob matches
  nothing." The three byte-identical pins in `chat.rs` moved to the zsh
  wording. Because the prompt is now longer, the token-ceiling test
  `a_turn_stops_before_the_call_that_would_cross_the_ceiling` crosses 1000
  estimated tokens one call earlier (3 → 2), and `tests/token_ceiling.rs`
  pins `shell: "/bin/zsh"` so the count no longer depends on `$SHELL`. #62
  was still "Not started", so its wording was not a base.
- **Amendment (2026-10-24): loops, `set` and `sed`.** A reporting session on
  2026-10-24 left two open **error** findings that were its own failed
  look-arounds: `for w in …; do echo "=== $w"; sed -n '1,6p' "…"*/proposal.md
  2>/dev/null; done`, which zsh rejected with "no matches found", and
  `set +e; for f in …; do …; done`. Both were `Other`, because `for`, `do`,
  `done` and `set` were not on the list, so both were filed as findings and
  offered for repair. §5.1 now looks through loop syntax and option-only `set`,
  judges a loop by the commands it runs, makes any command substitution
  `Other` (which also closes `echo $(rm x)` being exploration), accepts every
  `sed` that does not edit in place, and adds `read` for `| while read f`. The
  rules and their counter-cases are pinned in `command_purpose.rs`'s unit
  tests. Findings already recorded are unchanged (§4); they can be dismissed.

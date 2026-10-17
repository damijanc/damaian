# Feature Spec: Approval Prompts Name the Reason

Status: Not started. Specified on 2026-10-17 from the session behind
[#59](59_exploration_round_allowance/proposal.md)–[#61](61_continue_the_stopped_turn.md),
in which the user answered 40 approval prompts that all gave the same reason,
"because it cannot run in sandbox mode", and none gave the real one.
Order: 62 of 62
Plan: none. Evidence-driven, like #47 — an exception to the rule in
[`README.md`](README.md).
Depends on: [#10](10_persistent_command_approval.md) (the approval card and
*Allow Always*) — built; [#31](31_permission_profiles/proposal.md) (the
plain-word rule names in `effective_policy.rs`) — built. Everything else named
below is a cross-reference, not a prerequisite.
Related implementation specs:
[`20_working_modes`](20_working_modes/proposal.md) (mode refusals, which already
name their reason), [`64_read_only_command_boundary.md`](64_read_only_command_boundary.md)
(adds reasons of its own to the classifier this spec makes structured).

## 1. Motivation

Every command that needs approval is introduced the same way:

> I need your approval before running `cd .claude/worktrees/spec25-task8 && git
> --no-pager diff -- … | head -220` because it cannot run in sandbox mode.

There is no sandbox mode. A command that needs no approval runs directly, on the
user's machine, in their login shell (`chat.rs`, the auto-run path passes the
literal label `"sandbox"`). The sentence is wrong twice: it names a mechanism
that does not exist, and it hides the reason the policy actually had, which the
classifier knew and recorded. For that command the reason was that it uses `&&`
and `|`, and Damaian never runs shell control without asking.

A user deciding forty times in fifteen minutes needs the reason in the first
line, because the reason is what tells them whether a prompt is routine or
worth reading. "Uses `&&` and `|`" on a `git diff` is routine; "may use the
network" on a `curl` is not. With one sentence for every prompt, every prompt
looks routine, which is how approval fatigue turns into approving without
reading.

The word "sandbox" also misleads in a way that matters for security. A user who
believes auto-run commands are sandboxed will trust them more than they should.

## 2. Current State

- `command_proposal_response` (`chat.rs:5105`) returns one of two fixed
  sentences, both claiming "sandbox mode", whatever the proposal says.
- `CommandProposal` (`validation.rs:17`) carries `risk`, `requires_approval`,
  `blocked`, `expected_effects`, `may_use_network` and `reasons: Vec<String>` —
  one fixed English string per classifier rule that fired. It records neither the
  rule as a value nor which setting turned a low-risk command into one that needs
  approval.
- The classifier (`command_policy.rs`, `classify_pattern`) checks, in order: the
  blocklist and destructive patterns; shell control (`;`, `&`, `|`, a backtick,
  `<`, `>`, a newline, `$(`); the allowlist; `is_low_risk_read_only`; validation
  commands; Docker; high-risk keywords; and anything else as unknown.
  `classify_at` then raises the risk for a working directory or path argument
  outside the repository, and applies the permission profile's `command_access`.
- `require_approval_for_all_commands` and `require_approval_for_risky_commands`
  set `requires_approval` without adding a reason. A low-risk command that needs
  approval only because of them still reads "Read-only command".
- The approval card (`createCommandApprovalPreview`, `app.js:9170`) shows the
  command and a risk badge, and puts the plain-text `command_approval_prompt`,
  which includes the reasons, behind a **Why this command** disclosure. The
  model's own stated reason is not shown.
- `effective_policy.rs`'s `rule_label` names configuration keys in plain words
  ("Ask before every command", "Commands that are always blocked"). It does not
  name classifier branches.
- The tool description for `run_command` and the system prompt tell the model
  that Damaian "runs sandbox-safe read-only commands automatically"
  (`chat.rs:4589`, and the command envelope paragraphs).

## 3. Requirements

1. The classifier returns **why** a command needs approval or is blocked as a
   value, not only as display strings: one or more causes from a closed set.
2. Every approval prompt states the deciding cause in its first line, in plain
   words, naming the concrete construct where there is one (which shell-control
   characters, which network keyword, which path is outside the repository,
   which setting).
3. When approval is required by a setting rather than by the command, the prompt
   names the setting by its `rule_label` and where it came from (user,
   repository, profile), so the user can see why a read-only command asked.
4. The word "sandbox" no longer appears in anything shown to the user or the
   model about local commands, because there is no sandbox. Commands that run
   without approval are described as what they are: commands the policy runs
   without asking.
5. The card shows the model's stated reason for the command beside the policy's
   reason, so the user sees both what the agent wants and why Damaian is asking.
6. The causes are recorded in the audit record of the proposal, so a session can
   be analysed for which causes produce the most prompts.

## 4. Non-goals

- Changing which commands need approval. This spec explains the decision; it
  does not move it. [#64](64_read_only_command_boundary.md) changes the
  boundary.
- Adding a real sandbox.
- Approving several prompts at once, or session-scoped grants. Both are worth
  considering once the reasons are visible and measured (§5.5); neither is
  specified here.
- Changing *Allow Always*'s exact-match rule (#10).

## 5. Design

### 5.1 Causes

A new enum in `command_policy.rs`:

| `ApprovalCause` | Plain words (first line of the prompt) |
|---|---|
| `Blocklisted { rule }` | "Blocked: matches *Commands that are always blocked* (`<rule>`)" |
| `Destructive { pattern }` | "Blocked: `<pattern>` can destroy work" |
| `ShellControl { constructs }` | "Asks first: uses `&&`, `|` — Damaian never runs shell control without asking" |
| `Docker` | "Asks first: Docker commands always ask" |
| `HighRisk { keyword }` | "Asks first: `<keyword>` changes your system or repository" |
| `Network { keyword }` | "Asks first: may use the network (`<keyword>`)" |
| `Unknown { program }` | "Asks first: Damaian does not know what `<program>` does" |
| `OutsideRepository { path }` | "Asks first: refers to `<path>`, outside the repository" |
| `Setting { key, source }` | "Asks first: *<rule_label(key)>* is on (set by <source>)" |
| `ProfileBlocked { access }` | "Blocked: the permission profile allows <access> commands only" |

`CommandClassification` gains `causes: Vec<ApprovalCause>`, ordered most
decisive first. The existing `reasons` strings are rendered from the causes, so
there is one source and the strings cannot drift. Blocking causes sort before
approval causes; among approval causes, the classifier's own order applies.

`Setting` is the new information: `classify_at` adds it wherever a setting, not
the command, raised `requires_approval`. Its `source` comes from the same
attribution `effective_policy.rs` already computes for the policy view (#31).

### 5.2 The message and the card

`command_proposal_response` becomes, for example:

> Damaian asks before running this: it uses `&&` and `|`, and shell control
> always asks.
> `cd .claude/worktrees/spec25-task8 && git --no-pager diff -- … | head -220`

The card's header line shows the first cause's plain words next to the risk
badge, outside any disclosure. The model's `reason` follows as "The agent says:
…". **Why this command** keeps the full prompt, now including every cause.

A blocked command says what blocked it and that nothing will run, and offers no
approval, as today.

### 5.3 Model-facing wording

The `run_command` description and the command envelope paragraphs replace
"sandbox-safe" with what is true: Damaian runs a short list of read-only commands
without asking and asks the user for everything else, including any command that
uses shell control. Naming shell control there is deliberate: a model that knows
`a && b | c` always asks may prefer two tool calls, or #60's tools, over one
compound command.

The test that asserts the read-only prompt omits the auto-run sentence
(`chat.rs`, around line 6178) is updated with the new wording, not removed.

### 5.4 Recording

The audit record for a command proposal gains `causes`, the enum variant names
in order (`shell_control,unknown`). The session log's assistant message carries
the plain words already, so no new event is needed.

### 5.5 Measuring

The eval harness reports, per run, prompts grouped by first cause. The motivating
session, re-read under this spec, would show its 40 prompts as nearly all
`shell_control`. That is the evidence a future grouped-approval or
session-grant design would need, and the reason those are not designed here.

### 5.6 Documentation

On Done: `docs/USER_GUIDE.md` (the approval card, and what runs without asking —
without the word "sandbox"), `docs/TROUBLESHOOTING.md` (reading an approval's
cause), and `SECURITY.md` if it describes auto-run commands as sandboxed.

## 6. Acceptance Criteria

- `cd x && git diff | head` produces a prompt whose first line names `&&` and
  `|` as shell control.
- `curl https://example.com` names the network and the keyword; `docker ps`
  names Docker; `python3 script.py` names `python3` as unknown.
- `git status` with `require_approval_for_all_commands` on names *Ask before
  every command* and whether the user, the repository or a profile set it.
- A blocklisted command names its rule and offers no approval.
- No string shown to the user or sent to the model about local commands
  contains "sandbox"; a test enforces this over the prompt builders and
  `command_proposal_response`.
- The card shows the first cause and the model's reason without opening a
  disclosure.
- The proposal's audit record lists its causes in order.
- Classification results — risk, `requires_approval`, `blocked` — are unchanged
  for every command in the existing policy tests.

## 7. Implementation Notes

None yet.

# Feature Spec: Approval Prompts Name the Reason

Status: Done 2026-10-23 (started 2026-10-18; Tasks 1–7, the last on
2026-10-23). Tasks 1 (`ApprovalCause` and structured causes) and 2
(`Setting` and profile-block causes) landed 2026-10-18; Task 3 (prompt and
model-facing wording) landed 2026-10-23; Task 4 (the card names the cause and
the model's reason) landed 2026-10-23; Task 5 (recording causes in the proposal
and audit record) landed 2026-10-23; Task 6 (the eval harness groups prompts by
first cause) landed 2026-10-23; Task 7 (docs, live re-run and close-out)
landed 2026-10-23, and its live re-run named every prompt's cause (§6).
Specified on 2026-10-17
from the session behind
[#59](../59_exploration_round_allowance/proposal.md)–[#61](../61_continue_the_stopped_turn/proposal.md),
in which the user answered 40 approval prompts that all gave the same reason,
"because it cannot run in sandbox mode", and none gave the real one.
Order: 62 of 62
Plan: none. Evidence-driven, like #47 — an exception to the rule in
[`README.md`](../README.md).
Depends on: [#10](../10_persistent_command_approval.md) (the approval card and
*Allow Always*) — built; [#31](../31_permission_profiles/proposal.md) (the
plain-word rule names in `effective_policy.rs`) — built. Everything else named
below is a cross-reference, not a prerequisite.
Related implementation specs:
[`20_working_modes`](../20_working_modes/proposal.md) (mode refusals, which already
name their reason), [`64_read_only_command_boundary.md`](../64_read_only_command_boundary.md)
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

> Written before spec 64 merged on 2026-10-17/18. Spec 64 rewrote the read-only
> boundary and its classifier, and its reason strings are what §5.1's
> `ApprovalCause` replaces. The `tasks.md` Corrections table records where the
> code now differs from this section; the bullets below are the corrected view.
> Task 3 has since rewritten the wording and removed the strings named here;
> Task 4 has since shown the first cause and the model's reason on the card.
> The line references point at the current source for the pre-Task-3 state, and
> the card bullet below describes the pre-Task-4 card.

- `command_proposal_response` (`chat.rs:6249`) returns one of two fixed
  sentences, both claiming "sandbox mode", whatever the proposal says.
- `CommandProposal` (`validation.rs:18`) carries the model's stated `reason`
  (set from the tool call in `propose_command_at`), plus `risk`,
  `requires_approval`, `blocked`, `expected_effects`, `may_use_network` and
  `reasons: Vec<String>` — one fixed English string per classifier branch or
  read-only refusal that fired. It records neither the rule as a value nor which
  setting turned a low-risk command into one that needs approval. The model's
  `reason` is stored but never reaches the card.
- The classifier (`command_policy.rs`): `classify_pattern` first runs
  `read_only_verdict` (spec 64's tokenizer — `shell_line`, `command_verdict`,
  `pipeline_verdict`), then `classify_text` checks, in order: the blocklist and
  destructive patterns; shell control (an admitted read-only pipeline passes
  through); the allowlist; the read-only verdict (`Admitted` / `Refused` /
  `NotReadOnly`); validation commands; Docker; high-risk keywords; and anything
  else as unknown. `classify_at` then escalates for a working directory or path
  argument outside the repository, a restricted path (`RESTRICTED_PATH_REASON`,
  whose doc comment names this spec), a `cd` target that is not a directory,
  `ls -R` of the root, and applies the permission profile's `command_access`
  (which blocks with "Blocked by permission profile: command_access=…").
- There is no separate network branch: `curl`/`wget` fall into the high-risk
  bucket with the generic "may modify dependencies, Git state, permissions,
  network, or shell state" reason, and `may_use_network` is a boolean kept
  beside the reasons rather than a cause. §5.1's `Network { keyword }` must be
  split out and the keyword named.
- `require_approval_for_all_commands` and `require_approval_for_risky_commands`
  set `requires_approval` without adding a reason. A low-risk command that needs
  approval only because of them still reads "Read-only command".
- The approval card (`createCommandApprovalPreview`, `app.js:9325`) shows the
  command and a risk badge, and puts the plain-text `command_approval_prompt`,
  which includes the reasons, behind a **Why this command** disclosure. The
  model's own stated reason is not shown; `AgentCommandProposal` (`chat.rs:314`)
  carries no `reason` field to show.
- `effective_policy.rs`'s `rule_label` names configuration keys in plain words
  ("Ask before every command", "Commands that are always blocked"). It does not
  name classifier branches. The source attribution it computes (`Config::load_scoped`'s
  `RepositoryConfigReport` → `EffectivePolicy`) is **not** available where
  `classify_at` runs: `CommandPolicy` is built from `Config` alone
  (`workspace_engine.rs:78`), so §5.1's `Setting.source` needs that attribution
  threaded in.
- The tool description for `run_command` (`chat.rs:5641`) and the system prompt
  and command envelope paragraphs (`chat.rs:5099`, `5110`) tell the model that
  Damaian "runs sandbox-safe read-only commands automatically". Three more
  "sandbox" strings are not in those paragraphs: the auto-run path passes the
  literal `"sandbox"` as `approved_by` (`chat.rs:3140`), the model-facing
  context separator is `--- sandbox_command_result ---` (`chat.rs:5159`), and
  the audit turn status includes `complete_with_sandbox_command` (now
  `complete_with_command`, `chat.rs:3825`).

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
  does not move it. [#64](../64_read_only_command_boundary.md) changes the
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
| `RestrictedPath { path }` | "Asks first: reads `<path>`, which the policy restricts" |
| `OutsideRepository { path }` | "Asks first: refers to `<path>`, outside the repository" |
| `Setting { key, source: Option<String> }` | "Asks first: *<rule_label(key)>* is on (set by <source>)", omitting " (set by …)" when the source is unknown |
| `ProfileBlocked { access }` | "Blocked: the permission profile allows <access> commands only" |

`RestrictedPath` is added for the reason spec 64 already emits
(`RESTRICTED_PATH_REASON`, whose doc comment names this spec); the spec-64
read-only form refusals ("not an admitted read-only command", "uses shell
expansion", "not an admitted filter") map to `Unknown` (or a dedicated variant
if the task finds the distinction worth naming), and the `Network`/`HighRisk`
split requires pulling the network keywords out of `is_high_risk_command`.
Blocking causes sort before approval causes; among approval causes, the
classifier's own order applies.

`CommandClassification` gains `causes: Vec<ApprovalCause>`, ordered most
decisive first. The existing `reasons` strings are rendered from the causes, so
there is one source and the strings cannot drift. Blocking causes sort before
approval causes; among approval causes, the classifier's own order applies.

`Setting` is the new information: the classifier adds it wherever a setting, not
the command, raised `requires_approval` (`require_approval_for_all_commands`,
`require_approval_for_risky_commands`). Its `source` reuses the attribution
`effective_policy.rs` computes for the policy view (#31), but that attribution
is not available where `classify_at` runs today (`CommandPolicy` holds `Config`
only) — the task threads the `RepositoryConfigReport` (or a derived source map)
into the classifier or the proposal path rather than resolving it a second time.
`source` is `None` (unknown) when no report was threaded, and the plain words
then omit " (set by …)": reporting "default" there would misattribute a key the
user turned on themselves. With a report, a key no scope set is genuinely the
default and may say so.

### 5.2 The message and the card

`command_proposal_response` becomes, for example:

> Asks first: uses `&&`, `|` — Damaian never runs shell control without asking
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
uses shell control. Plan and Review carry the same sentence but say that
everything else is refused outright in that mode, because those modes raise no
approval card. Naming shell control there is deliberate: a model that knows
`a && b | c` always asks may prefer two tool calls, or #60's tools, over one
compound command.

The test that asserts the read-only prompt omits the auto-run sentence is
`ask_mode_system_prompt_has_no_command_envelope` (`chat.rs:7421`), not "around
line 6178". It is updated with the new wording, not removed, and
`code_mode_system_prompt_is_byte_identical_to_todays` (`chat.rs:7413`) pins the
Code system prompt against a `TODAYS_CODE_SYSTEM_PROMPT` constant — that constant
and the `CODE_PARAGRAPH_BEFORE_SPEC_50` / `READ_ONLY_PARAGRAPH_BEFORE_SPEC_50`
fixtures (`chat.rs:7775`, `7776`) change in the same commit or the byte-identical
test breaks.

### 5.4 Recording

The audit record for a command proposal — the `command_proposal_stored` event
written in `validation.rs`'s `store_proposal`, which today carries
`risk`/`requiresApproval`/`blocked` only — gains `causes`, the enum variant
names in order (`unknown,outside_repository` for a command the policy does not
recognise that also names a path outside the repository). Because the stored
proposal (`serialize_proposal`) round-trips to disk and is reloaded for reattach
and *Allow Always*, `CommandProposal` gains a `causes` field written and read
with it, so the audit value and the reloaded proposal cannot drift. The field
stores the causes themselves, not only their names: the audit names a cause,
and the proposal keeps enough to render it. An older proposal without the field
reads with no causes. The session log's assistant message carries the plain
words already, so no new event is needed.

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

### Live re-run (2026-10-23)

The motivating approval was re-run against `deepseek-v4-flash` with native tools
on, in-process against this worktree, under a `DAMAIAN_DATA_DIR` inside the
worktree. Four turns each asked for one command. No command was approved, so
nothing ran and the working tree stayed clean. The first line of each prompt:

| Command proposed | First line shown | Names the real cause? |
|---|---|---|
| `ls crates; ls docs` | Asks first: uses `;` — Damaian never runs shell control without asking | Yes. This is the spec 59 case: a `;`-chained read-only `ls` sequence, named as `;` shell control. |
| `curl -sS https://example.com` | Asks first: may use the network (`curl`) | Yes: the network keyword, not the generic high-risk sentence. |
| `python3 -c 'print(1)'` | Asks first: Damaian does not know what `python3` does | Yes: the program is named. |
| `docker ps` | Asks first: Docker commands always ask | Yes: the Docker cause. |

Each card payload carried the same first cause and the model's own stated
reason (`checking the layout`, `checking the page`, `checking Python`,
`checking containers`), so the card's two lines match §5.2. The run used a
temporary in-process `#[ignore]` driver because the CLI cannot observe a pending
approval without a UI; the driver and its data directory were deleted
afterwards. Criteria 3, 4, 5, 6, 7 and 8 are covered by the tests recorded in
[`tasks.md`](tasks.md); this live run exercises criteria 1 and 2.

## 7. Implementation Notes

The per-task record — including each deliberate deviation, the falsification
behind every load-bearing test, and the scoped check results — is in
[`tasks.md`](tasks.md). The live re-run is in §6 above.

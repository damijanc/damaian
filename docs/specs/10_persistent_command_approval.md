# Feature Spec: Persistent Command Approval

Status: Done
Order: 10 of 10
Related spec sections: `ai_coding_assistant_specification.md` §7.4 (command
approval), §7.10 (policy configuration).

## 1. Motivation

The command approval card offers exactly two actions: `Approve Run` and
`Reject`. Both are one-shot. A user who runs the same project command twenty
times in a session approves it twenty times, and there is no way to say "this
one is fine, stop asking."

The escape hatch exists but is unreachable from where the decision is made.
`command_allowlist` in `Config` (`crates/workspace-engine/src/config.rs`) already
suppresses approval for commands that match it exactly
(`configured_exact_matches` in `crates/workspace-engine/src/command_policy.rs`).
Reaching it means leaving the conversation, opening a config file by hand, and
knowing both the key name and its pipe-separated format. In practice nobody
does this, so the prompt fatigue stands — and prompt fatigue is a safety
problem, not just an annoyance: a user who reflexively clicks `Approve Run`
twenty times is not reading the twenty-first.

## 2. Requirements

1. The approval card offers a third action that runs the command *and* records
   it, so the same command stops prompting.
2. The recorded allowance is scoped to the repository the proposal was raised
   against. An allowance granted in one project must not affect another.
3. Only the exact command is allowed. Allowing `npm run test:unit` must not
   allow `npm run test:unit --watch`, and allowing `git push` must not allow
   `git push --force`.
4. The action is offered only where it can actually work. If the policy would
   ignore the resulting allowlist entry, the option must not appear.
5. Granting the allowance and running the command is one user action and one
   round trip.
6. If the allowance cannot be recorded, the command does not run. Silently
   downgrading to a one-time approval would leave the user believing they had
   granted something they had not.
7. Recording an allowance must not revoke an existing one.
8. The CLI reaches the same behaviour as the desktop app.
9. The decision is auditable.

## 3. Non-goals

- **Prefix or glob allowances.** `npm *` would let one decision authorise
  commands the user never saw. Exact match only; requirement 3.
- **A "deny always" counterpart.** `command_blocklist` already exists and is
  prefix-matched. Nothing observed suggests users want to build it up
  incrementally from the prompt.
- **Editing arbitrary repository config from the UI.** This spec adds exactly
  one key to what the desktop app may write into `.damaian/config.conf`. The
  general restriction in `desktop_settings_config_path` stands.
- **Undo from the UI.** Removing an entry means editing
  `.damaian/config.conf`, same as before.

## 4. Design

### 4.1 Eligibility

`allow_always_eligible(config, command, blocked)` in `command_policy.rs` is the
single authority, consulted by every surface so the UI and the CLI cannot
disagree. It refuses in three cases, each for a distinct reason:

- **Blocked commands.** A block is a policy decision, not an approval
  decision. There is no approval that makes `rm -rf /` run, so offering to
  make it permanent is incoherent.
- **Commands containing shell control syntax** (`;`, `&`, `|`, backtick, `<`,
  `>`, newline, `$(`). `classify_pattern` checks `contains_shell_control`
  *before* the allowlist, so an entry for one of these could never match — the
  button would appear to work and silently change nothing. Independently,
  `command_allowlist` is serialised pipe-separated on disk, so a piped command
  cannot round-trip through the config file at all.
- **`require_approval_for_all_commands = true`.** The setting means "prompt me
  for everything," and an allowlist entry cannot satisfy it. Offering the
  option here would promise something the policy then refuses to honor.

High-risk commands (`git push`, `curl`, `npm install`) *are* eligible. The user
is looking at the command, its risk, and its expected effects when they choose;
this is an informed opt-in, and excluding exactly the commands that prompt most
often would leave the motivating problem unsolved.

### 4.2 Persistence

`ValidationOrchestrator::allow_command_always(proposal_id, approved_by)` loads
the proposal, checks eligibility, and appends the trimmed command to
`command_allowlist` in `<repo>/.damaian/config.conf` — resolved from the
proposal's own `working_directory`, which satisfies requirement 2. The file is
gitignored, so an allowlist never reaches the shared repository.

Two details carry the correctness of this step:

**Seeding.** `Config::apply_overlay` *replaces* `command_allowlist` rather than
merging it. A repository entry naming only the newly allowed command would
therefore silently revoke every command allowed at user scope. The write seeds
the repository entry from the already-merged effective list the first time the
repository takes ownership of the key; afterwards the repository entry is the
authority and is appended to directly. Requirement 7.

**Config source.** Eligibility is answered against the config the orchestrator
was constructed with, not a fresh load from disk, so the answer matches the
policy that classified the proposal and the method stays a pure function of its
injected dependencies. Callers construct the orchestrator for the proposal's
repository; the CLI, which is handed only a proposal id, looks up the working
directory via `load_proposal` and re-scopes before calling.

Repeat grants are idempotent, and the operation records a `command_allowlisted`
audit event carrying the command, the actor, and whether it was already
allowed. Requirement 9.

### 4.3 Surfaces

`AgentCommandProposal` carries `allow_always`, serialised to clients as
`allowAlways`. It is always `false` for MCP tool proposals, which reuse the
same struct but are not shell commands — per-server `require_approval` is the
knob for those.

The desktop card renders `Allow Always` between `Approve Run` and `Reject`,
only when `allowAlways` is set. `/api/run-command` and
`/api/resume-command-stream` accept `always=true`, which records the allowance
*before* running or resuming and fails the whole request if the write fails
(requirements 5 and 6). The CLI equivalent is
`damaian run-command <id> --approve --always`; `--always` without `--approve`
is an error, since rejecting cannot grant anything.

## 5. Acceptance criteria

1. Approving `chmod 644 README.md` with `Allow Always` writes
   `command_allowlist=chmod 644 README.md` to `<repo>/.damaian/config.conf` and
   runs the command.
2. A later proposal for `chmod 644 README.md` in that repository classifies as
   `low` with `requiresApproval: false`.
3. A later proposal for `chmod 777 README.md` still classifies as `high` with
   `requiresApproval: true`.
4. Granting an allowance in a repository whose effective config already allows
   `ls -la` leaves `ls -la` allowed.
5. Granting the same allowance twice produces one entry.
6. Blocked commands, shell-control commands, and any command under
   `require_approval_for_all_commands` are refused with `PolicyBlocked`, and the
   repository config is left untouched.
7. A refused grant does not run the command.
8. MCP tool approvals never offer the action.

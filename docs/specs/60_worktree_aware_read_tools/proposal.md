# Feature Spec: Worktree-Aware Read Tools

Status: Done 2026-10-23 (started 2026-10-17). Registered worktrees are
resolved and refused (§5.1), the git reads are hardened (§5.4), the five read
tools take `worktree` (§5.3), `list_worktrees` and `read_git_log` are live
(§5.2), the `worktree_survey` eval scenario passes (§5.7), and the docs are
written (§5.8). The motivating prompt, re-run live against
`deepseek-v4-flash` on 2026-10-23, surveyed every worktree with no approval
stop (§7). Plan and progress in [`tasks.md`](tasks.md). Specified on 2026-10-17 from the same
session as [#59](../59_exploration_round_allowance/proposal.md), after it was
continued three times and still did not finish: 35 of its 40 approval prompts
were read-only commands the agent ran in worktrees because no tool could reach
them ([`context.md`](context.md) §1).
Order: 60 of 60
Plan: none. Evidence-driven like [#47](../47_agent_working_capability/proposal.md)
and #59 — an exception to the rule in [`README.md`](../README.md).
Also in this spec: [`context.md`](context.md) (the session, why the tools stop at
the worktree boundary today, the git hazards, and the alternatives rejected).
Depends on: [#47](../47_agent_working_capability/proposal.md) (the read tools
and their trust boundary) — built;
[#34](../34_repository_config_trust_boundary.md) (repository content is
untrusted) — built; [#20](../20_working_modes/proposal.md) (read-only tools are
allowed in every mode) — built. Everything else named below is a
cross-reference, not a prerequisite.
Related implementation specs:
[`03_structured_tool_calling.md`](../03_structured_tool_calling.md) (owns the
tool surface this extends),
[`57_worker_sessions`](../57_worker_sessions/proposal.md) (creates the
`.damaian/worktrees/` this reaches),
[`59_exploration_round_allowance`](../59_exploration_round_allowance/proposal.md)
(reads made through these tools count as exploration rounds; not a
prerequisite — this spec is useful without it, and #59 is more useful with this),
[`52_mcp_server_mode.md`](../52_mcp_server_mode.md) (exposes `list_directory`,
`read_git_status` and `read_git_diff` and must decide whether it exposes the
new argument).

## 1. Motivation

Damaian's own workflow puts work in progress in git worktrees: spec 57's
workers under `.damaian/worktrees/`, and coding agents such as Claude Code under
`.claude/worktrees/`. "What is in progress?" is therefore a question about
worktrees, and the agent cannot answer it with its tools.

`list_directory` and `search_content` honour `.gitignore`, and both directories
are ignored, so the worktrees are invisible to them. `read_git_status` and
`read_git_diff` take no argument naming a worktree, so they only ever describe
the session's own checkout. `read_file` can technically read a worktree file by
path, but nothing tells the model that the worktrees exist or where they are.

So the model falls back to the shell. In the motivating session it asked for 40
commands, and 35 of them only read: `cd .claude/worktrees/spec25-task8 && git
--no-pager diff -- … | head`, `git worktree list --porcelain`, `grep -n … | sed
-n …`. Every one needed approval, because a compound command never qualifies
as low-risk read-only. The user approved 40 prompts in about fifteen minutes and
the turn still ran out of rounds.

#47 already established the principle: navigation the agent needs often should
be a tool, which needs no approval, not a shell command with an allow-list
entry. This spec applies it to the one place #47's tools stop.

## 2. Current State

- `list_directory` and `search_content` (`navigation.rs:85`, `navigation.rs:172`)
  resolve the requested directory through `PathPolicy::resolve_existing` against
  the repository root and walk it with `tree_walk::walk`, which accumulates each
  directory's `.gitignore` plus the configured `ignore_patterns`. This
  repository's `.gitignore` lists `.claude/` and `.damaian/`, and
  `DEFAULT_IGNORE_PATTERNS` lists `.damaian/` as well.
- `read_file` (`file_access.rs:88`) resolves through the same `PathPolicy` and
  checks restricted patterns, but does not consult ignore rules. A worktree file
  is readable if the model already knows its path.
- `GitService::status` and `GitService::diff` (`git_service.rs:178`,
  `git_service.rs:297`) run `git -C <repository root>` with fixed arguments.
  `diff` returns the whole diff, **unbounded**, and does not exclude restricted
  paths: a tracked file matching `restricted_patterns` shows its changed lines,
  protected only by secret redaction ([`context.md`](context.md) §3).
- The tool definitions (`chat.rs`, `read_git_status_tool_definition` and
  siblings) describe "the repository" with no worktree argument.
- `action_effect` marks all six read tools `ActionEffect::ReadOnly`, so they
  batch within a round, need no approval, and are allowed in every mode
  (`mode.rs:244`).
- `PathPolicy::canonical_root` refuses a root outside `allowed_roots` when that
  list is non-empty, and allows any root when it is empty.

## 3. Requirements

1. The agent can list the repository's git worktrees with a tool, and learn each
   one's path, branch, `HEAD`, whether it is dirty, and how far it is ahead of and
   behind a base branch — in one call and with no approval.
2. `read_file`, `list_directory`, `search_content`, `read_git_status` and
   `read_git_diff` accept an optional `worktree` argument. With it, paths
   resolve against that worktree's root and git runs in that worktree. Without
   it, every tool behaves exactly as today.
3. The agent can read the commits a worktree's branch has that a base branch
   does not, as one-line summaries, with a tool.
4. `read_git_diff` can limit itself to given paths, return a `--stat` summary
   instead of hunks, and compare against a base branch's merge base instead of
   the index.
5. A `worktree` argument is honoured only when it names a worktree **registered
   with this repository's git**, determined by git itself and compared by
   canonical path. A directory that merely looks like a worktree — including a
   symlink a cloned repository ships under `.claude/worktrees/` — is refused.
6. Worktrees inside the repository root are reachable by default. A registered
   worktree outside it is reachable only when a configured `allowed_roots` entry
   contains it.
7. Inside a worktree the session's effective policy applies unchanged:
   restricted patterns, ignore patterns, read and search caps, and secret
   redaction. The worktree's own repository configuration is never loaded.
8. Every git read — in a worktree or the main checkout — excludes restricted
   paths, is bounded by a cap that says what it cut, and runs git with external
   diff drivers, text conversion and filesystem-monitor hooks disabled.
9. No new command enters `command_allowlist`, `is_low_risk_read_only` keeps its
   commands, and the shell-control gate is untouched. This is #47 requirement 3,
   restated because this spec is the first to widen what the tools reach.
10. Every new tool and argument records through `AuditLog::record`, naming the
    worktree it acted in.

## 4. Non-goals

- Making read-only shell commands auto-approvable, including compound ones such
  as `cd x && git diff | head`. That would widen the command trust boundary,
  which #47 requirement 3 and AGENTS.md forbid without a spec of its own. It
  stays a separate question ([`context.md`](context.md) §5).
- Writing to a worktree: edits, patches, commands or checkpoints. A patch
  proposed against a worktree path is out of scope and is refused as today.
- Indexing worktrees for `search_codebase`, the symbol index or context
  assembly. Those are per-repository and building them per worktree is a cost
  this spec does not take on.
- Reading another repository's worktrees, or any worktree not registered with
  this repository.
- Reading a committed version of a file (`git show <ref>:<path>`). The session
  used it once; `read_git_diff` with a base answers the same question.
- Creating, removing or pruning worktrees. Spec 57 owns its own.

## 5. Design

### 5.1 Which worktrees are reachable

`GitService::worktrees(repository_root)` runs
`git -C <root> worktree list --porcelain -z` and parses each entry's `worktree`,
`HEAD`, `branch`, `bare`, `detached`, `locked` and `prunable` lines. `-z`
terminates fields with NUL, so a worktree path containing a newline cannot
forge a second entry.

A worktree is **reachable** when all of these hold:

- it is listed, is not `bare`, is not the session's own checkout (addressed by
  omitting `worktree`), and is not `prunable`. The own checkout is the entry
  whose canonical path is the session's canonical root. That is usually git's
  main worktree, but not in a spec 57 worker session, whose root is a linked
  worktree; there the main worktree is an ordinary entry, reachable only under
  the location rule below (`tasks.md` C1);
- its listed path canonicalizes, and the canonical path is a directory;
- the canonical path is inside the canonical repository root, or inside a
  canonical `allowed_roots` entry (requirement 6);
- `PathPolicy::canonical_root` accepts it.

**Consequence for a linked-worktree session.** The containment test uses the
session's own canonical root. A session whose root is a linked worktree — a
spec 57 worker, or a development worktree — therefore sees its own entry as the
own checkout and every other entry, including the main checkout and its
siblings, as outside that root. With the default empty `allowed_roots` the
reachable set is empty: `list_worktrees` reads none of them and names each one
as not readable (§5.2). To read them from such a session, add the main checkout
to `allowed_roots` or run the tool from the main checkout. This is deliberate
(`tasks.md` C1): the location rule exists so a session is not silently given
another checkout, and the containment root is the checkout the user opened, not
the repository's main worktree. The pinned behavior is
`from_a_linked_worktree_list_worktrees_reads_none_but_names_them_without_allowed_roots`
(`tests/worktree_reads.rs`).

**An unreachable worktree is named, never read.** Until 2026-10-29
`list_worktrees` listed only reachable entries, so a model reading the result
could conclude a worktree it could not reach did not exist (§7). It now lists
every registered worktree except the session's own checkout: the reachable ones
as before, then the unreachable ones with their refusal. For those, branch and
`HEAD` come from git's porcelain list, and ahead/behind from
`rev-list --left-right --count base...<HEAD>` run at the session's root, which
shares the repository's object store. Nothing runs in, or reads from, the
worktree's directory, so the location rule still decides every read; naming a
registration git already holds for this repository gives nothing the rule
protects. The `HEAD` id is passed back to git only when it is all hex digits.

A `worktree` argument is resolved by canonicalizing it — as given, absolute or
relative to the repository root — and comparing it with the canonical paths of
the reachable set. Only an exact match is accepted. Everything else is refused
with the reachable worktrees listed in the error, so the model can correct
itself in the same round rather than guess. When the argument is a registered
worktree that is not reachable, the refusal also says why: the session's own
checkout, bare, prunable, missing, or outside the repository root and
`allowed_roots`.

Comparing canonical paths with a set git produced is the whole defence against
a planted directory. A cloned repository can ship `.claude/worktrees/x` as a
symlink to anywhere, but it cannot register a worktree: registration lives in
`.git/worktrees/`, which is local and never cloned. The symlink canonicalizes to
its target, the target is not in git's list, and the argument is refused.

The reachable set is computed once per tool call. It is not cached across calls,
so a worktree created or removed mid-turn is seen correctly; the cost is one
extra git spawn per call that names a worktree.

### 5.2 The tool surface

| Tool | New or changed arguments | Returns |
|---|---|---|
| `list_worktrees` (new) | `base?` | One entry per reachable worktree: repository-relative path (absolute when outside the root), branch or `detached`, short `HEAD`, changed-file count, and commits ahead and behind `base`; then, when there are any, one entry per unreachable worktree other than the session's own checkout: path, branch, short `HEAD`, ahead and behind, and the refusal (§5.1) |
| `read_file` | `worktree?` | Unchanged, with paths relative to the worktree |
| `list_directory` | `worktree?` | Unchanged, walking the worktree under its own `.gitignore` |
| `search_content` | `worktree?` | Unchanged, searching the worktree |
| `read_git_status` | `worktree?` | Unchanged, for that worktree |
| `read_git_diff` | `worktree?`, `paths?`, `stat?`, `base?` | As today, or `--stat`, or against `merge-base(base, HEAD)`, limited to `paths`, bounded (§5.4) |
| `read_git_log` (new) | `worktree?`, `base?`, `max_count?` | `base..HEAD` as `short-sha subject` lines, newest first, capped |

`base` defaults to the branch checked out in the main worktree, which is what
"how far ahead of main" means in practice; when the main worktree is detached
the default is absent and `ahead`/`behind` are omitted rather than guessed.

`list_worktrees` puts ahead and behind counts in one result because the session
spent a round, and an approval, per worktree to get them. With `n` worktrees it
spawns `1 + 3n` git processes (the list, then `status`, `rev-parse --short HEAD`
and `rev-list --left-right --count` per worktree), capped at 20 worktrees with
the rest counted, plus `rev-parse` and `rev-list` per unreachable worktree under
its own cap of 20. On this machine a spawn is usually milliseconds and
occasionally ~280 ms (`docs/DEVELOPMENT.md`, "The quality gate is slow
locally"), so the cap also bounds the worst case.

Tool descriptions say plainly what the session lacked: worktrees exist,
`list_worktrees` finds them, and the read tools take `worktree` — so the model
does not need to discover them with a shell command.

### 5.3 One resolution path

The worktree argument is resolved in one place, before dispatch, into the root
the existing code already takes. `FileAccess::read_file`,
`Navigation::list_directory` and `search_content`, and `GitService` already
accept a root path; they receive the worktree's canonical root instead of the
repository's. `PathPolicy`, the walker, the ignore and restricted rules, the
caps and the `SecretScanner` are therefore the same code on the same path, which
is how requirement 7 holds structurally rather than by a parallel
implementation.

The policy applied is the session's: the `Config` the engine was built with,
meaning the user's configuration and **this** repository's overlay. A worktree
is the same repository on another branch, and its `.damaian/config.toml` may
differ, but reading files needs no repository configuration, and loading one
from a branch the user did not open would be repository content choosing its own
policy (#34).

### 5.4 Git reads are bounded and policed

These apply to every git read, including today's `read_git_status` and
`read_git_diff` on the main checkout, because the same function serves both and
the gaps are the same ([`context.md`](context.md) §3).

- **Restricted paths are excluded** with `PathPolicy::is_restricted`, the
  matcher `read_file` and `list_directory` use. `restricted_patterns` are
  gitignore-style (`.env` matches at any depth, `!` negates), which a
  `:(exclude,glob)` pathspec cannot express, so the changed paths are listed
  first (`--name-status -z --no-renames`, so both sides of a rename are
  checked), restricted ones are dropped, and the content diff runs over the
  rest as `:(top,literal)` pathspecs (`tasks.md` C2). Status entries for
  restricted paths are dropped from both the parsed files and the raw porcelain
  the way `list_directory` drops them, since a path can name a secret; a
  rename is dropped when either name is restricted. A path below a restricted
  directory counts as restricted, since `list_directory` never descends into
  one. A status whose only changes are restricted reads as clean.
- **Output is bounded** by `max_read_lines` and `max_file_bytes`, whichever
  binds first, and a truncated result says so and how much it cut — the rule
  #47 §5.5 set for reads. A diff ends `[Diff truncated by lines: 400 of 5120
  lines shown; 7 of 12 files not shown.]`; a status, `[Status truncated: 400 of
  1830 entries shown.]`. The diff is redacted before it is cut, so a cut cannot
  leave a secret's prefix the scanner no longer recognises.
- **No repository-supplied code runs.** Every invocation passes
  `-c core.fsmonitor=false`, and `diff` and `log` pass `--no-ext-diff` and
  `--no-textconv`. `.gitattributes` is repository content; it can name a diff
  driver, and a user's own git config can define one with that name.
- **A read takes no lock.** Every invocation passes `--no-optional-locks`, as
  `checkpoint.rs::run_user_git` does, and `-c diff.autoRefreshIndex=false`.
  Otherwise `git status` may take `index.lock` to refresh the index, and `git
  diff` does the same whenever it finds stat-only changes, whatever
  `--no-optional-locks` says. In a worktree where another agent is working,
  that agent's own `git add` could fail (`tasks.md` C3).
- **Arguments cannot become options.** `base` must satisfy git's ref-name rules
  and must not start with `-`; it is passed after `--end-of-options`. `paths`
  are resolved through `PathPolicy` against the worktree first and passed after
  `--`. Arguments go to `Command` as argv; no shell is involved.
- **Output is redacted** by `SecretScanner`, as `diff` is today.

### 5.5 Recording

- The audit records of the five read tools (`file_read`, `directory_listed`,
  `content_searched`, `git_status_read` and `git_diff_read`) gain a `worktree`
  field, the worktree's display path, empty for the session's own checkout. `list_worktrees` records
  `git_worktrees_listed` with the count, the omitted count and the number of
  unreachable worktrees named, and `read_git_log` records
  `git_log_read`.
- The action marker's `ref` carries the worktree prefix
  (`.claude/worktrees/spec25-task8:crates/…/symbol_index.rs`), so the session log
  says where a read happened and #59's repeat detection distinguishes the same
  path in two worktrees. A worktree read's `FileRead` evidence carries the same
  prefixed path.
- A refused `worktree` argument is an ordinary failed tool call with
  `ActionOutcome::Failed`. It is not a finding: a refusal of a read is not a
  defect in the user's code.

### 5.6 Modes and batching

`ToolAction::ListWorktrees` and `ToolAction::ReadGitLog` are
`ActionEffect::ReadOnly` in `action_effect`, so they batch within a round, need
no approval, count as exploration under #59, and the mode matrix allows them
everywhere, as it does today's git reads. `mode.rs` crosses tools with two
exhaustive matches, `profile_permits` and `mode_permits`; both gain the two
variants in their read-only arms, and `ReadGitStatus` becomes a struct variant
there (`tasks.md` C6). The `worktree` field on the existing variants changes
none of that.

### 5.7 Eval scenario

A deterministic scenario, `worktree_survey`: a fixture repository with two
registered worktrees — one with uncommitted changes, one with commits ahead of
`main` — and a planted symlink under `.claude/worktrees/` pointing outside the
repository. The scripted turn calls `list_worktrees`, then `read_git_diff` with
`stat` and `read_git_log` on each, then tries the symlink. Assertions: zero
approval stops, the symlink is refused, the restricted file changed in one
worktree does not appear in its diff, and the answer names both worktrees.

### 5.8 Documentation

On Done: `docs/USER_GUIDE.md` (what the agent can see without asking, now
including worktrees), `docs/TROUBLESHOOTING.md` (a refused `worktree` argument
and why: not registered, outside the allowed roots, or prunable), and
`SECURITY.md` (the git read hardening in §5.4).

## 6. Acceptance Criteria

- `list_worktrees` on this repository lists every worktree `git worktree list`
  shows inside the repository root, except the main one, with branch, `HEAD`,
  dirty count and ahead/behind against the main worktree's branch. Every other
  registered worktree but the session's own checkout is named after them with
  branch, `HEAD`, ahead/behind and its refusal, and without a dirty count. From
  a session whose own checkout is a linked worktree it reads none unless
  `allowed_roots` contains the main checkout, and names them all; recorded in
  §5.1 and pinned by
  `from_a_linked_worktree_list_worktrees_reads_none_but_names_them_without_allowed_roots`.
- Each of the five existing read tools, given `worktree`, reads that worktree;
  without it, its output is byte-identical to before on the existing
  deterministic scenarios.
- A symlink under `.claude/worktrees/` that points outside the repository is
  refused, and so is a plain directory that is not a registered worktree; the
  refusal lists the reachable worktrees.
- A registered worktree outside the repository root is refused with
  `allowed_roots` empty, and reachable when an `allowed_roots` entry contains it.
- A `prunable` worktree is refused.
- A tracked file matching `restricted_patterns` and changed in a worktree
  appears in neither `read_git_status` nor `read_git_diff`, in that worktree or
  the main checkout.
- A diff larger than the caps is truncated and says what it cut.
- A `base` of `--output=/tmp/x` or `main..; rm` is refused before git runs.
- A `.gitattributes` diff driver in a worktree, with a matching driver in the
  test's git config, is not invoked by `read_git_diff`.
- No change to `command_allowlist`, `is_low_risk_read_only` or the shell-control
  gate, asserted by the existing policy tests passing unchanged.
- `worktree_survey` passes with zero approval stops.
- The motivating prompt, re-run live against `deepseek-v4-flash` on this
  repository, makes no approval stop for reading worktrees. Record how many
  approvals it did make and for what.

## 7. Implementation Notes

- Task 1 (2026-10-17): resolution lives in `worktree_access.rs`
  (`parse_worktree_list`, `classify_worktrees`, `resolve_worktree`), with
  `GitService::worktrees` and `PathPolicy::is_inside_allowed_root`. Nothing
  dispatches through it until Task 3.
- Task 2 (2026-10-17): every git process `git_service.rs` starts goes through
  one runner, `read_git`, and `GitService::new` takes the `PathPolicy` and
  `GitReadCaps`. The hardening applies to the CLI's `git-status`/`git-diff`
  and the desktop shell's `/api/git-status` too, since they call the same
  functions. `tasks.md` lists every way main-checkout output changed.
- Task 3 (2026-10-18): dispatch resolves `worktree` once and calls `_in`
  siblings of the existing read functions (`read_file_in`,
  `list_directory_in`, `search_content_in`, `status_in`, `diff_in`) with the
  worktree's root and display path; the old functions pass `None`. `base` is
  validated in Rust and the merge base resolved by its own git call; `paths`
  go through `PathPolicy::resolve_for_write` so a deleted file can be named.
- Found in Task 2, not addressed: `.gitattributes` can select a
  `filter=<name>` whose `clean` command the user's own git config defines,
  and `status` and `diff` run it on stat-dirty files. Git has no flag that
  disables filters, and turning them off by name would make git-lfs or
  git-crypt files read as modified, so §5.4's "no repository-supplied code
  runs" holds for diff drivers, textconv and fsmonitor, not for filters. It
  needs its own decision.
- Live re-run (2026-10-23, Task 6). The motivating prompt, verbatim ("check
  what specs are left to implement, also check worktrees to see specs that
  are in progress, this is a reporting only task, no code changes"), against
  `deepseek-v4-flash` on this repository with five registered worktrees,
  native tools on and default budgets, driven through a desktop shell with an
  isolated data directory. It made **no approval stop**: 9 model calls and 21
  tool calls, all through tools — `list_worktrees` once, `read_git_diff` five
  times (with `stat` and with `paths`, on three worktrees), `read_git_status`
  three times (two worktrees and the session's checkout), `read_file` five
  times (one with `worktree`), `search_content` five times, plus
  `propose_plan` and `complete_step`. The turn hit the 8-round budget once and
  paused (#61); after *Continue* it answered with a report that named every
  in-progress spec and what each worktree held. Usage: 886,386 input tokens
  (677,888 cached, 76%), 7,798 output tokens. Against the motivating session's
  40 approvals and no report (`context.md` §1), the acceptance criterion holds.
- Two earlier attempts that day are not evidence for or against #60, and are
  recorded so they are not repeated. A CLI run on a fresh data directory got
  the text envelope, not tools, because the built-in `deepseek` provider sets
  `supports_native_tools: false` (`config.rs`); with no tools offered, the
  model asked to run `git worktree list --porcelain`. A CLI run with native
  tools on read the worktrees through `list_worktrees` without asking, then
  stopped at `grep -H -m1 '^Status:' docs/specs/*.md … | sort` — rightly, under
  #64 (a bare `grep` and an unquoted glob) — and `damaian ask` cannot resume a
  turn after an approval, which is why the full run went through the desktop
  shell. That approval's text still blamed "sandbox mode", which is
  [#62](../62_approval_prompts_name_the_reason/proposal.md)'s defect.
- **Documented limitation, 2026-10-25.** A session rooted in a linked worktree
  (a spec 57 worker, or a development worktree) has that worktree as its
  containment root, so with the default empty `allowed_roots` every other entry
  — the main checkout and its siblings — is outside it and `list_worktrees`
  returns 0. The result lists only reachable entries and does not name the
  refusals, so a model can read it as "no worktrees are registered"; `git
  worktree list` and the `git_worktrees_listed` audit record (`count: 0`,
  `resourcePath` = the session's root) show the truth. The remedy is to add the
  main checkout to `allowed_roots` or run the tool from the main checkout.
  Observed in the spec 65 live run, whose session root was this repository's
  spec 65 worktree. Pinned by
  `from_a_linked_worktree_list_worktrees_is_empty_without_allowed_roots`
  (renamed and extended on 2026-10-29; see the next note). Deliberate under `tasks.md` C1; using the main worktree as the containment
  root would be a design change, not a bug fix.
- **Unreachable worktrees are named, 2026-10-29.** The limitation above also
  hid worktrees outside the repository root from a session in the main
  checkout. In a live run that day (`deepseek-v4-flash`, "check what specs are
  left to implement, also check worktrees…"), eight of this repository's eleven
  worktrees lived under another tool's data directory. `list_worktrees` showed
  the other three, and the model reported that nothing was in progress and that
  spec 30 had no branch, while `spec-30-memory-recall` was six commits ahead.
  A second run found the eight only by running `git worktree list` and other
  compound git commands, at seven approvals. `list_worktrees` now names every
  unreachable worktree but the session's own checkout, with branch, `HEAD`,
  ahead/behind and the refusal, read from git's list and the shared object
  store and never from the worktree (§5.1, §5.2). The containment rule is
  unchanged: those worktrees are still refused to every read tool. The pinned
  test is renamed
  `from_a_linked_worktree_list_worktrees_reads_none_but_names_them_without_allowed_roots`.

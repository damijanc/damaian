# Feature Spec: The Read-Only Command Boundary

Status: In progress. Started 2026-10-17: requirements 1–4 and 8 (the
tightening) are implemented; requirement 5 waits for #60, and requirements 6–7
(pipelines) are not started. Specified on 2026-10-17. It began as "let read-only
pipelines run without approval", from the session behind
[#59](59_exploration_round_allowance/proposal.md)–[#63](63_exploratory_command_failures/proposal.md).
Reading the policy to design that found the existing read-only list already
lets through more than it should: a command that writes a file, one that reads
outside the repository, and one that reads a restricted file all run today
without asking (§2). So this spec tightens first and widens second, and the
widening is small.
Order: 64 of 64
Plan: none. Evidence-driven, like #47 — an exception to the rule in
[`README.md`](README.md).
Depends on: [#20](20_working_modes/proposal.md) (Plan and Review allow only
commands that need no approval) — built;
[#34](34_repository_config_trust_boundary.md) — built;
[#60](60_worktree_aware_read_tools/proposal.md) (tools for git content and
worktrees) — **not built**. Requirements 1–4 and 6–8 do not need #60 and may
ship first. Requirement 5 waits for it, for the reason in §5.4. Everything else
named below is a cross-reference, not a prerequisite.
Related implementation specs:
[`10_persistent_command_approval.md`](10_persistent_command_approval.md)
(*Allow Always*, unchanged), [`31_permission_profiles`](31_permission_profiles/proposal.md)
(`command_access`, unchanged),
[`62_approval_prompts_name_the_reason.md`](62_approval_prompts_name_the_reason.md)
(the causes this spec adds), [`47_agent_working_capability`](47_agent_working_capability/proposal.md)
(requirement 3: tools gain capabilities, the shell does not).

## 1. Motivation

Two problems, found together.

**Approval fatigue.** In the motivating session, 35 of 40 approval prompts were
read-only commands, refused automatic execution only because they used `&&` or
`|`: `cd <worktree> && git status --short`, `git log --oneline | head`. Most of
them are better served by #60's tools. What remains is a small, real case: a
read-only command whose output is trimmed by a filter, or run in a directory
inside the repository.

**The read-only list is not read-only.** `is_low_risk_read_only` admits any
command that *starts with* `pwd`, `ls`, `git status`, `git diff`, `git log` or
`git show`, checks only the raw text for shell-control characters, and runs it
without approval in Code mode — and in Plan and Review, the modes meant not to
change anything. Three consequences, each from reading `command_policy.rs`.
None was run:

- **It can write.** `git diff --output=README.md` and `git log
  --output=<file>` write git's output to a file. Neither contains shell
  control, both start with an admitted prefix, and the path check skips tokens
  that start with `-`.
- **It can read outside the repository.** `token_escapes_root` treats
  `$HOME/.aws/credentials` as a relative path, because it does not start with `/`
  or `~`, so it resolves inside the repository root. The shell then expands
  `$HOME`. `git diff --no-index $HOME/.aws/credentials README.md` therefore runs
  without asking, and `ls $HOME/.ssh` lists a directory outside the repository.
- **It reads restricted files.** `restricted_patterns` is checked against the
  working directory, never against arguments. `git show HEAD:.env` and `git diff
  -- .env` run without asking. Secret redaction catches what the scanner
  recognises, but restricted patterns exist for what it does not, and
  `read_file` refuses the same file.

The fix for the first problem must not widen the second. So the order matters:
first make "read-only, runs without asking" mean something precise, then let a
narrow class of pipelines through under that precise meaning.

## 2. Current State

- `classify_pattern` (`command_policy.rs`) checks, in order: blocklist and
  destructive patterns; `contains_shell_control` (`;`, `&`, `|`, a backtick,
  `<`, `>`, a newline, a carriage return, `$(`) → High, approval; the allowlist
  (exact match) → Low; `is_low_risk_read_only` (a prefix match) → Low;
  validation, Docker, high-risk keywords and unknown commands after that.
- A Low command needs approval only if `require_approval_for_all_commands` is
  on, which by default it is not.
- `classify_at` raises Low to Medium (approval) when the working directory or a
  path-like argument resolves outside the repository. Its comment calls
  `references_path_outside_root` "heuristic, not a security boundary"; it skips
  tokens that start with `-` and tokens without `/` or `..`, and it does not
  expand `$`.
- `restricted_patterns` is applied to a command's working directory only
  (`validation.rs:67`).
- `mode_permits` (`mode.rs`) allows a command in Plan and Review only when
  `is_read_only_without_approval()` — so in those modes, the read-only list *is*
  the boundary.
- Commands run as `<shell> -lc <command>`, so the command text is interpreted by
  the user's login shell after classification.

## 3. Requirements

1. A command runs without approval as read-only only if a real shell tokenizer
   can split it into words with **no expansion**: no `$`, no backtick, no
   unquoted `*`, `?`, `[`, `{` or `~`. Anything the shell would rewrite after
   classification needs approval.
2. Each admitted program has a closed set of admitted options. An option
   outside the set — `--output`, `--no-index`, `--ext-diff`, `--textconv`, `-c`,
   `-C`, `--git-dir`, `--work-tree`, `--exec-path`, and anything not listed —
   needs approval.
3. Every path-like argument — positional paths, pathspecs after `--`, and the
   path half of `<rev>:<path>` — is resolved through `PathPolicy` against the
   working directory. One outside the repository, or matching
   `restricted_patterns`, needs approval.
4. The working directory must not match `restricted_patterns`, as today.
5. Once #60 is built, `git diff`, `git show` and `git log -p`/`--patch` no longer
   run without approval in the shell; the model is pointed to `read_git_diff` and
   `read_git_log`, which bound their output and exclude restricted paths
   themselves. `git status`, `git log` without patch output, `ls` and `pwd`
   remain.
6. A **read-only pipeline** runs without approval when: its only operators are
   `&&` and `|`; it may begin with `cd <dir> &&`, where `<dir>` resolves inside
   the repository (or, with #60, a registered worktree) and is not restricted;
   every `&&` segment's first command is itself admitted by requirements 1–5,
   checked against the directory it will run in; and every command after a `|`
   is a **stdin-only filter** from a closed list, with no file arguments.
7. The stdin-only filters are `head -n N`, `tail -n N`, `wc` with `-l`, `-w` or
   `-c`, `sort` with `-r`, `-n` or `-u`, `uniq` with `-c`, `cut` with `-d` and
   `-f`, and `grep` with `-i`, `-v`, `-n`, `-c`, `-E` or `-F` and one pattern.
   None of them accepts a file argument in this position, so a pipeline reads
   nothing a single admitted command could not.
8. Plan and Review modes follow the tightened definition, because they use the
   same classification.

## 4. Non-goals

- Admitting file-reading commands such as `cat`, `grep -r`, `sed -n` or `find`
  without approval. They would need requirement 3 applied to every file they
  could touch, including recursive ones, which a command line does not tell you.
  The read tools exist so the shell does not have to.
- Sandboxing commands at the OS level.
- Changing *Allow Always*. An allowlisted command is an exact string the user
  approved, and it keeps bypassing these rules as it does today. Whether it
  should is a question for #10, not this spec.
- Admitting `;`, `||`, redirects, subshells or command substitution in any form.

## 5. Design

### 5.1 One parser

A tokenizer in `command_policy.rs` (or a vetted crate such as `shell-words`,
licence-checked through `cargo deny`) turns the command into words, operators
(`&&`, `|`, and everything else as "other") and a flag for "contains expansion".
It is the only code that decides auto-run, and it replaces the prefix match:
`is_low_risk_read_only` becomes a function over tokens. The lexical split in #63
is for describing failures and is not reused here.

Anything the tokenizer cannot parse, or that contains an operator other than
`&&` and `|`, falls through to today's shell-control branch, so a parsing gap
asks for approval rather than running.

### 5.2 Admitted commands and options

| Program | Admitted options and arguments |
|---|---|
| `pwd` | none |
| `ls` | `-a`, `-l`, `-1`, `-d`, `-h`, `-R` (restricted, see below), paths |
| `git status` | `--short`, `-s`, `--porcelain`, `--porcelain=v1`, `--branch`, `-b`, `--untracked-files=no` |
| `git log` | `--oneline`, `-n N`, `--max-count=N`, `--stat`, `--name-only`, `--name-status`, `--format=…`, `--pretty=…`, `--decorate`, `--graph`, revision ranges, `--` and paths |
| `git diff`, `git show` | until #60: `--stat`, `--name-only`, `--name-status`, `--staged`, `--cached`, `-U N`, revisions, `<rev>:<path>`, `--` and paths |
| `git` (global) | `--no-pager` only |

`ls -R` is admitted only with a path argument that is not the repository root,
so a recursive listing cannot walk into restricted directories unasked; the path
itself must pass requirement 3. Restricted entries inside an admitted directory
are still listed by name: `ls` is not path-policed per entry the way
`list_directory` is. That is today's behaviour for `ls dir`, and the tool is the
answer for a listing that must hide restricted names.

A revision argument must satisfy git's ref-name rules (or be a revision range or
a hash) and must not start with `-`.

### 5.3 Paths

Each path-like word is resolved by `PathPolicy::resolve_existing` (or its
lexical counterpart for paths that do not exist yet) against the directory the
segment runs in. Outside the repository → `OutsideRepository` cause; matching
`restricted_patterns` → a new `RestrictedPath` cause in #62's enum. Both need
approval; neither blocks, because the user may legitimately want it.

This replaces `references_path_outside_root` for admitted commands. Because
requirement 1 has already refused expansion, a word's text is what the shell
will see, and the check is no longer a heuristic.

### 5.4 Why requirement 5 waits for #60

`git diff`, `git show` and `git log -p` print file contents. Even with every
argument checked, `git diff` with no pathspec prints changes to every file,
restricted or not, and a command line cannot be rewritten to exclude them
without changing what the user approved. #60's `read_git_diff` and
`read_git_log` exclude restricted paths and bound their output, so once they
exist the shell forms can ask first without leaving the agent unable to read a
diff. Before #60, removing them would turn a common, mostly harmless read into
an approval prompt with no alternative — the opposite of what the session
needed — so requirements 1–3 narrow them now and requirement 5 finishes the job.

### 5.5 Pipelines

A pipeline is admitted segment by segment:

1. An optional leading `cd <dir>` sets the directory for what follows. `<dir>`
   must resolve inside the repository (with #60, also a registered worktree) and
   pass requirement 4. `cd` anywhere else in the line is not admitted.
2. Each `&&` segment is `<admitted command> [| <filter>]*`.
3. Each filter is from requirement 7's list, with its listed options only and no
   file argument.

The result is classified Low, needing no approval unless
`require_approval_for_all_commands` is on, exactly like a single admitted
command. `may_use_network` is false for every admitted program.

`cd <dir> && <cmd>` is equivalent to proposing `<cmd>` with a working directory,
which the engine already supports for sub-roots (#24). Admitting the `cd` form
too is what lets the model write the command the way it naturally does.

### 5.6 What is recorded

Auto-run decisions record their parsed shape in the command's audit record
(`segments`, `filters`, `cd`), so a later review can see what was admitted
without re-parsing. Refusals use #62's causes, plus `RestrictedPath` and
`Expansion` from this spec.

### 5.7 Documentation

On Done: `docs/USER_GUIDE.md` (what runs without asking, stated as the table in
§5.2 and the pipeline rule), `SECURITY.md` (the read-only command boundary,
including what it does not cover: allowlisted commands, `ls` listing restricted
names), and `docs/TROUBLESHOOTING.md` (a command that used to run unasked and
now asks, and why).

## 6. Acceptance Criteria

Tightening (requirements 1–4, 8):

- `git diff --output=README.md` and `git log --output=x` need approval and do not
  run in Plan or Review.
- `ls $HOME/.ssh`, `git diff --no-index $HOME/x README.md`, `ls ~`, `ls *`
  need approval.
- `git show HEAD:.env`, `git diff -- .env` and `ls secrets/` (with `secrets/`
  restricted) need approval, with the `RestrictedPath` cause.
- `git -c core.pager=x log`, `git -C /tmp status` and `git --git-dir=x log` need
  approval.
- `git status --short`, `git log --oneline -n 5`, `ls crates` and `pwd` still
  run without asking.

Pipelines (requirements 6–7):

- `cd .claude/worktrees/x && git status --short && git log --oneline -n 5 |
  head -n 3` runs without asking when `x` is inside the repository (and, after
  #60, a registered worktree).
- `git log --oneline | grep -i fix | wc -l` runs without asking.
- `git log --oneline | grep fix secrets.txt`, `ls | xargs cat`,
  `git status; rm x`, `git status || true`, `git log > out.txt` and
  `cd /tmp && ls` need approval.

After #60 (requirement 5):

- `git diff`, `git show HEAD` and `git log -p` need approval, and the model's
  command description points to `read_git_diff` and `read_git_log`.

Throughout:

- Every existing policy test either passes unchanged or is updated with a
  comment naming the requirement that tightened it.

## 7. Implementation Notes

### Requirements 1–4 and 8 (2026-10-17)

All in `command_policy.rs`; no new dependency.

The full seven-check gate passed in CI on 2026-10-17; scoped checks passed
locally: fmt, node, lint, typos, deny, and `cargo clippy -p workspace-engine`.

- **Tokenizer.** `shell_words` is hand-written rather than `shell-words`,
  because that crate reports no unquoted globs or tildes, and requirement 1
  needs exactly that. It refuses (returns `None`, so a read-only program asks)
  on a backslash, an unclosed quote, an operator or a control character, and
  flags expansion on `$`, a backtick, `$`/backtick inside double quotes,
  unquoted `* ? [ { ( )`, and `= ^ #` at the start of a word (zsh's `=cmd`,
  `extended_glob`'s `^`, a comment).
- **`~` is refused only where the shell expands it**: at the start of a word,
  or after `=` or `:`. Requirement 1 says "no unquoted `~`", but `HEAD~1` is
  common and the shell leaves a mid-word `~` alone, so it stays admitted.
- **Verdicts.** `read_only_verdict` returns `NotReadOnly` (not `pwd`, `ls` or
  `git status|diff|log|show`, so the later branches classify it as before —
  `git -C /tmp commit` keeps its old answer), `Refused` (a read-only program
  in a form outside §5.2: High risk, approval, a reason naming the argument or
  the expansion) or `Admitted` with its path arguments.
  `is_low_risk_read_only` is "admitted", so `is_read_only_without_approval`
  and therefore Plan, Review and `command_access=read_only` follow it
  (requirement 8) with no change to `mode.rs`.
- **Options** follow §5.2's table, with these readings: short flags may be
  combined (`ls -la`, `git status -sb`); `git log` also takes `-n5` and `-5`;
  `git diff`/`git show` take `-U3` and `--unified=3` as spellings of `-U N`.
  `git log -p`/`--patch` stay admitted until #60, because requirement 5 and
  §5.4 say the content-bearing forms ask only once `read_git_log` exists,
  although the table omits them. `git status` takes no paths, as the table
  says. Anything else, including harmless options such as `git log --since`,
  needs approval.
- **Paths.** Every git positional word is path-checked, since git decides
  between revision and path only when it runs. The path half of
  `<rev>:<path>` resolves from the repository root unless it starts with
  `./` or `../`, as git resolves it; other paths resolve from the working
  directory. A path is checked lexically, then, if it exists, again after
  resolving symlinks. A path is restricted when it, an ancestor, or *every
  child* would match `restricted_patterns`: the default patterns restrict a
  directory's contents (`secrets/**`), not its name, and `ls secrets` reads
  exactly those. Git positional words containing `* ? [ (`, and pathspecs
  after `--` starting with `:`, are refused even when quoted, because git
  expands globs and pathspec magic itself.
- **Requirement 4** is also checked in `classify_at` for admitted commands
  (with the same contents rule), not only in `resolve_command_directory`, so
  a caller that classifies directly (the CLI) gets the same answer.
- **Reasons.** Without #62's `ApprovalCause`, the restricted-path reason is
  the string `RESTRICTED_PATH_REASON` followed by the path; the expansion
  reason says "shell expansion". The outside-repository reason is unchanged.
  `references_path_outside_root` still runs for every command that is not
  admitted read-only (allowlisted ones included, a non-goal here).
- **Not covered.** `ls -R crates` lists a restricted directory nested below
  `crates`, and `git diff -- crates` diffs one; the path check sees only the
  path named. Requirement 5 closes the git half; the `ls` half is §5.2's
  accepted behaviour for names.
- **Tests.** `command_policy::tests::read_only_boundary` covers every
  "Tightening" criterion against a real temporary repository, plus symlink
  escape, `<rev>:<path>` resolution, `ls -R`, a restricted working directory
  and git globs. `mode::tests::plan_and_review_refuse_commands_the_read_only_boundary_refuses`
  covers requirement 8. No existing policy test needed changing.

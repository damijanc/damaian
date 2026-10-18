# Worktree-Aware Read Tools — Background

Background for [`proposal.md`](proposal.md): the session that motivated it, why
the read tools stop at the worktree boundary, the gaps found in the git reads on
the way, and the alternatives rejected.

## 1. The session

The session behind [#59](../59_exploration_round_allowance/context.md) §1 was
continued three times with *Continue debugging* on 2026-10-17. Across its four
turns:

| | |
|---|---|
| Model calls | 54 |
| Tool calls | 57: 40 `run_command`, 10 `read_file`, 5 `search_content`, 1 `list_directory`, 1 `read_git_status` |
| Approval prompts | 40 |
| Input tokens | about 4.43 million |
| Outcome | No report. Turns 2 and 3 stopped at the 16-round cap; turn 4 was abandoned at an approval prompt |

Of the 40 approved commands, 35 only read, and 37 named a path under
`.claude/worktrees/`. The verbs, counted across all 40: `grep` 28, `head` 26,
`git diff` 21, `ls` 16, `sed -n` 13, `tail` 9, `git status` 6, `git worktree` 2,
and one each of `git show`, `git log`, `git rev-list`. The other five ran
`cargo check` or `cargo nextest`, two of them chained after reads — not reads,
and not something a reporting-only request should have run; that drift is a
separate defect (§5).

Every one of the 35 reads is answerable by a tool in [`proposal.md`](proposal.md)
§5.2. The model packed several reads into one command with `echo '=== … ==='`
separators, which is the signature of a model rationing rounds, and each packed
command still cost a round and an approval.

## 2. Why the tools stop at the worktree boundary

Not by design. #47 built `list_directory` and `search_content` to honour
`.gitignore`, which is right for the main checkout: build output and caches stay
out of the model's view. Worktree directories are gitignored for the same reason
— they are not part of the repository's content — and so they disappeared too.
The git tools were written for "the repository" when a session had exactly one
checkout. Spec 57 then made worktrees part of Damaian's own workflow, and coding
agents adopted `.claude/worktrees/`, without the read tools following.

Keeping the walker's ignore rules and adding an explicit `worktree` argument
keeps both properties: the main checkout's listing does not fill with duplicate
copies of every file, and the worktrees are reachable when asked for by name.

## 3. Gaps in the existing git reads

Found while reading `git_service.rs` for this spec. They are fixed here rather
than recorded as observations because the change rewrites the same two
functions, and a worktree argument would otherwise multiply them.

- **`read_git_diff` is unbounded.** It returns git's whole stdout. Every other
  read tool is capped by #47 §5.5, and a large diff is exactly the result the cap
  exists for.
- **`read_git_diff` does not exclude restricted paths.** A tracked file matching
  `restricted_patterns` — `.env.example`, a test fixture named `*.pem` — shows its
  changed lines in full. Secret redaction catches what the scanner recognises;
  restricted patterns exist for what it does not. `read_file` refuses such a
  path outright, so the diff is a way around a refusal.
- **`read_git_status` lists restricted paths by name.** `list_directory` drops
  them, because a path can name a secret (`navigation.rs`, the doc comment on
  `list_directory`).
- **No git hardening flags.** `git diff` honours `diff.external` and textconv
  drivers, and `git status` honours `core.fsmonitor`. A cloned repository cannot
  set git config, but `.gitattributes` is repository content and can select a
  driver by name that the user's own config happens to define. Disabling
  external diffs, textconv and fsmonitor for reads costs nothing a read needs.

None of these was observed being exploited. They are gaps in a boundary the rest
of the read surface already holds. Task 2 closed all four on 2026-10-17, and
found a fifth it did not close: `.gitattributes` can also select a clean
filter by name (`proposal.md` §7).

## 4. Alternatives rejected

- **Stop honouring `.gitignore` for worktree directories.** The listing of the
  main checkout would then contain every file once per worktree, and
  `search_content` would return each match several times. It also gives no way
  to run git in a worktree.
- **Allow `read_file` and friends to take any absolute path under the
  repository root.** `read_file` already almost can, and the model still did not
  find the worktrees. Discovery (`list_worktrees`) is the half that was missing,
  and the git tools still need a root.
- **Make read-only compound commands auto-approvable.** The broader fix for
  approval fatigue, and a change to the command trust boundary, which needs its
  own spec (§5). It would also keep the reads in the shell, where nothing bounds
  their output or applies restricted patterns.
- **Treat a worktree as a project root (#24).** Roots are subdirectories of one
  checkout that share its index and policy; a worktree is another checkout of a
  different branch. Reusing roots would make every root-scoped feature — command
  working directories, validation per root, the repository map — reach into
  worktrees, which is far more than reading.
- **Trust any directory under `.claude/worktrees/` or `.damaian/worktrees/`.**
  Those paths are repository content and a clone can ship a symlink there
  (spec 57's `worktree.rs` makes the same point for `.damaian/`). Only git's
  own registration, which a clone cannot carry, is trustworthy.

## 5. Found while specifying, out of scope

- **Continue loses the original request.** *Continue debugging* sends a fixed
  prompt ("continue from the unresolved diagnostic state") as a new turn, and a
  new turn sees the last eight messages, each cut to 2,000 characters
  (`build_model_prompt`). After the first turn's thirty messages, the user's
  "reporting only, no code changes" was outside that window. The model treated
  the work as debugging spec 25's worktree and proposed `cargo check` and
  `cargo nextest`. Spec 47 requirement 6 already names the round cap as "a
  cliff whose only continuation is a manual re-ask that starts a new task and
  loses in-turn state". Specified as
  [#61](../61_continue_the_stopped_turn/proposal.md).
- **Compound read-only commands always need approval.** Even with this spec, a
  model that reaches for `grep … | head` in the main checkout is interrupted.
  Whether a pipeline of individually read-only commands should be eligible for
  automatic execution is a command-policy question, specified in
  [#64](../64_read_only_command_boundary.md), which also found and closes
  auto-run holes in the existing read-only list.
- **The approval reason is always "cannot run in sandbox mode"**
  (`command_proposal_response`, `chat.rs`), whatever actually triggered
  approval. Naming the real reason — shell control, unknown command, network —
  would let a user approve or reject faster. Specified as
  [#62](../62_approval_prompts_name_the_reason.md).
- **Benign exits become error findings.** Two `finding_recorded` events in the
  session, severity `error`, came from exploratory commands: an unquoted
  `--include=*.rs` that zsh's `nomatch` rejected, and an `ls -d` of a directory
  that did not exist. Neither was a defect in the user's code, and both were
  offered for repair. Specified as
  [#63](../63_exploratory_command_failures/proposal.md).

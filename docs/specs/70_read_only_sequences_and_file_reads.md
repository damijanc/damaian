# Feature Spec: Read-Only Sequences and Named-File Reads

Status: Done 2026-10-30 (specified 2026-10-29, built 2026-10-29–30; local
checks in §7). The full seven-check gate passed in CI on 2026-10-30 (Quality run 41028254554). The user, after two reporting sessions: "shell
command with `;` needs an update, it is super annoying."
Order: 70 of 70
Plan: none. Evidence-driven, like #47, #64 and #69 — an exception to the rule
in [`README.md`](README.md).
Depends on: [#64](64_read_only_command_boundary.md) (the tokenizer, the
admitted forms and the path checks this widens) — built. Everything else named
below is a cross-reference, not a prerequisite.
Related implementation specs:
[`60_worktree_aware_read_tools`](60_worktree_aware_read_tools/proposal.md)
(the read tools stay the preferred way to read),
[`62_approval_prompts_name_the_reason`](62_approval_prompts_name_the_reason/proposal.md)
(the causes a refusal names, unchanged).

## 1. Motivation

#64 admitted read-only commands joined by `&&` and `|`, and listed `;`,
redirects and file-reading commands as non-goals. In use, the model writes its
surveys as `;`-separated sequences with `echo` headings, `2>/dev/null`, and
short file reads, so most of them ask:

```
echo '=== CHANGELOG Unreleased ==='; sed -n '1,80p' CHANGELOG.md
git log --oneline -20 2>/dev/null; echo "---"; git branch -a; echo "---"; git remote -v
```

Measured on 2026-10-29 against the 156 distinct commands this user had approved
(the audit log's `command_proposed` with `requiresApproval: true`), each
classified with `damaian classify-command`:

- 65 contained `;`. Admitting `;` alone would have let **none** of them run
  unasked: every one also had a segment that asks on its own. The segments
  that asked were `echo` (77), `grep` (35), `git` (18), `curl` (15), `sed`
  (13), `ls` (11), loops (10) and `cat` (5).
- Some pure reads ask today: `echo hi`, `git worktree list`, `git branch -a`,
  `git remote -v`, `git -C <worktree> status`, `ls src | head`.
- Admitting `;`, `echo`, `/dev/null` redirects, the missing git listings and
  short `head`/`tail` forms would have saved about 6 of the 156. Adding reads
  of explicitly named files inside the repository would have saved about 30.

The user chose the wider scope.

## 2. Current State

- `shell_line` (`command_policy.rs`) tokenizes `&&` and `|` and returns `None`
  for `;`, newlines and every redirect, so those lines fall to the
  shell-control branch and ask.
- Admitted programs: `pwd`, `ls`, `git status`, `git log`. Admitted filters
  after `|`: `head -n N`, `tail -n N`, `wc`, `sort`, `uniq`, `cut`, `grep` with
  one pattern.
- Path arguments of admitted commands go through `check_path_argument`:
  outside the repository → `OutsideRepository`, restricted → `RestrictedPath`,
  after resolving symlinks.

## 3. Requirements

1. `;` and a newline separate commands in a read-only line, with the same rule
   as `&&`: every segment must be admitted on its own. A trailing separator is
   allowed.
2. A line with a leading `cd <dir>` may use only `&&` and `|`. After `;`, a
   command runs even when `cd` failed, in a directory other than the one its
   paths were checked against.
3. The only redirects admitted are to `/dev/null` (`>`, `1>`, `2>`, with or
   without a space before the target) and `2>&1`, `1>&2`, `>&2`. Any other
   redirect still makes the line unparsable, so it asks.
4. `echo` is admitted with any arguments. Requirement 1 of #64 has already
   refused expansion, so it prints only its literal text.
5. These git forms are admitted, with revisions path-checked as #64 does:
   `git worktree list` (`--porcelain`, `-v`); `git branch` with only `-a`,
   `-r`, `-v`, `-vv`, `--list` or `--show-current`; `git remote` with nothing
   or `-v`; `git rev-parse` (`--short`, `--abbrev-ref`, `--verify`, `-q`,
   `--show-toplevel`, `--git-dir`, revisions); `git rev-list` (`--count`,
   `--left-right`, revisions and ranges); `git for-each-ref` (`--sort=`,
   `--format=`, `--count=`, ref patterns); `git merge-base` (revisions).
6. `git -C <dir>` is admitted when `<dir>` passes the same checks as a `cd`
   directory. Paths after it resolve against `<dir>`; a `<rev>:<path>`
   argument under `-C` asks, because its path is relative to that worktree's
   top level, not this repository's.
7. After `|`, `head` and `tail` are admitted bare and as `-N`, beside `-n N`.
8. **Named-file reads.** These run unasked when every file argument passes the
   #64 path check and there is at least one:
   - `cat` with `-n`;
   - `head`/`tail` with `-n N`, `-nN` or `-N` (`tail -f` is not admitted);
   - `wc` with `-l`, `-w`, `-c`;
   - `sed -n` (optionally `-E`) whose script only prints: items separated by
     `;`, each an address or range followed by `p`, where an address is a line
     number, `$`, or `/regex/` without `/` or a newline inside;
   - `grep` with `-i -v -n -c -E -F -l -L -h -H -w -o -s -q`, `-m N`, `-A N`,
     `-B N`, `-C N`, one pattern (or `-e pattern`), and files. `-r`, `-R`,
     `--recursive`, `-d`, `-f`, `--include`, `--exclude` and anything else
     ask.
9. A form of a new program that is not admitted falls through to today's
   classification, so `sed -i`, `cat > x` or `grep -r` are judged exactly as
   before.

## 4. Non-goals

- `||`, `&`, subshells, command substitution, loops, other redirects.
- Recursive or globbed reads. A command line does not say which files a
  recursion or glob reaches, which is #64's reason and still holds.
- Reading outside the repository, or restricted files: those still ask.
- *Allow Always*: unchanged; a line with `;` is still not eligible.
- Bounding output. A command's output is already capped by
  `max_command_output_bytes` and secret-redacted before the model sees it;
  `read_file` stays the tool that bounds by line.

## 5. Design

### 5.1 Tokenizer

`Operator` gains `Sequence`. `;` and `\n` produce it; `\r` and other control
characters still refuse. A redirect is read where it appears and dropped from
the words when it is one of requirement 3's forms; a file-descriptor digit
written against `>` (`2>`) is the redirect's, not a word. Everything else that
returned `None` still does.

### 5.2 Verdicts

`pipeline_verdict` treats `Sequence` like `And`, and refuses a leading `cd`
followed anywhere by `Sequence` (requirement 2). `names_read_only_program`
names `echo`, the requirement 5 git subcommands, and a requirement 8 program
only in an admitted form, so requirement 9 holds by construction. `git -C`
rewrites each relative path argument as `<dir>/<arg>` and adds `<dir>` itself
as a path that must exist as a directory.

### 5.3 Recording and documentation

Unchanged recording: the classification and its causes. Documentation:
`docs/USER_GUIDE.md` (what runs without asking), `SECURITY.md` (the boundary,
now including named-file reads), `docs/TROUBLESHOOTING.md`. #64 §4's non-goal
list points here.

## 6. Acceptance Criteria

- The two commands in §1 run unasked in a repository where `CHANGELOG.md`
  exists.
- `cd sub && ls; ls ..` asks; `cd sub && ls && ls ..` is judged as before.
- `cat .env`, `sed -n 1p secrets/x`, `head -n 5 ../outside` ask.
- `grep -r x .`, `sed -n 'w out' f`, `sed -i s/a/b/ f`, `tail -f log`,
  `cat f > g`, `echo $HOME`, `echo hi > f` ask.
- `git -C <dir> status` runs unasked for a directory inside the repository and
  asks for one outside it.
- Every existing #64 test passes, except pins that asserted a form this spec
  now admits, which are updated on purpose (§7).

## 7. Implementation Notes

### What landed (2026-10-29–30)

Built in `crates/workspace-engine/src/command_policy.rs`:

- Tokenizer (`shell_line`): `Operator::Sequence` for `;` and newline, a
  trailing separator dropped, the requirement 3 redirects read and dropped
  (`word_quoted` stops `'2'>x` being read as a descriptor).
- `pipeline_verdict`: a leading `cd` with any `Sequence` is refused.
- `names_read_only_program`: `echo`; the git listings in
  `READ_ONLY_GIT_LISTINGS` only when `git_verdict` admits them; the file
  readers only when `file_read_admission` admits them (requirement 9).
- `command_verdict` → `git_command_verdict` (`--no-pager`, one `-C <dir>`,
  paths rewritten under `<dir>`, `<rev>:<path>` refused), `git_listing_verdict`,
  `file_read_admission`, `is_print_only_sed_script`, bare and `-N` `head`/`tail`
  filters.

Documentation per §5.3: the user guide's admitted table and "Read-only
pipelines and sequences", `SECURITY.md`'s "Command Execution",
`TROUBLESHOOTING.md`'s "A command that used to run unasked now asks", and a
pointer from #64 §4.

### Pins updated on purpose

Tests that used a now-admitted form as their example of "asks" were changed,
not the policy: in the `command_policy` unit tests, `git status | head` is now
admitted, `a; b` and `a\nb` parse, and `git -C /tmp status` asks as
`OutsideRepository` rather than `High`; `tests/foundation.rs` used `echo` as
its "needs approval" command in five places and now uses `printf`, and its
`classifies_command_risk` and `permission_profiles.rs`'s
`classification_results_are_unchanged_for_existing_policy_tests` pinned
`ls | head` as asking, now admitted (requirement 7), with `ls | xargs cat` as
the pipe that still asks; `tests/agent_tools.rs`'s pinned boundary now lists
`echo hi; git branch -a`, `cat src/lib.rs` and `head -n 5 a` as approval-free
and `cat /etc/hosts` and `head -n 5 .env` as asking.

### Measurement

Re-run on 2026-10-30 with this branch's `damaian classify-command`, each
command classified in its recorded working directory: the audit log then held
125 distinct commands proposed with `requiresApproval: true`, not the 156
counted on 2026-10-29 (the two counts were not taken the same way, and the
difference was not traced). **26 of the 125 now run unasked**, close to §1's
estimate of about 30 in 156; 9 of the 40 that contain `;` do, including both
§1 examples.

### Checks

Local, on 2026-10-30: the `workspace-engine` suite (2071 tests, run outside
the sandbox because the `external_reference` tests need a loopback port; two
pins updated, then all pass), the `desktop-shell` suite (224 passed), scoped
clippy for `workspace-engine`, the deterministic eval tier (40 scenarios
pass), and the fast checks (fmt, node, web lint, typos, deny). The full seven-check
gate passed in CI on 2026-10-30 for the `main` push that contains this spec
(Quality run 41028254554). Not yet tried in the running app.

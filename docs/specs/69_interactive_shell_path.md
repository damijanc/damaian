# Feature Spec: Commands See the PATH the User's Terminal Sees

Status: Done 2026-10-29 (specified and built the same day; local checks in §7).
The full seven-check gate passed in CI on 2026-10-29 (Quality run 40940844885). A live session could not run `npm`, because the
app's command shell never reads the file where nvm sets `PATH`.
Order: 69 of 69
Plan: none. Evidence-driven, like #47 and #64 — an exception to the rule in
[`README.md`](README.md).
Depends on: [#34](34_repository_config_trust_boundary.md) (`shell` is user and
admin configuration only) — built;
[#46](46_process_registry_and_orphan_sweep/proposal.md) (every spawned process
is registered) — built. Everything else named below is a cross-reference, not a
prerequisite.
Related implementation specs:
[`13_docker_command_support.md`](13_docker_command_support.md) (recorded the
GUI `PATH` gap for Docker, unsolved),
[`63_exploratory_command_failures`](63_exploratory_command_failures/proposal.md)
(tells the model which shell runs its commands, unchanged),
[`64_read_only_command_boundary.md`](64_read_only_command_boundary.md) (what a
command means is judged on its text; this spec keeps that true).

## 1. Motivation

On 2026-10-29 a reporting turn asked Damaian, opened from Finder, to check which
specs were left. It ran `npm run specs:check` and got `zsh:1: command not
found: npm` (exit 127). It then spent four approvals probing for `node`, found
it under `~/.nvm/versions/node/…` by listing directories, and ran the script by
absolute path. The failed check also blocked a plan step for the rest of the
turn (#21 §7, "A blocked step is judged again").

`npm` was installed and on the user's `PATH` in Terminal. Reproduced the same
day with the environment a Finder launch gives:

```
env -i HOME=… PATH=/usr/bin:/bin:/usr/sbin:/sbin zsh -lc  'command -v npm'   # not found
env -i HOME=… PATH=/usr/bin:/bin:/usr/sbin:/sbin zsh -ilc 'command -v npm'   # ~/.nvm/…/bin/npm
```

## 2. Current State

- `CommandRunner` runs every agent command and approved validation as
  `<shell> -lc <command>` (`command_runner.rs`), with the app's own environment.
- A login shell that is not interactive reads `~/.zprofile` (zsh) or
  `~/.bash_profile` (bash) but **not** `~/.zshrc`. nvm's installer, pyenv,
  rbenv, and most version managers write their `PATH` setup to `~/.zshrc`.
- A GUI launch inherits launchd's minimal `PATH`, not the Terminal's, so the
  login profile is all a command gets. #13 §2 recorded this for Docker and
  `docs/TROUBLESHOOTING.md` tells users to configure absolute paths.
- The assistant cannot tell the difference between "not installed" and "not on
  this shell's `PATH`", so it either gives up or hunts for binaries, at an
  approval each.

## 3. Requirements

1. Commands run by `CommandRunner` see the `PATH` the user's interactive login
   shell ends up with, including what `~/.zshrc` adds, in the same order.
2. Only `PATH` is taken. Aliases, functions and other variables from the
   interactive files never reach a command, so the command policy's judgement
   of a command's text (#64) still describes what runs.
3. The interactive shell is started once per shell per process, not per
   command, in the user's home directory, never in a repository.
4. It is bounded: a shell that does not answer within 5 seconds is killed with
   its process group, and the result is "no `PATH`".
5. Any failure — timeout, non-zero exit, unreadable output, no usable entries —
   leaves commands exactly as they run today. Nothing fails because of it.
6. Empty and relative `PATH` entries are dropped, so a command never resolves a
   program against its working directory.
7. A user setting turns it off: `command_path_from_interactive_shell`, on in
   the product. It is user and admin configuration only; a repository or a
   permission profile may not set it.
8. The outcome is audited once per resolution.

## 4. Non-goals

- Taking other variables (`JAVA_HOME`, `GOPATH`, …). They are often set beside
  `PATH`, but each one widens what a repository's commands are influenced by,
  and none was needed by the motivating session. A later spec can add a named
  list.
- Hooks (#32) and language servers (#25). They spawn their program directly,
  not through a shell, and their docs already ask for an absolute path.
- The desktop terminal panel, which is a real interactive login shell already.
- Running commands in an interactive shell (`-ilc`). That would load aliases
  and functions, so `ls` or `git` could stop meaning what the policy judged,
  and it is slower and noisier on every command.
- Re-resolving while the app runs. A user who edits `~/.zshrc` restarts
  Damaian, as with any other app.

## 5. Design

### 5.1 Resolution

`shell_path::resolve(shell, timeout)` runs:

```
<shell> -ilc 'printf "%s%s%s" "<start-marker>" "$PATH" "<end-marker>"'
```

with the working directory set to `$HOME` (`/` when unset), stdin from
`/dev/null`, stderr discarded, its own process group, and
`DAMAIAN_RESOLVING_SHELL_PATH=1` in its environment so a user can skip slow
setup in their rc files. The `PATH` is the text between the markers, because an
interactive rc file may print anything before or after. The process is
registered with the #46 registry for its lifetime.

`-i` with no terminal is normal for zsh and bash: they print a warning about
the missing tty to stderr, which is discarded, and run the rc files.

Entries that are empty or not absolute are dropped. If none are left, the
resolution failed.

### 5.2 Use

The result is cached per shell path for the life of the process, failures
included, so a shell that times out costs 5 seconds once, not on every command.
It is resolved lazily, on the first command, so a session that runs no command
never starts the interactive shell.

When a `PATH` was resolved, the command is spawned with `PATH` and
`DAMAIAN_SHELL_PATH` both set to it, and the command text prefixed with
`export PATH="$DAMAIAN_SHELL_PATH"; `.

The prefix exists because a login shell rewrites `PATH` after it starts: on
macOS `/etc/zprofile` runs `path_helper`, which moves the system directories to
the front, and `~/.zprofile` commonly prepends Homebrew. Without it, a Homebrew
`node` would shadow the nvm one the Terminal uses. With it, the command sees the
interactive shell's final `PATH` in its order. It is on the same line as the
command, so error line numbers do not move, and the classification, approval,
audit and session log all see the command's own text; only the spawn argument
carries the prefix.

Only POSIX-family shells (`sh`, `bash`, `zsh`, `ksh`, `dash`, `mksh`) are
asked. In fish `"$PATH"` is a list joined by spaces and `export` is not
portable, so for any other shell nothing is resolved and commands run as today.

Variables the caller passes (#35's Git variables) are applied after, as today.

### 5.3 Recording

One `shell_path_resolved` audit event per resolution: `shell` (the configured
path), `status` (`resolved`, `timed_out`, `failed`, `unsupported`), `entries`
(the count kept), and `durationMs`. With the setting off nothing is resolved,
so nothing is recorded. The `PATH` itself is not audited; it names the
user's directories, and the count says whether it worked.

### 5.4 Security

The interactive shell runs the user's own rc files, which their Terminal runs
on every window; the shell is user configuration (#34), and a repository cannot
reach either. It runs in `$HOME`, not the repository, so an rc hook that reacts
to the working directory (`direnv`, `chpwd`) sees the home directory. Only
`PATH` crosses, without relative entries (requirement 6). The command policy is
unchanged: it judges the command's text, and the text is unchanged.

### 5.5 Documentation

`docs/USER_GUIDE.md` (the setting, and that commands see the Terminal's `PATH`),
`docs/TROUBLESHOOTING.md` (a command not found, and reading the audit event),
`SECURITY.md` (§5.4).

## 6. Acceptance Criteria

- With a shell whose interactive setup adds a directory, a command run through
  `CommandRunner` sees that directory on `PATH`, in the interactive shell's
  order even when the login setup reorders `PATH`.
- Interactive output before and after the markers does not reach the `PATH`.
- A shell that does not answer is killed after the timeout, the command still
  runs with today's environment, and the audit says `timed_out`.
- With the setting off, no interactive shell is started.
- Empty and relative entries are dropped.
- The setting is refused from repository configuration and from a permission
  profile.
- Two commands with the same shell start the interactive shell once.

## 7. Implementation Notes

### What landed (2026-10-29)

- `crates/workspace-engine/src/shell_path.rs`: `extract_path` (the markers,
  dropping empty and relative entries), `resolve` (the bounded `-ilc` run),
  `cached` (once per shell per process, failures included, with a callback
  that runs only for the call that resolved, so the audit is written once),
  `is_posix_family` and `command_with_path`.
- `CommandRunner::interactive_path` resolves through the cache and writes
  `shell_path_resolved`; the spawn sets `PATH` and `DAMAIAN_SHELL_PATH` and
  prefixes the command, before the caller's variables.
- `command_path_from_interactive_shell` is a `Capability` key, refused at
  repository and profile scope, labelled "Commands use your Terminal's PATH".

### Deviations and discoveries

- **The product default is not `Config::default()`.** `zsh -ilc` took about
  one second on the development Mac (three runs, 0.95–1.11 s) against 10 ms for
  `zsh -lc`, and it runs the user's rc files. Hundreds of tests build their
  config from `Config::default()` and run commands, and nextest runs each test
  in its own process, so a default of `true` would have started the
  developer's interactive shell once per such test. So `Config::default()`
  leaves it off, and the new `Config::product_default()`, which
  `load_for_repository_reporting` (every production load: the shell, the CLI,
  workers) starts from, turns it on. A test that wants it sets it.
- **Reading stops at the end marker**, not at end of file: an rc file can
  start a background job that holds stdout open after the shell exits.
- **Non-POSIX shells are not asked** (§5.2), rather than given the environment
  variable alone as first written: fish prints a `PATH` list joined by spaces,
  which would have produced one bogus entry.
- **The `disabled` status was dropped.** With the setting off nothing runs, so
  there is nothing to record.

### Tests

`tests/shell_path.rs` drives a fake `zsh` (a `/bin/sh` script) whose `-ilc`
and `-lc` branches set different `PATH`s and print rc noise: the `PATH` is read
between the noise; the shell runs in `$HOME` with the variable set; a shell
that sleeps is killed at a 500 ms timeout and its pid is gone; no markers is
`failed`; a command sees the interactive `PATH` in its order although the
fake's login branch rewrites it; the setting off starts nothing; a failed
resolution leaves commands unchanged; two commands start the shell once and
audit once, without the `PATH` in the record. Unit tests in `shell_path.rs`
cover extraction and the shell families, and `config.rs` the two defaults and
the scope refusals. Removing the `export` prefix fails the two tests that read
`PATH` through a command.

Local checks: scoped clippy for `workspace-engine` and `desktop-shell`, the
`workspace-engine` and `desktop-shell` test suites, the deterministic eval
tier, and the fast checks (fmt, node, web lint, typos, deny). The full seven-check gate
passed in CI on 2026-10-29 for the `main` push that contains this spec (Quality
run 40940844885).

`the_users_own_shell_answers_with_a_usable_path` is `#[ignore]`d, since it
starts the developer's real interactive shell; run by hand on 2026-10-29 with
`/bin/zsh` and an nvm setup in `~/.zshrc`, it passed in 2.4 s: the `PATH`
resolved, every entry absolute, and a command run through `CommandRunner`
printed exactly that `PATH`, so the prefix survived macOS's `path_helper` and
`~/.zprofile`. Not yet tried in the running app.

`permission_profiles.rs`'s exhaustiveness guard
(`every_capability_key_resists_weakening_from_repository_scope`) failed until
the key had its weakening case, as it is meant to.

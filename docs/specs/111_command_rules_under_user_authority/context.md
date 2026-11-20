# Spec 111 context

**Specified:** 2026-11-12. Line numbers are approximate; re-read before editing.

## Evidence

- Spec 109 D5 design (2026-11-12): "Approve for this session" needed a way to
  say which commands qualify without a list of languages or tools, and a user
  setting to admit more.
- The owner, same day: "if user wants to explicitly approve a dangerous command
  damaian must not block it. That we can support from config"; "user needs full
  authority, even if damaian suggests something else"; "how are we handling
  command options and parameters? rm -rf /tmp should be different than rm -rf
  / - both commands need explicit allow configuration".
- Spec 33 Task 1 restart (2026-11-11): plan checks written as `&&` chains asked
  every time and offered no Always allow (`allowAlways: false`, cause "uses
  `&&` — Damaian never runs shell control without asking").

## Survey: what exists today

- **Config** (`config.rs` ≈290–320): `command_allowlist`,
  `command_allowlist_by_repository`, `command_blocklist`,
  `require_approval_for_risky_commands`, `require_approval_for_all_commands`,
  `command_access`; `allow_always_entries` (≈195) records what the Always-allow
  fold added. Repository `command_allowlist` entries are reported and never
  applied (≈180, spec 34).
- **Classification** (`command_policy.rs` `classify_text` ≈440): the user
  blocklist by text prefix (`configured_prefix_match`) → `Blocked`; then
  `destructive_pattern` (≈914, text: `rm -rf /` prefix, `rm -rf .`, `git reset
  --hard`, `git clean -fd`, `dd if=`, ` mkfs`, `shutdown`, `reboot`) →
  `Blocked`, no override; then shell control (`contains_shell_control` ≈845) →
  `High`, always asks unless the read-only tokenizer admits the pipeline; then
  read-only admission (spec 64), the allowlist, Docker, network, high-risk and
  unknown programs, each with an `ApprovalCause` (spec 62).
- **Always allow** (`allow_always_eligible` ≈768): never for blocked commands,
  never for shell control, never when every command must ask.
- **Gaps** confirmed by reading: `rm -fr /`, `rm -r -f /`,
  `rm --recursive --force /`, `rm -rf ..`, `rm -rf $HOME` match no destructive
  pattern; `rm -rf /tmp` is blocked only through the `rm -rf /` prefix.
- **OpenCode, for comparison** (opencode.ai/docs/permissions, read
  2026-11-12): ordered `{ action, resource, effect }` rules with
  `allow`/`ask`/`deny`, glob patterns, last match wins; a shell scanner checks
  every subcommand and refuses what it cannot analyse; prompt answers `once`,
  `always` (saves a project-scoped allow, proposing a prefix) and `reject`; a
  separate `policies` hard-deny layer from global or managed config overrides
  everything. Unlike Damaian, a project's own config may allow.

## Decisions

- **D1 — one ordered rule list, last match wins (recommended, following
  OpenCode).** A rule is `effect pattern [in directory-pattern]`, written one
  per line in `user.conf` as `command_rule = allow rm -rf /tmp/**` (repeatable
  key, order preserved) and edited in Settings. Evaluation per command part:
  built-in default first, then repository rules, then profile rules, then user
  rules, then session rules; the last match wins **within** the user's
  authority, with the trust limits of D2.
- **D2 — authority (decided by the owner, 2026-11-12).** The user's `allow` and
  `deny` are final: an `allow` overrides every built-in block, cause and
  default; a `deny` overrides every built-in admission. Repository and profile
  rules can only move a decision toward `ask`/`deny`: a repository `allow` is a
  load-time config error naming the line, and neither can override a user
  `deny` nor a built-in block. Session rules (spec 109 D5) can only `allow`,
  only for exact commands, and never override a user `deny` or a block.
- **D3 — parsing and option normalisation.** Commands are split with the same
  no-expansion tokenizer the read-only boundary uses (spec 64), extended to
  split chains (`&&`, `||`, `;`, `|`) into parts. Anything it cannot analyse is
  `ask` and uncoverable by `allow`. A small table of known programs states
  their flag grammar (`rm`, `git`, `find`, `chmod`, `chown`, `dd`, `mv`, `cp`,
  `curl`, `wget`, …): which short flags combine, their long equivalents, and
  which arguments are paths. Options of known programs compare as a set;
  unknown programs compare word for word.
- **D4 — paths.** Arguments the table marks as paths (and, for unknown
  programs, arguments that look like paths: absolute, `./`, `../`, `~`) are
  resolved lexically against the working directory; symlinks are not followed
  (a stated limit). Pattern syntax: `*` one segment, `**` any depth, trailing
  ` *` any further words. A rule's path pattern matches the resolved path.
- **D5 — built-in defaults become rules too.** The destructive patterns are
  rewritten as built-in `deny` rules over parsed commands (recursive `rm` on
  `/`, the home directory, the repository root or a parent of it; `git reset
  --hard`; `git clean` with `-f` and `-d`; `dd of=` a device; `mkfs*`;
  `shutdown`; `reboot`), shown in Settings as "Damaian's defaults", not
  editable, but overridable by user rules (D2). Read-only admission (spec 64)
  and the approval causes (spec 62) remain the built-in `allow`/`ask` defaults.
- **D6 — migration.** At load, `command_blocklist` entries become user `deny`
  rules, `command_allowlist` and Always-allow entries become user `allow`
  rules (prefix entries become `prefix *` patterns, which is what they meant);
  the old keys keep loading for one release with a deprecation note in
  Settings, then are removed. A test replays a corpus of commands through old
  and new policy and asserts identical decisions for every pre-existing entry.
- **D7 — what the card says.** The card names the deciding rule and its
  source; an override of a built-in block shows a warning line ("Damaian
  would block this: recursive `rm` on `/`. Your rule `…` allows it.") and is
  audited `command_rule_override` with the rule text and the built-in reason
  name (never the command's output).

- **D8 — how Task 2 reads D1, D2 and D5 (2026-11-20).** Settled while
  building `command_rules.rs`, in the reading the decisions above imply:
  - *Order and limits.* Per part: built-in `deny` defaults, then repository,
    profile, user, admin, session rules (admin config has the user's
    authority and is applied after it, as it is for every other key).
    Repository and profile rules only raise a part toward `ask`/`deny`; a
    user or admin rule sets it either way, so a user `allow` also lifts a
    repository `ask` or `deny` (D1's order). A session rule only allows and
    lifts any `ask` but never a `deny` of any source (D2 names user `deny`
    and blocks; a repository `deny` is not lifted either).
  - *Built-in recursive `rm`.* Denied when an operand is not strictly inside
    the repository: `/`, home, the root itself, a parent of it, or anything
    outside (that is what makes `rm -rf /tmp` blocked, proposal §4). Any
    recursive `rm`, with or without `-f`. Differences from the text patterns
    for Task 3's corpus: `rm -rf /abs/path/inside/repo` is no longer a
    built-in block (the relative spelling never was); `rm -r /` is now one;
    `rm -rf *` is unanalysable, so it asks instead of being blocked.
  - *The other defaults.* `git reset` with `--hard`; `git clean` with
    `--force` and `-d` (any spelling); `dd` whose `of=` is under `/dev`
    except `/dev/null`, `/dev/stdout`, `/dev/stderr`, `/dev/tty` (the text
    pattern blocked every `dd if=`); a program named `mkfs*`; `shutdown` and
    `reboot` with any arguments (the text pattern matched only the bare
    word). Text inside another program's arguments (`echo git reset --hard`)
    no longer matches. macOS's own `newfs_*` is not in D5's list and is not
    denied.
  - *Patterns.* A written rule's `*` is always a wildcard (quoting does not
    make it literal); a trailing ` *` word matches any further arguments,
    options included, and also none; a trailing `**` needs at least one
    segment (`/tmp/**` is below `/tmp`, not `/tmp`); a path pattern matches
    only a path argument and a word pattern only a word, so an unknown
    program's `x` and `./x` differ; a glob in a known program's name does
    not match it. `in <directory>` must be absolute or start at `~`, and
    cannot contain spaces. A pattern that is a chain or unanalysable is a
    config error.
  - *Session rules* are exact: the approved command parsed in its
    directory; each part of an approved chain is covered, alone or in
    another chain, in the directory it ran in. A quoted `'*'` stays literal.
  - *Unanalysable commands* ask; no rule of any effect applies to them.
  - *Config.* `command_rule` is repeatable; `ConfigOverlay::command_rules`
    keeps file order and line numbers and writes the lines back on save; a
    repository or profile `allow` is refused as `restrict_only`, with the
    line in the new `RejectedConfigKey::line`. `Config::to_policy_text` (the
    effective-policy text) does not list rules yet: Task 5's Settings work.

- **D9 — wrappers are unwrapped (decided by the owner, 2026-11-20: "A:
  unwrap them").** `sudo`, `env`, `nice`, `time` and `command` parse into the
  wrapper as its own part, holding only its own options (and `env`'s
  `NAME=value` assignments), followed by the command it runs as a further
  part, joined by `ChainOperator::Wraps`; wrappers nest (`sudo nice rm` is
  three parts). Every part must be allowed, so the built-in defaults and a
  user `deny` see `rm -rf /` in `sudo rm -rf /`, and `allow sudo *` covers
  only the `sudo` part. A one-part pattern covers any single part, wrapped
  or not; a wrapped pattern (`allow sudo make install`) covers a group that
  matches it part for part, and not the command without its wrapper. A
  session rule covers the exact group it was approved for. Unanalysable
  (asks, no rule covers it): a wrapper option the table does not model, a
  value-taking short flag inside a group (`-uroot`), `sudo -s`/`-i`/`-D`,
  `env -C`/`-S`/`-P`, `env PATH=…` (it changes which program runs), `cd`
  behind a wrapper, and `xargs` anywhere, since its command takes arguments
  from its input; a rule pattern naming `xargs` is therefore a config error.
  `command -v`/`-V` and a wrapper with no command run nothing and stay one
  part. Not covered: other wrappers (`nohup`, `timeout`, `caffeinate`,
  `arch`, …) still parse as unknown programs.

- **D10 — how Task 3 reads D6 (2026-11-20).** `Config::legacy_command_rules(
  repository_root, home, report)` builds the rules; it is computed for a
  repository rather than stored at load, because what an allowlist entry
  meant depends on where the command runs.
  - *Allowlist and Always allow.* The old allowlist matched exact text, only
    at the repository root (or at the sub-root a `cd <root> && <command>`
    entry names, spec 24), so each entry becomes `allow <command> in
    <that directory>` — not a bare `allow <command>`, which would cover every
    directory. Source: the scope that set `command_allowlist` last (user or
    admin); Always allow entries are the user's. An entry is not migrated,
    and is named in `LegacyCommandRules::unmigrated` with a reason, when the
    old policy never let it run (a blocklist prefix or a destructive pattern
    caught it, or it holds shell control, which was asked before the
    allowlist was consulted) — as a user `allow` it would now override those
    blocks — or when a rule cannot say it exactly (`*` is literal in the
    allowlist and a wildcard in a rule; `$…` and other unanalysable text).
    An entry the old policy did run is migrated even where the new built-in
    defaults deny it (`rm -r /tmp/x`, which the text patterns missed): the
    user `allow` overrides, and Task 4's card warns (D7).
  - *Blocklist.* Each entry becomes `deny <entry> *`, with the source of the
    scope that put it in force, found by replaying the merge from the
    report's applied keys (user and admin replace the list; repository and
    profile add to it). Without a report every entry is the user's. An
    entry that is not one command is not migrated.
  - *Order.* The allows, then the denies, then the `command_rule` lines:
    within one source the last match wins, so a blocklist entry still beats
    an allowlist entry, and the user's own rules beat both.
  - *Deprecation note.* `LegacyCommandRules::deprecation_notes()` gives one
    line per old key in use and one per unmigrated entry. Showing it in
    Settings is Task 5's; nothing shows it yet.
  - *Equivalence.* `tests/command_rule_equivalence.rs` replays the command
    policy's own test commands, the destructive spellings, the legacy-key
    entries and their variants, and 217 commands recorded from real
    Damaian sessions on this repository (`tests/fixtures/command_corpus/`,
    home paths replaced), through the old policy and the migrated rules,
    with no legacy keys and with every key from user, repository and Always
    allow, at the root and a sub-root. The recorded set shows no
    difference. The listed differences are D8/D9's fixes and four migration
    effects: another spelling of an allowlisted command is allowed too
    (`cargo  test`, `rm -fr target`, the path written out); a blocklist
    prefix ending inside a word (`docker` vs `dockerx`) no longer matches;
    a blocklisted command now denies inside a chain and behind a wrapper;
    an allowlist entry a rule cannot say asks. None lets a command run
    that the old policy asked for, except another spelling of an
    allowlisted command.
  - *Open for Task 4.* An unanalysable command asks and no rule applies
    to it (D8), so a blocklisted command written with `$…` or a glob asks
    instead of being blocked; and the allowlist branch kept
    `require_approval_for_all_commands`, so an `allow` from a migrated
    entry must still ask when that key is on.

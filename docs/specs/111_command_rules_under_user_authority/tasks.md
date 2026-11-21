# Spec 111 tasks

**Started:** 2026-11-12

Implementation plan for [`proposal.md`](proposal.md). The decisions and the code
survey are in [`context.md`](context.md).

## Global constraints

- One task, one session (AGENTS.md).
- Security boundary (AGENTS.md "Command execution policy"): this spec changes
  the policy by an explicit specification; every change keeps a test that a
  repository cannot widen anything and that only the user's own rules can.
- Until Task 4 lands, every existing decision stays as it is: Tasks 1–3 add
  the new path beside the old and prove equivalence.
- Scoped checks per task; workspace gate on CI.

| Task | State | Durable result |
|---|---|---|
| 1 · Parser: chains, words, known programs' options, paths (D3, D4) | Done 2026-11-12 | New `command_parse.rs`: `parse_command(command, &ParseContext { working_directory, home })` → `CommandParse::Chain { parts, operators }` or `Unanalysable { reason }`. Each `CommandPart` has `program`, the `working_directory` it runs in, and a `PartShape`: `Known { subcommand, global_options, options: BTreeSet, operands }` or `Unknown { arguments }` (word for word, path-like words resolved). The tokenizer is spec 64's `shell_line`, generalised in `command_policy.rs` as `scan_shell_line(command, admit_or)`: `chain_line` also splits `\|\|` (new `Operator::Or`), and `ShellLine` gained `unanalysable` (like `expansion` minus home tildes) and `home_tilde` (unquoted `~`/`~/…` words); `shell_line` is unchanged, so no decision changed (748 lib tests pass). Table: `rm`, `rmdir`, `mv`, `cp`, `ln`, `mkdir`, `touch`, `chmod`, `chown`/`chgrp`, `find` (leading options, start paths, expression in order), `dd` (`key=value` as a set, `if`/`of` paths), `curl`, `wget`, and `git` (global options; `-C` moves path resolution; subcommands `reset`, `clean`, `rm`, `add`, `restore`, `checkout`, `switch`, `branch`, `push`, `commit`; any other subcommand word for word). Paths resolve lexically (`normalize_lexically`, now `pub(crate)`), `/..` stays `/`. Discoveries for Task 2: `cd <dir> &&` moves later parts' directory, any other `cd` with a later part is unanalysable; a leading `NAME=value` is unanalysable; `/bin/rm` etc. are recorded as `rm`; options are read anywhere among operands (GNU), so BSD's `rm / -rf` reads as options; an unknown letter in a short group keeps the rest of the group as one literal option (`rm -rZf` → `--recursive`, `-Zf`); wrappers (`sudo`, `env`, `xargs`, `command`, `nice`, `time`) are unknown programs, so a built-in `deny` on `rm` does not see `sudo rm -rf /` — Task 2 should decide them. 11 parser tests in `command_parse::tests`. Checks: scoped clippy, `cargo nextest run -p workspace-engine --lib` (748 passed) and the config-trust and profile suites, fast checks all passed locally; workspace gate left to CI. |
| 2 · Rules and evaluation with the trust limits (D1, D2, D5) | Done 2026-11-20 | New `command_rules.rs`: `CommandRule { effect, pattern, directory, source, line }` (`RuleSource`: repository, profile, user, admin, session), `CommandRule::parse` for `command_rule` lines, `CommandRule::session(command, working_directory, home)` for spec 109's exact approvals; `evaluate_command(command, rules, &RuleContext { repository_root, working_directory, home })` → `RuleEvaluation::Unanalysable` (asks) or per-part `PartDecision { part, verdict, decided_by, built_in }` with `overrides_built_in()`; chain verdict = strictest part (`Allow < Default < Ask < Deny`; `Default` = no rule matched and no built-in deny, so the existing defaults decide — Task 4 resolves those parts). Built-in `deny` defaults are `BuiltInDefault` checks over parsed parts with a `detail` for the card. Wrappers unwrapped per the owner's decision (context.md D9): the parser adds `ChainOperator::Wraps` parts; rules match over wrap groups. Config: repeatable `command_rule` → `ConfigOverlay::command_rules` / `Config::command_rules` (Capability, classified; `CONFIGURATION.md` row says "not yet used to decide"), lines kept and written back on save, repository/profile `allow` refused as `restrict_only` with the new `RejectedConfigKey::line`. How D1/D2/D5 were read, and every difference from the old text patterns, is context.md D8. Nothing calls the evaluator yet, so no decision changed. Tests: 11 in `command_rules::tests`, 1 new in `command_parse::tests`, `repository_command_rules_may_only_ask_or_deny` and `command_rules_survive_a_config_round_trip_in_order` in `repository_config_trust.rs`, a `command_rules` row in the weakening table. Checks: scoped clippy, fast checks (fmt, node, lint, typos, deny), `cargo nextest run -p workspace-engine --lib --test repository_config_trust --test permission_profiles --test config_reference` 927/928 — the failure, `every_capability_key_resists_weakening_from_repository_scope`, fails identically on unmodified `main` (no case for spec 109's `project_checks_by_repository`); workspace gate left to CI; the full seven-check gate passed in CI on 2026-11-20 (Quality run 44713350730). For Task 3: the corpus differences are listed in D8 (`rm -rf *` now asks rather than blocks; absolute paths inside the repository are no longer built-in blocks; `rm -r /`, `shutdown -h now` and wrapped commands now are). For Task 4: home directory must be supplied in `RuleContext` (without it `~` is unanalysable); `Default` parts still need the old classifier; `Config::to_policy_text` does not list rules yet (Task 5). For spec 109 Task 5: build the session rule with `CommandRule::session(...)`, store its `pattern` and `directory` in the session log, rebuild with the same call; it lifts any `ask` and no `deny`. |
| 3 · Migration of the old keys, with an equivalence corpus (D6) | Done 2026-11-20 | `Config::legacy_command_rules(repository_root, home, report) -> LegacyCommandRules { rules, unmigrated, allowlist_entries, always_allow_entries, blocklist_entries }` (`config.rs`), with `deprecation_notes()`. Computed per repository rather than stored at load, because an allowlist entry applied only at the root (or the sub-root of a `cd <root> && <command>` entry, spec 24): it becomes `allow <command> in <that directory>`. Blocklist entries become `deny <entry> *`. Sources are real: the blocklist merge is replayed from `report.applied` (user/admin replace, repository/profile add); the allowlist takes the scope that set it last; Always allow entries are the user's; no report → user. Rules come out allows first, then denies, so a blocklist entry still wins within a source. Not migrated, and named in `unmigrated` with a reason: allowlist entries the old policy never let run (blocklist prefix, destructive pattern, shell control) and those a rule cannot say exactly (`*`, `$…`); blocklist entries that are not one command. An allowlist entry the old policy ran is migrated even where the new built-in defaults deny it (`rm -r /tmp/x`): the user `allow` overrides, as the allowlist did (decided during the task; the first draft dropped them, which turned old "runs" into "blocked"). New `CommandRule::new(effect, pattern, directory, source)` validates a built rule and refuses a pattern ending in `in <dir>`; `contains_shell_control` and `destructive_pattern` are now `pub(crate)`. The old policy still decides; nothing calls the migration yet. Full reading in context.md D10; `CONFIGURATION.md` describes the coming migration. Test `tests/command_rule_equivalence.rs` (4 tests): 218 commands from the `command_policy` tests, 43 destructive spellings, 39 legacy-key variants and 217 commands recorded from real Damaian sessions on this repository (`tests/fixtures/command_corpus/recorded_sessions.json`, from the audit log, home paths replaced, commands mentioning passwords/secrets/tokens or other projects left out), compared old vs migrated with no legacy keys and with every key from user, repository and Always allow, at the root and a sub-root. The recorded set shows no difference. 21 listed groups of differences, each with its reason: D8/D9 fixes, plus four migration effects (another spelling of an allowlisted command is allowed too; a blocklist prefix ending inside a word, `docker`/`dockerx`, no longer matches; a blocklisted command now denies in any chain part and behind a wrapper; an allowlist entry a rule cannot say asks). Checks: scoped clippy; `cargo nextest run -p workspace-engine --lib --test command_rule_equivalence --test config_reference --test permission_profiles --test repository_config_trust` 932 passed; fast checks fmt, node, lint, typos passed, `cargo deny` passed outside the sandbox (it needs a lock in `~/.cargo`); `npm run specs:check` passed; workspace gate left to CI. **For Task 4:** build the policy's rules as `legacy.rules` then `config.command_rules` (order matters: last match wins within a source); a blocklisted command written with `$…` or a glob is unanalysable and now asks instead of being blocked (listed in the corpus as open) — decide whether to keep a text-prefix check for those; the old allowlist branch kept `require_approval_for_all_commands`, so an `allow` must still ask when that key is on; the deprecation notes are not shown anywhere yet (Task 5 Settings). |
| 3b · Shell special parameters (D11) | Not started | — |
| 4 · Switch the policy to rules; cards and audit name the rule (D7) | Not started | — |
| 5 · Settings: the rule list, sources and editing | Not started | — |
| 6 · Documentation and close-out | Not started | — |

## Task 1 · Parser: chains, words, known programs' options, paths (D3, D4)

- [x] Extend the spec 64 tokenizer to split chains into parts and to report
      "cannot analyse" for expansion, substitution, globs and here-docs.
- [x] Known-program flag table (combined short flags, long forms, path
      arguments); options compare as sets for known programs.
- [x] Lexical path resolution against the working directory.
- [x] Tests: `rm -rf`, `-fr`, `-r -f`, `--recursive --force` parse equal;
      `/tmp/..` resolves to `/`; `$(…)`, `$HOME`, backticks, `*` are
      unanalysable; chains split correctly, quotes respected.

Files: `command_policy.rs` (or a new `command_parse.rs`), tests.
Checks: `cargo clippy -p workspace-engine --all-targets --locked -- -D warnings`;
`cargo nextest run -p workspace-engine --test command_policy` (or the policy
tests).

## Task 2 · Rules and evaluation with the trust limits (D1, D2, D5)

- [x] `CommandRule { effect, pattern, directory, source }`; parsing of
      `command_rule` lines; pattern matching over parsed parts.
- [x] Evaluation order and trust limits; repository `allow` is a config error;
      the built-in destructive defaults as `deny` rules over parsed commands.
- [x] Tests: the acceptance examples for `rm -rf /tmp` and `rm -rf /`; a user
      `allow` overrides a built-in `deny`; a repository rule never widens; a
      session rule never overrides a user `deny`; any `deny` in a chain denies.

Files: `command_policy.rs`, `config.rs` (overlay classification for
`command_rule`), tests. Checks: as Task 1, plus the config overlay tests.

## Task 3 · Migration of the old keys, with an equivalence corpus (D6)

- [x] Load `command_blocklist`, `command_allowlist`, Always-allow entries as
      rules; deprecation note.
- [x] Equivalence corpus test: every command in the existing policy tests and
      a recorded set from real sessions decides the same under old and new
      evaluation (except the documented fixes: `rm -fr /` and friends now hit
      the built-in default).

Files: `config.rs`, `command_policy.rs`, tests. Checks: as Task 2.

## Task 3b · Shell special parameters (D11)

- [ ] Shared tokenizer (`command_policy.rs`): accept `$?` `$#` `$$` `$!` `$-`
      `$@` `$*` `$1`–`$9` (bare, double-quoted, braced, embedded in a word) as
      opaque argument text; keep the command unanalysable when one is the
      program word, a redirection target or `cd`'s argument, and for every
      other `$` form.
- [ ] `command_shell_parameters` (user/admin key, any subset of `$0 $_ $IFS`)
      adds those under the same placement rules. Repository and profile may
      only remove entries; classify the key in the overlay table (spec 34).
      Document it in `docs/CONFIGURATION.md` (the config reference test reads
      it).
- [ ] Tests: each accepted form parses as an argument and matches as written
      (`rm -rf /tmp/$$` against `/tmp/*` and `/tmp/build`); program-word,
      redirection and `cd` placements stay unanalysable; `$0`/`$_`/`$IFS`
      unanalysable until enabled; a repository cannot enable them (the
      repository-weakening test in `permission_profiles.rs` gets a case); the
      spec 64 read-only boundary admits `ls; echo $?`; Task 3's equivalence
      corpus gains these forms.

Files: `crates/workspace-engine/src/command_policy.rs`, `command_parse.rs`,
`config.rs`, `docs/CONFIGURATION.md`, tests. Checks: `cargo clippy -p
workspace-engine --all-targets --locked -- -D warnings`; `cargo nextest run -p
workspace-engine --test command_policy` and the parser, rules, config reference
and permission-profile tests; the deterministic eval tier.

## Task 4 · Switch the policy to rules; cards and audit name the rule (D7)

- [ ] `classify_text` evaluates rules; `ApprovalCause` gains the deciding rule;
      Always allow writes a user rule of the exact command.
- [ ] Card text and `command_rule_override` audit; the shell's card payload
      carries the rule and source.
- [ ] Tests: card and audit text for allow-override, deny, repository ask;
      the eval deterministic tier unchanged.

Files: `command_policy.rs`, `validation.rs`, `audit.rs`, desktop-shell
`lib.rs`, `app.js`, tests. Checks: scoped clippy; policy and `plan_turn`
tests; `cargo nextest run -p desktop-shell`; deterministic eval; web checks.

## Task 5 · Settings: the rule list, sources and editing

- [ ] Settings → Commands: effective rules in order with source; add, edit,
      reorder, delete user rules; built-in defaults shown read-only.
- [ ] Manual check in the running app; UI tests.

Files: `app.js`, `index.html`, `style.css`, desktop-shell routes (classify any
new route in `routes.rs`), tests. Checks: web checks;
`cargo nextest run -p desktop-shell`.

## Task 6 · Documentation and close-out

- [ ] `USER_GUIDE.md`, `TROUBLESHOOTING.md`, `SECURITY.md` (the user's
      authority and the repository limit), spec 34/62/64 dated notes.
- [ ] Status Done in all four places only after CI is green; README row,
      CHANGELOG Unreleased item, dependency lines; `npm run specs:check -- --strict`.

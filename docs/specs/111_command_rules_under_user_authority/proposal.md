# Feature Spec: Command Rules Under the User's Authority

Status: Done 2026-11-23 (specified 2026-11-12; started 2026-11-12; Tasks
1–6 and 3b in [tasks.md](tasks.md); the evidence for each acceptance
criterion is in §5)
Order: 111 of 111
Plan: none. Written from the owner's design discussion on 2026-11-12 (spec 109
D5 "Approve for this session", and a comparison with OpenCode's permission
rules); the code survey and decisions are in [context.md](context.md).
Depends on: [#34](../34_repository_config_trust_boundary.md) (repository
config may only tighten), [#62](../62_approval_prompts_name_the_reason/proposal.md)
(approval causes), [#64](../64_read_only_command_boundary.md) (the read-only
boundary) — all built. Spec
[#109](../109_fast_verification/proposal.md) Task 5 (Approve for this session)
builds on this spec, which is Done.

## 1. Motivation

Damaian decides whether a command runs, asks or is blocked from several
separate pieces: a user `command_blocklist`, `command_allowlist` and **Always
allow** entries, a built-in list of destructive patterns that nothing can
override, the read-only boundary, and the approval causes. Three problems came
out of dogfooding and design (2026-11-11/12):

- **The user cannot overrule Damaian.** A built-in destructive pattern is a
  wall. The owner: "if user wants to explicitly approve a dangerous command
  damaian must not block it"; "user needs full authority, even if damaian
  suggests something else."
- **Matching is by text.** `rm -rf /tmp` is blocked only because it starts
  with `rm -rf /`, while `rm -fr /`, `rm -r -f /`, `rm --recursive --force /`,
  `rm -rf ..` and `rm -rf $HOME` match no pattern and reach an ordinary card. A
  user rule written as a prefix would be as loose the other way. The owner:
  "rm -rf /tmp should be different than rm -rf / - both commands need explicit
  allow configuration."
- **Chains always ask.** `cargo fmt --check && cargo clippy …` asks every time
  and can never be remembered, although each part alone could be.

## 2. Requirements

1. **One ordered list of command rules in the user's config** decides, last
   matching rule wins. Each rule is `allow`, `ask` or `deny` with a command
   pattern and an optional working-directory pattern. It replaces
   `command_blocklist`, `command_allowlist` and the **Always allow** entries,
   which are migrated into it (context D6).
2. **The user has full authority** (decided by the owner, 2026-11-12). A user
   `allow` wins over every built-in judgement, including the destructive
   patterns, shell control and network; a user `deny` wins over every built-in
   permission, including read-only admission. Damaian's built-in judgements are
   its **defaults** — what happens when no user rule matches — and stay as they
   are today: destructive patterns blocked, unknown programs asked, read-only
   commands admitted.
3. **A repository can only tighten.** Repository config may add `ask` and
   `deny` rules and never `allow`; it can never lift a built-in block or a
   user rule's `deny` (spec 34). A permission profile may only tighten too.
4. **Commands are matched parsed, not as text.** A command is split into words
   with no shell expansion; a chain is split into its parts and every part must
   be allowed for the chain to run without asking (any `deny` denies, any `ask`
   asks). A command that cannot be analysed safely (`$VAR`, `$(…)`, backticks,
   an unquoted glob the shell would expand, here-docs) is `ask`, and no `allow`
   ever covers it.
   *(2026-11-21, context D11: the special parameters `$?` `$#` `$$` `$!` `$-`
   and the positional `$@` `$*` `$1`–`$9` are analysable as argument words.
   `$0`, `$_` and `$IFS` are too once the user enables them with
   `command_shell_parameters`, which a repository can only narrow. A
   parameter that can expand to nothing — `$1`–`$9`, `$@`, `$*`, `$!` and the
   opt-in ones — counts only as a whole argument, never inside a path; a real
   parameter never matches the same text quoted; the opt-in applies to the
   rules, not the read-only check.)*
5. **Options in any spelling.** For programs Damaian knows (context D3), short
   flags combined or apart and their long forms are the same option, in any
   order: `rm -rf`, `rm -fr`, `rm -r -f` and `rm --recursive --force` are one
   command, for the built-in checks and for user rules alike. For other
   programs, options match as written.
6. **Paths are resolved before matching.** Path arguments are made absolute and
   normalised (`.`, `..`, `~`) against the command's working directory before
   any rule or built-in check compares them. In patterns `*` matches within one
   path segment and `**` across segments; a trailing ` *` word matches any
   further arguments.
7. **Exact means exact.** A rule covers what it names and nothing beside it:
   `allow rm -rf /tmp` does not cover `/tmp/x`, `/tmpfiles` or `/`;
   `allow rm -rf /tmp/**` does not cover `/tmp/..`, which resolves to `/`.
   `rm -rf /` needs its own explicit `allow`.
8. **The decision names its rule.** Every card and audit record says what
   decided: "Allowed by your rule `rm -rf /tmp/**` (user config)"; "Damaian
   would block this (`rm` recursive on `/`); no rule of yours allows it"; "Asked
   by repository rule `…`". A user `allow` that overrides a built-in block
   still shows a warning on the card and is audited as an override, but runs
   as the user decided.
9. **Approvals write rules.** **Always allow** writes a user rule; spec 109's
   **Approve for this session** writes a session-scoped rule (D5 there) that
   lives in the session log, not in config. Both write the exact parsed
   command and directory, never a widened prefix unless the user edits it.
10. **Visible and editable.** Settings lists the effective rules in order, with
    their source (user, repository, profile, session, built-in default), and
    lets the user add, edit, reorder and delete user rules; the same list is
    plain text in `user.conf`.

## 3. Non-goals

- Sandboxing commands (what an allowed command can reach is unchanged).
- Rules for file edits, MCP tools or web access; this spec covers commands.
- Changing which commands are read-only by default (spec 64).

## 4. Acceptance criteria

- `rm -rf /tmp` and `rm -rf /` are blocked by default; each runs only after its
  own user `allow`; neither rule covers the other; `rm -fr /`, `rm -r -f /`,
  `rm --recursive --force /` and `rm -rf /tmp/..` are treated as `rm -rf /`.
- A user `allow` runs a command the built-ins would block, with a warning and
  an override audit record; a repository `allow` is refused at load with a
  config error naming the line.
- `cargo fmt --check && cargo clippy` runs without asking when both parts are
  allowed and asks when either is not; `echo $(rm -rf /)` is never covered by
  an `allow`.
- Existing `command_blocklist`, `command_allowlist` and Always-allow entries
  migrate into rules with the same effect; a test shows every old decision
  unchanged.
- `USER_GUIDE.md` and `TROUBLESHOOTING.md` explain rules, defaults, overrides
  and how to read "what decided".

## 5. Evidence

Filled in at close-out (Task 6, 2026-11-23) from the code and tests on `main`
after Task 5, not from the task notes. Test names are in
`crates/workspace-engine` unless a crate is named. The scoped run that backs
them: `cargo nextest run -p workspace-engine --lib --test command_rules_decide
--test command_rule_equivalence --test repository_config_trust --test
config_reference --test permission_profiles`, 985 passed.

- **`rm -rf /tmp` and `rm -rf /`.** `command_rules::tests::rm_rf_tmp_and_rm_rf_root_are_denied_by_default_and_each_needs_its_own_allow`:
  with no rules, `rm -rf /`, `rm -rf /tmp`, `rm -fr /`, `rm -r -f /`,
  `rm --recursive --force /`, `rm -rf /tmp/..`, `rm -rf ..`, `rm -rf ~` and
  `/bin/rm -rf /` are denied by the built-in `RecursiveRemove`; `allow rm -rf
  /tmp` allows `/tmp` and `/tmp/` and none of `/`, `/tmp/x`, `/tmpfiles`,
  `/tmp/..`; `allow rm -rf /tmp/**` covers `/tmp/x/y` and not `/tmp/..`,
  `/tmp` or `/`; `allow rm -rf /` allows every spelling of `rm -rf /`
  including `/tmp/..`, and not `/tmp`. Through the live policy,
  `command_rules_decide::a_built_in_block_names_itself_when_no_rule_allows_it`
  blocks `rm -fr /` with the default named. **Met.**
- **A user `allow` over a built-in block; a repository `allow` refused.**
  `command_rules_decide::a_user_allow_over_a_built_in_block_runs_warns_and_is_audited`:
  `allow rm -rf /tmp/damaian-scratch/**` lets `rm -rf
  /tmp/damaian-scratch/cache` run unasked, the proposal carries the warning
  ("Damaian would block this: recursive `rm` of … Your rule … allows it."),
  one `command_rule_override` audit record has the rule, `ruleSource` `user`
  and `builtInDefault` `recursive_remove`, and `rm -rf /` stays blocked;
  `command_rules::tests::a_user_allow_overrides_a_built_in_deny_and_says_so`
  does the same for `git reset --hard`. Desktop shell:
  `command_proposal_endpoint_names_the_deciding_rule_and_its_source` serves
  the rule, source, summary and warning to the card.
  `repository_config_trust::repository_command_rules_may_only_ask_or_deny`:
  a repository `command_rule=allow rm -rf /` is refused as `restrict_only`
  with `line` 3 and never reaches the rule list, so `rm -rf /` stays denied;
  `command_rules::tests::a_repository_or_profile_rule_never_widens` covers
  profile rules too. Desktop shell:
  `command_rules_route_lists_sources_and_edits_only_user_rules` shows the
  refusal by line in Settings → Commands. **Met**, with one reading: the
  "config error naming the line" is a refused key reported with its line
  (spec 34's mechanism, audited as `repository_config_key_rejected`), not a
  failed load — the rest of the repository's config still applies.
- **Chains; `echo $(rm -rf /)`.**
  `command_rules::tests::any_deny_in_a_chain_denies_and_every_part_must_be_allowed`
  and, through the live policy, `command_rules_decide::a_chain_runs_only_when_every_part_is_allowed`
  (added in Task 6): with `allow cargo fmt --check` and `allow cargo clippy *`,
  `cargo fmt --check && cargo clippy` runs without asking, and `cargo fmt
  --check && cargo test` asks; under `allow echo *` and `allow rm *`, `echo
  $(rm -rf /)` asks and no rule is named. `a_session_rule_allows_its_exact_command_and_never_lifts_a_deny`
  shows a session approval of `echo $(rm -rf /)` is refused. **Met.**
- **Migration of the old keys.** `command_rule_equivalence::the_old_keys_and_the_migrated_rules_decide_the_same`
  replays the command policy's own test commands, 43 destructive spellings,
  the shell-parameter forms, the legacy-key commands and 217 commands recorded
  from real Damaian sessions through the old policy (kept behind the hidden
  `CommandPolicy::deciding_by_legacy_keys()`) and the migrated rules, with no
  old keys and with user, repository and Always-allow keys, at the root and a
  sub-root. The recorded set shows no difference. Every other difference is
  one of 20 listed groups, each with its reason, and the test fails if a
  listed one stops occurring: the D8/D9 fixes (`rm -fr /` and friends now
  blocked, wrapped commands checked, text inside another program's arguments
  no longer matching) and the migration effects of context.md D10. So the
  criterion's "every old decision unchanged" holds with those listed,
  intended exceptions, as Task 3's plan anticipated.
  `migrated_rules_keep_their_source_and_directory`,
  `the_old_keys_carry_a_deprecation_note` and
  `without_a_report_every_entry_is_the_users` cover sources, the note Settings
  shows and the fallback. **Met, with the listed differences.**
- **Documentation.** `docs/USER_GUIDE.md` (Command rules; Shell parameters in
  commands; Allow Always), `docs/TROUBLESHOOTING.md` (Reading an approval's
  cause; Command rules: what decided, and why a rule did not apply),
  `docs/CONFIGURATION.md` and `SECURITY.md` (Command Execution; Repository
  Config Trust Boundary) explain rules, defaults, overrides, sources, the
  migration, `command_shell_parameters` and how to read what decided.
  **Met.**

Requirements 9 and 10 beyond the criteria: *Always allow* writes an exact
user rule (`always_allow_writes_an_exact_user_rule_that_decides`) and Settings
lists, adds, edits, reorders and deletes rules (Task 5's route and UI tests).
Spec 109's **Approve for this session** is not built yet; `CommandRule::session`
is ready for it, and Settings will list session rules once that task feeds
them in.

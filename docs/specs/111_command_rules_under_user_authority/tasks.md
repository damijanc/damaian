# Spec 111 tasks

**Started:** not yet — specified 2026-11-12

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
| 1 · Parser: chains, words, known programs' options, paths (D3, D4) | Not started | — |
| 2 · Rules and evaluation with the trust limits (D1, D2, D5) | Not started | — |
| 3 · Migration of the old keys, with an equivalence corpus (D6) | Not started | — |
| 4 · Switch the policy to rules; cards and audit name the rule (D7) | Not started | — |
| 5 · Settings: the rule list, sources and editing | Not started | — |
| 6 · Documentation and close-out | Not started | — |

## Task 1 · Parser: chains, words, known programs' options, paths (D3, D4)

- [ ] Extend the spec 64 tokenizer to split chains into parts and to report
      "cannot analyse" for expansion, substitution, globs and here-docs.
- [ ] Known-program flag table (combined short flags, long forms, path
      arguments); options compare as sets for known programs.
- [ ] Lexical path resolution against the working directory.
- [ ] Tests: `rm -rf`, `-fr`, `-r -f`, `--recursive --force` parse equal;
      `/tmp/..` resolves to `/`; `$(…)`, `$HOME`, backticks, `*` are
      unanalysable; chains split correctly, quotes respected.

Files: `command_policy.rs` (or a new `command_parse.rs`), tests.
Checks: `cargo clippy -p workspace-engine --all-targets --locked -- -D warnings`;
`cargo nextest run -p workspace-engine --test command_policy` (or the policy
tests).

## Task 2 · Rules and evaluation with the trust limits (D1, D2, D5)

- [ ] `CommandRule { effect, pattern, directory, source }`; parsing of
      `command_rule` lines; pattern matching over parsed parts.
- [ ] Evaluation order and trust limits; repository `allow` is a config error;
      the built-in destructive defaults as `deny` rules over parsed commands.
- [ ] Tests: the acceptance examples for `rm -rf /tmp` and `rm -rf /`; a user
      `allow` overrides a built-in `deny`; a repository rule never widens; a
      session rule never overrides a user `deny`; any `deny` in a chain denies.

Files: `command_policy.rs`, `config.rs` (overlay classification for
`command_rule`), tests. Checks: as Task 1, plus the config overlay tests.

## Task 3 · Migration of the old keys, with an equivalence corpus (D6)

- [ ] Load `command_blocklist`, `command_allowlist`, Always-allow entries as
      rules; deprecation note.
- [ ] Equivalence corpus test: every command in the existing policy tests and
      a recorded set from real sessions decides the same under old and new
      evaluation (except the documented fixes: `rm -fr /` and friends now hit
      the built-in default).

Files: `config.rs`, `command_policy.rs`, tests. Checks: as Task 2.

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

# Feature Spec: Command Rules Under the User's Authority

Status: In progress (specified 2026-11-12; started 2026-11-12, Task 1 the
parser done; Tasks 1–6 in [tasks.md](tasks.md))
Order: 111 of 111
Plan: none. Written from the owner's design discussion on 2026-11-12 (spec 109
D5 "Approve for this session", and a comparison with OpenCode's permission
rules); the code survey and decisions are in [context.md](context.md).
Depends on: [#34](../34_repository_config_trust_boundary.md) (repository
config may only tighten), [#62](../62_approval_prompts_name_the_reason/proposal.md)
(approval causes), [#64](../64_read_only_command_boundary.md) (the read-only
boundary) — all built; **ready to start**. Spec
[#109](../109_fast_verification/proposal.md) Task 5 (Approve for this session)
builds on Task 1 here.

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

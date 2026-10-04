# Context: Permission Profiles

Background for [`tasks.md`](tasks.md). This file records corrections to
[`proposal.md`](proposal.md) (the flat spec, unchanged in substance) where its
"Current State" or design sections no longer match the code, checked on
2026-09-30. It also records the decisions the proposal leaves open. Read it
before Task 1. Do not "fix" any of these back to the proposal's wording without
re-reading the code they cite.

## 1. Spec 34 has already built most of §5.2

The proposal was written before [spec 34](../34_repository_config_trust_boundary.md)
shipped. Every file reference in its §1 and §2 is stale. Spec 34 implemented
the scope-aware overlay that §5.2's table describes, in
`crates/workspace-engine/src/config.rs`:

| Proposal cites | Now |
|---|---|
| `load_with_policy_paths` at `:218-239`, uniform last-writer-wins | `Config::load_scoped` (`:515`) applies user (strict parse), then repository (`parse_untrusted`, never fatal), then admin (strict), then `apply_repository_allowlist` |
| `apply_overlay` at `:259` | `apply_overlay` (`:587`) delegates to `apply_overlay_scoped(overlay, scope) -> Vec<RejectedConfigKey>` (`:596`) |
| No key is scope-restricted | `apply_overlay_scoped` destructures `ConfigOverlay` with no `..`, so an unclassified field fails to compile. Refusals are `RepositoryKeyClass { Forbidden, RestrictOnly, UserOwned, Unparsable }` (`:70`) |
| `active_mcp_servers` at `:475-488` | The intersection rule is `intersect_allowlist` (`:2648`) |
| Allowlist bypass at `command_policy.rs:107-117` | Closed. `command_allowlist` is `UserOwned`, and Allow Always writes `command_allowlist.<repository_id>` to user config (`validation.rs`, `allow_command_always`) |
| `effective_policy_for_repo` at `desktop-shell/src/lib.rs:220, :838, :863` | `lib.rs:1643`, `fn effective_policy_for_repo(repo) -> (String, String)`. It returns `Config::to_policy_text()` as text, plus an error string, used by `GET`/`POST /api/config-file` and `POST /api/model-key` |

Where the proposal's §5.2 table and the code disagree, **the code wins**,
because spec 34 is authoritative for the trust boundary (proposal §1):

- `allowed_roots` is **Forbidden** at repository scope, not intersected. Spec 34
  §5.1 explains why: a repository has no legitimate reason to know or narrow
  the user's roots.
- Spec 34 added a fourth merge shape the table does not have: **lower value
  wins**, for `max_read_lines`, `max_list_entries`, `max_search_matches`,
  `max_match_line_chars`, `command_timeout_secs`, `agent_max_turn_messages` and
  `agent_max_task_tokens`.
- `checkpoint_retention_days`, `checkpoint_max_total_bytes` and
  `checkpoint_census_max_paths` are Forbidden.
- A repository may define a new MCP server (created disabled and
  approval-gated), but redefining a server the user already has is Forbidden
  (spec 34 §5.1 amendment).
- Refusals are audited once per key per repository, in
  `RepositoryTrustStore::review` (`repository_trust.rs:79`), not at load.
  Config is loaded on every HTTP request.

**What this spec still has to build:** the named capability/preference
partition (§2 below), the profiles (§3–§4), `profile ∩ mode` (§5), source
attribution (§8), and export/import (§9). The trust-boundary tests in
`crates/workspace-engine/tests/repository_config_trust.rs` must keep passing
unchanged. None of these tasks may edit them.

## 2. The partition is spec 34's classes, named

§5.1 lists capability and preference keys by hand. The code already has a
finer classification, at repository scope, inside `apply_overlay_scoped`. The
two must not drift (acceptance criterion 2), so the partition is derived from
spec 34's classes by one rule:

> **Capability ⇔ repository scope does not apply the key freely.** Forbidden,
> Restrict-only and User-owned keys are capability keys. Free keys are
> preference keys.

That rule changes three things in §5.1's lists, and the proposal's lists are
the ones that are wrong:

- **`model_*` are capability keys**, not "model selection" preferences.
  `model_base_url` decides where the user's code and API key are sent, and
  spec 34 made all six `model_*` fields Forbidden for that reason.
- **The seven lower-wins caps are capability keys.** Spec 34 §5.2 explains
  why: they bound how much of the repository leaves the machine per call.
- **`checkpoint_*` are capability keys.**

The preference keys are exactly spec 34's Free block (`config.rs:944-965`):
`max_file_bytes`, `max_command_output_bytes`, `audit_retention_days`,
`enable_semantic_search`, `agent_max_tool_rounds`,
`agent_web_debug_max_tool_rounds` and `agent_tool_retry_limit`.

**Decision:** the partition is declared once, in `config.rs`, by a macro that
expands to an exhaustive destructure of `ConfigOverlay` **and** the list of
`(field, kind)` pairs. So the list cannot omit a field the destructure names,
and adding a field fails to compile until it is classified (Task 1). A test
then drives the *real* `apply_overlay_scoped` at repository scope with a
weakening value for every capability field and a sample value for every
preference field. It asserts that each capability key is refused or has no
weakening effect, and that each preference key applies. That test is what
enforces "agrees with spec 34" (criterion 2). Without it the macro would only
be a second hand-written list.

**Open, not decided here:** `audit_retention_days` is Free, so a cloned
repository can shorten how long the user's audit trail is kept. That is a
spec 34 classification question, not this spec's. Task 1 classifies it as
preference, to agree with the code, and records it as an open question in its
progress row. Do not reclassify it inside this spec.

## 3. The built-in profiles need keys that do not exist

§5.5 describes the profiles in terms of what they allow ("no writes, no
commands", "no network commands", "no browser diagnostics"). No config key
expresses any of those today. `require_approval_for_file_edits` only prompts,
and nothing turns commands, browser diagnostics or mutating MCP tools off. A
profile is "a named bundle of capability-key values" (§5.5), so the keys have
to exist first.

**Decision:** four new capability keys (Task 2). Each defaults to today's
behaviour, and each is Restrict-only at repository scope: a repository may
turn it down, never up.

| Key | Values | Default | Narrower means |
|---|---|---|---|
| `allow_file_edits` | `true`/`false` | `true` | `false`: no patch proposals, edits or stored-patch applies |
| `command_access` | `none`, `read_only`, `local`, `all` | `all` | Further left. `read_only` is the predicate Plan mode already uses (Low risk, no approval needed, `is_low_risk_read_only`). `local` is `all` minus any command `may_use_network` flags |
| `allow_browser_diagnostics` | `true`/`false` | `true` | `false`: no web diagnostic tools |
| `allow_mutating_mcp_tools` | `true`/`false` | `true` | `false`: only MCP tools with `readOnlyHint: true` |

The built-in profiles in those terms (Task 3). "Unchanged" means the profile
does not set the key:

| Profile | `allow_file_edits` | `command_access` | `allow_browser_diagnostics` | `allow_mutating_mcp_tools` | Other |
|---|---|---|---|---|---|
| Read-only | `false` | `none` | `false` | `false` | none |
| Safe local development | unchanged | `local` | unchanged | unchanged | `require_approval_for_file_edits=true` |
| Full repository development | unchanged | unchanged | unchanged | unchanged | none (the empty overlay) |
| Offline private | unchanged | `local` | `false` | unchanged | `mcp_enabled=false`; `audit_retention_days` and `checkpoint_retention_days` lowered to 7 |
| Custom | the user's own file (§4) | | | | |

Read-only matches Ask mode's tool set. That is deliberate, so "Code mode under
Read-only cannot edit" and "Ask mode under Full cannot edit" (criterion 10)
refuse the same things for different reasons.

## 4. Where a profile sits in the layering: last, and restrict-only

The proposal never says where a profile is applied relative to user,
repository and admin config, or whether a profile can widen anything.

**Decision:** the selected profile is applied **after admin, restrict-only**
(Task 3):

```text
effective config = restrict_only( admin( repository( user ) ), profile )
effective capability = effective config ∩ mode
```

Why this order, and what it buys:

- **Full repository development is the empty overlay**, so an upgrading user's
  behaviour is identical (§5.4, §5.5). The existing trust tests, including
  spec 34's `admin_config_can_still_widen_and_narrow`, are untouched.
- **A profile can only narrow.** So a profile, including an imported one,
  cannot reintroduce a weakenable path (criterion 1), and "deny always wins"
  (requirement 3) holds across the profile layer by construction.
- **Admin can still widen the base** (proposal §5.2), but cannot undo a
  narrower profile the user picked for themselves. A user narrowing their own
  session is not something installation policy needs to override.
- **Profiles carry only capability keys**, plus the two retention keys that
  Offline private lowers (lower-wins). A profile that sets a preference key or
  a Forbidden redirecting key (`shell`, `model_*`, `data_dir`) has that key
  ignored and reported. A profile narrows capability. It does not reconfigure
  Damaian.

**Selection** is stored in user config, keyed by repository the way Allow
Always is: `permission_profile.<repository_id>=<profile id>`. It is
**User-owned** at repository scope: a repository cannot choose its own
profile, in either direction. With no entry, the profile is Full repository
development.

**Custom profiles** are user-authored overlay files at
`<data_dir>/config/profiles/<id>.conf`. Built-in ids are reserved:
`read_only`, `safe_local`, `full`, `offline_private`. A custom id is
`[a-z0-9_]{1,40}` and may not collide with one. Import (§9) writes one of these
files.

This needs a new `ConfigScope::Profile`. `apply_overlay_scoped` currently
decides trust with `scope != ConfigScope::Repository` (`config.rs:649`).
Task 3 replaces that with an exhaustive `match` on the scope, so the fourth
variant has to be handled deliberately. It must not fall into "trusted" by
default.

## 5. `profile ∩ mode` touches `chat.rs`

The proposal's file references never name `chat.rs`, and
[`docs/specs/README.md`](../README.md) "Parallel work" treats that file as the
constraint on running two specs at once. But spec 20's permission check is
called from `chat.rs`, so any combined check is wired there:

- the tool-list filter (Layer 1), `chat.rs:1401-1520`;
- `action_permission`, `chat.rs:3015`, which is called for the main loop at
  `chat.rs:2046`;
- the three resume branches in `resume_after_command_decision_with_options`,
  `chat.rs:838/872` (web diagnostic), `:890/901` (MCP) and `:934/954`
  (command).

Outside `chat.rs`, the stored-patch gate is `refuse_unless_mode_permits_patches`
(`edit.rs:298`, called at `:350` and `:573`).

**Decision:** the combination lives in `mode.rs`, beside `mode_permits`, as one
function taking the mode and a `ProfileCapabilities` value read from `Config`.
The `chat.rs` edits are only calls to that function. They are all in **one
task (Task 5)**. It must not run at the same time as spec 22's Task 8 (renumbered from 7 on 2026-10-02), which
may also edit `chat.rs`. Check `tasks.md` in
[`22_findings_model_and_panel`](../22_findings_model_and_panel/tasks.md)
before starting it. Every other task in this spec leaves `chat.rs` alone.

A refusal must say which axis refused, profile or mode. "Not available in Ask
mode" is a different fix for the user than "not available under the Read-only
profile". `refusal_message` (`mode.rs:84`) gains the profile case rather than
a second message function.

## 6. "The next action" means the next decision point, as for mode

Requirement 6 and §5.8 say a profile change affects "the next action", and
§5.8 says it is "the same rule spec 20 §5.4 applies to mode, where a turn
captures its mode at start".

How the code actually behaves: the desktop shell builds a fresh `Config` and
`WorkspaceEngine` on every HTTP request (`engine_for_repo`, `lib.rs:2551`).
`CommandPolicy` and `PathPolicy` hold copies built at construction
(`workspace_engine.rs:52, 72`). So config is captured **per turn**, and again
**per resume**, because each resume is a new request with a new engine. That
is the same granularity as mode, which is read at `chat.rs:1395` and re-read
on resume at `chat.rs:828`.

**Decision:** use that granularity and do not re-read config inside a running
turn. A profile switch takes effect at the next turn start, the next resume
after an approval, and the next standalone `/api/run-command`. Any action
already running finishes under the profile it started with. So a turn already
in its tool loop keeps its profile until it pauses for approval or ends,
exactly as it keeps its mode. The acceptance criterion "switching to Read-only
mid-session does not interrupt an in-flight action but blocks the next one" is
tested at those decision points (Task 5). Re-reading per tool call would make
profile stricter than mode for no stated reason, and would cost a config load
per call.

## 7. What Read-only and "no network" cannot promise

Two open items in `docs/PLAN/OBSERVATIONS.md` (local-only) apply directly to
the profiles, and one heuristic applies to both "local" profiles:

- **Observation 11:** `git diff --output=<file>` passes `is_low_risk_read_only`.
  `command_access=read_only` reuses that predicate, so it inherits the gap. The
  built-in Read-only profile uses `none`, not `read_only`, so it does not.
  Custom profiles using `read_only` do. Do not fix the predicate here: that is
  a `command_policy.rs` classification change, which proposal §4 rules out.
- **Observation 10:** a rejected command proposal can still be run by id
  through `/api/run-command`, because `run_proposal` checks only `blocked` and
  `requires_approval`. So **`command_access` is enforced in `CommandPolicy` as
  a block** (Task 4), the same way `command_blocklist` is, and not only in the
  mode-layer check. A stored proposal run by id is re-classified under the
  current profile and refused. This sets `blocked` and a reason. It does not
  change risk, so proposal §4's "no change to risk classification" holds.
- **"No network" is a name heuristic.** `may_use_network`
  (`command_policy.rs:359`) matches a fixed list: curl, wget, npm, pnpm, yarn,
  pip, docker, and `git pull`/`push`/`fetch`/`clone`. `python -c` with
  `urllib` is not on it. `local` therefore means "no command Damaian
  recognises as networked". The profile description, the policy view and the
  user guide must say so in those words. This is not a sandbox, and nothing
  here may claim to be one.
- **Offline private does not stop model traffic.** The model request is
  network traffic, and `model_base_url` is Forbidden at every scope this spec
  controls. The policy view shows a warning when Offline private is selected
  and the active provider's base URL is not a loopback address. It does not
  refuse to run.

## 8. Attribution has no data to read yet

§5.7 asks for a source per rule. Nothing in the code records which scope set a
value: `Config` holds plain values, and `RepositoryConfigReport` records only
repository refusals, without values.

**Decision:** record provenance where values are applied, in
`apply_overlay_scoped`, and do not reconstruct it afterwards with a second
resolver (Task 6). A second resolver would drift from the real merge. The
function's return type grows from `Vec<RejectedConfigKey>` to an outcome that
also lists applied keys with their scope. List keys (`restricted_patterns`,
`ignore_patterns`, `command_blocklist`, `command_allowlist`) record a source
**per entry**, because §5.7's example attributes `.env` and `secrets/**`
separately. Admin values that loosen what the earlier scopes resolved are
marked as widenings (criterion 5).

**Refused requests show key and class, never the value.** §5.7's example says
"repository config requested false — ignored". Spec 34 §5.3 wins on this
point: a refused value is attacker-controlled text. The view says the
repository tried to loosen `require_approval_for_file_edits`, which carries the
same information without the value. The one exception is spec 34's own
itemised allowlist migration, which already shows exact commands.

## 9. Export and import

- **The overlay serializer drops keys.** `ConfigOverlay::to_policy_text`
  (`config.rs:1975`) never writes `command_timeout_secs` or
  `agent_max_turn_messages`, although `set` parses both. That was a live bug
  outside this spec: any `save()` of user config, including Allow Always,
  erased them. **Fixed on 2026-09-30, while this spec was being planned.**
  `to_policy_text` now destructures `ConfigOverlay` with no `..`. A new field
  fails to compile, and a field bound but never written is an unused-variable
  error under the lint gate. The fix is pinned by
  `command_timeout_and_turn_message_window_survive_a_round_trip_through_the_overlay_text`
  (`foundation.rs`) and `allow_always_keeps_the_users_other_settings`
  (`repository_config_trust.rs`). Export (Task 8) reuses this serializer and
  keeps its own round-trip test.
- **An export contains capability keys only.** MCP server definitions, the
  only place `auth_token_env` lives, are not profile keys. `model_api_key_env`
  is Forbidden in a profile (§4). So "no `auth_token_env` or
  `model_api_key_env` value" (criterion 11) holds by construction. The test
  asserts it against the text anyway.
- **"Widenings require review" becomes "widenings are listed and not
  applied".** An imported profile is a Custom profile, and a profile can only
  narrow (§4). So an imported key that would loosen the resolved config has no
  effect when the profile is selected. Import reports those keys, itemised,
  before it writes the file, and audits the import. There is nothing to
  approve: approving a widening would need a profile that can widen, and §4
  rules that out. This is stricter than proposal §5.9, and it matches that
  section's intent: "an imported profile is not more trusted for having been
  exported by Damaian".

## 10. Things the proposal says that were checked and hold

- Admin config is `$DAMAIAN_ADMIN_CONFIG` or `<data_dir>/config/admin.conf`
  (`config.rs:472-477`), parsed strictly. It is trusted, so it both widens and
  narrows, as the test `admin_config_can_still_widen_and_narrow` pins.
- `SessionMode { Ask, Plan, Code, Review }` and
  `mode_permits(mode, &ToolAction, Option<&CommandClassification>, Option<bool>) -> Permission`
  are in `mode.rs:12, 126`. The matrix test is
  `the_permission_matrix_matches_the_spec_table` (`mode.rs:259`). There is no
  separate tool-class enum: the tool class is the `ToolAction` variant.
- `approval_policy_violations` is computed in
  `crates/eval-harness/src/runner.rs:425-445`, from `stored_command_rejected`
  followed by `stored_command_executed` for the same proposal id. Task 9 reads
  it for criterion 14.
- The effective policy is rendered by `renderConfigPolicy` in
  `crates/desktop-shell/static/app.js`, into `<pre id="config-output">` on
  Settings › General. Desktop settings write user config only
  (`desktop_settings_config_path`, `lib.rs:1632`).
- The CLI has `config-show [repo]`, which prints `Config::to_policy_text()`,
  and `config-set <user|repo|admin>`. It has no `policy` or `profile`
  subcommand.
- `command_allowlist` remains exact-command (criterion 13). No task here adds
  matching to it.

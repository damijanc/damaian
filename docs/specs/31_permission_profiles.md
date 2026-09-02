# Feature Spec: Permission Profiles

Status: Not started
Order: 31 of 33
Roadmap: `docs/ROADMAP/04_phase_4_customization_and_extensibility.md`, Phase 4,
Work Package 3 (Must). That directory is local-only and not committed, so the
reference is a name rather than a link; this spec is self-contained.
Related spec sections: `ai_coding_assistant_specification.md` section 7.3 (path
and secret policy), section 7.4 (command approval), section 7.8 (risk
classification and approval). Related implementation specs:
[`10_persistent_command_approval.md`](10_persistent_command_approval.md) (wrote
`command_allowlist` to repository config — §1 explains why that mattered here;
[spec 34](34_repository_config_trust_boundary.md) has since moved it to user
config, and §1's live defect is closed),
[`11_agents_md_support.md`](11_agents_md_support.md) (repository content is
untrusted),
[`13_docker_command_support.md`](13_docker_command_support.md),
[`20_working_modes.md`](20_working_modes.md) (mode narrows a profile; the
permission matrix this extends),
[`32_hooks.md`](32_hooks.md) and
[`33_mcp_management_and_deferred_discovery.md`](33_mcp_management_and_deferred_discovery.md)
(both reference "the effective profile" this spec defines).

Implementation order note: the roadmap lists this as WP3, after WP2 (hooks).
It is specified first because WP2 requirement 6 ("hooks cannot expand the active
mode or the permission profile") and WP4 requirement 7 ("unless a user profile
specifically allows them") both depend on a profile existing. The thing that
cannot be widened has to exist before the things that cannot widen it.

## 1. Motivation

**Repository configuration can currently weaken user-level protections, and one
consequence is silent command execution on clone.**

The config layering applies overlays in order user → repo → admin
(`crates/workspace-engine/src/config.rs:218-239`), and `apply_overlay`
(`config.rs:259`) is uniform last-writer-wins for every field. There is no
scope-based key filtering and no test asserting otherwise. So a repository's
`.damaian/config.conf` overrides the user's settings for every key it sets —
including `restricted_patterns`, `command_blocklist`, `allowed_roots`,
`require_approval_for_*`, and `data_dir`.

The sharpest case is `command_allowlist`. `CommandPolicy::classify_pattern`
(`command_policy.rs:107-117`) treats an exact allowlist match as
`risk: Low, blocked: false, requires_approval: require_approval_for_all_commands`
— which defaults to false. An allowlisted command therefore **runs without
approval**. `command_allowlist` is settable in repository config, that file
travels with a clone, and nothing asks the user before honouring it.

Cloning a repository whose committed `.damaian/config.conf` carries a
`command_allowlist` therefore grants no-approval command execution on the user's
machine. Two existing gates limit the blast radius and neither closes it:
`contains_shell_control` runs before the allowlist check and the on-disk format
is pipe-separated, so a piped or chained command cannot be allowlisted
(`command_policy.rs:243-247`); and hard-blocked commands are rejected first. A
single unchained command is still enough — `npm install` runs postinstall
scripts, `make` runs a Makefile the repository controls, `curl -o` writes a file.

This is inconsistent with the product's own stance elsewhere.
[Spec 11](11_agents_md_support.md) and [spec 20](20_working_modes.md) §5.5 both
establish that repository content is untrusted with respect to capability;
`AGENTS.md` cannot widen a mode. Repository *config* is treated as trusted, and
it is the same threat model with a different filename.

Requirement 4 of this work package — "repository configuration cannot weaken a
user-level deny" — is therefore not a new feature layered on a sound base. It is
a fix.

**That fix has been extracted into
[spec 34](34_repository_config_trust_boundary.md), to be implemented ahead of
this phase.** Investigating the weakness found two vectors worse than the
`command_allowlist` one — a repository-set `shell` executing its own script for
every approved command, and a repository-set `model_base_url` exfiltrating the
user's API key and code with no approval step — which made waiting for Phase 4
untenable. Spec 34 delivers the scope-aware overlay, the forbidden-key list, the
restrict-only merges, and the move of `Allow Always` to user scope.

This spec therefore assumes that boundary exists and builds the rest of the work
package on it: named profiles, the capability/preference partition as a general
mechanism, source attribution in the effective-policy view, the intersection with
mode, and export/import. Where the two overlap, spec 34 is authoritative for the
trust boundary and this spec for the profile machinery.

The second motivation is legibility. `restricted_patterns`,
`command_allowlist`, `command_blocklist`, `allowed_roots`, `ignore_patterns`, and
three `require_approval_for_*` flags are flat values spread across three files. A
user cannot answer "what is this session allowed to do, and who decided?" — and
this work package's validation criterion is that someone who did not build the
system can read the resolved state.

## 2. Current State

- **Overlays are uniform last-writer-wins, user → repo → admin.**
  `Config::load_with_policy_paths` (`config.rs:218-239`) applies user, then
  repository, then admin. `apply_overlay` (`config.rs:259`) assigns each
  `Some(...)` field over the current value with no regard for scope.
- **No key is scope-restricted.** `ConfigOverlay::set` (`config.rs:777`) accepts
  the same key set regardless of which file it is writing, and nothing filters
  by scope on load. There is no test asserting that a repository cannot weaken a
  user setting.
- **Admin is last and therefore wins**, which is the one part of the ordering
  that is correct: `DAMAIAN_ADMIN_CONFIG` or
  `<data_dir>/config/admin.conf` (`config.rs:243-250`).
- **An allowlist match bypasses approval.** `command_policy.rs:107-117`, as
  described in §1. `allow_always_eligible` (`command_policy.rs:251`) governs
  whether the UI *offers* `Allow Always`, not whether an entry already on disk
  is honoured.
- **Repository config is a real file that travels with a clone**, at
  `<repo>/.damaian/config.conf` (`Config::repository_config_path`,
  `config.rs:251-256`), and [spec 10](10_persistent_command_approval.md) writes
  to it deliberately.
- **Policy values are flat and spread across two modules.**
  `command_policy.rs` enforces `command_allowlist`, `command_blocklist`, and the
  three `require_approval_for_*` flags. `path_policy.rs` enforces
  `allowed_roots`, `ignore_patterns`, `restricted_patterns`, exposing
  `is_restricted` (`path_policy.rs:138`) and `assert_not_restricted`
  (`path_policy.rs:142`).
- **An effective-policy surface already exists** and is rendered per repository:
  `effective_policy_for_repo` is called from several endpoints in
  `crates/desktop-shell/src/lib.rs` (`:220`, `:838`, `:863`) and returned as
  `effectivePolicy`. It shows resolved values with **no source attribution**.
- **MCP has a narrowing precedent worth copying.** `Config::active_mcp_servers`
  (`config.rs:475-488`) requires the global switch, the server's own `enabled`,
  **and** membership of `mcp_server_allowlist` when that list is non-empty — an
  intersection rather than a last-writer-wins override.
- **Mode exists as a capability boundary** ([spec 20](20_working_modes.md)) with
  a permission matrix as its primary artifact.

## 3. Requirements

1. Profiles build on the existing `ConfigOverlay` and user/repo/admin scopes
   rather than adding a parallel configuration system.
2. The effective resolved policy is displayed, along with where each rule came
   from.
3. More specific deny rules override allow rules. Deny always wins.
4. **Repository configuration cannot weaken a user-level deny.**
5. Mode ([spec 20](20_working_modes.md)) narrows a profile and can never widen
   one. The effective capability is the intersection.
6. Profile changes affect new actions, not actions already executing.
7. Sanitized profile configuration can be exported and imported.

## 4. Non-goals

- Organization-wide or remotely managed policy — Phase 6 WP6. `admin.conf`
  remains a local file.
- Per-tool MCP permissions — [spec 33](33_mcp_management_and_deferred_discovery.md).
- Hook permissions — [spec 32](32_hooks.md).
- Replacing modes. Mode and profile are different axes: a profile is what the
  installation permits, mode is what this session is doing.
  §5.6 defines the intersection.
- Adding pattern-matching to `command_allowlist`. It stays exact-command, per
  [spec 10](10_persistent_command_approval.md); a profile must not introduce
  globbing by the back door.
- Changing risk classification in `command_policy.rs`.
- A migration that discards existing repository config. §5.4 handles existing
  files by asking, not by deleting.
- Per-directory profiles within one repository.

## 5. Design

### 5.1 Capability keys versus preference keys

The fix for requirement 4 is to stop treating every config key the same way.
Keys are partitioned, and the partition is declared in one place:

**Capability keys** — can grant or remove the ability to read, write, execute, or
reach the network:

`allowed_roots`, `ignore_patterns`, `restricted_patterns`,
`command_allowlist`, `command_blocklist`, `require_approval_for_file_edits`,
`require_approval_for_risky_commands`, `require_approval_for_all_commands`,
`mcp_enabled`, `mcp_server_allowlist`, per-server `enabled` and
`require_approval`, `data_dir`, and every profile field added by
[spec 32](32_hooks.md) or
[spec 33](33_mcp_management_and_deferred_discovery.md).

**Preference keys** — everything else: `max_file_bytes`,
`max_command_output_bytes`, `agent_max_tool_rounds`, model selection, UI
settings, retention windows.

Preference keys keep last-writer-wins semantics, so nothing about ordinary
configuration changes. Capability keys get the merge rules in §5.2.

A new key defaults to **capability** until classified otherwise. Adding a key
that silently defaults to weakenable-by-repository is exactly the mistake this
partition exists to prevent, so the safe default is the restrictive one, enforced
by an exhaustive match that fails to compile when a field is added without a
classification.

### 5.2 Merge rules: restrictions accumulate, permissions intersect

For capability keys, the user scope sets a floor that repository config cannot
lower:

| Key | Merge across user → repo | Rationale |
|---|---|---|
| `restricted_patterns` | **Union** | Either scope may add a restriction |
| `ignore_patterns` | **Union** | Same |
| `command_blocklist` | **Union** | Same |
| `allowed_roots` | **Intersection** | A repository cannot widen readable paths |
| `require_approval_for_*` | **Logical OR** | A scope may turn approval on, never off |
| `mcp_enabled`, per-server `enabled` | **Logical AND** | A repository may disable a server, never enable one |
| `mcp_server_allowlist` | **Intersection** when both non-empty | Matches `active_mcp_servers` (`config.rs:475`) |
| `command_allowlist` | **Never taken from repo scope** — [spec 34](34_repository_config_trust_boundary.md) §5.4 | An allowlist entry bypasses approval |
| `shell`, `data_dir`, `model_*`, `secret_patterns`, `audit_enabled`, `block_generated_secrets` | **Repo value ignored entirely** — [spec 34](34_repository_config_trust_boundary.md) §5.1 | Redirect execution, model traffic, credentials, redaction, or the audit trail |

Admin remains applied last and **may both widen and narrow**. That is the
existing behaviour and the correct one: `admin.conf` is a local file owned by
whoever administers the machine, is not carried by a clone, and exists precisely
to express installation policy. Its ability to widen is documented rather than
removed, and every admin-sourced widening is attributed in the policy view
(§5.5) so it is visible rather than assumed.

Requirement 3 — "deny always wins" — is this table read as a whole: every rule
resolves toward the more restrictive outcome, and there is no key on which a
later scope can remove an earlier scope's restriction except admin.

### 5.3 The trust boundary comes from spec 34

Superseded, and **now shipped**. [Spec 34](34_repository_config_trust_boundary.md)
owns the repository config trust boundary and implemented it ahead of this
phase: the scope-aware
overlay (§5.1 there), the forbidden-key list, the restrict-only merges, and the
move of `command_allowlist` to user config keyed by repository (§5.4 there).

An earlier draft of this spec proposed tracking the repository config file's
content hash so entries Damaian wrote could be distinguished from entries that
arrived with a clone, then presenting the rest for itemised review. For
`command_allowlist` that is unnecessary: storing the user's own approval decision
in user config removes the ambiguity entirely rather than resolving it after the
fact. The hash-and-review flow survives only for the keys a repository may
legitimately set and where a user might want to see what changed — it is not a
gate on anything unsafe, because unsafe keys are rejected outright.

What this spec adds on top is the *general* mechanism: the capability/preference
partition applied to every future config field, and the source attribution in
§5.7 that makes the resolved state legible.

### 5.4 Existing repository config

Owned by [spec 34](34_repository_config_trust_boundary.md) §5.5, which migrates
existing `command_allowlist` entries into user config through a one-time itemised
prompt and leaves every repository file untouched. By the time this work package
is implemented, that migration has already run.

What remains here is narrower: a profile assignment for a repository the user has
not chosen one for. Existing repositories default to **Full repository
development** (§5.5), which reproduces today's behaviour exactly, so adopting
profiles changes nothing until a user picks a different one.

### 5.5 Built-in profiles

| Profile | Intent |
|---|---|
| **Read-only** | Reads and explanations. No writes, no commands |
| **Safe local development** | Writes with approval; read-only commands automatic; no network commands |
| **Full repository development** | Current default behaviour |
| **Offline private** | No network, no MCP, no browser diagnostics, minimal retention |
| **Custom** | Explicit per-key configuration |

A profile is a named bundle of capability-key values stored as a
`ConfigOverlay`, per requirement 1 — not a new type of file. `Full repository
development` reproduces today's defaults exactly, so an upgrading user's
behaviour does not change until they choose otherwise.

### 5.6 Profile, mode, and the intersection

Requirement 5. Two independent narrowing axes:

```text
effective capability = profile ∩ mode
```

The profile says what this installation permits; the mode
([spec 20](20_working_modes.md)) says what this session is doing. Ask mode under
`Full repository development` cannot edit files; Code mode under `Read-only`
cannot either. Neither can widen the other.

[Spec 20](20_working_modes.md)'s permission matrix gains the profile dimension,
and the matrix test extends rather than duplicates — that matrix is named as
that work package's primary artifact, and this one adds an axis to it.

### 5.7 The effective policy view gains attribution

Requirement 2 extends the existing surface rather than adding one.
`effective_policy_for_repo` (`desktop-shell/src/lib.rs:220`) already resolves and
returns policy; it gains a source per rule:

```text
Effective policy — Safe local development ∩ Code mode

  restricted_patterns    .env, *.pem          user profile
                         secrets/**           repository config
  allowed_roots          ~/work/damaian       user profile ∩ repository config
  command_allowlist      cargo test           this repository (Allow Always)
                         npm ci               repository config — NOT APPLIED,
                                              pending your review
  require_approval_for_file_edits  true       user profile (repository config
                                              requested false — ignored)
  mcp_enabled            false                Offline private profile
```

Two things this must show, because they are the whole point: a rule's **source**,
and a **request that was refused**. A repository asking for something it did not
get is more informative than the resolved value alone — it tells the user
something about the repository.

Requirement 12's validation criterion is that someone who did not build this can
read the view, so it is reviewed by a second person before the work package is
closed.

### 5.8 Profile changes mid-session

Requirement 6. A profile change takes effect for the next action, not the
executing one — the same rule [spec 20](20_working_modes.md) §5.4 applies to
mode, where a turn captures its mode at start.

An action already running to completion under the old profile is not aborted:
interrupting a patch application or a command mid-flight creates exactly the
unknown-outcome state [spec 17](17_durable_task_state_and_crash_recovery.md)
exists to avoid. The next action is evaluated under the new profile.

### 5.9 Export and import

Requirement 7. Export writes the profile's capability-key values as a
`ConfigOverlay` file, with `auth_token_env` and `model_api_key_env` reference
values **excluded** rather than exported — they are references
(`keychain:<account>`) rather than secrets, but a reference names an account, and
an exported profile is a file people paste into issues.

Import treats the file as untrusted, on the same footing as §5.3: restrictions
apply, widenings are itemised for review. An imported profile is not more trusted
for having been exported by Damaian.

### 5.10 Documentation

`docs/USER_GUIDE.md`: the profiles, how profile and mode intersect, how to read
the effective policy view, and what the repository-config review prompt is asking.
`docs/TROUBLESHOOTING.md`: why a rule is in force, why a repository's config entry
was not applied, and how to review or re-review a repository's configuration.
`SECURITY.md`: the repository-config trust boundary, alongside the existing
secret, command, path, and key boundaries.

## 6. Acceptance Criteria

- The trust-boundary criteria are [spec 34](34_repository_config_trust_boundary.md)'s
  and are not restated here. This work package must not regress them: its
  matrix test runs with a hostile fixture repository present, so a profile that
  reintroduced a weakenable path would fail.
- The capability/preference partition covers every `ConfigOverlay` field, and the
  classification agrees with [spec 34](34_repository_config_trust_boundary.md)
  §5.1 for every field that spec classifies — asserted by test, so the two
  cannot drift.
- Repository config that only *adds* restrictions applies immediately with no
  prompt.
- Every refused widening is audited.
- Admin config can both widen and narrow, and every admin-sourced widening is
  attributed in the policy view.
- The effective policy view names the source of every rule, and shows requests
  that were refused alongside the resolved value.
- A second person, who did not build it, can read the effective policy view and
  correctly state what the session may do.
- Adding a new config field without classifying it as capability or preference
  fails to compile.
- The permission matrix passes across every profile crossed with every tool
  class, extending [spec 20](20_working_modes.md)'s matrix rather than
  duplicating it.
- `profile ∩ mode` holds in both directions: Ask mode under the most permissive
  profile cannot edit; Code mode under Read-only cannot edit.
- Switching to Read-only mid-session does not interrupt an in-flight action but
  blocks the next one.
- An exported profile contains no `auth_token_env` or `model_api_key_env` value,
  and an imported profile's widenings require review.
- `command_allowlist` remains exact-command; no profile introduces pattern
  matching.
- The five quality-gate commands from `AGENTS.md` pass, and the
  [spec 18](18_local_evaluation_harness.md) baseline shows no increase in
  approval-policy violations.

## 7. Implementation Notes

To be completed during implementation.

The §1 finding was extracted into
[spec 34](34_repository_config_trust_boundary.md) and is implemented ahead of this
phase, so this work package starts from a codebase where the trust boundary
already holds. Confirm that before starting: if spec 34 has not landed, the
matrix test here will pass while the underlying weakness remains, which is the
worst of both outcomes.

Also record:

- How many repositories in real use turned out to carry a widening entry, and
  what it was. The answer indicates whether the review prompt is a rare event or
  a routine annoyance.
- Whether the second-person review of the policy view (§5.7) actually passed, and
  what they misread if it did not. That is the work package's real acceptance
  test.

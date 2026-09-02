# Feature Spec: Repository Config Trust Boundary

Status: Not started
Order: 34 of 34
Priority: **Implement ahead of the roadmap queue.** This is a bug-driven spec,
not a roadmap graduation — like [`07_generated_secret_override.md`](07_generated_secret_override.md)
(reported bug), [`09_release_quality_gate.md`](09_release_quality_gate.md)
(defect found in CI), and [`10_persistent_command_approval.md`](10_persistent_command_approval.md)
(usability with a safety edge). Its number is last because numbers are assigned
in creation order; its implementation order is first.
Related spec sections: `ai_coding_assistant_specification.md` section 7.3 (path
and secret policy), section 7.4 (command approval), section 7.8 (risk
classification and approval). Related implementation specs:
[`10_persistent_command_approval.md`](10_persistent_command_approval.md) (§5.4
changes where its allowlist entries are stored),
[`11_agents_md_support.md`](11_agents_md_support.md) and
[`20_working_modes.md`](20_working_modes.md) (repository content is untrusted for
capability — this extends the same rule to repository *config*),
[`13_docker_command_support.md`](13_docker_command_support.md),
[`31_permission_profiles.md`](31_permission_profiles.md) (this spec is the
security subset of its requirement 4, extracted so it need not wait for
Phase 4; §5.3 of that spec is superseded by §5.4 here).
See also [`SECURITY.md`](../../SECURITY.md).

## 1. Motivation

Damaian treats repository configuration as trusted. It arrives with a clone.

`Config::load_with_policy_paths` (`crates/workspace-engine/src/config.rs:218-239`)
applies three overlays in order — user, repository, admin — by calling
`apply_overlay` (`config.rs:259`), which assigns every `Some(...)` field over the
current value. The function takes no scope parameter and behaves identically for
all three. Repository config is the only one of the three that comes from an
untrusted source, and nothing distinguishes it.

So a repository shipping `.damaian/config.conf` overrides the user's settings for
every key it sets. Three consequences are severe.

**1. `shell` gives arbitrary code execution on any approved command.**
`CommandRunner` runs
`Command::new(&self.config.shell).arg("-lc").arg(command).current_dir(cwd)`
(`crates/workspace-engine/src/command_runner.rs:89-93`), and `shell` is settable
from an overlay (`config.rs:820`). On Unix a relative program path resolves
against `current_dir`, which was verified rather than assumed:

```text
Command::new("./tools/sh").arg("-lc").arg("echo hello").current_dir("repo")
→ HIJACKED: repo-controlled shell ran: -lc echo hello   status=Some(0)
```

A repository with `shell=./tools/sh` and a committed `tools/sh` therefore
executes its own script for **every command the user approves**. The real command
arrives as an argument the script can ignore. This needs no allowlist entry: it
hijacks legitimate approvals, so the user's caution provides no protection.

**2. `model_base_url` with `model_api_key_env` exfiltrates the key and the code,
with no approval step at all.** Both are settable from an overlay
(`config.rs:259` assignments; keys at `config.rs:777-900`). A repository setting
`model_base_url` to its own endpoint, leaving `model_api_key_env` pointing at the
user's Keychain reference, receives the user's API key and their repository
content on every turn. No command runs and no prompt appears.
`docs/MACOS_INSTALLATION.md` documents the override as intended:
"If repository config sets `model_api_key_env`, it overrides the user Keychain
reference." `model_providers` allows the same through a provider entry.

**3. `command_allowlist` gives no-approval command execution.**
`CommandPolicy::classify_pattern` (`command_policy.rs:107-117`) treats an exact
allowlist match as `risk: Low, blocked: false,
requires_approval: require_approval_for_all_commands` — false by default. Two
existing gates narrow this and neither closes it: `contains_shell_control` runs
first and the pipe-separated on-disk format prevents chained commands
(`command_policy.rs:243-247`), and hard-blocked commands are rejected earlier. A
single unchained command is still enough: `npm install` runs postinstall scripts,
`make` runs a repository-controlled Makefile.

And a weakening set that makes the above quieter: `restricted_patterns` and
`ignore_patterns` (read `.env`), `secret_patterns` (defeat redaction),
`command_blocklist` (unblock destructive commands), the three
`require_approval_for_*` flags, `block_generated_secrets` (defeats
[spec 07](07_generated_secret_override.md)), `allowed_roots`, `data_dir`, and
**`audit_enabled`** — which hides the trail while any of the rest happens.

This is inconsistent with the product's own position.
[Spec 11](11_agents_md_support.md) and [spec 20](20_working_modes.md) §5.5
establish that repository *content* is untrusted with respect to capability —
`AGENTS.md` cannot widen a mode. Repository *config* is the same threat model
with a different filename, and it is trusted.

[Spec 31](31_permission_profiles.md) fixes this as part of Phase 4's permission
profiles. Phase 4 is five phases out. This spec extracts the security subset so
it can ship on its own, without the profile machinery.

## 2. Current State

- **`apply_overlay` is scope-blind.** `config.rs:259` onward assigns each
  `Some(...)` field with no notion of origin. `load_with_policy_paths`
  (`config.rs:218-239`) calls it for user, then repository, then admin.
- **Admin last is correct.** `DAMAIAN_ADMIN_CONFIG` or
  `<data_dir>/config/admin.conf` (`config.rs:243-250`) is a local file, not
  carried by a clone.
- **Repository config path**: `<repo>/.damaian/config.conf`
  (`Config::repository_config_path`, `config.rs:251-256`).
- **Every overlay field is repo-settable.** `ConfigOverlay` carries `data_dir`,
  `max_file_bytes`, `max_command_output_bytes`, `allowed_roots`,
  `ignore_patterns`, `restricted_patterns`, `command_allowlist`,
  `command_blocklist`, `secret_patterns`, the three `require_approval_for_*`,
  `block_generated_secrets`, `audit_enabled`, `audit_retention_days`,
  `enable_semantic_search`, `agent_max_tool_rounds`,
  `agent_web_debug_max_tool_rounds`, `agent_tool_retry_limit`, `shell`,
  `model_provider`, `model_name`, `model_base_url`, `model_api_key_env`,
  `model_reasoning_level`, `model_providers`, `mcp_enabled`,
  `mcp_server_allowlist`, and `mcp_servers`. `ConfigOverlay::set`
  (`config.rs:777`) accepts the same keys regardless of destination file.
- **No test asserts the boundary.** No test in the workspace checks that a
  repository overlay cannot weaken a user overlay.
- **`Allow Always` writes to repository config**
  ([spec 10](10_persistent_command_approval.md)), which is why
  `command_allowlist` cannot simply be ignored there.
- **MCP already has the correct pattern.** `Config::active_mcp_servers`
  (`config.rs:475-488`) intersects the global switch, the server's `enabled`, and
  `mcp_server_allowlist` rather than letting a later scope override an earlier
  one.
- **`AuditLog::record`** (`config.rs` neighbour `audit.rs:42`) redacts field
  values and is the mechanism for recording refused keys.

## 3. Requirements

1. Repository configuration cannot weaken any user- or admin-level restriction.
2. Repository configuration cannot set a key that redirects execution, model
   traffic, credentials, or data location.
3. Repository configuration can still *add* restrictions, and those apply with no
   prompt.
4. `Allow Always` ([spec 10](10_persistent_command_approval.md)) continues to
   work from the user's point of view.
5. A repository-sourced `command_allowlist` entry that Damaian did not write is
   never honoured.
6. Every rejected repository-sourced key is audited.
7. Admin configuration retains its current ability to both widen and narrow.
8. Existing repositories keep working: no config file is rewritten or deleted,
   and no user loses an allowlist entry they created.
9. A test asserts the boundary against a fixture repository that attempts to set
   every capability key to its most permissive value.

## 4. Non-goals

- Permission profiles, the capability/preference partition as a general
  mechanism, the effective-policy source attribution, and profile export/import
  — all [spec 31](31_permission_profiles.md), Phase 4. This spec is the security
  subset only.
- A repository-config review UI. §5.3 rejects unsafe keys outright rather than
  offering them for approval; the itemised review flow is
  [spec 31](31_permission_profiles.md).
- Changing risk classification, the blocklist, or shell-control detection in
  `command_policy.rs`.
- Changing what `AGENTS.md` can do — already correct.
- Sandboxing command execution.
- Signing or verifying repository configuration.
- Removing admin config's ability to widen.

## 5. Design

### 5.1 A scope parameter, and one classification

```rust
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConfigScope {
    User,
    /// Untrusted: arrives with a clone.
    Repository,
    Admin,
}

impl Config {
    pub fn apply_overlay_scoped(&mut self, overlay: ConfigOverlay, scope: ConfigScope);
}
```

`apply_overlay` keeps its current signature and behaviour by delegating with
`ConfigScope::User`, so existing callers and tests are unaffected.
`load_with_policy_paths` passes the real scope for each of its three overlays.

Every field is classified, in one exhaustive match so that **adding a field
without classifying it fails to compile**. That property is the difference
between a fix and a fix that decays: the next field added to `ConfigOverlay`
would otherwise default to repo-writable.

| Class | Repository behaviour | Fields |
|---|---|---|
| **Forbidden** | Ignored, audited | `shell`, `data_dir`, `model_provider`, `model_name`, `model_base_url`, `model_api_key_env`, `model_reasoning_level`, `model_providers`, `secret_patterns`, `audit_enabled`, `block_generated_secrets`, `allowed_roots` |
| **Restrict-only** | Applied only in the more restrictive direction | `restricted_patterns`, `ignore_patterns`, `command_blocklist` (union); `require_approval_for_file_edits`, `require_approval_for_risky_commands`, `require_approval_for_all_commands` (logical OR); `mcp_enabled`, per-server `enabled` (logical AND); `mcp_server_allowlist` (intersection when both non-empty) |
| **User-owned** | Never taken from repository config — §5.4 | `command_allowlist` |
| **Free** | Applied as today | `max_file_bytes`, `max_command_output_bytes`, `audit_retention_days`, `enable_semantic_search`, `agent_max_tool_rounds`, `agent_web_debug_max_tool_rounds`, `agent_tool_retry_limit`, `mcp_servers` definitions (subject to the restrict-only enable rules above) |

`allowed_roots` is **Forbidden** rather than intersect-only, because a repository
setting it can only be an attempt to change which paths are reachable, and a
repository has no legitimate reason to know or narrow the user's roots. Narrowing
what is readable *within* the repository is what `restricted_patterns` and
`ignore_patterns` are for, and those remain available.

`mcp_servers` definitions stay free — a repository suggesting a useful MCP server
is legitimate — but a repository cannot *enable* one (`enabled` is
logical AND, and a new server's default is off per `config.rs:168-169`), and
cannot clear its `require_approval`. So a suggested server is inert until the
user turns it on.

### 5.2 Restrict-only merges

The three merge shapes, all resolving toward the more restrictive outcome:

- **Union** — the repository's patterns are appended to the user's, deduplicated.
  Either scope may add a restriction.
- **Logical OR** — a repository may set an approval flag to `true`. A `false`
  from repository scope is ignored, not applied.
- **Logical AND / intersection** — a repository may disable MCP or a server;
  enabling is ignored. This is the pattern `active_mcp_servers`
  (`config.rs:475-488`) already uses.

Requirement 3 is satisfied by these applying immediately with no prompt. A
repository asking for *less* capability needs no defence, and prompting for it
would train users to click through the prompt that matters.

### 5.3 Rejection is silent to the model, visible to the user

A forbidden key in repository config is ignored, and:

- audited through `AuditLog::record` as a rejected-config-key event with the key
  name, the repository, and the class — never the value, since a rejected
  `model_api_key_env` or `shell` value is attacker-controlled text;
- surfaced once per repository in the UI as a notice naming the keys, because a
  repository attempting to set `shell` or `model_base_url` is information the
  user should have about that repository.

The notice is informational, not an approval prompt. There is deliberately no
"allow anyway" for the Forbidden class: none of those keys has a legitimate
repository use case, so an override would exist only to be socially engineered.

### 5.4 `command_allowlist` moves to user scope

This is the part that closes vector 3 without breaking
[spec 10](10_persistent_command_approval.md), and it **supersedes
[spec 31](31_permission_profiles.md) §5.3's** content-hash-tracking approach for
this key.

An `Allow Always` decision is *the user's* decision about a repository. It
therefore belongs in user config, keyed by repository:

```text
# <data_dir>/config/user.conf
command_allowlist.repo_9f2c1a7b3d4e5f60=cargo test|npm ci
```

The key is the existing `repository_id` (`indexer.rs:382-385`,
`sha256(canonical path)[..16]`), which is the right identity here precisely
because it is per checkout: an approval granted for a repository at one path
should not silently transfer to a different checkout the user has not looked at.
(This is the opposite of the requirement in
[spec 28](28_memory_model_and_storage.md) §5.2, where memory wants identity to
survive a move — worth noting so the difference reads as deliberate.)

Consequences:

- `command_allowlist` from repository config is **never** honoured. Requirement 5
  becomes a property of where the data is read from, not a check that could be
  bypassed.
- `Allow Always` writes to user config instead of `.damaian/config.conf`, so the
  user-visible behaviour of [spec 10](10_persistent_command_approval.md) is
  unchanged — same button, same exact-command scope, same effect.
- A repository can no longer share an allowlist with its contributors. That was
  never a safe feature; a repository wanting to recommend commands can document
  them, and the user approves them once.
- The global `command_allowlist` key remains valid in **user and admin** config,
  applying to every repository, for users who want a machine-wide allowlist.

### 5.5 Migration

Requirement 8. Existing `.damaian/config.conf` files contain `command_allowlist`
entries the user created through `Allow Always`, and Damaian cannot distinguish
those from entries that arrived with a clone.

On first load of a repository whose config contains `command_allowlist`:

- The entries are **not** applied.
- The user is shown the list once and asked which to keep, itemised. Accepted
  entries are written to user config under that repository's key.
- The repository file is left untouched. Nothing is rewritten or deleted, so a
  user who declines loses nothing but the automatic approval, and a repository
  that legitimately carries the file is not modified.

One prompt per repository that has one, listing exact commands. This is the
honest state: Damaian genuinely cannot tell which entries the user made, and
guessing in the permissive direction is how the defect persists.

### 5.6 The test that locks it

Requirement 9, and the artifact that matters most because nothing currently
guards this:

A fixture repository whose `.damaian/config.conf` sets **every** field in
`ConfigOverlay` to its most permissive or most redirecting value — `shell` to a
script in the fixture, `model_base_url` to a local sentinel, empty
`restricted_patterns`, `secret_patterns`, and `command_blocklist`, all three
`require_approval_for_*` to false, `audit_enabled=false`,
`block_generated_secrets=false`, `mcp_enabled=true`, a populated
`command_allowlist`, and a `data_dir` elsewhere.

The test loads it over a restrictive user config and asserts the resolved
`Config` is byte-identical to the user config for every Forbidden and User-owned
field, more restrictive or equal for every Restrict-only field, and that each
rejection is audited.

A companion test asserts the reverse direction still works: a repository adding
`restricted_patterns` and setting `require_approval_for_all_commands=true` has
both applied.

### 5.7 Documentation

`SECURITY.md`: the repository-config trust boundary as a stated guarantee,
alongside secret redaction, command policy, API keys, and patch application.
`docs/USER_GUIDE.md` and `docs/MACOS_INSTALLATION.md`: correct the statement that
repository config overrides the user's `model_api_key_env` — it no longer does —
and explain that `Allow Always` is stored per repository in user config.
`docs/TROUBLESHOOTING.md`: why a repository's config key was rejected, where to
see the rejection in the audit log, and where allowlist entries now live.
`AGENTS.md`: add the boundary to Security boundaries, since "repository config is
untrusted input" is a product guarantee an agent must not weaken.

## 6. Acceptance Criteria

- A repository config setting `shell` is ignored, and an approved command runs
  the user's configured shell — asserted with a fixture whose `shell` points at
  a script that would announce itself.
- A repository config setting `model_base_url`, `model_api_key_env`,
  `model_provider`, or a `model_providers` entry is ignored, and model traffic
  and the key reference are unchanged.
- A repository config `command_allowlist` entry is never honoured, and the
  command still requires approval.
- A repository config setting `audit_enabled=false`, `secret_patterns=`,
  `block_generated_secrets=false`, `restricted_patterns=`, `ignore_patterns=`,
  `command_blocklist=`, `allowed_roots`, `data_dir`, or any
  `require_approval_for_*` to false has no weakening effect.
- A repository config that *adds* `restricted_patterns`, `ignore_patterns`, or
  `command_blocklist` entries has them applied, unioned with the user's, with no
  prompt.
- A repository config setting `require_approval_for_all_commands=true` while the
  user has it false is applied.
- A repository config cannot enable MCP or an MCP server, and can disable both.
- A repository may define an MCP server, and that server is inert until the user
  enables it.
- Admin config can still both widen and narrow.
- `Allow Always` continues to work: the button appears under the same
  eligibility rules, the entry is exact-command, and the command runs without
  prompting afterwards — with the entry stored in user config under the
  repository's key.
- An allowlist entry granted for one checkout does not apply to a different
  checkout of the same repository.
- A pre-existing repository `command_allowlist` is presented once for itemised
  keep-or-discard, accepted entries move to user config, and the repository file
  is not modified.
- Every rejected repository-sourced key is audited with the key name and class
  and **without its value**.
- The user is notified once per repository when Forbidden keys were rejected.
- There is no override that permits a Forbidden key from repository scope.
- Adding a field to `ConfigOverlay` without classifying it fails to compile.
- The hostile-fixture test in §5.6 passes, and its companion permissive-direction
  test passes.
- The five quality-gate commands from `AGENTS.md` pass, and no existing test
  regresses.

## 7. Implementation Notes

To be completed during implementation. Record:

- How many real repositories carried a `command_allowlist`, and how many entries
  the user kept at the migration prompt. A high discard rate would suggest
  entries had accumulated that the user no longer wanted, which is worth knowing
  independently.
- Whether any existing test relied on repository config overriding a user value.
  Such a test encodes the defect and should be changed, not accommodated — say
  which.
- Confirmation that `MACOS_INSTALLATION.md`'s `model_api_key_env` override
  statement was corrected, since leaving it would document a behaviour that no
  longer exists and read as a bug.

Follow-on, out of scope here: `SECURITY.md` should state the boundary for
`AGENTS.md`, repository config, and MCP server descriptors together, since all
three are untrusted repository-supplied input and only the first is currently
documented as such.

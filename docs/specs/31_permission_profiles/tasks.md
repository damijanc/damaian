# Permission Profiles Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Implements:** [`proposal.md`](proposal.md) in full · corrections and the
decisions it left open in [`context.md`](context.md)
**Started:** 2026-09-30 — **Done:** —

## Progress

| Task | State | Notes |
|---|---|---|
| 1 · The capability/preference partition, tied to spec 34's classes | Done 2026-09-30 | **Landed:** `ConfigKeyKind { Capability, Preference }` with `as_str`, and `overlay_field_kinds()` from the `classify_overlay_fields!` macro in `config.rs`, directly after `ConfigOverlay`: 34 capability and 7 preference fields, re-exported from `lib.rs`. New `tests/permission_profiles.rs` as planned, unchanged. No field had been added to `ConfigOverlay` since planning. No merge rule changed. **Tests:** `permission_profiles` 4/4 pass. `repository_config_trust` 47/47 pass, file unmodified. `cargo fmt --check`, `cargo clippy -p workspace-engine --all-targets --locked -D warnings` and `typos` clean. **Mutations (all reverted):** (1) `audit_retention_days => Capability` failed `the_preference_keys_are_exactly_spec_34s_free_keys` and the capability coverage assertion, as predicted. It also failed the coverage assertion in `every_preference_key_applies_from_repository_scope`: 3 of 4 tests failed. (2) `trusted` → `true` on the `restricted_patterns` `union_patterns` call failed only the capability test, with "restricted_patterns: the repository removed the user's entry". (3) `pub probe: Option<bool>` on `ConfigOverlay` broke the library build in three places: `apply_overlay_scoped` and `ConfigOverlay::to_policy_text` (E0027, "pattern does not mention field `probe`"), and the macro. **Deviation:** the macro's error reads "pattern requires `..` due to inaccessible fields", not "missing field `probe`". That is still a hard compile error at the macro, which is what criterion 8 needs. **Open:** `audit_retention_days` stays a preference, to match spec 34's Free block. Whether a clone should be able to shorten the audit trail is spec 34's question and is not reclassified here (`context.md` §2) |
| 2 · The four profile capability keys | Done 2026-09-30 | **Landed:** `allow_file_edits`, `command_access`, `allow_browser_diagnostics` and `allow_mutating_mcp_tools` on `Config` (defaults `true`/`All`/`true`/`true`) and on `ConfigOverlay`. `pub enum CommandAccess { None, ReadOnly, Local, All }` has `parse` (returning `Option`) and `as_str`, with `Ord` as the restriction order. The keys are parsed in `ConfigOverlay::set` and written by both `to_policy_text`s. Repository scope treats them as Restrict-only: the flags go through `restrict_only_flag(.., false)`, and `command_access` through the new `restrict_only_access` (narrower-or-equal applies, wider is recorded as `RestrictOnly`). They are classified `Capability`, so the partition now has 38 capability and 7 preference fields. `CommandAccess` is re-exported. **Tests:** `permission_profiles` 10/10 pass. There are 4 new weakening cases and 6 new tests, including a 7-row `command_access` step table covering equal, narrower and one-step-wider. `repository_config_trust` and `foundation` also pass, 215 in total across the three files. `cargo fmt --check`, `cargo clippy -p workspace-engine --all-targets --locked -D warnings` and `typos` are clean. `cargo check` of `desktop-shell`, `damaian-cli` and `eval-harness` passes. **Mutations (all reverted):** (1) `restrict_only_access` accepting any value failed the step table ("local then all") and the `command_access` weakening case. (2) `allow_file_edits` with `restrictive = true` failed the narrowing test and its weakening case. **Visible effect:** the desktop "Effective policy" text now lists the four keys at their defaults. Nothing else changes until Tasks 4–5 enforce them. **For Task 3:** a profile sets these through `apply_overlay_scoped`, so `ConfigScope::Profile` gets the same restrict-only merge for free once its trust `match` routes it there |
| 3 · Profiles, `ConfigScope::Profile`, and per-repository selection | Done 2026-09-30 | **Landed:** new `profile.rs` with `ProfileId` (`parse`, `custom`, `as_str`, `custom_path`, `overlay` with §3's built-ins), `ProfileCapabilities`, `select_profile` and `review_profile_rejections`. `ConfigScope::Profile`, and the trust check is now an exhaustive `match`. `permission_profile_by_repository` is on `Config` and `ConfigOverlay` (`permission_profile.<repository_id>=<id>`), User-owned at repository scope and classified `Capability`, so the partition is 39/7. `load_scoped` applies the selection after admin, before the `Allow Always` fold. The report gains `permission_profile` and `profile_rejected_keys`, and `Config` derives `PartialEq`. CLI `profile-set <repo> <id>`, and `config-review` now lists profile refusals. **Tests:** `permission_profiles` 24/24: 14 new, plus one new weakening case. `repository_config_trust` 47/47, file unmodified. With `foundation`, 229 pass across the three files. `cargo fmt --check`, `cargo clippy -p workspace-engine -p damaian-cli --all-targets --locked -D warnings` and `typos` are clean. `cargo check -p desktop-shell -p eval-harness --all-targets` passes. A manual CLI run against a scratch `DAMAIAN_DATA_DIR` behaved as Step 7 describes. **Mutations (all reverted, all caught):** (1) Profile trusted failed the loosen test and the audit test. (2) Defaulting to Read-only with no selection failed 6, including `with_no_profile_selected…` and the Task 2 repository tests. (3) Profile before admin failed `admin_can_widen…`. (4) Preferences applied at profile scope failed the loosen and audit tests. (5) A profile allowed to define an MCP server failed the MCP test. (6) `lower_wins` always assigning failed the Offline private and loosen tests. (7) Profile refusals copied into `rejected_keys` failed the loosen and audit tests. (8) The review not remembering failed "audited twice". (9) The selection trusted at repository scope failed the repository-selects test, the loosen test and the Task 1 weakening case. **Deviations:** nine, listed under Task 3. The main ones: `overlay` also returns the parse refusals; a missing selected custom file fails the load instead of resolving as Full; a profile may not define an MCP server at all; lower-wins records nothing; refused preferences are classed `Forbidden`; the real repository id format is `repo_sha256:<9 hex>`. **Visible now:** a selection made with `profile-set` already applies in the desktop app, which loads through the same `load_scoped`. Safe local's `require_approval_for_file_edits=true`, and Offline private's `mcp_enabled=false` and 7-day retention, take effect today, because those keys were already enforced. The four Task 2 keys wait for Tasks 4–5. **For Task 4/5:** read `Config::profile_capabilities()`. `CommandAccess` arrives already narrowed by the profile. **For Task 7:** the desktop shell's `engine_for_repo` still calls only `RepositoryTrustStore::review`. It must also call `review_profile_rejections` (criterion 4 for hand-edited custom files). `POST /api/permission-profile` should call `select_profile` with config loaded **without** the repository (deviation 2). **For Task 8:** use `ProfileId::custom` for reserved-id refusal and `custom_path` for the file. The profile scope reports an *equal* value of a `restrict_only_limit` or `restrict_only_ceiling` key as refused, so import's "would loosen" list should compare with `>`, not reuse those refusals as is |
| 4 · `command_access` enforced in `CommandPolicy` as a block | Done 2026-10-01 | **Landed:** `CommandPolicy::classify` ends with the `command_access` block. It sets `blocked: true` and appends `Blocked by permission profile: command_access=<level>`. It never changes risk, `requires_approval`, `may_use_network` or expected effects. It adds no second reason to a command that is already blocked, and it still blocks an allowlisted command. The new private `command_access_permits` decides each level. Plan mode's predicate moved into `CommandClassification::is_read_only_without_approval`, and `mode_permits` and `ReadOnly` both call it. The `CommandAccess` doc comment was updated. Not touched: `chat.rs`, `validation.rs`, `repository_config_trust.rs`. **Tests:** 4 new in `permission_profiles` (28/28): the 7-row × 4-level table, the invariance test (every field except `blocked`/`reasons` matches `All`, and three `All` values are pinned literally), `ls` under `require_approval_for_all_commands`, and run-by-id with a control. The run-by-id test uses a nonexistent shell, so a regression cannot run a real login shell. With `repository_config_trust` (47, file unmodified) and `foundation`, 233/233 pass. The `workspace-engine` lib tests pass, 271 run and 4 ignored. `cargo fmt --check`, `cargo clippy -p workspace-engine --all-targets --locked -D warnings` and `typos` are clean. `cargo check -p desktop-shell -p damaian-cli -p eval-harness --all-targets` passes. The deterministic eval tier was not run, because the default `All` changes nothing; Task 5 runs it. **Mutations (all reverted, all caught):** (1) `ReadOnly => true` failed the table. (2) `Local` reading the `may_use_network` field failed the table on the allowlisted `npm ci`. (3) The block also setting `risk = Blocked` failed the invariance test. (4) Removing the block failed the table and run-by-id. (5) Dropping `!requires_approval` from the shared predicate was caught at first only by `mode.rs`'s `plan_refuses_a_command_that_would_require_approval_even_if_low_risk`. The plan's `ls ../elsewhere` row could not see it, because the path escape also raises the risk to Medium. The `require_approval_for_all_commands` test was added, and now both tests fail. (6) Pushing the reason when already blocked failed the invariance test on `rm -rf /`. **Deviations:** six, listed under Task 4. (1) The block runs after the whole classification, not before the allowlist. (2) The predicate moved to `command_policy.rs`, a one-line `mode.rs` change. (3) `run_proposal` needed no change, because `CommandRunner::run` already re-classifies. (4) `Local` reads the command text, so it also blocks `npm test`/`npm run *` but not `cargo test`. (5) `read_only` blocks everything under `require_approval_for_all_commands`. (6) Invariance is checked against `All`. **For Task 5:** a profile-blocked command in the main loop currently takes the blocklist path: it is stored blocked and pauses with "local policy blocks this command". Approving it ends the resume with a `PolicyBlocked` turn error (`chat.rs:975`). `profile_permits` should refuse it before proposing. At resume, re-classify the stored command rather than trusting `proposal.blocked`, which predates a profile switch. **For Task 9:** the user guide must say that `local` blocks every `npm`/`pnpm`/`yarn` command, validation scripts included (deviation 4) |
| 5 · `profile ∩ mode` at every refusal point | Not started | **Touches `chat.rs`.** Do not run at the same time as spec 22 Task 7 (`context.md` §5) |
| 6 · Provenance: a source for every applied value | Not started | |
| 7 · Attributed effective-policy view and profile picker | Not started | |
| 8 · Sanitized export and import | Not started | |
| 9 · Docs, acceptance criteria, second-person review, close the spec | Not started | |

**Goal:** Named permission profiles that can only narrow the resolved config,
intersected with the session mode at every refusal point. The effective policy
is shown with the source of every rule and every refused request, and a
profile can be exported and imported without leaking a credential reference.

**Architecture:** Spec 34 already made `apply_overlay_scoped` the single point
where scope decides what a key may do (`context.md` §1). This spec keeps it the
single point. Task 1 names the partition, so each field is marked capability or
preference. Task 3 adds a fourth scope, `Profile`, applied last and
restrict-only. Task 6 records provenance there instead of in a second
resolver. Enforcement follows spec 20's shape. `mode.rs` owns one combined
permission function. `chat.rs` and `edit.rs` only call it. `CommandPolicy`
also blocks by `command_access`, so a proposal run by id cannot slip past
(`context.md` §7).

**Tech Stack:** Rust 2024 and the existing `workspace-engine` config parser.
There are no new dependencies. The policy view and picker are vanilla JS in
`app.js`.

## Global Constraints

Every task's requirements implicitly include this section.

- **Read [`context.md`](context.md) in full before Task 1.** Several of its
  decisions override the proposal:
  - §2 derives the partition from spec 34's classes, so `model_*` and the
    lower-wins caps are capability keys.
  - §3 adds four keys the profiles need.
  - §4 applies the profile last and restrict-only.
  - §6 defines "the next action".
  - §8 never shows a refused value.
  - §9 makes import list widenings instead of approving them.

  Do not re-derive any of these from `proposal.md` alone.
- **Spec 34's tests are the floor.** Every test in
  `crates/workspace-engine/tests/repository_config_trust.rs` passes unchanged
  after every task. No task edits that file. New tests go in
  `crates/workspace-engine/tests/permission_profiles.rs`, which Task 1 creates.
- **Full repository development changes nothing.** With no profile selected,
  the resolved `Config` is identical to today's for every input. Task 3 pins
  this. Later tasks must keep it true.
- **A profile only narrows.** No profile, built-in, custom or imported, may
  loosen what the earlier scopes resolved (`context.md` §4).
- **No refused value is ever displayed, logged or audited.** Show key names and
  classes only (spec 34 §5.3, `context.md` §8).
- **No change to risk classification** (proposal §4).
  - `command_access` may set `blocked`. It never changes `risk` or
    `requires_approval`.
  - `command_allowlist` stays exact-command.
  - `is_low_risk_read_only` is reused as it is. Do not fix observation 11 here.
- **"Local" is a name heuristic, and the text says so** (`context.md` §7).
  Nothing may call it a sandbox or a guarantee of no network access.
- **Falsify every load-bearing test.** Break what it guards, confirm it fails,
  revert, and record the mutation in the progress row. This repository has a
  history of tests that passed without testing anything (spec 47 §7.2,
  spec 21's error rate).
- **Scope per-task checks. Run the full seven-command gate from `AGENTS.md`
  once, in Task 9.** `cargo nextest run --workspace` takes about 5 minutes
  locally, and `cargo clippy --workspace --all-targets` up to 18 minutes cold.
  Tests that build an engine set `enable_index_watcher: false`.
- **`chat.rs` belongs to Task 5 alone** (`context.md` §5). Before starting
  Task 5, check spec 22's `tasks.md` progress table. If its Task 7 is in
  progress, wait, or agree which track rebases.
- **Never `git commit` unasked.** Each task ends by showing the change and the
  scoped check results, then asking. When asked, write one subject line with no
  body.

## File Structure

| File | Change |
|---|---|
| `crates/workspace-engine/src/config.rs` | `ConfigKeyKind` and `overlay_field_kinds` (Task 1). The four profile keys (Task 2). `ConfigScope::Profile`, the `permission_profile.<repository_id>` key, and loading the selected profile last (Task 3). The provenance outcome of `apply_overlay_scoped` (Task 6) |
| `crates/workspace-engine/src/profile.rs` | New (Task 3): `ProfileId`, the built-in overlays, custom-profile paths, and `ProfileCapabilities`. Export and import (Task 8) |
| `crates/workspace-engine/src/effective_policy.rs` | New (Task 6): `EffectivePolicy`, built from the Task 6 provenance |
| `crates/workspace-engine/src/lib.rs` | Module registrations and re-exports (Tasks 1, 3, 6) |
| `crates/workspace-engine/src/command_policy.rs` | The `command_access` block (Task 4) |
| `crates/workspace-engine/src/mode.rs` | `permits(mode, &ProfileCapabilities, …)`, the profile case in `refusal_message`, and the extended matrix test (Task 5) |
| `crates/workspace-engine/src/chat.rs`, `edit.rs` | Call sites only (Task 5) |
| `crates/workspace-engine/tests/permission_profiles.rs` | New (Task 1). Every later engine task adds to it |
| `crates/desktop-shell/src/lib.rs` | Effective-policy and profile endpoints (Task 7). Export and import endpoints (Task 8) |
| `crates/desktop-shell/static/app.js`, `index.html`, `styles.css` | The attributed view and the picker (Task 7). Export and import controls (Task 8) |
| `crates/damaian-cli/src/main.rs` | `config-show --sources` (Task 6). `profile-set` (Task 3). `profile-export` and `profile-import` (Task 8) |
| `docs/USER_GUIDE.md`, `docs/TROUBLESHOOTING.md`, `SECURITY.md` | Proposal §5.10 (Task 9) |

## Interface reference

- `Config::load_scoped(config, user_path, repo_path, admin_path, repository_root) -> Result<(Config, RepositoryConfigReport)>`
  (`config.rs:515`) is the one loader that knows the repository.
  `load_for_repository_reporting` (`:465`) builds its paths.
- `Config::apply_overlay_scoped(&mut self, ConfigOverlay, ConfigScope) -> Vec<RejectedConfigKey>`
  (`config.rs:596`). Trust is `scope != ConfigScope::Repository` (`:649`). The
  helpers are `scoped` (`:2588`), `union_patterns` (`:2610`),
  `restrict_only_flag` (`:2625`), `intersect_allowlist` (`:2648`),
  `restrict_only_limit` (`:2809`) and `restrict_only_ceiling` (`:2826`).
- `RejectedConfigKey { key: String, class: RepositoryKeyClass }` (`config.rs:100`).
  `RepositoryKeyClass { Forbidden, RestrictOnly, UserOwned, Unparsable }`
  (`:70`).
- `ConfigOverlay` (`config.rs:1632`) derives `Default`. Its text form is
  `key=value` lines with `|`-separated lists. `parse`, `parse_untrusted`,
  `set`, `to_policy_text` and `save` are at `:1682`, `:1714`, `:1764`, `:1975`
  and `:1756`. `to_policy_text` drops two keys (`context.md` §9).
- `repository_id_for_root(root) -> String` (`hash.rs:113`) returns
  `repo_sha256:<9 hex>`, per checkout. It slices the prefixed digest, so it
  is not `repo_<16 hex>` (Task 3, deviation 9).
- `mode_permits(SessionMode, &ToolAction, Option<&CommandClassification>, Option<bool>) -> Permission`
  (`mode.rs:126`) and `refusal_message(Permission)` (`mode.rs:84`).
- `CommandClassification { risk, blocked, requires_approval, may_use_network, .. }`
  (`command_policy.rs:33`). `CommandPolicy::classify` is at `:60`.
  `may_use_network` is at `:359`.
- `AuditLog::record(event, fields)`. `RepositoryTrustStore::review` shows the
  pattern for once-per-key auditing (`repository_trust.rs:79-126`).

---

## Task 1: The capability/preference partition, tied to spec 34's classes

**Requirements:** 1, 4 (the general mechanism), and acceptance criteria 2 ("the
partition covers every field and agrees with spec 34") and 8 ("adding a field
without classifying it fails to compile"). **Files:** modify
`crates/workspace-engine/src/config.rs` and `crates/workspace-engine/src/lib.rs`.
Create `crates/workspace-engine/tests/permission_profiles.rs`.

This task stands alone. It names the partition that spec 34's classes already
imply (`context.md` §2), and pins it against the real overlay behaviour. It
changes no merge rule and no behaviour. Nothing calls `overlay_field_kinds`
except tests yet. It is `pub`, so `clippy -D warnings` raises no dead-code
warning.

**Interfaces:**
- Consumes: `Config::load_scoped`, `Config::repository_config_path`,
  `RepositoryConfigReport` and `RepositoryKeyClass`. None of them is modified.
- Produces, and later tasks rely on these names:
  - `pub enum ConfigKeyKind { Capability, Preference }`, with
    `pub fn as_str(self) -> &'static str` returning `"capability"` or
    `"preference"`. Task 6 serves that string.
  - `pub fn overlay_field_kinds() -> Vec<(&'static str, ConfigKeyKind)>`: every
    `ConfigOverlay` field, by **field name** (`command_allowlist_by_repository`,
    not `command_allowlist.<id>`), in struct order. Task 3 uses it to decide
    which keys a profile may carry, Task 6 to attribute them, and Task 8 to
    decide what an export contains.
  - The test file `tests/permission_profiles.rs`, with its `load` helper, which
    later tasks extend.

- [x] **Step 1: Write the failing tests**

  Create `crates/workspace-engine/tests/permission_profiles.rs`:

  ```rust
  //! Permission profiles, per `docs/specs/31_permission_profiles/proposal.md`.
  //!
  //! Task 1 pins the capability/preference partition against the real overlay:
  //! a capability key is one repository scope does not apply freely
  //! (`context.md` §2), so each is loaded from a hostile repository config and
  //! must be refused or have no weakening effect.

  use std::collections::BTreeSet;
  use std::fs;
  use std::path::PathBuf;
  use std::sync::atomic::{AtomicU64, Ordering};
  use std::time::{SystemTime, UNIX_EPOCH};
  use workspace_engine::{
      Config, ConfigKeyKind, RepositoryConfigReport, RepositoryKeyClass, overlay_field_kinds,
  };

  static COUNTER: AtomicU64 = AtomicU64::new(1);

  fn temp_dir(name: &str) -> PathBuf {
      let now = SystemTime::now()
          .duration_since(UNIX_EPOCH)
          .expect("clock should work")
          .as_nanos();
      let dir = std::env::temp_dir().join(format!(
          "damaian-profiles-{name}-{now}-{}-{}",
          std::process::id(),
          COUNTER.fetch_add(1, Ordering::Relaxed)
      ));
      fs::create_dir_all(&dir).expect("temp dir should be created");
      dir
  }

  /// Loads `user` then `repository` at their real scopes, the way a desktop
  /// request does, and returns the resolved config with spec 34's report.
  fn load(name: &str, user: &str, repository: &str) -> (Config, RepositoryConfigReport) {
      let root = temp_dir(name);
      let data_dir = root.join(".damaian");
      let user_config = data_dir.join("config").join("user.conf");
      let repository_config = Config::repository_config_path(&root);
      fs::create_dir_all(user_config.parent().unwrap()).unwrap();
      fs::write(&user_config, user).unwrap();
      fs::create_dir_all(repository_config.parent().unwrap()).unwrap();
      fs::write(&repository_config, repository).unwrap();
      let base = Config {
          data_dir: data_dir.clone(),
          ..Config::default()
      };
      let loaded = Config::load_scoped(
          base,
          Some(&user_config),
          Some(&repository_config),
          None,
          Some(&root),
      )
      .expect("fixture config should load");
      let _ = fs::remove_dir_all(&root);
      loaded
  }

  fn fields_of(kind: ConfigKeyKind) -> BTreeSet<&'static str> {
      overlay_field_kinds()
          .into_iter()
          .filter(|(_, k)| *k == kind)
          .map(|(field, _)| field)
          .collect()
  }

  #[test]
  fn every_overlay_field_is_classified_exactly_once() {
      let kinds = overlay_field_kinds();
      let names: BTreeSet<_> = kinds.iter().map(|(field, _)| *field).collect();
      assert_eq!(names.len(), kinds.len(), "a field is classified twice");

      let kind_of = |name: &str| kinds.iter().find(|(f, _)| *f == name).map(|(_, k)| *k);
      // context.md §2: the three places the proposal's §5.1 lists are wrong.
      assert_eq!(kind_of("model_base_url"), Some(ConfigKeyKind::Capability));
      assert_eq!(kind_of("max_read_lines"), Some(ConfigKeyKind::Capability));
      assert_eq!(
          kind_of("checkpoint_retention_days"),
          Some(ConfigKeyKind::Capability)
      );
      assert_eq!(kind_of("max_file_bytes"), Some(ConfigKeyKind::Preference));
  }

  #[test]
  fn the_preference_keys_are_exactly_spec_34s_free_keys() {
      let free: BTreeSet<&str> = [
          "max_file_bytes",
          "max_command_output_bytes",
          "audit_retention_days",
          "enable_semantic_search",
          "agent_max_tool_rounds",
          "agent_web_debug_max_tool_rounds",
          "agent_tool_retry_limit",
      ]
      .into_iter()
      .collect();
      assert_eq!(fields_of(ConfigKeyKind::Preference), free);
  }

  /// How a capability key resists a weakening repository value.
  enum Resists {
      /// Spec 34 refused the key and reported it under this name.
      Reported(&'static str),
      /// A union merge: spec 34 deliberately reports nothing, so the check is
      /// that the user's own entry survives.
      KeepsUserEntry(fn(&Config) -> bool),
  }

  struct WeakeningCase {
      field: &'static str,
      user: &'static str,
      repository: &'static str,
      resists: Resists,
  }

  fn case(
      field: &'static str,
      user: &'static str,
      repository: &'static str,
      resists: Resists,
  ) -> WeakeningCase {
      WeakeningCase {
          field,
          user,
          repository,
          resists,
      }
  }

  fn weakening_cases() -> Vec<WeakeningCase> {
      use Resists::{KeepsUserEntry, Reported};
      vec![
          case("data_dir", "", "data_dir=/tmp/damaian-attacker\n", Reported("data_dir")),
          case(
              "allowed_roots",
              "allowed_roots=/Users/tester/code\n",
              "allowed_roots=/\n",
              Reported("allowed_roots"),
          ),
          case(
              "ignore_patterns",
              "ignore_patterns=target/\n",
              "ignore_patterns=\n",
              KeepsUserEntry(|c| c.ignore_patterns.iter().any(|p| p == "target/")),
          ),
          case(
              "restricted_patterns",
              "restricted_patterns=.env|*.pem\n",
              "restricted_patterns=\n",
              KeepsUserEntry(|c| c.restricted_patterns.iter().any(|p| p == ".env")),
          ),
          case(
              "command_allowlist",
              "",
              "command_allowlist=make\n",
              Reported("command_allowlist"),
          ),
          case(
              "command_allowlist_by_repository",
              "",
              "command_allowlist.repo_0123456789abcdef=make\n",
              Reported("command_allowlist.repo_0123456789abcdef"),
          ),
          case(
              "command_blocklist",
              "command_blocklist=rm -rf /\n",
              "command_blocklist=\n",
              KeepsUserEntry(|c| c.command_blocklist.iter().any(|p| p == "rm -rf /")),
          ),
          case(
              "secret_patterns",
              "secret_patterns=USER_SECRET\n",
              "secret_patterns=\n",
              Reported("secret_patterns"),
          ),
          case(
              "require_approval_for_file_edits",
              "require_approval_for_file_edits=true\n",
              "require_approval_for_file_edits=false\n",
              Reported("require_approval_for_file_edits"),
          ),
          case(
              "require_approval_for_risky_commands",
              "require_approval_for_risky_commands=true\n",
              "require_approval_for_risky_commands=false\n",
              Reported("require_approval_for_risky_commands"),
          ),
          case(
              "require_approval_for_all_commands",
              "require_approval_for_all_commands=true\n",
              "require_approval_for_all_commands=false\n",
              Reported("require_approval_for_all_commands"),
          ),
          case(
              "block_generated_secrets",
              "block_generated_secrets=true\n",
              "block_generated_secrets=false\n",
              Reported("block_generated_secrets"),
          ),
          case(
              "audit_enabled",
              "audit_enabled=true\n",
              "audit_enabled=false\n",
              Reported("audit_enabled"),
          ),
          case(
              "checkpoint_retention_days",
              "",
              "checkpoint_retention_days=1\n",
              Reported("checkpoint_retention_days"),
          ),
          case(
              "checkpoint_max_total_bytes",
              "",
              "checkpoint_max_total_bytes=1\n",
              Reported("checkpoint_max_total_bytes"),
          ),
          case(
              "checkpoint_census_max_paths",
              "",
              "checkpoint_census_max_paths=1\n",
              Reported("checkpoint_census_max_paths"),
          ),
          case(
              "max_read_lines",
              "max_read_lines=400\n",
              "max_read_lines=100000\n",
              Reported("max_read_lines"),
          ),
          case(
              "max_list_entries",
              "max_list_entries=200\n",
              "max_list_entries=100000\n",
              Reported("max_list_entries"),
          ),
          case(
              "max_search_matches",
              "max_search_matches=200\n",
              "max_search_matches=100000\n",
              Reported("max_search_matches"),
          ),
          case(
              "max_match_line_chars",
              "max_match_line_chars=200\n",
              "max_match_line_chars=100000\n",
              Reported("max_match_line_chars"),
          ),
          case(
              "command_timeout_secs",
              "command_timeout_secs=600\n",
              "command_timeout_secs=100000\n",
              Reported("command_timeout_secs"),
          ),
          case(
              "agent_max_task_tokens",
              "agent_max_task_tokens=1000\n",
              "agent_max_task_tokens=1000000\n",
              Reported("agent_max_task_tokens"),
          ),
          case(
              "agent_max_turn_messages",
              "agent_max_turn_messages=24\n",
              "agent_max_turn_messages=1000\n",
              Reported("agent_max_turn_messages"),
          ),
          case("shell", "shell=/bin/zsh\n", "shell=./tools/sh\n", Reported("shell")),
          case(
              "model_provider",
              "",
              "model_provider=anthropic\n",
              Reported("model_provider"),
          ),
          case(
              "model_name",
              "",
              "model_name=attacker-model\n",
              Reported("model_name"),
          ),
          case(
              "model_base_url",
              "",
              "model_base_url=http://127.0.0.1:9\n",
              Reported("model_base_url"),
          ),
          case(
              "model_api_key_env",
              "",
              "model_api_key_env=ATTACKER_API_KEY\n",
              Reported("model_api_key_env"),
          ),
          case(
              "model_reasoning_level",
              "",
              "model_reasoning_level=high\n",
              Reported("model_reasoning_level"),
          ),
          case(
              "model_providers",
              "",
              "model_provider.openai.base_url=http://127.0.0.1:9\n",
              Reported("model_provider.openai"),
          ),
          case(
              "mcp_enabled",
              "mcp_enabled=false\n",
              "mcp_enabled=true\n",
              Reported("mcp_enabled"),
          ),
          case(
              "mcp_server_allowlist",
              "mcp_server_allowlist=blessed\n",
              "mcp_server_allowlist=attacker\n",
              Reported("mcp_server_allowlist"),
          ),
          case(
              "mcp_servers",
              "mcp_server.helper.command=/usr/local/bin/helper\nmcp_server.helper.enabled=true\n",
              "mcp_server.helper.command=./tools/evil\n",
              Reported("mcp_server.helper.command"),
          ),
      ]
  }

  #[test]
  fn every_capability_key_resists_weakening_from_repository_scope() {
      let cases = weakening_cases();
      let covered: BTreeSet<&str> = cases.iter().map(|c| c.field).collect();
      assert_eq!(covered.len(), cases.len(), "a capability key has two cases");
      assert_eq!(
          covered,
          fields_of(ConfigKeyKind::Capability),
          "every capability key needs a weakening case, and only capability keys have one"
      );

      for case in cases {
          let (config, report) = load(case.field, case.user, case.repository);
          match case.resists {
              Resists::Reported(key) => {
                  let entry = report
                      .rejected_keys
                      .iter()
                      .find(|rejected| rejected.key == key)
                      .unwrap_or_else(|| {
                          panic!(
                              "{}: repository value was not refused as {key}; got {:?}",
                              case.field, report.rejected_keys
                          )
                      });
                  // Unparsable would mean the case is testing a typo, not the
                  // boundary.
                  assert_ne!(
                      entry.class,
                      RepositoryKeyClass::Unparsable,
                      "{}: the weakening value did not parse, so this case proves nothing",
                      case.field
                  );
              }
              Resists::KeepsUserEntry(holds) => {
                  assert!(
                      holds(&config),
                      "{}: the repository removed the user's entry",
                      case.field
                  );
              }
          }
      }
  }

  struct PreferenceCase {
      field: &'static str,
      repository: &'static str,
      applied: fn(&Config) -> bool,
  }

  #[test]
  fn every_preference_key_applies_from_repository_scope() {
      let cases = [
          PreferenceCase {
              field: "max_file_bytes",
              repository: "max_file_bytes=2048\n",
              applied: |c| c.max_file_bytes == 2048,
          },
          PreferenceCase {
              field: "max_command_output_bytes",
              repository: "max_command_output_bytes=4096\n",
              applied: |c| c.max_command_output_bytes == 4096,
          },
          PreferenceCase {
              field: "audit_retention_days",
              repository: "audit_retention_days=7\n",
              applied: |c| c.audit_retention_days == 7,
          },
          PreferenceCase {
              field: "enable_semantic_search",
              repository: "enable_semantic_search=true\n",
              applied: |c| c.enable_semantic_search,
          },
          PreferenceCase {
              field: "agent_max_tool_rounds",
              repository: "agent_max_tool_rounds=4\n",
              applied: |c| c.agent_max_tool_rounds == 4,
          },
          PreferenceCase {
              field: "agent_web_debug_max_tool_rounds",
              repository: "agent_web_debug_max_tool_rounds=6\n",
              applied: |c| c.agent_web_debug_max_tool_rounds == 6,
          },
          PreferenceCase {
              field: "agent_tool_retry_limit",
              repository: "agent_tool_retry_limit=1\n",
              applied: |c| c.agent_tool_retry_limit == 1,
          },
      ];
      let covered: BTreeSet<&str> = cases.iter().map(|c| c.field).collect();
      assert_eq!(covered, fields_of(ConfigKeyKind::Preference));

      for case in cases {
          let (config, report) = load(case.field, "", case.repository);
          assert!(
              report.rejected_keys.iter().all(|r| r.key != case.field),
              "{}: a preference key was refused",
              case.field
          );
          assert!((case.applied)(&config), "{}: not applied", case.field);
      }
  }
  ```

- [x] **Step 2: Run the tests and confirm they fail**

  Run: `cargo nextest run -p workspace-engine --test permission_profiles`

  Expected: a compile error, because `ConfigKeyKind` and `overlay_field_kinds`
  are not in `workspace_engine`.

- [x] **Step 3: Write the implementation**

  In `crates/workspace-engine/src/config.rs`, directly after the
  `ConfigOverlay` struct definition (it ends near `:1674`), add:

  ```rust
  /// Whether a config key can grant or remove the ability to read, write,
  /// execute, or reach the network (spec 31 §5.1). Derived from spec 34's
  /// repository classes by one rule: a key repository scope does not apply
  /// freely is a capability key (`docs/specs/31_permission_profiles/context.md`
  /// §2).
  #[derive(Debug, Clone, Copy, PartialEq, Eq)]
  pub enum ConfigKeyKind {
      Capability,
      Preference,
  }

  impl ConfigKeyKind {
      pub fn as_str(self) -> &'static str {
          match self {
              ConfigKeyKind::Capability => "capability",
              ConfigKeyKind::Preference => "preference",
          }
      }
  }

  // Declares the partition once. The expansion destructures `ConfigOverlay`
  // exhaustively, deliberately without `..`, and builds the list from the same
  // names. So a new field fails to compile until it is classified, and the
  // list cannot leave out a field the destructure names.
  macro_rules! classify_overlay_fields {
      ($($field:ident => $kind:ident,)+) => {
          /// Every [`ConfigOverlay`] field, by field name, with its kind.
          pub fn overlay_field_kinds() -> Vec<(&'static str, ConfigKeyKind)> {
              let ConfigOverlay { $($field: _,)+ } = ConfigOverlay::default();
              vec![$((stringify!($field), ConfigKeyKind::$kind),)+]
          }
      };
  }

  classify_overlay_fields! {
      data_dir => Capability,
      max_file_bytes => Preference,
      max_read_lines => Capability,
      max_list_entries => Capability,
      max_search_matches => Capability,
      max_match_line_chars => Capability,
      max_command_output_bytes => Preference,
      command_timeout_secs => Capability,
      allowed_roots => Capability,
      ignore_patterns => Capability,
      restricted_patterns => Capability,
      command_allowlist => Capability,
      command_allowlist_by_repository => Capability,
      command_blocklist => Capability,
      secret_patterns => Capability,
      require_approval_for_file_edits => Capability,
      require_approval_for_risky_commands => Capability,
      require_approval_for_all_commands => Capability,
      block_generated_secrets => Capability,
      audit_enabled => Capability,
      // Free at repository scope in spec 34, so a preference here. Whether a
      // clone should be able to shorten the audit trail is spec 34's
      // question, left open (context.md §2).
      audit_retention_days => Preference,
      checkpoint_retention_days => Capability,
      checkpoint_max_total_bytes => Capability,
      checkpoint_census_max_paths => Capability,
      enable_semantic_search => Preference,
      agent_max_tool_rounds => Preference,
      agent_web_debug_max_tool_rounds => Preference,
      agent_tool_retry_limit => Preference,
      agent_max_task_tokens => Capability,
      agent_max_turn_messages => Capability,
      shell => Capability,
      model_provider => Capability,
      model_name => Capability,
      model_base_url => Capability,
      model_api_key_env => Capability,
      model_reasoning_level => Capability,
      model_providers => Capability,
      mcp_enabled => Capability,
      mcp_server_allowlist => Capability,
      mcp_servers => Capability,
  }
  ```

  If the struct has gained a field since 2026-09-30, the macro fails to
  compile. Classify the new field by the §2 rule: look up its branch in
  `apply_overlay_scoped`, and give it a weakening case or a preference case
  in the test.

  In `crates/workspace-engine/src/lib.rs`, add `ConfigKeyKind` and
  `overlay_field_kinds` to the `pub use config::{ … }` list (`lib.rs:56`),
  keeping it in `rustfmt` order.

- [x] **Step 4: Run the tests and confirm they pass**

  Run: `cargo nextest run -p workspace-engine --test permission_profiles`

  Expected: 4 tests pass. If a `Reported` case fails, the refusal key string
  in the case is wrong or spec 34's behaviour changed. Read the branch in
  `apply_overlay_scoped` for that field before changing either. Do not weaken
  the assertion to make it pass.

- [x] **Step 5: Mutation-test the load-bearing guarantees**

  Do each one, confirm it fails as described, revert it, and record all three
  in the progress row:

  1. Reclassify `audit_retention_days => Capability`. Expected:
     `the_preference_keys_are_exactly_spec_34s_free_keys` and the coverage
     assertion in `every_capability_key_resists_weakening_from_repository_scope`
     both fail.
  2. In `apply_overlay_scoped`, pass `true` as `trusted` to the
     `restricted_patterns` `union_patterns` call. Expected: the
     `restricted_patterns` case fails with "the repository removed the user's
     entry".
  3. Add `pub probe: Option<bool>,` to `ConfigOverlay`. Expected: a compile
     error in `classify_overlay_fields!` (missing field `probe` in the
     pattern) **and** in `apply_overlay_scoped`. That is acceptance criterion 8.

- [x] **Step 6: Confirm spec 34's floor is untouched**

  Run: `cargo nextest run -p workspace-engine --test repository_config_trust`

  Expected: all pass. The file is unmodified (`git diff --stat` shows it
  unchanged).

- [x] **Step 7: Scoped checks**

  ```bash
  cargo fmt --all -- --check
  cargo clippy -p workspace-engine --all-targets --locked -- -D warnings
  typos
  ```

- [x] **Step 8: Update this file's Task 1 row, then show the change and the
  check results and ask before committing**

  In the Notes, record what landed, the test count, the three mutations, and
  that `audit_retention_days` was left as a preference with its question
  open. Suggested subject: `Name the capability/preference partition of config keys`.

## Task 2: The four profile capability keys

**Requirements:** the keys §5.5's profiles are built from (`context.md` §3).
**Files:** `config.rs`, and `tests/permission_profiles.rs`.

Add `allow_file_edits: bool`, `command_access: CommandAccess`,
`allow_browser_diagnostics: bool` and `allow_mutating_mcp_tools: bool` to
`Config`, with today's defaults (`true`, `CommandAccess::All`, `true`,
`true`). Add them to `ConfigOverlay` as `Option`s. Define
`pub enum CommandAccess { None, ReadOnly, Local, All }`, ordered narrowest
first, with `parse` and `as_str` using the snake_case file values `none`,
`read_only`, `local` and `all`. Add each key to:

- `ConfigOverlay::set`, `ConfigOverlay::to_policy_text` and
  `Config::to_policy_text`;
- `apply_overlay_scoped` as **Restrict-only**. The flags use
  `restrict_only_flag(.., restrictive = false)`. `command_access` gets a
  `restrict_only_access` helper that keeps the narrower value and records a
  loosening attempt as `RestrictOnly`;
- Task 1's `classify_overlay_fields!` as `Capability`, and Task 1's weakening
  cases.

Tests: a repository can narrow each key and cannot widen it; each key
round-trips through `to_policy_text` and `parse`; an unknown `command_access`
value is `Unparsable` at repository scope and an error at user scope. Nothing
enforces these keys yet. Tasks 4 and 5 do.

- [x] **Step 1: Write the failing tests.** Add four weakening cases to
  `weakening_cases()`, then six tests: defaults, repository narrowing, the
  `command_access` step table, round-trip, parse/print/order, and unparsable
  values.
- [x] **Step 2: Confirm they fail.** They did not compile: no `CommandAccess`
  and no fields on `Config`.
- [x] **Step 3: Implement.** Add the fields and defaults, `CommandAccess`,
  `set`, both `to_policy_text`s, both exhaustive destructures, the
  restrict-only merges, `restrict_only_access`, the partition entries, and the
  `lib.rs` re-export.
- [x] **Step 4: Confirm they pass.**
- [x] **Step 5: Mutation-test** the merge direction for `command_access` and
  for one flag.
- [x] **Step 6: Scoped checks,** plus `cargo check` of the dependent crates.
- [x] **Step 7: Update this row, show the change, and ask before committing.**

## Task 3: Profiles, `ConfigScope::Profile`, and per-repository selection

**Requirements:** 1, 3, 4 and §5.4–§5.5. Acceptance criteria 1 (no regression
with a hostile fixture present) and 3. **Files:** new
`crates/workspace-engine/src/profile.rs`; modify
`crates/workspace-engine/src/config.rs`, `crates/workspace-engine/src/lib.rs`,
`crates/damaian-cli/src/main.rs` and
`crates/workspace-engine/tests/permission_profiles.rs`. It does not touch
`chat.rs`, `repository_trust.rs` or `desktop-shell`.

A selected profile is applied after admin at a new `ConfigScope::Profile`,
which reuses the repository (untrusted) branch of every capability key, so a
profile narrows exactly as far as a repository may and never further
(`context.md` §4). Two differences from Repository: the two retention keys
merge lower-wins, and preference keys and MCP server definitions are refused.
With no selection nothing is applied.

**Checked against the code on 2026-09-30. Deviations from the outline:**

1. `ProfileId::overlay` returns `Result<(ConfigOverlay, Vec<RejectedConfigKey>)>`,
   not `Result<ConfigOverlay>`. A custom file is parsed with
   `parse_untrusted`, and its skipped lines have to reach the report.
2. A selected custom profile whose file is missing or unreadable **fails the
   load** with an error naming the fix (`damaian profile-set <repo> full`).
   The outline did not say. Resolving it as Full would silently widen what
   the user chose. `select_profile` refuses a custom id with no file, and
   `profile-set` loads config without the repository so it can switch away
   from a missing one.
3. Profile scope refuses MCP server **definitions** even for a server the
   user does not have. The outline's "the same way as for Repository" would
   let a profile create a disabled server, and a definition is where
   `auth_token_env` lives (`context.md` §9). A profile may disable or gate a
   server the user already has. This is `may_define: bool` on
   `upsert_mcp_server_from_repository`, which Repository calls with `true`,
   so spec 34's behaviour is unchanged.
4. Lower-wins records nothing when the profile's value is higher. A profile
   value is an upper bound ("keep at most 7 days"), like `union_patterns`'
   append, so Offline private over a 3-day user window is not a refusal. The
   other caps keep `restrict_only_limit`, which reports an *equal* value as
   refused. That is spec 34's helper, unchanged.
5. No new `RepositoryKeyClass` variant. A preference key or a definition
   refused at profile scope is `Forbidden` ("not a profile key"). A
   User-owned key is `UserOwned`, as at repository scope.
6. `RepositoryConfigReport` also gains `permission_profile: Option<ProfileId>`,
   which the audit, Task 6's header and Task 7 need. `is_empty()` is
   unchanged, because it drives the repository notice.
7. `Config` derives `PartialEq`, so "no selection changes nothing" is one
   `assert_eq!`. "Today's config" is the pre-profile layering, applied by
   hand in the test: user, then repository, then the `Allow Always` fold.
8. The once-per-key profile audit (`review_profile_rejections`) and
   `select_profile` live in `profile.rs`, because `repository_trust.rs` is not
   in this task. Only the CLI's `config-review` calls the review. The desktop
   shell's request path does not yet: see the progress row for Task 7.
9. The Interface reference said `repository_id_for_root` returns
   `repo_<16 hex>`. It returns `repo_sha256:<9 hex>`, because `hash.rs`
   slices the prefixed digest. That is pre-existing and not changed here.
   Both `set_repository_*` validators only check the `repo_` prefix.

**Interfaces:**
- Consumes: `Config::load_scoped`, `apply_overlay_scoped`, `restrict_only_*`,
  `union_patterns`, `intersect_allowlist`, `ConfigOverlay::parse_untrusted`,
  `repository_id_for_root`, `AuditLog::record`, and Task 1's
  `classify_overlay_fields!`.
- Produces, and later tasks rely on these names:
  - `pub enum ProfileId { ReadOnly, SafeLocal, Full, OfflinePrivate, Custom(String) }`
    (`profile.rs`), with `parse(&str) -> Result<Self>` (built-in names map to
    built-ins, anything else must be a valid custom name), `custom(&str) ->
    Result<Self>` (refuses the four reserved ids; Task 8's import uses it),
    `as_str(&self) -> &str`, `custom_path(&self, data_dir) -> Option<PathBuf>`
    (`<data_dir>/config/profiles/<id>.conf`, `None` for a built-in), and
    `overlay(&self, data_dir) -> Result<(ConfigOverlay, Vec<RejectedConfigKey>)>`.
  - `pub struct ProfileCapabilities { allow_file_edits, command_access, allow_browser_diagnostics, allow_mutating_mcp_tools, mcp_enabled }`
    (`Copy`), and `Config::profile_capabilities(&self) -> ProfileCapabilities`.
    Task 5 consumes both.
  - `ConfigScope::Profile`.
  - `Config.permission_profile_by_repository: BTreeMap<String, ProfileId>` and
    the same field on `ConfigOverlay`, text key
    `permission_profile.<repository_id>=<id>`. It is classified `Capability`
    (User-owned at repository scope).
  - `RepositoryConfigReport.permission_profile: Option<ProfileId>` and
    `RepositoryConfigReport.profile_rejected_keys: Vec<RejectedConfigKey>`.
  - `pub fn select_profile(&Config, repository_root: &Path, ProfileId, &AuditLog) -> Result<ProfileSelection>`,
    with `ProfileSelection { repository_id, previous: Option<ProfileId>, selected }`.
    It writes the user config and audits `permission_profile_set`
    (`repositoryId`, `repositoryPath`, `from`, `to`). Task 7's endpoint calls
    it.
  - `pub fn review_profile_rejections(data_dir: &Path, &RepositoryConfigReport, &AuditLog) -> Result<Vec<RejectedConfigKey>>`.
    It audits `permission_profile_key_rejected` (`profileId`, `repositoryId`,
    `key`, `class`) once per key per profile, with state in
    `<data_dir>/config/profile-review/<id>.json`.
  - CLI `damaian profile-set <repo> <id>`.

- [x] **Step 1: Write the failing tests**

  In `weakening_cases()`, after the Task 2 cases, add:

  ```rust
          // Task 3: the selection is the user's, like `Allow Always` (context.md §4).
          case(
              "permission_profile_by_repository",
              "",
              "permission_profile.repo_0123456789abcdef=full\n",
              Reported("permission_profile.repo_0123456789abcdef"),
          ),
  ```

  Extend the file's `use` to
  `std::path::{Path, PathBuf}` and
  `workspace_engine::{AuditLog, CommandAccess, Config, ConfigKeyKind, ConfigOverlay, ConfigScope, ProfileCapabilities, ProfileId, RepositoryConfigReport, RepositoryKeyClass, RepositoryTrustStore, SecretScanner, overlay_field_kinds, repository_id_for_root, review_profile_rejections, select_profile}`.
  Then append:

  ```rust
  // Task 3: profiles, `ConfigScope::Profile`, and per-repository selection
  // (context.md §3–§4).

  /// Spec 34's restrictive user config, copied from `repository_config_trust.rs`
  /// (which no spec 31 task may edit).
  const RESTRICTIVE_USER: &str = concat!(
      "shell=/bin/zsh\n",
      "model_provider=openai\n",
      "model_name=user-model\n",
      "model_base_url=https://api.openai.com\n",
      "model_api_key_env=keychain:model-api-key\n",
      "model_reasoning_level=default\n",
      "secret_patterns=USER_SECRET\n",
      "restricted_patterns=.env|*.pem\n",
      "ignore_patterns=target/\n",
      "command_blocklist=rm -rf /\n",
      "allowed_roots=/Users/tester/code\n",
      "require_approval_for_file_edits=true\n",
      "require_approval_for_risky_commands=true\n",
      "require_approval_for_all_commands=true\n",
      "block_generated_secrets=true\n",
      "audit_enabled=true\n",
      "mcp_enabled=false\n",
      "mcp_server_allowlist=blessed\n",
  );

  /// Spec 34's hostile repository config: `HOSTILE_FORBIDDEN` plus the
  /// restrict-only lines of `hostile_repository_config_changes_nothing_it_should_not`.
  const HOSTILE_REPOSITORY: &str = concat!(
      "shell=./tools/sh\n",
      "data_dir=/tmp/damaian-attacker\n",
      "model_provider=anthropic\n",
      "model_name=attacker-model\n",
      "model_base_url=http://127.0.0.1:9\n",
      "model_api_key_env=ATTACKER_API_KEY\n",
      "model_reasoning_level=high\n",
      "model_provider.openai.base_url=http://127.0.0.1:9\n",
      "model_provider.openai.api_key_env=ATTACKER_API_KEY\n",
      "secret_patterns=\n",
      "audit_enabled=false\n",
      "block_generated_secrets=false\n",
      "allowed_roots=/\n",
      "restricted_patterns=\n",
      "ignore_patterns=\n",
      "command_blocklist=\n",
      "require_approval_for_file_edits=false\n",
      "require_approval_for_risky_commands=false\n",
      "require_approval_for_all_commands=false\n",
      "mcp_enabled=true\n",
      "mcp_server_allowlist=attacker\n",
      "command_allowlist=npm install|make\n",
  );

  /// A checkout whose repository id is known before its user config is written,
  /// so the user config can select a profile for it. Unlike [`load`], the
  /// directory lives until `cleanup`: the id hashes its canonical path.
  struct ProfileFixture {
      root: PathBuf,
      data_dir: PathBuf,
      user_config: PathBuf,
      repository_config: PathBuf,
      repository_id: String,
  }

  fn profile_fixture(name: &str) -> ProfileFixture {
      let root = temp_dir(name);
      let data_dir = root.join(".damaian");
      let user_config = data_dir.join("config").join("user.conf");
      let repository_config = Config::repository_config_path(&root);
      fs::create_dir_all(user_config.parent().unwrap()).unwrap();
      fs::write(&user_config, "").unwrap();
      fs::write(&repository_config, "").unwrap();
      let repository_id = repository_id_for_root(&root);
      ProfileFixture {
          root,
          data_dir,
          user_config,
          repository_config,
          repository_id,
      }
  }

  impl ProfileFixture {
      fn base(&self) -> Config {
          Config {
              data_dir: self.data_dir.clone(),
              ..Config::default()
          }
      }

      /// The user config line selecting `profile` for this checkout.
      fn select(&self, profile: &str) -> String {
          format!("permission_profile.{}={profile}\n", self.repository_id)
      }

      fn write_user(&self, text: &str) {
          fs::write(&self.user_config, text).unwrap();
      }

      fn write_repository(&self, text: &str) {
          fs::write(&self.repository_config, text).unwrap();
      }

      fn write_custom_profile(&self, name: &str, text: &str) {
          let path = ProfileId::parse(name)
              .unwrap()
              .custom_path(&self.data_dir)
              .expect("a custom id has a file");
          fs::create_dir_all(path.parent().unwrap()).unwrap();
          fs::write(path, text).unwrap();
      }

      fn try_load(
          &self,
          admin: Option<&Path>,
      ) -> workspace_engine::Result<(Config, RepositoryConfigReport)> {
          Config::load_scoped(
              self.base(),
              Some(&self.user_config),
              Some(&self.repository_config),
              admin,
              Some(&self.root),
          )
      }

      fn load(&self) -> (Config, RepositoryConfigReport) {
          self.try_load(None).expect("fixture config should load")
      }

      /// The same files layered the way `load_scoped` did before profiles
      /// existed: user, then repository, then the `Allow Always` fold.
      fn resolve_without_profiles(&self) -> Config {
          let mut config = self.base();
          config.apply_overlay_scoped(
              ConfigOverlay::load(&self.user_config).unwrap(),
              ConfigScope::User,
          );
          let (repository, _) =
              ConfigOverlay::parse_untrusted(&fs::read_to_string(&self.repository_config).unwrap());
          config.apply_overlay_scoped(repository, ConfigScope::Repository);
          config.apply_repository_allowlist(&self.root);
          config
      }

      fn audit_log(&self) -> AuditLog {
          AuditLog::new(&self.data_dir, true, SecretScanner::new(Vec::new()))
      }

      fn audit_events(&self) -> String {
          fs::read_to_string(self.data_dir.join("audit").join("events.jsonl")).unwrap_or_default()
      }

      fn cleanup(self) {
          let _ = fs::remove_dir_all(self.root);
      }
  }

  #[test]
  fn with_no_profile_selected_the_resolved_config_is_todays() {
      let fixture = profile_fixture("no-selection");
      fixture.write_user(RESTRICTIVE_USER);
      fixture.write_repository(HOSTILE_REPOSITORY);

      let (config, report) = fixture.load();

      assert_eq!(config, fixture.resolve_without_profiles());
      assert_eq!(report.permission_profile, None);
      assert!(report.profile_rejected_keys.is_empty());

      fixture.cleanup();
  }

  #[test]
  fn selecting_full_or_another_checkouts_profile_changes_nothing_here() {
      let fixture = profile_fixture("full-selection");
      fixture.write_repository(HOSTILE_REPOSITORY);

      fixture.write_user(&format!("{RESTRICTIVE_USER}{}", fixture.select("full")));
      let (config, report) = fixture.load();
      assert_eq!(config, fixture.resolve_without_profiles());
      assert_eq!(report.permission_profile, Some(ProfileId::Full));
      assert!(report.profile_rejected_keys.is_empty());

      fixture.write_user(&format!(
          "{RESTRICTIVE_USER}permission_profile.repo_0123456789abcdef=read_only\n"
      ));
      let (config, report) = fixture.load();
      assert_eq!(config, fixture.resolve_without_profiles());
      assert_eq!(config.command_access, CommandAccess::All);
      assert_eq!(report.permission_profile, None);

      fixture.cleanup();
  }

  struct ProfileRow {
      id: ProfileId,
      capabilities: ProfileCapabilities,
      require_approval_for_file_edits: bool,
      audit_retention_days: u64,
      checkpoint_retention_days: u64,
  }

  #[test]
  fn each_built_in_profile_resolves_to_its_table_row() {
      let capabilities = |edits, access, browser, mutating_mcp, mcp| ProfileCapabilities {
          allow_file_edits: edits,
          command_access: access,
          allow_browser_diagnostics: browser,
          allow_mutating_mcp_tools: mutating_mcp,
          mcp_enabled: mcp,
      };
      // context.md §3, over a permissive user config that turned edit approval off.
      let rows = [
          ProfileRow {
              id: ProfileId::ReadOnly,
              capabilities: capabilities(false, CommandAccess::None, false, false, true),
              require_approval_for_file_edits: false,
              audit_retention_days: 90,
              checkpoint_retention_days: 90,
          },
          ProfileRow {
              id: ProfileId::SafeLocal,
              capabilities: capabilities(true, CommandAccess::Local, true, true, true),
              require_approval_for_file_edits: true,
              audit_retention_days: 90,
              checkpoint_retention_days: 90,
          },
          ProfileRow {
              id: ProfileId::Full,
              capabilities: capabilities(true, CommandAccess::All, true, true, true),
              require_approval_for_file_edits: false,
              audit_retention_days: 90,
              checkpoint_retention_days: 90,
          },
          ProfileRow {
              id: ProfileId::OfflinePrivate,
              capabilities: capabilities(true, CommandAccess::Local, false, true, false),
              require_approval_for_file_edits: false,
              audit_retention_days: 7,
              checkpoint_retention_days: 7,
          },
      ];
      let fixture = profile_fixture("built-ins");
      assert_eq!(
          Config::default().profile_capabilities(),
          capabilities(true, CommandAccess::All, true, true, true)
      );

      for row in rows {
          let name = row.id.as_str().to_string();
          fixture.write_user(&format!(
              "require_approval_for_file_edits=false\n{}",
              fixture.select(&name)
          ));
          let (config, report) = fixture.load();
          assert_eq!(config.profile_capabilities(), row.capabilities, "{name}");
          assert_eq!(
              config.require_approval_for_file_edits, row.require_approval_for_file_edits,
              "{name}"
          );
          assert_eq!(
              config.audit_retention_days, row.audit_retention_days,
              "{name}"
          );
          assert_eq!(
              config.checkpoint_retention_days, row.checkpoint_retention_days,
              "{name}"
          );
          assert_eq!(report.permission_profile, Some(row.id), "{name}");
          assert!(
              report.profile_rejected_keys.is_empty(),
              "{name}: a built-in carried a key a profile may not set: {:?}",
              report.profile_rejected_keys
          );
      }

      fixture.cleanup();
  }

  #[test]
  fn offline_private_only_lowers_the_retention_windows() {
      let fixture = profile_fixture("offline-retention");
      fixture.write_user(&format!(
          "audit_retention_days=3\ncheckpoint_retention_days=30\n{}",
          fixture.select("offline_private")
      ));

      let (config, report) = fixture.load();

      assert_eq!(config.audit_retention_days, 3, "7 must not raise 3");
      assert_eq!(config.checkpoint_retention_days, 7);
      assert!(
          report.profile_rejected_keys.is_empty(),
          "lower-wins is the documented merge, not a refusal: {:?}",
          report.profile_rejected_keys
      );

      fixture.cleanup();
  }

  /// The profile-scope counterpart of Task 1's weakening table: the user narrows
  /// every capability key, and a custom profile tries to loosen every one of
  /// them. Nothing may move.
  #[test]
  fn a_profile_cannot_loosen_anything_the_user_config_narrowed() {
      let fixture = profile_fixture("loosen-everything");
      // Task 1's table, except `checkpoint_retention_days`: its repository value
      // is a lower one, which a profile may apply (lower-wins), so the loosening
      // value here is a higher one.
      let cases: Vec<_> = weakening_cases()
          .into_iter()
          .filter(|case| case.field != "checkpoint_retention_days")
          .collect();
      let user: String = cases.iter().map(|case| case.user).collect();
      let profile: String = cases.iter().map(|case| case.repository).collect();
      fixture.write_user(&format!(
          "{user}checkpoint_retention_days=3\naudit_retention_days=3\n{}",
          fixture.select("loosen")
      ));
      fixture.write_custom_profile(
          "loosen",
          &format!(
              "{profile}checkpoint_retention_days=365\naudit_retention_days=365\n\
               max_file_bytes=1\nagent_tool_retry_limit=1\n"
          ),
      );

      let (config, report) = fixture.load();

      assert_eq!(
          config,
          fixture.resolve_without_profiles(),
          "the profile loosened something"
      );
      assert!(
          report.rejected_keys.is_empty(),
          "a profile's refusals are not the repository's: {:?}",
          report.rejected_keys
      );
      let refused: BTreeSet<&str> = report
          .profile_rejected_keys
          .iter()
          .map(|rejected| rejected.key.as_str())
          .collect();
      for case in &cases {
          if let Resists::Reported(key) = case.resists {
              assert!(
                  refused.contains(key),
                  "{}: not refused at profile scope; got {refused:?}",
                  case.field
              );
          }
      }
      // A profile narrows capability; it does not reconfigure preferences.
      assert!(refused.contains("max_file_bytes"), "{refused:?}");
      assert!(refused.contains("agent_tool_retry_limit"), "{refused:?}");

      fixture.cleanup();
  }

  #[test]
  fn a_profile_can_disable_an_mcp_server_but_not_define_one() {
      let fixture = profile_fixture("profile-mcp");
      fixture.write_user(&format!(
          "mcp_server.helper.command=/usr/local/bin/helper\nmcp_server.helper.enabled=true\n{}",
          fixture.select("mine")
      ));
      fixture.write_custom_profile(
          "mine",
          concat!(
              "mcp_server.helper.enabled=false\n",
              "mcp_server.newbie.command=/usr/bin/true\n",
              "mcp_server.newbie.auth_token_env=keychain:newbie\n",
          ),
      );

      let (config, report) = fixture.load();

      assert!(!config.mcp_server_config("helper").unwrap().enabled);
      assert!(
          config.mcp_server_config("newbie").is_none(),
          "a profile defined an MCP server"
      );
      for key in [
          "mcp_server.newbie.command",
          "mcp_server.newbie.auth_token_env",
      ] {
          assert!(
              report
                  .profile_rejected_keys
                  .iter()
                  .any(|r| r.key == key && r.class == RepositoryKeyClass::Forbidden),
              "{key}: {:?}",
              report.profile_rejected_keys
          );
      }

      fixture.cleanup();
  }

  #[test]
  fn a_repository_cannot_select_a_profile_in_either_direction() {
      let fixture = profile_fixture("repository-selects");
      let key = format!("permission_profile.{}", fixture.repository_id);

      // Narrowing: a repository choosing Read-only for itself.
      fixture.write_repository(&fixture.select("read_only"));
      let (config, report) = fixture.load();
      assert_eq!(config.command_access, CommandAccess::All);
      assert_eq!(report.permission_profile, None);
      assert!(
          report
              .rejected_keys
              .iter()
              .any(|r| r.key == key && r.class == RepositoryKeyClass::UserOwned),
          "{:?}",
          report.rejected_keys
      );

      // Widening: a repository undoing the user's Read-only.
      fixture.write_user(&fixture.select("read_only"));
      fixture.write_repository(&fixture.select("full"));
      let (config, report) = fixture.load();
      assert_eq!(config.command_access, CommandAccess::None);
      assert_eq!(report.permission_profile, Some(ProfileId::ReadOnly));

      fixture.cleanup();
  }

  #[test]
  fn admin_can_widen_the_base_but_not_undo_the_users_profile() {
      let fixture = profile_fixture("admin-vs-profile");
      fixture.write_user(&format!(
          "require_approval_for_all_commands=true\n{}",
          fixture.select("read_only")
      ));
      let admin = fixture.data_dir.join("config").join("admin.conf");
      fs::write(
          &admin,
          concat!(
              "require_approval_for_all_commands=false\n",
              "command_access=all\n",
              "allow_file_edits=true\n",
          ),
      )
      .unwrap();

      let (config, _) = fixture.try_load(Some(&admin)).unwrap();

      assert!(
          !config.require_approval_for_all_commands,
          "admin widened the base"
      );
      assert_eq!(config.command_access, CommandAccess::None);
      assert!(!config.allow_file_edits);

      fixture.cleanup();
  }

  #[test]
  fn profile_refusals_stay_out_of_the_repository_notice_and_are_audited_once() {
      let fixture = profile_fixture("profile-audit");
      fixture.write_user(&fixture.select("mine"));
      fixture.write_custom_profile("mine", "shell=/tmp/evil-shell\nmax_file_bytes=1\n");
      let audit = fixture.audit_log();

      let (_, report) = fixture.load();

      assert!(
          report.rejected_keys.is_empty(),
          "{:?}",
          report.rejected_keys
      );
      assert!(report.is_empty(), "no repository notice for a profile");
      assert_eq!(
          report
              .profile_rejected_keys
              .iter()
              .map(|r| (r.key.as_str(), r.class))
              .collect::<Vec<_>>(),
          vec![
              ("shell", RepositoryKeyClass::Forbidden),
              ("max_file_bytes", RepositoryKeyClass::Forbidden),
          ]
      );
      assert!(
          RepositoryTrustStore::new(&fixture.data_dir)
              .review(&report, &audit)
              .unwrap()
              .is_none()
      );

      let fresh = review_profile_rejections(&fixture.data_dir, &report, &audit).unwrap();
      assert_eq!(fresh.len(), 2);
      let again = review_profile_rejections(&fixture.data_dir, &report, &audit).unwrap();
      assert!(again.is_empty(), "audited twice: {again:?}");

      let events = fixture.audit_events();
      assert_eq!(
          events.matches("permission_profile_key_rejected").count(),
          2,
          "{events}"
      );
      assert!(events.contains("\"profileId\":\"mine\""), "{events}");
      assert!(events.contains("\"key\":\"shell\""), "{events}");
      assert!(events.contains("\"class\":\"forbidden\""), "{events}");
      assert!(!events.contains("evil-shell"), "a refused value was logged");
      assert!(
          !events.contains("repository_config_key_rejected"),
          "{events}"
      );

      fixture.cleanup();
  }

  #[test]
  fn a_malformed_custom_profile_line_is_reported_not_fatal() {
      let fixture = profile_fixture("profile-malformed");
      fixture.write_user(&fixture.select("mine"));
      fixture.write_custom_profile(
          "mine",
          "command_access=everything\nallow_file_edits=false\nnot a config line\n",
      );

      let (config, report) = fixture.load();

      assert!(!config.allow_file_edits, "the good line still applies");
      assert_eq!(config.command_access, CommandAccess::All);
      for key in ["command_access", "line 3"] {
          assert!(
              report
                  .profile_rejected_keys
                  .iter()
                  .any(|r| r.key == key && r.class == RepositoryKeyClass::Unparsable),
              "{key}: {:?}",
              report.profile_rejected_keys
          );
      }

      fixture.cleanup();
  }

  #[test]
  fn a_selected_custom_profile_with_no_file_fails_the_load_instead_of_widening() {
      let fixture = profile_fixture("profile-missing");
      fixture.write_user(&fixture.select("gone"));

      let error = fixture
          .try_load(None)
          .expect_err("a missing profile must not resolve as Full")
          .to_string();

      assert!(error.contains("gone"), "{error}");
      assert!(error.contains("profile-set"), "{error}");

      fixture.cleanup();
  }

  #[test]
  fn profile_ids_parse_the_built_ins_and_validate_custom_names() {
      for (name, id) in [
          ("read_only", ProfileId::ReadOnly),
          ("safe_local", ProfileId::SafeLocal),
          ("full", ProfileId::Full),
          ("offline_private", ProfileId::OfflinePrivate),
      ] {
          assert_eq!(ProfileId::parse(name).unwrap(), id);
          assert_eq!(id.as_str(), name);
          assert!(id.custom_path("/tmp").is_none(), "{name} has no file");
          assert!(ProfileId::custom(name).is_err(), "{name} is reserved");
      }
      assert_eq!(
          ProfileId::parse("team_ci_2").unwrap(),
          ProfileId::Custom("team_ci_2".to_string())
      );
      assert!(ProfileId::parse(&"x".repeat(40)).is_ok());
      for bad in ["", "Has-Dash", "UPPER", "../escape", "a b", &"x".repeat(41)] {
          assert!(ProfileId::parse(bad).is_err(), "{bad:?} parsed");
      }
      assert_eq!(
          ProfileId::parse("mine")
              .unwrap()
              .custom_path("/data")
              .unwrap(),
          PathBuf::from("/data/config/profiles/mine.conf")
      );
  }

  #[test]
  fn the_selection_round_trips_through_the_overlay_text_and_is_validated() {
      let text = "permission_profile.repo_0123456789abcdef=safe_local\n";
      let overlay = ConfigOverlay::parse(text).unwrap();
      assert_eq!(overlay.to_policy_text(), text);

      assert!(ConfigOverlay::parse("permission_profile.repo_0123456789abcdef=Nope!\n").is_err());
      assert!(ConfigOverlay::parse("permission_profile.not_a_repo=full\n").is_err());
  }

  #[test]
  fn selecting_a_profile_writes_user_config_and_audits_the_change() {
      let fixture = profile_fixture("profile-set");
      fixture.write_user("shell=/bin/zsh\n");
      let audit = fixture.audit_log();
      let config = fixture.base();

      let first = select_profile(&config, &fixture.root, ProfileId::ReadOnly, &audit).unwrap();
      assert_eq!(first.repository_id, fixture.repository_id);
      assert_eq!(first.previous, None);
      assert_eq!(first.selected, ProfileId::ReadOnly);
      let user = fs::read_to_string(&fixture.user_config).unwrap();
      assert!(user.contains("shell=/bin/zsh"), "{user}");
      assert!(user.contains(fixture.select("read_only").trim()), "{user}");
      assert_eq!(fixture.load().0.command_access, CommandAccess::None);

      let second = select_profile(&config, &fixture.root, ProfileId::SafeLocal, &audit).unwrap();
      assert_eq!(second.previous, Some(ProfileId::ReadOnly));

      let missing = ProfileId::parse("gone").unwrap();
      assert!(select_profile(&config, &fixture.root, missing, &audit).is_err());
      assert!(
          fs::read_to_string(&fixture.user_config)
              .unwrap()
              .contains(fixture.select("safe_local").trim()),
          "a refused selection must not be written"
      );

      let events = fixture.audit_events();
      assert_eq!(
          events.matches("permission_profile_set").count(),
          2,
          "{events}"
      );
      assert!(
          events.contains(&format!("\"repositoryId\":\"{}\"", fixture.repository_id)),
          "{events}"
      );
      assert!(events.contains("\"from\":\"none\""), "{events}");
      assert!(events.contains("\"to\":\"read_only\""), "{events}");
      assert!(events.contains("\"from\":\"read_only\""), "{events}");
      assert!(events.contains("\"to\":\"safe_local\""), "{events}");

      fixture.cleanup();
  }
  ```

- [x] **Step 2: Run the tests and confirm they fail**

  Run: `cargo nextest run -p workspace-engine --test permission_profiles`

  Expected: a compile error. `ProfileId`, `ProfileCapabilities`,
  `select_profile` and `review_profile_rejections` are unresolved, the report
  has no `permission_profile` or `profile_rejected_keys`, and `Config` has no
  `==`. Observed: 25 errors, all of those kinds.

- [x] **Step 3: Write the implementation**

  Create `crates/workspace-engine/src/profile.rs`:

  ```rust
  //! Named permission profiles (spec 31, `docs/specs/31_permission_profiles/`).
  //!
  //! A profile is a bundle of capability-key values applied after admin config
  //! at [`ConfigScope::Profile`](crate::config::ConfigScope::Profile), which is
  //! restrict-only: a profile can narrow what user, repository and admin config
  //! resolved, never loosen it (`context.md` §4). The selection is the user's,
  //! stored in user config per checkout as `permission_profile.<repository_id>`.

  use crate::audit::AuditLog;
  use crate::config::{
      CommandAccess, Config, ConfigOverlay, RejectedConfigKey, RepositoryConfigReport,
  };
  use crate::error::{ClientError, Result};
  use crate::hash::repository_id_for_root;
  use serde::{Deserialize, Serialize};
  use std::fs;
  use std::path::{Path, PathBuf};

  const MAX_CUSTOM_NAME_CHARS: usize = 40;

  /// Which profile a checkout runs under. No selection behaves as
  /// [`ProfileId::Full`], the empty overlay.
  #[derive(Debug, Clone, PartialEq, Eq)]
  pub enum ProfileId {
      ReadOnly,
      SafeLocal,
      Full,
      OfflinePrivate,
      /// A user-authored overlay file under `<data_dir>/config/profiles/`.
      Custom(String),
  }

  impl ProfileId {
      /// A built-in id, or a custom name of `[a-z0-9_]{1,40}`.
      pub fn parse(value: &str) -> Result<Self> {
          match value {
              "read_only" => Ok(ProfileId::ReadOnly),
              "safe_local" => Ok(ProfileId::SafeLocal),
              "full" => Ok(ProfileId::Full),
              "offline_private" => Ok(ProfileId::OfflinePrivate),
              _ => Self::custom(value),
          }
      }

      /// A custom profile name. The built-in ids are reserved, so an imported
      /// file can never be mistaken for one.
      pub fn custom(name: &str) -> Result<Self> {
          if matches!(
              name,
              "read_only" | "safe_local" | "full" | "offline_private"
          ) {
              return Err(ClientError::InvalidInput(format!(
                  "{name} is a built-in permission profile and cannot be used as a custom name"
              )));
          }
          if !is_valid_custom_name(name) {
              return Err(ClientError::InvalidInput(format!(
                  "A permission profile name must be 1 to {MAX_CUSTOM_NAME_CHARS} characters \
                   of a-z, 0-9 and _"
              )));
          }
          Ok(ProfileId::Custom(name.to_string()))
      }

      pub fn as_str(&self) -> &str {
          match self {
              ProfileId::ReadOnly => "read_only",
              ProfileId::SafeLocal => "safe_local",
              ProfileId::Full => "full",
              ProfileId::OfflinePrivate => "offline_private",
              ProfileId::Custom(name) => name,
          }
      }

      /// Where a custom profile's file lives; `None` for a built-in. The name is
      /// checked again here because `Custom` can be built without [`Self::parse`],
      /// and it becomes a path segment.
      pub fn custom_path(&self, data_dir: impl AsRef<Path>) -> Option<PathBuf> {
          match self {
              ProfileId::Custom(name) if is_valid_custom_name(name) => Some(
                  data_dir
                      .as_ref()
                      .join("config")
                      .join("profiles")
                      .join(format!("{name}.conf")),
              ),
              _ => None,
          }
      }

      /// The profile's overlay, and the lines of a custom file that did not
      /// parse. A custom file is parsed like repository config, because Task 8
      /// imports them: a bad line is reported and skipped, not fatal. A missing
      /// file is an error, because resolving a selected profile as Full would
      /// silently widen what the user chose.
      pub fn overlay(
          &self,
          data_dir: impl AsRef<Path>,
      ) -> Result<(ConfigOverlay, Vec<RejectedConfigKey>)> {
          // The values in `context.md` §3's table.
          let overlay = match self {
              ProfileId::ReadOnly => ConfigOverlay {
                  allow_file_edits: Some(false),
                  command_access: Some(CommandAccess::None),
                  allow_browser_diagnostics: Some(false),
                  allow_mutating_mcp_tools: Some(false),
                  ..ConfigOverlay::default()
              },
              ProfileId::SafeLocal => ConfigOverlay {
                  command_access: Some(CommandAccess::Local),
                  require_approval_for_file_edits: Some(true),
                  ..ConfigOverlay::default()
              },
              ProfileId::Full => ConfigOverlay::default(),
              ProfileId::OfflinePrivate => ConfigOverlay {
                  command_access: Some(CommandAccess::Local),
                  allow_browser_diagnostics: Some(false),
                  mcp_enabled: Some(false),
                  audit_retention_days: Some(7),
                  checkpoint_retention_days: Some(7),
                  ..ConfigOverlay::default()
              },
              ProfileId::Custom(name) => {
                  let path = self.custom_path(&data_dir).ok_or_else(|| {
                      ClientError::InvalidInput(format!("Invalid permission profile name: {name}"))
                  })?;
                  let content = fs::read_to_string(&path).map_err(|error| {
                      ClientError::InvalidInput(format!(
                          "Permission profile {name} is selected but {} cannot be read ({error}). \
                           Restore the file, or choose another profile with \
                           `damaian profile-set <repo> full`.",
                          path.display()
                      ))
                  })?;
                  return Ok(ConfigOverlay::parse_untrusted(&content));
              }
          };
          Ok((overlay, Vec::new()))
      }
  }

  fn is_valid_custom_name(name: &str) -> bool {
      (1..=MAX_CUSTOM_NAME_CHARS).contains(&name.len())
          && name
              .chars()
              .all(|character| matches!(character, 'a'..='z' | '0'..='9' | '_'))
  }

  /// The resolved values the refusal points consult (spec 31 Task 5): the four
  /// profile keys plus the MCP kill-switch.
  #[derive(Debug, Clone, Copy, PartialEq, Eq)]
  pub struct ProfileCapabilities {
      pub allow_file_edits: bool,
      pub command_access: CommandAccess,
      pub allow_browser_diagnostics: bool,
      pub allow_mutating_mcp_tools: bool,
      pub mcp_enabled: bool,
  }

  /// The result of [`select_profile`].
  #[derive(Debug, Clone, PartialEq, Eq)]
  pub struct ProfileSelection {
      pub repository_id: String,
      pub previous: Option<ProfileId>,
      pub selected: ProfileId,
  }

  /// Writes `permission_profile.<repository_id>=<id>` to user config and audits
  /// the change. A custom profile must already exist, since selecting one that
  /// does not would make every later load of this checkout fail.
  ///
  /// `config` supplies the data directory and user config path; it should be
  /// loaded without a repository, so a checkout whose current profile file is
  /// missing can still be switched away from it.
  pub fn select_profile(
      config: &Config,
      repository_root: &Path,
      id: ProfileId,
      audit_log: &AuditLog,
  ) -> Result<ProfileSelection> {
      if let ProfileId::Custom(name) = &id {
          let exists = id
              .custom_path(&config.data_dir)
              .is_some_and(|path| path.is_file());
          if !exists {
              return Err(ClientError::InvalidInput(format!(
                  "No custom permission profile named {name}"
              )));
          }
      }
      let repository_id = repository_id_for_root(repository_root);
      let path = config.user_config_path();
      let mut overlay = ConfigOverlay::load_or_default(&path)?;
      let previous = overlay
          .permission_profile_by_repository
          .insert(repository_id.clone(), id.clone());
      overlay.save(&path)?;
      audit_log.record(
          "permission_profile_set",
          &[
              ("actor", "user".to_string()),
              ("repositoryId", repository_id.clone()),
              (
                  "repositoryPath",
                  repository_root.to_string_lossy().to_string(),
              ),
              (
                  "from",
                  previous
                      .as_ref()
                      .map_or("none", ProfileId::as_str)
                      .to_string(),
              ),
              ("to", id.as_str().to_string()),
          ],
      )?;
      Ok(ProfileSelection {
          repository_id,
          previous,
          selected: id,
      })
  }

  #[derive(Debug, Clone, Default, Serialize, Deserialize)]
  #[serde(rename_all = "camelCase")]
  struct ProfileReviewState {
      #[serde(default)]
      reported_keys: Vec<String>,
  }

  /// Audits the keys the selected profile carried and could not apply, each once
  /// per profile, and returns the ones not reported before.
  ///
  /// Kept apart from `RepositoryTrustStore::review` on purpose: a profile's
  /// refusals are not a repository's, so they must not reach spec 34's
  /// repository notice. Once per profile rather than per load for the same
  /// reason as that store: config is loaded on every request.
  pub fn review_profile_rejections(
      data_dir: &Path,
      report: &RepositoryConfigReport,
      audit_log: &AuditLog,
  ) -> Result<Vec<RejectedConfigKey>> {
      let Some(profile) = report.permission_profile.as_ref() else {
          return Ok(Vec::new());
      };
      if report.profile_rejected_keys.is_empty() {
          return Ok(Vec::new());
      }
      let state_path = data_dir
          .join("config")
          .join("profile-review")
          .join(format!("{}.json", profile.as_str()));
      // A corrupt state file must not make a repository unopenable; the worst
      // case of treating it as absent is a key audited twice.
      let mut state: ProfileReviewState = fs::read_to_string(&state_path)
          .ok()
          .and_then(|content| serde_json::from_str(&content).ok())
          .unwrap_or_default();
      let fresh = report
          .profile_rejected_keys
          .iter()
          .filter(|rejected| !state.reported_keys.contains(&rejected.key))
          .cloned()
          .collect::<Vec<_>>();
      if fresh.is_empty() {
          return Ok(fresh);
      }

      let repository_id = report
          .repository_root
          .as_ref()
          .map(repository_id_for_root)
          .unwrap_or_default();
      for rejected in &fresh {
          audit_log.record(
              "permission_profile_key_rejected",
              &[
                  ("actor", "system".to_string()),
                  ("profileId", profile.as_str().to_string()),
                  ("repositoryId", repository_id.clone()),
                  ("key", rejected.key.clone()),
                  ("class", rejected.class.as_str().to_string()),
              ],
          )?;
          state.reported_keys.push(rejected.key.clone());
      }
      if let Some(parent) = state_path.parent() {
          fs::create_dir_all(parent)?;
      }
      let json = serde_json::to_string(&state).map_err(|error| {
          ClientError::InvalidInput(format!(
              "Failed to serialize permission profile review state: {error}"
          ))
      })?;
      fs::write(state_path, json)?;
      Ok(fresh)
  }
  ```

  In `crates/workspace-engine/src/config.rs`:

  - `use crate::profile::{ProfileCapabilities, ProfileId};`.
  - `ConfigScope` gains `Profile`, documented as restrict-only by design.
  - `RepositoryConfigReport` gains `permission_profile` and
    `profile_rejected_keys` (deviation 6).
  - `Config` derives `PartialEq`. `Config` and `ConfigOverlay` gain
    `permission_profile_by_repository` directly after
    `command_allowlist_by_repository`. `Config::default` gives it an empty map.
  - `ConfigOverlay::set` routes the prefix `permission_profile.` to
    `set_repository_permission_profile`, which requires `repo_` and
    `ProfileId::parse`s the value. `ConfigOverlay::to_policy_text` writes one
    `permission_profile.<id>=<profile>` line per entry, since the exhaustive
    destructure forces it. `Config::to_policy_text` does not print the map,
    for the same reason it skips the allowlist map.
  - `classify_overlay_fields!` gains
    `permission_profile_by_repository => Capability`.
  - `Config::profile_capabilities`, before `Config::to_policy_text`.
  - In `load_scoped`, after the admin overlay and before
    `apply_repository_allowlist`:

    ```rust
            if let Some(root) = repository_root
                && let Some(profile) = config
                    .permission_profile_by_repository
                    .get(&repository_id_for_root(root))
                    .cloned()
            {
                let (overlay, mut rejected) = profile.overlay(&config.data_dir)?;
                rejected.extend(config.apply_overlay_scoped(overlay, ConfigScope::Profile));
                report.profile_rejected_keys = rejected;
                report.permission_profile = Some(profile);
            }
    ```

  - In `apply_overlay_scoped`, bind the new field in the destructure. Replace
    `let trusted = scope != ConfigScope::Repository;` with:

    ```rust
            let trusted = match scope {
                ConfigScope::User | ConfigScope::Admin => true,
                ConfigScope::Repository | ConfigScope::Profile => false,
            };
    ```

    Then:
    - `permission_profile_by_repository`: the same shape as
      `command_allowlist_by_repository`. Trusted scopes insert. Otherwise
      each entry is refused as `permission_profile.<id>` / `UserOwned`.
    - `mcp_servers`: an exhaustive `match scope`. User and Admin upsert.
      Repository calls `upsert_mcp_server_from_repository(server, true, ..)`,
      Profile calls it with `false` (deviation 3). With `may_define == false`,
      definition fields are refused `Forbidden` whether or not the server
      exists, and an unconfigured server returns before anything is upserted.
    - `checkpoint_retention_days`: User, Repository and Admin keep
      `scoped(.., forbidden, ..)`. Profile calls `lower_wins`.
    - `audit_retention_days`: User, Repository and Admin assign. Profile calls
      `lower_wins`.
    - The other six Free keys go through
      `preference(value, key, scope, &mut rejected)`. It returns the value at
      User, Repository and Admin, and at Profile records `Forbidden` and
      returns `None`.
  - New helpers beside `scoped`: `preference<T>` as above, and
    `fn lower_wins(current: &mut u64, value: u64)`, which assigns only when
    `value < *current` and records nothing (deviation 4).

  In `crates/workspace-engine/src/lib.rs`, add `pub mod profile;` and
  `pub use profile::{ProfileCapabilities, ProfileId, ProfileSelection, review_profile_rejections, select_profile};`.

  In `crates/damaian-cli/src/main.rs`:
  - a `profile-set <repo> <profile>` usage line and dispatch arm;
  - `set_permission_profile`, which does `ProfileId::parse`, then
    `Config::load_for_repository(None)` (deviation 2), and prints the id with
    the previous one;
  - `repository_config_review` calls `review_profile_rejections` after the
    repository notice and prints each fresh key as
    `profile <id> could not apply <key> (<class>)`.

- [x] **Step 4: Run the tests and confirm they pass**

  Run: `cargo nextest run -p workspace-engine --test permission_profiles`

  Expected: 24 tests pass: the 10 from Tasks 1–2 and 14 new ones.

- [x] **Step 5: Mutation-test the load-bearing guarantees**

  Do each one, confirm it fails as described, revert it, and record the
  results in the progress row:

  1. Move `ConfigScope::Profile` to the trusted arm. Expected:
     `a_profile_cannot_loosen_anything_the_user_config_narrowed` fails. This
     is the outline's mutation.
  2. `.or(Some(ProfileId::ReadOnly))` after the selection lookup in
     `load_scoped`. Expected: `with_no_profile_selected_the_resolved_config_is_todays`
     fails.
  3. Apply the profile before the admin overlay. Expected:
     `admin_can_widen_the_base_but_not_undo_the_users_profile` fails.
  4. `preference` returns the value at Profile scope. Expected: the loosen
     test and the audit test fail.
  5. Profile calls `upsert_mcp_server_from_repository(.., true, ..)`.
     Expected: `a_profile_can_disable_an_mcp_server_but_not_define_one` fails.
  6. `lower_wins` always assigns. Expected:
     `offline_private_only_lowers_the_retention_windows` fails.
  7. Also extend the profile's refusals into `report.rejected_keys`.
     Expected: the audit test fails.
  8. `review_profile_rejections` never pushes to `reported_keys`. Expected:
     "audited twice".
  9. Trust the selection map at repository scope. Expected:
     `a_repository_cannot_select_a_profile_in_either_direction` and Task 1's
     weakening case fail.

- [x] **Step 6: Confirm spec 34's floor is untouched**

  Run: `cargo nextest run -p workspace-engine --test repository_config_trust --test foundation`

  Expected: all pass. `repository_config_trust.rs` is unmodified.

- [x] **Step 7: Scoped checks**

  ```bash
  cargo fmt --all -- --check
  cargo clippy -p workspace-engine -p damaian-cli --all-targets --locked -- -D warnings
  typos
  cargo check -p desktop-shell -p eval-harness --all-targets --locked
  ```

  Then drive the CLI once against a scratch `DAMAIAN_DATA_DIR` and
  repository:
  - `profile-set` built-in and custom, plus a bad name and a missing custom;
  - `config-show` under each;
  - `config-review` twice (the profile line appears once);
  - `config-show` after deleting the selected custom file (the load fails
    with the fix);
  - `profile-set <repo> full` to recover.

- [x] **Step 8: Update this file's Task 3 row, then show the change and the
  check results and ask before committing**

  Suggested subject: `Apply a per-repository permission profile last and restrict-only`.

## Task 4: `command_access` enforced in `CommandPolicy` as a block

**Requirements:** 3 and 5 for commands, on every execution path (`context.md`
§7, observation 10). **Files:** `command_policy.rs`, and
`tests/permission_profiles.rs`.

**Also modified:** `mode.rs`, one line (deviation 2). **Not touched:**
`chat.rs`, `validation.rs` (deviation 3), `repository_config_trust.rs`.

`CommandPolicy` blocks a command the resolved `command_access` does not
permit. Task 3 already narrows `command_access` by the profile before any
engine is built, so the policy reads only `self.config.command_access`:

- `None`: every command.
- `ReadOnly`: anything Plan mode would refuse, meaning anything that is not
  Low risk, needs approval, or fails `is_low_risk_read_only`. This is the
  predicate `mode_permits` uses, moved so that both call one function
  (deviation 2).
- `Local`: anything `may_use_network(command)` flags, judged by the command
  **text** (deviation 4).
- `All`: nothing.

The block sets `blocked: true` and appends the reason
`Blocked by permission profile: command_access=<level>`. It never changes
`risk`, `requires_approval`, `may_use_network` or `expected_effects`. An
allowlisted command is still blocked, because Allow Always cannot outrank a
profile. A command that is already blocked (blocklist or hard block) gets no
second reason.

**Deviations from the outline, checked against the code on 2026-09-30:**

1. **The block is applied in `classify`, after the whole classification, not
   inside `classify_pattern` "before the allowlist".** `classify_pattern`
   returns early from each branch with that branch's risk. An early `blocked`
   return before the allowlist would have to invent a risk, which proposal §4
   forbids. It would also run before the outside-root check in `classify` sets
   `requires_approval`, so `ReadOnly` would let `ls ../elsewhere` through
   although Plan mode refuses it. Applying the block after the full
   classification keeps every other field exactly as it was and still blocks
   allowlisted commands.
2. **The Plan predicate moves to `command_policy.rs`.** The outline says
   "read it from `mode.rs`, do not copy it". But in `mode.rs` it is an inline
   expression inside `mode_permits` (`read_only_no_approval`), not a function,
   and `command_policy.rs` cannot depend on `mode.rs` without inverting the
   module order (`mode.rs` imports `command_policy`). So it becomes
   `CommandClassification::is_read_only_without_approval`, and `mode_permits`
   calls it. That is a one-line change in `mode.rs`, which Task 5 owns for
   everything else. `the_permission_matrix_matches_the_spec_table` pins that
   the mode behaviour is unchanged.
3. **`run_proposal` needs no change.** `context.md` §7 says a stored proposal
   run by id must be re-classified. It already is: `run_proposal` calls
   `CommandRunner::run`, which classifies the command again with its own
   `CommandPolicy` (`command_runner.rs:194`) and returns
   `ClientError::PolicyBlocked` on `blocked` (`:215`). The desktop shell and
   CLI build that policy from the current config for each request. The stored
   `proposal.blocked` flag is stale, but the runner does not rely on it.
   Task 4 pins this with a test and does not add a second check.
4. **`Local` reads `may_use_network` on the command text, not the
   classification's `may_use_network` field.** The allowlist and read-only
   branches hard-code the field to `false`, and so does the validation
   branch for `npm test`. Reading the field would let Allow Always of
   `npm ci` pass `local`, which is the exact escape the outline forbids. As a
   result, `local` also blocks `npm test` and `npm run build`, but not
   `cargo test` or `pytest`, because `npm` is on the name list
   (`context.md` §7). Task 9's user guide must say this.
5. **`command_access=read_only` blocks everything when
   `require_approval_for_all_commands=true`.** Every classification then
   needs approval, which is also why Plan mode refuses every command under
   that setting. This is inherited, not new.
6. **The invariance test compares each level with `All`, not "before and
   after this change".** A test cannot run the old code. With the same
   config, `All` is the old code: it never blocks. The test compares every
   field except `blocked` and `reasons` across the four levels, and pins
   literal values under `All` for three commands.

**Interfaces:**
- Consumes `Config::command_access` and `CommandAccess` (Task 2). Nothing in
  `config.rs` changes except the `CommandAccess` doc comment, which currently
  says nothing enforces it.
- Produces:
  - `pub(crate) fn CommandClassification::is_read_only_without_approval(&self) -> bool`
    in `command_policy.rs`, used by `mode_permits` and the `ReadOnly` level.
  - `fn command_access_permits(CommandAccess, &CommandClassification) -> bool`,
    which is private.
  - The reason text `Blocked by permission profile: command_access=<level>`.
    Task 5's refusal wording may quote it.
- **For Task 5:** a profile-blocked command in the main loop currently takes
  the existing blocked path. The proposal is stored with `blocked: true`, and
  the turn pauses with `command_proposal_response`'s "local policy blocks this
  command" (`chat.rs:2198`). Approving it makes `run_proposal` fail with
  `PolicyBlocked`, which the resume branch propagates as a turn error
  (`chat.rs:975`, `?`). That is the same as a blocklisted command today.
  Task 5's `profile_permits` refuses a `blocked` command before proposing, so
  it becomes a clean profile refusal. Task 5 must also re-classify the stored
  proposal at resume and not trust `proposal.blocked`, because the stored flag
  predates any profile switch (`context.md` §6).

- [x] **Step 1: Write the failing tests**

  Append to `crates/workspace-engine/tests/permission_profiles.rs`, and add
  `CancelToken`, `ClientError`, `CommandClassification`, `CommandPolicy`,
  `CommandRisk` and `WorkspaceEngine` to its `workspace_engine` import:

  ```rust
  // Task 4: `command_access` is enforced in `CommandPolicy` as a block
  // (`context.md` §7), so every execution path sees it, including a stored
  // proposal run by id.

  const ACCESS_LEVELS: [CommandAccess; 4] = [
      CommandAccess::None,
      CommandAccess::ReadOnly,
      CommandAccess::Local,
      CommandAccess::All,
  ];

  fn classify_under(access: CommandAccess, command: &str, allowlisted: bool) -> CommandClassification {
      let config = Config {
          command_access: access,
          command_allowlist: if allowlisted { vec![command.to_string()] } else { Vec::new() },
          ..Config::default()
      };
      CommandPolicy::new(config).classify(command, Path::new("/Users/example/project"))
  }

  /// (command, allowlisted, blocked under [none, read_only, local, all]).
  const ACCESS_TABLE: &[(&str, bool, [bool; 4])] = &[
      ("cargo test", false, [true, true, false, false]),
      ("ls", false, [true, false, false, false]),
      ("git diff", false, [true, false, false, false]),
      // Plan mode refuses this because the path escape needs approval.
      ("ls ../elsewhere", false, [true, true, false, false]),
      ("curl example.com", false, [true, true, true, false]),
      ("npm ci", false, [true, true, true, false]),
      // Allow Always cannot outrank a profile, at any level.
      ("npm ci", true, [true, true, true, false]),
  ];

  #[test]
  fn each_command_access_level_blocks_exactly_its_row() {
      for (command, allowlisted, expected) in ACCESS_TABLE {
          for (access, blocked) in ACCESS_LEVELS.iter().zip(expected) {
              let classification = classify_under(*access, command, *allowlisted);
              assert_eq!(
                  classification.blocked,
                  *blocked,
                  "{command} (allowlisted: {allowlisted}) under command_access={}",
                  access.as_str()
              );
              let reason = format!(
                  "Blocked by permission profile: command_access={}",
                  access.as_str()
              );
              assert_eq!(
                  classification.reasons.contains(&reason),
                  *blocked,
                  "{command} under {}: {:?}",
                  access.as_str(),
                  classification.reasons
              );
          }
      }
  }

  /// Proposal §4: a profile may block a command, never reclassify it.
  #[test]
  fn a_command_access_block_changes_nothing_but_blocked_and_reasons() {
      let samples = ACCESS_TABLE
          .iter()
          .map(|(command, allowlisted, _)| (*command, *allowlisted))
          .chain([("rm -rf /", false), ("git push", false)]);
      for (command, allowlisted) in samples {
          let all = classify_under(CommandAccess::All, command, allowlisted);
          for access in ACCESS_LEVELS {
              let narrowed = classify_under(access, command, allowlisted);
              let context = format!("{command} under {}", access.as_str());
              assert_eq!(narrowed.command, all.command, "{context}");
              assert_eq!(narrowed.risk, all.risk, "{context}");
              assert_eq!(narrowed.requires_approval, all.requires_approval, "{context}");
              assert_eq!(narrowed.may_use_network, all.may_use_network, "{context}");
              assert_eq!(narrowed.expected_effects, all.expected_effects, "{context}");
              if all.blocked {
                  // Already blocked by local policy: no second reason.
                  assert_eq!(narrowed.reasons, all.reasons, "{context}");
              } else {
                  assert!(narrowed.reasons.starts_with(&all.reasons), "{context}");
              }
          }
      }

      // `All` is today's classifier, so pin it literally too.
      let curl = classify_under(CommandAccess::All, "curl example.com", false);
      assert_eq!(
          (curl.risk, curl.requires_approval, curl.blocked),
          (CommandRisk::High, true, false)
      );
      let ls = classify_under(CommandAccess::All, "ls", false);
      assert_eq!(
          (ls.risk, ls.requires_approval, ls.blocked),
          (CommandRisk::Low, false, false)
      );
      let allowlisted = classify_under(CommandAccess::All, "npm ci", true);
      assert_eq!(
          (allowlisted.risk, allowlisted.requires_approval, allowlisted.blocked),
          (CommandRisk::Low, false, false)
      );
  }

  /// `context.md` §7, observation 10: a proposal stored while commands were
  /// allowed must not run by id after the profile narrows. The shell is a path
  /// that does not exist, so if the block ever fails, the spawn fails with a
  /// different error instead of running a real login shell.
  #[test]
  fn a_proposal_stored_under_all_is_refused_by_id_once_command_access_narrows() {
      let root = temp_dir("run-by-id");
      let data_dir = root.join(".damaian");
      let engine = |access| {
          WorkspaceEngine::new(Config {
              data_dir: data_dir.clone(),
              command_access: access,
              enable_index_watcher: false,
              shell: "/nonexistent/damaian-profile-test-shell".to_string(),
              ..Config::default()
          })
      };
      let proposal = engine(CommandAccess::All)
          .validation_orchestrator
          .propose_command(&root, "ls", "list the checkout")
          .unwrap();
      assert!(!proposal.blocked);

      let run = |access| {
          let mut on_output = |_line: &str| {};
          engine(access).validation_orchestrator.run_proposal(
              &proposal.id,
              true,
              "tester",
              None,
              &CancelToken::new(),
              &mut on_output,
          )
      };
      let narrowed = run(CommandAccess::None);
      assert!(
          matches!(narrowed, Err(ClientError::PolicyBlocked(_))),
          "{narrowed:?}"
      );
      // Control: under `All` the same id gets past policy and fails only at
      // the missing shell, so the refusal above is the profile's.
      let control = run(CommandAccess::All);
      assert!(
          control.is_err() && !matches!(control, Err(ClientError::PolicyBlocked(_))),
          "{control:?}"
      );

      let _ = fs::remove_dir_all(&root);
  }
  ```

- [x] **Step 2: Run them and confirm they fail for the right reason**

  ```bash
  cargo nextest run -p workspace-engine --test permission_profiles -E 'test(command_access) | test(by_id)'
  ```

  Expected: `each_command_access_level_blocks_exactly_its_row` fails on its
  first `None` row, because nothing blocks yet. The run-by-id test fails
  because `run_proposal` under `None` reaches the missing shell instead of
  `PolicyBlocked`. The invariance test passes before the change, since
  nothing blocks. It is a regression guard, and the mutations in Step 5 make
  it fail.

- [x] **Step 3: Implement**

  In `command_policy.rs`, import `CommandAccess` and add:

  ```rust
  impl CommandClassification {
      /// What Plan and Review mode allow, and what `command_access=read_only`
      /// allows: low risk, no approval, *and* read-only by its text. The
      /// third check stops the allowlist, which also yields low risk with no
      /// approval, from widening either one (spec 20 `context.md` §5). One
      /// function, so the mode and the profile cannot drift apart.
      pub(crate) fn is_read_only_without_approval(&self) -> bool {
          self.risk == CommandRisk::Low
              && !self.requires_approval
              && is_low_risk_read_only(&self.command)
      }
  }
  ```

  At the end of `CommandPolicy::classify`, before returning:

  ```rust
  // After the whole classification, so the block never has to invent a risk
  // (proposal §4) and sees the outside-root approval, the way Plan mode does.
  // After the allowlist too: Allow Always cannot outrank a profile.
  if !classification.blocked
      && !command_access_permits(self.config.command_access, &classification)
  {
      classification.blocked = true;
      classification.reasons.push(format!(
          "Blocked by permission profile: command_access={}",
          self.config.command_access.as_str()
      ));
  }
  ```

  ```rust
  /// Whether `command_access` lets this command run. `Local` judges the text,
  /// not `classification.may_use_network`, which the allowlist branch sets to
  /// `false`. It is a name heuristic, not a sandbox (spec 31 `context.md` §7).
  fn command_access_permits(access: CommandAccess, classification: &CommandClassification) -> bool {
      match access {
          CommandAccess::None => false,
          CommandAccess::ReadOnly => classification.is_read_only_without_approval(),
          CommandAccess::Local => !may_use_network(&classification.command),
          CommandAccess::All => true,
      }
  }
  ```

  In `mode.rs`, replace the inline `read_only_no_approval` expression with
  `classification.is_read_only_without_approval()`, and drop the imports that
  are now unused. Update the `CommandAccess` doc comment in `config.rs`: it is
  now enforced in `CommandPolicy`.

- [x] **Step 4: Run the scoped tests**

  ```bash
  cargo nextest run -p workspace-engine --test permission_profiles --test repository_config_trust --test foundation
  cargo nextest run -p workspace-engine --lib -E 'test(mode::) | test(command_policy::) | test(validation::) | test(chat::)'
  ```

  Expected: all pass, and `repository_config_trust.rs` is unmodified
  (`git diff --stat` shows no change to it).

- [x] **Step 5: Falsify** (revert each one)

  1. `ReadOnly => true`: the table fails on `cargo test` under `read_only`.
  2. `Local => !classification.may_use_network`: the table fails on the
     allowlisted `npm ci` under `local`.
  3. The block also sets `classification.risk = CommandRisk::Blocked`: the
     invariance test fails.
  4. Remove the block: the table and the run-by-id test fail.
  5. Drop `!self.requires_approval` from the shared predicate. The plan
     predicted that the table would fail on `ls ../elsewhere`. **It did not:**
     the path escape also raises that command's risk to Medium, so `risk == Low`
     still refused it, and only `mode.rs`'s
     `plan_refuses_a_command_that_would_require_approval_even_if_low_risk`
     failed. The test
     `read_only_access_blocks_a_low_risk_command_that_needs_approval` was added
     during implementation (`ls` with `require_approval_for_all_commands=true`
     stays Low risk). With it, this mutation fails both tests, which shows that
     the mode and the profile share one predicate.
  6. Push the reason even when already blocked: the invariance test fails on
     `rm -rf /`.

- [x] **Step 6: Scoped checks**

  ```bash
  cargo fmt --all -- --check
  cargo clippy -p workspace-engine --all-targets --locked -- -D warnings
  typos
  cargo check -p desktop-shell -p damaian-cli -p eval-harness --all-targets
  ```

- [x] **Step 7: Update this file's progress row, show the change and the
  check results, and ask before committing**

  Suggested subject: `Block commands the permission profile does not allow`.

## Task 5: `profile ∩ mode` at every refusal point

**Requirements:** 5 and 6. Acceptance criteria 9, 10 and 12. **Files:**
`mode.rs`, `chat.rs`, `edit.rs`, and `tests/permission_profiles.rs`.
**Touches `chat.rs`. Read Global Constraints before starting.**

- In `mode.rs`, add
  `pub(crate) fn permits(mode, &ProfileCapabilities, &ToolAction, Option<&CommandClassification>, Option<bool>) -> Permission`.
  It returns the first refusal of `mode_permits` then `profile_permits`.
  `profile_permits` refuses:
  - edits and patch proposals when `allow_file_edits` is false;
  - web diagnostics when `allow_browser_diagnostics` is false;
  - an MCP call whose read-only hint is not `Some(true)` when
    `allow_mutating_mcp_tools` is false, and any MCP call when `mcp_enabled`
    is false;
  - a `blocked` command. Task 4 makes the classification carry the profile.
- `Permission::Refused` gains which axis refused it. `refusal_message` names
  the mode or the profile.
- Replace every `mode_permits` call site with `permits`:
  - the tool-list filter;
  - `action_permission`;
  - the three resume branches;
  - `refuse_unless_mode_permits_patches`, renamed to say both.

  Capabilities come from the orchestrator's `Config`, which is captured per
  turn and per resume (`context.md` §6).
- Extend `the_permission_matrix_matches_the_spec_table` into a matrix over
  every mode × the four built-in profiles × every tool class. Do not add a
  second matrix test.

Tests:
- "Ask under Full cannot edit" and "Code under Read-only cannot edit", each
  with the expected refusal wording.
- Mid-session switching:
  - A turn paused for command approval under Full, resumed after the
    selection changed to Read-only, refuses at resume.
  - A command already executing is not interrupted. Drive it with a
    long-running command and assert it completes.
- The eval harness's `approval_policy_violations` still counts a
  profile-refused resume. It calls `reject_proposal` like a mode refusal
  does.

Run the deterministic eval tier.

## Task 6: Provenance: a source for every applied value

**Requirements:** 2 and §5.7 (the data), and acceptance criterion 5. **Files:**
`config.rs`, new `effective_policy.rs`, `lib.rs`, `damaian-cli/src/main.rs`,
and `tests/permission_profiles.rs`.

`apply_overlay_scoped` returns an `OverlayOutcome { rejected: Vec<RejectedConfigKey>, applied: Vec<AppliedKey> }`.
Every existing caller keeps its behaviour by reading `.rejected`.
`AppliedKey { key, scope, entries: Option<Vec<String>>, widened: bool }`:

- List keys record the entries each scope contributed.
- `widened` is set when an admin value loosened what the earlier scopes
  resolved, by the same direction rules the restrict-only helpers use.

`load_scoped` keeps the outcomes in order, plus a `Default` source for
untouched keys. `EffectivePolicy::resolve(repository_root) -> Result<EffectivePolicy>`
builds, per key:

- the resolved value;
- its sources, as scope names, "profile: <id>", or "this repository (Allow
  Always)";
- the refused requests from the repository and the profile, by key and
  class, never by value.

The header is "`<profile> ∩ <mode>`" when a mode is given.

CLI: `damaian config-show --sources [repo]` prints it.

Tests:
- The §5.7 example's shape: `.env` from user config and `secrets/**` from
  the repository on the same key.
- An admin widening marked as one.
- A refused repository `require_approval_for_file_edits=false` shown as
  refused, with the string `false` absent from the refusal.
- A resolver-agreement test: every value in `EffectivePolicy` equals the
  `Config` from `load_scoped`.

## Task 7: Attributed effective-policy view and profile picker

**Requirements:** 2 and §5.7 (the view), and acceptance criterion 6. **Files:**
`desktop-shell/src/lib.rs`, `app.js`, `index.html`, and `styles.css`.

- `GET /api/effective-policy?repo=…&session=…` serves `EffectivePolicy` as
  JSON with camelCase keys. It passes the session's mode when a session is
  given.
- `POST /api/permission-profile` writes the selection through the same code
  as `profile-set`, and audits it. It returns the new policy.
- Settings › General's "Effective policy" `<pre>`
  (`renderConfigPolicy`) becomes a table with columns rule, value and source,
  plus a refused-request line under any key that has one. Keep feeding the
  raw text to the provider and model syncing that `renderConfigPolicy` does
  today, and do not break it.
- Add a per-repository profile `<select>` above the table. It describes each
  profile, including the `local` caveat from `context.md` §7 in those words.
  It shows the Offline private model-traffic warning when it applies.
- Follow `docs/UI_STYLE_GUIDE.md`.

Verify in the running app. Follow the repository's desktop-shell UI
verification practice: rebuild, restart on a port other than 4765 with a
separate `DAMAIAN_DATA_DIR`, and drive it from the browser.

## Task 8: Sanitized export and import

**Requirements:** 7, and acceptance criterion 11. **Files:** `profile.rs`,
`desktop-shell/src/lib.rs`, `app.js`, `damaian-cli/src/main.rs`, and
`tests/permission_profiles.rs`.

- `export(id, data_dir) -> String`: the profile's overlay, restricted to the
  keys a profile may carry (Task 3's rule) and serialized field by field
  through `overlay_field_kinds`. Round-trip-tested, because the general
  serializer drops keys (`context.md` §9).
- `import(text, id, data_dir) -> ImportReport { written, not_applicable: Vec<RejectedConfigKey> }`:
  - It parses with `parse_untrusted`.
  - It lists keys a profile cannot carry, and keys that would loosen the
    current resolved config and so have no effect.
  - It then writes `<data_dir>/config/profiles/<id>.conf` and audits
    `permission_profile_imported`, with counts and key names only.
  - It refuses a reserved id or an existing custom id unless asked to
    replace.
- CLI `profile-export` / `profile-import`. Shell endpoints and Settings
  buttons, where the import shows the not-applicable list before writing.

Tests:
- An export of every built-in profile contains no `auth_token_env` and no
  `model_api_key_env` text, including when user config sets both.
- An import carrying `model_base_url`, `shell` and a loosening
  `command_access` writes none of them and lists all three.
- An imported profile, once selected, cannot widen anything. Reuse Task 3's
  widening test with the imported file.

## Task 9: Docs, acceptance criteria, second-person review, close the spec

**Requirements:** §5.10, §6 and §7. **Files:** `docs/USER_GUIDE.md`,
`docs/TROUBLESHOOTING.md`, `SECURITY.md`, `proposal.md` §7, and every status
record.

- Write §5.10's documentation. Include the `local` caveat and the Offline
  private model-traffic caveat in `context.md` §7's words.
- Map every acceptance criterion in proposal §6 to its test, in §7.
- Run the full seven-command gate from `AGENTS.md`, plus
  `npm run specs:check`, and the deterministic eval tier. Confirm the
  baseline's `approval_policy_violations` did not increase (criterion 14).
- **Second-person review (criterion 7).** Ask the user to have someone who did
  not build this read the policy view for a prepared repository and say what
  the session may do. Record what they misread in §7. The spec does not close
  on this criterion until that has happened. If it cannot happen yet, record
  that, and leave the status In progress rather than marking it Done.
- Close out per `AGENTS.md` "When a spec becomes Done":
  - update the `Depends on:` lines of #32, #33, #35 and #52;
  - update the README's "What to build next";
  - remove the `CHANGELOG.md` `Unreleased` entry;
  - fill in proposal §7's two recording questions.

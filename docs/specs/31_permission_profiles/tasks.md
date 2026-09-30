# Permission Profiles Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Implements:** [`proposal.md`](proposal.md) in full · corrections and the
decisions it left open in [`context.md`](context.md)
**Started:** 2026-09-30 — **Done:** —

## Progress

| Task | State | Notes |
|---|---|---|
| 1 · The capability/preference partition, tied to spec 34's classes | Not started | Planned in full on 2026-09-30 |
| 2 · The four profile capability keys | Not started | |
| 3 · Profiles, `ConfigScope::Profile`, and per-repository selection | Not started | |
| 4 · `command_access` enforced in `CommandPolicy` as a block | Not started | |
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
  `repo_<16 hex>`, per checkout.
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

- [ ] **Step 1: Write the failing tests**

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

- [ ] **Step 2: Run the tests and confirm they fail**

  Run: `cargo nextest run -p workspace-engine --test permission_profiles`

  Expected: a compile error, because `ConfigKeyKind` and `overlay_field_kinds`
  are not in `workspace_engine`.

- [ ] **Step 3: Write the implementation**

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

- [ ] **Step 4: Run the tests and confirm they pass**

  Run: `cargo nextest run -p workspace-engine --test permission_profiles`

  Expected: 4 tests pass. If a `Reported` case fails, the refusal key string
  in the case is wrong or spec 34's behaviour changed. Read the branch in
  `apply_overlay_scoped` for that field before changing either. Do not weaken
  the assertion to make it pass.

- [ ] **Step 5: Mutation-test the load-bearing guarantees**

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

- [ ] **Step 6: Confirm spec 34's floor is untouched**

  Run: `cargo nextest run -p workspace-engine --test repository_config_trust`

  Expected: all pass. The file is unmodified (`git diff --stat` shows it
  unchanged).

- [ ] **Step 7: Scoped checks**

  ```bash
  cargo fmt --all -- --check
  cargo clippy -p workspace-engine --all-targets --locked -- -D warnings
  typos
  ```

- [ ] **Step 8: Update this file's Task 1 row, then show the change and the
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

## Task 3: Profiles, `ConfigScope::Profile`, and per-repository selection

**Requirements:** 1, 3, 4 and §5.4–§5.5. Acceptance criteria 1 (no regression
with a hostile fixture present) and 3. **Files:** new `profile.rs`;
`config.rs`, `lib.rs`, `damaian-cli/src/main.rs`, and
`tests/permission_profiles.rs`.

- `pub enum ProfileId { ReadOnly, SafeLocal, Full, OfflinePrivate, Custom(String) }`.
  `parse` validates a custom id as `[a-z0-9_]{1,40}` and rejects the four
  reserved ids.
- `ProfileId::overlay(&self, data_dir) -> Result<ConfigOverlay>`. The built-in
  profiles are the values in `context.md` §3's table. `Full` is
  `ConfigOverlay::default()`. A custom profile is parsed from
  `<data_dir>/config/profiles/<id>.conf` with `parse_untrusted`, so a
  malformed file is reported and not fatal.
- Add `ConfigScope::Profile`. Replace `let trusted = scope != ConfigScope::Repository`
  with an exhaustive `match`. At `Profile` scope:
  - capability keys merge restrict-only, the same way as for Repository;
  - `audit_retention_days` and `checkpoint_retention_days` merge lower-wins;
  - every other key is ignored and reported. That covers preference keys and
    the Forbidden redirecting keys.

  Its refusals go into a separate `profile_rejected_keys` on the report. They
  are not a repository's refusals, and must not reach spec 34's repository
  notice or audit. They are audited on their own, as
  `permission_profile_key_rejected` with the profile id, key and class. Like
  `RepositoryTrustStore::review`, each key is audited once per profile, not
  on every load. That covers criterion 4 ("every refused widening is
  audited") for a custom profile file edited by hand, which never goes
  through import.
- Selection: `permission_profile.<repository_id>=<id>` in user config, parsed
  into `Config.permission_profile_by_repository`. It is `UserOwned` at
  repository scope, and gets a Task 1 case. `load_scoped` applies the selected
  profile after admin and before `apply_repository_allowlist`. With no entry,
  nothing is applied.
- `Config::profile_capabilities(&self) -> ProfileCapabilities`, a small
  `Copy` struct of the four Task 2 values plus `mcp_enabled`. Task 5 consumes
  it.
- CLI: `damaian profile-set <repo> <id>` writes the selection. It audits
  `permission_profile_set` with the repository id and the old and new ids.

Tests: with no selection, the resolved config is `==` to today's for
`RESTRICTIVE_USER` over the spec 34 hostile repository config (copy both
fixtures). Each built-in profile resolves to the §3 values over a permissive
user config. A profile cannot widen a narrow user config: set every
capability key restrictive in user config, select a profile that tries to
loosen all of them, and assert nothing moved. A repository cannot select a
profile. Admin can widen the base but not undo a profile the user selected.
Mutation: make `Profile` scope trusted and confirm the widening test fails.

## Task 4: `command_access` enforced in `CommandPolicy` as a block

**Requirements:** 3 and 5 for commands, on every execution path (`context.md`
§7, observation 10). **Files:** `command_policy.rs`, and
`tests/permission_profiles.rs`.

In `CommandPolicy::classify`, after the hard block and blocklist checks and
before the allowlist, block a command the profile does not permit:

- `None`: every command.
- `ReadOnly`: anything that is not Low risk, needs approval, or fails
  `is_low_risk_read_only`. Reuse the exact predicate Plan mode uses (read it
  from `mode.rs`, do not copy it).
- `Local`: anything `may_use_network` flags.
- `All`: nothing.

The block sets `blocked: true` with a reason naming the profile key
("blocked by permission profile: command_access=local"). It never changes
`risk` or `requires_approval`. An allowlisted command is still blocked:
Allow Always cannot outrank a profile.

Tests:
- The four levels against `cargo test`, `ls`, `git diff`, `curl example.com`,
  `npm ci`, and an allowlisted `npm ci`.
- A stored proposal created under `All` is refused by `run_proposal` after
  the config narrows to `None`. That is the run-by-id path.
- The risk and `requires_approval` of every sample command are identical
  before and after this change. That test pins proposal §4.

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

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
| 5 · `profile ∩ mode` at every refusal point | Done 2026-10-04 | Expanded into full steps on 2026-10-04. Every call site was re-located: `context.md` §5's line numbers still held, except that `edit.rs` calls the patch gate twice at apply (`:573`, `:574`). It was planned on the old base while spec 22 Task 8 was being written in its own worktree, and implemented after that task merged to main (`00173ee`). **This branch is not rebased onto it.** Task 8 also edits `chat.rs` next to the web-diagnostic and command resume branches and the imports, so the merge back will need those few hunks resolved. **Landed:** `mode.rs` gains `ProfileLimit`, `Permission::RefusedByProfile { limit }`, `profile_permits` and `permits` (mode first, first refusal wins), and the profile case of `refusal_message`: `Refused: the permission profile does not allow this (<key>=<value>). Switching mode will not allow it.` `command_access_permits` is now `pub(crate)`. Every `mode_permits` call in `chat.rs` now calls `permits`: the tool list, where the closure is renamed `offered`; `action_permission`; and the three resume branches. The command resume re-classifies the stored command. A profile refusal is rejected as `profile_policy`, and a profile-refused web diagnostic is audited as `refused_by_profile`. `edit.rs`'s gate is now `refuse_unless_mode_and_profile_permit_patches`, and it asks the profile even with no session. `the_permission_matrix_matches_the_spec_table` is now one table: 17 tool-class rows × 4 modes × the 4 built-in profiles, from their real overlays. The Full column must equal `mode_permits` exactly. **Tests:** new in `mode.rs`: the matrix (rewritten) and 2 message tests, 18/18. Two in `chat.rs`'s `mode_refusal_tests`: the web and MCP resume points. Ten in `permission_profiles.rs` (36 pass, 1 ignored): the two criterion 10 cases with exact wording, the profile-blocked command refused before any proposal, the next turn, the resume refusal, the violation pairing, sessionless propose and apply. The in-flight test is `#[ignore]` because it uses the real login shell. It passes when run by hand. `repository_config_trust` 47/47, file unmodified. The three integration files total 241. The whole `workspace-engine` crate: 868 passed, 19 skipped. The deterministic eval tier: 16/16 scenarios pass and `approval_policy_violations` is 0. The other metrics were not compared with a run on the base commit. `cargo fmt --check`, `cargo clippy -p workspace-engine --all-targets --locked -D warnings` and `typos` are clean. `cargo check -p desktop-shell -p damaian-cli -p eval-harness --all-targets` passes. **Mutations (all reverted, all caught):** (1) `permits` = mode alone failed 8. The edit tests did not fail, which the plan had predicted they would: the sessionless path calls `profile_permits` directly. (2) Profile first failed the matrix. (3) `blocked` instead of `command_access_permits` failed the matrix, `code_under_read_only…` and `the_next_turn…`. (4) The tool list with mode alone failed `code_under_read_only…`. (5) The command resume with Full's capabilities failed the resume and violation tests. Spec 20's `a_mode_refused_proposal_that_is_later_run_by_id…` also failed once in that parallel run, but passes under the same mutation in isolation and 5/5 unmutated. That one failure is unexplained, probably its real login shell under load. (6) The web and MCP resumes with Full's capabilities each failed their `chat.rs` test. (7) The early return on an empty session failed only the sessionless propose test. Apply goes through `propose_edit`'s own session. (8) Mode wording for the profile case failed 4. (9) The in-flight test, falsified by switching before the resume engine is built, failed on "never started". As first planned, with the switch before the thread, it was not caught, because the test reused the turn's engine. The test now builds its own resume engine. **Deviations:** ten, listed under Task 5. (1) A new `RefusedByProfile` variant. (2) The refusal names `key=value`, not the profile. (3) The mode is named when both refuse. (4) `command_access_permits` is shared, and `blocked` is not read, so a blocklisted command keeps its card. (5) The resume re-classifies the stored command for both axes. (6) Sessionless patches are profile-checked. (7) Read-only still offers `propose_plan` and `complete_step`. (8) The web and MCP resume tests are in `chat.rs`. (9) The shell is `/usr/bin/true`: this Mac stalls exec of any freshly written executable, so a script cannot stand in. (10) Audit wording. **For Task 6/7:** a refusal shows the resolved `key=value` and never the scope that set it. The attributed view must show all five keys, including `mcp_enabled`. If the view needs the strings, make `ProfileLimit::setting` `pub(crate)` rather than copying it. **For Task 9:** the user guide should give the two refusal strings, deviation 3 (Ask under Read-only names the mode first) and deviation 7 |
| 6 · Provenance: a source for every applied value | Done 2026-10-04 | Expanded into full steps on 2026-10-04, after checking every `apply_overlay_scoped` caller, each merge helper and `apply_repository_allowlist` against the code. **Landed:** `apply_overlay_scoped` returns `OverlayOutcome { rejected, applied: Vec<AppliedKey> }`. `AppliedKey { key, scope, entries, widened }` is recorded at every application. The seven list keys carry the entries the scope holds. MCP servers and model providers are recorded per field. The helpers report what they applied: `union_patterns`, `intersect_allowlist` → `Option<(entries, loosened)>`; the flag, access, limit and ceiling helpers → `Option<loosened>`; `lower_wins` → `bool`; `upsert_mcp_server_from_repository` → its fields. `widened` is set only at admin scope. `RepositoryConfigReport` gains `applied` and `allow_always_entries`, and `apply_repository_allowlist` returns what it added. New `effective_policy.rs`: `EffectivePolicy::{resolve, from_load, rule, to_text}`, with `PolicyRule`, `PolicyEntry`, `PolicySource`, `SourceKind`, `RefusedRequest` and `RefusedBy`, all re-exported. Its rules are the lines of `to_policy_text`, so the view and the text cannot disagree. Added `ProfileId::label`. CLI `config-show --sources [repo]`. Callers: `damaian-cli` and one `foundation.rs` test read `.rejected`. The rest ignore the result unchanged. The refusal order is unchanged. Not touched: `chat.rs`, the web UI, `repository_config_trust.rs`. **Tests:** 5 new in `permission_profiles`: the outcome API, §5.7's per-entry example with Allow Always and a profile, admin widening against narrowing and user loosening, a refusal by key and class with no value in the JSON, the text or `report.applied`, and resolver agreement against an independent `load_scoped`. The file passes 41, with 1 ignored. The three integration files total 246, all passing. `repository_config_trust` is 47/47 and the file is unmodified. `foundation`'s FSEvents watcher test timed out once under parallel load and passed on rerun. The `mode::`, `config::` and `profile::` lib tests pass, 18/18. `cargo fmt --check`, `cargo clippy -p workspace-engine -p damaian-cli --all-targets --locked -D warnings` and `typos` are clean. `cargo check -p desktop-shell -p damaian-cli -p eval-harness --all-targets` passes. A manual `config-show --sources` against a scratch data dir showed the per-entry sources, the profile attribution, the admin widening and the refusals. The refused `shell` value was absent. **Mutations (8, all reverted, all caught in the end):** (1) the untrusted union recording nothing failed 2 tests. (2) `widened` at every trusted scope was **not caught at first**, because `from_load` re-checked the scope. It now reads `widened` alone, and the mutation fails. (3) The flag ignoring the current value failed the admin test. (4) Values rendered from defaults failed agreement. (5) A narrowed allowlist recording `incoming` was **not caught at first**, because the view prints entries from `Config`. The test now scans `report.applied`, and the mutation fails. (6) Dropping the Allow Always record failed §5.7; the first version of this mutation did not compile and was redone. (7) First applier wins failed 3. (8) The limit never loosening failed the admin test. A restore with `shutil.move` kept the backup's older mtime, so cargo reused a mutated build once. Touch the sources after restoring. **Deviations:** ten, listed under Task 6. (1) Provenance travels on the report, and `load_scoped`'s signature is unchanged. (2) Allow Always is a report field, not a scope. (3) A source is the last scope whose value holds, so an untrusted scope that sets the restrictive value takes the attribution. (4) Forbidden and User-owned keys have no direction, so an admin value there is attributed but never marked widened (open for Task 9's review). (5) Limit and ceiling widen on a strict `>`. (6) `resolve(Option<&Path>, Option<SessionMode>)`, plus `from_load`. (7) Keys the text omits while unset have no rule, and their refusals go to `otherRefused`. (8) Repository allowlist entries awaiting migration are not in the view. (9) Attribution per field for MCP and providers. (10) `ProfileId::label`. **For Task 7:** serve `EffectivePolicy::from_load` (or `resolve`) as JSON. The exact shape is under Task 6, "The `EffectivePolicy` JSON shape". It is camelCase: `header`, `profile`, `profileLabel`, `profileSelected`, `mode`, and `rules[] { key, value, sources[] { kind, label }, entries[] | null, adminWidened, refused[] { key, class, by } }`, plus `otherRefused[]`. `kind` is one of `default`, `user`, `repository`, `admin`, `profile` or `allowAlways`, and `by` is `repository` or `profile`. The view describes the loaded config. The chat's per-request provider override (`config_for_repo_with_provider`) is not reflected. Pending allowlist entries come from the existing migration notice, not from this structure |
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
  Task 5, check spec 22's `tasks.md` progress table. If its Task 8 is in
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
**Also modified:** `command_policy.rs`, one visibility change (deviation 4).
**Not touched:** `config.rs`, `profile.rs`, `validation.rs`,
`repository_config_trust.rs`.
**Touches `chat.rs`. Read Global Constraints before starting.**

`mode.rs` gains `profile_permits`, which crosses the resolved
`ProfileCapabilities` with the tool class, and `permits`, which asks the mode
first and then the profile and returns the first refusal. Every place that
called `mode_permits` now calls one of these. `chat.rs` and `edit.rs` only call
them. They read the capabilities from the orchestrator's own `Config`, which a
desktop request loads once. So a profile switch takes effect at the next turn
start, the next resume and the next apply, and never in the middle of an action
already running (`context.md` §6).

`profile_permits` refuses:

| Tool class | Refused when | Names |
|---|---|---|
| `ProposePatch`, `EditFile` | `allow_file_edits=false` | `allow_file_edits=false` |
| `Command` | Task 4's `command_access_permits` says no | `command_access=<level>` |
| `WebDiagnostic` | `allow_browser_diagnostics=false` | `allow_browser_diagnostics=false` |
| `McpCall` | `mcp_enabled=false`, or `allow_mutating_mcp_tools=false` and the hint is not `Some(true)` | that key |
| Reads, `ProposePlan`, `CompleteStep` | never | |

The refusal names the axis. A mode refusal keeps its wording. A profile refusal
reads:
`Refused: the permission profile does not allow this (<key>=<value>). Switching mode will not allow it.`

**Call sites, re-located against the code on 2026-10-04.** `context.md` §5's
line numbers, taken on 2026-09-30, still match apart from edit.rs:

- **The tool-list filter:** `chat.rs:1389-1521`. The mode is captured at
  `:1395`. `mode_permits` is called at `:1401` (the `permits` closure that every
  built-in definition uses), `:1420` (`run_command`) and `:1508` (MCP).
- **`action_permission`:** `chat.rs:3015`, which calls `mode_permits` at
  `:3037`. The main loop calls it once per action at `:2046` and renders a
  refusal at `:2174`.
- **The three resume branches** in `resume_after_command_decision_with_options`
  (`chat.rs:800`). The mode is read at `:828`. Web diagnostic at `:837`, with
  the decision at `:843` and the message at `:872`. MCP at `:890`, with the
  message at `:901`. Command at `:934`, with the rejection at `:953` and the
  message at `:954`.
- **`refuse_unless_mode_permits_patches`:** `edit.rs:298`. It is called at
  `:350` (`propose_edit`) and **twice** at apply, at `:573` (`session_id`) and
  `:574` (`origin_session_id`). `context.md` §5 lists only `:573`. The second
  call is spec 20's origin-session check, added later.

No other `mode_permits` caller exists outside `mode.rs`'s own tests. The
single apply path is `apply_stored_patch` (`edit.rs:554`), and nothing else
calls `PatchEngine::apply_patch`.

**Deviations from the outline:**

1. **`Permission` gains a variant, `RefusedByProfile { limit }`, rather than a
   field on `Refused`.** The variant is the axis. `Refused { blocked_by,
   allowed_in }` stays the mode refusal, so spec 20's matches and tests are
   unchanged. `is_allowed` and `refusal_message` handle both.
2. **A profile refusal names the setting, not the profile.**
   `ProfileCapabilities` holds resolved values, and user, repository or admin
   config can also narrow each of these keys (Task 2). "Under the Read-only
   profile" would then be false. So the message says `allow_file_edits=false`,
   which is true whatever set it. Task 7's attributed view says which scope
   set it.
3. **When both axes refuse, the mode is named.** The outline says "first
   refusal of `mode_permits` then `profile_permits`". So Ask under Read-only
   says "Switch to Code mode", and the next attempt in Code is then refused
   by the profile. Each message is true about its own axis. Task 7's view
   shows both.
4. **`command_access_permits` becomes `pub(crate)`** in `command_policy.rs`,
   a one-line change. The outline says `profile_permits` refuses "a `blocked`
   command". But `blocked` also means the blocklist and the hard block, which
   are local policy, not the profile. Reading it would label a blocklisted
   command a profile refusal. It would also refuse that command before the
   blocked-proposal card it gets today, which changes behaviour under Full.
   Task 4's predicate does not read `blocked`, so a stale stored flag cannot
   decide either.
5. **The command resume re-classifies the stored command**
   (`ValidationOrchestrator::classify_command` on its working directory) and
   asks both axes about that classification. This follows Task 4's note.
   Before this task, the mode was asked about the fields stored with the
   proposal. It now sees the current config too. The command text and
   directory are the same, so only a config change between proposal and
   approval can change the answer, and the current config is the one
   `run_proposal` would enforce anyway.
6. **The patch gate asks the profile even when the session id is empty.**
   That case is the CLI's sessionless `propose-edit` and a legacy patch with
   no session. A profile is not a property of a session. Without this, the
   CLI could propose and apply edits under Read-only. The gate is renamed
   `refuse_unless_mode_and_profile_permit_patches`. With no session it calls
   `profile_permits` alone.
7. **Read-only does not withhold `propose_plan` or `complete_step`.** No
   capability key covers planning, and planning writes nothing. So Code under
   Read-only offers Ask's tools plus the two planning tools. That differs from
   `context.md` §3's "matches Ask mode's tool set", which holds for every
   mutating class.
8. **The web-diagnostic and MCP resume tests are in `chat.rs`'s
   `mode_refusal_tests`**, beside their spec 20 siblings, because they reuse
   that module's counting web runner and fake MCP server. The narrowed engine
   is a clone of the config with the key turned off. The command and edit
   tests are in `permission_profiles.rs`. They go through the real selection:
   `select_profile` writes user config, and a new engine loads it.
9. **The integration tests use `/usr/bin/true` as the shell.** The engine
   calls `<shell> -lc <command>`. `true` ignores its arguments and exits 0,
   so a command is recorded as executed without a real login shell, which
   would force `#[ignore]` (`AGENTS.md`). The plan first used a stand-in
   script written by the test. On this Mac, any freshly written executable
   stalls at exec, even a copy of `/bin/echo`, inside the sandbox and outside
   it. So a script written at test time cannot stand in. The one test that
   needs a command which really runs for a while, the in-flight test, uses
   the real `$SHELL`. It is `#[ignore]`d, with its manual command in its doc
   comment.
10. **Audit wording.** A profile-refused command is rejected with
    `rejectedBy: profile_policy`, where a mode refusal uses `mode_policy`. A
    profile-refused web diagnostic is audited with the decision
    `refused_by_profile`. The eval harness pairs `stored_command_rejected` with
    `stored_command_executed` by proposal id and never reads `rejectedBy`
    (`runner.rs:425-445`). So a profile refusal counts in
    `approval_policy_violations` as a mode refusal does.

**Interfaces:**
- Consumes `Config::profile_capabilities()` and `ProfileCapabilities` (Task 3),
  `CommandAccess::as_str` (Task 2), and `command_access_permits` (Task 4,
  now `pub(crate)`).
- Produces, in `mode.rs`:
  - `pub(crate) enum ProfileLimit { FileEdits, CommandAccess(CommandAccess), BrowserDiagnostics, MutatingMcpTools, McpDisabled }`,
    with a private `setting(self) -> String` that returns `key=value`.
  - `Permission::RefusedByProfile { limit: ProfileLimit }`.
  - `pub(crate) fn profile_permits(&ProfileCapabilities, &ToolAction, Option<&CommandClassification>, Option<bool>) -> Permission`.
    It panics on a `Command` with no classification, as `mode_permits` does.
  - `pub(crate) fn permits(SessionMode, &ProfileCapabilities, &ToolAction, Option<&CommandClassification>, Option<bool>) -> Permission`.
  - `refusal_message` with the profile case.
- **For Task 6–7:** a refusal does not say which scope set the refusing value,
  only `key=value` (deviation 2). The attributed view is where a user finds
  that out, so it should show these five keys, `mcp_enabled` included.
  `ProfileLimit::setting` is private. If the view wants the same strings, make
  it `pub(crate)` rather than writing a second copy.
- **For Task 9:** the user guide's refusal examples are the two strings
  pinned in Step 1. Deviation 7 needs saying there, and so does deviation 3:
  one action can be refused by the mode first and by the profile next.

- [x] **Step 1: Write the failing tests**

  In `mode.rs`'s test module, import `permits`, `ProfileLimit`, `Config`,
  `ConfigScope`, `CommandAccess` and `ProfileId`. Replace the body of
  `the_permission_matrix_matches_the_spec_table` with one table. Each tool
  class has a mode row and a profile row, and `permits` must allow an action
  exactly where both rows allow it. The old assertions are all rows in it,
  and the Full column must equal `mode_permits` exactly:

  ```rust
  /// The work package's primary artifact per `proposal.md` §6, extended by
  /// spec 31 Task 5: every mode × the four built-in profiles × every tool
  /// class. Each class has a mode row (spec 20 `proposal.md` §5.1 plus
  /// `context.md` §1's extension) and a profile row (spec 31 `context.md`
  /// §3). `permits` allows exactly where both rows allow, and when the mode
  /// refuses, the mode is the one named.
  #[test]
  fn the_permission_matrix_matches_the_spec_table() {
      use SessionMode::*;
      const T: bool = true;
      const F: bool = false;
      let modes = [Ask, Plan, Code, Review];
      let profiles = [
          ProfileId::ReadOnly,
          ProfileId::SafeLocal,
          ProfileId::Full,
          ProfileId::OfflinePrivate,
      ];
      // The real overlays at the real scope, not a hand-copied table.
      let capabilities_of = |profile: &ProfileId| {
          let mut config = Config::default();
          let (overlay, refused) = profile.overlay(&config.data_dir).unwrap();
          assert!(refused.is_empty());
          config.apply_overlay_scoped(overlay, ConfigScope::Profile);
          config.profile_capabilities()
      };
      let command = |text: &str| {
          ToolAction::Command(CommandRequest {
              command: text.into(),
              reason: String::new(),
          })
      };
      let mcp = || ToolAction::McpCall {
          server_id: "sentry".into(),
          tool_name: "search_issues".into(),
          arguments_json: "{}".into(),
      };
      let git_status = read_only_command();
      let touch = CommandClassification {
          command: "touch x".to_string(),
          risk: CommandRisk::Medium,
          requires_approval: true,
          ..read_only_command()
      };
      let curl = CommandClassification {
          command: "curl example.com".to_string(),
          risk: CommandRisk::High,
          requires_approval: true,
          may_use_network: true,
          ..read_only_command()
      };

      // (action, classification, MCP hint,
      //  allowed in [Ask, Plan, Code, Review],
      //  allowed under [read_only, safe_local, full, offline_private])
      #[allow(clippy::type_complexity)]
      let rows: Vec<(ToolAction, Option<&CommandClassification>, Option<bool>, [bool; 4], [bool; 4])> = vec![
          (ToolAction::ReadFile { path: "x".into(), range: None }, None, None, [T, T, T, T], [T, T, T, T]),
          (ToolAction::ListDirectory { dir: None, depth: None }, None, None, [T, T, T, T], [T, T, T, T]),
          (
              ToolAction::SearchContent { pattern: "x".into(), path_glob: None, max_matches: None },
              None, None, [T, T, T, T], [T, T, T, T],
          ),
          (
              ToolAction::SearchCodebase { query: "x".into(), semantic: false, limit: 8 },
              None, None, [T, T, T, T], [T, T, T, T],
          ),
          (ToolAction::ReadGitStatus, None, None, [T, T, T, T], [T, T, T, T]),
          (ToolAction::ReadGitDiff { staged: false }, None, None, [T, T, T, T], [T, T, T, T]),
          (
              ToolAction::ProposePatch(GeneratedEdit { summary: "x".into(), changes: vec![] }),
              None, None, [F, F, T, F], [F, T, T, T],
          ),
          (
              ToolAction::EditFile { summary: "x".into(), edits: vec![] },
              None, None, [F, F, T, F], [F, T, T, T],
          ),
          // Deviation 7: no capability key covers planning.
          (ToolAction::ProposePlan(vec![]), None, None, [F, T, T, F], [T, T, T, T]),
          (ToolAction::CompleteStep, None, None, [F, T, T, F], [T, T, T, T]),
          (command("git status"), Some(&git_status), None, [F, T, T, T], [F, T, T, T]),
          (command("touch x"), Some(&touch), None, [F, F, T, F], [F, T, T, T]),
          (command("curl example.com"), Some(&curl), None, [F, F, T, F], [F, F, T, F]),
          (
              ToolAction::WebDiagnostic(WebDiagnosticCall {
                  kind: WebDiagnosticKind::Inspect,
                  url: "http://localhost".into(),
                  arguments_json: "{}".into(),
                  session_id: None,
                  task_id: None,
              }),
              None, None, [F, F, T, T], [F, T, T, F],
          ),
          // Offline private turns MCP off altogether.
          (mcp(), None, Some(true), [T, T, T, T], [T, T, T, F]),
          (mcp(), None, Some(false), [F, F, T, F], [F, T, T, F]),
          (mcp(), None, None, [F, F, T, F], [F, T, T, F]),
      ];

      for (action, classification, hint, mode_row, profile_row) in &rows {
          for (mode, mode_allows) in modes.iter().zip(mode_row) {
              let mode_only = mode_permits(*mode, action, *classification, *hint);
              assert_eq!(mode_only.is_allowed(), *mode_allows, "{mode:?} on {action:?} {hint:?}");
              for (profile, profile_allows) in profiles.iter().zip(profile_row) {
                  let permission =
                      permits(*mode, &capabilities_of(profile), action, *classification, *hint);
                  let context = format!("{mode:?} under {} on {action:?} {hint:?}", profile.as_str());
                  match (mode_allows, profile_allows) {
                      (true, true) => assert_eq!(permission, Permission::Allowed, "{context}"),
                      (true, false) => assert!(
                          matches!(permission, Permission::RefusedByProfile { .. }),
                          "{context}: {permission:?}"
                      ),
                      // The mode is asked first, so it is the axis named.
                      (false, _) => assert_eq!(permission, mode_only, "{context}"),
                  }
                  if *profile == ProfileId::Full {
                      assert_eq!(permission, mode_only, "Full changes nothing: {context}");
                  }
              }
          }
      }
  }
  ```

  Add beside it:

  ```rust
  #[test]
  fn a_profile_refusal_names_the_setting_and_says_mode_will_not_help() {
      let message = refusal_message(Permission::RefusedByProfile {
          limit: ProfileLimit::FileEdits,
      });
      assert_eq!(
          message,
          "Refused: the permission profile does not allow this (allow_file_edits=false). \
           Switching mode will not allow it."
      );
      let message = refusal_message(Permission::RefusedByProfile {
          limit: ProfileLimit::CommandAccess(CommandAccess::Local),
      });
      assert!(message.contains("(command_access=local)"), "{message}");
      assert!(!message.contains(" mode does not"), "{message}");
  }

  /// Each limit names the key that refused, so a refusal can be traced to
  /// one setting.
  #[test]
  fn each_profile_limit_names_its_own_key() {
      let names = [
          (ProfileLimit::FileEdits, "allow_file_edits=false"),
          (ProfileLimit::CommandAccess(CommandAccess::None), "command_access=none"),
          (ProfileLimit::BrowserDiagnostics, "allow_browser_diagnostics=false"),
          (ProfileLimit::MutatingMcpTools, "allow_mutating_mcp_tools=false"),
          (ProfileLimit::McpDisabled, "mcp_enabled=false"),
      ];
      for (limit, setting) in names {
          assert!(
              refusal_message(Permission::RefusedByProfile { limit }).contains(&format!("({setting})")),
              "{limit:?}"
          );
      }
  }
  ```

  Append to `tests/permission_profiles.rs`, and add `ChatTurnResult`,
  `MockModelAdapter`, `ModelProviderConfig`, `SessionMode`, `ToolCall`,
  `TurnProgress` and `TurnSink` to its import:

  ```rust
  // Task 5: `profile ∩ mode` at every refusal point. Selecting a profile writes
  // user config. Like the desktop shell, every turn start, resume and apply
  // loads config and builds a new engine, and that is where a switch takes
  // effect (`context.md` §6).

  const PROFILE_REFUSED_EDIT: &str = "Refused: the permission profile does not allow this \
       (allow_file_edits=false). Switching mode will not allow it.";

  fn native_tools_provider() -> ModelProviderConfig {
      ModelProviderConfig {
          id: "openai".to_string(),
          label: "OpenAI".to_string(),
          base_url: String::new(),
          api_key_env: String::new(),
          models: Vec::new(),
          supports_native_tools: true,
          max_output_tokens: None,
          context_token_budget: None,
          provider_reports_usage: true,
          price_per_million_input_tokens: None,
          price_per_million_output_tokens: None,
          price_per_million_cached_input_tokens: None,
          supports_explicit_cache_breakpoints: false,
      }
  }

  impl ProfileFixture {
      /// The engine one desktop request builds: config loaded from disk now,
      /// so it runs under whatever profile is selected now. The shell is
      /// `/usr/bin/true`: called as `<shell> -lc <command>`, it runs nothing and
      /// exits 0, so a command "executes" without a real login shell
      /// (`AGENTS.md`). A script written by the test cannot stand in: macOS
      /// stalls exec of a freshly written executable on this machine.
      fn engine(&self) -> WorkspaceEngine {
          self.engine_with_shell("/usr/bin/true")
      }

      fn engine_with_shell(&self, shell: &str) -> WorkspaceEngine {
          let (mut config, _) = self.load();
          config.enable_index_watcher = false;
          config.model_providers.push(native_tools_provider());
          config.shell = shell.to_string();
          WorkspaceEngine::new(config)
      }

      fn switch_to(&self, profile: &str) {
          select_profile(
              &self.base(),
              &self.root,
              ProfileId::parse(profile).unwrap(),
              &self.audit_log(),
          )
          .unwrap();
      }

      /// The proposal ids of every audit event of `event_type`, in log order.
      fn audited_proposal_ids(&self, event_type: &str) -> Vec<String> {
          self.audit_events()
              .lines()
              .filter_map(|line| serde_json::from_str::<serde_json::Value>(line).ok())
              .filter(|event| event["eventType"] == event_type)
              .filter_map(|event| event["proposalId"].as_str().map(str::to_string))
              .collect()
      }
  }

  /// A warm-up turn creates the session, then its mode is set.
  fn chat_session(engine: &WorkspaceEngine, root: &Path, mode: SessionMode) -> String {
      let mut warm = MockModelAdapter::new("Ready.");
      let mut on_token = |_token: &str| {};
      let first = engine
          .chat_orchestrator
          .ask(root, "warm up", &[], &mut warm, &mut on_token)
          .unwrap();
      engine
          .session_store
          .set_session_mode(&first.session.id, mode, "user")
          .unwrap();
      first.session.id
  }

  fn with_sink<T>(run: impl FnOnce(&mut TurnSink<'_>) -> T) -> T {
      let mut on_token = |_token: &str| {};
      let mut on_progress = |_event: TurnProgress| {};
      let cancel = CancelToken::new();
      let mut sink = TurnSink {
          on_token: &mut on_token,
          on_progress: &mut on_progress,
          cancel: &cancel,
      };
      run(&mut sink)
  }

  fn chat_turn(
      engine: &WorkspaceEngine,
      root: &Path,
      session_id: &str,
      adapter: &mut MockModelAdapter,
  ) -> ChatTurnResult {
      with_sink(|sink| {
          engine
              .chat_orchestrator
              .ask_with_session(root, "Go ahead.", &[], Some(session_id), adapter, sink)
              .unwrap()
      })
  }

  fn approve(engine: &WorkspaceEngine, proposal_id: &str) -> ChatTurnResult {
      let mut after = MockModelAdapter::new("Understood.");
      with_sink(|sink| {
          engine
              .chat_orchestrator
              .resume_after_command_decision(proposal_id, true, "tester", &mut after, sink)
              .unwrap()
      })
  }

  /// The model makes `calls` in its first round, then answers in plain text.
  fn calls_then_answer(calls: &[(&str, &str)]) -> MockModelAdapter {
      let calls = calls
          .iter()
          .enumerate()
          .map(|(index, (name, arguments_json))| ToolCall {
              id: format!("call_{}", index + 1),
              name: name.to_string(),
              arguments_json: arguments_json.to_string(),
          })
          .collect();
      MockModelAdapter::new_sequence_with_tool_calls(
          vec![String::new(), "Understood.".to_string()],
          vec![calls, Vec::new()],
      )
  }

  fn offered_tools(adapter: &MockModelAdapter) -> Vec<String> {
      adapter.requests[0]
          .tools
          .as_ref()
          .expect("native tools should be offered")
          .iter()
          .map(|tool| tool.name.clone())
          .collect()
  }

  fn tool_results(engine: &WorkspaceEngine, session_id: &str) -> Vec<String> {
      engine
          .session_store
          .read_messages(session_id)
          .unwrap()
          .into_iter()
          .filter(|message| message.role == "tool")
          .map(|message| message.content)
          .collect()
  }

  const PROPOSE_NEW_FILE: &str =
      r#"{"summary":"Add a file","files":[{"path":"new.txt","content":"hello\n"}]}"#;

  /// Criterion 10, first half: Code mode under Read-only cannot edit, and the
  /// refusal names the profile's setting, not a mode.
  #[test]
  fn code_under_read_only_cannot_edit_and_the_refusal_names_the_profile() {
      let fixture = profile_fixture("code-read-only");
      fixture.switch_to("read_only");
      let engine = fixture.engine();
      let session = chat_session(&engine, &fixture.root, SessionMode::Code);

      let mut adapter = calls_then_answer(&[("propose_patch", PROPOSE_NEW_FILE)]);
      let result = chat_turn(&engine, &fixture.root, &session, &mut adapter);

      let offered = offered_tools(&adapter);
      for withheld in ["propose_patch", "edit_file", "run_command"] {
          assert!(!offered.iter().any(|tool| tool == withheld), "offered {withheld}: {offered:?}");
      }
      // Deviation 7: planning is not a capability.
      for kept in ["read_file", "propose_plan"] {
          assert!(offered.iter().any(|tool| tool == kept), "withheld {kept}: {offered:?}");
      }
      assert!(result.patch_proposal.is_none());
      assert!(!fixture.root.join("new.txt").exists());
      assert_eq!(tool_results(&engine, &session), vec![PROFILE_REFUSED_EDIT]);

      fixture.cleanup();
  }

  /// Criterion 10, second half: Ask mode under Full cannot edit, and the
  /// refusal names the mode.
  #[test]
  fn ask_under_full_cannot_edit_and_the_refusal_names_the_mode() {
      let fixture = profile_fixture("ask-full");
      fixture.switch_to("full");
      let engine = fixture.engine();
      let session = chat_session(&engine, &fixture.root, SessionMode::Ask);

      let mut adapter = calls_then_answer(&[("propose_patch", PROPOSE_NEW_FILE)]);
      let result = chat_turn(&engine, &fixture.root, &session, &mut adapter);

      assert!(result.patch_proposal.is_none());
      assert_eq!(
          tool_results(&engine, &session),
          vec!["Refused: Ask mode does not allow this. Switch to Code mode to allow it."]
      );

      fixture.cleanup();
  }

  /// Task 4's note: a command the profile blocks used to be stored as a
  /// blocked proposal and pause the turn for a decision nobody could make.
  /// It is now refused before anything is stored.
  #[test]
  fn a_command_the_profile_blocks_is_refused_before_any_proposal_or_card() {
      let fixture = profile_fixture("safe-local-curl");
      fixture.switch_to("safe_local");
      let engine = fixture.engine();
      let session = chat_session(&engine, &fixture.root, SessionMode::Code);

      let mut adapter = calls_then_answer(&[(
          "run_command",
          r#"{"command":"curl example.com","reason":"Fetch"}"#,
      )]);
      let result = chat_turn(&engine, &fixture.root, &session, &mut adapter);

      assert!(result.command_proposal.is_none(), "no approval card");
      assert!(!fixture.audit_events().contains("command_proposal_stored"));
      let results = tool_results(&engine, &session);
      assert!(results[0].contains("(command_access=local)"), "{results:?}");

      fixture.cleanup();
  }

  /// "Blocks the next one", at a turn start: what the last turn ran is
  /// refused by the next turn once the selection has changed.
  #[test]
  fn the_next_turn_after_switching_to_read_only_refuses_what_the_last_turn_ran() {
      let fixture = profile_fixture("next-turn");
      let engine = fixture.engine();
      let session = chat_session(&engine, &fixture.root, SessionMode::Code);
      let ls = [("run_command", r#"{"command":"ls","reason":"Look"}"#)];

      let mut before = calls_then_answer(&ls);
      chat_turn(&engine, &fixture.root, &session, &mut before);
      assert_eq!(fixture.audited_proposal_ids("stored_command_executed").len(), 1);

      fixture.switch_to("read_only");
      let engine = fixture.engine();
      let mut after = calls_then_answer(&ls);
      chat_turn(&engine, &fixture.root, &session, &mut after);

      assert!(!offered_tools(&after).iter().any(|tool| tool == "run_command"));
      assert_eq!(fixture.audited_proposal_ids("stored_command_executed").len(), 1);
      let results = tool_results(&engine, &session);
      assert!(results.last().unwrap().contains("(command_access=none)"), "{results:?}");

      fixture.cleanup();
  }

  /// Proposed under Full, approved after the selection changed to Read-only.
  fn command_refused_at_resume(fixture: &ProfileFixture) -> (String, String) {
      let engine = fixture.engine();
      let session = chat_session(&engine, &fixture.root, SessionMode::Code);
      let mut adapter = calls_then_answer(&[(
          "run_command",
          r#"{"command":"touch resumed-marker","reason":"Change"}"#,
      )]);
      let first = chat_turn(&engine, &fixture.root, &session, &mut adapter);
      let proposal = first.command_proposal.expect("Full asks for approval");

      fixture.switch_to("read_only");
      approve(&fixture.engine(), &proposal.id);
      (session, proposal.id)
  }

  /// "Blocks the next one", at a resume: approving is a new decision, made
  /// under the profile selected now.
  #[test]
  fn a_command_paused_under_full_is_refused_at_resume_after_switching_to_read_only() {
      let fixture = profile_fixture("resume-read-only");
      let (session, proposal_id) = command_refused_at_resume(&fixture);

      assert!(!fixture.root.join("resumed-marker").exists(), "the command ran");
      assert!(fixture.audited_proposal_ids("stored_command_executed").is_empty());
      assert_eq!(fixture.audited_proposal_ids("stored_command_rejected"), vec![proposal_id]);
      let results = tool_results(&fixture.engine(), &session);
      assert!(results.last().unwrap().contains("(command_access=none)"), "{results:?}");

      fixture.cleanup();
  }

  /// Criterion 12 and spec 20 `context.md` §9: a profile refusal marks the
  /// proposal rejected, as a mode refusal does. Run by id once the profile is
  /// wide again, it is the pairing `approval_policy_violations` counts
  /// (`eval-harness/src/runner.rs`).
  #[test]
  fn a_profile_refused_proposal_later_run_by_id_counts_as_an_approval_policy_violation() {
      let fixture = profile_fixture("resume-violation");
      let (_session, proposal_id) = command_refused_at_resume(&fixture);

      fixture.switch_to("full");
      let mut on_output = |_line: &str| {};
      fixture
          .engine()
          .validation_orchestrator
          .run_proposal(&proposal_id, true, "tester", None, &CancelToken::new(), &mut on_output)
          .unwrap();

      let rejected = fixture.audited_proposal_ids("stored_command_rejected");
      let violations = fixture
          .audited_proposal_ids("stored_command_executed")
          .into_iter()
          .filter(|executed| rejected.contains(executed))
          .count();
      assert_eq!(violations, 1);

      fixture.cleanup();
  }

  /// "Does not interrupt an in-flight action": the switch lands while the
  /// approved command is running, and the command still finishes.
  ///
  /// `#[ignore]`d per `AGENTS.md`: it needs a command that really runs for a
  /// while, so it spawns the real login shell (`$SHELL -lc`). Run it by hand:
  ///
  /// ```sh
  /// cargo test -p workspace-engine --test permission_profiles -- --ignored --exact \
  ///   a_command_already_running_finishes_after_a_switch_to_read_only
  /// ```
  #[test]
  #[ignore]
  fn a_command_already_running_finishes_after_a_switch_to_read_only() {
      let fixture = profile_fixture("in-flight");
      let shell = Config::default().shell;
      let engine = fixture.engine_with_shell(&shell);
      let session = chat_session(&engine, &fixture.root, SessionMode::Code);
      let mut adapter = calls_then_answer(&[(
          "run_command",
          r#"{"command":"touch started && sleep 2 && touch finished","reason":"Slow"}"#,
      )]);
      let first = chat_turn(&engine, &fixture.root, &session, &mut adapter);
      let proposal = first.command_proposal.expect("a chained command needs approval");

      // The approval is its own request, so it builds its own engine, under Full.
      let resume_engine = fixture.engine_with_shell(&shell);
      let running = std::thread::spawn(move || {
          approve(&resume_engine, &proposal.id);
      });
      let started = fixture.root.join("started");
      let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
      while !started.exists() {
          assert!(std::time::Instant::now() < deadline, "the command never started");
          std::thread::sleep(std::time::Duration::from_millis(20));
      }
      assert!(!fixture.root.join("finished").exists(), "switch before the end");
      fixture.switch_to("read_only");
      running.join().unwrap();

      assert!(fixture.root.join("finished").exists(), "the command was interrupted");
      assert_eq!(fixture.audited_proposal_ids("stored_command_executed").len(), 1);
      assert_eq!(fixture.load().0.command_access, CommandAccess::None);

      fixture.cleanup();
  }

  const EDIT_RESPONSE: &str = "DAMAIAN_EDIT_V1\nSUMMARY: Add a\nFILE: a.txt\nSTATUS: added\nCONTENT:\nhello\nEND_FILE\nEND_PATCH\n";

  /// Deviation 6: the CLI's sessionless `propose-edit` has no mode, and the
  /// profile still applies to it, before any model call.
  #[test]
  fn propose_edit_under_read_only_is_refused_before_the_model_even_without_a_session() {
      let fixture = profile_fixture("edit-read-only");
      fixture.switch_to("read_only");
      let engine = fixture.engine();
      let mut adapter = MockModelAdapter::new(EDIT_RESPONSE);

      let error = engine
          .edit_orchestrator
          .propose_edit(&fixture.root, "Add a", &[], None, &mut adapter)
          .expect_err("Read-only must refuse");

      assert!(
          matches!(&error, ClientError::AccessDenied(message) if message == PROFILE_REFUSED_EDIT),
          "{error:?}"
      );
      assert!(adapter.requests.is_empty(), "the model was called");
      assert!(!fixture.data_dir.join("patches").exists(), "a patch was stored");

      fixture.cleanup();
  }

  /// An apply is its own decision point: a patch proposed under Full is
  /// refused once the selection is Read-only, and nothing is written.
  #[test]
  fn a_patch_proposed_under_full_is_refused_at_apply_after_switching_to_read_only() {
      let fixture = profile_fixture("apply-read-only");
      let mut adapter = MockModelAdapter::new(EDIT_RESPONSE);
      let proposal = fixture
          .engine()
          .edit_orchestrator
          .propose_edit(&fixture.root, "Add a", &[], None, &mut adapter)
          .expect("Full allows the proposal");

      fixture.switch_to("read_only");
      let error = fixture
          .engine()
          .edit_orchestrator
          .apply_stored_patch(&fixture.root, &proposal.patch.id, None, None, "tester", false)
          .expect_err("Read-only must refuse the apply");

      assert!(
          matches!(&error, ClientError::AccessDenied(message) if message == PROFILE_REFUSED_EDIT),
          "{error:?}"
      );
      assert!(!fixture.root.join("a.txt").exists(), "the patch was applied");

      fixture.cleanup();
  }
  ```

  In `chat.rs`'s mode refusal test module (deviation 8), beside points 8 and 9:

  ```rust
  /// An engine over the same data directory as `engine`, with `narrow`
  /// applied to its config: a new desktop request after the profile changed.
  fn narrowed(engine: &WorkspaceEngine, narrow: impl FnOnce(&mut Config)) -> WorkspaceEngine {
      let mut config = engine.config.clone();
      narrow(&mut config);
      WorkspaceEngine::new(config)
  }

  // Spec 31 Task 5: the web-diagnostic resume point also asks the profile.
  #[test]
  fn a_web_diagnostic_approved_after_the_profile_turned_diagnostics_off_is_refused_at_resume() {
      let repo = temp_repo("resume-web-profile");
      let (engine, calls) = engine_with_web_runner(&repo);
      let session = session_in(&engine, &repo, SessionMode::Code);
      let mut adapter = calls_then_answer(vec![call(
          "call_1",
          "run_web_scenario",
          r##"{"url":"http://localhost:5001/","actions":[{"action":"click","selector":"#go"}]}"##,
      )]);
      let proposal = turn(&engine, &repo, &session, &mut adapter)
          .command_proposal
          .expect("a scenario needs approval in Code");

      let mut after_switch = narrowed(&engine, |config| config.allow_browser_diagnostics = false);
      after_switch
          .chat_orchestrator
          .set_web_diagnostics_runner(WebDiagnosticsRunnerHandle::new(CountingWebRunner {
              calls: calls.clone(),
          }));
      let mut after = MockModelAdapter::new("Understood.");
      resume(&after_switch, &proposal.id, &mut after);

      assert_eq!(calls.load(Ordering::SeqCst), 0, "the diagnostic ran");
      let results = tool_results(&engine, &session);
      assert!(
          results.last().unwrap().contains("(allow_browser_diagnostics=false)"),
          "{results:?}"
      );
      assert!(audit_log(&repo).contains("refused_by_profile"));

      fs::remove_dir_all(repo).unwrap();
  }

  // Spec 31 Task 5: the MCP resume point also asks the profile.
  #[test]
  fn an_mcp_call_approved_after_the_profile_turned_mutating_tools_off_is_refused_at_resume() {
      let repo = temp_repo("resume-mcp-profile");
      let (engine, marker) = engine_with_mcp(&repo, true);
      let session = session_in(&engine, &repo, SessionMode::Code);
      let mut adapter = calls_then_answer(vec![call("call_1", "mcp__fake__echo", "{}")]);
      let proposal = turn(&engine, &repo, &session, &mut adapter)
          .command_proposal
          .expect("the server requires approval");

      let after_switch = narrowed(&engine, |config| config.allow_mutating_mcp_tools = false);
      let mut after = MockModelAdapter::new("Understood.");
      resume(&after_switch, &proposal.id, &mut after);

      assert!(!marker.exists(), "the MCP call reached the server");
      let results = tool_results(&engine, &session);
      assert!(
          results.last().unwrap().contains("(allow_mutating_mcp_tools=false)"),
          "{results:?}"
      );

      fs::remove_dir_all(repo).unwrap();
  }
  ```

- [x] **Step 2: Run them and confirm they fail for the right reason**

  ```bash
  cargo nextest run -p workspace-engine --test permission_profiles
  ```

  Expected, before Step 3: the `mode.rs` tests do not compile (`permits`,
  `ProfileLimit` and `RefusedByProfile` do not exist), so run the integration
  file on its own first. Its Task 5 tests compile against today's public API.
  They should fail as follows:
  - `code_under_read_only…`: `propose_patch` is offered and creates a patch.
  - `a_command_the_profile_blocks…`: an approval card for the blocked proposal.
  - `the_next_turn…`: `run_command` is offered. `ls` is stored blocked and
    pauses, but does not run.
  - `a_command_paused_under_full…`: the resume reaches `run_proposal` and fails
    with `PolicyBlocked`. The `approve` helper unwraps, so it panics.
  - Both edit tests: the profile is not asked, so the edit is proposed and
    applied.

  Two tests are regression guards that already pass:
  `ask_under_full…` (the mode wording) and `a_command_already_running…`
  (nothing re-reads config today). Step 5's mutations are what make them
  fail.

- [x] **Step 3: Implement**

  `command_policy.rs`: make `command_access_permits` `pub(crate)` (deviation
  4).

  `mode.rs`:

  ```rust
  use crate::command_policy::{CommandClassification, command_access_permits};
  use crate::config::CommandAccess;
  use crate::profile::ProfileCapabilities;

  pub(crate) enum Permission {
      Allowed,
      Refused { blocked_by: SessionMode, allowed_in: SessionMode },
      /// The mode allows the action and the resolved profile capabilities
      /// do not (spec 31). Switching mode cannot help, so it names no mode.
      RefusedByProfile { limit: ProfileLimit },
  }

  /// The one capability setting that refused an action. It names the
  /// resolved value, not the profile, because user, repository or admin
  /// config can narrow the same key (spec 31 Task 5, deviation 2).
  #[derive(Debug, Clone, Copy, PartialEq, Eq)]
  pub(crate) enum ProfileLimit {
      FileEdits,
      CommandAccess(CommandAccess),
      BrowserDiagnostics,
      MutatingMcpTools,
      McpDisabled,
  }

  impl ProfileLimit {
      fn setting(self) -> String {
          match self {
              Self::FileEdits => "allow_file_edits=false".to_string(),
              Self::CommandAccess(access) => format!("command_access={}", access.as_str()),
              Self::BrowserDiagnostics => "allow_browser_diagnostics=false".to_string(),
              Self::MutatingMcpTools => "allow_mutating_mcp_tools=false".to_string(),
              Self::McpDisabled => "mcp_enabled=false".to_string(),
          }
      }
  }
  ```

  `refusal_message` matches all three variants. `Allowed` still panics, the
  mode wording is unchanged, and the profile case is:

  ```rust
  Permission::RefusedByProfile { limit } => format!(
      "Refused: the permission profile does not allow this ({}). \
       Switching mode will not allow it.",
      limit.setting()
  ),
  ```

  ```rust
  /// `profile ∩ mode` (spec 31 `context.md` §5): the mode first, then the
  /// profile, and the first refusal wins. Every refusal point calls this, or
  /// `profile_permits` alone where there is no session to read a mode from.
  pub(crate) fn permits(
      mode: SessionMode,
      capabilities: &ProfileCapabilities,
      action: &ToolAction,
      command: Option<&CommandClassification>,
      mcp_tool_read_only: Option<bool>,
  ) -> Permission {
      match mode_permits(mode, action, command, mcp_tool_read_only) {
          Permission::Allowed => profile_permits(capabilities, action, command, mcp_tool_read_only),
          refused => refused,
      }
  }

  /// The profile axis alone: the resolved capability keys crossed with the
  /// tool class. Commands use Task 4's `command_access_permits`, not the
  /// classification's `blocked`, which also means the blocklist, a local
  /// policy verdict that keeps its own blocked-proposal path. Every variant
  /// is named, so a new tool class has to be placed here.
  pub(crate) fn profile_permits(
      capabilities: &ProfileCapabilities,
      action: &ToolAction,
      command: Option<&CommandClassification>,
      mcp_tool_read_only: Option<bool>,
  ) -> Permission {
      let limit = match action {
          ToolAction::ReadFile { .. }
          | ToolAction::ListDirectory { .. }
          | ToolAction::SearchContent { .. }
          | ToolAction::SearchCodebase { .. }
          | ToolAction::ReadGitStatus
          | ToolAction::ReadGitDiff { .. }
          | ToolAction::ProposePlan(_)
          | ToolAction::CompleteStep => None,
          ToolAction::ProposePatch(_) | ToolAction::EditFile { .. } => {
              (!capabilities.allow_file_edits).then_some(ProfileLimit::FileEdits)
          }
          ToolAction::Command(_) => {
              let classification = command.expect(
                  "profile_permits called with ToolAction::Command and no \
                   CommandClassification",
              );
              (!command_access_permits(capabilities.command_access, classification))
                  .then_some(ProfileLimit::CommandAccess(capabilities.command_access))
          }
          ToolAction::WebDiagnostic(_) => {
              (!capabilities.allow_browser_diagnostics).then_some(ProfileLimit::BrowserDiagnostics)
          }
          ToolAction::McpCall { .. } => {
              if !capabilities.mcp_enabled {
                  Some(ProfileLimit::McpDisabled)
              } else if !capabilities.allow_mutating_mcp_tools && mcp_tool_read_only != Some(true) {
                  Some(ProfileLimit::MutatingMcpTools)
              } else {
                  None
              }
          }
      };
      limit.map_or(Permission::Allowed, |limit| Permission::RefusedByProfile { limit })
  }
  ```

  `chat.rs`, calls only:
  - The import is `permits` instead of `mode_permits`.
  - **Tool list:** after `let mode = …` (`:1395`), add
    `let capabilities = self.config.profile_capabilities();`. The three
    `mode_permits(mode, …)` calls become `permits(mode, &capabilities, …)`.
    Under `command_access=none` the placeholder `pwd` is refused, so
    `run_command` is withheld. Update the comments that say `mode_permits`.
  - **`action_permission`:** `permits(mode, &self.config.profile_capabilities(), …)`.
  - **Resume:** read `let capabilities = self.config.profile_capabilities();`
    beside the mode (`:828`).
    - Web: call `permits`. The decision is `refused_by_mode` or
      `refused_by_profile`, depending on the variant.
    - MCP: call `permits`.
    - Command: replace the hand-built classification from stored fields with
      `self.validation_orchestrator.classify_command(Path::new(&proposal.working_directory), &proposal.command)`
      (deviation 5). Call `permits`, and reject with `mode_policy` or
      `profile_policy` (deviation 10).

  `edit.rs`: rename the gate to
  `refuse_unless_mode_and_profile_permit_patches` and update its three
  callers. With an empty session id it asks `profile_permits` alone. Otherwise
  it asks `permits` with the session's mode. In both cases it uses
  `self.config.profile_capabilities()` (deviation 6).

- [x] **Step 4: Run the scoped tests**

  ```bash
  cargo nextest run -p workspace-engine --test permission_profiles --test repository_config_trust --test foundation
  cargo nextest run -p workspace-engine --lib -E 'test(mode::) | test(command_policy::) | test(validation::) | test(chat::) | test(edit::)'
  cargo run -p eval-harness -- run --tier deterministic
  ```

  Expected: all pass. `repository_config_trust.rs` is unmodified. The spec 20
  mode tests in `chat.rs` and `foundation.rs` pass unchanged. The
  deterministic tier reports no new failures and `approval_policy_violations`
  stays 0, because no scenario selects a profile.

- [x] **Step 5: Falsify** (revert each one)

  1. `permits` returns `mode_permits` alone: the matrix fails, and so do
     `code_under_read_only…`, `a_command_the_profile_blocks…` and both edit
     tests. **Result:** 8 failed: the matrix, both `chat.rs` resume tests,
     and five integration tests. **The edit tests did not fail.** The
     sessionless propose calls `profile_permits` directly. The apply path's
     patch carries `propose_edit`'s own session, but no test reaches it
     through `permits` alone. Mutation 7 covers the gate instead.
  2. `permits` asks the profile first: the matrix fails on Ask under
     Read-only, because the mode must be named.
  3. `profile_permits` refuses a command on `classification.blocked` instead
     of `command_access_permits`: the matrix fails, because its `git status`
     row is not blocked but Read-only refuses it.
  4. The tool list keeps `mode_permits`: `code_under_read_only…` fails on
     the offered tools.
  5. The command resume keeps `mode_permits`: `a_command_paused_under_full…`
     panics on `PolicyBlocked`.
  6. The web or MCP resume keeps `mode_permits`: the matching `chat.rs` test
     fails.
  7. The patch gate returns early on an empty session id, as before:
     `propose_edit_under_read_only…` fails. **Result:** only that test
     fails. The apply test still passes, because `propose_edit` stores the
     patch under an edit session of its own. So apply takes the `permits`
     branch, not the empty-id one.
  8. `refusal_message` gives the profile case the mode wording: both message
     tests and `code_under_read_only…` fail.
  9. `a_command_already_running…` is a regression guard. No code mutation
     can make it fail today, because nothing reads config after an engine is
     built. So the test itself is falsified. **As planned, this did not
     work:** moving `switch_to` before the thread changed nothing, because
     the test approved through the turn's engine, which had loaded its
     config before the switch. A resume is a new request with a new engine,
     so the test now builds `resume_engine` just before approving. Moving the
     switch before that line fails the test on "the command never started".

- [x] **Step 6: Scoped checks**

  ```bash
  cargo fmt --all -- --check
  cargo clippy -p workspace-engine --all-targets --locked -- -D warnings
  typos
  cargo check -p desktop-shell -p damaian-cli -p eval-harness --all-targets
  ```

- [x] **Step 7: Update this file's progress row, show the change and the
  check results, and ask before committing**

  Suggested subject: `Refuse what the permission profile does not allow at every mode check`.

## Task 6: Provenance: a source for every applied value

**Requirements:** 2 and §5.7 (the data), and acceptance criterion 5. **Files:**
modify `crates/workspace-engine/src/config.rs`, `profile.rs`, `lib.rs`,
`crates/damaian-cli/src/main.rs` and `crates/workspace-engine/tests/foundation.rs`
(one caller). Create `crates/workspace-engine/src/effective_policy.rs`. Extend
`tests/permission_profiles.rs`.

Expanded from the outline on 2026-10-04, after checking it against the code.
`context.md` §8 decides the shape: provenance is recorded **where values are
applied**, in `apply_overlay_scoped` and its merge helpers, never by a second
resolver. A refused request shows key and class, never the value. Not touched:
`chat.rs`, the web UI (Task 7), and `repository_config_trust.rs`.

**What the outline was checked against:**

- **Callers of `apply_overlay_scoped`, all crates.** `config.rs`: the four
  scopes in `load_scoped`, and `apply_overlay`, which ignores the result.
  `damaian-cli/src/main.rs` `repository_scope_refusals` returns the refusals.
  `mode.rs`'s matrix test, and `permission_profiles.rs`'s
  `resolve_without_profiles`, ignore the result. `foundation.rs`
  `a_repository_cannot_raise_the_users_ceiling` binds it as `rejected`.
  `desktop-shell` and `eval-harness` call only `apply_overlay`, whose
  signature does not change.
- **The merge helpers.** `scoped` keys are Forbidden or User-owned and have no
  restrict-only direction. `union_patterns`, `restrict_only_flag`,
  `restrict_only_access`, `intersect_allowlist`, `restrict_only_limit` and
  `restrict_only_ceiling` each have one, and each now reports what it applied
  and whether a trusted scope moved in the direction an untrusted one is
  refused. `preference` and `lower_wins` report whether their value now holds.
  `upsert_mcp_server_from_repository` reports the fields it applied.
- **`apply_repository_allowlist`** is not a `ConfigScope`. It now returns the
  entries it added, which `load_scoped` keeps, so they can be attributed
  "this repository (Allow Always)".

**Interfaces:**
- Consumes: `Config::load_scoped`, `Config::to_policy_text`,
  `RepositoryConfigReport { rejected_keys, permission_profile, profile_rejected_keys }`,
  `RepositoryKeyClass::as_str`, `SessionMode::{as_str, label}`.
- Produces, re-exported from `lib.rs`:
  - `pub struct AppliedKey { pub key: String, pub scope: ConfigScope, pub entries: Option<Vec<String>>, pub widened: bool }`.
    `key` is the `to_policy_text` name, so MCP servers and model providers
    are recorded per field: `mcp_server.<id>.<field>`,
    `model_provider.<id>.<field>`. `entries` is `Some` for the seven list keys.
  - `pub struct OverlayOutcome { pub rejected: Vec<RejectedConfigKey>, pub applied: Vec<AppliedKey> }`,
    returned by `Config::apply_overlay_scoped`. Every caller that read the
    `Vec` now reads `.rejected`.
  - `RepositoryConfigReport` gains `pub applied: Vec<AppliedKey>` (user,
    repository, admin, profile, in order) and
    `pub allow_always_entries: Vec<String>`.
  - `Config::apply_repository_allowlist(&mut self, &Path) -> Vec<String>`:
    the entries it added.
  - `ProfileId::label(&self) -> &str`: "Read-only", "Safe local
    development", "Full repository development", "Offline private", or the
    custom name. Task 7's picker reuses it.
  - `effective_policy.rs`: `EffectivePolicy`, `PolicyRule`, `PolicyEntry`,
    `PolicySource`, `SourceKind`, `RefusedRequest`, `RefusedBy`, with
    `EffectivePolicy::resolve(Option<&Path>, Option<SessionMode>) -> Result<Self>`,
    `EffectivePolicy::from_load(&Config, &RepositoryConfigReport, Option<SessionMode>) -> Self`,
    `rule(&self, key) -> Option<&PolicyRule>` and `to_text(&self) -> String`.
  - CLI `damaian config-show --sources [repo]`.

**The attribution rules:**

- A key's source is **the last scope whose value it holds**. A trusted scope
  holds what it set. An untrusted scope (repository, profile) holds a value it
  set in the restrictive direction, including one equal to what was already
  there, because that value would now survive the user loosening their own
  config. So Safe local's `require_approval_for_file_edits=true` is attributed
  to the profile even when user config also says `true`.
- A list entry's source is the last scope that held that entry. A trusted
  scope replaces the list and holds all of it. An untrusted one holds what it
  listed. `intersect_allowlist` reports only the ids that survived, never the
  ones it refused. After every overlay, Allow Always entries are attributed
  to "this repository (Allow Always)", but only those the fold actually added.
- `widened` is set only at admin scope, and only where the merge has a
  direction: a flag moved off its restrictive value, `command_access` raised,
  a pattern removed from a union list, an id added to or the restriction
  removed from `mcp_server_allowlist`, a limit or ceiling raised (strictly),
  or an MCP server enabled or its approval gate cleared.
- A key no overlay applied is `default`.

**The `EffectivePolicy` JSON shape Task 7 serves** (serde, camelCase):

```json
{
  "header": "Safe local development ∩ Code mode",
  "profile": "safe_local",
  "profileLabel": "Safe local development",
  "profileSelected": true,
  "mode": "code",
  "rules": [
    {
      "key": "restricted_patterns",
      "value": ".env|*.pem|secrets/**",
      "sources": [
        { "kind": "user", "label": "user config" },
        { "kind": "repository", "label": "repository config" }
      ],
      "entries": [
        { "value": ".env", "source": { "kind": "user", "label": "user config" } },
        { "value": "*.pem", "source": { "kind": "user", "label": "user config" } },
        { "value": "secrets/**", "source": { "kind": "repository", "label": "repository config" } }
      ],
      "adminWidened": false,
      "refused": []
    },
    {
      "key": "require_approval_for_file_edits",
      "value": "true",
      "sources": [{ "kind": "profile", "label": "profile: safe_local" }],
      "entries": null,
      "adminWidened": false,
      "refused": [
        { "key": "require_approval_for_file_edits", "class": "restrict_only", "by": "repository" }
      ]
    }
  ],
  "otherRefused": [
    { "key": "model_provider.openai", "class": "forbidden", "by": "repository" }
  ]
}
```

- `rules` has one rule per line of `Config::to_policy_text()`, in that order,
  so the view and the plain text never disagree. `value` is that line's
  value. A list key's `value` is `|`-joined, and `entries` holds the items.
- `sources[].kind` is one of `default`, `user`, `repository`, `admin`,
  `profile` or `allowAlways`. A scalar has exactly one source. A list has the
  distinct sources of its entries, in entry order.
- `entries` is non-null only for `allowed_roots`, `ignore_patterns`,
  `restricted_patterns`, `command_allowlist`, `command_blocklist`,
  `secret_patterns` and `mcp_server_allowlist`.
- `refused[].by` is `repository` or `profile`. `class` is
  `RepositoryKeyClass::as_str`. No refused value is in the structure.
- `mode` is `null`, and `header` is the profile label alone, when no mode is
  given.

- [x] **Step 1: Write the failing tests**

  Append to `crates/workspace-engine/tests/permission_profiles.rs`, and add
  `AppliedKey`, `EffectivePolicy`, `RefusedBy`, `RefusedRequest` and
  `SourceKind` to its imports:

  ```rust
  // Task 6: provenance recorded where values are applied (context.md §8).

  /// Every rule's entries as `(value, source kind)`, in the resolved order.
  fn entry_sources(policy: &EffectivePolicy, key: &str) -> Vec<(String, SourceKind)> {
      policy
          .rule(key)
          .unwrap_or_else(|| panic!("{key} has no rule"))
          .entries
          .as_ref()
          .unwrap_or_else(|| panic!("{key} is a list key and must carry entries"))
          .iter()
          .map(|entry| (entry.value.clone(), entry.source.kind))
          .collect()
  }

  fn source_kinds(policy: &EffectivePolicy, key: &str) -> Vec<SourceKind> {
      policy
          .rule(key)
          .unwrap_or_else(|| panic!("{key} has no rule"))
          .sources
          .iter()
          .map(|source| source.kind)
          .collect()
  }

  #[test]
  fn apply_overlay_scoped_reports_what_it_applied_alongside_what_it_refused() {
      let mut config = Config::default();
      let outcome = config.apply_overlay_scoped(
          ConfigOverlay::parse(concat!(
              "restricted_patterns=secrets/**\n",
              "require_approval_for_file_edits=false\n",
              "shell=/tmp/evil-shell\n",
          ))
          .unwrap(),
          ConfigScope::Repository,
      );

      let refused: Vec<&str> = outcome
          .rejected
          .iter()
          .map(|key| key.key.as_str())
          .collect();
      assert_eq!(refused, ["shell", "require_approval_for_file_edits"]);
      assert_eq!(
          outcome.applied,
          [AppliedKey {
              key: "restricted_patterns".into(),
              scope: ConfigScope::Repository,
              entries: Some(vec!["secrets/**".into()]),
              widened: false,
          }],
          "a refused key must not also be reported as applied"
      );
  }

  #[test]
  fn the_section_5_7_example_attributes_each_list_entry_to_its_scope() {
      let fixture = profile_fixture("sources-5-7");
      fixture.write_user(&format!(
          "restricted_patterns=.env|*.pem\nrequire_approval_for_file_edits=true\n\
           command_allowlist=cargo check\ncommand_allowlist.{}=cargo test\n{}",
          fixture.repository_id,
          fixture.select("safe_local"),
      ));
      fixture.write_repository("restricted_patterns=secrets/**\n");

      let (config, report) = fixture.load();
      let policy = EffectivePolicy::from_load(&config, &report, Some(SessionMode::Code));

      assert_eq!(policy.header, "Safe local development ∩ Code mode");
      assert_eq!(
          entry_sources(&policy, "restricted_patterns"),
          [
              (".env".to_string(), SourceKind::User),
              ("*.pem".to_string(), SourceKind::User),
              ("secrets/**".to_string(), SourceKind::Repository),
          ]
      );
      assert_eq!(
          entry_sources(&policy, "command_allowlist"),
          [
              ("cargo check".to_string(), SourceKind::User),
              ("cargo test".to_string(), SourceKind::AllowAlways),
          ]
      );
      let allow_always = &policy
          .rule("command_allowlist")
          .unwrap()
          .entries
          .as_ref()
          .unwrap()[1];
      assert_eq!(allow_always.source.label, "this repository (Allow Always)");
      // The user set it, and Safe local sets it again: the profile is what holds
      // it now, so the profile is named.
      let file_edits = policy.rule("require_approval_for_file_edits").unwrap();
      assert_eq!(file_edits.sources.len(), 1);
      assert_eq!(file_edits.sources[0].kind, SourceKind::Profile);
      assert_eq!(file_edits.sources[0].label, "profile: safe_local");
      assert_eq!(
          source_kinds(&policy, "max_file_bytes"),
          [SourceKind::Default]
      );
      assert_eq!(
          source_kinds(&policy, "command_access"),
          [SourceKind::Profile]
      );

      let text = policy.to_text();
      assert!(text.starts_with("Effective policy — Safe local development ∩ Code mode\n"));
      assert!(text.contains("secrets/**"), "{text}");
      assert!(text.contains("repository config"), "{text}");

      fixture.cleanup();
  }

  #[test]
  fn an_admin_widening_is_marked_and_an_admin_narrowing_is_not() {
      let fixture = profile_fixture("admin-widening");
      fixture.write_user(concat!(
          "require_approval_for_all_commands=true\n",
          "require_approval_for_file_edits=false\n",
          "command_access=read_only\n",
          "restricted_patterns=.env|*.pem\n",
          "max_read_lines=100\n",
      ));
      let admin = fixture.data_dir.join("config").join("admin.conf");
      fs::write(
          &admin,
          concat!(
              "require_approval_for_all_commands=false\n",
              "command_access=all\n",
              "restricted_patterns=.env\n",
              "max_read_lines=1000\n",
              // Narrowings and no-ops: none of these is a widening.
              "require_approval_for_file_edits=false\n",
              "max_list_entries=10\n",
              "allow_file_edits=false\n",
          ),
      )
      .unwrap();

      let (config, report) = fixture.try_load(Some(&admin)).unwrap();
      let policy = EffectivePolicy::from_load(&config, &report, None);

      for key in [
          "require_approval_for_all_commands",
          "command_access",
          "restricted_patterns",
          "max_read_lines",
      ] {
          let rule = policy.rule(key).unwrap();
          assert!(rule.admin_widened, "{key} was widened by admin");
          assert_eq!(source_kinds(&policy, key), [SourceKind::Admin], "{key}");
      }
      for key in [
          "require_approval_for_file_edits",
          "max_list_entries",
          "allow_file_edits",
      ] {
          let rule = policy.rule(key).unwrap();
          assert!(!rule.admin_widened, "{key} was not widened");
          assert_eq!(source_kinds(&policy, key), [SourceKind::Admin], "{key}");
      }
      assert_eq!(policy.header, "Full repository development");
      assert!(policy.to_text().contains("widened by admin config"));

      // The user's own loosening of a default is not an admin widening.
      let fixture_user = profile_fixture("user-loosening");
      fixture_user.write_user("require_approval_for_risky_commands=false\n");
      let (config, report) = fixture_user.load();
      let policy = EffectivePolicy::from_load(&config, &report, None);
      let rule = policy.rule("require_approval_for_risky_commands").unwrap();
      assert!(!rule.admin_widened);
      assert_eq!(
          source_kinds(&policy, "require_approval_for_risky_commands"),
          [SourceKind::User]
      );

      fixture.cleanup();
      fixture_user.cleanup();
  }

  #[test]
  fn a_refused_request_is_shown_by_key_and_class_never_by_value() {
      let fixture = profile_fixture("refused-no-value");
      fixture.write_user(&format!(
          "{RESTRICTIVE_USER}mcp_server_allowlist=blessed|other\n{}",
          fixture.select("mine")
      ));
      // A partial overlap: the shared id is kept, and the repository's own id
      // must not reach the view as an entry.
      fixture.write_repository(&format!(
          "{HOSTILE_REPOSITORY}mcp_server_allowlist=blessed|attacker\n"
      ));
      fixture.write_custom_profile("mine", "max_file_bytes=7654321\nshell=/tmp/profile-shell\n");

      let (config, report) = fixture.load();
      let policy = EffectivePolicy::from_load(&config, &report, Some(SessionMode::Ask));

      let file_edits = policy.rule("require_approval_for_file_edits").unwrap();
      assert_eq!(file_edits.value, "true");
      assert_eq!(
          file_edits.refused,
          [RefusedRequest {
              key: "require_approval_for_file_edits".into(),
              class: "restrict_only".into(),
              by: RefusedBy::Repository,
          }]
      );
      let refusal = serde_json::to_string(&file_edits.refused).unwrap();
      assert!(!refusal.contains("false"), "{refusal}");
      assert!(
          policy
              .rule("max_file_bytes")
              .unwrap()
              .refused
              .iter()
              .any(|refused| refused.by == RefusedBy::Profile && refused.class == "forbidden")
      );

      let json = serde_json::to_string(&policy).unwrap();
      let text = policy.to_text();
      // The record itself, not only what the view chose to print from it: a
      // refused value must never be recorded as applied.
      let applied = format!("{:?}", report.applied);
      for output in [&json, &text, &applied] {
          for value in [
              "./tools/sh",
              "damaian-attacker",
              "127.0.0.1:9",
              "ATTACKER",
              "attacker",
              "npm install",
              "7654321",
              "profile-shell",
          ] {
              assert!(!output.contains(value), "{value} leaked into\n{output}");
          }
      }
      let refused_line = text
          .lines()
          .skip_while(|line| !line.starts_with("require_approval_for_file_edits ="))
          .nth(1)
          .unwrap();
      assert!(refused_line.contains("refused"), "{text}");
      assert!(!refused_line.contains("false"), "{refused_line}");

      fixture.cleanup();
  }

  #[test]
  fn the_effective_policy_agrees_with_load_scoped_for_every_key() {
      let fixture = profile_fixture("resolver-agreement");
      fixture.write_user(&format!(
          "{RESTRICTIVE_USER}command_allowlist.{}=cargo test\n\
           mcp_server.docs.transport=stdio\nmcp_server.docs.command=/usr/bin/true\n\
           mcp_server.docs.enabled=true\nmodel_provider.deepseek.max_output_tokens=4096\n\
           max_read_lines=100\n{}",
          fixture.repository_id,
          fixture.select("offline_private"),
      ));
      fixture.write_repository(&format!(
          "{HOSTILE_REPOSITORY}restricted_patterns=secrets/**\nmax_list_entries=50\n"
      ));
      let admin = fixture.data_dir.join("config").join("admin.conf");
      fs::write(
          &admin,
          "max_read_lines=300\nignore_patterns=target/|dist/\n",
      )
      .unwrap();

      let (config, report) = fixture.try_load(Some(&admin)).unwrap();
      let policy = EffectivePolicy::from_load(&config, &report, None);
      // A second, independent load: the view must agree with the resolver, not
      // with the copy it was built from.
      let (fresh, _) = fixture.try_load(Some(&admin)).unwrap();

      let expected: Vec<(String, String)> = fresh
          .to_policy_text()
          .lines()
          .map(|line| {
              let (key, value) = line.split_once('=').unwrap();
              (key.to_string(), value.to_string())
          })
          .collect();
      let actual: Vec<(String, String)> = policy
          .rules
          .iter()
          .map(|rule| (rule.key.clone(), rule.value.clone()))
          .collect();
      assert_eq!(actual, expected);

      let strings = |values: &[String]| values.to_vec();
      let lists: [(&str, Vec<String>); 7] = [
          (
              "allowed_roots",
              fresh
                  .allowed_roots
                  .iter()
                  .map(|root| root.to_string_lossy().to_string())
                  .collect(),
          ),
          ("ignore_patterns", strings(&fresh.ignore_patterns)),
          ("restricted_patterns", strings(&fresh.restricted_patterns)),
          ("command_allowlist", strings(&fresh.command_allowlist)),
          ("command_blocklist", strings(&fresh.command_blocklist)),
          ("secret_patterns", strings(&fresh.secret_patterns)),
          ("mcp_server_allowlist", strings(&fresh.mcp_server_allowlist)),
      ];
      for (key, values) in lists {
          let Some(rule) = policy.rule(key) else {
              assert!(values.is_empty(), "{key} has values but no rule");
              continue;
          };
          let entries: Vec<String> = rule
              .entries
              .as_ref()
              .unwrap()
              .iter()
              .map(|entry| entry.value.clone())
              .collect();
          assert_eq!(entries, values, "{key}");
      }
      assert_eq!(
          policy.rule("command_access").unwrap().value,
          fresh.command_access.as_str()
      );
      assert_eq!(policy.rule("max_read_lines").unwrap().value, "300");
      assert_eq!(source_kinds(&policy, "max_read_lines"), [SourceKind::Admin]);
      assert_eq!(
          source_kinds(&policy, "max_list_entries"),
          [SourceKind::Repository]
      );
      // The user turned it off and Offline private turns it off again.
      assert_eq!(source_kinds(&policy, "mcp_enabled"), [SourceKind::Profile]);
      assert_eq!(source_kinds(&policy, "shell"), [SourceKind::User]);
      assert_eq!(
          source_kinds(&policy, "checkpoint_retention_days"),
          [SourceKind::Profile]
      );
      assert_eq!(
          source_kinds(&policy, "mcp_server.docs.command"),
          [SourceKind::User]
      );
      assert_eq!(
          source_kinds(&policy, "model_provider.deepseek.max_output_tokens"),
          [SourceKind::User]
      );
      assert_eq!(
          source_kinds(&policy, "model_provider.deepseek.label"),
          [SourceKind::Default]
      );

      fixture.cleanup();
  }
  ```

- [x] **Step 2: Run them and watch them fail**

  ```bash
  cargo nextest run -p workspace-engine --test permission_profiles
  ```

  Expected: the test target does not compile. The five names are unresolved,
  and `.rejected`/`.applied` do not exist on `Vec<RejectedConfigKey>`.

- [x] **Step 3: Record what each merge applied**

  In `config.rs`:
  1. Add `AppliedKey` and `OverlayOutcome` after `RejectedConfigKey`.
     `OverlayOutcome::record(scope, key, entries, loosened)` is private, and
     sets `widened = loosened && scope == ConfigScope::Admin`.
  2. `apply_overlay_scoped` returns `OverlayOutcome`. Collect the refusals in
     a local `Vec`, so a helper can hold it while `outcome.record` runs, and
     move it into `outcome.rejected` at the end. Record after every
     application, in the order the keys are applied today. **Keep the order
     of the refusals.** `command_access` stays between `allow_file_edits` and
     `allow_browser_diagnostics`, so `report.rejected_keys` does not reorder.
  3. The helpers return what they applied:
     - `union_patterns -> Option<(Vec<String>, bool)>`: trusted gives
       `(incoming, removed_any)`; untrusted gives `(incoming, false)`, or
       `None` when it listed nothing.
     - `restrict_only_flag`, `restrict_only_access`, `restrict_only_limit` and
       `restrict_only_ceiling -> Option<bool>`: `Some(loosened)` when applied,
       `None` when refused or a no-op. Limit and ceiling widen only on `>`,
       and the untrusted refusal stays `>=`.
     - `intersect_allowlist -> Option<(Vec<String>, bool)>`: after a
       narrowing, it returns the retained list, never `incoming`.
     - `lower_wins -> bool`: whether the profile's value holds.
     - `upsert_mcp_server_from_repository -> Vec<&'static str>`: the narrowed
       overlay's present fields.
  4. Add `present_fields` to `ModelProviderConfigOverlay` and
     `McpServerConfigOverlay`, as exhaustive destructures, so a new field
     cannot go unattributed.
  5. When `model_provider` is applied, also record `model_name`,
     `model_base_url` and `model_api_key_env` for that scope, if
     `apply_model_provider_defaults` changed them.
  6. `load_scoped` extends `report.applied` from each scope's outcome and sets
     `report.allow_always_entries` from `apply_repository_allowlist`.
  7. Update the callers: `damaian-cli` `repository_scope_refusals` and
     `foundation.rs` read `.rejected`.

- [x] **Step 4: Build `EffectivePolicy`**

  Create `effective_policy.rs` with the types above. `from_load`:
  - walks `config.to_policy_text()` line by line;
  - takes each scalar's source from the last `AppliedKey` with that key;
  - takes each list entry's source from the last `AppliedKey` whose entries
    contain it, then from `allow_always_entries` for `command_allowlist`;
  - sets `adminWidened` from any admin record with `widened`;
  - attaches the repository and profile refusals to the rule with that key,
    and puts the rest in `otherRefused`.

  `resolve` is `load_for_repository_reporting` followed by `from_load`.
  `to_text` prints `key = value    [sources]`, then
  `(widened by admin config)`, a `refused: <by> asked to set <key> (<class>)`
  line per refusal, and the entries when a list has more than one source.
  Register the module and re-export from `lib.rs`. Add `ProfileId::label`.

- [x] **Step 5: CLI**

  `config-show --sources [repo]` prints `EffectivePolicy::resolve(repo, None)?.to_text()`.
  Without the flag, the output is unchanged. Update `usage()`.

- [x] **Step 6: Run the scoped tests**

  ```bash
  cargo nextest run -p workspace-engine --test permission_profiles --test repository_config_trust --test foundation
  ```

  Expected: all pass. `repository_config_trust.rs` is unmodified.

- [x] **Step 7: Falsify** (revert each one)

  Each was run against the five new tests, with the source restored after
  every run.
  1. `union_patterns` untrusted returns `None`: the outcome test and the §5.7
     test fail (`secrets/**` loses its source).
  2. `record` sets `widened = loosened` at every trusted scope. **At first
     not caught**, because `from_load` also checked `scope == Admin`, so the
     rule lived in two places. `from_load` now reads `widened` alone, and the
     mutation fails the admin test's user-loosening control.
  3. The flag's `loosens` ignores the current value: the admin test fails on
     `require_approval_for_file_edits=false` over an equal user value.
  4. `from_load` renders `Config::default().to_policy_text()`: the agreement
     test fails.
  5. A narrowed `mcp_server_allowlist` records `incoming`. **At first not
     caught**, because the view takes entries from `Config`, so the refused
     id was recorded but never printed. The refusal test now also scans
     `report.applied`, and fails on `attacker`.
  6. The Allow Always result is dropped in `load_scoped`: the §5.7 test fails
     (`cargo test` becomes `default`). The first version of this mutation did
     not compile, and was replaced.
  7. The first applier wins instead of the last: the §5.7 test, the admin
     test and the agreement test fail.
  8. `restrict_only_limit` never loosens: the admin test fails on
     `max_read_lines`.

- [x] **Step 8: Scoped checks**

  ```bash
  cargo fmt --all -- --check
  cargo clippy -p workspace-engine -p damaian-cli --all-targets --locked -- -D warnings
  typos
  cargo check -p desktop-shell -p damaian-cli -p eval-harness --all-targets
  ```

  Then a manual `damaian config-show --sources <repo>` against a scratch
  `DAMAIAN_DATA_DIR` and `DAMAIAN_ADMIN_CONFIG`.

- [x] **Step 9: Update this file's progress row, show the change and the
  check results, and ask before committing**

  Suggested subject: `Attribute every effective policy rule to its source`.

**Deviations from the outline:**

1. The provenance travels on `RepositoryConfigReport` (`applied`,
   `allow_always_entries`). `load_scoped` keeps its signature, so no caller of
   it changes.
2. Allow Always is not a `ConfigScope`, so it is not an `AppliedKey`.
   `apply_repository_allowlist` returns what it added. An entry user config
   already listed stays attributed to user config.
3. The source rule is "the last scope whose value holds". An untrusted scope
   that sets the restrictive value, or relists an entry, takes the
   attribution, because its value is the one that survives a user loosening.
   The outline said "the entries each scope contributed". A union now reports
   every entry it listed, not only the new ones.
4. `widened` is defined only where a restrict-only direction exists. An admin
   value for a Forbidden or User-owned key (`audit_enabled`,
   `block_generated_secrets`, `allowed_roots`, `secret_patterns`,
   `command_allowlist`, …) is attributed to admin, but never marked a
   widening. Whether criterion 5 needs more is a Task 9 question for the
   second-person review.
5. A limit or ceiling widens on `>`. The untrusted refusal of an equal value
   (`>=`) is unchanged.
6. `resolve` takes `Option<&Path>` and an optional mode. `from_load` exists
   because `resolve` reads `DAMAIAN_DATA_DIR`, which tests cannot set per
   test.
7. Rules are `to_policy_text`'s lines. So `agent_max_task_tokens` and
   `mcp_server_allowlist` have no rule while unset, exactly as the text omits
   them, and a refusal of a key with no rule goes to `otherRefused`.
8. The repository `command_allowlist` entries awaiting spec 34's migration,
   shown as "NOT APPLIED, pending your review" in §5.7's example, are **not**
   in `EffectivePolicy`, because they are refused values. They appear only as
   a `user_owned` refusal. `context.md` §8's one exception stays with the
   existing migration notice, which Task 7 can show beside the table.
9. MCP servers and model providers are attributed per field. Choosing a
   `model_provider` also attributes the three fields its defaults changed.
10. `ProfileId::label` was added here rather than in Task 7, for the header.

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

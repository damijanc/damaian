# Repository Map and Monorepo Boundaries Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Implements:** [`proposal.md`](proposal.md) in full · corrections and the
decisions it left open in [`context.md`](context.md)
**Started:** 2026-10-06

## Progress

| Task | State | Notes |
|---|---|---|
| 1 · Root detection from the index's path lists | Done 2026-10-06 | Landed as planned: `PROJECT_MANIFESTS` in `command_policy.rs` (`detect_project_commands` unchanged), new `repository_map.rs` with `detect_roots`, `MAX_ROOT_DEPTH = 6`, `VENDOR_DIRECTORIES` and the serde types, re-exported from `lib.rs`. Nothing calls `detect_roots` outside tests yet. `tests/repository_map.rs`: 7 tests, all pass; written first and confirmed failing to compile. Mutations, each reverted and confirmed failing for the stated reason: (1) `>=` for `>` on the depth ceiling fails the depth test (`a/b/c/d/e/f` excluded); (2) dropping the files-loop `vendor_prefix` check fails the vendor test (`node_modules/left-pad` and `packages/api/node_modules/react` become roots); (3) first-seen-wins evidence fails the order test (backward run names `Cargo.toml`); (4) adding `setup.py` to `PROJECT_MANIFESTS` fails the manifest-command test with its §10 message; (5) dropping the repository-root insertion fails the no-manifest and depth tests; (6) `contains` for `==` fails the nested-roots test (`docs` and `examples` become roots). `repository_config_trust` and `permission_profiles`: 97 pass, 1 skipped, neither file touched. `fmt --check`, scoped `clippy -D warnings`, `typos` clean. No deviation from the plan |
| 2 · Root-aware command classification, proposals and Allow Always | Done 2026-10-06 | Landed as planned. `command_policy.rs`: `classify_at(command, repository_root, working_directory)`, with `classify` delegating as `(wd, wd)`. Also `pub(crate) relative_location` and `root_qualified_allowlist_entry`, a three-argument `references_path_outside_root` that resolves tokens from the directory and checks containment against the root, and `classify_pattern(command, location)`. `command_runner.rs`: `run_at`, with `run` delegating. `validation.rs`: `CommandProposal.repository_root`, plus `propose_command_at` and `classify_command_at` (the old methods delegate). `run_proposal` calls `run_at` with the proposal's pair. `REPOSITORY_ROOT` is written after `STATUS`, and the reader falls back to `working_directory` when it is absent. `command_proposal_stored` audits `repositoryRoot`. `allow_command_always` takes the id from `repository_root` and stores `cd <rel> && <cmd>` in a sub-root. `tests/repository_map.rs`: 10 new tests, 17 in total, all pass. They were written first and failed to compile on the missing API. Mutations, each reverted and confirmed failing: (1) a plain entry matching at every location fails `a_plain_allowlist_entry_matches_only_at_the_repository_root` (`packages/api` became Low); (2) containment against the working directory fails the containment test on `../web/src/index.ts`; (3) the repository id from the working directory fails the Allow Always test (no `command_allowlist.<root id>=cd packages/api && npm test` line); (4) `run_proposal` at the repository root fails the run test (`working_directory` does not end in `packages/api`). `repository_config_trust` and `permission_profiles`: 97 pass, 1 skipped, neither file touched. The `workspace-engine` lib tests pass too, 472 in this run together with the two suites. `fmt --check`, scoped `clippy -D warnings` and `typos` are clean. **Deviations:** `lib.rs` is unchanged, because the new API is methods plus `pub(crate)` helpers. `command_policy.rs`'s three inline `references_path_outside_root` unit tests now pass `(root, root)`. **Decisions the plan left open:** a working directory outside the repository (`relative_location` is `None`) escalates Low to Medium with the reason "Working directory is outside the selected repository". Allow Always on such a proposal returns `PolicyBlocked` rather than writing a grant that can never match. The `command_allowlisted` audit's `command` field records the entry written, so the qualified form in a sub-root. **For Task 3:** per-root `ProjectCommand.risk` should come from `classify_at(cmd, repo_root, root_dir)` so qualified grants count. `relative_location` gives the `""`/`rel` form `root_qualified_allowlist_entry` expects. Nothing in production calls the `_at` variants yet (Task 7 wires `chat.rs`) |
| 3 · `RepositoryMap::build`: per-root metadata and commands | Done 2026-10-06 | Landed as planned. `repository_map.rs`: `REPOSITORY_MAP_SCHEMA_VERSION = 1`, `RootCommand`, `ProjectRoot` (no `user_override` yet), `RepositoryMap { repository_id, schema_version, generated_at_ms: u128, fingerprint, roots, excluded }`, `RepositoryMap::build(&RepositoryIndex, &CommandPolicy)` and `root_for_path`, all re-exported from `lib.rs`. `command_policy.rs`: `CommandRisk` derives serde (`lowercase`), the body of `detect_project_commands` moved into private `detect_in(repository_root, directory)` classifying with `classify_at`, `detect_project_commands(root)` is `detect_in(root, root)`, and `pub fn detect_root_commands(repository_root, root: &str) -> Result<Vec<RootCommand>>` sorts by name. `context_manager.rs`: `agent_instruction_paths` and `AGENT_INSTRUCTIONS_FILE` became `pub(crate)`, nothing else. `tests/repository_map.rs`: 7 new tests, 24 in total, all pass. They were written first and failed to compile on the missing types. `detect_project_commands_is_unchanged` pins today's exact output (script order, shortcut, then table, unsorted, a plain grant applying) and passed before and after, as a pin should. Mutations, each reverted and confirmed failing: (1) `languages.sort()` removed fails the nested-roots test (`crates/engine` is met as markdown, toml, rust) and the determinism test's reversed-index build; (2) `owns` as `path.starts_with(root)` fails `root_for_path_…` (`packages/apiary/x.ts` → `packages/api`) and the nested-roots test; (3) fingerprint without manifest content hashes fails the fingerprint test; (4) `detect_in` classifying with `(directory, directory)` fails `a_roots_command_risk_honours_a_grant_qualified_with_that_root` (Medium, not Low). `repository_config_trust` and `permission_profiles`: 97 pass, 1 skipped, neither file touched. The `workspace-engine` lib tests with those two suites and this one: 496 pass, 5 skipped. `fmt --check`, scoped `clippy -D warnings` and `typos` are clean. **Decisions the plan left open:** every path in a `ProjectRoot` is repository-relative, like `manifests` and `instruction_files`. Entry points and test directories are *matched* root-relative but stored repository-relative (`packages/api/src/index.ts`). A child directory is listed for every enclosing root, not only the nearest, so `crates` is a major directory of `""` although its files belong to member crates. Test directories also appear in `major_directories`. `languages` drops only `text`, so `markdown`, `json` and `toml` appear. `instruction_files` keeps `agent_instruction_paths`' broadest-first order, the one collection not sorted, because the order is the precedence. A `detect_root_commands` error gives the root no commands rather than failing the build. **Deviation:** the fingerprint also hashes the `excluded` entries, because `generated_paths` is derived from the skip list and §3's input list missed it. **For Task 4:** `build` builds the fingerprint input as a string in `fingerprint()`. Append the override lists there. Adding `user_override` to `ProjectRoot` means updating the three full-struct literals in `nested_roots_each_carry_their_own_metadata_and_commands`. Decide whether a removed root still counts as a root for `root_for_path`, `nearest_root` and the major-directory rule. Today a child that is a root is never a major directory. **For Task 5:** commands are the only part of `build` that reads the disk (`package.json` scripts). The fingerprint covers manifest content, so a stale `Reused` map cannot hold old scripts |
| 4 · User overrides in repository config | Done 2026-10-07 | Landed as planned. `config.rs`: `project_roots_added`/`project_roots_removed` (`Vec<String>` on `Config`, default empty, `Option<Vec<String>>` on `ConfigOverlay`, `\|`-separated through `split_list`/`join_list`), classified **Preference** in `classify_overlay_fields!`, applied with `preference(..)` in `apply_overlay_scoped`, and handled in `set` and both `to_policy_text`s. `profile.rs`: `split_profile_keys` refuses both as Forbidden (not carried). `effective_policy.rs`: `rule_label` names both. `repository_map.rs`: `RootEvidence::UserOverride`, `ExclusionReason::InvalidOverride`, `pub enum RootOverride { Added, Removed }`, `ProjectRoot.user_override`, private `apply_overrides` and `valid_override`, the fingerprint hashes both lists, and `pub enum RootOverrideEdit { Add, Remove, Clear }` with `pub fn edit_root_overrides(&mut ConfigOverlay, edit, path)`, all re-exported. CLI: `damaian repo-root <repo> add\|remove\|clear <path>` edits `<repo>/.damaian/config.conf` through `ConfigOverlay::load_or_default` and `save`, so a line the strict parser rejects refuses the write with that error (smoke-tested by hand, including the refusal). `tests/repository_map.rs`: 7 new tests, 31 in total, all pass. They were written first and failed to compile on the missing fields and variants. `tests/permission_profiles.rs`: both keys added to `the_preference_keys_are_exactly_spec_34s_free_keys` and a `PreferenceCase` each, nothing else touched, 50 pass, 1 ignored. `repository_config_trust`: 47 pass, file untouched. The `workspace-engine` lib tests pass too: 375, with 4 ignored. Mutations, each reverted and confirmed failing: (1) `valid_override` always true fails the invalid-entry test; (2) an added root that detection did not find not pushed (the override lost on a rescan) fails the added test and the rebuild test; (3) both keys classed `Capability` fails all three spec 31 partition tests; (4) `removed` skipping validation for `""` fails the invalid-entry test (the root is marked removed). `fmt --check`, scoped `clippy -D warnings` (`workspace-engine`, `damaian-cli`) and `typos` are clean. **Deviations:** `build` keeps its `(index, policy)` signature and reads both lists and `restricted_patterns` from `policy.config()`, instead of taking the lists as arguments. That way the config that classifies the commands also decides the roots, and no Task 3 caller changed. `edit_root_overrides` is in the engine, not the CLI, so Task 8's endpoint can reuse it. `EVERY_PREFERENCE` in `permission_profiles.rs` was left alone, as instructed: the profile-split agreement test does not cover the new keys, but `repository_config_sets_the_override_keys_and_a_profile_cannot_carry_them` covers the split for them. **Decisions the plan left open:** a removed root is **not** a root for `root_for_path`, `nearest_root` or the major-directory rule. Its files, languages, vendor output and directory go to the nearest root that is not removed, so `examples/legacy` becomes a major directory of `examples`. Reason: the user said it is not a project, and Task 6's rendering and Task 7's finding resolution use ownership, so a removed root that still owned its files would keep shaping both. It stays in `roots` with its detected evidence, `user_override: Some(Removed)` and every collection empty. A removed root keeps its manifest evidence rather than `UserOverride`, so the UI can say "found by `package.json`, removed by you". More rejected entries beyond the plan's list: a `removed` entry that names no root (it would do nothing, so the user is shown it), a path in both lists (neither applies), and a file rather than a directory. The restricted check tests every ancestor, so a pattern on `secrets` also rejects `secrets/app`. Added roots are not held to `MAX_ROOT_DEPTH`. **For Task 5:** the map also depends on config the fingerprint does not hash: `command_allowlist` (command risk) and `restricted_patterns` (override validation). A `Reused` map can therefore be stale after either one changes. Hash them, or rebuild on a config change. `propose_detected_validations` must skip roots with `user_override == Some(Removed)`. Their `commands` are already empty, so skipping by that field gives the same result |
| 5 · Persistence, rebuild reporting, per-root validations, CLI | Done 2026-10-07 | Landed as planned. `repository_map.rs`: `pub enum MapLoad { Reused, Built, Rebuilt { reason } }`, `pub enum RebuildReason { Corrupt, SchemaMismatch { found }, Stale }` (`as_str`: `corrupt`/`schemaMismatch`/`stale`), `pub struct RepositoryMapStore` with `new(data_dir, AuditLog)`, `path(data_dir, repository_id)` (`<data_dir>/repository-map/<id>.json`) and `load_or_build(index, policy)`, plus `RepositoryMap::to_json`, all re-exported. Reuse is decided on the fingerprint alone: private `input_fingerprint` runs detection and overrides only (no command detection, no manifest read), and `detect_with_overrides` is now shared with `build`. The stored file is read as JSON, its `schemaVersion` is checked before its shape (so another version is `SchemaMismatch`, not `Corrupt`), and a map with another `repositoryId` is `Corrupt`. Writes go to a uniquely named `.<id>.json.<id>.tmp` in the same directory, then a rename. A rebuild audits `repository_map_rebuilt` with `repositoryId`, `reason` and, for a mismatch, `foundSchemaVersion`. `Built` (no file) and `Reused` are not audited. `workspace_engine.rs`: `repository_map(root)` through `IndexCache::get_or_build`, with the store built per call (no new engine field). `validation.rs`: `propose_detected_validations(repository_root, &RepositoryMap)` skips `user_override == Some(Removed)` and proposes each command with `propose_command_at(repository_root, root dir, ..)`. It classifies again rather than trusting the map's stored risk, so a stale map can never widen approval. Reason text unchanged. `foundation.rs`' caller goes through `engine.repository_map`. CLI: `damaian repo-map <repo> [--json]` (summary: outcome line, one line per root, excluded paths; `--json`: `{"load":{..},"map":{..}}`), `propose-validations` per root, `detect-commands` untouched. **The Task 4 gap, closed by hashing:** the fingerprint now also hashes `command_allowlist` and `command_blocklist`, in config order. Hashing was chosen over rebuilding on a config change because there is no config-change signal to hook. Config is loaded each time an engine is constructed (CLI, shell), so detecting a change would mean storing the previous config, which is what a hash already does. The blocklist was added because it changes risk too (a matching command is `Blocked`). `restricted_patterns` is deliberately **not** hashed: its only effect on the map is whether an override applies, and a rejected override is an `invalidOverride` entry in `excluded`, which Task 3 already hashes. So that half of Task 4's note was not a real gap. The mutation proved it: dropping a `restricted_patterns` hash line left all 40 tests passing, so the line was removed, and `a_new_restricted_pattern_makes_the_stored_map_stale` stays as the pin. `tests/repository_map.rs`: 9 new tests, 40 in total, all pass. They were written first and failed to compile on the missing `MapLoad`/`RebuildReason`/`RepositoryMapStore`, `WorkspaceEngine::repository_map` and the two-argument `propose_detected_validations`. `foundation`: 158 pass. `repository_config_trust` and `permission_profiles`: 97 pass, 1 skipped (nextest flags 1 as leaky), both files untouched. Mutations, each reverted and confirmed failing: (1) reuse without the fingerprint check fails the manifest-stale, allowlist and restricted tests; (2) a schema mismatch treated as reuse fails the schema test; (3) proposals at the repository root fail the per-root proposal test and the removed-root test; (4) no allowlist/blocklist in the fingerprint (the gap) fails `a_new_allowlist_grant_makes_the_stored_map_stale` (`Reused`, not `Stale`), and dropping only the blocklist line fails its blocklist half; (5) removed roots not skipped fails `a_removed_root_yields_no_proposal`, whose hand-marked map keeps its commands, so the field is what decides. **Measured** on this repository, index build excluded, `DAMAIAN_DATA_DIR` in a scratch directory, 5 rounds: 277 indexed files, **7 roots**, a 6.7 KB file. `Built` takes about 1.5 ms debug and 0.38 ms release. `Reused` takes about 1.0 ms debug and 0.17 ms release. A reuse still runs detection and the fingerprint over the index and parses the file, which is why it costs half a build. The on-demand design (`context.md` §3) needs no revisit. Smoke-tested by hand against this checkout with a scratch data dir: built → reused → garbage file → `rebuilt: unreadable` with one audit event, and `--json` parses. **Deviations:** `load_or_build` takes `(index, policy)`, not `(index, policy, config)`: since Task 4, `build` reads its config from `policy.config()`, and a separate config could disagree with the policy. `RepositoryMap::to_json` was added so the CLI needs no `serde_json` dependency. A write or audit failure fails the load rather than returning an unsaved map. **For Task 6:** call `WorkspaceEngine::repository_map` (or the store) per turn; a reuse is about 0.2 ms release. If a read-only data dir must not fail a turn, catch the error at the call site rather than in the store. On this repository a Cargo workspace root and each member all propose `cargo test`, so `propose_detected_validations` yields 7 `cargo test` proposals. The rendering may want to show the workspace's command once, and spec 23 should know that per-root proposals can overlap. The root's evidence reads `package.json`, not `Cargo.toml`, because of `PROJECT_MANIFESTS` order, and its `npm run lint` is missed because the script is named `lint:web`. Both are Task 1/3 behaviour, unchanged here |
| 6 · Bounded rendering and the `repository_map` context item | Done 2026-10-07 | Landed as planned. `config.rs`: `repository_map_max_tokens: usize` (default 800, overlay `Option<usize>`, `0` accepted and turns the item off), classed **Capability** in `classify_overlay_fields!`, merged in `apply_overlay_scoped`'s restrict-only loop beside `max_read_lines`, and handled in `set` and both `to_policy_text`s. `profile.rs`: `split_profile_keys` carries it like the other read caps, and `equals_base_limit` treats an equal value as not loosening. `effective_policy.rs`: `rule_label` "Tokens the repository map may use per turn". `repository_map.rs`: `RepositoryMap::render_for_model(max_tokens) -> RenderedMap { text, tokens, roots_shown, roots_total, dropped }` and `pub enum Degradation { EntryPointsAndTestPaths { root }, MajorDirectories, GeneratedPathsCounted, Roots { omitted } }`, both re-exported. `context_manager.rs`: `pub struct RepositoryMapSource { store, command_policy, max_tokens }` (re-exported) passed to `ContextManager::new`. `build_context` adds one `repository_map` item, `path: None`, after `project_rule` and before `retrieved_file`, when an index is present and the ceiling is non-zero, through `add_text`, so it is secret-scanned and counted against the flat budget like every item. A load or write error leaves the item out (`repository_map_text` returns `None`) and never fails the turn. `workspace_engine.rs` builds the `CommandPolicy` before the `ContextManager` and hands it the engine's own policy and a store on `data_dir`. `chat.rs`, `edit.rs` and `build_model_prompt` are untouched. `tests/repository_map.rs`: 6 new tests, 46 in total, all pass. They were written first and failed to compile on the missing API. `tests/permission_profiles.rs`: one weakening case (`800` → `100000`, `Reported`), nothing else touched. Together with `repository_config_trust` (untouched): 143 pass, 1 skipped. The `workspace-engine` lib tests with `foundation` and `prompt_cache`, which cover both `build_context` callers: 535 pass, 4 skipped. Mutations, each reverted and confirmed failing: (1) no ceiling check fails the 60-root test and the step test; (2) generated paths counted before major directories are dropped fails the step test; (3) the item placed after `retrieved_file` fails `build_context_places_one_map_item_before_retrieved_files`; (4) the key merged as a preference (and classed one) fails the ceiling-config test and five spec 31 partition tests, including `every_capability_key_resists_weakening_from_repository_scope`; (4b) merged as a preference but still classed Capability fails the weakening case `every_capability_key_resists_weakening_from_repository_scope` (the repository's `100000` was applied), the ceiling-config test, and two profile tests; (5, extra) smallest root first fails the step test. **Eval:** the deterministic tier passes all 16 scenarios. Input tokens summed over the scenarios went from 101,787 in `evals/baseline.json` to 104,747 (+2,960, +2.9%). Per scenario the rise is about 75 tokens per model call (+74 to +75 for single-call scenarios, +666 for `failed_validation_retry`), because the map sits in the first user message, which every call resends. A pre-change run matched the baseline except `batched_reads` (4,798, not 4,811), so that scenario's +136 is +149 over today. `evals/baseline.json` was not regenerated. **On this repository** the map renders whole at 465 tokens (7 roots). At 300 it drops entry points, test paths and major directories, and at 150 it shows 3 of 7 roots. **Decisions the plan left open:** "largest root" means the most bytes of entry points and test paths, the thing step 1 removes, ties by path. Step 4 drops deepest first, then from the end of path order, so what stays is a prefix of each depth. When not even the repository root fits, the text is empty and the item is left out, because the ceiling is a bound, not a target. Manifests are rendered by file name (they sit in the root's own directory). Other paths stay repository-relative, as the tools take them. Test directories are not repeated under `directories` while the `tests` line is shown. Only a root's own `AGENTS.md` is listed under `instructions`: an enclosing root lists the files above it, and spec 11 loads them for any path in context. Commands render as the command only, without name or risk. **Duplicate commands are not collapsed:** the Cargo workspace root and its six members each show `cargo test`, about 28 of the 465 tokens. The string is the same but the scope is not: in a member it tests that crate only, a cheaper check than the workspace's. Collapsing would make the members look like they have no commands, or need a notation the model must decode. **Deviation:** `ContextManager::new` takes one `RepositoryMapSource` instead of three loose arguments, so the constructor stays under clippy's argument limit and the three travel together. No audit event is written when the map is left out. **For Task 7:** the rendered header says "A root's commands belong to its directory", but until Task 7 adds `working_directory` to `run_command` the model cannot ask for a directory. Task 7 should make the header say how to run a command in a root, and re-run the eval tier. The map is built per turn from `build_context`'s `index`, so it adds no index build. Spec 26 absorbs the item as `ContextCategory::RepositoryMap` |
| 7 · Commands and findings in a sub-root, in the chat loop | Done 2026-10-08 | Landed as planned. `validation.rs`: `pub fn resolve_command_directory(path_policy, repository_root, requested) -> Result<PathBuf>` — the empty request is `repository_root` unchanged, so a missing argument behaves exactly as before; otherwise it goes through `resolve_existing(.., allow_outside_root: false)`, requires a directory, checks `is_restricted`, and returns `repository_root.join(relative_path)` rather than the canonical absolute, so `classify_at`'s lexical `relative_location` compares like with like (`/var` versus `/private/var` on macOS). Re-exported from `lib.rs`. `finding.rs`: `pub fn resolve_finding_path(repository_root, working_directory, printed) -> Option<String>` tries the working directory first, then each ancestor up to and including the repository root, canonicalising both, rejecting empty and absolute paths; `Finding::with_range_path` (`pub(crate)`) rewrites the range's path. `chat.rs` (**the only task that edits it**): `CommandRequest.working_directory: Option<String>`; the `run_command` schema gains an optional `working_directory`; `command_request_from_tool_call` and `parse_command_request` read the tool argument and `WORKING_DIRECTORY:` (PendingChatTurn persists no `CommandRequest`, so no serde default was needed); new `ChatOrchestrator::command_working_directory`; `action_permission` classifies with `classify_command_at(repository_root, resolved)`; the dispatch loop resolves once into `refused_directory` and, on failure, returns `ActionOutcome::Failed` with the `ClientError` text before anything is proposed; the Command arm calls `propose_command_at`; the resume path calls `classify_command_at(proposal.repository_root, proposal.working_directory)`; `record_findings` takes the execution's working directory and resolves each range through `resolve_finding_path`, rewriting the path. `repository_map.rs`: the rendered header now tells the model to pass `working_directory` to `run_command` (or `WORKING_DIRECTORY:` in an envelope). `mode.rs`: `working_directory: None` added to its `CommandRequest` literals. `build_model_prompt` untouched. `tests/repository_map.rs`: 6 new tests (52 in total), written first and confirmed failing to compile on the missing `resolve_finding_path`. Mutations, each reverted and confirmed failing: (1) `allow_outside_root: true` in `resolve_command_directory` fails `a_refused_working_directory_never_stores_a_proposal` (the `..` case ran); (2) the resume classifying at `repository_root` fails `a_resumed_proposal_is_re_classified_at_its_own_directory`; (3) `resolve_finding_path` resolving from the root only fails the npm case and the recorded-range test; (4) ancestors not walked fails the cargo case. **Eval:** the deterministic tier passes all 16 scenarios; input tokens summed over the scenarios went 101,787 (`evals/baseline.json`) → 107,169, and Task 6 recorded 104,747, so the schema and header add **+2,422** (+2.31%); output stays 176. `evals/baseline.json` was not regenerated. **Checks:** `fmt --check` clean, scoped `clippy -p workspace-engine --all-targets --locked -- -D warnings` clean, `typos` clean, and `cargo nextest run -p workspace-engine` over the lib tests plus `repository_map`, `foundation`, `prompt_cache`, `permission_profiles` and `repository_config_trust`: 684 pass, 5 skipped. Spec 20's mode tests and spec 31's `permission_profiles` pass unmodified, and `repository_config_trust` is untouched. **Deviation:** the recorded-range test runs its rustc-shaped fixture as `sh ./cargo check` (a non-executable script read by the system `sh`) instead of launching a freshly written executable, because macOS XProtect can stall a new binary for minutes (OBSERVATIONS.md row 25; the first run showed a 350s stall that also starved the foundation watcher test). **For the spec 56 and spec 57 rebase:** the `chat.rs` functions changed are `command_request_from_tool_call`, `parse_command_request`, `run_command_tool_definition`, the new `ChatOrchestrator::command_working_directory`, `action_permission`, the Command arm and `refused_directory` block inside `run_agentic_turn`, `resume_after_command_decision`, `record_command_findings`, `record_web_findings`, and `record_findings`, plus the `CommandRequest` struct. **For Task 8:** the approval card is built by `agent_command_proposal(config, &CommandProposal)` into `AgentCommandProposal { id, command, prompt, risk, requires_approval, blocked, allow_always, allow_browser_diagnostics_for_session }`. The card can show "Runs in …" by comparing the `CommandProposal.working_directory` it already has with `repository_root` (both stored and served); `working_directory` is where the command runs, `repository_root` is the boundary. |

| 8 · Shell API and UI | Done 2026-10-08 | Landed as planned, plus one extra read endpoint the plan did not name. `crates/desktop-shell/src/lib.rs`: `GET /api/repository-map` (`handle_repository_map`) returns `{"load":{...},"map":{...}}`, the same shape as `damaian repo-map --json`, via `map_load_json` and `RepositoryMap::to_json`; `POST /api/repository-roots` (`handle_repository_roots`) requires `repo`, `action` (add/remove/clear) and `path`, refuses a repo that is not a directory, edits `<repo>/.damaian/config.conf` through `ConfigOverlay::load_or_default` + `edit_root_overrides` + `save` (the same function the Task 4 CLI uses), rebuilds the map and answers `{"configPath","load","map"}`. Both are `handle_*` functions, not inline arms (observation 26). Before writing, the POST checks the repository against `allowed_roots` through `PathPolicy::canonical_root` (`repository_config_write_target`), because indexing does not consult `allowed_roots` and the write would otherwise land in any directory named; the map in the answer comes from a fresh engine, so it reflects the config just written. **Extra endpoint:** `GET /api/command-proposal` (`handle_command_proposal`) returns a stored proposal's `workingDirectory` and `repositoryRoot`. `AgentCommandProposal` (chat.rs) carries neither and Task 8 must not edit chat.rs, so the approval card looks the proposal up by id through the shell's `CommandStore` — the correction to Task 7's note. UI: a **Repository roots** section in Settings › General (`index.html`), rendered by `renderRepositoryRoots`/`renderRepositoryRootDetail`: each root shows its path (`.` for `""`), languages, command count and `detected` / `added by you` / `removed by you`; the detail shows the evidence sentence, instruction files and commands with their working directories; Add uses `promptDialog`; Remove/Undo confirm first and the section states the write goes to `.damaian/config.conf` in the repository, which Git shows. `app.js` gains a `repositoryRoots` cache, `rootForPath` (longest path-segment prefix, the same rule as `RepositoryMap::root_for_path`), `repositoryRelative`, `applyRepositoryMap` and `writeRootOverride`, refreshed on repository switch and on opening Settings. A pinned chip whose file is in a sub-root reads `index.ts · packages/api`; each patch file header gains a `.diff-root` tag; the command approval card fetches `/api/command-proposal` and shows "Runs in `packages/api`" when the working directory is not the repository root. `style.css` gains the roots, `.command-approval-location` and `.diff-root` rules, and `docs/ui-style-guide.html` gains a Roots specimen, a diff-card root specimen and the "Runs in" line. `tests`: 6 new shell tests, including `a_root_override_is_never_written_outside_the_allowed_roots` (removing the `canonical_root` call fails it, confirmed and reverted), and in `lib.rs` — the map/load JSON shape, an add written and re-read, an invalid path reported as `invalidOverride` without becoming a root, a foreign (non-directory) repository refused, and the proposal-directories endpoint. They were written first and confirmed failing (404/500) before the handlers existed. `cargo nextest run -p desktop-shell` with `RUST_MIN_STACK=67108864`: 98 pass, 4 skipped (92 before). **Verified in the running app** on port 4899 with the ignored `serves_the_ui_for_manual_inspection` server (its own scratch data dir), driven headlessly through Chrome's DevTools protocol against a scratch Git monorepo: the roots list contains `packages/api`, `packages/web` and an added `tools/scripts`; the two pinned chips read `index.ts · packages/api` and `index.ts · packages/web`; a staged `packages/api` proposal's card reads "Runs in packages/api"; a two-file patch shows root tags `packages/api` and `packages/web`; and the added override reads `added by you` after a fresh map load, with `.damaian/config.conf` written and visible in `git status` (server and Chrome stopped by PID only). **Deviations:** (1) the third read endpoint above, since chat.rs is out of scope; (2) the shell reaches `Config::load_for_repository`, which has no overlay key for `enable_index_watcher`, so the endpoint tests index a tiny fixture with the watcher on (off-thread, no measurable cost); (3) the scoped suite was run with `RUST_MIN_STACK=67108864`. At the default 2 MB test-thread stack, 20 of 92 `desktop-shell` tests already abort with SIGABRT on `main` before this task (not one, as first noted), and the 5 Task 8 endpoint tests join them (25 of 98). That is pre-existing, recorded in `OBSERVATIONS.md`, and not fixed here. `fmt --check`, scoped `clippy -p desktop-shell --all-targets --locked -- -D warnings`, `node --check`, `npm run lint:web` (0 errors; one pre-existing info in `scripts/specs-check.mjs`) and `typos` are clean. **For Task 9:** the user guide should describe the Repository roots list and detail, that add/remove/undo write to `.damaian/config.conf` (which Git shows), what an `invalidOverride` excluded entry means, `repository_map_max_tokens` turning the map off, and where a command runs. |
| 9 · Docs, acceptance criteria, close the spec | In progress 2026-10-09: **blocked, the spec is not closed** | **Gate**, full run in a fresh worktree from `main`, with `RUST_MIN_STACK` unset and nothing in config raising it. `cargo fmt --all -- --check` pass; `cargo clippy --workspace --all-targets --locked -- -D warnings` pass; `node --check crates/desktop-shell/static/app.js` pass; `npm run lint:web` pass; `typos` pass; `cargo deny check` pass. **`cargo nextest run --workspace --locked` fails:** 1118 run, 1092 pass, 26 fail, 24 skipped, 121 s wall. Of the 26, **25 are `desktop-shell` tests aborting with SIGABRT** at the default 2 MiB test-thread stack (`OBSERVATIONS.md` #26, pre-existing on `main`, being fixed in a separate session). They include Task 8's 5 endpoint tests (`repository_map_endpoint_…`, `repository_roots_endpoint_…` ×3, `command_proposal_endpoint_…`). As the task instruction says, no stack setting was raised to make them pass. **The 26th is a regression this spec caused:** `token_ceiling::a_turn_stops_before_the_call_that_would_cross_the_ceiling` expects 3 model calls under a 1,000-token ceiling and now gets 2. The `repository_map` item and the longer `run_command` schema make the estimate cross 1,000 one call sooner. This was confirmed by setting `repository_map_max_tokens: 0` in that test's config: all 13 `token_ceiling` tests then pass. That change was reverted, so the test file is unchanged. No scoped run in Tasks 6–7 included this file. **Fixed, at the user's go-ahead:** `engine_with_ceiling` in `tests/token_ceiling.rs` now sets `repository_map_max_tokens: 0`, with a comment saying why, so the exact count of 3 still holds. All 13 `token_ceiling` tests pass with `RUST_MIN_STACK` unset. Without that line the test fails with `left: 2, right: 3`, as the gate run showed. The test was not changed to expect 2 (proposal §7.3 explains why). `npm run specs:check` passed: 57 specs, dependency lines agree. **Eval** (`cargo run -p eval-harness -- run --tier deterministic`): all 16 scenarios pass. Tokens are 107,345 against `evals/baseline.json`'s 101,963, a rise of **+5,382**, exactly Task 6's +2,960 plus Task 7's +2,422. That total is 107,169 input, as Task 7 recorded, plus 176 output, unchanged. Every rate metric equals the baseline: completion 1.0, check pass 0.111, error rate 0.273, 0 violations, 2.5 calls per task. Latency is 80 ms median and 133 ms p90, against the baseline's 44 and 109. That is wall-clock in a run that started straight after the workspace suite, and the map costs about 0.2 ms per reuse (Task 5), so the rise is not attributed to this spec, but it was not re-measured on an idle machine. The baseline was not regenerated. **Docs.** `docs/USER_GUIDE.md` gains a **Repository Roots** section. It covers: what a root is and the seven manifests; that `.gitignore`d, `ignore_patterns` and vendor directories are never roots; the depth ceiling; which `package.json` scripts count; Settings › General › Repository roots with add, remove and undo, which write `project_roots_added`/`project_roots_removed` to `<repo>/.damaian/config.conf`, which Git shows; that the keys are also accepted in `user.conf`; `invalidOverride`; the "Runs in …" line and where a command runs; finding paths resolved from the working directory; chip and patch root tags; Allow Always qualified per root, while a plain `command_allowlist` entry applies at the repository root only; and the map in context, bounded by `repository_map_max_tokens` (800; `0` turns it off; a repository can lower but not raise it). The guide's Settings list of repository-config keys now names the two override keys (preferences) and the map ceiling (lower-only). `docs/TROUBLESHOOTING.md` gains a `repository-map/<repo-id>.json` row in the data-directory table and a **Repository roots and the repository map** section. That section covers: the map file and its fingerprint inputs; forcing a rebuild by deleting it; `damaian repo-map <repo> [--json]` and `repo-root`; reading `detectedBy` and `excluded` (`vendor`, `belowDepthCeiling`, `invalidOverride`); a missed root, a wrong root, a missing command and the wrong directory; the `repository_map_rebuilt` audit event, with a `jq` line; and what the model saw. Troubleshooting also gains the cwd-first finding-path rule under Findings, and two lines under CLI reproduction. Every claim was checked against the code, not the plan. That check found that scripts match by exact name, not by substring as `context.md` §15 puts it. `proposal.md` §7 is written: the §6-to-test mapping (§7.1); the two requirements not built (frameworks, per-root permission restrictions), marked not met; the recording answers (§7.2); and the `token_ceiling` regression (§7.3). **Fresh measurement** on this repository: 7 roots, a 6,710-byte map file, `node_modules` and `target` excluded. One surprising root not recorded before: `crates/eval-harness/fixtures/rust-workspace`, a test fixture with its own `Cargo.toml`. **Left undone because the gate is red**, as the task prompt says: every status record stays In progress (`proposal.md`, the README row, this table, and no `Done:` date in the header); the `Depends on:` lines in #25 and #26; README "What to build next"; and the CHANGELOG `Unreleased` bullet. **Done in this task anyway**, because neither depends on the spec's status: spec 23's references to `propose_detected_validations` now cite `validation.rs:273` (was `:167`, twice), and its §5.2 sentence describes the two-argument, per-root signature and the overlapping per-root proposals. Spec 23's other stale line numbers were left alone. The `OBSERVATIONS.md` notes on #31 and #32 were written but not applied: `docs/PLAN/` is local-only, absent from this worktree, and the session could not write the main checkout. **To close:** (1) the #26 stack fix, in progress in another session, lands on `main`; (2) rebase or merge this work onto it; (3) re-run `cargo nextest run --workspace --locked` with `RUST_MIN_STACK` unset, and expect 0 failures; (4) set the four status records to Done, with **Done:** set to that date; (5) mark #24 built in the `Depends on:` lines of #25 (`25_symbol_and_relationship_index.md:8`) and #26 (`26_context_assembly.md:10`); (6) re-derive README "What to build next": remove #24, promote #25, and note that #26 still waits on #25; (7) remove "Map project roots so a command found in one package runs in that package…" from CHANGELOG `Unreleased`; (8) run `npm run specs:check`. |

**Goal:** A deterministic, token-bounded map of a repository's project roots.
Each root carries its languages, manifests, entry points, tests, generated
paths, instruction files and commands. A command discovered in, or requested
for, a sub-root runs there, without changing what needs approval or what
paths are reachable.

**Architecture:** Root detection is a pure function of the index's file and
skip lists (Task 1), so the map needs no second walk and no index change
(`context.md` §3). The command location becomes a pair, the repository root
and the working directory (Task 2). Every boundary check uses the root of the
pair and every execution uses the directory (`context.md` §1–§2).
`RepositoryMap::build` derives the rest from the index (Task 3). Overrides
come from repository config (Task 4), and the result is persisted and rebuilt
on an input fingerprint (Task 5). The model sees the map as one more
`ContextItem` built in `context_manager.rs` (Task 6), so `build_model_prompt`
is never edited. `chat.rs` changes in Task 7 only: the `run_command`
working directory, two classification call sites, and finding paths.

**Tech Stack:** Rust 2024, `serde`/`serde_json` (already dependencies of
`workspace-engine`), the existing `ConfigOverlay` parser, vanilla JS in
`app.js`. No new dependencies.

## Global Constraints

Every task's requirements implicitly include this section.

- **Read [`context.md`](context.md) in full before Task 1.** Several of its
  decisions override the proposal:
  - §1 and §2: the location pair, and root-qualified Allow Always.
  - §3: no `root_path` on `FileRecord`; the map is derived on demand.
  - §5: the map is a `ContextItem`, and `chat.rs` belongs to Task 7.
  - §6 and §7: the override keys are preferences; `repository_map_max_tokens`
    is a lower-wins capability key.
  - §8: finding paths resolve cwd-first, then ancestors.
  - §11 and §12: what `excluded` records, and the two requirements not built.

  Do not re-derive any of these from `proposal.md` alone.
- **A root never widens anything.** It changes the directory an approved
  command runs in. It never changes whether approval is needed, and never
  what paths are reachable (proposal §4, §5.5). Containment is always
  checked against the repository root.
- **Existing behaviour at the repository root is unchanged.** Every existing
  caller of `classify`, `propose_command`, `run` and
  `detect_project_commands` behaves exactly as before. Each task's tests
  include a case at the root that pins this.
- **No task edits `build_model_prompt`.** **Only Task 7 edits `chat.rs`**
  (`context.md` §5). Before starting Task 7, check the `tasks.md` progress
  table of any spec in flight that also edits `chat.rs` (#23, #32, #50, #56,
  #57). If that spec's `chat.rs` task is in progress, wait, or agree which
  track rebases.
- **Spec 34's tests are the floor.** `crates/workspace-engine/tests/repository_config_trust.rs`
  passes unchanged after every task, and no task edits it. Spec 31's
  `tests/permission_profiles.rs` is extended in Tasks 4 and 6 only, as
  `context.md` §6–§7 describe. No existing assertion is weakened.
- **New engine tests go in `crates/workspace-engine/tests/repository_map.rs`**,
  which Task 1 creates. Fixtures that build an engine or an index set
  `enable_index_watcher: false`.
- **Tests never spawn a real login shell.** A test that needs a command to run
  sets `shell` to `/usr/bin/true` and asserts on `CommandExecution`, as spec 31
  Task 5 did (observation 25: freshly written executables stall on this Mac).
- **Falsify every load-bearing test.** Break what it guards, confirm it fails,
  revert, and record the mutation in the progress row. This repository has a
  history of tests that passed without testing anything (spec 47 §7.2,
  spec 21's error rate).
- **Scope per-task checks. Run the full seven-command gate from `AGENTS.md`
  once, in Task 9.** `cargo nextest run --workspace` takes about 5 minutes
  locally, and `cargo clippy --workspace --all-targets` up to 18 minutes cold.
- **Never `git commit` unasked.** Each task ends by showing the change and the
  scoped check results, then asking. When asked, write one subject line with no
  body and no trailer. Never push.

## File Structure

| File | Change |
|---|---|
| `crates/workspace-engine/src/repository_map.rs` | New (Task 1): root detection. `ProjectRoot`, `RootCommand`, `RepositoryMap::build`, `root_for_path` (Task 3). Override application (Task 4). `RepositoryMapStore` (Task 5). `render_for_model` (Task 6) |
| `crates/workspace-engine/src/command_policy.rs` | `PROJECT_MANIFESTS` (Task 1). `classify_at`, root-aware outside-root check and allowlist (Task 2). Shared detection body for per-root commands, `CommandRisk` serde (Task 3) |
| `crates/workspace-engine/src/validation.rs` | `CommandProposal.repository_root`, `propose_command_at`, `classify_command_at`, `allow_command_always` repository id (Task 2). Per-root `propose_detected_validations` (Task 5). `resolve_command_directory` (Task 7) |
| `crates/workspace-engine/src/command_runner.rs` | `run_at` (Task 2) |
| `crates/workspace-engine/src/context_manager.rs` | `agent_instruction_paths` becomes `pub(crate)` (Task 3). The `repository_map` item (Task 6) |
| `crates/workspace-engine/src/config.rs`, `profile.rs`, `effective_policy.rs` | `project_roots_added`/`project_roots_removed` (Task 4). `repository_map_max_tokens` (Task 6) |
| `crates/workspace-engine/src/workspace_engine.rs` | `repository_map()` (Task 5). `ContextManager` gets the map builder (Task 6) |
| `crates/workspace-engine/src/finding.rs` | `resolve_finding_path` (Task 7) |
| `crates/workspace-engine/src/chat.rs` | **Task 7 only:** the `run_command` working directory, two classification call sites, `record_findings` |
| `crates/workspace-engine/src/lib.rs` | Module registration and re-exports (Tasks 1–7) |
| `crates/workspace-engine/tests/repository_map.rs` | New (Task 1). Every later engine task adds to it |
| `crates/workspace-engine/tests/permission_profiles.rs` | New preference cases (Task 4), a weakening case (Task 6) |
| `crates/workspace-engine/tests/foundation.rs` | The `propose_detected_validations` caller (Task 5) |
| `crates/damaian-cli/src/main.rs` | `repo-root` (Task 4). `repo-map`, per-root `propose-validations` (Task 5) |
| `crates/desktop-shell/src/lib.rs` | Map and override endpoints (Task 8) |
| `crates/desktop-shell/static/app.js`, `index.html`, `style.css`, `docs/ui-style-guide.html` | Roots view, approval-card and patch root tags, chip qualification (Task 8) |
| `docs/USER_GUIDE.md`, `docs/TROUBLESHOOTING.md` | Proposal §5.10 (Task 9) |

## Interface reference

Checked on 2026-10-06. Re-check a line before editing near it: other specs
edit these files.

- `CommandRisk` (`command_policy.rs:7`): no serde derives; `as_str` gives
  `low`/`medium`/`high`/`blocked`. `CommandClassification` (`:26`).
  `ProjectCommand { name, command, risk }` (`:50`), `Debug, Clone` only.
- `CommandPolicy::classify(&self, command: &str, working_directory: &Path) -> CommandClassification`
  (`command_policy.rs:73`). `classify_pattern` (`:102`): blocklist (`:105`),
  shell control (`:119`), then allowlist via `configured_exact_matches`
  (`:133`, helper `:300`). `contains_shell_control` (`:304`).
  `references_path_outside_root(command, working_directory)` (`:421`) and
  `token_escapes_root` (`:434`) treat the working directory as the boundary
  (`context.md` §1).
- `CommandPolicy::detect_project_commands(&self, root_path) -> Result<Vec<ProjectCommand>>`
  (`command_policy.rs:215-260`): the `package.json` scripts, then the six-row
  manifest table at `:242-249`.
- `CommandProposal` (`validation.rs:14`), `working_directory: String` at `:17`.
  `propose_command(working_directory, command, reason)` (`:172`),
  `propose_detected_validations(repository_root, &RepositoryMap)` (`:238`, per root since Task 5), `run_proposal`
  (`:218`), `allow_command_always` (`:279`, the repository id from the
  working directory at `:281`), `proposal_from_classification` (`:356`),
  `serialize_proposal` (`:391`), the `WORKING_DIRECTORY` read at `:431`.
- `CommandRunner::run(&self, command, cwd, reason, options) -> Result<CommandExecution>`
  (`command_runner.rs:187`), re-classifying at `:194` and spawning with
  `.current_dir` at `:234`. Its only production caller is `run_proposal`.
- `FileRecord` (`indexer.rs:22`), `SkippedFile { path, reason }` (`:36`),
  `SearchResult` (`:42`), `RepositoryIndex { repository_id, root_path, indexed_at_ms, files, skipped }`
  (`:52`), no serde. `index_repository` (`:152`), `index_single_file` (`:222`).
  `SkippedFile` is reachable as `workspace_engine::indexer::SkippedFile`.
- `tree_walk::walk` (`tree_walk.rs:49`) visits entries sorted, reports ignored
  entries as `Skipped { reason: "ignored" }` without descending, and reports
  no directories.
- `IndexCache::get_or_build(indexer, root) -> Result<RepositoryIndex>`
  (`index_cache.rs:43`) returns a clone. The watcher patches through
  `apply_single_path_change` (`:147`).
- `vector_index_path` (`vector_index.rs:149-153`), `load`/`save` (`:22`/`:29`).
- `detect_language(path: &str) -> &'static str` (`language.rs:3`).
- `ContextItem { kind, path: Option<String>, content, tokens, redaction_status }`
  (`context_manager.rs:22`). `build_context` (`:66`) adds items in the order
  `user_prompt`, `explicit_file`, `agent_instruction` (`:150`), `project_rule`
  (`:165`), `retrieved_file` (`:180`). `add_text` estimates `len.div_ceil(4)`
  (`:265`). `agent_instruction_paths` (`:284-312`), private.
  `ContextManager::new` has one caller (`workspace_engine.rs:66`).
- `build_model_prompt` (`chat.rs:3612`) renders items generically at
  `:3639-3652`. Callers of `build_context`: `chat.rs:753`, `edit.rs:404`.
- `chat.rs`, for Task 7: `record_command_findings` (`:1298`), `record_findings`
  (`:1343`, root join at `:1355`), the resume re-classification (`:953-955`),
  `propose_command` in the loop (`:2312`), `action_permission`'s
  classification (`:3158-3161`), `CommandRequest` (`:3713`),
  `run_command_tool_definition` (`:3993`), `command_request_from_tool_call`
  (`:4421`), `parse_command_request` (`:4440`), `sandbox_command_context`
  (`:4750`).
- `workspace_range` (`finding/rust_diagnostics.rs:136`) keeps the printed path,
  rejecting absolute and `..` paths.
- Config: `DEFAULT_IGNORE_PATTERNS` (`config.rs:15`),
  `DEFAULT_CONTEXT_TOKEN_BUDGET = 16_000` (`:13`), `load_scoped` (`:681`),
  `repository_config_path` (`:768`), `apply_overlay_scoped` (`:786`),
  `apply_repository_allowlist` (`:1771`), `ConfigOverlay` (`:2056`),
  `classify_overlay_fields!` (`:2165`), `preference` (`:3272`), `lower_wins`
  (`:3294`), `split_list` (`:3440`, `|`-separated). `split_profile_keys`
  (`profile.rs:322`).
- `repository_id_for_root(root) -> String` (`hash.rs:113`), `repo_sha256:<9 hex>`.
- `PathPolicy::canonical_root` (`path_policy.rs:41`), `resolve_existing`
  (`:58`), `is_restricted` (`:138`).
- Spec 31's partition tests: `the_preference_keys_are_exactly_spec_34s_free_keys`
  (`tests/permission_profiles.rs:103`), `weakening_cases` (`:148`),
  `every_capability_key_resists_weakening_from_repository_scope` (`:385`),
  `every_preference_key_applies_from_repository_scope` (`:436`).
- CLI: `detect-commands` (`damaian-cli/src/main.rs:185`), `repo-map` (`:285`),
  `propose-validations` (`:300`, per root since Task 5).

---

## Task 1: Root detection from the index's path lists

**Requirements:** §5.2 and §5.3, and acceptance criteria "nested roots are both
detected", "`node_modules`, `vendor`, `target`, `dist`, and `build` never
appear as roots, and appear in `excluded`", "a repository with no manifest
still produces a map with one root", and "roots below the depth ceiling are
recorded in `excluded`". **Files:** create
`crates/workspace-engine/src/repository_map.rs` and
`crates/workspace-engine/tests/repository_map.rs`; modify
`crates/workspace-engine/src/command_policy.rs` (one constant) and
`crates/workspace-engine/src/lib.rs`.

This task stands alone. Detection is a pure function of the index's file
paths and skip list (`context.md` §3), so its tests need no filesystem,
except the one that ties the manifest list to `detect_project_commands`
(`context.md` §10). It changes no behaviour: nothing calls `detect_roots`
except tests yet. It is `pub`, so `clippy -D warnings` raises no dead-code
warning.

**Interfaces:**
- Consumes: `SkippedFile` (`indexer.rs:36`) and
  `CommandPolicy::detect_project_commands`, neither modified.
- Produces, and later tasks rely on these names:
  - `pub const PROJECT_MANIFESTS: [&str; 7]` in `command_policy.rs`, in
    `detect_project_commands`' order.
  - In `repository_map.rs`:
    - `pub const MAX_ROOT_DEPTH: usize = 6`;
    - `pub const VENDOR_DIRECTORIES: [&str; 7]`;
    - `pub enum RootEvidence { RepositoryRoot, Manifest { path: String } }`.
      Task 4 adds `UserOverride`. It serialises as
      `{"kind":"manifest","path":…}`;
    - `pub enum ExclusionReason { Vendor, BelowDepthCeiling }`. Task 4 adds
      `InvalidOverride`;
    - `pub struct ExcludedPath { path, reason }`;
    - `pub struct DetectedRoot { path, detected_by }`;
    - `pub struct RootDetection { roots, excluded }`;
    - `pub fn detect_roots<'a>(files: impl IntoIterator<Item = &'a str>, skipped: &[SkippedFile]) -> RootDetection`.
      Task 3 calls it with `index.files.iter().map(|f| f.path.as_str())` and
      `&index.skipped`.
  - The test file `tests/repository_map.rs`, with its `temp_dir` helper,
    which later tasks extend.

- [x] **Step 1: Write the failing tests**

  Create `crates/workspace-engine/tests/repository_map.rs`:

  ```rust
  //! Repository map and monorepo boundaries, per
  //! `docs/specs/24_repository_map_and_monorepo_boundaries/proposal.md`.
  //!
  //! Task 1 pins root detection. It is a pure function of the index's file
  //! and skip lists (`context.md` §3), so only the test that ties the manifest
  //! list to `detect_project_commands` touches the filesystem.

  use std::fs;
  use std::path::PathBuf;
  use std::sync::atomic::{AtomicU64, Ordering};
  use std::time::{SystemTime, UNIX_EPOCH};
  use workspace_engine::indexer::SkippedFile;
  use workspace_engine::{
      CommandPolicy, Config, DetectedRoot, ExcludedPath, ExclusionReason, MAX_ROOT_DEPTH,
      PROJECT_MANIFESTS, RootDetection, RootEvidence, detect_roots,
  };

  static COUNTER: AtomicU64 = AtomicU64::new(1);

  fn temp_dir(name: &str) -> PathBuf {
      let now = SystemTime::now()
          .duration_since(UNIX_EPOCH)
          .expect("clock should work")
          .as_nanos();
      let dir = std::env::temp_dir().join(format!(
          "damaian-repomap-{name}-{now}-{}-{}",
          std::process::id(),
          COUNTER.fetch_add(1, Ordering::Relaxed)
      ));
      fs::create_dir_all(&dir).expect("temp dir should be created");
      dir
  }

  fn ignored(path: &str) -> SkippedFile {
      SkippedFile {
          path: path.to_string(),
          reason: "ignored".to_string(),
      }
  }

  fn root(path: &str, manifest: &str) -> DetectedRoot {
      DetectedRoot {
          path: path.to_string(),
          detected_by: RootEvidence::Manifest {
              path: manifest.to_string(),
          },
      }
  }

  fn repository_root_without_manifest() -> DetectedRoot {
      DetectedRoot {
          path: String::new(),
          detected_by: RootEvidence::RepositoryRoot,
      }
  }

  fn excluded(path: &str, reason: ExclusionReason) -> ExcludedPath {
      ExcludedPath {
          path: path.to_string(),
          reason,
      }
  }

  /// A Cargo workspace with two members beside two npm packages, plus two
  /// files whose names only contain a manifest's name.
  const MONOREPO: &[&str] = &[
      "Cargo.toml",
      "README.md",
      "crates/cli/Cargo.toml",
      "crates/cli/src/main.rs",
      "crates/engine/Cargo.toml",
      "crates/engine/src/lib.rs",
      "docs/package.json.md",
      "examples/Cargo.toml.orig",
      "packages/api/package.json",
      "packages/api/src/index.ts",
      "packages/web/package.json",
      "packages/web/src/index.ts",
  ];

  #[test]
  fn a_repository_with_no_manifest_has_exactly_one_root() {
      let detection = detect_roots(["README.md", "src/notes.txt"], &[]);
      assert_eq!(
          detection,
          RootDetection {
              roots: vec![repository_root_without_manifest()],
              excluded: vec![],
          }
      );
  }

  #[test]
  fn nested_roots_are_kept_and_each_names_its_manifest() {
      let detection = detect_roots(MONOREPO.iter().copied(), &[]);
      assert_eq!(
          detection.roots,
          vec![
              root("", "Cargo.toml"),
              root("crates/cli", "crates/cli/Cargo.toml"),
              root("crates/engine", "crates/engine/Cargo.toml"),
              root("packages/api", "packages/api/package.json"),
              root("packages/web", "packages/web/package.json"),
          ]
      );
      assert!(detection.excluded.is_empty());
  }

  #[test]
  fn vendor_and_build_output_directories_are_never_roots_and_are_recorded() {
      let files = [
          "package.json",
          // Indexed only because a user removed `node_modules/` from
          // `ignore_patterns`. It still must not become a root (§5.2).
          "node_modules/left-pad/package.json",
          "node_modules/left-pad/index.js",
          "packages/api/node_modules/react/package.json",
          "packages/api/package.json",
      ];
      let skipped = [
          ignored("vendor"),
          ignored("target"),
          ignored("dist"),
          ignored("build"),
          ignored("packages/api/dist"),
          // Ignored, but not a vendor directory: not listed (context.md §11).
          ignored("coverage"),
          ignored(".env.local"),
      ];
      let detection = detect_roots(files, &skipped);
      assert_eq!(
          detection.roots,
          vec![
              root("", "package.json"),
              root("packages/api", "packages/api/package.json"),
          ]
      );
      use ExclusionReason::Vendor;
      assert_eq!(
          detection.excluded,
          vec![
              excluded("build", Vendor),
              excluded("dist", Vendor),
              excluded("node_modules", Vendor),
              excluded("packages/api/dist", Vendor),
              excluded("packages/api/node_modules", Vendor),
              excluded("target", Vendor),
              excluded("vendor", Vendor),
          ]
      );
  }

  #[test]
  fn roots_below_the_depth_ceiling_are_recorded_not_dropped() {
      assert_eq!(MAX_ROOT_DEPTH, 6, "context.md §11 records this value");
      let detection = detect_roots(
          [
              "a/b/c/d/e/f/package.json",
              "a/b/c/d/e/f/g/package.json",
              "a/b/c/d/e/f/g/h/Cargo.toml",
          ],
          &[],
      );
      assert_eq!(
          detection.roots,
          vec![
              repository_root_without_manifest(),
              root("a/b/c/d/e/f", "a/b/c/d/e/f/package.json"),
          ]
      );
      use ExclusionReason::BelowDepthCeiling;
      assert_eq!(
          detection.excluded,
          vec![
              excluded("a/b/c/d/e/f/g", BelowDepthCeiling),
              excluded("a/b/c/d/e/f/g/h", BelowDepthCeiling),
          ]
      );
  }

  #[test]
  fn detection_does_not_depend_on_input_order() {
      // The repository root holds two manifests. Its evidence is the first in
      // `PROJECT_MANIFESTS` order, whichever one the walk met first.
      let forward: Vec<&str> = [&["package.json"][..], MONOREPO].concat();
      let mut backward = forward.clone();
      backward.reverse();

      let a = detect_roots(forward.iter().copied(), &[ignored("target"), ignored("dist")]);
      let b = detect_roots(backward.iter().copied(), &[ignored("dist"), ignored("target")]);

      assert_eq!(a, b);
      assert_eq!(a.roots[0], root("", "package.json"));
      assert_eq!(
          serde_json::to_string(&a).unwrap(),
          serde_json::to_string(&b).unwrap()
      );
  }

  #[test]
  fn detection_serialises_in_the_shape_the_map_file_and_the_ui_read() {
      let value = serde_json::to_value(root("packages/api", "packages/api/package.json")).unwrap();
      assert_eq!(
          value,
          serde_json::json!({
              "path": "packages/api",
              "detectedBy": { "kind": "manifest", "path": "packages/api/package.json" }
          })
      );
      let value = serde_json::to_value(excluded("a/b/c/d/e/f/g", ExclusionReason::BelowDepthCeiling))
          .unwrap();
      assert_eq!(
          value,
          serde_json::json!({ "path": "a/b/c/d/e/f/g", "reason": "belowDepthCeiling" })
      );
  }

  #[test]
  fn every_root_manifest_gives_its_directory_a_command() {
      for manifest in PROJECT_MANIFESTS {
          let dir = temp_dir(manifest);
          let content = if manifest == "package.json" {
              "{\"scripts\":{\"test\":\"node test.js\"}}"
          } else {
              ""
          };
          fs::write(dir.join(manifest), content).unwrap();
          let policy = CommandPolicy::new(Config {
              data_dir: dir.join(".damaian"),
              enable_index_watcher: false,
              ..Config::default()
          });
          let commands = policy
              .detect_project_commands(&dir)
              .expect("detection should read the fixture");
          let _ = fs::remove_dir_all(&dir);
          assert!(
              !commands.is_empty(),
              "{manifest} makes a directory a root but gives it nothing to run (context.md §10)"
          );

          let path = format!("pkg/{manifest}");
          let detection = detect_roots([path.as_str()], &[]);
          assert_eq!(detection.roots.last(), Some(&root("pkg", &path)));
      }
  }
  ```

- [x] **Step 2: Run the tests and confirm they fail**

  Run: `cargo nextest run -p workspace-engine --test repository_map`

  Expected: a compile error, because `detect_roots`, `PROJECT_MANIFESTS` and
  the other names are not in `workspace_engine`.

- [x] **Step 3: Write the implementation**

  In `crates/workspace-engine/src/command_policy.rs`, above
  `impl CommandPolicy` (`:61`), add:

  ```rust
  /// Every manifest that gives a directory project commands, in the order
  /// `detect_project_commands` checks them. Root detection (spec 24) reads this
  /// list, so a directory is a root exactly when it has something to run. A
  /// test ties the two together (`every_root_manifest_gives_its_directory_a_command`).
  pub const PROJECT_MANIFESTS: [&str; 7] = [
      "package.json",
      "pyproject.toml",
      "pytest.ini",
      "pom.xml",
      "build.gradle",
      "go.mod",
      "Cargo.toml",
  ];
  ```

  Leave `detect_project_commands` itself unchanged. Task 3 shares its body.

  Create `crates/workspace-engine/src/repository_map.rs`:

  ```rust
  //! The repository map (spec 24): where a repository's project roots are,
  //! and what each one holds.
  //!
  //! Detection reads the index rather than walking again, so it honours
  //! exactly the ignore, size and symlink rules the index does, and it is a
  //! pure function of the index's paths (`context.md` §3).

  use crate::command_policy::PROJECT_MANIFESTS;
  use crate::indexer::SkippedFile;
  use serde::{Deserialize, Serialize};
  use std::collections::{BTreeMap, BTreeSet};

  /// Deepest directory, in path segments, that can be a root. `""` is depth 0
  /// and `packages/api` is depth 2. A manifest deeper than this is recorded in
  /// `excluded`, not silently dropped (§5.2).
  pub const MAX_ROOT_DEPTH: usize = 6;

  /// Directories that are never roots, whatever `ignore_patterns` says (§5.2).
  /// Independent of the ignore list on purpose: a user may index
  /// `node_modules`, but its packages are still not the user's projects.
  pub const VENDOR_DIRECTORIES: [&str; 7] = [
      "node_modules",
      "vendor",
      "target",
      "dist",
      "build",
      ".venv",
      "venv",
  ];

  /// Why a directory is a root. The UI turns this into a sentence the user can
  /// disagree with (requirement 7).
  #[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
  #[serde(tag = "kind", rename_all = "camelCase")]
  pub enum RootEvidence {
      /// The repository root, which is a root even with no manifest.
      RepositoryRoot,
      /// This manifest, repository-relative, exists.
      Manifest { path: String },
  }

  #[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
  #[serde(rename_all = "camelCase")]
  pub enum ExclusionReason {
      Vendor,
      BelowDepthCeiling,
  }

  #[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
  #[serde(rename_all = "camelCase")]
  pub struct ExcludedPath {
      pub path: String,
      pub reason: ExclusionReason,
  }

  #[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
  #[serde(rename_all = "camelCase")]
  pub struct DetectedRoot {
      /// Repository-relative. `""` is the repository root.
      pub path: String,
      pub detected_by: RootEvidence,
  }

  #[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
  #[serde(rename_all = "camelCase")]
  pub struct RootDetection {
      /// Sorted by path, so `""` is first.
      pub roots: Vec<DetectedRoot>,
      /// Sorted by path, then reason.
      pub excluded: Vec<ExcludedPath>,
  }

  /// Finds the project roots among an index's files.
  ///
  /// A directory is a root when it holds one of [`PROJECT_MANIFESTS`]. Nested
  /// roots are all kept: a Cargo workspace and its members are each right for
  /// their own commands (§5.2). The output does not depend on input order:
  /// both collections are sorted, and a directory's evidence is its first
  /// manifest in `PROJECT_MANIFESTS` order (§5.3).
  pub fn detect_roots<'a>(
      files: impl IntoIterator<Item = &'a str>,
      skipped: &[SkippedFile],
  ) -> RootDetection {
      // directory -> (rank in PROJECT_MANIFESTS, manifest path)
      let mut found: BTreeMap<String, (usize, String)> = BTreeMap::new();
      let mut excluded = BTreeSet::new();

      for path in files {
          let (directory, file_name) = path.rsplit_once('/').unwrap_or(("", path));
          let Some(rank) = PROJECT_MANIFESTS
              .iter()
              .position(|manifest| *manifest == file_name)
          else {
              continue;
          };
          if let Some(vendor) = vendor_prefix(directory) {
              excluded.insert(ExcludedPath {
                  path: vendor,
                  reason: ExclusionReason::Vendor,
              });
              continue;
          }
          if depth(directory) > MAX_ROOT_DEPTH {
              excluded.insert(ExcludedPath {
                  path: directory.to_string(),
                  reason: ExclusionReason::BelowDepthCeiling,
              });
              continue;
          }
          let candidate = (rank, path.to_string());
          found
              .entry(directory.to_string())
              .and_modify(|current| {
                  if candidate < *current {
                      *current = candidate.clone();
                  }
              })
              .or_insert(candidate);
      }

      // Only vendor directories are listed. Other ignored entries were never
      // candidates, and listing them would grow with the repository
      // (context.md §11).
      for skip in skipped {
          let last = skip.path.rsplit('/').next().unwrap_or(&skip.path);
          if skip.reason == "ignored" && VENDOR_DIRECTORIES.contains(&last) {
              excluded.insert(ExcludedPath {
                  path: skip.path.clone(),
                  reason: ExclusionReason::Vendor,
              });
          }
      }

      let mut roots: Vec<DetectedRoot> = found
          .into_iter()
          .map(|(path, (_, manifest))| DetectedRoot {
              path,
              detected_by: RootEvidence::Manifest { path: manifest },
          })
          .collect();
      if roots.first().is_none_or(|first| !first.path.is_empty()) {
          roots.insert(
              0,
              DetectedRoot {
                  path: String::new(),
                  detected_by: RootEvidence::RepositoryRoot,
              },
          );
      }

      RootDetection {
          roots,
          excluded: excluded.into_iter().collect(),
      }
  }

  fn depth(directory: &str) -> usize {
      if directory.is_empty() {
          0
      } else {
          directory.split('/').count()
      }
  }

  /// The path up to and including the first vendor segment, if any.
  fn vendor_prefix(directory: &str) -> Option<String> {
      if directory.is_empty() {
          return None;
      }
      let segments: Vec<&str> = directory.split('/').collect();
      let index = segments
          .iter()
          .position(|segment| VENDOR_DIRECTORIES.contains(segment))?;
      Some(segments[..=index].join("/"))
  }
  ```

  `BTreeMap` iterates in byte order of the path, so `""` sorts first. The
  `roots.first()` check therefore finds an existing repository-root entry.

  In `crates/workspace-engine/src/lib.rs`:
  - add `pub mod repository_map;` in alphabetical order (after `render`);
  - add `PROJECT_MANIFESTS` to the `pub use command_policy::{ … }` list;
  - add `pub use repository_map::{DetectedRoot, ExcludedPath, ExclusionReason, MAX_ROOT_DEPTH, RootDetection, RootEvidence, VENDOR_DIRECTORIES, detect_roots};`.

  Then run `cargo fmt --all` so the lists are in `rustfmt` order.

- [x] **Step 4: Run the tests and confirm they pass**

  Run: `cargo nextest run -p workspace-engine --test repository_map`

  Expected: 7 tests pass. If a `vendor` or ordering assertion fails, check
  the expected vector's byte order before changing the implementation.
  `ExcludedPath` sorts by path, then reason. Do not weaken an assertion to
  make it pass.

- [x] **Step 5: Mutation-test the load-bearing guarantees**

  Do each one, confirm it fails as described, revert it, and record all six
  in the progress row:

  1. `depth(directory) >= MAX_ROOT_DEPTH`. Expected:
     `roots_below_the_depth_ceiling_are_recorded_not_dropped` fails, because
     `a/b/c/d/e/f` is excluded.
  2. Delete the `vendor_prefix` check in the files loop. Expected:
     `vendor_and_build_output_directories_are_never_roots_and_are_recorded`
     fails, because `node_modules/left-pad` becomes a root.
  3. Replace `.and_modify(..).or_insert(candidate)` with
     `.or_insert(candidate)` (first seen wins). Expected:
     `detection_does_not_depend_on_input_order` fails, because the backward
     run names `Cargo.toml`.
  4. Add `"setup.py"` to `PROJECT_MANIFESTS` (size 8). Expected:
     `every_root_manifest_gives_its_directory_a_command` fails for
     `setup.py`. That is `context.md` §10's drift guard.
  5. Delete the repository-root insertion. Expected:
     `a_repository_with_no_manifest_has_exactly_one_root` and the depth test
     fail.
  6. Match with `file_name.contains(manifest)` instead of `==`. Expected:
     `nested_roots_are_kept_and_each_names_its_manifest` fails on
     `docs` and `examples`.

- [x] **Step 6: Confirm nothing else moved**

  Run: `cargo nextest run -p workspace-engine --test repository_config_trust --test permission_profiles`

  Expected: all pass. Neither file is modified (`git diff --stat`).

- [x] **Step 7: Scoped checks**

  ```bash
  cargo fmt --all -- --check
  cargo clippy -p workspace-engine --all-targets --locked -- -D warnings
  typos
  ```

- [x] **Step 8: Update this file's Task 1 row, then show the change and the
  check results and ask before committing**

  In the Notes, record what landed, the test count, the six mutations, and
  any deviation. Suggested subject: `Detect project roots from the index`.

## Task 2: Root-aware command classification, proposals and Allow Always

**Requirements:** 5 (the boundary half), and acceptance criteria "the same
command text in two roots classifies per root, and an exact-command allowlist
entry in one root does not authorise it in another" and "a root cannot widen
`path_policy.rs`" (the command half). **Files:** `command_policy.rs`,
`validation.rs`, `command_runner.rs`, `lib.rs`, `tests/repository_map.rs`.
It does not touch `chat.rs` (`context.md` §5) or `config.rs`.

Implements `context.md` §1 and §2. It needs no map: a location is any pair of
directories.

- `CommandPolicy::classify_at(&self, command: &str, repository_root: &Path, working_directory: &Path) -> CommandClassification`.
  `classify(command, wd)` becomes `self.classify_at(command, wd, wd)`.
- `references_path_outside_root(command, repository_root, working_directory)`:
  join relative tokens to `working_directory`, then check containment against
  `repository_root`. The reason text is unchanged.
- `classify_pattern` gains the working directory's repository-relative path,
  computed lexically by `relative_location(repository_root, working_directory) -> Option<String>`.
  `Some("")` is the root, and `None` means the directory is outside the
  repository: no allowlist entry matches, and the command needs approval.
  At `Some("")` matching is today's. At `Some(rel)` only the entry
  `root_qualified_allowlist_entry(rel, command)` matches.
- `pub(crate) fn root_qualified_allowlist_entry(relative: &str, command: &str) -> String`
  returns `format!("cd {relative} && {command}")`. Its doc comment says why
  that form cannot match a typed command (`context.md` §2).
- `CommandProposal` gains `repository_root: String`. `serialize_proposal`
  writes `REPOSITORY_ROOT`. The reader defaults it to `working_directory`
  when absent. `proposal_from_classification` takes both.
  `ValidationOrchestrator` gains `propose_command_at(repository_root, working_directory, command, reason)`
  and `classify_command_at(repository_root, working_directory, command)`.
  The existing `propose_command` and `classify_command` delegate with
  `root = wd`. The `command_proposal_stored` audit event gains
  `repositoryRoot`.
- `CommandRunner::run_at(&self, command, repository_root, cwd, reason, options)`.
  `run` delegates with `root = cwd`. `run_proposal` calls `run_at` with the
  proposal's pair.
- `allow_command_always` takes the repository id from
  `proposal.repository_root`, and stores the root-qualified entry when the
  working directory is not the root.

Tests (fixture: a temp repository with `packages/api` and `packages/web`, and
`data_dir` and user config inside the temp dir):
- At `packages/api` with the user-config grant
  `command_allowlist.<id>=cd packages/api && npm test`, `npm test` is Low with
  no approval. At `packages/web` it is Medium and needs approval. At the
  root it needs approval.
- A plain `command_allowlist=npm test` matches at the root and not at
  `packages/api` (`context.md` §2, "What this narrows").
- `allow_command_always` on a proposal made at `packages/api` writes
  `cd packages/api && npm test` under `repository_id_for_root(<repo>)`, not
  under the sub-directory's id. A config reloaded with `load_scoped` then
  classifies `npm test` as Low at `packages/api` and not at `packages/web`.
- At `packages/api`, `cat ../web/src/index.ts` is not escalated, and
  `cat ../../../../etc/passwd` is escalated with the existing reason text.
- A proposal file written without `REPOSITORY_ROOT` loads with
  `repository_root == working_directory`.
- `run_proposal` of a proposal at `packages/api`, with `shell=/usr/bin/true`,
  returns `CommandExecution.working_directory` ending in `packages/api`.
- At the root, every classification in the fixture equals
  `classify(command, root)`. That pins "unchanged at the root".

Mutations to record: match plain entries at every location; check
containment against the working directory; take the repository id from the
working directory; run at the repository root.

## Task 3: `RepositoryMap::build`: per-root metadata and commands

**Requirements:** 1, 3 (except permission restrictions, `context.md` §12),
6 (`root_for_path`), 8, and §5.1, §5.3, §5.5. Acceptance criteria
"two runs over an unchanged repository produce identical map output",
"nested roots … each with their own commands", "nested `AGENTS.md` files
resolve per root". **Files:** `repository_map.rs`, `command_policy.rs`,
`context_manager.rs` (visibility only), `lib.rs`, `tests/repository_map.rs`.

- `pub struct ProjectRoot` as proposal §5.1, minus `user_override` (Task 4),
  plus `major_directories: Vec<String>` (`context.md` §12):
  `path, detected_by, languages, manifests, entry_points, test_paths, generated_paths, major_directories, instruction_files, commands`.
- `pub struct RootCommand { name, command, risk: CommandRisk, working_directory: String }`,
  with a repository-relative working directory. `CommandRisk` gains
  `Serialize, Deserialize` with `rename_all = "lowercase"`, matching `as_str`.
- `pub struct RepositoryMap` as proposal §5.1: `repository_id, schema_version, generated_at_ms, fingerprint, roots, excluded`.
  `pub const REPOSITORY_MAP_SCHEMA_VERSION: u32 = 1`.
- `RepositoryMap::build(index: &RepositoryIndex, policy: &CommandPolicy) -> RepositoryMap`.
  It calls `detect_roots`, assigns every file to its nearest root, and fills:
  - `languages`: the distinct `FileRecord.language` values except `text`,
    sorted by name;
  - `manifests`: the root's own `PROJECT_MANIFESTS` files;
  - `entry_points`: a fixed list of root-relative paths that exist
    (`src/main.rs`, `src/lib.rs`, `src/bin/*.rs`, `main.go`, `cmd/*/main.go`,
    `index.{js,ts}`, `src/index.{js,ts}`, `src/main.{js,ts}`, `main.py`,
    `__main__.py`, `app.py`, `manage.py`). It is path-based, because
    proposal §4 rules out parsing manifests;
  - `test_paths`: the root's immediate child directories named `tests`,
    `test`, `__tests__`, `spec` or `e2e`;
  - `generated_paths`: the `excluded` vendor entries whose nearest root is
    this one;
  - `major_directories`: immediate child directories that are not roots;
  - `instruction_files`: `agent_instruction_paths(&[format!("{root}/_")])`,
    kept when indexed (`context.md` §14). `agent_instruction_paths` becomes
    `pub(crate)`;
  - `commands`: per-root detection.
- `CommandPolicy` moves the body of `detect_project_commands` into
  `fn detect_in(&self, repository_root: &Path, directory: &Path) -> Result<Vec<ProjectCommand>>`,
  which classifies with `classify_at`. `detect_project_commands(root)`
  becomes `detect_in(root, root)`, so its behaviour is unchanged. Add
  `pub fn detect_root_commands(&self, repository_root: &Path, root: &str) -> Result<Vec<RootCommand>>`.
- `pub fn root_for_path(&self, path: &str) -> &str`: the longest root that
  is a path-segment prefix. `packages/a` does not own `packages/ab/x`.
- `fingerprint`: `sha256` over the sorted indexed paths, the `content_hash`
  of every manifest and instruction file, and the schema version
  (`context.md` §3). Task 4 adds the override keys.
- Every collection is sorted. Commands are sorted by name.

Tests (a temp-directory monorepo indexed with `ProjectIndexer`, watcher off):
- Two builds serialise identically once `generatedAtMs` is removed from the
  JSON. That is the determinism criterion, compared as full serialisations.
- The Cargo workspace root and both members appear, each with
  `cargo test` at its own `working_directory`. `packages/api`'s `npm test`
  has `working_directory: "packages/api"`.
- With `AGENTS.md`, `crates/AGENTS.md` and `crates/engine/AGENTS.md`,
  `crates/engine`'s `instruction_files` are those three, broadest first,
  equal to `agent_instruction_paths` for a path inside it.
- `root_for_path` for `packages/api/src/index.ts`, `packages/apiary/x.ts`
  and `README.md`.
- Changing `packages/api/package.json`'s scripts changes the fingerprint.
  Touching a non-manifest file's mtime does not.

Mutations: sort removed from `languages`; `root_for_path` by string prefix;
fingerprint without manifest content hashes; `detect_in` classifying with the
root directory as the boundary.

## Task 4: User overrides in repository config

**Requirements:** 7 and §5.7. Acceptance criterion "a misdetected root
corrected by the user persists across a rescan, and its `detected_by` reports
the override". **Files:** `config.rs`, `profile.rs`, `effective_policy.rs`,
`repository_map.rs`, `lib.rs`, `damaian-cli/src/main.rs`,
`tests/repository_map.rs`, `tests/permission_profiles.rs`.

Implements `context.md` §6.

- `project_roots_added: Vec<String>` and `project_roots_removed: Vec<String>`
  on `Config` (default empty), as `Option<Vec<String>>` on `ConfigOverlay`.
  They are `|`-separated through `split_list` and `join_list`.
- Classified as **Preference**: `classify_overlay_fields!`, `preference(..)`
  in `apply_overlay_scoped`, `set`, both `to_policy_text`s,
  `split_profile_keys` (not carried by a profile), `rule_label`.
- `RootEvidence::UserOverride` and `ExclusionReason::InvalidOverride`.
  `ProjectRoot` gains `user_override: Option<RootOverride>`, with
  `pub enum RootOverride { Added, Removed }`. A removed root is kept in the
  map with `user_override: Some(Removed)` and no commands, so the UI can show
  it and undo it. It is dropped from rendering and from command proposal.
- `RepositoryMap::build` takes the two lists, validates each entry, and
  records rejects as `invalidOverride`. Rejected: absolute, `..`, not
  normalised, under a vendor directory, restricted, no indexed file beneath,
  or the repository root itself in `removed`. An added root that detection
  also found keeps the manifest evidence, with `user_override: Some(Added)`.
- The fingerprint includes both lists.
- CLI `damaian repo-root <repo> add|remove|clear <path>` edits
  `<repo>/.damaian/config.conf` with `ConfigOverlay::load_or_default` and
  `save`. It refuses when the strict parse fails, and prints the error.
  `clear` deletes the path from both lists.
- `tests/permission_profiles.rs`: add both keys to
  `the_preference_keys_are_exactly_spec_34s_free_keys`, and a
  `PreferenceCase` each.

Tests:
- `project_roots_added=tools/scripts` makes a root with
  `detectedBy.kind == "userOverride"`.
- `project_roots_removed=examples/legacy` marks a detected root removed.
- Both persist across a rebuild from a fresh index after the repository
  changed: a new file added and a manifest touched.
- `../outside`, `/etc`, `node_modules/x`, an empty directory, `.env` (under
  the default restricted patterns) and the root in `removed` are each
  `invalidOverride` and change nothing else.
- Loaded through `load_scoped` from repository config, the keys apply and no
  `RejectedConfigKey` is recorded. A profile overlay carrying them reports
  them as not carried.
- Spec 31's partition tests pass with the two new preference keys.

Mutations: entries not validated; override dropped on rebuild; keys
classified as Capability (spec 31's preference test fails); `removed`
accepting the repository root.

## Task 5: Persistence, rebuild reporting, per-root validations, CLI

**Requirements:** §5.8, §5.5 (proposal side), and acceptance criteria "a
corrupt or version-mismatched map file rebuilds cleanly and reports that it
did" and "in a multi-root fixture, a validation command discovered in
`packages/api` runs with `packages/api` as its working directory". **Files:**
`repository_map.rs`, `validation.rs`, `workspace_engine.rs`, `lib.rs`,
`damaian-cli/src/main.rs`, `tests/repository_map.rs`, `tests/foundation.rs`.

- `pub struct RepositoryMapStore`, with `path(data_dir, repository_id)` as
  `<data_dir>/repository-map/<repository_id>.json` and
  `load_or_build(&self, index, policy, config) -> Result<(RepositoryMap, MapLoad)>`.
  `pub enum MapLoad { Reused, Built, Rebuilt { reason: RebuildReason } }`,
  where `RebuildReason` is `Corrupt`, `SchemaMismatch { found: u32 }` or
  `Stale`. The file is written with a temp file and a rename, so a crash
  leaves the old file or the new one. A rebuild audits
  `repository_map_rebuilt` with the reason.
- `WorkspaceEngine::repository_map(&self, repository_root) -> Result<(RepositoryMap, MapLoad)>`
  goes through `IndexCache::get_or_build`.
- `propose_detected_validations(repository_root, &RepositoryMap)` proposes
  each non-removed root's commands with `propose_command_at` at that root's
  directory (`context.md` §9). Update `foundation.rs`' caller and the CLI.
- CLI `damaian repo-map <repo> [--json]` prints the map, or a plain summary
  by default, and the `MapLoad` outcome. `propose-validations <repo>` goes
  per root. `detect-commands` is unchanged.
- Measure `load_or_build` on this repository, cold (`Built`) and warm
  (`Reused`). Record both times and the root count in this row
  (`context.md` §3, proposal §7).

Tests:
- A garbage file gives `Rebuilt { Corrupt }`. `schemaVersion: 0` gives
  `SchemaMismatch { found: 0 }`. A manifest change gives `Stale`. An
  unchanged repository gives `Reused`, and the file's bytes are unchanged.
  The audit event is present for each rebuild and absent for `Reused`.
- `propose_detected_validations` in the fixture yields a `packages/api`
  proposal whose `working_directory` ends in `packages/api` and whose
  `repository_root` is the repository. Run with `shell=/usr/bin/true`, its
  `CommandExecution.working_directory` is `packages/api`.
- A removed root yields no proposal.

Mutations: reuse without the fingerprint check; a schema mismatch treated as
reuse; proposals at the repository root.

## Task 6: Bounded rendering and the `repository_map` context item

**Requirements:** 2 (the ceiling) and §5.4. Acceptance criterion "the map stays
under `repository_map_max_tokens` on a large fixture, and states what it
dropped when it degrades". **Files:** `repository_map.rs`, `config.rs`,
`profile.rs`, `effective_policy.rs`, `context_manager.rs`,
`workspace_engine.rs`, `lib.rs`, `tests/repository_map.rs`,
`tests/permission_profiles.rs`. Not `chat.rs` and not `edit.rs`
(`context.md` §5).

- `repository_map_max_tokens: usize`, default 800, is a **capability** key
  with lower-wins at repository scope. It uses the same merge as
  `max_read_lines` (`context.md` §7). It also needs a
  `classify_overlay_fields!` entry, the policy texts, `rule_label`, and a
  weakening case in `weakening_cases()`. `0` disables the context item.
- `RepositoryMap::render_for_model(&self, max_tokens: usize) -> RenderedMap { text, tokens, roots_shown, roots_total, dropped: Vec<Degradation> }`.
  It estimates tokens with `len.div_ceil(4)`, as `add_text` does, and
  degrades in §5.4's order:
  1. drop entry points and test paths per root, largest root first;
  2. drop major directories;
  3. reduce generated paths to a count;
  4. as a last resort, which §5.4 leaves open, drop whole roots, deepest
     first, then by path, always keeping `""`.

  Any degradation adds an explicit line ("12 of 47 roots shown; entry points
  and test paths omitted"). Removed roots are never rendered.
- `ContextManager::new` takes the map builder (a `RepositoryMapStore`, the
  `CommandPolicy` and the ceiling). `build_context` adds a `repository_map`
  item, with `path: None`, after `project_rule` and before
  `retrieved_file`, when an index is present and the ceiling is non-zero.
  A failure to build the map omits the item and never fails the turn.

Tests:
- A generated fixture of 60 roots with long paths stays at or under 800
  tokens and names what it dropped. A small fixture is rendered whole, with
  no degradation line.
- Each degradation step fires in order: assert on `dropped` at decreasing
  ceilings.
- `build_context` with a monorepo index contains exactly one
  `repository_map` item, placed before any `retrieved_file`. With the ceiling
  at `0` it has none.
- A repository config `repository_map_max_tokens=100000` is refused; `50` is
  applied.

Run the deterministic eval tier
(`cargo run -p eval-harness -- run --tier deterministic`). Every scenario must
still pass. Record the token metric's change, which is expected to rise by
about the map's size, against the baseline.

Mutations: no ceiling check; degradation in the wrong order (the step test
fails); the item placed after `retrieved_file`; the key merged as a
preference (the weakening case fails).

## Task 7: Commands and findings in a sub-root, in the chat loop

**Requirements:** 5 (the chat half), 6 (findings), and "a root cannot widen
`path_policy.rs`" (the requested-directory half). **Files:** `chat.rs`,
`validation.rs`, `finding.rs`, `lib.rs`, `tests/repository_map.rs`.
**This is the only task that edits `chat.rs`. Read Global Constraints before
starting.**

Implements `context.md` §5 (items 1–3) and §8.

- `pub fn resolve_command_directory(path_policy: &PathPolicy, repository_root: &Path, requested: &str) -> Result<PathBuf>`
  in `validation.rs`. It goes through `resolve_existing(.., allow_outside_root: false)`,
  then requires a directory and checks `is_restricted`. A refusal is a
  `ClientError` whose text the model sees as the tool result.
- `CommandRequest` gains `working_directory: Option<String>`. The
  `run_command` schema gains an optional `working_directory`, described as
  "Repository-relative directory to run in; defaults to the repository
  root". The envelope accepts `WORKING_DIRECTORY:`. If `PendingChatTurn`
  persists a `CommandRequest`, the new field is `#[serde(default)]`.
- `chat.rs:2312` calls `propose_command_at` with the resolved directory.
  `:955` calls `classify_command_at` with the proposal's pair. `:3158` calls
  it with the resolved directory, so the mode and profile check sees the
  same classification as the proposal.
- `pub fn resolve_finding_path(repository_root: &Path, working_directory: &Path, printed: &str) -> Option<String>`
  in `finding.rs` (`context.md` §8). `record_findings` takes the
  execution's working directory and uses it. Re-check the spec 22 range
  invariants before editing: the range path becomes the repository-relative
  result.

Tests:
- A mock-model turn whose tool call has
  `"working_directory": "packages/api"` stores a proposal there. A missing
  argument stores one at the root, as today.
- `..`, `/etc`, a symlink out of the repository, a file, and a restricted
  directory are each refused before any proposal is stored, and the tool
  result says why.
- `resolve_finding_path` for an npm-style `src/x.ts` from `packages/api`,
  a cargo-style `crates/engine/src/lib.rs` from `crates/engine`, a path that
  exists in neither place, and a path in both, where the working directory
  wins.
- A recorded finding from a `packages/api` command keeps its range, with
  `packages/api/src/x.ts` and a file hash.
- The resume path re-classifies at the proposal's directory. A
  root-qualified grant for `packages/api` auto-runs there and not at the root.
- Spec 20's and spec 31's mode and profile tests pass unchanged.

Run the deterministic eval tier again.

Mutations: no `resolve_existing` (the `..` test fails); the resume
classifying at the root; findings resolved against the root only (the npm
case fails); ancestors not walked (the cargo case fails).

## Task 8: Shell API and UI

**Requirements:** 4, 6 (display), 7 (UI), §5.9, and acceptance criterion "search
results and file displays qualify a basename by its root". **Files:**
`desktop-shell/src/lib.rs`, `app.js`, `index.html`, `style.css`,
`docs/ui-style-guide.html`.

- `GET /api/repository-map?repo=` serves the map plus its `MapLoad`.
  `POST /api/repository-roots` (`repo`, `action` add/remove/clear, `path`)
  writes repository config through the same function as the Task 4 CLI, and
  returns the new map. Both are `handle_*` functions, not inline arms
  (observation 26).
- A Roots section, per repository, follows `docs/UI_STYLE_GUIDE.md`.
  - The list shows path (`.` for `""`), languages, command count, and
    "detected" or "added/removed by you".
  - The detail shows the evidence sentence ("Treated as a root because
    `packages/api/package.json` exists"), instruction files, and commands
    with their working directories.
  - Add, remove and undo. Before the first write, a note says the change is
    written to `.damaian/config.conf` in the repository, which Git shows.
- The command approval card shows "Runs in `packages/api`" when the
  proposal's working directory is not the repository root. Each patch-review
  file header shows its root tag (`context.md` §13). Both use a longest-prefix
  match over the fetched roots, the same rule as `root_for_path`.
- A pinned chip whose file has a non-`""` root reads `index.ts · packages/a`
  (`context.md` §4).

Tests: shell tests for both endpoints (the JSON shape, a write and re-read,
an invalid path reported as `invalidOverride`, and a foreign repository
refused). Verify in the running app: rebuild, start on a port other than 4765
with its own `DAMAIAN_DATA_DIR`, drive it from the browser, and stop it by
PID. Check two `index.ts` chips, an approval card in `packages/api`, a
patch across two roots, and an override that survives a reload.

## Task 9: Docs, acceptance criteria, close the spec

**Requirements:** §5.10, §6 and §7. **Files:** `docs/USER_GUIDE.md`,
`docs/TROUBLESHOOTING.md`, `proposal.md` §7, and every status record.

- Write §5.10's documentation. The user guide covers:
  - what a root is, and that `.gitignore`d and vendor directories are never
    roots;
  - correcting detection, and that it writes to the repository;
  - why a command runs where it does;
  - that Allow Always in a sub-root applies to that root only, and that a
    machine-wide allowlist entry applies at the repository root only
    (`context.md` §2);
  - that the map reaches the model, bounded, and how to turn it off.

  Troubleshooting covers the map file, forcing a rebuild by deleting it,
  reading `detectedBy`, `invalidOverride` and `belowDepthCeiling`, and the
  `repository_map_rebuilt` audit event.
- Map every acceptance criterion in proposal §6 to its test in §7. Name the
  two requirements not built (`context.md` §12) as not met, with the reason.
- Answer proposal §7's recording questions: the depth ceiling, the root count
  and map size on the largest repository tested (this one at least), the
  token default and which degradation steps fired, and any surprising roots.
- Run the full seven-command gate from `AGENTS.md`, plus
  `npm run specs:check` and the deterministic eval tier. Confirm no regression
  against `evals/baseline.json` beyond the token change recorded in Task 6.
- Close out per `AGENTS.md` "When a spec becomes Done":
  - set every status record to Done;
  - update the `Depends on:` lines of #25 and #26;
  - re-derive "What to build next" (#25's last unmet dependency is #24);
  - remove the "Map project roots…" bullet from `CHANGELOG.md`'s
    `Unreleased` table.

# Approval Prompts Name the Reason — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Implements:** [`proposal.md`](proposal.md)
**Started:** 2026-10-18
**Status:** In progress. Tasks 1–6 done (1–2 on 2026-10-18, 3–6 on 2026-10-23); task 7 remains.

## Progress

| Task | State | Notes |
|---|---|---|
| 1 · `ApprovalCause` and structured causes | Done 2026-10-18 | **Landed:** `ApprovalCause` (all 11 §5.1 variants), `CommandClassification.causes`, `reasons` rendered from `causes` via `plain_words()` (one source, no drift), the `Network`/`HighRisk`/`Unknown` split (C2), the `Blocklisted`/`Destructive` split, and `ShellControl`/`Docker`/`RestrictedPath`/`OutsideRepository` causes for each escalation. Helpers: `network_keyword`, `high_risk_keyword`, `destructive_pattern`, `configured_prefix_match` (returns the matched rule), `shell_control_constructs`, `first_word`, and `escaping_path_outside_root` (returns the escaping token so the cause can name it). `read_only_classification` no longer sets a `READ_ONLY_REASON` marker; `classify_text` returns an `admitted_read_only` bool instead, so `classify_pattern` no longer sniffs a reason string. **Tests:** 12 new in `tests/permission_profiles.rs` (one per cause + `reasons_are_derived_from_causes` + the regression guard over a 18-command table). `cargo nextest run -p workspace-engine --locked`: 1314 passed, 24 skipped. Scoped `cargo clippy -p workspace-engine --all-targets --locked -- -D warnings` clean; `scripts/quality-gate.sh fmt node lint typos deny` clean. **Falsified:** `reasons` written directly (empty) → `reasons_are_derived_from_causes` fails; `network_keyword` returning `None` → `a_curl_command_names_the_network_keyword` fails (curl falls to `Unknown`); shell-control branch pushing no cause → `a_shell_control_command_names_its_constructs` fails. All reverted. **Deviations:** (1) The shell-control branch's secondary read-only refusal is dropped — `ShellControl` is the deciding cause — so spec 64's two-reason shell-control output collapses to one cause. (2) `cd <dir>`-not-a-directory maps to `Unknown { program: "cd" }` and `ls -R` of the root maps to `RestrictedPath { path }`, because the closed enum has no dedicated variant for either; neither is in Task 1's test list. (3) The spec-64 refusal that named `read_git_diff`/`read_git_log` now maps to `Unknown { program: "git" }` per C1, so that tool pointer survives only in the model-facing description (Task 3); `git_diff_show_and_log_patch_ask_and_name_the_replacement_tool` was repurposed to assert the `Unknown` mapping. (4) Necessary ripple beyond the two named files: `causes: Vec::new()` added to `CommandClassification` literals in `mode.rs` (3) and `chat.rs` (1); `ApprovalCause` re-exported in `lib.rs`; classifier reason-string assertions in `foundation.rs` and `repository_map.rs` moved from `reasons` to `causes`. **Discoveries for later tasks:** Task 2 wires `Setting`/`ProfileBlocked` (declared, unconstructed) and must convert the `command_access` block, which still pushes its literal `Blocked by permission profile: command_access=…` string, and the `require_approval_for_*` settings, which currently leave allowlisted/admitted/validation commands with empty `causes`; `reasons_are_derived_from_causes` is scoped to the default `command_access=all` config so it does not trip on that block today. |
| 2 · Setting and profile-block causes | Done 2026-10-18 | **Landed:** `CommandPolicy` gains `sources: Option<BTreeMap<String, ConfigScope>>` derived from `RepositoryConfigReport.applied` (last scope per key, the same attribution `EffectivePolicy::from_load` derives) and a `new_with_report` constructor; `WorkspaceEngine::new_with_report` threads it (C4), and `new` delegates with `sources: None`. The `Setting { key, source: Option<String> }` cause is added in the three branches where a setting alone raises approval — the allowlist and read-only branches (`require_approval_for_all_commands`) and the validation branch (`require_approval_for_all_commands`, else `require_approval_for_risky_commands`) — naming the `rule_label` and the source (`Some("user")`/`Some("repository")`/`Some("admin")`/`Some("profile")`, or `Some("default")` when a report is present and no scope set the key). `source` is `None` when no report was threaded (a bare `Config`), and the plain words then omit " (set by …)" rather than falsely blaming "default". The `command_access` block now pushes `ProfileBlocked { access }` instead of the free-text "Blocked by permission profile" reason. **Tests:** 6 new in `tests/permission_profiles.rs` (`require_approval_for_all_commands_names_the_setting_and_its_source`, `require_approval_for_risky_commands_names_the_setting`, `a_command_access_block_is_a_profile_blocked_cause`, `a_low_risk_command_that_needs_approval_only_from_a_setting_names_that_setting_first`, `a_setting_cause_without_a_report_does_not_name_a_source`, `a_setting_cause_with_a_report_and_no_override_names_the_default`) plus `each_command_access_level_blocks_exactly_its_row` extended to assert the cause. `cargo nextest run -p workspace-engine --locked`: 1320 passed, 24 skipped. Scoped `cargo clippy -p workspace-engine --all-targets --locked -- -D warnings` clean; `cargo fmt --all` clean; `scripts/quality-gate.sh fmt node lint typos deny` clean; `npm run specs:check` clean. **Falsified (all reverted, all caught):** `setting_source` returning empty → `require_approval_for_all_commands_names_the_setting_and_its_source` fails; the `command_access` block pushing the free-text string → `a_command_access_block_is_a_profile_blocked_cause` fails; `new_with_report` dropping the report → `a_setting_cause_without_a_report_does_not_name_a_source` fails. **Deviations/discoveries:** (1) `require_approval_for_risky_commands` defaults to `true`, so a validation command (`cargo test`/`npm test`) carries `Setting { require_approval_for_risky_commands, … }` even under `Config::default()` — with a report its source is `Some("default")`; `risk`/`requires_approval`/`blocked` are unchanged. (2) `validation.rs` needed no change; its `CommandProposal.causes`/serialize/audit work belongs to Task 5. (3) **Review fix (2026-10-18):** reporting "default" for a bare `Config` was a false statement — a user-set key would have been blamed on the default in the card's Why-this-command; the source is now `None` there and the plain words omit " (set by …)". **Hand-off to Task 3 Step 1:** production still builds with `WorkspaceEngine::new`, so in the app the source stays `None` until Task 3 threads `Config::load_for_repository_reporting` + `WorkspaceEngine::new_with_report` into `desktop-shell/src/lib.rs` `config_for_repo` (used by `engine_for_repo`, `config_for_repo_with_model_options`, `config_for_repo_with_provider`) and `damaian-cli/src/main.rs` `default_engine`/`engine_for_repo`. |
| 3 · Prompt and model-facing wording | Done 2026-10-23 | **Landed:** `command_proposal_response` now leads with `reasons.first()` — the deciding cause's plain words, rendered from `causes` — then the command in backticks; a blocked command keeps its "Blocked: …" line, says "Damaian will not run it. Nothing has run.", and offers no approval. The `run_command` description and both envelope paragraphs replace "sandbox-safe" with "Damaian runs a short list of read-only commands without asking": the unrestricted paragraph says everything else needs approval, the read-only paragraph says everything else is refused outright in this mode, both naming shell control. C3's three strings are renamed (`approved_by` `"sandbox"` → `"policy"`; `--- sandbox_command_result ---` → `--- command_result ---`; `complete_with_sandbox_command` → `complete_with_command`). C5's pinned `TODAYS_CODE_SYSTEM_PROMPT`, `CODE_PARAGRAPH_BEFORE_SPEC_50`, `READ_ONLY_PARAGRAPH_BEFORE_SPEC_50` and the `ask_mode_system_prompt_has_no_command_envelope` assertion are updated together. Step 1 threads `Config::load_for_repository_reporting`/`WorkspaceEngine::new_with_report` through the CLI's `default_engine`/`engine_for_repo` and the shell's `default_engine`/`engine_for_repo`/`config_for_repo*`/`chat_engine`; the config helpers carry the `RepositoryConfigReport`, and the worker's `load_worker_config` returns it too. **Tests:** new `chat::approval_prompt_wording_tests` (4 unit tests over the builders, every mode prompt and both proposal cases) and 3 new `tests/foundation.rs` end-to-end tests (cause-first prompt; a chat turn and audit with no "sandbox"; auto-run wording in the offered tool and system prompt). `cargo nextest run -p workspace-engine --test foundation --locked`: 164 passed. `cargo nextest run -p workspace-engine --lib -E 'test(chat::)' --locked`: 65 passed. The `desktop-shell` worker config test passed. Scoped clippy clean for `workspace-engine`, `desktop-shell` and `damaian-cli`; `scripts/quality-gate.sh fmt node lint typos deny` clean. **Falsified (all reverted, all caught):** the unrestricted envelope sentence reverted to "The app will run sandbox-safe commands automatically" → both `no_local…` tests and both auto-run-description tests fail; `command_proposal_response` reverted to a fixed "I need your approval…" sentence → the unit and foundation cause-first tests fail; the audit status reverted to `complete_with_sandbox_command` → the foundation no-sandbox test fails on the audit log. **Deviations/discoveries:** (1) Task 3's file list omits `crates/desktop-shell/src/worker.rs`, but `chat_engine` now takes `(Config, RepositoryConfigReport)`, so the worker's `load_worker_config` returns the report and its four call sites destructure; worker turns get the same source attribution. (2) `config_for_repo_with_model_options`/`config_for_repo_with_provider` now return `(Config, RepositoryConfigReport)`; `config_for_repo` stays a Config-only convenience for non-engine consumers (`reveal_web_diagnostic_artifact`, `model_key_status_json`). (3) `docs/TROUBLESHOOTING.md`'s audit-status list is updated to `complete_with_command`; the remaining "not sandboxed" prose in `USER_GUIDE.md`/`SECURITY.md`/`app.js` is accurate and belongs to Task 7. (4) The `approved_by` `"sandbox"` literal is unobservable: auto-run has `requires_approval == false`, so the runner records no approver (`approvedBy: ""`); the rename is still made per C3. (5) Two existing foundation assertions that the approval message `contains("approval")` now assert `starts_with("Asks first:")`, since the message names the cause and no longer uses that word. (6) The MCP approval prompt still says "runs outside the local sandbox"; it is not a local-command string, is outside C3's list and the acceptance test, and is left for a later pass. (7) Task 3's line references had drifted with #59–#64 (e.g. `command_proposal_response` is at 6249, not 5310); every named item still exists, and the proposal's §2/§5.3 references are refreshed. (8) Step 1 adds no test of its own: Task 2's report-aware test covers the behaviour, and the production call sites are compile-checked; observing the source from `desktop-shell` would require mutating the test binary's shared data dir or a repository config. |
| 4 · The card names the cause and the model's reason | Done 2026-10-23 | **Landed:** `AgentCommandProposal` gains `cause` (the first rendered `reasons` entry, i.e. the deciding cause's plain words) and `reason` (the model's own stated reason); `agent_command_proposal` fills both from the stored `CommandProposal`. The shell's three hand-written card payloads now carry them: `command_proposal_json` for chat results and worker cards (`crates/desktop-shell/src/lib.rs`), `handle_propose_command`'s response, and `recovery.rs`'s reattached-proposal JSON. `createCommandApprovalPreview` renders the cause as its own line outside the disclosure, next to the risk badge, and the reason as "The agent says: …" after the command; **Why this command** keeps the full prompt with every cause. MCP and browser-diagnostic proposals leave both fields empty and the card hides the lines. **Tests:** new `chat::approval_prompt_wording_tests::the_card_payload_carries_the_first_cause_and_the_models_reason` (two causes; the first wins) and `a_blocked_command_still_names_its_cause_on_the_card`; new `tests/agent_tools.rs::a_network_command_reaches_the_card_with_its_cause_and_the_models_reason` (a `MockModelAdapter` turn proposing `curl https://example.com` with a reason). `cargo nextest run -p workspace-engine --test agent_tools --locked`: 30 passed. `cargo nextest run -p workspace-engine --lib -E 'test(approval_prompt_wording_tests)' --locked`: 6 passed. `cargo nextest run -p desktop-shell --lib --locked`: 160 passed, 5 skipped. Scoped clippy clean for `workspace-engine` and `desktop-shell`; `node --check` and `npm run lint:web` clean; `scripts/quality-gate.sh fmt node lint typos deny` clean. **Falsified (all reverted, all caught):** `cause`/`reason` set empty in `agent_command_proposal` → both named card tests fail; the cause moved inside the `details` disclosure panel → the DOM harness fails ("card renders a cause line"); the agent reason omitted → the harness fails on `The agent says: …`. The card harness is a throwaway DOM script (there is no JS test suite; style guide §9), not committed. **Deviations/discoveries:** (1) Task 4's file list omits the shell's JSON bridges — `crates/desktop-shell/src/lib.rs` (`command_proposal_json`, `handle_propose_command`) and `crates/desktop-shell/src/recovery.rs` build the card payload with format strings, so the new fields never reach `app.js` without editing them; all three now carry `cause`/`reason`, and worker cards use the same lib.rs builder. (2) `CommandProposal` has no `causes` yet (Task 5), so the first cause is `reasons.first()` — the first rendered cause, keeping the one-source rule; Task 5 can switch to `causes.first().plain_words()` without changing the wire format. (3) MCP and browser-diagnostic proposals have no policy cause, so they carry empty strings and the card hides both lines rather than inventing a cause outside the closed enum. (4) `docs/UI_STYLE_GUIDE.md` §5 said rationale is progressive. This spec owns the surface and requires the first cause and the agent's reason outside the disclosure, so the rule and the `docs/ui-style-guide.html` specimens were updated in the same change. (5) The pre-existing `command_approval_prompt` already lists every cause under **Why this command**; no prompt builder changed. (6) No classification decision moved. |
| 5 · Recording causes in the proposal and audit record | Done 2026-10-23 | **Landed:** `CommandProposal` gains `causes: Vec<ApprovalCause>` filled from `classification.causes` in `proposal_from_classification`, so the values travel with the proposal. `serialize_proposal` writes a last, optional `CAUSES` field as JSON (every field of every cause survives, not only its name) and `deserialize_proposal` reads it when present, falling back to an empty list on older files; `serialize_proposal` now returns `Result`. `ApprovalCause` derives `Serialize`/`Deserialize` (`snake_case`) and gains `as_str()` for the audit's variant names; `store_proposal` records `causes` on `command_proposal_stored` as the comma-joined names in classifier order, through `AuditLog::record` and therefore the same secret scanner as every field beside it. **Tests:** 5 new, non-ignored tests in `tests/command_lifecycle.rs`: `a_stored_proposal_round_trips_its_causes_in_order` (two causes, order preserved, whole proposal equal after reload), `a_stored_proposal_round_trips_a_profile_block` (`CommandAccess` round-trips inside `ProfileBlocked`), `a_proposal_stored_before_causes_loads_with_no_causes` (hand-written pre-change file loads with an empty list), `the_stored_proposal_audit_names_its_causes_in_order` (`unknown,outside_repository`), and `the_audit_causes_name_causes_without_command_text` (a credential in the command is redacted in the audit beside a names-only `causes`). `cargo nextest run -p workspace-engine --test command_lifecycle --locked`: 5 passed, 5 skipped. `cargo nextest run -p workspace-engine --test foundation --locked`: 164 passed. Scoped `cargo clippy -p workspace-engine --all-targets --locked -- -D warnings` clean; `cargo fmt --all` clean; `node --check` and `npm run lint:web` clean; `scripts/quality-gate.sh fmt node lint typos deny` clean. **Falsified (all reverted, all caught):** storing only the first cause → `a_stored_proposal_round_trips_its_causes_in_order` fails on the reloaded list; making `CAUSES` mandatory on read → the legacy test fails with "Expected field CAUSES, found END_COMMAND_PROPOSAL"; omitting the audit tuple → `the_stored_proposal_audit_names_its_causes_in_order` fails. **Deviations/discoveries:** (1) The plan named `tests/command_lifecycle.rs`, but that file was the all-`#[ignore]` shell-lifecycle suite with no proposal or audit tests; the five storage/audit tests are non-ignored additions there and the module doc now says only the shell-spawning tests are ignored. (2) The plan's sample audit value `shell_control,unknown` is unreachable: the shell-control branch returns a single cause. Order is asserted with `python3 ../script.py` → `unknown,outside_repository`, and the profile-block round-trip covers `shell_control,profile_blocked` separately. (3) Necessary ripple beyond the three named files: `config.rs`'s `CommandAccess` gains the same serde derives (it is `ProfileBlocked`'s field), and `chat.rs`'s test-only `CommandProposal` helper gains `causes`. (4) `agent_command_proposal` deliberately keeps `reasons.first()` rather than `causes.first()`: a proposal written before this change has `REASONS` but no `CAUSES`, and reading `causes` there would blank its card cause on reattach. (5) No classification decision moved: `risk`, `requires_approval` and `blocked` are untouched; the new data is recording only. |
| 6 · Eval harness groups prompts by first cause | Done 2026-10-23 | **Landed:** `RecordedApproval` gains `cause: Option<String>`, the first name in `command_proposal_stored`'s `causes` field (read by `first_cause` in `runner.rs`), `None` for a decision event and for a proposal that ran without asking. `RunRecord` gains `approvals_by_cause`, a `BTreeMap<String, u64>` derived by `group_approvals_by_cause` from `approvals`, so the grouping and the list cannot drift; the `BTreeMap` keeps the text report and a baseline diff in a stable cause order. `MetricValue::Groups { counts }` is new, and `approval_prompts_by_cause` ("Approval prompts by first cause") is the 23rd `KEYS` entry, summing the per-run maps and reporting `NotApplicable { phase: "no-approval-prompts" }` when nothing asked. `report::to_text` gains a per-run "Approval prompts by first cause" section printing `cause count`. New scenario `approval_causes.toml`: `approval_decision = true`; `cd src && git diff | head -20` → `shell_control`, then `python3 ../script.py` → `unknown,outside_repository`, so a last-cause reader would group it as `outside_repository`. The two pinned scenario counts in `tests/harness.rs` moved 21 → 22. **Tests:** 4 new in `tests/harness.rs`: `a_run_records_each_approval_prompt_by_its_first_cause`, `a_proposal_that_needed_no_approval_is_not_counted_as_a_prompt` (`planned_task`'s `ls src` stores a proposal with empty `causes`), `the_report_groups_approval_prompts_by_their_first_cause` (per-run line and metric), and `the_approval_cause_metric_sums_across_runs`. `cargo nextest run -p eval-harness --locked`: 83 passed, 1 skipped. `cargo run -p eval-harness -- run --tier deterministic`: all 22 scenarios pass; the report reads `approval_causes shell_control 1 unknown 1`, `denied_approval unknown 1`, `resume_interrupted_session setting 1`, and the metric `setting 1 shell_control 1 unknown 2`. Scoped `cargo clippy -p eval-harness --all-targets --locked -- -D warnings` clean; `scripts/quality-gate.sh fmt node lint typos deny` clean; `node --check` and `npm run lint:web` included there. **Falsified (both reverted, both caught):** `first_cause` returning a constant `"prompt"` → all three grouping tests fail (the record test on `shell_control` vs `prompt`, the auto-run test on a cause it must not have, the report test on the line); the metric returning `Count { value: 1 }` → both metric tests fail. **Deviations:** (1) `approvals` still lists every stored proposal, auto-run ones included, as it did before; grouping filters them out by their empty `causes`, so the counts describe prompts rather than proposals. (2) A new scenario was added rather than extending an existing one, because no committed scenario produced two distinct prompts in one run. (3) `evals/baseline.json` was not regenerated: it predates `findingsByPurpose` too, and this repository treats regenerating it as a human-reviewed step. (4) Task 6 said the approvals were built "around line 390"; they are at 490 in `runner.rs` — the line reference was stale, the described code was not. |
| 7 · Docs, live re-run and close-out | Not started | |

## Global Constraints

Every task's requirements implicitly include this section.

- **Read [`proposal.md`](proposal.md) §2, §3 and §5 and the Corrections table
  below before starting.** Where a correction contradicts the proposal's prose,
  the correction wins, and the task that acts on it updates the proposal in the
  same change (most were already folded in while planning).
- **No command-policy decision change** (requirement 4 of §4; acceptance §6,
  last row). This spec explains the decision; it does not move it. `risk`,
  `requires_approval` and `blocked` must come out byte-for-byte identical for
  every command in the existing policy tests. Spec 64 has already landed on
  `command_policy.rs`; if a task finds itself wanting to change *which* commands
  ask, stop and say so.
- **One source for the words.** `reasons` strings are rendered from `causes`
  (§5.1), so the string and the value cannot drift. No branch may add a reason
  string that no `ApprovalCause` produces.
- **`ApprovalCause` is a closed set.** A new cause is a design decision, not an
  implementation convenience. The variants are the table in §5.1 plus
  `RestrictedPath`; the spec-64 read-only refusals map onto the existing
  variants (Correction C1) rather than growing the set silently.
- **The `Setting` source is the attribution `effective_policy.rs` already
  computes** (Correction C4). Do not resolve the config files a second time in
  `command_policy.rs`; thread the `RepositoryConfigReport` (or a source map
  derived from it) into the classifier or the proposal path.
- **"sandbox" leaves every user- and model-facing string about local commands**
  (requirement 4). Internal function names (`sandbox_command_context`) may stay
  unless a task is already renaming that code; the strings named in Correction
  C3 must go.
- **`chat.rs` is shared and this spec does not pair.** Spec 63's Task 4 rewrites
  the same model-facing command text, and #59/#60 also edit `chat.rs`
  (`docs/specs/README.md`, "Parallel work"). Tasks here run sequentially and
  must not run beside #59, #60 or #63.
- **Tests:** engines built in tests set `enable_index_watcher: false` and their
  own data directory. Model-dependent paths use `MockModelAdapter` or
  `DAMAIAN_MOCK_MODEL_RESPONSE`. Fixture repositories are fresh `git init`
  directories under the temp dir.
- **Falsify every load-bearing test.** Break what it guards, confirm it fails,
  revert, and record that in the Progress row.
- **A private Cargo target directory per worktree.** Prefix every `cargo`
  command with `CARGO_TARGET_DIR="$PWD/target"` from the worktree root.
- **Scoped checks per task; the full gate runs on CI only** (AGENTS.md, since
  2026-10-15). Each task runs `scripts/quality-gate.sh fmt node lint typos deny`,
  `cargo clippy -p <crate> --all-targets --locked -- -D warnings` for every
  crate it touches, and the test files it names.
- **Never `git commit` or `git push` unasked.** Each task ends by showing the
  change and the check results. One-line commit subject, no body.

## Corrections to the proposal found while planning

Each one names the task that owns it.

| # | Proposal said | Code says | Resolution |
|---|---|---|---|
| C1 | The classifier is the pre-64 branch list: blocklist/destructive → shell control → allowlist → `is_low_risk_read_only` → validation → Docker → high-risk → unknown (§2, §5.1) | Spec 64 merged 2026-10-17/18. `classify_pattern` first runs `read_only_verdict` (the `shell_line`/`command_verdict`/`pipeline_verdict` tokenizer), then `classify_text` checks blocklist/destructive → shell control (an admitted pipeline passes) → allowlist → `ReadOnlyVerdict` (`Admitted`/`Refused`/`NotReadOnly`) → validation → Docker → high-risk → unknown; `classify_at` escalates for restricted paths, `cd` targets, outside-repository paths and `command_access` | `ApprovalCause` replaces *these* reason strings, including spec 64's read-only-form refusals (map to `Unknown`) and `RESTRICTED_PATH_REASON` (which becomes `RestrictedPath { path }`, already anticipated by its doc comment). §5.1 table already adds `RestrictedPath`. **Task 1** |
| C2 | `curl`/`wget` produce a distinct network reason (§1, §5.1 `Network { keyword }`) | There is no network branch. `curl` and `wget` fall into `is_high_risk_command` with the generic "may modify dependencies, Git state, permissions, network, or shell state" string; `may_use_network` is a separate boolean, not a cause | Split the network keywords (`curl`, `wget`, `git pull/push/fetch/clone`, package installers) into a `Network { keyword }` branch before the high-risk branch, and have `HighRisk { keyword }` name the keyword that fired. §2 corrected. **Task 1** |
| C3 | "sandbox" lives only in `command_proposal_response` and the two envelope paragraphs/tool description (§2) | Three more strings: the auto-run path passes the literal `"sandbox"` as `approved_by` (`chat.rs:2756`), the model-facing context separator is `--- sandbox_command_result ---` (`chat.rs:4300`), and the audit turn status includes `complete_with_sandbox_command` (`chat.rs:3411`) | All three are renamed (e.g. `approved_by` becomes `"policy"`/`"auto"`, the separator becomes `--- command_result ---`, the status `complete_with_command`). §2 corrected. **Task 3** |
| C4 | `Setting.source` "comes from the same attribution `effective_policy.rs` already computes" (§5.1) | That attribution is not available where `classify_at` runs: `CommandPolicy::new` takes `Config` only (`workspace_engine.rs:78`), and the `RepositoryConfigReport` returned by `Config::load_scoped` is discarded before the engine is built | Thread the report (or a `ConfigScope` source map derived from it) into `CommandPolicy` or the proposal path, so `Setting` can name user/repository/profile without re-resolving. §5.1 corrected. **Task 2** |
| C5 | `command_proposal_response` at `chat.rs:5105`; tool description at 4589; "omits the auto-run sentence" test around line 6178 (§2, §5.3) | `command_proposal_response` is at `chat.rs:5310`; the tool description at 4747; the envelope paragraphs at 4247 (Code) and 4255 (Plan). The "omits the auto-run sentence" test is `ask_mode_system_prompt_has_no_command_envelope` at 6414; `code_mode_system_prompt_is_byte_identical_to_todays` (6406) pins the Code prompt verbatim against `TODAYS_CODE_SYSTEM_PROMPT`, and the `CODE_PARAGRAPH_BEFORE_SPEC_50`/`READ_ONLY_PARAGRAPH_BEFORE_SPEC_50` fixtures live at 6665 | Update the wording in one change with the pinned constant and both fixtures, or the byte-identical test breaks. §2, §5.3 corrected. **Task 3** |
| C6 | The proposal's audit record "gains `causes`" (§5.4), implying it already carries the reasons | The `command_proposal_stored` event (`validation.rs` `store_proposal`) carries `risk`/`requiresApproval`/`blocked` only — no reasons at all. The proposal also round-trips through `serialize_proposal`/`deserialize_proposal` and is reloaded for reattach and *Allow Always* | `CommandProposal` gains a `causes` field, written and read with the file format, and `store_proposal` records it; the audit value and the reloaded proposal then cannot drift. §5.4 corrected. **Task 5** |

## File Structure

| File | Change |
|---|---|
| `crates/workspace-engine/src/command_policy.rs` | `ApprovalCause` enum, `CommandClassification.causes`, reasons rendered from causes, `Network`/`HighRisk` split, `RestrictedPath` (Tasks 1, 2) |
| `crates/workspace-engine/src/workspace_engine.rs` | Thread the `RepositoryConfigReport` (or source map) into `CommandPolicy` (Task 2) |
| `crates/workspace-engine/src/validation.rs` | `CommandProposal.causes`, serialize/deserialize, `command_proposal_stored` audit field (Tasks 2, 5) |
| `crates/workspace-engine/src/chat.rs` | `command_proposal_response`, tool description, system prompt and envelope paragraphs, `approved_by`, `--- sandbox_command_result ---` separator, `complete_with_sandbox_command` status, pinned prompt constant (Task 3); `AgentCommandProposal` gains `reason` + first cause (Task 4) |
| `crates/desktop-shell/static/app.js` | `createCommandApprovalPreview`: first cause beside the risk badge, model's `reason` as "The agent says: …" (Task 4) |
| `crates/workspace-engine/tests/permission_profiles.rs` | Setting/ProfileBlocked causes; unchanged classification under profiles (Tasks 1, 2) |
| `crates/workspace-engine/tests/foundation.rs` | The chat-level approval prompt and "no sandbox" assertions (Task 3) |
| `crates/workspace-engine/tests/command_lifecycle.rs` | Audit `causes` on the stored proposal (Task 5) |
| `crates/eval-harness/src/{record,runner,metrics,report}.rs` | Prompts grouped by first cause, per run (Task 6) |
| `docs/USER_GUIDE.md`, `docs/TROUBLESHOOTING.md`, `SECURITY.md`, `docs/specs/63_exploratory_command_failures/proposal.md`, `docs/specs/64_read_only_command_boundary.md`, `docs/specs/README.md`, `CHANGELOG.md` | Task 7 |

## Interfaces

Task 1 lands the cause enum; later tasks consume it.

```rust
// command_policy.rs
pub enum ApprovalCause {
    Blocklisted { rule: String },
    Destructive { pattern: String },
    ShellControl { constructs: String },
    Docker,
    HighRisk { keyword: String },
    Network { keyword: String },
    Unknown { program: String },
    RestrictedPath { path: String },
    OutsideRepository { path: String },
    Setting { key: String, source: Option<String> },
    ProfileBlocked { access: CommandAccess },
}

pub struct CommandClassification {
    // …existing fields…
    pub causes: Vec<ApprovalCause>,   // most decisive first; blocking before approval
}

impl ApprovalCause {
    pub fn plain_words(&self) -> String;   // the first line of the prompt
}
```

The exact `Setting`/`ProfileBlocked` shapes are Task 2's to settle; the field
names above are a sketch, not a contract. `reasons` is kept as the rendered
strings (spec 63 and the card still read it) but is now derived from `causes`.

## Order and parallelism

```
1 ──> 2 ──> 3 ──> 4 ──> 5 ──> 6 ──> 7
```

Sequential. Tasks 1, 2 and 5 all edit `command_policy.rs` and `validation.rs`;
Tasks 3 and 4 both edit `chat.rs`. No task may run beside #59, #60 or #63
(`docs/specs/README.md`, "Parallel work").

---

## Task 1: `ApprovalCause` and structured causes

**Requirements:** 1, 2 (the value, not the sentence). **Proposal:** §5.1.
**Corrections:** C1, C2. **Files:** `command_policy.rs`,
`tests/permission_profiles.rs`.
**Started:** 2026-10-18 — **Done:** 2026-10-18.

- [x] **Step 1: Write the failing tests.** For each branch of `classify_text` and
      each escalation in `classify_at`, assert `causes` carries the right variant
      in the right order, and that `reasons` equals the rendered plain words:
  - `a_shell_control_command_names_its_constructs` (`cd x && git diff | head` →
    `ShellControl` naming `&&` and `|`);
  - `a_curl_command_names_the_network_keyword` (`curl https://example.com` →
    `Network { keyword: "curl" }`, not high-risk);
  - `a_high_risk_command_names_its_keyword` (`chmod`, `git commit` →
    `HighRisk` with the keyword);
  - `an_unknown_command_names_its_program` (`python3 script.py` → `Unknown` with
    `python3`);
  - `a_docker_command_is_a_docker_cause`;
  - `a_blocklisted_command_is_blocked_with_its_rule` and `a_destructive_command_is_blocked_with_its_pattern`;
  - `a_restricted_path_escalation_is_a_restricted_path_cause`;
  - `a_path_outside_the_repository_is_an_outside_repository_cause`;
  - `the_read_only_refusal_reasons_are_rendered_from_causes` (spec 64's
    `Refused` forms still produce a `causes` entry, mapped per C1);
  - `reasons_are_derived_from_causes` (no branch adds a string absent from
    `causes`).
  And the regression guard: for every command in the existing policy tests,
  `risk`, `requires_approval` and `blocked` are unchanged (assert this over the
  same command table, not by editing the existing assertions).
- [x] **Step 2: Run them and confirm they fail** (no `causes` field exists yet).
- [x] **Step 3: The enum, the field, and the renderer.** Add `ApprovalCause` and
      `CommandClassification.causes`; move each branch's reason into a cause and
      render `reasons` from `causes`. Keep `is_low_risk_read_only`'s verdicts
      intact.
- [x] **Step 4: The `Network`/`HighRisk` split (C2).** A `Network` branch before
      the high-risk branch, naming the keyword; `HighRisk` names the keyword that
      fired; `Unknown` names the program (first word).
- [x] **Step 5: Falsify.** Revert each mapping in turn (a branch that pushes no
      cause, a keyword left unnamed, `reasons` written directly) and confirm a
      named test fails for each, then revert.
- [x] **Step 6: Update the proposal** for C1 and C2 (already folded in; check
      §2 and §5.1 agree).
- [x] **Step 7: Scoped checks.** `cargo nextest run -p workspace-engine --test
      permission_profiles`, the `command_policy` unit tests, scoped clippy for
      `workspace-engine`, and the fast checks.

## Task 2: Setting and profile-block causes

**Requirements:** 3; the `ProfileBlocked` half of §5.1.
**Proposal:** §5.1 (Setting, ProfileBlocked). **Corrections:** C4.
**Files:** `command_policy.rs`, `workspace_engine.rs`, `validation.rs`,
`tests/permission_profiles.rs`.
**Started:** 2026-10-18 — **Done:** 2026-10-18.

- [x] **Step 1: Write the failing tests.**
  - `require_approval_for_all_commands_names_the_setting_and_its_source`
    (`git status` under that key → `Setting { key: "require_approval_for_all_commands" }`
    whose plain words are "Ask before every command", and the source is
    `user`/`repository`/`profile` as set in the fixture);
  - `require_approval_for_risky_commands_names_the_setting` (`cargo test` →
    "Ask before test, build and lint commands");
  - `a_command_access_block_is_a_profile_blocked_cause` (the existing
    `each_command_access_level_blocks_exactly_its_row` table extended to assert
    the cause, `ProfileBlocked { access }`, not a free-text reason);
  - `a_low_risk_command_that_needs_approval_only_from_a_setting_names_that_setting_first`.
- [x] **Step 2: Run them and confirm they fail.**
- [x] **Step 3: Thread the source (C4).** Give `CommandPolicy` (or the proposal
      path) the `RepositoryConfigReport`'s attribution for the two
      `require_approval_for_*` keys and for `command_access`, reusing
      `effective_policy.rs` rather than re-resolving config.
- [x] **Step 4: `Setting` and `ProfileBlocked`.** Build the two causes in
      `classify_at` where the settings currently flip `requires_approval`/`blocked`
      without a reason, naming the `rule_label` and the source.
- [x] **Step 5: Falsify.** A `Setting` cause with no source, a `ProfileBlocked`
      cause with the free-text string instead of the variant, and a source that
      re-resolves config all fail a named test; revert each.
- [x] **Step 6: Update the proposal** for C4 (already folded in).
- [x] **Step 7: Scoped checks.** `cargo nextest run -p workspace-engine --test
      permission_profiles`, scoped clippy for `workspace-engine`, fast checks.

## Task 3: Prompt and model-facing wording

**Requirements:** 2 (the first-line sentence), 4.
**Proposal:** §5.2 (message), §5.3 (wording). **Corrections:** C3, C5.
**Files:** `chat.rs`, `tests/foundation.rs`, `crates/desktop-shell/src/lib.rs`,
`crates/damaian-cli/src/main.rs`.
**Started:** 2026-10-23 — **Done:** 2026-10-23.

- [x] **Step 1: Thread the report into production (C4 hand-off).** Without
      this the app and CLI build their engine with `WorkspaceEngine::new`,
      which leaves `Setting.source` `None`, so a setting's prompt never names
      who set it. Load with `Config::load_for_repository_reporting` and build
      with `WorkspaceEngine::new_with_report` at the repository-scoped call
      sites: `desktop-shell/src/lib.rs` `config_for_repo` (used by
      `engine_for_repo`, `config_for_repo_with_model_options` and
      `config_for_repo_with_provider`), and `damaian-cli/src/main.rs`
      `default_engine` and `engine_for_repo` — all of which today call
      `Config::load_for_repository` then `WorkspaceEngine::new`.
- [x] **Step 2: Write the failing tests.**
  - `command_proposal_response_leads_with_the_deciding_cause` (a shell-control
    proposal's first line names `&&` and `|`; a blocklisted one names its rule
    and offers no approval);
  - `no_local_command_string_shown_or_sent_contains_sandbox` (assert over
    `command_proposal_response`, `command_approval_prompt`, the `run_command`
    tool definition, the two envelope paragraphs, `system_prompt` for every
    mode, and the `approved_by`/separator/status strings in Correction C3);
  - `the_auto_run_description_says_the_policy_runs_it_without_asking` (the new
    wording replaces "sandbox-safe").
- [x] **Step 3: Run them and confirm they fail.**
- [x] **Step 4: Rewrite `command_proposal_response`** to lead with the first
      cause's plain words, then the command; a blocked command says what blocked
      it and that nothing runs.
- [x] **Step 5: Remove "sandbox" (C3).** The tool description and envelope
      paragraphs describe auto-run as "a short list of read-only commands the
      policy runs without asking"; rename `approved_by` `"sandbox"`, the
      `--- sandbox_command_result ---` separator and the `complete_with_sandbox_command`
      status.
- [x] **Step 6: Update the pinned wording (C5).** Change `TODAYS_CODE_SYSTEM_PROMPT`
      and the `CODE_PARAGRAPH_BEFORE_SPEC_50`/`READ_ONLY_PARAGRAPH_BEFORE_SPEC_50`
      fixtures in the same change, and update
      `ask_mode_system_prompt_has_no_command_envelope`'s assertion string.
- [x] **Step 7: Falsify** (a string that keeps "sandbox", a first line that does
      not name the cause) and revert.
- [x] **Step 8: Scoped checks.** `cargo nextest run -p workspace-engine --test
      foundation`, the `chat.rs` unit tests, scoped clippy for `workspace-engine`
      and the two touched binaries, and the fast checks (which include
      `node --check` and web lint).

## Task 4: The card names the cause and the model's reason

**Requirements:** 5. **Proposal:** §5.2. **Corrections:** none (the
`AgentCommandProposal` gap is described in §2).
**Files:** `chat.rs`, `crates/desktop-shell/static/app.js`.
**Started:** 2026-10-23 — **Done:** 2026-10-23.

- [x] **Step 1: Write the failing tests.** `AgentCommandProposal` carries the
      model's `reason` and the first cause's plain words; a unit test over
      `agent_command_proposal` asserts both, and a `MockModelAdapter` turn that
      proposes `curl …` with a reason yields a proposal whose card payload
      carries them.
- [x] **Step 2: Run them and confirm they fail.**
- [x] **Step 3: `AgentCommandProposal`** gains `reason` and a first-cause string
      (or the `causes` list); `agent_command_proposal` fills both from the
      stored proposal.
- [x] **Step 4: The card.** In `createCommandApprovalPreview`, render the first
      cause beside the risk badge outside any disclosure, and the model's
      `reason` as "The agent says: …". **Why this command** keeps the full
      prompt including every cause.
- [x] **Step 5: Falsify** (the cause hidden behind the disclosure, the reason
      omitted) and revert.
- [x] **Step 6: Scoped checks.** `cargo nextest run -p workspace-engine --test
      agent_tools`, scoped clippy for `workspace-engine`, `node --check
      crates/desktop-shell/static/app.js` and `npm run lint:web`.

## Task 5: Recording causes in the proposal and audit record

**Requirements:** 6. **Proposal:** §5.4. **Corrections:** C6.
**Files:** `validation.rs`, `command_policy.rs`,
`tests/command_lifecycle.rs`.
**Started:** 2026-10-23 — **Done:** 2026-10-23.

- [x] **Step 1: Write the failing tests.** A stored proposal round-trips its
      `causes` in order (`save_proposal` → `load_proposal`); a proposal stored
      before this change reads back with empty causes rather than failing; the
      `command_proposal_stored` audit event carries a `causes` field
      (`shell_control,unknown`) in order.
- [x] **Step 2: Run them and confirm they fail.**
- [x] **Step 3: `CommandProposal.causes`.** Add the field in
      `proposal_from_classification`, and a `CAUSES` field to
      `serialize_proposal`/`deserialize_proposal` that is optional on read
      (older proposals lack it, matching how `PURPOSE` was added in spec 63).
- [x] **Step 4: Audit.** Record `causes` on `command_proposal_stored`.
- [x] **Step 5: Falsify** (a cause dropped on round-trip, a missing field that
      fails the read, the audit value omitted) and revert.
- [x] **Step 6: Scoped checks.** `cargo nextest run -p workspace-engine --test
      command_lifecycle` and `--test foundation`, scoped clippy for
      `workspace-engine`, fast checks.

## Task 6: Eval harness groups prompts by first cause

**Requirements:** §5.5. **Proposal:** §5.5. **Files:**
`crates/eval-harness/src/record.rs`, `runner.rs`, `metrics.rs`, `report.rs`,
`crates/eval-harness/scenarios/approval_causes.toml`.
**Started:** 2026-10-23 — **Done:** 2026-10-23.

- [x] **Step 1: Read the causes.** `runner.rs` builds `approvals` from
      `command_proposal_stored` events (around line 390); extend `RecordedApproval`
      with the first cause read from the event's new `causes` field.
- [x] **Step 2: Group and report.** Add a per-run metric and report line
      grouping prompts by first cause (count and cause name), so the motivating
      session would read as nearly all `shell_control`.
- [x] **Step 3: A scenario** (or an assertion on an existing approval scenario)
      that produces at least two distinct first causes and asserts the grouping
      distinguishes them.
- [x] **Step 4: Falsify** (grouping that ignores the cause, a metric that
      reports a constant) and revert.
- [x] **Step 5: Scoped checks.** `cargo nextest run -p eval-harness`, scoped
      clippy for `eval-harness`, fast checks.

## Task 7: Docs, live re-run and close-out

- [ ] **Step 1: Live re-run** of the motivating approval against
      `deepseek-v4-flash` on this repository. Record the first line each prompt
      showed and confirm it names the real cause, in proposal §6.
- [ ] **Step 2: Docs** (§5.6): `USER_GUIDE.md` (the approval card, and what runs
      without asking — without the word "sandbox"), `TROUBLESHOOTING.md` (reading
      an approval's cause), `SECURITY.md` if it describes auto-run commands as
      sandboxed.
- [ ] **Step 3: Cross-references.** #63 and #64's "Related" sections already
      point at the folder; confirm their wording still matches the shipped
      wording. Update `docs/specs/README.md`'s "What to build next" once the
      spec is Done.
- [ ] **Step 4: Close the spec** per AGENTS.md "When a spec becomes Done":
      status in the proposal, README row and this file; `Depends on:` search;
      What to build next; remove the item from `CHANGELOG.md` Unreleased
      ("Say why each command needs your approval, instead of one sentence about
      a sandbox that does not exist").
- [ ] **Step 5: Checks.** Fast checks and scoped clippy/tests locally; the full
      gate on CI. Record the CI result here when it is green.

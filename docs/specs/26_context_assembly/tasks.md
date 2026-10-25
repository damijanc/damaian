# Context Assembly — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Implements:** [`proposal.md`](proposal.md), with the corrections below and the
current-code map in [`context.md`](context.md)
**Started:** 2026-10-24 — **Done:** 2026-10-25
**Status:** Done. Planned 2026-10-24 against `main`; Tasks 1–6 landed 2026-10-25 and Task 7 closed on the same day. The live calibration was run by the owner; the offline half was not run and is recorded as outstanding in [`proposal.md`](proposal.md) §7, as are the spec's known open points.

## Open questions for the owner

Each one is answered by a stated assumption in the plan so that no task is
blocked. If the answer is different, the named task changes in one place.

1. **Is `context_token_budget` a packing budget or the provider's window?** The
   config docs call it a packing budget (`config.rs:1920-1924`), and the built-in
   grants 64k to a model with a 1M-token window on purpose. Requirement 7 and the
   acceptance criterion say the declared path subtracts "the documented safety
   margin". Subtracting from a packing budget would shrink every existing user's
   context by 10% for no safety gain. **Assumption:** the declared path uses the
   config value unchanged (`safety_margin_bps = 0`), and the margin applies only
   on the observed path, where a real window is known. Task 4's
   `EffectiveLimit` is the only place this decides anything (Correction C6).
2. **Should the `Findings` category collect automatically?** Spec 22 ships
   findings, but the agent sees only the subset the user selects for a repair
   request, and that flow already works. **Assumption:** it stays reserved in
   #26 (proposal §5.11); the decision belongs with a follow-up, not with a
   pipeline rewrite. Task 4 only has to keep the slot and redistribute its share.
3. **May the `Instructions` category borrow from later categories?** A 10% share
   can truncate a large `AGENTS.md` hierarchy, and the instructions change the
   model's behaviour toward the user's own rules. **Assumption:** keep the
   proposal's rule (a category over its share truncates within itself), with
   nesting priority *inside* the category so the most specific file survives, and
   measure the eval fixture in Task 4. Borrowing would contradict an acceptance
   criterion; if the owner wants it, it is a change to that criterion first.
4. **Who owns the prompt-section ordering, #26 or #49?** `docs/specs/README.md`
   says #26, #55 and #49's reuse slice all rewrite `build_model_prompt` and that
   "#26 decides the ordering"; `docs/PLAN/OBSERVATIONS.md` entry 5 says #49 or
   #26. **Assumption:** #26 orders the sections and exposes them by name; #49's
   reuse slice places breakpoints. #26 emits none.

## Progress

| Task | State | Notes |
|---|---|---|
| 1 · Categories, ranges and one renderer | Done 2026-10-25 | **Landed:** `ContextCategory` (all 11 variants, derived `Ord` so declaration order *is* pipeline priority) and `ContextKind` (10 variants, `as_str()` returning today's exact prompt spellings, and `category()` as the single kind→category mapping). `ContextItem` gained `category`, `kind: ContextKind`, `range: Option<LineRange>` (serialised `lineRange`), `provenance: Option<Provenance>` and `root_path`; `LineRange` gained `Serialize`/`Deserialize`; `add_text`/`add_file` take a `ContextKind` and derive the category, so no caller can set them inconsistently. `ContextPlan::render_item` is the one header format (a range appends `path:start-end`), `sections()` returns the plan-owned blocks stable→volatile (`PromptSection`/`PromptSectionName`, each with `heading()`), and `included_paths()` is the Task 5 seam. `build_model_prompt` now takes `&ContextPlan` + `&SecretScanner` and `build_edit_prompt` takes `&ContextPlan`; neither takes the raw prompt. Chat order is `Repository context:` → `Recent conversation:` → `User request:`, the request is the redacted `Required` item's content, and each prior message is redacted *before* truncation. The edit flow keeps `User edit request:` first (a single call has no cache prefix to order) but renders through the same sections. Existing `item.kind` readers moved to `ContextKind` in `tests/repository_map.rs`, `tests/symbol_index.rs` and `tests/hooks.rs`; `tests/foundation.rs`'s assertions were prompt strings and stayed unchanged; chat's `approval_prompt_wording_tests` now builds a plan by hand. **Tests:** 10 new in `tests/context_assembly.rs` (range-less byte-identical, ranged header, section order, request once, chat and edit prompt redaction, replayed-turn redaction, oversized-prompt survival added by the review fix, chat/edit items identical, category order) and 3 serde unit tests in `context_manager.rs`. `cargo nextest run -p workspace-engine --test context_assembly --test foundation --test repository_map --test symbol_index --test hooks --test prompt_cache`: 404 passed, 3 skipped. `--lib -E 'test(context_manager)'`: 3 passed. `--lib -E 'test(chat::)'`: 70 passed. Scoped `cargo clippy -p workspace-engine --all-targets --locked -- -D warnings` clean; `scripts/quality-gate.sh fmt node lint typos deny` clean (deny included, no lock issue); `cargo run -p eval-harness -- run --tier deterministic`: all 23 scenarios pass, completion 1.000, check pass 0.724, 0 restricted-path/secret violations. **Falsified (all reverted, all caught):** (a) rendering `Required` in the context block as well as the request → `the_user_request_appears_once` (2 occurrences); (b) emitting the conversation block before the context → the order test; (c) changing the range-less header ` ---` to ` ----` → the byte-identical test; (d) overriding the request with a reintroduced raw `prompt` argument → the chat prompt-secret test printed the raw `AKIA…` key, and the same mutation in `build_edit_prompt` → the edit prompt-secret test; (e) using `message.content.clone()` instead of `scanner.redact` → the replayed-secret test printed the raw key. **Deviations:** (1) The request section is the `Required` item's content, not its `--- user_prompt ---` header — the heading already names it; proposal §5.10 records this. (2) The edit flow keeps request-first order; only chat moves to stable→volatile. (3) `sections()` returns only the two plan-owned sections; the conversation block is the caller's, because the plan holds no messages. (4) `ContextItem::range` keeps the proposal's field name and is serialised as `lineRange` (Task 1 Step 1's required serde name). **Discoveries for later tasks:** (1) `RetrievedFile` maps to `FileRanges`, so keyword/semantic hits currently land in `FileRanges`, not `SearchResults`; Task 2/4 must either add a `ContextKind` for them or let a collection source set the category explicitly — `add_text` deliberately has no category parameter today. (2) The legacy `command_context` block still renders after the request with its own `--- command_result ---` header; its only production call passes `None`, so it was left alone. (3) `tests/prompt_cache.rs` passed unchanged, so the new section order is still deterministic. **Review fix 2026-10-25:** the first cut rendered the request only from the `Required` item but left it subject to `add_text`'s budget check, so a prompt larger than `context_token_budget` produced an empty `User request:` — a regression against the old raw-prompt path. `add_text` now exempts `ContextCategory::Required` from the over-budget refusal while still redacting it and charging its tokens to `token_estimate`; it stays one code path, and a comment records that Task 4 replaces the exemption with the visible "Required item does not fit" error. New test `a_prompt_larger_than_the_budget_still_reaches_the_model_redacted` (16-token budget, prompt with a distinctive marker and an AWS key) asserts the marker and the redaction marker arrive and the raw key does not. **Falsified (reverted):** restoring the unconditional budget check makes that test fail on the empty `User request:`. Rerun: `--test context_assembly --test foundation --test prompt_cache` 179 passed; the six-file set 404 passed, 3 skipped; scoped clippy clean; `scripts/quality-gate.sh fmt typos` clean. |
| 2 · Collection sources and exclusion at collection time | Done 2026-10-25 | **Landed:** `ContextRequest<'_>` replaces the eight arguments and all eight call sites (chat, edit, `tests/hooks.rs`, the four `tests/repository_map.rs` calls and `tests/symbol_index.rs`'s helper); `build_context_with_hooks` is gone and `hook_context` is a field. `CollectionSource` has all ten variants and `ContextManager::collection_exclusion` matches it with no wildcard arm, using the engine's own `PathPolicy` (passed to `ContextManager::new` from `workspace_engine.rs`; the read-time check inside `read_file` is untouched). Every file-bearing source runs it in `add_file` *before* the read, so a restricted path is never opened. `ContextPlan` gained `excluded: Vec<ExclusionRecord>`; `ExclusionReason` currently has `RestrictedPath` only and records dedupe on `(reason, path)` because a mention is also a keyword hit. Keyword and semantic hits are separate lists added in that order and deduped by path. **Decision (C18):** a search hit is `ContextKind::RetrievedFile` with `ContextCategory::SearchResults`; explicit and mentioned files stay `ExplicitFile`/`FileRanges`. The header spelling is unchanged (`retrieved_file`), no prompt byte moved, and the mapping is still `ContextKind::category()` alone. `PlanState` comes from `SessionStore::read_task_plan` read once in `chat.rs` after `carry_plan_from_the_previous_turn`, rendered as numbered steps (status + detail, no evidence) and pushed through `add_text`; the edit flow passes `None`. The symbols item now carries the weakest provenance among the lines it actually shows (`max`, since `Heuristic` sorts after `LanguageServer`). **Tests:** 12 new in `tests/context_assembly.rs` (six per-source restriction tests plus the combined never-included one, the exhaustive `CollectionSource` walk, keyword-before-semantic ordering with a semantic-only fixture, keyword/semantic dedup, carried-plan item, symbols lowest provenance), 22 in the file. `cargo nextest run -p workspace-engine --test context_assembly --test hooks --test foundation --test symbol_index --test repository_map --test prompt_cache`: 416 passed, 3 skipped. `--lib -E 'test(context_manager) \| test(chat::)'`: 73 passed, 443 skipped. Scoped clippy clean for `workspace-engine`, `desktop-shell`, `damaian-cli` and `eval-harness`; `scripts/quality-gate.sh fmt node lint typos deny` clean; `cargo run -p eval-harness -- run --tier deterministic`: all 23 scenarios pass, completion 1.000, check pass 0.724, 0 restricted-path/secret violations. **Falsified (all reverted, all caught):** (a) `CollectionSource::Mention` returning no exclusion → `a_restricted_prompt_mention_is_excluded_at_collection` failed (the record came back as `SearchResults` from the keyword route) and `every_collection_source_refuses_a_restricted_path` failed; (b) adding semantic hits before keyword hits → `keyword_hits_are_ordered_before_semantic_hits` failed on `probe/semanticprobe_01.rs`; (c) dropping the `HookContext` arm from `collection_exclusion` → `error[E0004]: non-exhaustive patterns: CollectionSource::HookContext not covered`, so the test cannot compile. **Deviations:** (1) `ExclusionReason` is **not** re-exported at the crate root: `crate::repository_map` already exports a different `ExclusionReason`, and the two names collided. The context type keeps the interface name and is reached as `workspace_engine::context_manager::ExclusionReason` (Task 5's manifest should import it from there or rename one of the two deliberately). (2) The interface wrote `plan: Option<&'a Plan>`; the code's type is `Option<&'a TaskPlan>` (`crate::plan::Plan` does not exist), and the interfaces block is corrected. (3) `add_text` gained a `provenance` parameter and one targeted `too_many_arguments` allow with the same justification as `add_file`. (4) The combined `a_restricted_path_is_never_an_included_item` proves the invariant across routes, but the per-source tests are what pin each route: with a Mention check removed, the combined test still passes because the keyword source catches the path — which is exactly why both exist. **Discoveries for later tasks:** (1) `ContextPlan.excluded` is the Task 5 seam; Task 5 folds it into `ContextManifest`, adds the remaining `ExclusionReason` variants, and must not double-count the `(reason, path)` pairs this task dedupes. (2) Task 4 budgets `SearchResults` via `ContextKind::category()`; `RetrievedFile` is the only kind whose category changed, and `ExplicitFile` keeps `FileRanges`. (3) `add_file` now takes `(source, excluded)`; Task 3's ranged read belongs beside its `ReadWindow::Whole` call and should pass the same `source` so the collection check still runs before a merge read. (4) The map and symbol stores already filter restricted files; `collection_exclusion`'s `Map`/`Symbols` arms apply the policy for the exhaustive test but production never passes them a path, because both items are pathless `add_text` blocks. |
| 3 · Range deduplication and ranged reads | Done 2026-10-25 | **Landed:** `ReadWindow::ContextRange(LineRange)` in `file_access.rs`: same validation, path policy, restriction check, byte cap and redaction as `Range`, but no `max_read_lines` cap; `Range` still truncates. `RANGE_GUTTER = 2`, `FileCandidate`/`MergedFile`, and `merge_file_ranges` in `context_manager.rs`: group by first-seen path, sort by `(start, end)`, merge with `next.start <= current.end + 1 + GUTTER`, whole-file absorption (a whole-file candidate, or a path already included whole by an earlier stage), provenance = the minimum (`LanguageServer` sorts first), one `ContextRange` read per merged range through `flush_file_ranges`. `add_file` gained `range`/`provenance`/the range accumulator and still reads whole files immediately, so item order is unchanged; `add_text` gained the `range` argument and remains the only content path. Every range that did not become its own item is recorded as the new `ExclusionReason::DeduplicatedIntoRange`, and `push_exclusion` now keys on `(reason, path, range)` so two absorbed ranges for one path are two facts. **Discovery (recorded in proposal §5.4/§7): no collection source yields a `LineRange` today.** `indexer::SearchResult` carries a path and a snippet but drops the chunk offsets, and `symbol_index::Symbol` carries one line and is collected as one rendered block. Every source therefore passes `range: None`, and `flush_file_ranges` is a no-op in production. `no_collection_source_yields_a_line_range_yet` pins that, so the day a source supplies a range the machinery stops being unit-only and the test fails. The Step 1 merge tests are unit tests of `merge_file_ranges` rather than `build_context` integration tests for that reason; read-window behaviour is tested through `read_file`. **Security:** the model's tool path maps `Option<LineRange>` to `ReadWindow::Range` (`chat.rs`), and the CLI's own `read` stays `Whole` (a human command, unchanged, not the agent tool), so `ContextRange` is unreachable from the model; `the_agent_read_tool_cannot_request_an_uncapped_context_range` drives a 600-line tool range and pins 400. No exhaustive match broke at any of the three named sites: `chat.rs` matches `Option<LineRange>`, the CLI passes `Whole` explicitly, and `tests/agent_tools.rs` names the variants it reads. `a_context_range_read_still_refuses_a_restricted_path` and `a_context_range_read_still_redacts` go through `read_file`, so redaction is on the read range rather than a sliced `Whole` read. **Tests:** 10 new — 5 merge unit tests in `context_manager.rs` (`two_search_hits_in_one_function_send_one_merged_range`, `ranges_two_lines_apart_merge_and_ranges_eight_apart_do_not`, `a_whole_file_item_absorbs_every_range_for_its_path`, `merging_keeps_the_language_server_provenance`, `every_absorbed_or_merged_range_is_recorded`), 3 read-window tests and 1 tool-cap test in `tests/agent_tools.rs`, 1 finding test in `tests/context_assembly.rs`. Counts: `context_manager` lib 8, `agent_tools` 34, `context_assembly` 23. **Falsify (all reverted):** `GUTTER = 0` → the two-apart merge test fails; `GUTTER = usize::MAX` → the eight-apart test fails; `if included_whole || whole.is_some()` → whole-file absorption test fails; `ContextRange` capped like `Range` → the 600-line test stops at 400; tool dispatch `Range` → `ContextRange` → the tool-cap test returns all 600 lines; removing `assert_not_restricted` → the restricted context read opens `.env`; skipping `scanner.redact` for `ContextRange` → the redaction test sees empty content. **Checks:** `cargo nextest run -p workspace-engine --test context_assembly --test agent_tools --test foundation --test prompt_cache` (226 pass) and `--lib -E 'test(context_manager) \| test(file_access) \| test(chat::)'` (78 pass); scoped clippy for `workspace-engine`, `damaian-cli`, `desktop-shell`, `eval-harness`; `scripts/quality-gate.sh fmt node lint typos deny`; deterministic eval completion 1.000. Workspace-wide checks left to CI. |
| 4 · Per-category budgets and the effective limit | Done 2026-10-25 | **Landed:** `ContextBudget` (ten shares + `safety_margin_bps`, integer basis points, defaults exactly §5.2: instructions 1000, hook_context 500, pinned 1500, plan_state 500, repository_map 500, symbols 1000, search_results 1500, file_ranges 2500, findings 500, conversation_summary 500, margin 1000), `EffectiveLimit` (`declared`/`observed`; declared uses the config value unchanged with margin 0), `LimitSource`, and `CategoryUsage`. `ContextPlan` gained `limit` and `categories` in pipeline order (including `Required`); Task 5 folds both into the manifest. Config: eleven explicit `ConfigOverlay` fields, each `Capability` in the exhaustive macro; `context_budget.<category>`/`context_budget.safety_margin` parse decimals without a float (at most four places, at most `1.00`, otherwise an error); `apply_overlay_scoped` merges shares lower-only and the margin raise-only through `restrict_only_share`/`restrict_only_minimum`, refusing a repository raise or margin lowering as `RestrictOnly` by key; a sum above 10000 is `validate_context_budget`'s error, called at the end of `load_scoped`; both `to_policy_text`s print canonical four-decimal values; `split_profile_keys` carries all eleven; `rule_label` names each. `context_manager.rs`: collection now produces `PendingItem`s — text already redacted and measured by `add_text`, and the map/symbols blocks kept unrendered so the budget pass can choose their ceiling. `apply_budget` subtracts `Required` first (over-limit is `ClientError::InvalidInput` naming the tokens and the limit), allocates each category as `floor(available × share / 10000)` plus the previous category's unused carry, iterates `Instructions` by descending path depth (project rules last) and other categories in collection order, drops from the first item that does not fit with `CategoryBudgetExhausted`, and re-renders an over-share block at the remaining allocation through its own `render_for_model` and then `add_text`, recording `Truncated` (never slicing redacted text). Final items keep collection order, `files` is recomputed as the unique included paths in that order, and `add_text` no longer budgets, so Task 1's interim `Required` exemption and its comment are gone. `ContextManager::new` takes the budget; chat propagates the error with `?` and edit marks the task failed through `record_edit_failure`. Proposal §5.2, §5.7, §5.11, C6/C7/C8/C14 and the restated acceptance criterion already matched this implementation, so no prose change was needed. **Defaults:** exactly the §5.2 fractions, so the deterministic tier's completion rate (1.000) and check-pass rate (0.724) are unchanged; per-category usage of that tier is Task 7's §7 measurement. **Tests:** 7 new in `tests/context_assembly.rs` (share cap, redistribution, FileRanges drop order, semantic-before-exact, nested `AGENTS.md` survival plus root-before-nested prompt order, map re-render with `Truncated`, declared limit) for 30 in the file; 1 new config unit test (basis-point round trip, oversubscription, more-than-four-place rejection); 2 new tests and 11 weakening cases in `tests/permission_profiles.rs`; `tests/repository_config_trust.rs` and command-policy tests unedited. Task 1's `a_prompt_larger_than_the_budget_still_reaches_the_model_redacted` became `a_required_item_that_does_not_fit_is_an_error`, asserting the assembly, chat and edit errors and that no request reaches the provider. **Checks:** `cargo nextest run -p workspace-engine --test context_assembly --test prompt_cache --test hooks --test repository_map --test symbol_index --test agent_tools --test foundation --test permission_profiles --test repository_config_trust`: 580 passed, 4 skipped; `--lib -E 'test(config) \| test(profile) \| test(context_manager) \| test(chat::)'`: 83 passed, 439 skipped; `--lib -E 'test(command_policy)'`: 30 passed; `command_lifecycle` + `token_accounting`: 20 passed, 5 skipped. Scoped clippy clean for `workspace-engine`, `desktop-shell`, `damaian-cli` and `eval-harness`; `scripts/quality-gate.sh fmt node lint typos deny` clean; `npm run specs:check -- --strict` passed; deterministic eval: all 23 scenarios pass, completion 1.000, check 0.724, 0 restricted-path/secret violations (unchanged from Tasks 1–3). **Falsified (all reverted, all caught):** (a) `carry = 0` → `an_unused_share_moves_to_later_categories` failed on allocation 999 versus nominal 999; (b) category allocation = the whole limit → `a_category_over_its_share_drops_its_lowest_priority_members` kept the mentioned path; (c) `restrict_only_share` applying every value → `every_capability_key_resists_weakening_from_repository_scope` failed (all ten raises applied and the 19000-basis-point sum guard refused the load); (d) `EffectiveLimit::declared` subtracting 10% → the declared-limit test saw 14400 against 16000. **Deviations:** (1) `ContextPlan` gained `limit`/`categories` here rather than only in Task 5's manifest; Task 5 should move them into `ContextManifest` and keep `plan.files` as it is. (2) `ContextManager::new` has an eighth argument and a targeted `too_many_arguments` allow, the same pattern as `add_file`, with spec 27 expected to add a pinned store there. (3) Block truncation records carry `path: None` because the block has no file identity. (4) The canonical policy text prints four decimal places (`0.1000`), not the proposal's `0.10`; the parser accepts both. **Discoveries for later tasks:** (1) `EffectiveLimit::observed` is implemented but no capability-profile source calls it yet (Phase 1 WP3), so every live assembly is `Declared`; the margin is exercised only by its constructor until that profile lands. (2) Task 5 should fold `limit`/`categories`/`excluded` into the manifest without double-counting the `(reason, path, range)` records this task dedupes. (3) Task 7 must add the `context_budget.*` keys and their defaults to `USER_GUIDE.md`'s context section and `TROUBLESHOOTING.md`; the Settings UI edits raw config text, so `app.js` needs no key list and was left alone. (4) `Pinned`, `Findings` and `ConversationSummary` appear with zero use and release their shares; the redistribution is visible in `CategoryUsage.allocated`. |
| 5 · The manifest, its audit record, and the callers | Done 2026-10-25 | **Landed:** `ContextManifest` (repositoryId, taskId, assembledAtMs, limit, totalTokens, categories, included, excluded) and `IncludedRef` (category, kind, path, lineRange, tokens, rootPath, provenance) in `context_manager.rs`; `IncludedRef::from_item` is the one projection and carries no content. `ContextPlan` now carries `manifest` and no longer has `limit`/`categories`/`excluded`, per Task 4's discovery; `files` is unchanged and still the unique included paths in inclusion order. `apply_budget` builds the manifest through `build_manifest`, which stamps `now_millis()` and projects the final budgeted items in inclusion order. Both callers record one `context_assembled` event through `AuditLog::record` with actor, sessionId and `ContextManifest::audit_fields()` (repositoryId, taskId, tokenEstimate, manifest JSON): chat right after `build_context`, edit immediately before `edit_model_request_prepared`. `ContextManager` still holds no audit log, so there is no second audit path. **Step 3a:** `normalise_candidate` runs `PathPolicy::resolve_existing` (the read path's resolver, keeping `relative_path`) before the dedup, the collection check and the candidate push, so `./`, `..` and absolute in-repo spellings are compared as the read path sees them; `record_read_failure` turns a read-time policy refusal, size refusal or binary refusal into a record with a fixed (never raw) detail. `ExclusionReason` gained `TooLarge` and `Binary`. A missing or unreadable file, and a path resolving outside the root, have no reason in §5.5's list and stay unrecorded rather than carrying a fabricated one. Proposal §5.5 (audit fields, which enum variants #26 carries) and §5.6 (normalisation and read-failure records) updated. **Tests:** 10 new in `tests/context_assembly.rs` — included refs mirror items, the three producible exclusion reasons, no content in manifest or audit file, one event per chat turn, one per edit, `files` uniqueness/order, dot-prefixed and absolute restricted spellings, binary and oversized reads — for 42 in the file (two added by the review fix below), plus 1 unit test in `context_manager.rs` (`every_exclusion_reason_reaches_the_manifest`, 9 unit tests there). `cargo nextest run -p workspace-engine --locked --test context_assembly --test foundation --test hooks --test prompt_cache --test repository_map --test symbol_index --test agent_tools --test token_accounting`: 483 passed, 3 skipped; `--lib -E 'test(context_manager) \| test(chat::) \| test(audit)'`: 81 passed, 442 skipped. Scoped clippy for `workspace-engine`, `desktop-shell`, `damaian-cli` and `eval-harness` clean; `scripts/quality-gate.sh fmt node lint typos deny` clean; `cargo nextest run -p eval-harness --locked`: 89 passed, 2 skipped; deterministic eval: all 23 scenarios pass, completion 1.000, check 0.724, 0 restricted-path/secret violations (unchanged). **Falsified (all reverted, all caught):** (a) `IncludedRef::from_item` copying `item.content` into `path` → `no_manifest_field_contains_item_content` printed the content marker in the manifest; (b) removing chat's `context_assembled` record → `the_manifest_is_recorded_once_per_turn_as_context_assembled` saw 0 events; (c) `build_manifest` discarding `excluded` → `the_manifest_names_the_reason_for_every_exclusion` saw `[]`; (d) `normalise_candidate` returning the raw spelling → the dot test saw `./secrets/x.rs` and 0 normalised records, and the absolute test saw the absolute spelling (before Step 3a's second half it would have had no record at all); (e) removing `record_read_failure` from `add_file` → the Binary and TooLarge tests saw no records. **Deviations:** (1) `DeduplicatedIntoRange` cannot be produced through `build_context`: no collection source yields a `LineRange` (Task 3's discovery, pinned by `no_collection_source_yields_a_line_range_yet`), so the integration reason test covers the other three and the unit test covers every variant's carry-through and serde name. (2) The plan's dot-prefixed premise predates the code: `ignore::normalize_path` already strips `./` inside the rule matcher, so that spelling was refused at collection before this task; what was not normalised was the record path. The test therefore asserts the record names `secrets/x.rs`, and the absolute-path test is the one that produced no record before the fix. (3) No test was added to `tests/foundation.rs`: the chat/edit audit tests use `tests/context_assembly.rs`'s existing `ask`/`propose_edit` helpers and fixtures, and `--test foundation` passes unchanged. (4) `SecretBearing`, `TotalBudgetExhausted` and `UserRemoved` are not added: nothing in #26 produces them, so no record can claim them. (5) `context.token_estimate` remains on `ContextPlan` beside `manifest.totalTokens` because `edit.rs` prints it; both are the same item sum. **Discoveries for later tasks:** (1) `tests/prompt_cache.rs` passed unchanged, so `assembledAtMs` has not reached the prompt; the manifest is only serialised into the audit event. (2) `docs/TROUBLESHOOTING.md` does not list individual audit event names outside the per-feature sections; Task 7's "Reading a turn's context manifest" section is where `context_assembled` belongs. (3) `ContextItem.root_path` is now filled by spec 24's `RepositoryMap::root_for_path` (review fix below): every included file item carries its owning root, `Some("")` for the repository root (the map's own representation) and `None` only without a loadable map, for an outside-root explicit path, or for content with no file identity. (4) A missing/unreadable file and a path outside the root produce no exclusion record; making every attempt visible would need a reason the proposal does not have yet. **Review fix 2026-10-25 (acceptance §6: every included range carries its owning root):** `add_file` resolves `root_path` from the loaded `RepositoryMap` for every file item through `owning_root`, which returns `None` for an absolute outside-root path and otherwise `Some(root_for_path(path))` (`Some("")` at the repository root). `FileCandidate`/`MergedFile` carry the root, so a merged range's item inherits it. `load_map` no longer short-circuits when both render ceilings are 0: the map is loaded for root resolution regardless, and `load_or_build` reuses the stored map. The map block gets a clone so the map stays available for the files collected after it; map/symbols rendering is unchanged. `ExclusionRecord` gained no field (it has none for a root), so exclusion paths carry no root. Tests: `every_included_file_carries_its_owning_root` (a root `Cargo.toml` plus `packages/api/Cargo.toml`; `packages/api/src/lib.rs` → `Some("packages/api")`, `src/lib.rs` → `Some("")`, every `ExplicitFile`/`AgentInstruction`/`ProjectRule`/`RetrievedFile` item has a root and the manifest mirrors it) and `the_owning_root_is_set_even_with_the_map_item_turned_off` (both ceilings 0: no map/symbols item, roots still set); `the_manifest_lists_every_included_item_by_path_range_and_root` updated from `None` to `Some("")`. **Falsified (both reverted):** forcing `root_path = None` in `add_file` → the first test failed on the nested root; gating the map load on the two ceilings → the second failed with `root_path: None`. **Checks:** `cargo nextest run -p workspace-engine --locked --test context_assembly --test repository_map --test symbol_index --test prompt_cache --test foundation`: 330 passed, 2 skipped; scoped `cargo clippy -p workspace-engine --all-targets --locked -- -D warnings` clean; `scripts/quality-gate.sh fmt typos` clean; deterministic eval 23/23 scenarios, completion 1.000, check 0.724, 0 violations. |
| 6 · A content-aware estimate | Done 2026-10-25 | **Landed:** `TokenEstimate` (public, re-exported) with three provisional ratios — `Prose` 4/1, `Code` 7/2, `Structured` 3/1 bytes/token — and `for_item(kind, path)` choosing between them from the file extension (its own table; `language::detect_language` is left alone because it drives symbol extraction), `bytes_per_token() -> (u32, u32)` and `tokens()` = `ceil(len × denominator / numerator)` in `u128` integers, so no float decides inclusion. `add_text` now measures `TokenEstimate::for_item(kind, path.as_deref()).tokens(&redaction.text)` **after** redaction; that is the only production change (`apply_budget`'s block re-render already measures through `add_text`). `ModelAdapter::estimate_tokens` and spec 19 are untouched; `tests/token_accounting.rs` unedited and passing. **Provisional, labelled as such in code and proposal §5.8 (Task 7 replaces them):** prose 4 = today's ratio, which spec 19 §7 measured 2.5–6.3% high on DeepSeek; code 3.5 = the commonly cited ~3–3.5 bytes/token case for short identifiers and punctuation-heavy lines; structured 3 = quoted keys/braces/commas put a boundary at nearly every mark. Every ratio is ≤ 4, so classification can raise an estimate but never lower it. **Discovery (code comment + proposal §5.8):** `Symbols` and `RepositoryMap` items are path-less blocks whose own `render_for_model` fits them to a ceiling counted with `len / 4`; measuring that text at a denser ratio would report more tokens than the ceiling it was rendered to, so both keep the prose ratio until their renderers share the estimator (outside "change only inside `add_text`"). A file with no recognised extension is measured as code, because over-counting is the safe direction. **Discovery:** Task 4's two exact figures moved because `src/lib.rs` (32 bytes) is now 10 tokens, not 8 — `each_category_stops_at_its_configured_share`'s FileRanges share went 0.01 → 0.0101 so the explicit file still fits exactly and README.md is still dropped, keeping the test's intent; `an_unused_share_moves_to_later_categories` expects 10. **Tests:** 6 new ordinary in `tests/context_assembly.rs` (three types differ at 1,000 bytes: 250/286/334; identifier-heavy code above prose of equal length; monotone in length; never zero for non-empty; the assembly measures an item with its type's ratio while prose keeps `len / 4`; all 11 categories report `estimated`) and 2 `#[ignore]` calibration tests, both with the exact manual command and reason in their doc comments. `calibration_compares_estimates_with_recorded_usage` reads `DAMAIAN_CALIBRATION_DATA_DIR`'s `model_request_prepared.tokenEstimate` and `task_usage_recorded` input tokens, pairs them per (session, task) in file order, prints per-pair and aggregate ratios in integer basis points, reads no key, writes nothing, and prints how to set the variable when it is absent. `calibration_measures_bytes_per_token` sends three fixed 1–2 kB fixtures (prose, Rust, JSON) one per call through `OpenAICompatibleAdapter` + `CurlModelTransport`, resolves the key only via `env::var(&config.model_api_key_env)` exactly as the CLI does (no new key code, never printed/logged/stored), refuses the real application-support directory, asserts `UsageSource::Measured` and a 1–8 bytes/token band, and prints bytes/tokens plus what `len / 4` would have said. It was **not run**; it compiles and is reported skipped. **Scoped results:** `context_assembly` 48 passed + 2 skipped; the seven named files 457 passed + 5 skipped; `--lib -E 'test(context_manager) \| test(chat::)'` 79 passed. **Falsified (all reverted, all caught):** (a) every ratio `(4, 1)` → `prose_code_and_json_get_different_estimates_for_the_same_length` and `an_identifier_heavy_snippet_is_estimated_higher_than_prose_of_the_same_length` failed; (b) `tokens()` returning the constant 1 → `the_estimate_is_monotone_in_length` failed on "must not be flat in length"; (c) `tokens()` returning 0 → `the_estimate_is_never_zero_for_non_empty_content` failed; (d) `add_text` back to `redaction.text.len().div_ceil(4)` → `the_assembly_measures_each_item_with_its_content_type_ratio` failed 8 vs 10, which is the only mutation that catches the wiring. **Eval:** `cargo run -p eval-harness -- run --tier deterministic` — all 23 scenarios pass, completion 1.000, check pass 0.724, 0 restricted-path/secret violations, token total 397245, tool and model error rate 0.119, model calls 4.043. Re-running with only the `add_text` line reverted produced identical figures, so **no metric moved**: the harness's token metric comes from the model adapter's own estimate, and no fixture's inclusion changed at the 16k limit. **Calibration fix 2026-10-25:** the owner's live run against `deepseek-v4-flash` failed with `Io("Model provider returned no assistant content")` because `max_tokens: Some(16)` was consumed by hidden reasoning before any content; the live test now asks for 1024 output tokens, with a comment saying why, and nothing else in it changed (only input usage is calibrated). **Checks:** `scripts/quality-gate.sh fmt node lint typos deny` ok; scoped clippy for `workspace-engine`, `desktop-shell`, `damaian-cli`, `eval-harness` ok. Workspace-wide clippy and tests left to CI. Calibration runs belong to Task 7. **Superseded by Task 7 (2026-10-25):** the ratios are now the measured code 18/5 (3.6) and structured 12/5 (2.4), with prose unchanged at 4/1, and the exact provisional figures quoted above (286/334 at 1,000 bytes, `src/lib.rs` at 10 tokens, the FileRanges share 0.0101 and the `[250, 286, 334]` assertion) moved with them; Task 7's row records the new values. The ignored live test's `max_tokens` is 1024, not 16. |
| 7 · Calibration, docs and close-out | Done 2026-10-25 | **Landed:** the docs and the acceptance run (Steps 3–4, 2026-10-25), the owner's live calibration (Step 2, 2026-10-25), and the close-out (Step 5). **Step 1 (offline calibration) was not run:** the owner did not provide a `DAMAIAN_DATA_DIR`, so `calibration_compares_estimates_with_recorded_usage` was not run and no distribution is invented; proposal §7 records the gap. **Step 2 (live):** the owner ran `calibration_measures_bytes_per_token` against provider `deepseek`, model `deepseek-v4-flash`, one call per document. Measured: prose 1130 bytes / 251 input tokens / 4.50 B/token (`len / 4` 283, +13% over); rust 1332 / 369 / 3.61 (333, −10% under); json 1037 / 424 / 2.45 (260, −39% under). The provider's input count includes ~15 tokens of instruction and chat-template overhead, so the constants over-count slightly; it is one sample per type from one provider. Chosen ratios: code `(18, 5)` = 3.6, structured `(12, 5)` = 2.4, prose `(4, 1)` unchanged — `len / 4` measured 13% high on prose, consistent with spec 19 §7, and keeping 4 preserves Task 6's rule that classification never estimates below `len / 4`, with the headroom standing in for the margin the Declared path does not subtract. `Map`/`Symbols` stay on prose. §5.8 now records the measured table and drops the provisional label for code and structured. **Tests whose figures moved (intent kept):** `prose_code_and_json_get_different_estimates_for_the_same_length` — 1,000 bytes now 250/278/417 instead of 250/286/334; `each_category_stops_at_its_configured_share` — `src/lib.rs` is now 9 tokens (32 × 5/18), so the FileRanges share was retuned 0.0101 → 0.0091 to keep the exact-fit-and-README-dropped intent (`allocated` and `used` both 9); `an_unused_share_moves_to_later_categories` — `used` 10 → 9. `the_assembly_measures_each_item_with_its_content_type_ratio` needed no edit: it asserts code > `len / 4` and prose == `len / 4`, both still true. **Test-only fix:** the live test's print passed a hundredths-scale value to the percent formatter (`format_basis_points`) and printed `45.00`; it now computes hundredths and prints two decimals (`4.50`) without a float. **Step 3 (docs):** `USER_GUIDE.md` **What goes into context** under *Output and Context Limits* (section and collection order, edit-flow variant, prompt/replayed-turn redaction, all eleven categories with real default shares, redistribution and within-category truncation, all `context_budget.*` keys and defaults, the `1.00` sum error, repository lower-share/raise-margin-only plus the same profile limit, and the visible `Required context needs N tokens but the context budget is M` failure); common-keys table and repository-config layering rule updated. `TROUBLESHOOTING.md` **Reading a turn's context manifest** (where `context_assembled` sits for chat and edit, a `jq` recipe, every event and manifest field including `user_prompt` and `rootPath: ""`, every producible `ExclusionReason` with `deduplicated_into_range`/`secret_bearing`/`total_budget_exhausted`/`user_removed` marked not produced, `lineRange` and `provenance` stated as they actually are, Declared vs Observed, what the limit excludes, four `category_budget_exhausted` remedies, symptom-index row, event-type table row, cross-reference from *A file the model should see is missing from context*). `SECURITY.md` now states that the prompt and replayed turns are secret-scanned before they reach the model, and why storage stays verbatim (C17). **Step 4 (acceptance):** run before and after the ratio change; all 23 scenarios pass, completion 1.000, check pass 0.7241 (21/29), 0 approval-policy, restricted-path/secret and unrelated-file violations, recovery 1.000, tool/model error rate 0.1190, 4.043 calls per task, 397245 tokens both times. Compared by hand with `evals/baseline.json` (16 scenarios then, 23 now) and recorded in proposal §7: moved aggregates are explained by the seven added scenarios and the 55 `chat.rs` commits since the baseline; the spec's own effect was measured against the pre-implementation tree (every count and rate identical, tokens 399391 → 397245, −0.54%, lower in all 23 scenarios). The ratio change moved nothing observable: the fixtures fit their shares either way and the harness counts the adapter's whole-request `len / 4` estimate, not the item estimator; every `context_contains`/`context_excludes`/`context_ranks_within` string is unchanged. **Step 5 (close-out):** proposal `Status:` → Done 2026-10-25; this header and every row; `docs/specs/README.md` #26 implementation row, **Current state** bullets, a dated History entry, re-derived **What to build next** and **Parallel work** (#26 is no longer in flight, so the `build_model_prompt` pairing rule now binds #55, #49's reuse slice, #65 and #66); `Depends on:` lines for #27, #28, #33, #51, #54 and #55 → built (Done 2026-10-25); `CHANGELOG.md` `Unreleased` entry removed. Promoted: #27, #33, #51 and #55. #54 and #49's reuse slice stay out: their own `Depends on:` lines still name Phase 1 WP3 as **unspecified**, and #49's reuse slice remains the spec's only unbuilt requirement. #28 stays blocked on #27. **Checks:** `cargo nextest run -p workspace-engine --locked --test context_assembly --test foundation --test prompt_cache --test token_accounting` → 232 passed, 2 skipped (the ignored calibration tests); the same run over `hooks`, `repository_map`, `symbol_index`, `agent_tools` and `permission_profiles` → 332 passed, 4 skipped; `cargo clippy -p workspace-engine --all-targets --locked -- -D warnings` clean; `scripts/quality-gate.sh fmt node lint typos deny` clean; `npm run specs:check -- --strict` passed; deterministic eval 23/23. **CI:** the full seven-check gate passed in CI on 2026-10-25 (Quality run 40204226481, on `main` at `8758f62`); workspace clippy and the workspace suite were not run locally, per `AGENTS.md`, so that run is their record. **Known open points (proposal §7, not blockers):** no collection source yields a `LineRange`, so range dedup and line provenance are implemented and unit-tested but inert; no Observed capability profile exists, so every limit is Declared and the configured margin never changes a live limit; Step 1 was not run; redistribution frequency and per-category eval usage were not measured. |

## Global Constraints

Every task's requirements implicitly include this section.

- **Read [`context.md`](context.md) in full before Task 1**, then §2, §3 and §5
  of [`proposal.md`](proposal.md) and the Corrections table below. Where a
  correction contradicts the proposal's prose, the correction wins, and the task
  that acts on it updates the proposal in the same change (they were folded in
  while planning; check §2, §5 and §6 agree after each task).
- **Secret redaction runs before any item is measured or stored.** `add_text`
  (`context_manager.rs:410-440`) is the only place an item's content is created,
  and `scanner.redact` stays ahead of the token estimate and the budget test in
  it. No new path may produce item content: a collection source yields paths and
  ranges, and the one reader redacts. Nothing bypasses `SecretScanner`, and
  `SecretScanner` itself is not changed.
- **Nothing unredacted reaches the provider.** The user's prompt and the
  prior-conversation block are not items, and today neither is redacted (C17):
  the request renders from the redacted `Required` item, never from the `prompt`
  argument, and each prior message passes through the orchestrator's
  `SecretScanner` before the conversation block is written. A test seeds a token a
  `SecretScanner` detects into the prompt and into an earlier turn and asserts the
  raw value is absent from the built request.
- **The manifest never contains content.** `ContextManifest` and everything it
  holds — included refs, exclusion records, category usage — carry paths,
  ranges, counts, reasons and enum names only. A test asserts that a seeded
  secret and a content marker appear in no manifest field.
- **`path_policy` is applied at collection time in every source and again at
  read time** (proposal §5.6). Every file-bearing source has its own test
  asserting that a restricted path becomes a `RestrictedPath` exclusion record
  and never an included item. The source list is an exhaustive enum (no wildcard
  arm), so a new source that skips the check fails to compile *and* fails
  `every_collection_source_refuses_a_restricted_path`.
- **A new config field must be classified explicitly.** The `context_budget.*`
  fractions and `context_budget.safety_margin` join `ConfigOverlay`,
  `classify_overlay_fields!` (exhaustive, `config.rs:2787-2795`),
  `apply_overlay_scoped`, both `to_policy_text`s and `split_profile_keys`
  (exhaustive, `profile.rs:322`). **Decision: `Capability`, restrict-only at
  repository scope.** A share steers model traffic toward or away from a
  repository's own content, and spec 34's rule is that a clone may tighten a
  bound but never redirect or widen one; so repository scope may lower a share
  and raise the safety margin, never raise a share or lower the margin. Profile
  scope keeps only the restrictive direction the other numeric caps take.
  `tests/permission_profiles.rs` gains the weakening cases; a repository-scope
  raise must be refused as `RestrictOnly`, never silently clipped.
- **No command-policy change.** `permission_profiles`, `command_policy`'s unit
  tests and `repository_config_trust` pass unchanged after every task — the last
  two are never edited. If a task finds itself wanting to change *which*
  commands run or ask, stop and say so.
- **No new dependency.** serde is already present in `workspace-engine`; basis
  points need nothing.
- **Tests that build a temp repository set `enable_index_watcher: false`** and
  their own `data_dir` inside the fixture (reuse `foundation.rs:55`'s
  `test_config`). Never write into the user's real Damaian data directory.
- **The prompt stays deterministic.** `tests/prompt_cache.rs`'s
  `two_assemblies_of_an_unchanged_repository_are_identical` must pass after every
  task, unchanged: no timestamp, hash iteration order, filesystem order, or float
  rounding may reach the rendered prompt or decide which item is included.
  `ContextManifest.assembledAtMs` is the one time-dependent field; the manifest
  is only written to the audit record and is never compared.
- **`ContextPlan.files` keeps meaning "unique included paths, in inclusion
  order".** The eval harness reads it as `context_files` and asserts
  `context_contains`, `context_excludes` and `context_ranks_within` against it.
- **A private Cargo target directory per worktree.** Prefix every `cargo`
  command with `CARGO_TARGET_DIR="$PWD/target"` from the worktree root.
- **Scoped checks per task; never workspace-wide.** Run
  `scripts/quality-gate.sh fmt node lint typos deny`,
  `cargo clippy -p workspace-engine --all-targets --locked -- -D warnings` (plus
  `-p desktop-shell` or `-p damaian-cli` in a task that touches them), and the
  named test files. **Never run `cargo clippy --workspace` or
  `cargo nextest run --workspace` in any task** (AGENTS.md, since 2026-10-15);
  the full seven-check gate runs on CI.
- **Falsify every load-bearing test.** Break what it guards, confirm the named
  test fails, revert, and record the mutation in the Progress row.
- **Never `git commit`, `git push`, stash or switch branches unasked.** Each
  task ends by showing the change and the scoped check results. One-line commit
  subject, no body.

## Corrections to the proposal found while planning

Each one names the task that owns it. All are folded into
[`proposal.md`](proposal.md) already; this table is why.

| # | Proposal said | Code says | Resolution |
|---|---|---|---|
| C1 | `build_context` at `context_manager.rs:66`, `add_text` at `:248`, the estimate at `:261`, redaction at `:260`, `ContextItem` at `:22-29`, `ContextPlan` at `:31-38`, `agent_instruction_paths` at `:280-309`, `context_token_budget` at `config.rs:115`, `estimate_tokens` at `model.rs:209` | `:96`, `:410`, `:423`, `:422`, `:25-32`, `:34-41`, `:442-470`, `config.rs:1925-1934`, `model.rs:354` | §2 and every section that cites a line corrected. The estimate is `len.div_ceil(4)`, not `len / 4`. **Task 1** |
| C2 | "Nothing is deduplicated" (§1, §2) | `add_file` skips a path already in `files` (`context_manager.rs:378`), so whole files are deduplicated today; ranges do not exist, so nothing *can* overlap | §1 corrected: the problem is that content is whole-file, plus the whole-file-absorbs-range case that ranges will introduce. §5.4 restated. **Task 3** |
| C3 | `kind` becomes the `ContextCategory` enum (§5.1) | `kind` is the model-facing source label: the prompt header prints it, and `foundation.rs:4354`, `repository_map.rs:1855-1858`, `symbol_index.rs:3377-3379` and `hooks.rs:3019` assert the strings `agent_instruction`, `retrieved_file`, `explicit_file`, `project_rule`, `hook_context`, `user_prompt` | Two fields: `category: ContextCategory` for budget and priority, `kind: ContextKind` for the header, with today's spellings. §5.1 corrected. **Task 1** |
| C4 | A new `LineRange { start: u32, end: u32 }` (§5.1) | `crate::file_access::LineRange` already exists (`file_access.rs:16`), is `usize`, 1-based inclusive, re-exported at `lib.rs:103`, and is what `ReadWindow::Range` takes | Reuse it. §5.1 corrected. **Task 1** |
| C5 | "Re-read the merged range once" (§5.4) | `ReadWindow::Range` is clamped to `max_read_lines` (400) at `file_access.rs:184-189`, deliberately; `Whole` opts out but returns redacted text whose line structure redaction may have changed | Add `ReadWindow::ContextRange(LineRange)`: no line cap, byte cap, path policy and redaction unchanged. Slicing a `Whole` read is rejected for the redaction reason. §5.4 corrected. **Task 3** |
| C6 | The declared limit is `context_token_budget` minus a 10% margin (§5.7, acceptance) | `context_token_budget` is documented as a *packing* budget (`config.rs:1920-1924`) and defaults to 64k against a 1M window (`:3843-3848`); a margin on it shrinks real context for no safety gain | Declared path uses the value unchanged (`safety_margin_bps = 0`); the margin applies to an observed window only. Acceptance criterion restated; the alternative is open question 1. **Task 4** |
| C7 | `safety_margin: f32` and decimal fractions (§5.2, §5.7) | `ContextItem` and `ContextPlan` derive `Eq` and tests compare plans; a float costs `Eq` and makes the last token's allocation depend on rounding | Fractions and the margin are parsed as decimals and stored as integer basis points (`u16`), sum `<= 10_000`, oversubscription is a config error. §5.2, §5.7 corrected. **Task 4** |
| C8 | Most category slots have a source and are simply budgeted (§5.2) | `Pinned` needs #27's storage, `ConversationSummary` needs #55, and `Findings` reaches the model only as the user-selected repair subset (spec 22). Only `PlanState` (on a resumed turn) and `HookContext` (with a hook configured) have real sources today | `PlanState` and `HookContext` are wired in #26; `Pinned`, `Findings` and `ConversationSummary` stay reserved (proposal §5.11), appear with zero use, and their shares redistribute. **Tasks 2, 4** |
| C9 | The manifest is "recorded through `AuditLog::record`" (§5.5) | `ContextManager` holds no `AuditLog` (`context_manager.rs:43-51`); both orchestrators already have one and already record `model_request_prepared` there (`chat.rs:2927`) | The manifest travels inside `ContextPlan`; the caller records one `context_assembled` event. Adding an audit log to the manager would be a second audit path for one event. **Task 5** |
| C10 | §5.6 applies path policy at collection time "in every source" | `ContextManager` holds no `PathPolicy` either; the policy lives in the private `FileAccessController` field and in `SymbolSource` | `ContextManager::new` gains the engine's `PathPolicy` (`workspace_engine.rs:88-117`). Same instance, not a second rule. **Task 2** |
| C11 | §5.4's dedup and §5.5's exclusions are all the manifest needs | Acceptance says every *dropped or truncated* item is recorded, and a block trimmed to fit is neither dropped nor `TooLarge` | `ExclusionReason::Truncated` added. §5.5 corrected. **Task 5** |
| C12 | Eight positional arguments to `build_context` | Plan state and findings come from `SessionStore`, not the index, and there are already `too_many_arguments` allowances at `context_manager.rs:95` | One `ContextRequest<'_>` struct; `build_context_with_hooks` is removed and its two production callers plus its one test are updated. **Task 2** |
| C13 | §5.3 orders `Symbols` by provenance, then match quality | `SymbolIndex::find_for_prompt` already sorts exactly that way (`symbol_index.rs:519-535`), and the symbols item is one rendered block with the hedge inside its text (`:545-591`) | #26 does not re-sort. The item's `provenance` is the lowest provenance it contains. §5.3 corrected. **Task 2** |
| C14 | A category over its share drops items (§5.2) | The map and symbols blocks have their own ceilings (800, 1,600) and `add_text` drops a block that does not fit in one go, so a smaller category share would drop the whole block | A block item over its share is re-rendered at the smaller ceiling (`render_for_model` already takes one); only a block that cannot render one line is excluded. §5.2 corrected. **Task 4** |
| C15 | `build_model_prompt` produces the prompt; the edit flow is separate (§5.9 mentions only docs) | `build_edit_prompt` (`edit.rs:1086-1105`) is a second copy of the item renderer, so the header format can drift | One renderer on `ContextPlan`; the two callers keep their own preambles. §5.10 added. **Task 1** |
| C16 | The prompt's shape is out of scope (never stated) | The sections are ordered conversation → request → context, the reverse of a cacheable prefix, and the user prompt is sent **twice** (`context_manager.rs:139-147` and `chat.rs:6126-6127`). #55 §2 and #49 `context.md` §3.2 both assign the ordering to #26 or #49; `OBSERVATIONS.md` entry 5 says the same | §5.10 added: stable-to-volatile order, `Required` rendered under the final `User request:` heading so the request appears once, sections named for #49. No breakpoints emitted. **Task 1** |
| C17 | §5.10 said the request moves out of the renderer but never said what it is rendered *from*; nothing claimed the prompt was redacted | Nothing redacts the user's prompt on any path into the engine (shell `lib.rs:2270`, CLI `main.rs:516/532`, harness `runner.rs:368`): `build_model_prompt` writes the raw `prompt` (`chat.rs:6126-6127`), `build_edit_prompt` the same (`edit.rs:1088-1089`), and the `Recent conversation:` block writes each stored message raw (`chat.rs:6117-6124`) with the transcript stored verbatim by spec 16. The duplicate `user_prompt` item *is* redacted (`context_manager.rs:422`), so one copy of the prompt is clean and the other is not | The request renders from the redacted `Required` item and every prior message passes through `SecretScanner` before it is written. No fallback to the raw argument. §5.10 and §6 corrected. Evidence in `context.md` §2. **Task 1** |
| C18 | §5.1's kind→category table mapped `RetrievedFile` to `FileRanges` | §5.3 orders and budgets search hits as `SearchResults` and drops semantic hits first, which `FileRanges` (explicit and mentioned paths) does not describe; Task 1 left `RetrievedFile` on `FileRanges` and flagged the conflict | `RetrievedFile` → `SearchResults`, `ExplicitFile` → `FileRanges`. The header still prints today's `retrieved_file`, so the prompt is unchanged. §5.1 corrected. **Task 2** |

## File Structure

| File | Change |
|---|---|
| `crates/workspace-engine/src/context_manager.rs` | `ContextCategory`, `ContextKind`, `ContextItem` fields, `ContextPlan` manifest and renderer (Task 1); `ContextRequest`, collection sources, collection-time policy (Task 2); dedup and merge (Task 3); `ContextBudget`, allocation, truncation (Task 4); manifest filling (Task 5); estimate (Task 6) |
| `crates/workspace-engine/src/file_access.rs` | `ReadWindow::ContextRange` only (Task 3). Path policy, byte cap and redaction untouched |
| `crates/workspace-engine/src/config.rs` | `context_budget.*` parse, defaults, overlay field, restrict-only merge, policy text (Task 4) |
| `crates/workspace-engine/src/profile.rs` | `split_profile_keys` carries the two restrictive directions (Task 4) |
| `crates/workspace-engine/src/effective_policy.rs` | `rule_label` for the new keys (Task 4) |
| `crates/workspace-engine/src/workspace_engine.rs` | Pass the engine's `PathPolicy` and the budget to `ContextManager::new` (Tasks 2, 4) |
| `crates/workspace-engine/src/chat.rs` | `build_model_prompt` uses the shared renderer and the new section order (Task 1); `build_context` call site, `context_assembled` record (Tasks 2, 5); plan handed to assembly (Task 2) |
| `crates/workspace-engine/src/edit.rs` | `build_edit_prompt` uses the shared renderer (Task 1); call site, manifest record (Tasks 2, 5) |
| `crates/workspace-engine/src/lib.rs` | Re-export the new context types (Tasks 1, 4, 5) |
| `crates/workspace-engine/tests/context_assembly.rs` | New. Every task adds to it |
| `crates/workspace-engine/tests/{foundation,repository_map,symbol_index,hooks}.rs` | Only where a `kind` string or an item position moved; no assertion weakened (Task 1) |
| `crates/workspace-engine/tests/permission_profiles.rs` | One weakening case per new capability key, plus a repository and a profile direction test (Task 4) |
| `crates/workspace-engine/tests/agent_tools.rs` | The new `ReadWindow` variant exhausts a match here (Task 3) |
| `docs/USER_GUIDE.md`, `docs/TROUBLESHOOTING.md` | Task 7 |
| `docs/specs/README.md`, `CHANGELOG.md`, specs naming #26 | Task 7 (close-out) |

## Interfaces

Task 1 lands the types; later tasks consume them. Names are a contract between
tasks; fields may grow, but a rename is a rebase across tasks.

```rust
// context_manager.rs
pub enum ContextCategory { Required, Instructions, Pinned, PlanState,
    RepositoryMap, Symbols, SearchResults, FileRanges, Findings,
    ConversationSummary }

pub enum ContextKind { UserPrompt, ExplicitFile, AgentInstruction, ProjectRule,
    HookContext, RepositoryMap, Symbols, RetrievedFile, PlanState, Pinned }

pub struct ContextItem { category, kind, path: Option<String>,
    range: Option<LineRange>, content: String, tokens, redaction_status,
    provenance: Option<Provenance>, root_path: Option<String> }

pub struct ContextRequest<'a> { repository_root: &'a Path, repository_id: &'a str,
    task_id: &'a str, prompt: &'a str, index: Option<&'a RepositoryIndex>,
    explicit_paths: &'a [String], limit: usize, hook_context: &'a [(String, String)],
    plan: Option<&'a TaskPlan> }

pub struct ContextPlan { /* today's fields */ limit: EffectiveLimit,
    categories: Vec<CategoryUsage>, excluded: Vec<ExclusionRecord> }
// Task 2 fills `excluded`, Task 4 `limit` and `categories`; Task 5 folds them
// into `ContextManifest`.

impl ContextPlan {
    pub fn sections(&self) -> Vec<PromptSection>;   // stable → volatile
    pub fn render_item(item: &ContextItem) -> String; // the one header format
    pub fn included_paths(&self) -> &[String];      // unique included paths
}

pub enum LimitSource { Observed, Declared }
pub struct EffectiveLimit { tokens, source, safety_margin_bps: u16,
    window: Option<usize> }
/// One share per budgeted category; `Required` has none.
pub struct ContextBudget { shares: [u16; 10], safety_margin_bps: u16 }
/// `estimated` is always true for assembly and exists so a figure can never be
/// read as measured (spec 19 requirement 4); a post-send view may set it false.
pub struct CategoryUsage { category, allocated, used, items, truncated,
    estimated: bool }
pub struct IncludedRef { category, kind, path: Option<String>,
    range: Option<LineRange>, tokens, root_path, provenance }
pub struct ExclusionRecord { reason: ExclusionReason, path: Option<String>,
    range: Option<LineRange>, category, detail: Option<String> }
pub struct ContextManifest { repository_id, task_id, assembled_at_ms, limit,
    total_tokens, categories, included, excluded }

pub enum CollectionSource { Explicit, Mention, KeywordHit, SemanticHit,
    Instruction, ProjectRule, Map, Symbols, HookContext, PlanState }
```

`CollectionSource` exists so §5.6's per-source check is exhaustive: the assembler
matches on it with no wildcard arm, and one test walks every variant.

`ContextRequest` is a struct rather than eight parameters for two reasons: the
argument list is already at the `too_many_arguments` limit, and spec 27 adds a
`pinned` field to it without touching what either caller passes for everything
else. #26 populates none.

## Order and parallelism

```
1 ──> 2 ──> 3 ──> 4 ──> 5 ──> 6 ──> 7
```

Sequential; Tasks 1–5 all edit `context_manager.rs`, and Tasks 1–2 and 5 also
edit `chat.rs`. **No task may run beside #55, #49's reuse slice, #65 or #66**:
all of them edit `build_model_prompt`, and #65/#66 also edit the turn loop
(`proposal.md` §5.10; `docs/specs/README.md`, "Parallel work"). Nothing else in
flight edits `context_manager.rs`, `file_access.rs` or the four config files.

---

## Task 1: Categories, ranges and one renderer

**Requirements:** 4 (the shape provenances and ranges travel in); proposal §5.1,
§5.10. **Corrections:** C1, C3, C4, C15, C16, C17.
**Files:** `context_manager.rs`, `chat.rs`, `edit.rs`, create
`tests/context_assembly.rs`, update the four test files that read `item.kind`.

No budgeting and no collection change yet: this task changes the shape of an
item, makes the range expressible, makes one renderer the only place a header is
written, and closes the two places where unredacted text reaches the provider
(C17).

- [x] **Step 1: Write the failing tests** in
      `crates/workspace-engine/tests/context_assembly.rs`, with `temp_dir`,
      `write_fixture` and `test_config` copied from `foundation.rs:20-61`
      (`enable_index_watcher: false`, `data_dir` inside the fixture).
  - `a_range_less_item_renders_exactly_as_it_does_today` — a `RetrievedFile`
    with path `src/lib.rs` renders `\n--- retrieved_file: src/lib.rs ---\n…`
    byte for byte, and an item with no path renders `\n--- hook_context: hook:x ---\n…`;
  - `a_ranged_item_renders_its_line_range` —
    `--- retrieved_file: src/lib.rs:12-40 ---`, 1-based inclusive;
  - `the_prompt_orders_context_before_conversation_before_the_request` — the
    three section headings appear in that order exactly once each;
  - `the_user_request_appears_once` — a distinctive prompt string occurs exactly
    once in the built prompt (it is currently twice);
  - `a_secret_in_the_user_prompt_is_redacted_in_the_model_request` — seed a token
    the `SecretScanner` detects (the `AKIAIOSFODNN7EXAMPLE` style the eval
    fixtures use) into the prompt, build a turn with `MockModelAdapter`, and
    assert the request's user message contains the scanner's marker and not the
    raw token. **It fails today**, because nothing redacts the prompt on any path
    into the engine (`context.md` §2, "The user's prompt is not redacted on any
    path") — so it is a fix, not only a guard. Assert it for the edit flow too
    (`build_edit_prompt`), which has the same hole;
  - `a_secret_in_an_earlier_turn_is_redacted_in_the_recent_conversation_block` —
    turn one's prompt holds the token; turn two of the same session renders it
    with the marker, not the token, because stored messages are verbatim
    (spec 16) and the renderer must not replay them raw;
  - `the_chat_and_edit_prompts_render_items_identically` — `ContextPlan::render_item`
    is what both call, asserted by rendering the same plan through both builders;
  - `a_category_orders_before_the_categories_after_it` — `Ord` over the eleven
    variants is the pipeline order (one test, exhaustive array, no wildcard);
  - unit tests in `context_manager.rs` for the serde names (`snake_case` for
    categories and kinds, `camelCase` for item fields, `lineRange`).
  In the four existing files, replace `item.kind == "retrieved_file"` with
  `item.kind == ContextKind::RetrievedFile` etc.; the prompt-string assertions
  stay as they are.
- [x] **Step 2: Run them and confirm they fail** on the missing types:
      `cargo nextest run -p workspace-engine --test context_assembly`.
- [x] **Step 3: Implement.** Add the two enums (each with `as_str`, `ContextKind`'s
      returning today's exact spellings), the three new fields (each `None` until a
      later task fills it), `PromptSection`, `render_item` and `sections()`; make
      `build_model_prompt` and `build_edit_prompt` use it.
      Order the sections `Repository context:` → `Recent conversation:` →
      `User request:`, and render `Required` items under the final heading. The
      request comes **only** from the `Required` item, which `add_text` has
      already redacted; the `prompt` argument is not rendered at all, and there is
      no fallback to it. Pass each prior message's content through the orchestrator's
      `SecretScanner` before writing the conversation block (C17). Keep the item
      text and the trailing-newline behaviour identical.
- [x] **Step 4: Falsify.** (a) Render the request as a section *and* as a
      `Required` item → `the_user_request_appears_once` fails; (b) restore the
      conversation-first order → the order test fails; (c) change the range-less
      header at all → the byte-identical test fails; (d) render the raw `prompt`
      argument instead of the `Required` items → the prompt-secret test fails and
      the raw token appears in the request; (e) write the conversation block
      without the scanner → the replayed-secret test fails. Revert each.
- [x] **Step 5: Update the proposal** for C1, C3, C4, C15, C16 and C17 (already
      folded in; confirm §2, §5.1, §5.10 and §6 agree).
- [x] **Step 6: Scoped checks.** `cargo nextest run -p workspace-engine --test
      context_assembly --test foundation --test repository_map --test symbol_index
      --test hooks --test prompt_cache`, `cargo nextest run -p workspace-engine
      --lib -E 'test(context_manager)'`, scoped clippy for `workspace-engine`,
      and the fast checks. Then `cargo run -p eval-harness -- run --tier
      deterministic` — the prompt order moved, so this is the first baseline.

## Task 2: Collection sources and exclusion at collection time

**Requirements:** 5; proposal §5.3 (separation), §5.6, §5.11 (PlanState).
**Corrections:** C8, C10, C12, C13.
**Files:** `context_manager.rs`, `workspace_engine.rs`, `chat.rs`, `edit.rs`,
`tests/context_assembly.rs`, `tests/hooks.rs` (call-site signature only).

- [x] **Step 1: Write the failing tests.**
  - `a_restricted_explicit_path_is_excluded_at_collection`,
    `a_restricted_prompt_mention_is_excluded_at_collection`,
    `a_restricted_keyword_hit_is_excluded_at_collection`,
    `a_restricted_semantic_hit_is_excluded_at_collection`,
    `a_restricted_instruction_path_is_excluded_at_collection` (fixture with
    `secrets/AGENTS.md` and a prompt mentioning `secrets/x.rs`),
    `a_restricted_project_rule_is_excluded_at_collection` (a fixture whose
    `restricted_patterns` names `README.md`), and
    `a_restricted_path_is_never_an_included_item` — each asserts a
    `RestrictedPath` record and no included item;
  - `every_collection_source_refuses_a_restricted_path` — walks every
    `CollectionSource` variant, no `_ =>` arm, and fails to compile when a
    variant is added without a case;
  - `keyword_hits_are_ordered_before_semantic_hits` and
    `a_keyword_hit_and_a_semantic_hit_for_one_path_become_one_item`;
  - `a_carried_plan_becomes_a_plan_state_item` (a resumed turn's plan renders
    its steps, and `PlanState` shares the item);
  - `the_symbols_item_keeps_the_lowest_provenance_it_contains`.
- [x] **Step 2: Run them and confirm they fail.**
- [x] **Step 3: Implement.** `ContextRequest`, `CollectionSource`, the
      collection-time `is_restricted` check per source, keyword/semantic kept
      apart, `PlanState` rendering from `SessionStore::read_task_plan`
      (`session.rs:1554`) read once in `chat.rs` before assembly. The turn's own
      mutable copy is still read inside `run_agentic_turn`
      (`chat.rs:2727-2738`) and is what the plan tools write through; the two
      cannot disagree because nothing writes a plan between them. Remove
      `build_context_with_hooks` and update its callers.
      `ContextManager::new` takes the `PathPolicy`.
- [x] **Step 4: Falsify.** Remove the check from one source → its test fails;
      merge the two search lists → the ordering test fails; drop a
      `CollectionSource` arm → the exhaustive test fails to compile.
- [x] **Step 5: Update the proposal** for C8, C10, C12, C13 (confirm §5.3, §5.6,
      §5.11 agree).
- [x] **Step 6: Scoped checks.** `cargo nextest run -p workspace-engine --test
      context_assembly --test hooks --test foundation --test symbol_index
      --test repository_map`, scoped clippy for `workspace-engine`, fast checks.

## Task 3: Range deduplication and ranged reads

**Requirements:** 2; proposal §5.3 (FileRanges order), §5.4.
**Corrections:** C2, C5.
**Files:** `context_manager.rs`, `file_access.rs`, `tests/context_assembly.rs`,
`tests/agent_tools.rs`.

- [x] **Step 1: Write the failing tests.**
  - `two_search_hits_in_one_function_send_one_merged_range` — one item, one
    range covering both, one read;
  - `ranges_two_lines_apart_merge_and_ranges_eight_apart_do_not` (the gutter is
    2; §7 records the reasoning);
  - `a_whole_file_item_absorbs_every_range_for_its_path`, and the absorbed ranges
    appear as `DeduplicatedIntoRange`;
  - `merging_keeps_the_language_server_provenance` (a `LanguageServer` range and
    a `Heuristic` one merge to `LanguageServer`);
  - `a_merged_range_longer_than_the_tool_read_cap_is_read_in_full` (a 600-line
    fixture range, against `max_read_lines` 400);
  - `a_context_range_read_still_refuses_a_restricted_path` and
    `a_context_range_read_still_redacts` (through `read_file`, not around it);
  - `every_absorbed_or_merged_range_is_recorded`.
- [x] **Step 2: Run them and confirm they fail.**
- [x] **Step 3: Implement.** Group by path, sort and merge with the gutter,
      whole-file absorption, provenance = the best of the parts, one
      `ReadWindow::ContextRange` read per merged range. Add the variant to
      `file_access.rs` and fix the exhaustive matches (`chat.rs:5333`,
      `damaian-cli/src/main.rs:169`, `tests/agent_tools.rs`).
- [x] **Step 4: Falsify.** Gutter 0 → the merge test fails; gutter ∞ → the
      eight-lines-apart test fails; drop whole-file absorption → that test fails;
      use `ReadWindow::Range` → the 600-line test fails and truncates at 400.
- [x] **Step 5: Update the proposal** for C2 and C5 (confirm §5.4's gutter and
      read paragraph).
- [x] **Step 6: Scoped checks.** `cargo nextest run -p workspace-engine --test
      context_assembly --test agent_tools`, scoped clippy for `workspace-engine`
      and `damaian-cli`, fast checks.

## Task 4: Per-category budgets and the effective limit

**Requirements:** 1, 3, 7; proposal §5.2, §5.3, §5.7.
**Corrections:** C6, C7, C8 (the reserved categories), C14.
**Files:** `config.rs`, `profile.rs`, `effective_policy.rs`, `context_manager.rs`,
`workspace_engine.rs`, `chat.rs`, `edit.rs`, `tests/context_assembly.rs`,
`tests/permission_profiles.rs`.

- [x] **Step 1: Write the failing tests.**
  - `each_category_stops_at_its_configured_share` — with a small limit, no
    category's `used` exceeds `allocated + what it borrowed`;
  - `an_unused_share_moves_to_later_categories` — with no plan and no findings,
    the file ranges get more than their nominal share and the manifest says so;
  - `a_category_over_its_share_drops_its_lowest_priority_members` — the
    `FileRanges` case: explicit, then mentioned, then implied; the dropped ones
    carry `CategoryBudgetExhausted`;
  - `semantic_hits_are_dropped_before_exact_matches`;
  - `the_most_specific_agents_md_survives_truncation` while the root one is
    dropped with `CategoryBudgetExhausted`, and the prompt still renders
    root-before-nested for whatever survived;
  - `a_block_item_too_large_for_its_share_is_rerendered_smaller` (a map that fits
    at 800 but not in its 5% share at a small limit appears, shortened, with a
    `Truncated` record);
  - `a_required_item_that_does_not_fit_is_an_error` — the assembly returns an
    error naming the limit, and the turn fails visibly rather than proceeding;
  - `shares_are_stored_in_basis_points_and_an_oversubscribed_set_is_a_config_error`;
  - `with_no_observed_profile_the_limit_is_the_config_budget_labelled_declared`
    — `tokens == context_token_budget()`, `source == Declared`,
    `safety_margin_bps == 0`;
  - `a_repository_may_lower_a_share_and_raise_the_margin` and
    `a_repository_cannot_raise_a_share_or_lower_the_margin` (the latter as a
    `WeakeningCase` in `tests/permission_profiles.rs`; a profile may only take
    the restrictive direction too).
- [x] **Step 2: Run them and confirm they fail.**
- [x] **Step 3: Implement.** `ContextBudget` from `Config`; parse
      `context_budget.<category>` and `context_budget.safety_margin` as decimals
      into basis points; classify both `Capability`; add the restrict-only merge
      beside the other numeric caps (`config.rs:1600-1650`); carry them through
      `profile.rs`, both `to_policy_text`s and `effective_policy.rs`;
      allocate, redistribute, truncate, and build `EffectiveLimit`.
      `build_context` now returns `Result<ContextPlan>`: a `Required` item that
      does not fit is `ClientError::InvalidInput` naming the limit, and both
      callers propagate it so the turn fails visibly. Replace Task 1's interim
      exemption that lets `add_text` keep a `Required` item regardless of budget.
- [x] **Step 4: Falsify.** Strict caps without redistribution → the
      redistribution test fails; over-share stealing from the next category →
      the truncation test fails; accept a repository raise → the weakening case
      fails; a 10% margin on the declared path → the declared test fails.
- [x] **Step 5: Update the proposal** for C6, C7, C8, C14 (confirm §5.2, §5.7,
      §5.11 and the restated acceptance criterion).
- [x] **Step 6: Scoped checks.** `cargo nextest run -p workspace-engine --test
      context_assembly --test permission_profiles --test repository_config_trust
      --test foundation`, `cargo nextest run -p workspace-engine --lib -E
      'test(config)'`, scoped clippy for `workspace-engine` and `desktop-shell`,
      fast checks, and `npm run specs:check` (no spec status moved yet).

## Task 5: The manifest, its audit record, and the callers

**Requirements:** 6; proposal §5.5.
**Corrections:** C9, C11.
**Files:** `context_manager.rs`, `chat.rs`, `edit.rs`, `tests/context_assembly.rs`,
`tests/foundation.rs`.

- [x] **Step 1: Write the failing tests.**
  - `the_manifest_lists_every_included_item_by_path_range_and_root`;
  - `the_manifest_names_the_reason_for_every_exclusion` — one fixture per reason
    that can be produced today (`RestrictedPath`, `CategoryBudgetExhausted`,
    `Truncated`, `DeduplicatedIntoRange`);
  - `no_manifest_field_contains_item_content` — seed a distinctive secret and a
    content marker into a fixture file, serialise the whole manifest to JSON, and
    assert neither appears; also assert the seeded secret is absent from the
    audit file;
  - `the_manifest_is_recorded_once_per_turn_as_context_assembled` — one event,
    `repositoryId`, `taskId`, `tokenEstimate`, the manifest JSON, and no content;
  - `the_edit_flow_records_its_manifest_too`;
  - `context_files_are_the_unique_included_paths_in_inclusion_order`;
  - `a_dot_prefixed_restricted_path_is_recorded` and
    `an_absolute_restricted_path_inside_the_repository_is_recorded` — each
    reaches collection in a non-normalised spelling, is refused, and produces
    one `RestrictedPath` record and no included item (the Task 2 review finding
    handled by Step 3a).
- [x] **Step 2: Run them and confirm they fail.**
- [x] **Step 3: Implement.** Fill `ContextManifest` while assembling; put it in
      `ContextPlan`; record it from both callers next to `model_request_prepared`;
      keep `files` as the unique included paths.
- [x] **Step 3a: Normalise before the collection check (found in Task 2's
      review).** `collection_exclusion` compares the raw candidate string, while
      `read_file` normalises `./`, `..` and an absolute in-repo path before
      `assert_not_restricted`. A non-normalised restricted path therefore passes
      collection, is refused at read, and `add_file` returns silently on that
      error, so no `RestrictedPath` record is written — the manifest cannot say
      why the file is absent. Normalise the candidate with the same normaliser
      the read path uses (`PathPolicy::resolve_existing`, keeping its relative
      path) before `collection_exclusion`, and when the read still refuses for a
      restriction, push a `RestrictedPath` record rather than returning without
      one. The same raw-vs-normalised comparison decides whole-file absorption
      and `files` membership in `merge_file_ranges`/`add_file`, so normalise
      once, before all three. Test with `./secrets/x.rs` and an absolute path
      inside the repository that names a restricted file.
- [x] **Step 4: Falsify.** Put the item content in `IncludedRef` → the content
      test fails; stop recording → the audit test fails; drop exclusion records →
      the reason test fails.
- [x] **Step 5: Update the proposal** for C9 and C11 (confirm §5.5).
- [x] **Step 6: Scoped checks.** `cargo nextest run -p workspace-engine --test
      context_assembly --test foundation --test hooks`, scoped clippy for
      `workspace-engine`, fast checks, and `cargo run -p eval-harness -- run
      --tier deterministic` (the manifest and `files` are what the harness reads).

## Task 6: A content-aware estimate

**Requirements:** 8; proposal §5.8.
**Corrections:** none.
**Files:** `context_manager.rs`, `tests/context_assembly.rs`,
`tests/token_accounting.rs` (unchanged assertions, rerun only).

- [x] **Step 1: Write the failing tests.**
  - `prose_code_and_json_get_different_estimates_for_the_same_length` — same
    character count, three content types, three different estimates;
  - `an_identifier_heavy_snippet_is_estimated_higher_than_prose_of_the_same_length`;
  - `the_estimate_is_monotone_in_length` and `never_zero_for_non_empty_content`;
  - `each_category_reports_whether_its_tokens_are_measured_or_estimated`
    (assembly is always `estimated`, per §5.8);
  - `#[ignore] calibration_compares_estimates_with_recorded_usage` with a doc
    comment giving the command (`DAMAIAN_CALIBRATION_DATA_DIR=… cargo nextest run
    -p workspace-engine --test context_assembly --run-ignored ignored-only -E
    'test(calibration_compares)'`; named apart from `DAMAIAN_DATA_DIR` so
    reading someone's recorded usage cannot be confused with running against
    that directory):
    it reads `model_request_prepared.tokenEstimate` and `task_usage_recorded` for
    paired rounds in the data directory it is given and prints the ratio. It reads
    no key and writes nothing. Ignored because it inspects a real data directory.
  - `#[ignore] calibration_measures_bytes_per_token` — **the live half.** This
    task only *writes* it; Task 7 runs it. It sends three fixed documents —
    a prose paragraph, a Rust function, a JSON object, each ~1–2 kB so the reply
    is one short line — one per `ModelAdapter::stream_response` call to the
    configured provider, reads `ModelRun.usage.input_tokens` (which must be
    `Some(measured)`), and prints `chars / tokens` per document plus the ratio
    `len / 4` would have used. It asserts nothing about the values except that
    each reported usage is measured and each ratio is within a sane band, because
    an assertion on a measured ratio would fail on a different provider.
    - **Doc comment, per `AGENTS.md` "Tests with real side effects":** it calls a
      real model provider over the network, it spends the user's money, it is
      ignored by default, and it is run manually. Give the exact command, with a
      **separate** `DAMAIAN_DATA_DIR` so the run cannot touch the user's real
      state:
      `DAMAIAN_DATA_DIR=/tmp/damaian-calibration cargo nextest run -p
      workspace-engine --test context_assembly --run-ignored ignored-only -E
      'test(calibration_measures_bytes_per_token)'`.
    - **Key handling.** It resolves the key the way a normal non-desktop turn
      does — `env::var(&config.model_api_key_env)` from the config it loaded,
      exactly as `damaian-cli`'s `live_model_api_key` does
      (`damaian-cli/src/main.rs:694-703`) — and passes it to
      `CurlModelTransport`. It never prints, logs, stores or asserts on the key,
      adds no new variable, and does not read the Keychain (that module belongs to
      `desktop-shell`). The doc comment says so: a `keychain:` reference cannot be
      resolved from this crate at all, so that setup runs the test against a
      `DAMAIAN_DATA_DIR` whose user config names an environment variable, or
      measures the same three documents through the app instead.
    - **The three documents are fixtures in the test**, not files on disk: no
      repository, no index, and no write to the data directory beyond what
      `Config`/`ProcessRegistry` already do.
- [x] **Step 2: Run them and confirm they fail.** (`cargo nextest run -p
      workspace-engine --test context_assembly` — the ignored calibration tests
      are reported as skipped, which is the confirmation they compile.)
- [x] **Step 3: Implement.** A `TokenEstimate` with a ratio per content type
      chosen from `kind` + file extension, replacing the `div_ceil(4)` inside
      `add_text` only. **Do not change `ModelAdapter::estimate_tokens`
      (`model.rs:354`)**: that one accounts for a provider that reports no usage
      (spec 19), it measures the whole request body, and changing it moves a
      different spec's numbers.
- [x] **Step 4: Falsify.** One ratio for every type → the three-type test fails;
      an estimate that ignores length → monotonicity fails; a zero estimate → the
      non-empty test fails. The two calibration tests are not falsified here —
      they are measurements, not guards, and Task 7 records what they reported.
- [x] **Step 5: Update the proposal** §5.8 with the provisional constants and say
      plainly that Task 7 replaces them with measured values.
- [x] **Step 6: Scoped checks.** `cargo nextest run -p workspace-engine --test
      context_assembly --test token_accounting --test foundation` (which shows the
      two ignored calibration tests as skipped), scoped clippy for
      `workspace-engine`, fast checks.

## Task 7: Calibration, docs and close-out

**Requirements:** 8's calibration, the docs half of acceptance, and
`AGENTS.md`'s "When a spec becomes Done".
**Proposal:** §5.8, §5.9, §7.
**Files:** `docs/USER_GUIDE.md`, `docs/TROUBLESHOOTING.md`, `proposal.md` §7,
`docs/specs/README.md`, `CHANGELOG.md`, this file, every spec whose `Depends on:`
names #26.

- [x] **Step 1: Offline calibration — not run (no data directory provided).**
      The owner did not point the agent at a `DAMAIAN_DATA_DIR`, so
      `calibration_compares_estimates_with_recorded_usage` was not run and no
      ratio is invented. It reads nothing by default and prints the command; the
      gap is recorded in proposal §7 under "Known open points". If a directory
      is named later, run it and record the distribution there.
- [x] **Step 2: Live calibration — run by the owner on 2026-10-25.** The
      human-run route: the agent ran neither ignored test and read no key.
      Provider `deepseek`, model `deepseek-v4-flash`; prose 4.50, Rust 3.61 and
      JSON 2.45 bytes/token against `len / 4`'s 283/333/260-token estimates
      (+13%, −10%, −39%). Ratios are now code 18/5 (3.6), structured 12/5 (2.4),
      prose 4/1 unchanged; the table, the ~15-token instruction overhead and the
      one-sample caveat are in proposal §7. The `max_tokens: 16` failure was
      fixed to 1024 before the run.
  - **Human-run:** ask the human to run
    `DAMAIAN_DATA_DIR=/tmp/damaian-calibration cargo nextest run -p workspace-engine --test context_assembly --run-ignored ignored-only -E 'test(calibration_measures_bytes_per_token)'`
    (Task 6's test, with its own doc comment).
  - **Agent-run on explicit approval:** ask in chat first and run it only after a
    clear yes for *this* live run. The key is resolved by the engine from
    `config.model_api_key_env` exactly as for a turn — the agent never reads,
    echoes, logs or stores it, and never passes it on a command line or in an
    environment assignment it types. If the approval is not given, take the
    human-run route or record the half as outstanding.
  - Either way the test sends three fixed fixture documents — a prose paragraph,
    a Rust function, a JSON blob — one per call, reads the provider's reported
    input tokens, and prints `chars / tokens` for each;
  - record the three ratios, the provider and model, and how far `len / 4` was in
    §7; replace Task 6's provisional constants with them;
  - if the human declines both routes, record in §7 that the per-content-type half
    is outstanding, keep the provisional constants with their derivation, and say
    so in the Progress row. Do not claim the calibration acceptance in full.
- [x] **Step 3: Docs** (§5.9). `USER_GUIDE.md`'s **Output and Context Limits**:
      the pipeline order, what each category is, the `context_budget.*` keys with
      their defaults and "lower only from repository config", and what the
      conversation/request order now is. `TROUBLESHOOTING.md`: a new section
      "Reading a turn's context manifest" — where the `context_assembled` event
      is, what each `ExclusionReason` means, observed vs declared limits, and what
      to do when content is `CategoryBudgetExhausted` (raise the share, name the
      file in the prompt, or lower a competing category). Add a row to
      TROUBLESHOOTING's symptom index.
- [x] **Step 4: Acceptance run.** `cargo run -p eval-harness -- run --tier
      deterministic`: all scenarios pass, and every metric is compared by hand
      against `evals/baseline.json` (spec 18 requirement 7). A metric that moved
      is explained in §7 or the run is a regression; a moved completion rate is
      the acceptance criterion failing, not a note. Record the result and the
      date in §7.
- [x] **Step 5: Close the spec** per `AGENTS.md`, in one change:
      proposal `Status:` → Done with the date and the acceptance evidence;
      README's implementation-order row and **Current state** bullets; every
      `Depends on:` line naming #26 (search
      `grep -rn "26_context_assembly" docs/specs/*.md docs/specs/*/*.md`) → built;
      "What to build next" re-derived, which promotes #27, #33, #51 and #55 and
      leaves #54 and #49's reuse slice out (their Depends lines still name Phase 1
      WP3 as unspecified); remove the `Unreleased` item ("Allocate context by
      category with a record of what was excluded and why") from `CHANGELOG.md`;
      this file's Progress rows and header.
- [x] **Step 6: Checks.** `scripts/quality-gate.sh fmt node lint typos deny`,
      `npm run specs:check -- --strict`, scoped clippy and the named test files.
      All passed locally, and the full seven-check gate passed in CI on
      2026-10-25 (Quality run 40204226481, on `main` at `8758f62`).

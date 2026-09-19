# Feature Specifications

Status: Done
Source: gap analysis against `ai_coding_assistant_specification.md` and `ai_coding_assistant_must_have.md`, and a review of the current implementation (2026-07-17).

These specs describe features that close gaps between the product specification and the current state of the codebase. Each spec is self-contained: motivation, current state (with file references), requirements, non-goals, design, and acceptance criteria.

They were meant to be implemented one at a time, in the order listed. That still holds by default, but **[What to build next](#what-to-build-next-and-what-may-run-in-parallel) is the section to read before picking one** — it records which specs are actually unblocked, which two keystones the rest of the directory waits on, and the one file-level constraint that decides whether two specs can be built at the same time.

Specs 1–6 came from the original gap analysis. Later entries are added as
design changes come up; #7 came from a reported bug rather than the analysis,
#8 from a usability gap reported in use, #9 from a release-engineering defect
found in CI, #10 from approval-prompt fatigue reported in use, #11 from making
repository agent instructions first-class, #12 from a web-app troubleshooting
session where browser runtime evidence was not first-class, and #13 from a real
session where Docker was needed but the assistant asked the user to run the
command manually. #34 is the other exception to the roadmap rule below: it was
written and implemented out of order, ahead of #14, because it is a security
defect rather than a graduation. #41 through #44 are a third exception: they
come from usability feedback on the desktop shell rather than a work package, and
#41 introduces [`../UI_STYLE_GUIDE.md`](../UI_STYLE_GUIDE.md), the standing
visual reference that later UI specs cite instead of restating. #47 is a fourth
exception, and the only one that came from measuring Damaian against this
directory rather than from using the product: a session asked what capabilities
Damaian needs to implement the remaining specs, and ran out of tool rounds
before it could answer.

#48 through #54 are roadmap graduations, not exceptions, but they share an
origin worth recording: a capability survey comparing Damaian against the
published feature surface of mainstream terminal coding agents, run on
2026-09-14. The survey produced seven capabilities with no home anywhere in the
roadmap — provider rate limiting, prompt-cache accounting, a way for the model
to ask a question, reading a page from the internet, serving Damaian's own reads
over MCP, finding anything in a past session, and image input — plus a set of
deliberate non-goals now recorded in the roadmap's deferred-work section so they
stop resurfacing. Each became a work package first and a spec second, per the
governance rule below.

#54 came from a second pass on 2026-09-15 asking what the first pass had missed.
It is the sharpest of the seven, because the capability was already half paid
for: #12 captures screenshots and nothing can look at them.

#55 is an ordinary roadmap graduation, written the same day. It is worth noting
here only because writing it corrected the work package it came from *and* the
justification this session had given for promoting that package — both recorded
in its §1. Specs in this directory are expected to contradict the plan where the
code says otherwise; that is what writing them is for.

#56 is a fifth exception: it was split out of #48 during that spec's
implementation, because the fallback *offer* turned out to be a feature in its
own right rather than a fragment of the backpressure work it came from — see
its §1.

From #14 onward, specs graduate from the delivery plan in `docs/PLAN/`, one spec
per work package, per that directory's governance section. Each carries a
`Plan:` line naming its phase and work package, and the plan's dashboard records
the spec number under `Spec target`. `docs/PLAN/` is local-only and not
committed, so those references are names rather than links, and each spec is
written to stand on its own.

**Three documents, three jobs** — separated on 2026-09-15, when the planning
directory was renamed from `ROADMAP` to `PLAN` because it was never a roadmap:

| Document | Committed | Holds |
|---|---|---|
| [`../../CHANGELOG.md`](../../CHANGELOG.md) | yes | Every release, newest first, plus an `Unreleased` section naming what is specified but not built |
| `docs/PLAN/` | no | The delivery plan: phases, work packages, dashboard, and `OBSERVATIONS.md`, the inbox for things noticed but not yet decided |
| This directory | yes | What was decided and built |

## What to build next, and what may run in parallel

Derived on 2026-09-19 from the `Depends on:` line of every unstarted spec (see
[Every spec names its prerequisites](#every-spec-names-its-prerequisites)),
which is the source of truth — this section is a reading of those lines, not a
second opinion. Re-derive it rather than trusting it if the table below disagrees with
a spec's own header.

**Only four specs have every dependency built:** [#20](20_working_modes.md),
[#49](49_prompt_cache_accounting_and_reuse/proposal.md) (in progress),
[#56](56_provider_fallback_consent.md), and [#38](38_subagent_model.md) (which
depends on nothing here, but is explicitly speculative — its own §1 allows
abandoning it). Everything else is transitively blocked, and the graph is
unusually linear:

```
#20 working modes ──> #22 findings ──> #23 verification, #32 hooks, #35 commit prep
                 │                └──> #24 repo map ──> #25 symbols
                 │                                 └──> #26 context assembly
                 ├──> #31 profiles ──> #52 MCP server mode
                 ├──> #33 MCP mgmt  ──┘
                 └──> #50 clarification, #51 fetch

#26 context assembly ──> #27 inspector ──> #28 ──> #29 ──> #30  (memory, serial)
                    ├──> #54 image input, #55 compaction, #51 fetch
                    └──> #49's reuse slice
```

**#20 and #26 are the keystones.** Thirteen specs sit downstream of #20 and it
is ready now; the whole tail waits on #26. Preferring a spec that unblocks
nothing over one of these costs more than it looks like it does.

| | Build | Why here |
|---|---|---|
| 1 | #49's accounting slice | Planned, in flight, and it corrects a figure that is wrong today |
| 2 | **#20 working modes** | The keystone: unblocks 13 specs, ready now |
| 3 | **#22 findings** | Second keystone; also closes #21's deferred `Evidence::Findings` |
| 4 | #24 → #26 | #26 is the real unlock; #25 follows #24 |
| 5 | #55 compaction | Also unblocks #49's reuse slice, which is why that slice waits |
| 6 | #27, #54, #51, #23 | The fan-out once #26 has landed |
| 7 | #28 → #29 → #30 | Memory, strictly serial |
| 8 | #35 → #36/#37; #31/#33 → #52; #32 | Delivery and extensibility clusters |
| 9 | #38/#39/#40 | Last, and #40 may legitimately conclude "do not" |

[#56](56_provider_fallback_consent.md) is the opportunistic one: small, ready,
and it closes the half of #48 that shipped as only a negative guarantee.

### Parallel work

Two specs may be built at once, which is a **deliberate exception to the
one-at-a-time rule above**, under one constraint that comes from this
repository's own history: [#47](47_agent_working_capability/context.md) was
deferred behind #17 because both edit `chat.rs`, and its line numbers drifted
twice in a single session while #17 was in flight. The binding constraint is not
the dependency graph, it is that file.

- **Safe together:** a spec that touches `chat.rs` and one that does not. #49's
  accounting slice states outright that it changes no request and touches
  `model.rs`, `config.rs`, `session.rs`, `app.js` and the harness — so it pairs
  with #20, which lives in `chat.rs` and the UI.
- **Not together:** #20 and #56 — both touch approval and resume in `chat.rs`.
- **Never together:** #26, #55 and #49's reuse slice. All three rewrite
  `build_model_prompt` and the assembly around it; that shared function is why
  #49's second slice is blocked in the first place. #54 and #55 also both
  restructure the message array.
- **Genuinely independent once #26 lands:** #27, #51 and #23.

Running two tracks: give each worktree its own `CARGO_TARGET_DIR` (a shared one
thrashes), agree up front which track owns `chat.rs` so the other rebases, and
expect the quality gate rather than the work to be the bottleneck — clippy is
about 18 minutes cold and the suite about 5, so two gates running at once on one
machine contend for the same cores.

**Keeping this honest:** when a spec becomes Done, move it out of the ready set
and promote whatever its `Depends on:` line was blocking. If that upkeep lapses,
re-derive from the `Depends on:` lines — the cost is one pass over the unstarted
specs, which is how this section was produced.

## Implementation order

Per-spec rationale, in spec-number order. For *what to build next*, read the
section above — this table records why each spec sits where it does, not which
is ready.

| # | Spec | Why this order |
|---|------|-----------------|
| 1 | [01_response_formatting.md](01_response_formatting.md) | **Done.** Touches every assistant response; highest visible impact for lowest risk. No architectural changes required. |
| 2 | [02_semantic_search.md](02_semantic_search.md) | **Done.** Spec-flagged open gap (`ai_coding_assistant_specification.md` §7.2, §19) — current "semantic search" is keyword overlap, not embeddings. Independent of #1. |
| 3 | [03_structured_tool_calling.md](03_structured_tool_calling.md) | **Done.** Replaces fragile text-envelope parsing (`DAMAIAN_EDIT_V1`, `DAMAIAN_COMMAND_V1`) with native tool schemas. Best done before #4, since hunk-level apply will want a clean tool-call surface for patch actions. |
| 4 | [04_hunk_level_patch_apply.md](04_hunk_level_patch_apply.md) | **Done.** Correction: hunk-level apply already existed end-to-end in the desktop app. This spec was narrowly scoped to CLI parity + an audit gap. |
| 5 | [05_clickable_file_references.md](05_clickable_file_references.md) | **Done.** Smallest, most independent change; benefits from #1's markdown renderer being in place first. |
| 6 | [06_mcp_support.md](06_mcp_support.md) | **Done.** New capability (not a gap-closer): lets users add local/remote MCP servers whose tools plug into the native tool-call loop from #3. Depends on #3's structured tool-calling surface being in place. |
| 7 | [07_generated_secret_override.md](07_generated_secret_override.md) | **Done.** Bug-driven, not from the original gap analysis: the generated-secret block false-positived on setup documentation and had no user override, so a false positive was an unrecoverable dead end. Closes the §7.10 "Override" requirement. |
| 8 | [08_stop_and_progress.md](08_stop_and_progress.md) | **Done.** Usability-driven: a chat turn can run ~90 minutes unstoppably and reports only a static `Thinking` badge while it does. Closes §7.5's cancellation requirements (`cancel(runId)` is present in the trait but dead code) and §7.1's "distinct UI states" requirement. Independent of #1–#7. |
| 9 | [09_release_quality_gate.md](09_release_quality_gate.md) | **Done.** Release-engineering, not a product gap: tagged builds publish even when Quality is red, because the release workflow has no dependency on it and Quality never runs on tags. Makes Quality a reusable workflow the release pipeline must pass. Independent of #1–#8. |
| 10 | [10_persistent_command_approval.md](10_persistent_command_approval.md) | **Done.** Usability-driven, with a safety edge: command approval was one-shot only, so the same command prompted on every run and users learned to click through without reading. Adds "allow always", writing the existing `command_allowlist` — to repository config as first built, and to user config keyed by repository since #34 moved it there. Independent of #1–#9. |
| 11 | [11_agents_md_support.md](11_agents_md_support.md) | **Done.** Turns the previous root-only generic project-rule handling for `AGENTS.md` into scoped repository instructions, including nested files and prompt precedence. Independent of #1–#10. |
| 12 | [12_web_app_troubleshooting.md](12_web_app_troubleshooting.md) | **In progress.** Adds first-class browser diagnostics for local web-app debugging: page errors, console/network evidence, interaction scenarios, screenshot artifacts, session-scoped diagnostic approval, and safer tool-round handling. Builds on #6, #8, and #10. |
| 13 | [13_docker_command_support.md](13_docker_command_support.md) | **Done.** Makes Docker a first-class approval-gated command family with Docker-specific risk messaging and diagnostics, while keeping automatic execution limited to sandbox-safe read-only commands. Builds on #3 and #10. |
| 14 | [14_developer_id_signing_and_notarization.md](14_developer_id_signing_and_notarization.md) | **Done.** Shipped in v0.31.0. Roadmap Phase 0 WP1. Replaces ad-hoc signing with Developer ID signing, notarization, stapling, and a fail-closed verification gate, plus stable and preview release channels. Nothing later in the roadmap can be tested with users until this ships. Builds on #9. |
| 15 | [15_install_and_update_verification.md](15_install_and_update_verification.md) | **Partially done, remainder skipped.** Roadmap Phase 0 WP2. The data-directory schema version and its refusal path shipped, wired into the shell, the CLI, and the app. The updater signature fixtures, the update rehearsal, the Keychain measurement, the documentation rewrite, and the second-person install were skipped as verified in practice — see its §7 for what that leaves open. Depends on #14. |
| 16 | [16_session_checkpoints_and_rewind.md](16_session_checkpoints_and_rewind.md) | **Done.** Roadmap Phase 1 WP1. Extends patch rollback into a session checkpoint covering files *and* conversation position, via a shadow Git object store: a working-tree census for approved commands, restore with conflict handling, conversation rewind by append, retention, a per-turn `Rewind` control, and a Settings › Checkpoints list. Settles the secret-redaction-versus-faithful-restore conflict the old rollback snapshots had; shares the session-log `seq` migration with #17. |
| 17 | [17_durable_task_state_and_crash_recovery/](17_durable_task_state_and_crash_recovery/proposal.md) | **Done.** Roadmap Phase 1 WP2. **Rescoped and split before implementation** — this spec is the engine core only: thirteen task states, parse-first session-log reads, before-and-after action markers, the recovery classifier, migration, pending-approval reattach, and the three recovery operations. Its central guarantee is that no action whose outcome is unknown is ever automatically repeated, enforced in `recovery::resume` so a UI cannot widen it. The kill matrix covers **all thirteen states by three crash shapes, 39 automated cells**, with one `#[ignore]`d real-`SIGKILL` test proving the on-disk signature those cells assume; it found a requirement-5 hole rather than merely confirming the code. Parse-first reads fixed a reader that *fabricated a message from a torn line*, and the sequence cache took appending 2000 events from 17.5s to 129ms by making per-append cost flat instead of doubling. The recovery prompt moved to #45 and the process registry to #46, **both now unblocked**. Note #16 already shipped the `seq` field this spec's migration section describes. Unblocked #18's thirteenth scenario, which now runs and makes `recovery_success` a measured value. |
| 18 | [18_local_evaluation_harness/](18_local_evaluation_harness/proposal.md) | **Done.** Roadmap Phase 1 WP4. The measuring instrument every later phase's improvement claim rests on. New `crates/eval-harness` with a `damaian-eval` binary: sixteen scenarios run and pass against fixture repositories (spec 47 added `navigated_edit` and `batched_reads`; `batched_reads` also added `tool_calls_at_least` and `tool_rounds_at_most`). The thirteenth was committed as blocked on #17, reporting `notApplicable` in every run rather than being quietly absent, and #17 unblocked it: it now injects a crash and measures that recovery classifies it and refuses to repeat it, making `recovery_success` a measured value. Full coverage of the roadmap metric set, each row carrying a value, a human-sourced marker or an explicit not-applicable naming the phase that will supply it. The deterministic tier takes 2.8s and runs inside `cargo test --workspace --locked`, so it adds no quality-gate command; the live tier is credential-gated and never run by CI. It first ran against a real provider on 2026-09-10 and found seven defects **in the harness itself** — including two metrics that had been reporting a plausible constant rather than a measurement, which is the failure an eval harness exists to prevent and is least able to detect in itself. All fixed; see its §7. Found and fixed a security defect while being built: a top-level `secrets/` or `credentials/` directory was unrestricted, because `**/` requires a leading path segment. Its baseline review gate then caught the harness leaking its own seeded credential into `evals/baseline.json` — see its §7. Consumes #19's token fields, which report not-applicable until that lands. |
| 19 | [19_token_and_cost_accounting/](19_token_and_cost_accounting/proposal.md) | **Done, with one measurement outstanding.** Roadmap Phase 1 WP6. Per-run token usage and per-task aggregation, distinguishing provider-measured from estimated and never presenting one as the other; retries, stopped turns and calls lost to a crash all count, because under-reporting is what makes Damaian look cheaper than it is. Measures only; the enforced ceiling is Phase 2. Turns spec 18's token row from `notApplicable` into a real 52,271-token figure, read through the same `read_task_usage` a session uses so the two cannot drift. **Validated against DeepSeek on 2026-09-10**: it reports usage on every call, and the `len / 4` estimate measured 2.5–6.3% high — always in the safe direction for a figure a reader acts on. OpenAI remains unmeasured, and is the one measurement the status line still calls outstanding. Planning against the code found five places where the design assumed behaviour the code does not have: a provider 4xx arrives as a *successful* read because `curl -sS` exits zero, so the probe branches on the parsed body; no test double could return a different response on a second call; a mid-stream stop discarded the partial run entirely; recovery cannot estimate a lost call from a request that no longer exists, so the estimate is written onto the action marker *before* the call; and §5.6's completion report does not exist yet, being spec 23's, so requirement 2 lands as the task-state surface. Implementation found two more: rendering a cost with `toFixed(4)` printed anything under $0.0001 as **`$0.0000`**, which reads as free, and spec 18's token metric was pinned by no test at all. |
| 20 | [20_working_modes.md](20_working_modes.md) | **Not started.** Roadmap Phase 2 WP1. Four session modes (Ask, Plan, Code, Review) as a capability boundary rather than a prompt instruction, enforced in three layers so the text-envelope fallback from #3 cannot be used as an escape. Its permission matrix is the artifact Phase 4 WP3 extends. |
| 21 | [21_task_plan_progress_and_budget/](21_task_plan_progress_and_budget/proposal.md) | **Done.** Roadmap Phase 2 WP2. Ordered steps with observable evidence, so a step is never complete because the model said so, plus an enforced per-turn token ceiling using #19's accounting. Persists through #17's event log. Two model-facing tools split the authority the spec left open: `propose_plan` supplies titles only, and `complete_step` takes **no arguments by design** — the model asks to move on, the engine decides from what it observed. Planning against the code found eight statements the design assumed and the code contradicts; three changed the design: a "task" is one turn, so "per task" means per turn and a resumed turn is a *new* task whose plan is carried across explicitly; the round-budget pattern §5.4 says to copy does not stop the loop but makes one more model call, which on a grown context is the turn's most expensive — so the ceiling check moved *before* the call; and the session log had no failure outcome to read evidence from, because every tool arm recorded `"ok"` whatever the tool reported, which is the same defect that made #18's error rate 0.000 by construction. Fixing it moved that metric to 0.333 and `check_pass_rate` off a by-construction zero. Implementation found three more: the review gate cannot reuse the crash-recovery `sideEffecting` flag to decide what "mutating" means (it is conservative on purpose and would gate a sandbox-safe `ls`), so it consults the command policy instead; `renderMessages` marked *every* assistant message of a turn, so a tool-budget stop had long been rendering a duplicate row per message on reload; and `Evidence::FileRead` existed with nothing constructing one, so every reading step reported "completed unverified" while the path and hash sat in hand. `Evidence::Findings` stays deferred because #22 does not exist to produce the ids, and the acceptance criterion citing #23's fixture is restated against #18's harness as the `planned_task` scenario, which runs in CI. |
| 22 | [22_findings_model_and_panel.md](22_findings_model_and_panel.md) | **Not started.** Roadmap Phase 2 WP6. One `Finding` type shared by compiler, test, lint, browser, and review sources, with parsers that degrade to a usable generic finding rather than losing a failure. Numbered ahead of #23 because the type must exist before the loop that produces them. |
| 23 | [23_verification_loop.md](23_verification_loop.md) | **Not started.** Roadmap Phase 2 WP3. Sequences apply → discover checks → run → find → repair → rerun → report, driven by the orchestrator rather than the model, so a completion report distinguishes verified from assumed. Consumes #21 and #22. |
| 24 | [24_repository_map_and_monorepo_boundaries.md](24_repository_map_and_monorepo_boundaries.md) | **Not started.** Roadmap Phase 3 WP2. A deterministic, token-bounded map of project roots, plus per-root working directories so a command discovered in `packages/api` runs there. Reuses `detect_project_commands`' manifest list per root rather than inventing a second one. |
| 25 | [25_symbol_and_relationship_index.md](25_symbol_and_relationship_index.md) | **Not started.** Roadmap Phase 3 WP3. A rescope, not greenfield: `FileRecord.symbols`/`imports` already exist as prefix heuristics. Adds locations, kinds, relationships, and a `Provenance` label separating language-server fact from heuristic guess. Satisfies Phase 2's deferred LSP dependency. |
| 26 | [26_context_assembly.md](26_context_assembly.md) | **Not started.** Roadmap Phase 3 WP4. Replaces the single first-come-first-served budget with per-category allocation, range deduplication, and a manifest recording what was excluded and why. Adds line ranges to `ContextItem`, without which two of its requirements are unexpressible. |
| 27 | [27_context_inspector.md](27_context_inspector.md) | **Not started.** Roadmap Phase 3 WP5. Renders #26's manifest pre- and post-send, and adds pins and path restrictions. Makes "the user can see what context the agent uses" true; Phase 3b's memory arrives as one more category in this view. |
| 28 | [28_memory_model_and_storage.md](28_memory_model_and_storage.md) | **Not started.** Roadmap Phase 3b WP1. The memory record and store, with a `project_key` resolved from the repository's root commit — because `repository_id` is a path hash and cannot give the sharing property the roadmap asks for. Memory refuses secrets rather than redacting them. |
| 29 | [29_memory_creation_and_consent.md](29_memory_creation_and_consent.md) | **Not started.** Roadmap Phase 3b WP2. The consent gate: `MemoryProposal` and `MemoryEntry` are separate types with one conversion requiring a user confirmation, so unconfirmed persistence is unrepresentable. Instruction-shaped candidates from untrusted origins get no proposal at all. |
| 30 | [30_memory_retrieval_and_lifecycle.md](30_memory_retrieval_and_lifecycle.md) | **Not started.** Roadmap Phase 3b WP4. Memory reaches the model only as a `ContextItem` in the lowest-priority category, is visible and removable in #27, and is marked stale by hash comparison against its evidence. Proven by prompt-injection evals, not design argument. |
| 31 | [31_permission_profiles.md](31_permission_profiles.md) | **Not started.** Roadmap Phase 4 WP3. Its security subset shipped as #34, which closed the repository-config override and moved `command_allowlist` to user scope; what remains here is the profile machinery — the capability/preference partition as a general mechanism, effective-policy source attribution, and profile export/import. Specified before #32 because both #32 and #33 reference the profile they cannot widen. |
| 32 | [32_hooks.md](32_hooks.md) | **Not started.** Roadmap Phase 4 WP2. Nine lifecycle events, hooks as external programs returning a verdict — `deny`/`request_approval`/`warn`/`allow`, with no `approve`, so widening is unrepresentable. Mandatory is the default classification so a broken hook fails closed. |
| 33 | [33_mcp_management_and_deferred_discovery.md](33_mcp_management_and_deferred_discovery.md) | **Not started.** Roadmap Phase 4 WP4. Every enabled server's full schema currently reaches every request, outside #26's accounting. Adds a search tool with on-demand schemas, per-tool enable, lazy startup, and treats a remote read-only claim as an assertion that cannot lower an approval requirement. |
| 34 | [34_repository_config_trust_boundary.md](34_repository_config_trust_boundary.md) | **Done — implemented ahead of #14 onward.** Bug-driven, not a roadmap graduation. `apply_overlay` is scope-blind, so repository config overrides user config: a repo-set `shell` runs its own script for every approved command (verified), a repo-set `model_base_url` exfiltrates the API key and code with no approval, and a repo `command_allowlist` executes commands unprompted. Adds a scope-aware overlay, a forbidden-key list, restrict-only merges, and moves `Allow Always` to user scope keyed by repository. Security subset of #31, extracted so it need not wait for Phase 4. |
| 35 | [35_commit_preparation.md](35_commit_preparation.md) | **Not started.** Roadmap Phase 5 WP1. Damaian's first write to a user's Git repository. Builds the commit through a private index so the user's staging is never touched, runs hooks rather than bypassing them, and closes a trap: `GitService::diff` redacts, so a user could approve a diff showing `[REDACTED]` and commit the real secret. |
| 36 | [36_branch_and_worktree_delivery.md](36_branch_and_worktree_delivery.md) | **Not started.** Roadmap Phase 5 WP2. Branch suggestion, creation, ahead/behind, merge-base, and conflict prediction via `merge-tree --write-tree` so preparation provably cannot execute. Written to work with no worktree support, since its stated dependency (Phase 2 WP4) is Should-tier and unspecified. |
| 37 | [37_pull_request_creation.md](37_pull_request_creation.md) | **Not started.** Roadmap Phase 5 WP4. Push and publication as two typed approvals that cannot substitute for each other, base branch asked for rather than inferred, force push refused rather than gated, and interrupted PR creation resolved by a duplicate search before any retry. Uses MCP rather than a bespoke client. |
| 38 | [38_subagent_model.md](38_subagent_model.md) | **Not started.** Roadmap Phase 6 WP1, and gated on that phase's eight readiness gates being measured first. Subagents as in-process tasks whose capability can only be derived by narrowing a parent's, staged read-only-first. Corrects the roadmap's process-based framing and the flat `CancelToken`. |
| 39 | [39_coordination_and_conflict_handling.md](39_coordination_and_conflict_handling.md) | **Not started.** Roadmap Phase 6 WP2. Exclusive file-ownership claims checked at spawn *and* before every write, serialized patch integration with `base_hash` revalidation between applies, and a combined-check rerun that catches two individually-passing patches failing together. |
| 40 | [40_autonomy_evaluations.md](40_autonomy_evaluations.md) | **Not started.** Roadmap Phase 6 WP7. The decision instrument: same scenarios run in both execution modes, cost amplification as a ratio, and an experimental label *derived* from the recorded comparison rather than chosen. Makes abandoning Phase 6 a first-class recorded outcome. |
| 41 | [41_ui_density_and_action_hierarchy/](41_ui_density_and_action_hierarchy/proposal.md) | **Done.** Collapsed command approval 161px → 104px, patch actions from half the conversation column to their natural width. Usability-driven, not a roadmap graduation: the shell has exactly one button style, so a one-shot `Approve Run` and a persistent `Allow Always` are visually identical, and a stray `min-height: 180px` on base `pre` makes every command approval reserve 180px of empty rationale pane. Introduces the button scale in [`../UI_STYLE_GUIDE.md`](../UI_STYLE_GUIDE.md) and applies it to the approval and patch cards. Presentation only — the approval policy from #10, #12 and #34 is untouched. Also builds `docs/ui-style-guide.html`, a specimen page that loads the shipping stylesheet so the guide has a rendered counterpart that cannot drift. First of three UI specs; the conversation column and the remaining chrome follow. |
| 42 | [42_conversation_column_density/](42_conversation_column_density/proposal.md) | **Done.** Chat log 459 → 606px at 1280×800; chrome down from 43% to 24% of the column. Second of the three UI specs from the same usability review as #41. 43% of the conversation column measured as chrome at 1280×800: the docked context strip folds into the turn that read the files, the thread header trades a hardcoded caption for the active folder and session, role labels move to screen-reader-only, and the composer grows from two rows instead of reserving four. Depends on #41's button scale and disclosure pattern, which it promotes to a shared class. |
| 43 | [43_chrome_density_and_hierarchy/](43_chrome_density_and_hierarchy/proposal.md) | **Done.** Third of the UI specs. Applies the scale to the surfaces #41 and #42 did not reach: settings action buttons 456 → 129px with one primary per group and `.btn-danger` on the destructive ones, every font-size brought onto a type scale that now documents its heading steps, and the terminal tab bar slimmed with the working directory folded into it (body 142 → 183px). Supersedes #41 §3.1 on `.inline-actions`. |
| 44 | [44_composer_control_density/](44_composer_control_density/proposal.md) | **Done.** Composer 149 → 129px, and the 58px right gutter every line paid is gone. Fourth UI spec from the same usability review, covering the one surface #41–#43 did not reach: the attach, model and send controls were 38px and positioned *over* the textarea, which reserved 70px of height and 58px of every line as padding to clear them, and the 220px model pill truncated its own effort label away because model and effort shared one string. Controls moved to a content-sized row below the box, model and effort split into separate `.btn-trigger` entry points onto the one existing popover, and pinned chips moved inside the frame. Adds `.btn-trigger`, the footer-row pattern and two anti-patterns to [`../UI_STYLE_GUIDE.md`](../UI_STYLE_GUIDE.md). Presentation only — model persistence, send/stop behaviour and the menu's contents are untouched. Leaves room in the row for #31's working-mode control without building it. |
| 45 | [45_crash_recovery_prompt.md](45_crash_recovery_prompt.md) | **Done.** Split out of #17: the user-facing half. A card at the top of the affected session names the specific in-flight action ("A patch application was in progress and its outcome is unknown"), offers Resume, Inspect, Mark failed and Abandon, and links `Inspect` to #16's checkpoint. Renders a decision #17 already constrained — the sentence and the reason a resume is absent are built in `recovery.rs`, and every decision is re-classified server-side before it reaches the engine, so a webview cannot claim a classification it was not given. Also the first caller of #17's launch sweep, which authorizes safe tasks without running them: a `waiting_for_model` task must not fire a billed model call because the app was opened. |
| 46 | [46_process_registry_and_orphan_sweep/proposal.md](46_process_registry_and_orphan_sweep/proposal.md) | **Done.** Split out of #17. An **owner-scoped** registry — one JSON file per live child under `<data_dir>/processes/`, written at spawn, unlinked on clean exit — covering **four** spawn sources, not three: MCP stdio servers, `curl` model calls, PTY sessions and shell commands. The fourth was added because #17's analysis that `Command::output()` cannot orphan a child was **read from the code and wrong**; measuring it showed a `SIGKILL`ed parent leaves the child reparented to `launchd`, so `output()` became `spawn()` + `wait_with_output()`. Turns on one question: a PID is reused, so a recorded PID is killed only when it is still alive *and* its start time matches, read via `proc_pidinfo` — every state that is not provably a live process of ours collapses to "do not kill", so the fail-closed direction falls out of the API instead of being imposed on it. Scoped on the **owner** rather than the session because two instances can run at once and a session-keyed sweep would kill the live instance's servers. Runs at launch in `run_server_with_ready` and at CLI startup — deliberately *not* inside #45's memoized `sweep_once`, which fires on the first HTTP request, so an orphan would outlive the crash until somebody opened the UI. A self-pipe `SIGINT`/`SIGTERM`/`SIGHUP` handler gives the same identity-gated kill a second caller, so Ctrl-C cleans up too; the handler does one `write` and the byte *is* the signal number, so the watchdog re-raises what arrived rather than a hardcoded `SIGINT`. Killing a stranger's process is a worse bug than the leak, so every kill goes through a `ProcessIdentity` comparison and `kill(-pgid)` is gated on the recorded leader still matching. |
| 47 | [47_agent_working_capability/](47_agent_working_capability/proposal.md) | **Done.** Split into a folder and designed on 2026-09-17. All eight requirements are built, tested and closed out on 2026-09-18: 1–4 (ranged reads, `list_directory`, `search_content`, anchor-based region edits) and 5, 6 and 8 (stoppable/timed/streaming commands, a bounded recorded continuation, and within-round concurrent dispatch of read-only calls). Requirement 6 is justified by the round-cap cliff and the unbounded in-turn array, **not** by the spec's opening premise, which §7.2's A/B did not support. Covered in CI by #18's `navigated_edit` scenario, which reaches `one_file_patch`'s outcome through the new tool surface. **The live-tier A/B ran on 2026-09-18 and did not support the spec's premise** (§7.2): against the slice's parent commit, three runs each, the slice costs **+85% model calls and +106% tokens** and buys **approval stops falling 17 → 4** as the model stops shelling out to navigate — a trade of tokens for human interruptions, not the round reduction the spec opens with. All 14 scenarios are short, so the long-task premise is untested rather than disproved, and requirement 6 should be re-argued from a long-task measurement when one exists. The A/B also found that `toolRounds` had been counted from the scenario script, which made it a constant in the tier that ignores scripts; it now reads the session log. Evidence-driven, not a roadmap graduation: a session asked what capabilities Damaian needs to implement the remaining specs and ran out of tool rounds before it could answer. Asks whether Damaian can do a spec's worth of work in one sitting — today it cannot, and the reason is the tool surface rather than the model. Ranged reads, listing and content search as first-class tools, region-bounded edits so an edit payload scales with the change rather than the file, cancellable commands with a timeout, and a bounded continuation past the eight-round limit. Adds no autonomy: every capability is read-only or produces the same reviewable patch. The cheap deterministic floor beneath #24, #25 and #26, specified first because those three cannot be built by an agent that lacks it. Defers its budget to #19 and #21 rather than inventing a second one. Extended by the capability survey with requirement 8, within-round concurrent dispatch of read-only tool calls — the cheapest latency win on the tool surface, and explicitly not subagent parallelism, which stays #38 and #39. |
| 48 | [48_provider_limits_and_backpressure/proposal.md](48_provider_limits_and_backpressure/proposal.md) | **Done.** Roadmap Phase 1 WP7. A provider rate limit ends the turn, and the code written to prevent that cannot see it: a 429 arrives as a parsed error body and returns immediately with no retry, while `is_retryable_message`'s `"rate limit"` and `"429"` arms are reachable only from the transport-failure path — where they never fire, because `curl -sS` exits zero on a 4xx. Captures status and headers via `dump-header` rather than a stdout sentinel a model could emit, classifies refusals from status first and never from prose, honours `Retry-After` under both an attempt count and a wall-clock ceiling, and makes the outcome a `failureKind` on the existing failed state rather than a fourteenth state — three fewer kill-matrix cells for the same information. A refused call records measured zero, so it cannot inflate #19's totals through the estimate written before the call. |
| 49 | [49_prompt_cache_accounting_and_reuse/](49_prompt_cache_accounting_and_reuse/proposal.md) | **In progress.** Split into a folder and planned on 2026-09-18; the **last unstarted Phase 1 work package**. Planning against the code split it in two. The **accounting slice** (requirements 1, 2, 3, 6, 8) is planned in `tasks.md` and changes no request at all. The **reuse slice** (requirement 4) is blocked, and not on what the flat spec assumed: reordering `build_model_prompt`'s sections is one edit, but the conversation section is a **sliding window of the last eight messages**, so past the eighth message every turn drops the oldest and the prefix changes at its first byte regardless of ordering. #55 recorded the ordering problem first and assigned it here; nobody had written down that the window is what makes reuse impossible. Requirement 5 turned out to be **already satisfied** — `system_prompt()` is a static string with no clock, run id or round counter — so its test is a regression guard rather than a fix, and it ships in the accounting slice. Roadmap Phase 1 WP8. Not a future problem: `estimated_cost` applies one input rate to every input token, so for any provider that caches automatically and reports the split, the cost figures #19 made honest are already overstated — and the overstatement grows with the conversation. Adds `cached_input_tokens: Option<u64>` as a subset of the input count, distinguishing "the provider did not say" from "none were cached", and a cached rate; where cached tokens are reported and no cached rate is set, cost is computed at the full rate and labelled an upper bound, which is a deliberate softening of #19's both-rates-or-nothing rule justified by the direction of the error. The reuse half is prefix ordering, with a test that fails the day a clock value enters the system prompt. |
| 50 | [50_model_initiated_clarification.md](50_model_initiated_clarification.md) | **Not started.** Roadmap Phase 2 WP8. The engine can ask for permission and cannot ask a question, so an ambiguous task is guessed at — and #21's enforced token ceiling made that sharper, since rounds spent on a misread instruction are rounds the correct work no longer has. An `ask_user` tool whose load-bearing rule is that an answer can never satisfy an approval, enforced by types rather than discipline: `PendingApprovalRef.kind` is a `String` today, so the cheap implementation is one string away from treating "yes, the CLI" as command approval. Reuses the existing waiting state and reattach machinery rather than adding a state, is bounded to two questions per turn, and is absent from the tool list entirely where no user can answer. |
| 51 | [51_external_reference_retrieval.md](51_external_reference_retrieval.md) | **Not started.** Roadmap Phase 3 WP8. Damaian cannot read a page, so every task against a third-party API runs on what the model remembers about an unnamed version of it. `fetch_url` and an optional `search_web`, with fetched content entering as a lowest-priority provenance-labelled `ContextItem` proven non-authoritative by injection evals rather than by argument — #30's treatment of memory, reused. Its §5.3 is the reason it is Should-tier: a URL is the first place the model chooses what leaves the machine, so host approval lives in user scope on the forbidden-for-repository list, a URL carrying repository content or a secret is refused *before* the approval dialog rather than by it, and cross-host redirects are not followed. |
| 52 | [52_mcp_server_mode.md](52_mcp_server_mode.md) | **Not started.** Roadmap Phase 4 WP6. #06 and #33 are client-side only, so Damaian's indexed search, redaction and path policy are reachable only from its own window — the alternative being to give another tool raw filesystem access. Exposes a read-only subset over stdio, with the set computed as the intersection of read-only and profile-permitted rather than listed, so a tool added later is excluded until classified. The repository is fixed at launch and unreachable from the wire; a server has no user, therefore no approval, therefore nothing that writes. |
| 53 | [53_session_search_and_export/proposal.md](53_session_search_and_export/proposal.md) | **Done.** Roadmap Phase 1 WP9. A rescope, not greenfield: listing, renaming and deleting already work end to end, and what is missing is finding anything and getting anything out. A bounded scan through #17's parse-first reader rather than a second index, anchored on `seq` because it survives rewinds. Its sharp edge is inherited from #16: `append_message` stores content verbatim and deliberately does not redact, so that a rewind restores what was actually said — which makes export a display path that writes a file, and redaction on the way out mandatory, with the redaction count stated rather than silently applied. |
| 54 | [54_image_input.md](54_image_input.md) | **Not started.** Roadmap Phase 3 WP9. Damaian takes screenshots it cannot look at: #12's diagnostics capture them as artifacts carrying a path and dimensions, and the model that asked receives the report text and a file reference. From the other side `ModelMessage.content` is a `String` and the composer offers "Add file" and a disabled "Add folder", so a user cannot paste a picture of the bug they are reporting. Its hard part is that **an image defeats `SecretScanner`** — a screenshot of a terminal can carry a key and no rule can match a pixel. OCR is rejected on three independent grounds, one being that a guarantee holding most of the time is worse than a stated limitation, because the user stops checking. The guarantee becomes informed consent plus one asymmetry: a user-attached image was chosen by the person accountable for it, an agent-captured screenshot was framed by Damaian, so no unseen image is ever sent and no setting can make screenshot inclusion automatic. |
| 55 | [55_conversation_compaction.md](55_conversation_compaction.md) | **Not started.** Roadmap Phase 3 WP6 (Must, and in that phase's minimum slice). Corrects the work package's premise: conversations do not grow until a provider refuses, they are **clipped to the last eight messages, each cut to 2,000 characters**, by two literals in `build_model_prompt` with no configuration behind them. So the defect is silent unmarked loss, not growth — a constraint stated in the first message is gone from the model's view at the ninth, with nothing on screen saying so, which is a worse failure than the loud one the plan describes. Its load-bearing rule is that **a model never writes a field the engine can compute**: files changed, plan steps, approvals and validation status are read from the session log, and only objective, constraints, decisions, failed approaches and uncertainty come from a model — because a summary is re-sent every turn, so an invented fact becomes a false belief the agent holds for the rest of the session. Compaction appends a `conversation_compacted` event rather than rewriting anything, which makes #16's rewind work with no special case and keeps #17's append-only rule intact.
| 56 | [56_provider_fallback_consent.md](56_provider_fallback_consent.md) | **Not started.** Split out of #48 during its implementation: the fallback *offer* — a one-shot, refused-by-default approval that re-issues a refused turn through a configured second provider — was recognisably its own feature (engine pause + shell Keychain + frontend approval + resume), the size of #10 or patch approval. #48 shipped only requirement 7's negative half (no switch without consent, fail closed); this spec is the positive half. Depends on #48's classification and `failureKind`, and reuses the pending-approval and resume machinery rather than building a second one.

**Ordering exception.** Numbers are assigned in creation order, so #34 is last in
the table but is the next thing to implement. It is a bug-driven spec covering a
security weakness found while writing #31, and it does not depend on any of
#14–#33. Where #34 and #31 overlap, #34 is authoritative for the repository-config
trust boundary and #31 for the profile machinery built on it.

Each spec's status is tracked at the top of its file: `Not started`, `In progress`, or `Done`.

## Every spec names its prerequisites

Each spec carries a `Depends on:` line under its header, added on 2026-09-15.
It names the specs that must exist before this one can be built, marks each as
built or **not built**, and states that everything else the spec references is a
cross-reference rather than a prerequisite.

It exists because that distinction was previously unrecoverable from a spec. A
spec links to half a dozen others — some it builds on, some it deliberately is
*not*, some it merely cites — and an agent picking up the work had to go to the
delivery plan's dashboard to learn which was which. The goal is that a spec is
enough on its own to start from.

Two rules follow, and the second is the one worth enforcing:

- Dependencies are taken from the plan's dashboard where the spec graduated from
  a work package, so the two cannot disagree. Where a spec is not a graduation
  (#1–#13, #34, #41–#47), they come from its own stated ordering.
- **A dependency marked *not built* must be answered in the design.** Either the
  spec states what to do until it lands — as [#55](55_conversation_compaction.md)
  §5.1 does for the context budget, [#49](49_prompt_cache_accounting_and_reuse/proposal.md)
  §5.3 for its prefix test, and [#31](31_permission_profiles.md) §7 for #34 — or
  it says plainly that the work is blocked. The pattern
  [#18](18_local_evaluation_harness/proposal.md) established is the model:
  ship the part you can, report the rest as `notApplicable` naming what will
  supply it, and never be quietly absent.

Writing the line is also the point at which "what if this isn't built yet?"
becomes unavoidable. That question is cheap to answer while specifying and
expensive to answer halfway through an implementation.

## Spec layout

Specs 1–40 are single files. **#41 is a trial of a folder layout** — if it earns
its keep, later specs follow it and the existing files migrate; if it does not,
it reverts to a single file and nothing else is affected.

A spec folder holds up to three documents, and creates only the ones it needs:

| File | Holds | Required |
|---|---|---|
| `proposal.md` | Requirements, non-goals, design, acceptance criteria — the decision, and the same header block single-file specs carry | Always |
| `context.md` | Motivation, current state, and corrections found during implementation — why the work exists and what the code looks like today | When there is more than a paragraph of it |
| `tasks.md` | Execution order, progress table, verification steps | When the work needs a breakdown |

The split exists so the decision stays readable as the background and the task
list grow. `proposal.md` is the entry point and the file the table above links
to; read it first.

**Do not cite commit SHAs in a spec.** They go stale the first time history is
rewritten, and then the document quietly lies. Record what changed and what was
measured; `git log` and `git blame` are how you find the commit.

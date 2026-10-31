# Feature Specifications

Status: Done
Source: gap analysis against `ai_coding_assistant_specification.md` and `ai_coding_assistant_must_have.md`, and a review of the current implementation (2026-07-17).

These specs describe features that close gaps between the product specification and the current state of the codebase. Each spec is self-contained: motivation, current state (with file references), requirements, non-goals, design, and acceptance criteria.

They were meant to be implemented one at a time, in the order listed. That still holds by default, but **[What to build next](#what-to-build-next-and-what-may-run-in-parallel) is the section to read before picking one** — it records which specs are actually unblocked and the one file-level constraint that decides whether two specs can be built at the same time.

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

#57 is a sixth exception: it came from describing how this repository is
actually built — a planner session that owns specs and verification, worker
sessions that implement one task each, in parallel worktrees — and asking what
Damaian needs to host that workflow itself. It is user-driven sessions, not
#38's agent delegation, so it is not subject to #38's readiness gates; its
`context.md` §2 records why.

#58 is a seventh exception, of the same kind as #41–#44: usability feedback on
the desktop shell, with a clickable mockup agreed before the spec was written.

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

### Current state (2026-10-31)

A summary of each spec's own `Status:`, `tasks.md` and `Depends on:` lines on
this date. Where it disagrees with them, they win; `npm run specs:check`
reports the disagreement.

#23, #25, #26, #27, #28, #29, #32, #35, #36, #50, #51, #55, #59, #62, #65 and
#66 are all Done and merged into `main`. #23 and #26 both finished on
2026-10-25, so the two keystones are no longer in flight; #35 and #55 finished
on 2026-10-26, and the full seven-check gate passed in CI on 2026-10-26
(Quality run 40323902715). #27 finished on 2026-10-26, and the full seven-check
gate passed in CI on 2026-10-26 (Quality run 40340511717); #28 and #29 finished
on 2026-10-26, and the full seven-check gate passed in CI on 2026-10-26 for the
`main` push that contains both. #51 external reference retrieval and #36 branch
and worktree delivery finished on 2026-10-28 and were merged into `main` that
day; the full seven-check gate passed in CI on 2026-10-29 for the `main` push
that contains both (Quality run 40902516310).

#60–#85, #87–#97, and #99 are also Done (see their rows below). Several were evidence-driven or
usability specs added outside the roadmap order; being finished, none of them
changes what may be built next.

- **Done:** #30 memory retrieval and lifecycle finished on
  2026-10-30, built on branch `spec-30-memory-recall` (Tasks 1–6 on
  2026-10-28, documentation and close-out on 2026-10-30; see its
  [`tasks.md`](30_memory_retrieval_and_lifecycle/tasks.md)). Its scoped checks
  passed locally, and the full seven-check gate passed in CI on 2026-10-30 for the `main` push that contains it (Quality run 41063831118). Its §7.3 records the one open item that is not a criterion:
  the phase's two-week real-session observation has not elapsed.
- **In progress:** none. #49 prompt cache's accounting slice is done and the
  reuse slice (requirement 4) is still unbuilt; #26's ordering exists, and #55
  replaced the eight-message sliding window on 2026-10-26, so the reuse slice is
  no longer blocked by either. #49's own `Depends on:` line still names Phase 1
  WP3 (provider capability validation) as unspecified, so it is not in the
  ready set.
- **Ready:** #33 MCP management. It was serialised behind #30 on `chat.rs`;
  with #30 Done it is free to start, from a `main` that contains #30's turn
  path so it does not rebase over it. [#98](98_task_scoped_protected_patch_paths.md)
  is also ready: it makes direct user no-touch paths an engine-enforced,
  task-scoped patch fence across worker and repair continuations.
- **Blocked:** #37 on #33; #52 on #33; #39 on #38; #40 on #38 and #39; #54 on
  Phase 1 WP3 (provider capability validation), which is unspecified.
- **Deferred:** #38 (stage 1 is due for re-evaluation now that #57 is done).
- **Pairing:** two specs may run together only if at most one of them edits
  `chat.rs`, and #49's reuse slice is a solo `chat.rs` track while it rewrites
  the request around `build_model_prompt`. #33 edits `chat.rs` through the MCP
  tool list and per-tool dispatch, and #98 edits it through patch dispatch and
  continuation state; either owns that file while it is in flight.
  Every Done spec no longer constrains anything. See
  [Parallel work](#parallel-work).
- **Open decision:** the priority table below predates #59–#80 and does not
  rank them; the text after it now calls #23 the priority with #65 done.

### History

Derived on 2026-09-19 from the `Depends on:` line of every unstarted spec (see
[Every spec names its prerequisites](#every-spec-names-its-prerequisites)),
which is the source of truth — this section is a reading of those lines, not a
second opinion. Re-derive it rather than trusting it if the table below disagrees with
a spec's own header.

Re-derived on 2026-09-25, when [#20](20_working_modes/proposal.md) was
finished. **Five specs now have every dependency built:**
[#22](22_findings_model_and_panel/proposal.md), [#31](31_permission_profiles/proposal.md),
[#50](50_model_initiated_clarification/proposal.md) — all three unblocked by #20 —
plus [#56](56_provider_fallback_consent/proposal.md) and [#38](38_subagent_model.md)
(which depends on nothing here, but is explicitly speculative — its own §1
allows abandoning it). #20 also cleared one of the dependencies of
[#33](33_mcp_management_and_deferred_discovery.md) and
[#51](51_external_reference_retrieval/proposal.md), but both still wait on #26, and #33
waits on #31 as well.
[#49](49_prompt_cache_accounting_and_reuse/proposal.md)'s accounting slice
shipped on 2026-09-23; its one remaining requirement (the reuse slice) is
blocked on #55 or #26, not ready, so the spec as a whole does not belong in
this set. Everything else is transitively blocked, and the graph is unusually
linear:

```
#20 working modes (done) ──> #22 findings (done) ──> #23 verification, #32 hooks, #35 commit prep
                        │                       └──> #24 repo map (done) ──> #25 symbols (done)
                        │                                               └──> #26 context assembly (ready)
                        ├──> #31 profiles (done) ──> #52 MCP server mode
                        ├──> #33 MCP mgmt  ──┘   (#33 also needs #26 and #31)
                        └──> #50 clarification, #51 fetch (#51 also needs #26)

#26 context assembly ──> #27 inspector ──> #28 ──> #29 ──> #30  (memory, serial)
                    ├──> #54 image input, #55 compaction, #51 fetch
                    └──> #49's reuse slice
```

Re-checked on 2026-09-29, when [#12](12_web_app_troubleshooting/proposal.md)
was finished. No `Depends on:` line names #12, so nothing is promoted and the
ready set above is unchanged. What changes is scope, not order. #22's Task 6
now converts #12's `WebDiagnosticDetails` into findings instead of adding its
own browser structure, and #54 builds on #12's shipped screenshot artifact
record. #22 is no longer waiting on anything from #12.

Re-checked on 2026-09-30, when [#31](31_permission_profiles/proposal.md) was
planned to run alongside #22. The ready set is unchanged. One correction to the
pairing: #31's file references never name `chat.rs`, but enforcing
`profile ∩ mode` does, because spec 20's permission check is called from it.
So #31 and #22 are safe together **except #31's Task 5 against #22's Task 8**,
the one task on each side that edits `chat.rs`. Each spec's `tasks.md` says so
in its Global Constraints.

Re-derived on 2026-10-04, when [#22](22_findings_model_and_panel/proposal.md)
was finished. Five `Depends on:` lines named #22 not built: #23, #24, #26, #32
and #35. **Two are promoted:** [#23](23_verification_loop/proposal.md), whose other
dependencies (#18, #21) were already built, and
[#24](24_repository_map_and_monorepo_boundaries/proposal.md), whose other one (#21)
was. The other three still wait. #26 waits on #24 and #25, #32 on #31, and #35
on #23 and #31. #31 is in progress, not ready. #22 also closed #21's deferred
`Evidence::Findings`. The ready set is now #23, #24, #50, #56, #57 and the
speculative #38.

Re-derived on 2026-10-06, when [#31](31_permission_profiles/proposal.md) was
finished. Four `Depends on:` lines named #31 not built: #32, #33, #35 and #52.
**One is promoted:** [#32](32_hooks/proposal.md), whose other dependencies (#17, #22)
were already built. The other three still wait. #33 waits on #26, #35 on #23,
and #52 on #33. The ready set is now #23, #24, #32, #50, #56, #57 and the
speculative #38.

Re-checked on 2026-10-06, when [#24](24_repository_map_and_monorepo_boundaries/proposal.md)
was planned. The ready set and the order are unchanged. #24 is in progress, so
it leaves the ready set. Three facts about pairing it, from its
[`context.md`](24_repository_map_and_monorepo_boundaries/context.md) §5 and §9:

- **It never edits `build_model_prompt`.** The map reaches the model as a
  `ContextItem` built in `context_manager.rs`. So #24 does not collide with
  #55 or #49's reuse slice. #26 rewrites `build_context`, but #26 waits on
  #24 and absorbs that item as its planned repository-map category.
- **Only its Task 7 edits `chat.rs`.** So #24 may run beside a `chat.rs` spec
  (#23, #32, #50, #56 or #57), except Task 7 against that spec's `chat.rs`
  task. Its `tasks.md` says so in its Global Constraints.
- **Its Task 5 changes `propose_detected_validations` to take the map** and
  propose per root. #23 calls that function as its only discovery mechanism.
  If #23 is built alongside, agree which side adapts.

Re-checked on 2026-10-07, when [#50](50_model_initiated_clarification/proposal.md)
was planned. The ready set and the order are unchanged. #50 stays ready rather
than in progress until its Task 1 starts. Two facts about pairing it, from its
[`context.md`](50_model_initiated_clarification/context.md) §11:

- **Its Tasks 3 and 4 edit `chat.rs`**, and Task 5 does only if recovery needs
  a paused-turn accessor. Of the `chat.rs` specs, it may pair with #25 only:
  not with #23, #32, #56 or #57, and not with #24's Task 7.
- **Its Task 2 adds a config key**, so it edits `config.rs`, `profile.rs`,
  `effective_policy.rs` and `tests/permission_profiles.rs`, the same four
  files as #24's Task 6. That is a rebase, not a blocker.

Re-derived on 2026-10-09, when [#24](24_repository_map_and_monorepo_boundaries/proposal.md)
was finished. Two `Depends on:` lines named #24 not built: #25 and #26.
**One is promoted:** [#25](25_symbol_and_relationship_index/proposal.md), whose only
dependency was #24. #26 still waits on #25. The ready set is now #23, #25,
#32, #50, #56, #57 and the speculative #38. Two facts from #24 that the next
specs inherit:

- **`propose_detected_validations` now takes the map and proposes per root**
  (`validation.rs:273`). #23 calls it as its only discovery mechanism. Its
  references were updated to the new signature, and roots overlap: a Cargo
  workspace root and each member all propose `cargo test`.
- **The map reaches the model as one `repository_map` context item** built in
  `context_manager.rs`. #26 absorbs it as its planned repository-map category,
  and `RepositoryMap::root_for_path` is the function #26's planned
  `ContextItem.root_path` should call.

Re-checked on 2026-10-09, when [#38](38_subagent_model.md) was deferred
rather than planned. It leaves the ready set, which is now #23, #25, #32, #50,
#56 and #57. Nothing is promoted or demoted, because #38's only dependents,
#39 and #40, were already blocked on it. Its stage 2 is superseded by #57's
worktree workers, and stage 1 is re-evaluated once #57 has shipped. #38's
Deferral section records the reasons.

Re-checked on 2026-10-09, when [#50](50_model_initiated_clarification/proposal.md)'s
Task 1 landed. #50 is in progress, so it leaves the ready set, which is now
#23, #25, #32, #56 and #57. Its pairing rule is unchanged: its `chat.rs` tasks
(3 and 4) may run beside #25 only. #24 is done, so its Task 7 no longer
constrains them.

Re-checked on 2026-10-09, when [#23](23_verification_loop/proposal.md) was
planned. The ready set and the order are unchanged. #23 stays ready rather
than in progress until its Task 1 starts. Two facts about pairing it, from its
[`context.md`](23_verification_loop/context.md) §10:

- **Its Tasks 4, 5 and 6 edit `chat.rs`**, and Task 7 does only if recovery
  needs a paused-turn accessor. While any of them is in progress, #23 may
  pair with #25 only, not with #32, #50, #56 or #57.
- **It adds a paused-turn kind**, a check batch, and must make every resume
  entry point refuse another kind's pause. #50 adds a question kind and the
  same refusal. Whichever lands second adds its kind to the other's match.
  Its Tasks 3 and 7 also touch `session.rs` and `recovery.rs`, as #50's
  Tasks 1 and 5 do. That is a rebase, not a blocker.

Re-checked on 2026-10-12, when [#32](32_hooks/proposal.md)'s Tasks 1 and 2
were merged. #32 was planned on 2026-10-07 and its Task 1 started the same
day, so it is in progress and leaves the ready set, which is now #23, #25, #56
and #57. Three facts about pairing it, from its
[`context.md`](32_hooks/context.md) §3 and §14:

- **Its Tasks 7 and 8 edit `chat.rs`**, and no other task does. Of the
  `chat.rs` specs, it may pair with #25 only: not with #23, #50, #56 or #57.
- **Its Task 1 added hook config keys** to `config.rs`, `profile.rs`,
  `effective_policy.rs` and `tests/permission_profiles.rs`, the same four
  files as #50's Task 2. That is a rebase, not a blocker.
- **Its Task 6 edits `edit.rs`'s `apply_stored_patch` and
  `context_manager.rs`**, which #26 rewrites. Same rule.

Re-derived on 2026-10-12, when [#56](56_provider_fallback_consent/proposal.md)
was finished. No `Depends on:` line names #56, so nothing is promoted. Spec
48's cross-reference now points at the shipped behaviour. #56 leaves the ready
set, which is now #23, #25 and #57. Its `chat.rs` work is finished, so the
"not with #56" clauses above no longer constrain anything.

Re-checked on 2026-10-12, when [#25](25_symbol_and_relationship_index/proposal.md)
was planned (split into a folder on 2026-10-09, re-checked against `main` on
2026-10-12). The ready set and the order are unchanged. #25 stays ready rather
than in progress until its Task 1 starts. Three facts about pairing it, from
its [`context.md`](25_symbol_and_relationship_index/context.md) §9 and §12:

- **No task edits `chat.rs` or `build_model_prompt`.** The model sees symbols
  as a `symbols` context item built in `context_manager.rs`, as #24's map
  does. So #25 may pair with any `chat.rs` spec (#23, #32, #50, #57). The
  three things that need `chat.rs` are deferred to a follow-up task and named
  in its `context.md` §12.
- **Its Task 8 adds the `symbols` item inside `build_context`**, which #32's
  Task 6 turns into a delegate of `build_context_with_hooks`. Its Tasks 5 and
  8 add config keys, so they edit `config.rs`, `profile.rs`,
  `effective_policy.rs` and `tests/permission_profiles.rs`, as #32 and #50
  do. Both are rebases, not blockers.
- **Its Tasks 5, 7 and 10 edit `desktop-shell/src/lib.rs`**: the exit path,
  `handle_findings`, and three new routes. #57 and #32's Task 6 also edit the
  shell. That is also a rebase. Its Task 5 adds `ProcessKind::LanguageServer`
  beside #32's `Hook` and #57's `Worker`.

Re-checked on 2026-10-13, when #25's Task 1 started. #25 is in progress, so it
leaves the ready set, which is now #23 and #57. The order is unchanged.

Re-checked on 2026-10-16, when [#57](57_worker_sessions/proposal.md)'s Task 11
finished. Its acceptance run with DeepSeek did not yield a *Start worker*
button, so #57 stayed in progress with a Task 12 (its §7 items 10 and 11).

Re-derived on 2026-10-16, when #57's Task 12 finished and acceptance passed on
its re-run. No `Depends on:` line names #57, so nothing is promoted, and #57
leaves the ready set, which is now #23 alone. #57's `chat.rs` work is finished,
so the "not with #57" clauses above no longer constrain anything. One
condition is now met rather than a dependency: #38's stage 1 was to be
re-evaluated once #57 shipped (#38's Deferral section). That re-evaluation is
a decision for the user, and #38 stays deferred until it is made.

Re-derived on 2026-10-23, when [#25](25_symbol_and_relationship_index/proposal.md)
was finished. One `Depends on:` line named #25 not built: #26. **One is
promoted:** [#26](26_context_assembly/proposal.md), whose other dependencies (#21, #22
and #24) were already built. #26 leaves the blocked set and joins the ready
set. #25 leaves the in-progress set. A Task 11 follow-up closed the one wiring
gap its close-out found: the first symbol search now starts the eager
definition pass, so language-server definitions and diagnostics are reachable in
the app.

Re-derived on 2026-10-24, when [#50](50_model_initiated_clarification/proposal.md)
and [#32](32_hooks/proposal.md) were finished and, with #25, #59 and #62, merged
into `main`. No `Depends on:` line names #50 (only
[#51](51_external_reference_retrieval/proposal.md) cites its §5.4 as a cross-reference)
or #32, so nothing further is promoted. The ready set is #23, #26 and #65;
#66 was built afterwards, on 2026-10-25.

Re-checked on 2026-10-24, when [#26](26_context_assembly/proposal.md) was
planned. It moved from the ready set into **In progress** without being built:
the flat spec became a folder, [`context.md`](26_context_assembly/context.md)
records the current pipeline and what other specs rely on in it, and
[`tasks.md`](26_context_assembly/tasks.md) holds seventeen corrections, four open
questions for the owner and seven tasks. No dependency line changes, because
nothing waits on #26 until it is Done. What the planning found that matters to
the order: #26 rewrites `build_model_prompt` and its section order, so it still
cannot run beside #55 or #49's reuse slice, and #65 and #66 must be added to
that list — all four edit the same function, and #65/#66 also edit the turn
loop.

Re-derived on 2026-10-25, when [#26](26_context_assembly/proposal.md) was
finished. Six `Depends on:` lines named #26 not built: #27, #28, #33, #51, #54
and #55. #26 leaves the in-progress set. **Four are promoted:** #27 (its only
dependency), #33 (#20, #31 and #26 all built), #51 (#20 and #26) and #55 (only
#26). **Two are not:** #28 still waits on #27, and #54 still waits on Phase 1
WP3 (provider capability validation), which is unspecified. #49's reuse slice
is no longer blocked on #26's ordering, but its own `Depends on:` line still
names Phase 1 WP3, and the eight-message window it is most sensitive to is
#55's to replace, so it is not promoted; #49 stays in progress with its
accounting slice done. #26's completion also retires the pairing rule that named
it: `build_model_prompt` is no longer work in progress, so the constraint now
binds #55, #49's reuse slice, #65 and #66. Two open points #26 recorded are not
blockers and are inherited by #27 and by whoever builds the first range
producer: no collection source yields a `LineRange` yet, so range deduplication
is implemented and unit-tested but inert in production, and no Observed
capability profile exists, so every limit is `Declared`. The ready set is #23,
#27, #33, #51, #55 and #66. The full seven-check gate passed in CI on
2026-10-25 (Quality run 40204226481).

Re-derived on 2026-10-25, when [#65](65_exploration_memory_within_a_turn/proposal.md)
finished. No `Depends on:` line names #65, so **nothing is promoted**; #65
leaves the ready/in-progress sets and its `chat.rs` pairing constraint ends.
[#66](66_plan_left_open_at_turn_end/proposal.md) finished the same day, so it
leaves the ready set too. The ready set is #23, #27, #33, #51 and #55; #23 is
the next priority and still the last thing in front of #35.

**#26 is Done (2026-10-25), so the tail it was blocking is open.** Nothing else
in front of it is unbuilt. The specs it unblocked are #27, #33, #51 and #55;
with #65 and #66 Done on 2026-10-25, #23 becomes the next priority and is still
the last thing in front of #35. No `Depends on:` line names #65 or #66, so
nothing is promoted by their completion.

Re-checked on 2026-10-28, when [#51](51_external_reference_retrieval/proposal.md)
was planned. It moved from the ready set into **In progress** without being
built: the flat spec became a folder,
[`context.md`](51_external_reference_retrieval/context.md) records the current
context, tool and config pipeline plus the corrections planning found, and
[`tasks.md`](51_external_reference_retrieval/tasks.md) holds ten tasks. No
dependency line changes, because nothing waits on #51 until it is Done. Two
facts matter to the order: **#51 edits `chat.rs`** (Tasks 7 and 8), so it is now
a `chat.rs` track under the pairing rule; and its content reaches the model
through a new eleventh context category, so it also edits
`context_manager.rs`, `config.rs` and spec 26's default share table. The ready
set was then #30, #33 and #36; #51's completion is recorded below.

| | Build | Why here |
|---|---|---|
| 1 | #23 verification | **Done 2026-10-25.** It was the last thing in front of #35, and its loop lives in `chat.rs` |
| 2 | #55 compaction | **Done 2026-10-26.** Replaced the eight-message window with a token budget plus an append-only summary, so #49's reuse slice now has a stable conversation block to measure |
| 3 | #28, #33, #51 | The fan-out #26 unblocked. **#27 is Done (2026-10-26)** and promoted #28; **#51 is Done (2026-10-28; CI green 2026-10-29)**. #28 and #29 are Done; #33 remains and is ready now that #30, which shared `chat.rs` with it, is Done |
| 4 | #29 → #30 | Memory, strictly serial behind #27. **#29 is Done (2026-10-26)** and promoted #30; **#30 is Done (2026-10-30)**, CI green (Quality run 41063831118) |
| 5 | #36 (**Done 2026-10-28**, CI green 2026-10-29)/#37; #33 → #52 | Delivery and extensibility clusters. #35 is Done (2026-10-26) and promoted #36, which finished on 2026-10-28. #37 is still blocked on #33, not on #36 |
| 6 | #38/#39/#40 | Last, and #40 may legitimately conclude "do not". #38 is deferred; #57 is done, so its stage 1 is due for re-evaluation |

#54 stays off this table until Phase 1 WP3's provider capability validation is
specified; #66 was built ahead of it and is Done 2026-10-25, with the full gate green in CI.

[#56](56_provider_fallback_consent/proposal.md), the opportunistic one, is done
(2026-10-12): it closed the half of #48 that had shipped as only a negative
guarantee.
[#50](50_model_initiated_clarification/proposal.md) is done (2026-10-24) and
unblocks nothing, which is why it sits off the table rather than above #23.
[#32](32_hooks/proposal.md), also done (2026-10-24), sits off the table for the
same reason: no `Depends on:` line names it.
[#57](57_worker_sessions/proposal.md), added on 2026-10-03, sits off the table
for the same reason: every dependency (#17, #20, #46) is built and nothing
depends on it. It is done (2026-10-16).
[#58](58_header_repo_bar_and_menus/proposal.md), added on 2026-10-13, is done
(2026-10-14). Re-derived then: no `Depends on:` line names #58, so nothing is
promoted. #35 and #37 name it only as a cross-reference: #35 (built 2026-10-26)
added *Commit…* to its contextual-action slot, and #37 will add *Create PR*
when it is built.
[#49](49_prompt_cache_accounting_and_reuse/proposal.md)'s accounting slice is
done, off this table for the same reason #56 is: nothing left in it is
unbuilt. Its reuse slice now waits only on the spec's Phase 1 WP3 line: #26's
ordering exists and #55 replaced the sliding window on 2026-10-26.
[#59](59_exploration_round_allowance/proposal.md), added on 2026-10-17, sits
off the table like #50 and #57: every dependency (#21, #34, #47) is built and
nothing depends on it, so it joined the ready set beside #23; it is done
(2026-10-23). It rewrote the round loop's counting in `chat.rs`, so it did not
pair with #23 or with #32's `chat.rs` tasks; it did pair with #25, which never
edited `chat.rs`.
[#60](60_worktree_aware_read_tools/proposal.md), added the same day, is ready
for the same reason (#20, #34 and #47 are built) and also unblocks nothing. It
adds tool arguments and dispatch in `chat.rs` and rewrites `git_service.rs`'s
reads, so it did not pair with #59, #23 or #32's `chat.rs` tasks. It was built
before #59, as recommended here — it removes the approvals, and its reads are
what #59's allowance then counts — and is done (2026-10-23).
[#61](61_continue_the_stopped_turn/proposal.md)–[#64](64_read_only_command_boundary.md),
added the same day from the same session, were all ready; #64's requirement 5
waited for #60's `read_git_diff` and shipped on 2026-10-18. #64's requirements
1–4 closed auto-run holes (`git diff --output=<file>` wrote, `$HOME/…` escaped
the repository, `git show HEAD:.env` read a restricted file) on 2026-10-17, the
day they were found, and its read-only pipelines (requirements 6–7) followed on
2026-10-18. #59, #60, #61, #62, #63 and #64 are done
(2026-10-23). #62 rewrote the model-facing command text that #63 updated in its
Task 4 and changed the approval prompt's first line, so its "does not pair with
another spec that edits `chat.rs`" constraint ended with it. No `Depends on:`
line names #62, so nothing is promoted.
[#65](65_exploration_memory_within_a_turn/proposal.md), added on 2026-10-18 from a
review of #59–#64 and revised on 2026-10-19, was planned as a folder spec
(2026-10-24: [`proposal.md`](65_exploration_memory_within_a_turn/proposal.md),
[`context.md`](65_exploration_memory_within_a_turn/context.md),
[`tasks.md`](65_exploration_memory_within_a_turn/tasks.md)). **Done
2026-10-25.** Investigating a large project is the prerequisite for everything
else Damaian does — an agent that cannot learn a codebase cannot fix, design or
implement in it — and #65 is what makes that investigation stick: #59 gives a
turn the rounds, #61 keeps a stopped turn alive, and #65 keeps what those rounds
found in view once the request window drops them, and carries it into the next
turn. Nothing formally depends on it, and its `chat.rs` pairing constraint ends
with it. The live acceptance is in its §6: the first prompt's default answer
used facts from seven elided calls, the second prompt finished in 10 model
calls with notes against 33 at `0`, and the next turn carried 2,809 characters
before its first request.

Re-checked on 2026-10-24, when [#66](66_plan_left_open_at_turn_end/proposal.md)
was planned. The ready set is unchanged: #66 stays **Not started** until its
Task 1 begins. Three facts about pairing it, from its
[`context.md`](66_plan_left_open_at_turn_end/context.md) §3 and §5:

- **It edits `chat.rs` in Tasks 1–8**, so it may pair only with a spec that
  never edits that file. Neither #55 nor #49's reuse slice qualifies, so #66 is
  a solo `chat.rs` track. #26 and #65 are Done and no longer compete for the
  file.
- **It adds two fields to `PendingChatTurn`** (`step_evidence`,
  `plan_reminder_sent`) at the same seven literals #61's round-budget pause and
  #65's now-built `turn_notes`/call list use. That is a rebase, not a blocker,
  but do it deliberately and keep old pending turns loadable.
- **It sits beside spec 32's `before_task_complete`** at both completion points
  and sends its own reminder only after the hook allows; it does not move the
  hook. A hook may therefore fire twice on a reminded turn, both times charged
  to the same action budget.

Re-checked on 2026-10-25, when [#23](23_verification_loop/proposal.md) started.
#23 leaves the ready set for in-progress; Tasks 1–3 landed the same day and do
not touch `chat.rs`. Its Tasks 4–6 edit `chat.rs` and waited for #26 to merge
(its [`context.md`](23_verification_loop/context.md) §13), so #23 and #26 were
strictly sequential. **#26 closed later the same day**, which unblocked Tasks
4–10. The ready set was then #27, #33, #51, #55 and #65.

Re-checked on 2026-10-25, when
[#66](66_plan_left_open_at_turn_end/proposal.md) was finished. It edited
`chat.rs` as a solo track, so it did not pair with #26 or #65. Two facts from
that work, from #66's
[`context.md`](66_plan_left_open_at_turn_end/context.md) §3 and §5:

- **`PendingChatTurn` now carries `step_evidence` and `plan_reminder_sent`**
  beside the seven literals #61's round-budget pause and #65's
  `turn_notes`/call list use. **#65 has since landed** and added its fields at
  the same sites; all new fields are `#[serde(default)]`, so old pending turns
  still load.
- **#66's reminder sits beside spec 32's `before_task_complete`** at both
  completion points and is sent only after the hook allows; it does not move the
  hook. A hook may therefore fire twice on a reminded turn, both times charged
  to the same action budget.

Re-derived on 2026-10-25, when [#23](23_verification_loop/proposal.md) finished.
One `Depends on:` line named #23 not built: [#35](35_commit_preparation/proposal.md).
**One is promoted:** #35, whose other dependencies (#16, #22 and #31) were
already built. #23 leaves the in-progress set. The ready set is now #27, #33,
#35, #51 and #55.

Re-checked on 2026-10-26, when [#35](35_commit_preparation/proposal.md) was
planned. It moved from the ready set into **In progress** without being built:
the flat spec became a folder, [`context.md`](35_commit_preparation/context.md)
records the pre-implementation state and eighteen corrections, and
[`tasks.md`](35_commit_preparation/tasks.md) holds nine tasks. No dependency
line changes, because nothing waits on #35 until it is Done. What planning found
that matters to the order: committing is not a capability in `ProfileCapabilities`
today, so the spec adds the sixth capability key `allow_git_commits`; the
durable record of acceptance is the session log's `patch_file_decided` events,
which do not yet carry the applied hash or accepted hunks the commit needs; and
message drafting belongs in the desktop shell because the engine holds no model
adapter. #35 never edits `chat.rs`, so it pairs with #55.

Re-derived on 2026-10-26, when [#35](35_commit_preparation/proposal.md) finished.
Two `Depends on:` lines named #35 not built: [#36](36_branch_and_worktree_delivery/proposal.md)
and [#37](37_pull_request_creation.md). **One is promoted:** #36, whose other
named dependency (Phase 2 WP4, worktree isolation) is optional and which is
written to work without it. #35 leaves the
ready and in-progress sets. #37 is still blocked on #33 and #36. What #35 leaves
for the next spec: committing is a Code-mode capability (`allow_git_commits`,
restrict-only from repository config); the desktop repo bar's contextual action
can now show *Commit…*; and the first write path uses a private index, a typed
per-commit approval, the repository's real hooks and a scan of the composed
bytes. Its [`proposal.md`](35_commit_preparation/proposal.md) §7 records what
shipped, the acceptance table and the one manual gap (no browser walk of the
dialog).

Re-derived on 2026-10-26, when
[#55](55_conversation_compaction/proposal.md) finished. No `Depends on:` line
names #55, so **nothing is promoted**. #55 leaves the ready and in-progress
sets; replacing the eight-message window is what #49's reuse slice was waiting
for, but #49's own `Depends on:` line still names Phase 1 WP3, so it stays in
progress. Its [`proposal.md`](55_conversation_compaction/proposal.md) §7 records
the acceptance walk, the live DeepSeek run and the one remaining manual gap (no
browser walk of the app).

Re-checked on 2026-10-26, when [#27](27_context_inspector/proposal.md) was
planned. It moved from the ready set into **In progress** without being built:
the flat spec became a folder, [`context.md`](27_context_inspector/context.md)
records the pre-implementation pipeline, twelve corrections and the resolved
§7 open question, and [`tasks.md`](27_context_inspector/tasks.md) holds ten
tasks. No dependency line changes, because nothing waits on #27 until it is
Done. What planning found that matters to the order: the pre-send view needs a
**new pause kind** in the existing typed-pause machinery, not a rewritten
pipeline, and removal is possible only while that review is paused; `chat.rs` is
therefore a solo track for #27, as it is for #49's reuse slice. Two manifest
extensions are needed before the view is truthful — the #55 conversation summary
was never a context item, and the symbols block carries only its weakest
provenance, not counts.

Re-derived on 2026-10-26, when [#27](27_context_inspector/proposal.md) finished.
One `Depends on:` line named #27 not built: [#28](28_memory_model_and_storage/proposal.md).
**One is promoted:** #28, whose other dependency (#26) was already built. #27
leaves the ready and in-progress sets, and its `chat.rs` pairing constraint ends
with it. #29 remains blocked on #28, and #30 on #28 and #29. What #27 leaves for
the next spec: the send path now has a typed `PausedFor::ContextReview` between
assembly and the first call, `PreparedContext` is the persisted half, and the
manifest carries `conversationSummary` and `symbolProvenance`; a memory category
adds one more `IncludedRef` category and is removable through the same removal
API. #27 also became the first production `LineRange` producer through session
pins, so #26's note that no collection source yields a range now refers to
automatic collection only.

Re-derived on 2026-10-26, when
[#28](28_memory_model_and_storage/proposal.md) finished. Two `Depends on:` lines
named #28 not built: [#29](29_memory_creation_and_consent/proposal.md) and
[#30](30_memory_retrieval_and_lifecycle/proposal.md). **One is promoted:** #29, whose only
dependency #28 satisfied; #30 remains blocked on #29. #28 leaves the ready and
in-progress sets. No `chat.rs` pairing constraint ends with it, because #28 did
not touch `chat.rs`; #30 will be a solo `chat.rs` track when it starts. What #28
leaves for the next two specs: `MemoryStore` is the single write path with the
secret, evidence, scope and sensitivity checks, `MemoryTarget` gives #29 and #30
append-only transitions and recall auditing, `ProjectIdentity::source_of_key`
reads the tier back for the inspector, and the `Memory` context category
deliberately does not exist yet — adding it is #30's task, together with the
end-to-end assertion that an undetectable credential stored directly in the file
is still redacted before it is sent. The scoped local checks passed, and the full
seven-check gate passed in CI on 2026-10-26 for the `main` push.

Re-derived on 2026-10-26, when
[#29](29_memory_creation_and_consent/proposal.md) finished. One `Depends on:`
line named #29 not built: [#30](30_memory_retrieval_and_lifecycle/proposal.md), which is
now promoted; it has no other dependency here. #29 leaves the ready and
in-progress sets. It was a solo `chat.rs` track, so no pairing constraint ends
with it; #30 will be a solo `chat.rs` track too, because retrieval changes the
turn path and the `Memory` context category. What #29 leaves for #30: confirmed
entries and their evidence hashes (`EvidenceRef` with a content hash), a store
whose `insert` still re-checks secrets, evidence eligibility, scope and
sensitivity, and a `MemoryProvenance` that answers why an entry exists. Also
left open deliberately: the `Memory` `ContextCategory` and its `ContextItem`
rendering do not exist, no memory is recalled, no staleness is revalidated in a
turn, the context inspector has no memory row, and the context manifest is still
content-free. #30 owns all of those, including the end-to-end assertion that an
undetectable credential stored directly in a memory file is still redacted
before it is sent. The scoped local checks passed, and the full seven-check gate
passed in CI on 2026-10-26 for the `main` push.

Re-checked on 2026-10-28, when [#36](36_branch_and_worktree_delivery/proposal.md)
was planned. It moved from the ready set into **In progress** without being
built: the flat spec became a folder,
[`context.md`](36_branch_and_worktree_delivery/context.md) records nineteen
corrections and decisions D1–D15, and
[`tasks.md`](36_branch_and_worktree_delivery/tasks.md) holds eight tasks. No
dependency line changes, because #37 still waits on #36 itself. What planning
found that matters to the order: read-only branch state is not wholly absent
(`summary`, the private `ahead_behind` and `merge_base` already exist), but there
is no typed `BranchInfo`, upstream tracking, or conflict prediction; the crate
already creates `damaian/<slug>` branches for spec 57's worker worktrees, and
spec 60 already lists worktree-to-branch identity, so §5.7 is reporting-only
rather than a new worktree feature; Git mutation has only `allow_git_commits`,
so branch mutation adds `allow_git_branches` as a seventh, restrict-only
capability; the proposal's `symbolic-ref` switch would move `HEAD` without the
index or working tree, so switching uses `git switch --no-guess`; and the repo
bar's single action slot is Commit…-first, so branch entry points belong in the
folder menu. A suggested name is built from text passed through the secret
scanner, because a task title can carry a credential. #36 never edits `chat.rs`,
so it pairs with a `chat.rs` spec.

Re-derived on 2026-10-28, when [#36](36_branch_and_worktree_delivery/proposal.md)
finished. One `Depends on:` line named #36 not built:
[#37](37_pull_request_creation.md). It now records #36 as built, but **nothing
is promoted**: #37 also needs #33, which is not built, so it stays blocked. #36
leaves the ready and in-progress sets, leaving #30, #33 and #51 as the ready
set. What #36 leaves for the next spec: branch mutation is a Code-mode
capability (`allow_git_branches`, restrict-only from repository config);
creation, switching and deletion are separate typed approvals consumed once;
deletion has a distinct unmerged confirmation; and merge/rebase preparation is
`merge-tree`-only with no execution path. Its
[`proposal.md`](36_branch_and_worktree_delivery/proposal.md) §7 records what
shipped, the acceptance table and the one manual gap (no browser walk). **CI has
not run for the branch yet**; scoped checks passed locally, and the full
seven-check gate is not claimed until CI is green.

Re-derived on 2026-10-28, when
[#51](51_external_reference_retrieval/proposal.md) finished. **No `Depends on:`
line names #51**, so nothing is promoted by its completion; the prose reference
that assumed it was unbuilt was updated instead
([#55](55_conversation_compaction/proposal.md) §5.6 no longer says "once spec 51
lands"), and the other references (#26, #50, #54) already read as
cross-references rather than dependencies. #51 leaves the ready and in-progress
sets, and its `chat.rs` pairing constraint ends with it. Task 11 then fixed the
review findings against the close-out: a later turn frames the page again, the
fetch refuses non-public destinations and pins the resolved address, proxies are
removed from the child, the egress guard reads the tool result rather than the
file on disk, and a fetch pause is reattachable after a crash.

- **What remains ready:** **#30** memory retrieval and lifecycle, and **#33**
  MCP management. #36 finished earlier the same day (above).
- **#30 and #33 are not safe together:** both edit `chat.rs`. #30 changes the
  turn path and adds the `Memory` category; #33 changes the MCP tool list and
  per-tool dispatch. One must own the file and the other wait or rebase.
- **CI passed on 2026-10-29.** #51's Tasks 1–10 ran scoped checks locally, and
  the close-out ran the task's scoped list; the full seven-check gate then
  passed in CI for the `main` push that contains branch
  `spec-51-external-reference` (Quality run 40902516310).
- **What #51 leaves open, recorded in its §7.3 rather than hidden here:** the
  `Redaction:` line counts only the post-conversion scan, so a page key caught in
  the transport renders as `clean` beside its placeholder; and the injection
  evidence is deterministic-tier only, with a scripted worst-case model and no
  live-provider run. Live DNS rebinding is also unmeasured — the `--resolve` pin
  is proven structurally, not against a racing resolver. No manual browser walk
  was performed either. The crash-reattachment gap the first close-out recorded
  is closed by Task 11.
- **Documentation and changelog:** `USER_GUIDE.md` gained "Reading an external
  page" and the `external_reference` context row, `TROUBLESHOOTING.md` gained "A
  fetch was refused", `SECURITY.md` gained the fetch paragraph and the
  forbidden-host-key rule, and #51's sole `Unreleased` clause was removed from
  `CHANGELOG.md`. No release row was added.

Re-checked on 2026-10-28, when [#30](30_memory_retrieval_and_lifecycle/proposal.md)
was planned. It moved from the ready set into **In progress** before any task
was built: the flat spec became a folder, [`context.md`](30_memory_retrieval_and_lifecycle/context.md)
records the pre-implementation state and seventeen corrections, and
[`tasks.md`](30_memory_retrieval_and_lifecycle/tasks.md) holds eight tasks. No
`Depends on:` line changes, because nothing waits on #30 until it is Done. What
planning found that matters to the order: the context manifest is content-free
and is written to the audit log, so memory statements cannot ride in it — the
manifest gains id-only rows and the inspector resolves them from the store; the
watcher in `index_cache.rs` has no path into the memory store, so retrieval-time
revalidation is the load-bearing implementation and the eager watcher transition
is the deliberate gap; `ContextBudget::shares` was a fixed `[u16; 10]` at
planning (spec 51 took it to 11 and #30's Task 1 to 12), so adding
the category touches the config overlay in the same four files specs 25, 32 and
50 used; and the tokenizer the proposal names is private and has moved. #30 is a
solo `chat.rs` track; #33 and #51 also edit that file and therefore wait, while
#36 does not.

Re-derived on 2026-10-30, when [#30](30_memory_retrieval_and_lifecycle/proposal.md)
was finished. `grep -rn 30_memory_retrieval docs/specs` finds **no `Depends on:`
line naming #30**, so no spec is promoted by dependency. What changes is the
`chat.rs` serialisation: #33 was waiting on #30 for that file, not for a
dependency, and is now the one ready spec. #30 leaves the in-progress set,
leaving it empty. What #30 leaves for later work: the memory management view
(Phase 3b WP3: listing, editing, deleting and purging entries) has the recall
path, `memory_recalled` audit and stale state it needs to show "last used" and
"may be out of date"; any retrieval cache must be keyed by entry id and
invalidated on state change (§5.6); and the phase is not complete until the
two-week real-session observation in its §7.3 is recorded.

### Parallel work

Two specs may be built at once, which is a **deliberate exception to the
one-at-a-time rule above**, under one constraint that comes from this
repository's own history: [#47](47_agent_working_capability/context.md) was
deferred behind #17 because both edit `chat.rs`, and its line numbers drifted
twice in a single session while #17 was in flight. The binding constraint is not
the dependency graph, it is that file.

- **Safe together:** a spec that touches `chat.rs` and one that does not — the
  precedent is #49's now-shipped accounting slice, which stated outright that
  it changed no request and touched only `model.rs`, `config.rs`, `session.rs`,
  `app.js` and the harness, so it could pair with #20, which lives in `chat.rs`
  and the UI.
- **Never together:** #55 and #49's reuse slice — any pair. Both rewrite
  `build_model_prompt` or the turn loop around it; that shared function is why
  #49's second slice was blocked in the first place. #54 and #55 also both
  restructure the message array. #26, #51, #65 and #66 are Done and no longer
  constrain anything.
- **#30 and #33 cannot run together:** both edit `chat.rs`. Pair one of them with
  a spec that does not.
- **Genuinely independent of that cluster:** #33 and #36; the remaining pair
  problem is #30 versus #33, as above.

Running two tracks: give each worktree its own `CARGO_TARGET_DIR` (a shared one
thrashes), agree up front which track owns `chat.rs` so the other rebases, and
run only scoped checks locally: the full gate runs on CI, as
[AGENTS.md](../../AGENTS.md#the-full-gate-runs-on-ci-not-locally) requires, so
two tracks do not contend for the same cores with workspace-wide builds.

**Keeping this honest:** when a spec becomes Done, move it out of the ready set
and promote whatever it was blocking — one item on the checklist in
[`../../AGENTS.md`](../../AGENTS.md)'s "Before you change a feature", which is
the canonical list of what a finished spec makes stale. The step that keeps
*this* section true is flipping the `Depends on:` lines that named the finished
spec **not built**: re-deriving from those lines is how this section was
produced, and it repairs nothing while the lines themselves are wrong.

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
| 12 | [12_web_app_troubleshooting/](12_web_app_troubleshooting/proposal.md) | **Done 2026-09-29.** First-class browser diagnostics for local web-app debugging: page errors, console/network evidence, interaction scenarios, screenshot artifacts, session-scoped diagnostic approval, and safer tool-round handling. Builds on #6, #8, and #10. The close-out ([`tasks.md`](12_web_app_troubleshooting/tasks.md)) added the missing parts. It types the companion's report as `WebDiagnosticDetails`, leads the model text with Damaian's own summary, and persists and streams a redacted record. It also adds a diagnostic card with Reveal in Finder, and was checked against live snake-game runs with DeepSeek. Those runs found and fixed a false refusal in #48 and a lost-reasoning defect in #47's batched rounds, and writing criterion 6's tests tightened remote-URL approval. #22's browser findings now only convert `WebDiagnosticDetails`. |
| 13 | [13_docker_command_support.md](13_docker_command_support.md) | **Done.** Makes Docker a first-class approval-gated command family with Docker-specific risk messaging and diagnostics, while keeping automatic execution limited to sandbox-safe read-only commands. Builds on #3 and #10. |
| 14 | [14_developer_id_signing_and_notarization.md](14_developer_id_signing_and_notarization.md) | **Done.** Shipped in v0.31.0. Roadmap Phase 0 WP1. Replaces ad-hoc signing with Developer ID signing, notarization, stapling, and a fail-closed verification gate, plus stable and preview release channels. Nothing later in the roadmap can be tested with users until this ships. Builds on #9. |
| 15 | [15_install_and_update_verification.md](15_install_and_update_verification.md) | **Partially done, remainder skipped.** Roadmap Phase 0 WP2. The data-directory schema version and its refusal path shipped, wired into the shell, the CLI, and the app. The updater signature fixtures, the update rehearsal, the Keychain measurement, the documentation rewrite, and the second-person install were skipped as verified in practice — see its §7 for what that leaves open. Depends on #14. |
| 16 | [16_session_checkpoints_and_rewind.md](16_session_checkpoints_and_rewind.md) | **Done.** Roadmap Phase 1 WP1. Extends patch rollback into a session checkpoint covering files *and* conversation position, via a shadow Git object store: a working-tree census for approved commands, restore with conflict handling, conversation rewind by append, retention, a per-turn `Rewind` control, and a Settings › Checkpoints list. Settles the secret-redaction-versus-faithful-restore conflict the old rollback snapshots had; shares the session-log `seq` migration with #17. |
| 17 | [17_durable_task_state_and_crash_recovery/](17_durable_task_state_and_crash_recovery/proposal.md) | **Done.** Roadmap Phase 1 WP2. The engine core of durable task state: thirteen task states, parse-first session-log reads, before-and-after action markers, the recovery classifier and three recovery operations. Its central guarantee is that no action whose outcome is unknown is ever automatically repeated, enforced in `recovery::resume`. The recovery prompt moved to #45 and the process registry to #46, **both now unblocked**. |
| 18 | [18_local_evaluation_harness/](18_local_evaluation_harness/proposal.md) | **Done.** Roadmap Phase 1 WP4. New `crates/eval-harness` with a `damaian-eval` binary running seventeen scenarios against fixture repositories across the roadmap metric set; the deterministic tier runs inside the workspace test suite and the live tier is credential-gated. Its first live run on 2026-09-10 found seven defects **in the harness itself**, including two metrics reporting a plausible constant rather than a measurement; all fixed (its §7). Its first OpenRouter run on 2026-10-25 found the live tier calling the provider's default model rather than the requested one (fixed), and the textual-tool-call, CLI provider-switch and reasoning-field findings were fixed afterwards; sending no built-in OpenRouter output ceiling was kept as a recorded decision (observation #44, §7). Nothing from that run remains open. |
| 19 | [19_token_and_cost_accounting/](19_token_and_cost_accounting/proposal.md) | **Done, with one measurement outstanding.** Roadmap Phase 1 WP6. Per-run token usage and per-task aggregation, distinguishing provider-measured from estimated; retries, stopped turns and calls lost to a crash all count. Measures only; the enforced ceiling is Phase 2. **Validated against DeepSeek on 2026-09-10** (the `len / 4` estimate ran 2.5–6.3% high), and **against OpenRouter on 2026-10-25** (reports cost; the estimate erred both ways, up to 29%); OpenAI's own API remains the outstanding measurement. |
| 20 | [20_working_modes/](20_working_modes/proposal.md) | **Done.** Roadmap Phase 2 WP1. Four session modes (Ask, Plan, Code, Review) as a capability boundary rather than a prompt instruction, enforced in three layers: the tool list via `mode_permits`, the fallback prompt, and orchestrator refusals at nine call sites. Two known gaps stay open in its §7: a rejected proposal can be run by id through `/api/run-command`, and `git diff --output=<file>` passes the read-only check. |
| 21 | [21_task_plan_progress_and_budget/](21_task_plan_progress_and_budget/proposal.md) | **Done.** Roadmap Phase 2 WP2. Ordered plan steps with observable evidence, so a step is never complete because the model said so, plus an enforced per-turn token ceiling using #19's accounting, persisted through #17's event log. `propose_plan` supplies titles only and `complete_step` takes no arguments: the model asks to move on, the engine decides from what it observed. |
| 22 | [22_findings_model_and_panel/](22_findings_model_and_panel/proposal.md) | **Done 2026-10-04.** Roadmap Phase 2 WP6. One `Finding` type shared by compiler, test, lint, browser and (later) review sources, with parsers that degrade to one honest generic finding rather than losing a failure, persisted in #17's session log. A session panel filters, opens, dismisses, or sends a selected subset to the agent as a scoped repair. It also closed #21's deferred `Evidence::Findings`. |
| 23 | [23_verification_loop/](23_verification_loop/proposal.md) | **Done 2026-10-25.** After a Code-mode patch settles, a continuation turn discovers per-root checks, runs the approved ones, turns failures into findings, lets the model repair within `agent_tool_retry_limit`, reruns, and reports what was verified. The report's `Checks passed` holds only exit-zero evidence; skipped, stale and unavailable checks are stated separately. Ten tasks; split into a folder and planned 2026-10-09, implemented on 2026-10-25 after #26 merged. [`context.md`](23_verification_loop/context.md) found that a patch is applied after the turn that proposed it has ended, so the loop is a continuation the apply path triggers rather than a step inside a turn. It also found that detection gives a Cargo root `cargo test` and nothing else, so checks are selected by the root that owns each changed file, not by a file-category table. Roadmap Phase 2 WP3. Consumes #21, #22 and #24; unblocks #35. |
| 24 | [24_repository_map_and_monorepo_boundaries/](24_repository_map_and_monorepo_boundaries/proposal.md) | **Done 2026-10-09.** Roadmap Phase 3 WP2, built in nine tasks. A deterministic map of project roots, persisted per repository and rebuilt on an input fingerprint, rendered into model context under `repository_map_max_tokens` (default 800, lower-only from a repository). A command discovered in, or requested for, `packages/api` runs there. Allow Always in a sub-root writes a root-qualified grant, and a plain allowlist entry applies at the repository root only. Containment is always checked against the repository root. Roots are corrected through `project_roots_added`/`project_roots_removed` in repository config, from Settings or `damaian repo-root`. Reuses `detect_project_commands`' manifest list per root rather than inventing a second one. `context.md` records where the code contradicted the design. Not built: frameworks and per-root permission restrictions (`proposal.md` §7.1). |
| 25 | [25_symbol_and_relationship_index/](25_symbol_and_relationship_index/proposal.md) | **Done 2026-10-23 (started 2026-10-13).** Split into a folder and planned 2026-10-09, re-checked against `main` 2026-10-12, built in eleven tasks. [`context.md`](25_symbol_and_relationship_index/context.md) records where the code contradicted the design: spec 46's registry is built, LSP framing is not MCP's, sessions have no close event, and restricted files are indexed. The three pieces that need `chat.rs` remain deferred (§12): an agent-facing symbol tool, task attribution for diagnostics, and symbols in `search_codebase` output. Roadmap Phase 3 WP3. A rescope, not greenfield: `FileRecord.symbols`/`imports` already existed as prefix heuristics. Adds locations, kinds, relationships, a `symbols` context item, on-demand references, and a `Provenance` label separating language-server fact from heuristic guess. Satisfies Phase 2's deferred LSP dependency. A Task 11 follow-up wired the eager definition pass to the first symbol search, so language-server definitions and diagnostics are reachable in the app; the Tauri app's exit path still relies on EOF and the next launch's sweep, as Task 5 recorded. |
| 26 | [26_context_assembly/proposal.md](26_context_assembly/proposal.md) | **Done 2026-10-25** (planned 2026-10-24; Tasks 1–6 on 2026-10-25, Task 7 closing the same day). Roadmap Phase 3 WP4. Replaced the single first-come-first-served budget with per-category allocation whose shares a repository may lower but never raise, range deduplication with ranged reads outside the tool line cap, exclusion records naming why content was dropped or shortened, and a content-free manifest recorded as `context_assembled` (spec 27 later moved the chat record to the send). Also fixed a security hole planning found: the user's prompt and replayed earlier turns now pass through the secret scanner, where both used to reach the provider in cleartext. The estimator is content-aware and calibrated against a live provider measurement (prose 4 B/token, code 3.6, structured 2.4). Its [`context.md`](26_context_assembly/context.md) records the pre-implementation pipeline; [`tasks.md`](26_context_assembly/tasks.md) holds the corrections and the seven tasks. Two open points in its §7 are inherited rather than fixed: no collection source yields a `LineRange` yet, so range merging is implemented and unit-tested but inert, and no Observed capability profile exists, so every limit is `Declared`. |
| 27 | [27_context_inspector/](27_context_inspector/proposal.md) | **Done 2026-10-26** (planned and built 2026-10-26; Tasks 1–10; the manual browser walk is in its §7; seven-check gate green in CI, Quality run 40340511717). Roadmap Phase 3 WP5. Renders #26's manifest pre- and post-send, and adds session pins and a path restriction. The view sees `ContextManifest` and never item content; removal is available only on a paused pre-send review, and `Required` is refused at the API. Its `chat.rs` work was a solo track. Its manual walk exercised the pre-send pause, a Required refusal, an item removed before send (74 → 67 tokens), the post-send Context link, pins and the restriction. It promoted #28. Makes "the user can see what context the agent uses" true; Phase 3b's memory arrives as one more category in this view. |
| 28 | [28_memory_model_and_storage/](28_memory_model_and_storage/proposal.md) | **Done 2026-10-26** (planned and built 2026-10-26; Tasks 1–8). Roadmap Phase 3b WP1. The memory record and store: a `project_key` resolved from the root commit, then a normalized origin, then the path hash — because `repository_id` is a path hash and cannot give the sharing property the roadmap asks for — and append-only JSONL under `<data dir>/memory/` with its own schema marker. Memory refuses detected secrets rather than storing a redaction, checks evidence against `restricted_patterns` and per-directory `.gitignore` on the write path, derives `Sensitivity` by rule (raise-only), and audits every lifecycle transition without a personal or environmental statement. Session scope lives in the session log through a `pub(crate)` `SessionStore` path; user scope is off by default and enforced on read and write. Its [`context.md`](28_memory_model_and_storage/context.md) records the pre-implementation state and the re-checked line references; [`tasks.md`](28_memory_model_and_storage/tasks.md) holds the eight tasks, 52 integration tests and 15 inline unit tests. The project identity (including the shallow-clone limit) and the secret-refusal path are the parts to review first. The full seven-check gate passed in CI on 2026-10-26 for the `main` push. |
| 29 | [29_memory_creation_and_consent/proposal.md](29_memory_creation_and_consent/proposal.md) | **Done 2026-10-26** (planned and built 2026-10-26; Tasks 1–9). Roadmap Phase 3b WP2. The consent gate: `MemoryProposal` and `MemoryEntry` are separate types; `MemoryEntry` is `#[non_exhaustive]` with no constructor except `confirm` and deserialization, and `confirm` requires a `UserConfirmation`. Instruction-shaped candidates from non-user origins, detected secrets, and evidence from protected or ignored paths produce no proposal at all; the refusal is audited with the path and reason, never the statement. Proposals live in the session log, survive a reload, and are confirmed one at a time through a route that takes an id and re-reads the proposal. The `propose_memory` tool and an explicit "remember that" are the two creation paths; the card is a collapsed stack below the conversation. Six deterministic eval scenarios cover repository and external injection, secret refusal, restricted and ignored evidence, no unconfirmed persistence, and user scope off; `run_scenarios` also asserts the no-unconfirmed-persistence invariant on every run. The manual walk on port 4899 confirmed the card, scope choices, Remember, Dismiss and reload behaviour. Its [`context.md`](29_memory_creation_and_consent/context.md) records the pre-implementation state and the decisions; the constructor barrier, the non-user-origin instruction path and evidence eligibility are the parts to review first. The full seven-check gate passed in CI on 2026-10-26 for the `main` push. |
| 30 | [30_memory_retrieval_and_lifecycle/](30_memory_retrieval_and_lifecycle/proposal.md) | **Done 2026-10-30** (planned 2026-10-28; Tasks 1–6 built 2026-10-28, Tasks 7–8 on 2026-10-30; scoped checks passed locally; the full seven-check gate passed in CI on 2026-10-30, Quality run 41063831118). Roadmap Phase 3b WP4. Memory reaches the model only as a `ContextItem` in `Memory`, the last and lowest-priority category (3% share, at most `memory_max_entries`=5 entries), recalled lexically by term overlap with the prompt and named paths, with a match reason the inspector shows. Evidence-derived entries are re-hashed at every recall and marked *may be out of date* in the text the model reads; evidence that becomes restricted or ignored withdraws the entry from recall without deleting it. A send-time re-check drops an entry deleted, expired or withdrawn during a paused review. The manifest and audit carry entry ids, never statements; the inspector resolves them through `POST /api/context-memory` and redacts again. Seven deterministic eval scenarios prove instruction-shaped memory changes nothing; the two spec 18 memory metrics carry proxy values (0.750, 0.125). Not done: the eager watcher transition (retrieval-time revalidation covers the criteria) and the phase's two-week real-session observation, recorded as outstanding in its §7.3. [`context.md`](30_memory_retrieval_and_lifecycle/context.md) records the pre-implementation state and eighteen corrections; [`tasks.md`](30_memory_retrieval_and_lifecycle/tasks.md) holds eight tasks. |
| 31 | [31_permission_profiles/](31_permission_profiles/proposal.md) | **Done (2026-10-06).** Roadmap Phase 4 WP3, built in nine tasks on top of #34's security subset. Four built-in profiles (Read-only, Safe local, Full, Offline private) plus imported custom ones are applied last and can only narrow; `profile ∩ mode` is checked at every refusal point. Two documented limits: `local` is a command-name heuristic, not a sandbox, and Offline private does not stop model traffic. |
| 32 | [32_hooks/](32_hooks/proposal.md) | **Done 2026-10-24** (started 2026-10-07; Tasks 1–10, the last on 2026-10-24; see its `tasks.md`). Roadmap Phase 4 WP2. Nine lifecycle events, hooks as external programs returning a verdict — `deny`/`request_approval`/`warn`/`allow`, with no `approve`, so widening is unrepresentable. A repository may define a hook that stays inactive until reviewed at a hash, and may never touch a user hook. The workspace gate is left to CI; its own §7.1 records one criterion (the context inspector) as met by #27's `HookContext` category row. |
| 33 | [33_mcp_management_and_deferred_discovery.md](33_mcp_management_and_deferred_discovery.md) | **Not started.** Roadmap Phase 4 WP4. Every enabled server's full schema currently reaches every request, outside #26's accounting. Adds a search tool with on-demand schemas, per-tool enable, lazy startup, and treats a remote read-only claim as an assertion that cannot lower an approval requirement. |
| 34 | [34_repository_config_trust_boundary.md](34_repository_config_trust_boundary.md) | **Done — implemented ahead of #14 onward.** Bug-driven, not a roadmap graduation. `apply_overlay` is scope-blind, so repository config overrides user config: a repo-set `shell` runs its own script for every approved command (verified), a repo-set `model_base_url` exfiltrates the API key and code with no approval, and a repo `command_allowlist` executes commands unprompted. Adds a scope-aware overlay, a forbidden-key list, restrict-only merges, and moves `Allow Always` to user scope keyed by repository. Security subset of #31, extracted so it need not wait for Phase 4. |
| 35 | [35_commit_preparation/](35_commit_preparation/proposal.md) | **Done 2026-10-26** (planned 2026-10-26; Tasks 1–9; seven-check gate green in CI, Quality run 40323902715). Roadmap Phase 5 WP1. Damaian's first write to a user's Git repository. Builds the commit through a private index so the user's staging is never touched, runs the repository's real `pre-commit`/`prepare-commit-msg`/`commit-msg`/`post-commit` hooks with the private index in their environment and no `--no-verify`, and closes the trap: `GitService::diff` redacts, so the commit scans the composed bytes, reports categories and counts only, requires a separate per-commit override, and proves each selected path's blob id against the prepared record. The candidate set is the session's uncommitted accepted changes, with hand-edited and command-changed files shown and unselected. Committing is a Code-mode capability (`allow_git_commits`) a profile or a repository may deny, takes its own typed approval, and is bracketed by #17's side-effecting markers with an Inspect view that compares HEAD with the prepared commit. `suggest_commit_message` remains the fallback; the shell may draft with the redacted diff. |
| 36 | [36_branch_and_worktree_delivery/proposal.md](36_branch_and_worktree_delivery/proposal.md) | **Done 2026-10-28** (planned 2026-10-28; Tasks 1–8 plus the review-fix row after the first Done, before merge, in its [`tasks.md`](36_branch_and_worktree_delivery/tasks.md)). The full seven-check gate passed in CI on 2026-10-29 (Quality run 40902516310); scoped checks passed locally. Roadmap Phase 5 WP2. Branch suggestion, creation, ahead/behind, merge-base, and conflict prediction via `merge-tree --write-tree` so preparation provably cannot execute. Create, switch and delete are separate typed one-shot approvals behind `allow_git_branches` in Code mode; switching carries `--no-overwrite-ignore`; an unmerged delete needs a distinct confirmation naming the count and subjects, including when Git's own `-d` reference disagrees with the default-branch count; merge and rebase are reports only, with no execution route. Written to work with no worktree support, since its stated dependency (Phase 2 WP4) is Should-tier and unspecified; spec 57's worker worktrees and spec 60's listing mean §5.7 stays reporting-only rather than building worktree support. |
| 37 | [37_pull_request_creation.md](37_pull_request_creation.md) | **Not started.** Roadmap Phase 5 WP4. Push and publication as two typed approvals that cannot substitute for each other, base branch asked for rather than inferred, force push refused rather than gated, and interrupted PR creation resolved by a duplicate search before any retry. Uses MCP rather than a bespoke client. |
| 38 | [38_subagent_model.md](38_subagent_model.md) | **Not started; deferred 2026-10-09.** Stage 2 (editing subagents) is superseded by #57's worktree workers; stage 1 is re-evaluated once #57 and its result-summary follow-up have shipped, and may then be recorded as abandoned. Its §Deferral says why. Roadmap Phase 6 WP1, and gated on that phase's eight readiness gates being measured first. Subagents as in-process tasks whose capability can only be derived by narrowing a parent's, staged read-only-first. Corrects the roadmap's process-based framing and the flat `CancelToken`. |
| 39 | [39_coordination_and_conflict_handling.md](39_coordination_and_conflict_handling.md) | **Not started.** Roadmap Phase 6 WP2. Exclusive file-ownership claims checked at spawn *and* before every write, serialized patch integration with `base_hash` revalidation between applies, and a combined-check rerun that catches two individually-passing patches failing together. |
| 40 | [40_autonomy_evaluations.md](40_autonomy_evaluations.md) | **Not started.** Roadmap Phase 6 WP7. The decision instrument: same scenarios run in both execution modes, cost amplification as a ratio, and an experimental label *derived* from the recorded comparison rather than chosen. Makes abandoning Phase 6 a first-class recorded outcome. |
| 41 | [41_ui_density_and_action_hierarchy/](41_ui_density_and_action_hierarchy/proposal.md) | **Done.** Collapsed command approval 161px → 104px, patch actions from half the conversation column to their natural width. Usability-driven, not a roadmap graduation: the shell has exactly one button style, so a one-shot `Approve Run` and a persistent `Allow Always` are visually identical, and a stray `min-height: 180px` on base `pre` makes every command approval reserve 180px of empty rationale pane. Introduces the button scale in [`../UI_STYLE_GUIDE.md`](../UI_STYLE_GUIDE.md) and applies it to the approval and patch cards. Presentation only — the approval policy from #10, #12 and #34 is untouched. Also builds `docs/ui-style-guide.html`, a specimen page that loads the shipping stylesheet so the guide has a rendered counterpart that cannot drift. First of three UI specs; the conversation column and the remaining chrome follow. |
| 42 | [42_conversation_column_density/](42_conversation_column_density/proposal.md) | **Done.** Chat log 459 → 606px at 1280×800; chrome down from 43% to 24% of the column. Second of the three UI specs from the same usability review as #41. 43% of the conversation column measured as chrome at 1280×800: the docked context strip folds into the turn that read the files, the thread header trades a hardcoded caption for the active folder and session, role labels move to screen-reader-only, and the composer grows from two rows instead of reserving four. Depends on #41's button scale and disclosure pattern, which it promotes to a shared class. |
| 43 | [43_chrome_density_and_hierarchy/](43_chrome_density_and_hierarchy/proposal.md) | **Done.** Third of the UI specs. Applies the scale to the surfaces #41 and #42 did not reach: settings action buttons 456 → 129px with one primary per group and `.btn-danger` on the destructive ones, every font-size brought onto a type scale that now documents its heading steps, and the terminal tab bar slimmed with the working directory folded into it (body 142 → 183px). Supersedes #41 §3.1 on `.inline-actions`. |
| 44 | [44_composer_control_density/](44_composer_control_density/proposal.md) | **Done.** Composer 149 → 129px, and the 58px right gutter every line paid is gone. Fourth UI spec from the same usability review, covering the one surface #41–#43 did not reach: the attach, model and send controls were 38px and positioned *over* the textarea, which reserved 70px of height and 58px of every line as padding to clear them, and the 220px model pill truncated its own effort label away because model and effort shared one string. Controls moved to a content-sized row below the box, model and effort split into separate `.btn-trigger` entry points onto the one existing popover, and pinned chips moved inside the frame. Adds `.btn-trigger`, the footer-row pattern and two anti-patterns to [`../UI_STYLE_GUIDE.md`](../UI_STYLE_GUIDE.md). Presentation only — model persistence, send/stop behaviour and the menu's contents are untouched. Leaves room in the row for #31's working-mode control without building it. |
| 45 | [45_crash_recovery_prompt.md](45_crash_recovery_prompt.md) | **Done.** Split out of #17: the user-facing half. A card at the top of the affected session names the specific in-flight action ("A patch application was in progress and its outcome is unknown"), offers Resume, Inspect, Mark failed and Abandon, and links `Inspect` to #16's checkpoint. Renders a decision #17 already constrained — the sentence and the reason a resume is absent are built in `recovery.rs`, and every decision is re-classified server-side before it reaches the engine, so a webview cannot claim a classification it was not given. Also the first caller of #17's launch sweep, which authorizes safe tasks without running them: a `waiting_for_model` task must not fire a billed model call because the app was opened. |
| 46 | [46_process_registry_and_orphan_sweep/proposal.md](46_process_registry_and_orphan_sweep/proposal.md) | **Done.** Split out of #17. An **owner-scoped** registry — one JSON file per live child under `<data_dir>/processes/` — covering MCP stdio servers, `curl` model calls, PTY sessions and shell commands, swept at launch, at CLI startup and on Ctrl-C. A recorded PID is killed only when it is still alive *and* its start time matches, so every state that is not provably a live process of ours collapses to "do not kill". |
| 47 | [47_agent_working_capability/](47_agent_working_capability/proposal.md) | **Done.** Split into a folder and designed on 2026-09-17; all eight requirements built and closed out on 2026-09-18: ranged reads, `list_directory`, `search_content`, anchor-based region edits, stoppable/timed/streaming commands, a bounded continuation past the eight-round limit and within-round concurrent dispatch of read-only calls. It adds no autonomy. **The live-tier A/B did not support the spec's premise** (§7.2): **+106% tokens** bought **approval stops falling 17 → 4**, not fewer rounds. |
| 48 | [48_provider_limits_and_backpressure/proposal.md](48_provider_limits_and_backpressure/proposal.md) | **Done.** Roadmap Phase 1 WP7. A provider rate limit ends the turn, and the code written to prevent that cannot see it: a 429 arrives as a parsed error body and returns immediately with no retry, while `is_retryable_message`'s `"rate limit"` and `"429"` arms are reachable only from the transport-failure path — where they never fire, because `curl -sS` exits zero on a 4xx. Captures status and headers via `dump-header` rather than a stdout sentinel a model could emit, classifies refusals from status first and never from prose, honours `Retry-After` under both an attempt count and a wall-clock ceiling, and makes the outcome a `failureKind` on the existing failed state rather than a fourteenth state — three fewer kill-matrix cells for the same information. A refused call records measured zero, so it cannot inflate #19's totals through the estimate written before the call. |
| 49 | [49_prompt_cache_accounting_and_reuse/](49_prompt_cache_accounting_and_reuse/proposal.md) | **In progress.** Split into a folder and planned on 2026-09-18. Roadmap Phase 1 WP8. The **accounting slice** (requirements 1, 2, 3, 6, 8, plus requirement 5's guard) is **Done as of 2026-09-23**: `cached_input_tokens` and a cached rate, so cost is no longer overstated for providers that cache automatically. The **reuse slice** (requirement 4) is the only unbuilt requirement. #26's stable-to-volatile ordering now exists, but the conversation section is still a **sliding window of the last eight messages** that changes the prefix every turn, so the slice waits on #55's replacement; the spec's own `Depends on:` line also names Phase 1 WP3 as unspecified. |
| 50 | [50_model_initiated_clarification/](50_model_initiated_clarification/proposal.md) | **Done 2026-10-24** (planned 2026-10-07; Tasks 1–8, the last on 2026-10-24; the live usage measurement was not run, see its §7). Roadmap Phase 2 WP8. An `ask_user` tool whose load-bearing rule is that an answer can never satisfy an approval, enforced by types rather than discipline. It reuses the existing waiting state and reattach machinery, is bounded to two questions per turn, and is absent from the tool list where no user can answer. |
| 51 | [51_external_reference_retrieval/](51_external_reference_retrieval/proposal.md) | **Done 2026-10-28** (planned and built 2026-10-28; Tasks 1–10 plus Task 11's review fixes on branch `spec-51-external-reference`; the full seven-check gate passed in CI on 2026-10-29, Quality run 40902516310). Roadmap Phase 3 WP8. `fetch_url` reads one URL the model names, under per-host approval in user scope, and returns bounded, redacted text that enters as the eleventh and lowest-priority context category. §5.3 is the feature: the egress guard refuses a URL carrying a secret or content from a file `read_file` returned before any dialog, no redirect is followed, the request carries no credential, non-public destinations are refused and pinned with curl `--resolve`, and the proxy environment is removed. Every later turn re-renders the page in the same data frame. The injection evidence is three deterministic scenarios with a scripted worst-case model — engine guards measured, real-model resistance not. Its §7 records the criterion-to-test table and two honest gaps: `Redaction:` counts only the post-conversion scan, and no manual browser walk was performed or live page fetched. `search_web` was deliberately not built (D2). |
| 52 | [52_mcp_server_mode.md](52_mcp_server_mode.md) | **Not started.** Roadmap Phase 4 WP6. #06 and #33 are client-side only, so Damaian's indexed search, redaction and path policy are reachable only from its own window — the alternative being to give another tool raw filesystem access. Exposes a read-only subset over stdio, with the set computed as the intersection of read-only and profile-permitted rather than listed, so a tool added later is excluded until classified. The repository is fixed at launch and unreachable from the wire; a server has no user, therefore no approval, therefore nothing that writes. |
| 53 | [53_session_search_and_export/proposal.md](53_session_search_and_export/proposal.md) | **Done.** Roadmap Phase 1 WP9. A rescope, not greenfield: listing, renaming and deleting already work end to end, and what is missing is finding anything and getting anything out. A bounded scan through #17's parse-first reader rather than a second index, anchored on `seq` because it survives rewinds. Its sharp edge is inherited from #16: `append_message` stores content verbatim and deliberately does not redact, so that a rewind restores what was actually said — which makes export a display path that writes a file, and redaction on the way out mandatory, with the redaction count stated rather than silently applied. |
| 54 | [54_image_input.md](54_image_input.md) | **Not started.** Roadmap Phase 3 WP9. Lets a user paste a picture into the composer and lets the model see #12's diagnostic screenshots, which today it receives only as a file reference. Its hard part is that **an image defeats `SecretScanner`** and OCR is rejected, so the guarantee becomes informed consent: no unseen image is ever sent, and no setting can make screenshot inclusion automatic. |
| 55 | [55_conversation_compaction/](55_conversation_compaction/proposal.md) | **Done 2026-10-26** (seven-check gate green in CI, Quality run 40323902715). Roadmap Phase 3 WP6 (Must, and in that phase's minimum slice). Conversations were silently **clipped to the last eight messages, each cut to 2,000 characters**, so a constraint stated in the first message was gone at the ninth; compaction appends a `conversation_compacted` event rather than rewriting anything and replaces the window with a token budget (`conversation_window_share`, default 0.25). Its load-bearing rule is that **a model never writes a field the engine can compute**: files changed, plan steps, approvals and validation status come from the session log, and only constraints, decisions, failed approaches and uncertainty are model-written. Automatic compaction is `conversation_auto_compact` (default on, forbidden to repository and profile config); the full transcript stays readable, rewind deactivates the compaction, a failed call produces a facts-only summary, and two injection scenarios prove a summary is data rather than an instruction. |
| 56 | [56_provider_fallback_consent/](56_provider_fallback_consent/proposal.md) | **Done** 2026-10-12. A terminal refusal with a qualifying second provider pauses on a one-shot, refused-by-default offer in the desktop app ("Keep it failed" or "Answer with …", no "always"); approval re-issues the refused call through the fallback and records the switch, and decline fails exactly as #48 does. Split out of #48, it depends on #48's classification and `failureKind` and reuses the pending-approval and resume machinery. |
| 57 | [57_worker_sessions/proposal.md](57_worker_sessions/proposal.md) | **Done 2026-10-16** (Tasks 1–12, started 2026-10-07; acceptance passed with DeepSeek on its re-run, see its §6). Hosts the planner-and-workers workflow behind one user setting, off by default and forbidden to repository config: a *Start worker* button on `worker-prompt` blocks runs the handoff in its own git worktree and process, with a per-project Workers board and pause, resume, stop and steering notes. Its load-bearing rule is that **nothing survives**: a stdin lifeline makes every worker exit when the shell dies, including on `SIGKILL`. |
| 58 | [58_header_repo_bar_and_menus/](58_header_repo_bar_and_menus/proposal.md) | **Done (2026-10-14).** Every acceptance criterion walked in the running app; the full seven-check gate passed in CI. Usability rework of the desktop shell: every "more actions" control becomes one vertical ⋮ menu, the header keeps only title, terminal and ⋮, and a repo bar above the prompt shows folder, branch, `+N −M` and contextual actions such as *Fix N errors*. `githubUrl` is reconstructed from a parsed `origin`, never opened raw, because the remote is repository-controlled. |
| 59 | [59_exploration_round_allowance/](59_exploration_round_allowance/proposal.md) | **Done 2026-10-23** (started 2026-10-18; Tasks 1–6, the last on 2026-10-23; the live re-run completed in 13 model calls with ten read-only rounds and two action rounds, no pause and no approval stop). Specified 2026-10-17 from a reporting-only session that hit *Tool budget exhausted* after eight rounds, five of them read-only. Exception to the graduation rule, like #47, whose read tools made the agent read in more, smaller rounds. A round in which every call is `ActionEffect::ReadOnly` is charged to a separate, clamped allowance (`agent_max_exploration_rounds`, default 24, ceiling 32) instead of `agent_max_tool_rounds`; a round with any command, edit or question, or a read that repeats an earlier one, is charged as today. Reads are not exempted outright because the round cap is the only runaway bound when no token ceiling is set (#21 §5.4). The key is restrict-only, `0` restores today's behaviour, and the pause and stop messages name both counts without assuming the turn was debugging. |
| 60 | [60_worktree_aware_read_tools/](60_worktree_aware_read_tools/proposal.md) | **Done 2026-10-23** (started 2026-10-17; the live re-run of the motivating prompt made no approval stop, see its §7). Adds `list_worktrees`, `read_git_log` and a `worktree` argument on the five existing read tools, because 35 of 40 approval prompts in one session were read-only shell commands in `.claude/worktrees/`. Only worktrees git itself has registered for this repository are reachable, so a symlink a clone ships there is refused. No command-policy change. |
| 61 | [61_continue_the_stopped_turn/proposal.md](61_continue_the_stopped_turn/proposal.md) | **Done 2026-10-23** (started 2026-10-18; Tasks 1–7, the last on 2026-10-23; §6's criteria walked against the tree on 2026-10-23). Specified 2026-10-17: *Continue debugging* starts a new turn that sees only the last eight messages, so a reporting request was lost after one stop and three continuations drifted into debugging a worktree. At the round limit the turn now pauses as a `round_budget` approval, persisted like any `PendingChatTurn`, and *Continue* resumes the same task with every message and plan step, or *Answer now* makes the forced final call. Pausing before the forced call saves the call that was being thrown away. Hard-bounded by `ABSOLUTE_TURN_ROUND_CAP`; completes #47 requirement 6 for users without a token ceiling. The `continue_keeps_state` scenario proves the resumed request still carries the original prompt and the earlier tool results. |
| 62 | [62_approval_prompts_name_the_reason/](62_approval_prompts_name_the_reason/proposal.md) | **Done 2026-10-23** (started 2026-10-18; Tasks 1–7, the last on 2026-10-23; the live re-run named every prompt's cause, see its §6): `command_policy.rs` gains the closed `ApprovalCause` enum and `CommandClassification.causes`, and `reasons` is rendered from it, so every classifier branch and escalation names its cause as a value — including the `Network`/`HighRisk` split and `Blocklisted`/`Destructive`; the `Setting`/`ProfileBlocked` causes name the `require_approval_for_*` key (with its user/repository/profile source, threaded from the `RepositoryConfigReport`) and the `command_access` block respectively. The approval prompt now leads with the deciding cause's plain words, and "sandbox" has left the model-facing command text and the audit status; the pinned Code prompt and its spec-50 fixtures moved with it. The approval card now shows the first cause and the model's stated reason outside the "Why this command" disclosure, in the live, worker and reattached paths. The proposal stores its `causes` (as values, read back tolerantly by older-file rules) and `command_proposal_stored` audits their names in order, so a session can be analysed by cause without changing any decision. The deterministic eval harness now reports each run's approval prompts grouped by their first cause (`approval_prompts_by_cause`, plus a per-run line), and the new `approval_causes` scenario produces two prompts with distinct causes so the grouping is measured. `USER_GUIDE.md`, `TROUBLESHOOTING.md` and `SECURITY.md` now document the card's two lines, how to read a cause, and what runs without asking without claiming a sandbox. Specified 2026-10-17: every approval said "because it cannot run in sandbox mode", and there is no sandbox. The classifier returns structured `ApprovalCause`s, the prompt's first line names the deciding one (which shell control, which keyword, which setting and who set it), the model's own reason is shown beside it, and "sandbox" leaves every user- and model-facing string. Changes no approval decision. |
| 63 | [63_exploratory_command_failures/](63_exploratory_command_failures/proposal.md) | **Done 2026-10-23.** Tasks 1–5 built 2026-10-17/18; Task 6 walked the §6 criteria and documented the purpose split. Commands get a purpose (check, exploration, other) that never influences approval; a failed exploration command records no finding and blocks no plan step, while checks and unknown commands behave as today. Prompted by a zsh glob error and an `ls` of a missing directory becoming open **error** findings; the model is now told its shell and zsh's glob rule. |
| 64 | [64_read_only_command_boundary.md](64_read_only_command_boundary.md) | **Done 2026-10-23.** Found the read-only list is not read-only (`git diff --output=<file>` writes) and replaces the prefix match with a tokenizer, per-program option lists and `PathPolicy` checks on every path argument. It then admits `cd <dir> &&` and `&&`/`|` pipelines of admitted commands into stdin-only filters, and the §5.7 documentation (user guide, security, troubleshooting) was written to close it. |
| 65 | [65_exploration_memory_within_a_turn/](65_exploration_memory_within_a_turn/proposal.md) | **Done 2026-10-25** (planned 2026-10-24; Tasks 1–10 landed and Task 7 was re-checked after #26 the same day; the live acceptance ran on 2026-10-25, see its §6). Specified 2026-10-18, revised 2026-10-19, planned as a folder spec on 2026-10-24 ([`tasks.md`](65_exploration_memory_within_a_turn/tasks.md)). `bounded_messages` sends only the last ~7 rounds, so in a long turn most of what the agent read is gone before it answers. Adds a `turn_notes` tool whose bounded text (default 8,000 characters, restrict-only) is pinned after the seed, never elided, and carried into the next turn of the session; the elision notice names the calls that left the window and a warning names the round about to leave. Requirement 8, the budget interaction, is settled by #59's exploration allowance (default 24, ceiling 32, restrict-only); the call list extends #59's `read_call_key`, covers read tools plus `turn_notes`, and is persisted with a paused turn. Live: prompt 2 finished in 10 model calls with notes against 33 with the key at `0`; the first prompt's default answer used facts from seven elided calls, and the next turn carried 2,809 characters before its first request. |
| 66 | [66_plan_left_open_at_turn_end/](66_plan_left_open_at_turn_end/proposal.md) | **Done 2026-10-25 (implementation, live acceptance, and the full seven-check gate green in CI)** (started 2026-10-24; Tasks 1–10, the last on 2026-10-25; §6's manual and deterministic criteria walked in its §7, and the deterministic `plan_answer_without_complete_step` and `plan_left_open_at_end` scenarios pass). Specified 2026-10-24 from a session that answered in full and left step 1 of 4 *running*. One reminder per turn when the model answers with open steps, sent after `before_task_complete` allows and only when the round and token budgets leave room for a tool-carrying call; step evidence persists in `PendingChatTurn` across every pause and is attached at a true turn end without closing the step; a declined plan's remaining steps are marked skipped; and an ended turn's open step reads "not finished" with "Turn ended with N steps not marked done." Nothing is closed on the model's behalf (#21 requirement 6). |
| 67 | [67_conversation_activity_and_plan_hierarchy.md](67_conversation_activity_and_plan_hierarchy.md) | **Done 2026-10-26** (started and finished the same day; the running-app walk and local checks are in its §7). Usability-driven, not a roadmap graduation: a real reporting session wrote 100 messages for 72 actions, each action a full bubble beside its result, and left the five-step plan expanded under the last message, so the answer was buried. Groups consecutive action/result pairs into one collapsed **Activity** disclosure, puts the plan's step list behind a collapsed `Steps (N)` disclosure, and previews the exact finding-repair prompt before it is sent, warning when the target is an environment/`PATH` failure rather than a code defect. Presentation only: it uses the session log's existing order, changes no stored message or model context, and old sessions group identically. |
| 68 | [68_ui_chrome_and_command_activity/](68_ui_chrome_and_command_activity/proposal.md) | **Done 2026-10-29** (started and finished the same day; the running-app walk and local checks passed, the full gate is left to CI). Usability-driven, like #41–#44, #58 and #67, with a before/after mockup agreed first. A turn's usage row is drawn once rather than under every ungrouped message; a command step inside an Activity reads `$ cmd`, an exit mark and the reason, with output folded and empty streams dropped; an approved command's request folds into its step. The sidebar draws *Workers* only when a project has workers and the update footer only when there is an update, the brand drops the folder name, and project-row controls show on hover or focus. Header icons share one style, *Context* becomes a meter, and composer triggers name what they set. Presentation only, and it edits no `chat.rs`. |
| 69 | [69_interactive_shell_path.md](69_interactive_shell_path.md) | **Done 2026-10-29** (specified and built the same day; local checks passed). The full seven-check gate passed in CI on 2026-10-29 (Quality run 40940844885). Evidence-driven, like #47 and #64: a live session could not find `npm` because commands run in `<shell> -lc`, which never reads `~/.zshrc`. Damaian now asks the user's interactive login shell for its `PATH` once per run, in `$HOME`, under a five-second limit, and gives commands only that `PATH`, in its order; aliases, functions and other variables never cross. `command_path_from_interactive_shell` (user and admin only) turns it off. |
| 70 | [70_read_only_sequences_and_file_reads.md](70_read_only_sequences_and_file_reads.md) | **Done 2026-10-30** (specified 2026-10-29, built 2026-10-29–30; local checks passed). The full seven-check gate passed in CI on 2026-10-30 (Quality run 41028254554). Evidence-driven, like #64 and #69: the model writes its surveys as `;`-separated lines with `echo` headings, `2>/dev/null` and short file reads, and every one asked. The read-only boundary now admits `;` and newlines, `/dev/null` redirects and `2>&1`, `echo`, the git listings (`worktree list`, `branch`, `remote`, `rev-parse`, `rev-list`, `for-each-ref`, `merge-base`), `git -C <dir>`, bare `head`/`tail` filters, and `cat`, `head`, `tail`, `wc`, print-only `sed -n` and non-recursive `grep` on named files inside the repository. Restricted or outside files, recursion, globs and a `cd` line with `;` still ask. Re-measured: 26 of 125 distinct approved commands in the audit log would now run unasked. |
| 71 | [71_provider_editor_shows_token_defaults.md](71_provider_editor_shows_token_defaults.md) | **Done 2026-10-30** (specified and built the same day; local checks passed). The full seven-check gate passed in CI on 2026-10-30 (Quality run 41034494208). Usability feedback, like #67 and #68: a blank *Max output tokens* or *Context budget* in the provider editor said `Automatic` without the number. The placeholders now show the value the engine resolves for the first listed model (`Default: 64,000`), a note lists each model when they differ, and the numbers come from one engine function, `builtin_token_defaults`, that the resolvers also use, so the editor cannot disagree with them. No default changed. |
| 72 | [72_change_requests_reach_the_agent_loop.md](72_change_requests_reach_the_agent_loop.md) | **Done 2026-10-30** (specified and built the same day; local checks passed, the full gate is left to CI). Evidence-driven: Damaian's first implementation task never reached the agent loop, because a word-matching heuristic in `app.js` sent every "add/implement/fix … test/code/docs" request to the one-shot patch flow, which has no tools, reads nothing and runs nothing. With native tool-calling on, every request now runs as an agentic turn; the one-shot flow remains only for providers without native tools, whose text envelope cannot propose a patch. |
| 73 | [73_provider_key_reference_wins.md](73_provider_key_reference_wins.md) | **Done 2026-10-30** (specified and built the same day; local checks passed, the full gate is left to CI). A defect reported in use: a key saved on the DeepSeek provider page was reported `Saved`, but turns sent a revoked key from an older `model_api_key_env=keychain:model-api-key` line, because loading the config applied the global reference after the provider while a per-request provider switch preferred the provider's own `keychain:` reference. The global reference is now applied first in each scope, so loading and switching resolve the same key; a provider without a key reference no longer blanks the global one, and `config-set`'s provider-switch warning names `model_api_key_env` only when it really overrides. |
| 74 | [74_resumed_turns_keep_the_chat_model.md](74_resumed_turns_keep_the_chat_model.md) | **Done 2026-10-30** (specified and built the same day; local checks passed, the full gate is left to CI). A defect reported in use, found while fixing #73: only a turn's first request sent the composer's provider, model and effort. Every route that resumes a paused turn (command, fetch, plan, question, round budget, context, verification, checks, the two non-streaming command fallbacks) and manual compaction built their engine from the saved config, so with no `model_provider` line every approval failed with `OPENAI_API_KEY is required`, and with one they could run on another provider or key than the turn began with. They now apply the same form fields as the first request; the fallback offer keeps its own provider. |
| 75 | [75_first_provider_becomes_the_default.md](75_first_provider_becomes_the_default.md) | **Done 2026-10-30** (specified and built the same day; local checks passed, the full gate is left to CI). A defect found reviewing a clean install: no Settings control has ever written `model_provider`, so a config made by the Quick start kept the built-in OpenAI default. Each new project's picker started on OpenAI, a worker session (which takes the configured provider, #57) failed to spawn with `OPENAI_API_KEY is required`, and the commit-message draft was silently skipped. Saving a provider now writes `model_provider` when none is set; renaming or removing the default provider keeps the line pointing at a configured one; and the commit draft uses the chat's selection like a turn. |
| 76 | [76_worktree_tasks_go_to_a_worker.md](76_worktree_tasks_go_to_a_worker.md) | **Done 2026-10-30** (specified and built the same day; local checks passed, the full gate is left to CI). A defect reported in use: asked to work in a new worktree, a session created one, read from it through #60's `worktree` argument, then edited the main checkout, because edits always land in the session's own checkout. #60 had left writing to a worktree a non-goal and said such a patch was refused, but nothing refused it. `edit_file` and `propose_patch` now refuse a path inside another registered worktree (a worker's own checkout is not foreign to it), their descriptions say where changes land, and with Worker sessions on the system prompt sends a branch or worktree request to a worker. No write path to worktrees was added. |
| 77 | [77_continue_the_task_after_verification.md](77_continue_the_task_after_verification.md) | **Done 2026-10-30** (specified and built the same day; local checks passed, the full gate is left to CI). A defect reported in use: a nine-step implementation task stopped after its first patch. A patch ends the turn that proposed it, and the #23 verification after the apply ended the task with its report, so nothing went back to the plan. When a verification does not fail and the carried plan still has open steps of the task's own, the model is now asked to continue in the same turn, with the user's request and the open steps; its next patch ends that turn and the cycle repeats. A failed verification, and a change without a plan, end as before. |
| 78 | [78_skipped_checks_are_not_passed.md](78_skipped_checks_are_not_passed.md) | **Done 2026-10-30** (specified and built the same day; local checks passed, the full gate is left to CI). A defect reported in use: a verification whose two checks were both declined recorded `verification_finished` with outcome `passed`, against #23's rule that an unrun check is never represented as passed; a shell test had pinned it. A verification with nothing failed or stale and nothing passed now records `nothing_verified`, its summary reads "No check ran: N checks skipped, so nothing was verified", and the report card shows it in the amber of a due verification. The task stays complete, and #77 still continues an open plan. |
| 79 | [79_context_window_and_turn_budget.md](79_context_window_and_turn_budget.md) | **Done 2026-10-30** (specified and built the same day; local checks passed, the full gate is left to CI). A defect reported in use: wanting the editor to show DeepSeek's 1M window, the user put it into `context_token_budget`, which is #26's per-turn packing budget, so every turn assembled about 830,000 tokens of whole files. The model's window is now its own provider field, `context_window` (built in as 1,000,000 for DeepSeek V4), the packing budget is clamped to it, and the editor shows the two as **Context per turn** and **Model context window**. The manifest carries the window and what a turn adds beside the assembled context, and the header meter and a Claude-style overview in the context panel measure the whole request against the window. |
| 80 | [80_worker_verifies_after_its_patch.md](80_worker_verifies_after_its_patch.md) | **Done 2026-10-30** (specified and built the same day; local checks passed, the full gate is left to CI). A defect reported in use: a worker proposed a patch, the user applied it, and the worker stayed idle, because the worker's patch card was drawn without the hook that starts verification, so neither #23's verification nor #77's continuation ran; the card also stayed up, and a second Apply failed the hash check. The worker now takes `verify` (runs `verify_due` and the continuation in its own process, which holds the session and the key) and `checks` (a check card's decisions); the worker's patch card sends `verify` once settled, and the new job clears the card. #79 is the context budget work. |
| 81 | [81_thinking_phase_while_the_model_reasons.md](81_thinking_phase_while_the_model_reasons.md) | **Done 2026-10-30** (specified and built the same day; local checks passed, the full gate is left to CI). Usability feedback from use: with DeepSeek thinking mode a model call often streams reasoning for 30–60 s before any answer text, and the indicator said only "Waiting for model", so the turn felt stuck. The provider's streamed reasoning (`reasoning_content`, OpenRouter's `reasoning`/`reasoning_details`) is now counted as it arrives and shown as "Thinking · 1.2k chars", in the chat and in a worker's live view, updated at most twice a second; the text itself is never shown, and the stored and replayed reasoning is unchanged. |
| 82 | [82_check_card_says_what_submit_does.md](82_check_card_says_what_submit_does.md) | **Done 2026-10-31** (specified and built the same day; local checks passed, the full gate is left to CI). A defect reported in use: a worker's check card with two `cargo test` checks was answered 6 s after it appeared, and both checks were declined, because the boxes start unticked and **Submit checks** quietly sent "declined" for every unticked box. The boxes still start unticked, since each check is an approval. The button now says what it will do (**Skip all checks**, **Run 2 checks**, **Run 1, skip 1**) and updates as boxes change, and a **Run all** button runs every check in one click. |
| 83 | [83_typed_turns_pick_up_the_open_plan.md](83_typed_turns_pick_up_the_open_plan.md) | **Done 2026-10-31** (specified and built the same day; local checks passed, the full gate is left to CI). A defect reported in use: the first task of a session wrote a seven-step plan and ended with its steps open, and each "continue" the user typed started a new task with no plan, so the verification after a later patch carried none and #77 had nothing to continue. A turn typed in Code mode now picks up the session's latest plan while it has open steps of its own (not only `verify_` steps), unless the turn that last carried it left it untouched. A carried plan keeps the user's approval, and the model may replace one it has not started on when the user has moved on. The hand-over carries after a stop are unchanged. |
| 84 | [84_redaction_markers_never_reach_a_patch.md](84_redaction_markers_never_reach_a_patch.md) | **Done 2026-10-31** (specified and built the same day; local checks passed, the full gate is left to CI). A defect reported in use: a worker's test line `let mut on_token = |_token: &str| {};` was read as a credential assignment, so the model saw a `[REDACTED_…]` marker in its place, copied the marker into its patch, and the applied file no longer compiled. A value starting with `|` (a closure) or `&` (a borrow) is now a runtime expression and is not flagged. `create_patch` refuses new content that holds a redaction marker the file does not already hold, naming the file and line. In the agent loop the refusal comes back as the tool's result, so the model can retry. Otherwise a marker would silently replace a real secret. |
| 85 | [85_verify_after_a_command_changes_files.md](85_verify_after_a_command_changes_files.md) | **Done 2026-10-31** (specified and built the same day; local checks passed, the full gate is left to CI). A defect reported in use: an agent that edited a file with an approved `python3` heredoc was never verified, because only a settled patch made verification due. An approved command in a Code-mode turn that changes files, as the command census of #16 sees them, now records a `Due` carrying those paths, and the turn ends with the verification once it finishes. In a repair turn the command counts as the repair. A command that changes nothing, or whose census could not be taken, starts none. |
| 87 | [87_delete_a_worker_session.md](87_delete_a_worker_session.md) | **Done 2026-10-31** (specified and built the same day; local checks passed, the full gate is left to CI). A defect reported in use: a worker's ⋯ menu had no way to delete its session, and **Remove worktree** refused a worktree already removed outside Damaian, so the row could not be cleared. The menu now has **Delete session**, which stops a running worker first and keeps its worktree and branch. **Remove worktree** on a worker worktree that is already gone prunes git's record of it and says so, and any other missing path is still refused. Number 86 is reserved for the edit-tools spec given to a Damaian worker the same day. |
| 88 | [88_flag_only_secrets_a_patch_adds.md](88_flag_only_secrets_a_patch_adds.md) | **Done 2026-10-31** (specified and built the same day; local checks passed, the full gate is left to CI). A defect reported in use: every patch to `chat.rs` was flagged as possibly adding a hardcoded secret, because its tests hold fake tokens and #07 scanned the whole file. The preview and the apply-time block now flag only matches the patch adds, compared per occurrence against the file's current matches. The commit flow's own whole-file scan is unchanged. |
| 89 | [89_commit_draft_names_the_users_request.md](89_commit_draft_names_the_users_request.md) | **Done 2026-10-31** (specified and built the same day; local checks passed, the full gate is left to CI). A defect reported in use: commit drafts read `chore: <file> The user reviewed the plan and approved it…` and `chore: <file> continue`, taken from the latest user-role message. The draft now uses the latest prompt the user typed to start a turn, skipping Damaian's own turns and bare continuations, and cut to its first line and 72 characters. |
| 90 | [90_one_check_per_place_it_runs.md](90_one_check_per_place_it_runs.md) | **Done 2026-10-31** (specified and built the same day; local checks passed, the full gate is left to CI). A defect reported in use: a check card listed `cargo test` twice, at the root and in `crates/workspace-engine`, without saying where either ran. The card now names each check's directory. A member's `cargo test` is not proposed when a selected virtual-workspace root's `cargo test` is known to run it, read conservatively from `Cargo.toml`, and any other case keeps both. |
| 91 | [91_commit_button_only_when_it_can_act.md](91_commit_button_only_when_it_can_act.md) | **Done 2026-10-31** (specified and built the same day; local checks passed, the full gate is left to CI). A defect reported in use: **Commit…** seemed to do nothing. With no session open it returned silently, and while a commit was being prepared it gave no sign, so a second press opened a second dialog that went stale. The button is now shown only with a session open, reads **Preparing…** while it works, and opens one dialog at a time. |
| 92 | [92_rerun_reuses_its_plan_steps.md](92_rerun_reuses_its_plan_steps.md) | **Done 2026-10-31** (specified and built the same day; local checks passed, the full gate is left to CI). A defect reported in use: a verification rerun appended a new `verify_` step for every check, numbered from `verify_1` again, beside the earlier attempt's. The pass closed the old step by id and left the new one open, so a passed run read "Stopped before finishing". A rerun now reuses the step of a check it already has, reset for the attempt, and adds a step only for a new check. |
| 93 | [93_recovery_cards_follow_the_conversation.md](93_recovery_cards_follow_the_conversation.md) | **Done 2026-10-31** (specified and built the same day; local checks passed, the full gate is left to CI). A defect reported in use: after Damaian was stopped and `main` reset, a session's first prompt sat below six recovery cards about 3,450 px tall. Three of the cards offered patches the log had already applied, which now looked unapplied again. Recovery no longer offers a patch whose every file has a decision, and its cards are drawn below the conversation instead of above it. |
| 94 | [94_run_a_plan_in_a_worker.md](94_run_a_plan_in_a_worker.md) | **Done 2026-10-31** (specified and built the same day; local checks passed, the full gate is left to CI). A gap reported in use: the offer to run work in a worker came only from the model's `worker-prompt` block, which it wrote once and then stopped writing, so a spec-sized task ran in the main checkout. With Worker sessions on, a plan review card now offers **Run in a worker**. It opens the New worker dialog with the request and the plan as the card has it, and once the worker starts it ends the paused turn without a model call, its steps skipped and the hand-over audited. |
| 95 | [95_one_workers_panel.md](95_one_workers_panel.md) | **Done 2026-10-31** (specified and built the same day; local checks passed, the full gate is left to CI). A usability gap reported in use: the sidebar's per-project **Workers** entry repeated the workers already listed under their parent sessions, and each board covered one project. The entry is gone; **Workers** on the header ⋮ opens one board for every project, with a Project column, and `GET /api/workers` without `repo` lists them all. |
| 96 | [96_continue_repair_at_the_limit.md](96_continue_repair_at_the_limit.md) | **Done 2026-10-31** (specified and built the same day; local checks passed, the full gate is left to CI). A defect reported in use: the repair limit defaulted to 2 and a worker had no way to continue after it, so an interrupted draft let only workers repair past the limit. The default is now 16, every session still stops at the limit, and a **Continue repair** action starts exactly one more attempt on the user's explicit decision, for ordinary and worker sessions. The failed plan and evidence carry into that attempt, every approval still applies, a second activation is refused, and the offer is redrawn from the durable log after a reload. |
| 97 | [97_reuse_planned_verification_steps.md](97_reuse_planned_verification_steps.md) | **Done 2026-10-31** (specified and built the same day; local checks passed, the full gate is left to CI). A task plan can name `cargo test` as its final step, but automatic verification selected the same command after every patch and activated a second `verify_` step beside the carried primary work. Verification now defers an exact planned whole-suite check until the worker reaches its final plan step, and leaves a secondary check pending while another primary step is active, so every durable log prefix preserves the one-in-progress invariant. |
| 98 | [98_task_scoped_protected_patch_paths.md](98_task_scoped_protected_patch_paths.md) | **Not started.** A worker ignored a direct no-touch instruction and proposed deleting an unrelated file after a failed check. Direct user path exclusions will become durable, task-scoped patch fences enforced before proposal and again before apply, across verification, repairs, worker restart, and reload. A rejected file alone remains a review decision, not an implicit permanent ban. |
| 99 | [99_worker_task_outcomes.md](99_worker_task_outcomes.md) | **Done 2026-10-31** (specified and built the same day; local checks passed, the full gate is left to CI). A worker's `idle` process state hid whether its task had completed, needed a decision, or had stopped short. The worker session now derives an explicit outcome and next action from durable task status, approvals, errors and progress timestamps; the board also treats an idle pending decision as **Needs you**. |
| 100 | [100_worker_return_and_inline_choice.md](100_worker_return_and_inline_choice.md) | **Not started.** A completed worker currently leaves its parent conversation at the original handoff and offers no result flow back. Workers will return one redacted, durable terminal outcome for the parent to explicitly continue, and every suggested worker prompt will offer **Work here instead** as well as **Start worker**. |

**Ordering exception.** Numbers are assigned in creation order, so build order
can differ from number order. #34 was built ahead of #14–#33:
it is a bug-driven spec covering a security weakness found while writing #31,
and it does not depend on any of them. Where #34 and #31 overlap, #34 is authoritative for the repository-config
trust boundary and #31 for the profile machinery built on it.

Each spec's status is tracked at the top of its file: `Not started`, `In progress`, or `Done` (#15 is the one exception: `Partially done, remainder skipped`).

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
  spec states what to do until it lands — as [#55](55_conversation_compaction/proposal.md)
  §5.1 did for the context budget, [#49](49_prompt_cache_accounting_and_reuse/proposal.md)
  §5.3 for its prefix test, and [#31](31_permission_profiles/proposal.md) §7 for #34 — or
  it says plainly that the work is blocked. The pattern
  [#18](18_local_evaluation_harness/proposal.md) established is the model:
  ship the part you can, report the rest as `notApplicable` naming what will
  supply it, and never be quietly absent.

Writing the line is also the point at which "what if this isn't built yet?"
becomes unavoidable. That question is cheap to answer while specifying and
expensive to answer halfway through an implementation.

## Spec layout

#41 started as a trial of a folder layout, and it earned its keep: most specs
planned since then are folders (12, 17–25, 31, 32, 35, 41–44, 46–50, 53, 56–63),
while the rest remain single files. A single-file spec becomes a folder when it
is planned into tasks.

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

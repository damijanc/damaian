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

### Current state (2026-10-23)

A summary of each spec's own `Status:`, `tasks.md` and `Depends on:` lines on
this date. Where it disagrees with them, they win; `npm run specs:check`
reports the disagreement.

- **In progress:** #25 symbols (Tasks 1–8 of 11), #32 hooks (1–4 of 10), #49
  prompt cache (accounting slice done; the reuse slice is blocked on #55 or
  #26), #50 clarification (1–3 of 8).
- **Ready, not started:** #23 verification loop; #65 exploration memory (its
  requirement 8's budget interaction is settled by #59, now done).
- **Blocked:** #26 on #25; #27, #33, #51, #54 and #55 on #26; #28 on #26 and
  #27; #29 on #28; #30 on #28 and #29; #35 on #23; #36 on #35; #37 on #33, #35
  and #36; #52 on #33; #39 on #38; #40 on #38 and #39.
- **Deferred:** #38 (stage 1 is due for re-evaluation now that #57 is done).
- **Pairing:** two specs may run together only if at most one of them edits
  `chat.rs`, and #26, #55 and #49's reuse slice never run together — see
  [Parallel work](#parallel-work).
- **Open decision:** the priority table below predates #59–#65 and does not
  rank them; the text after it calls #65 the priority now that #59 is done.
  Where #65 ranks against #25 → #26 is the owner's call.

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
[#51](51_external_reference_retrieval.md), but both still wait on #26, and #33
waits on #31 as well.
[#49](49_prompt_cache_accounting_and_reuse/proposal.md)'s accounting slice
shipped on 2026-09-23; its one remaining requirement (the reuse slice) is
blocked on #55 or #26, not ready, so the spec as a whole does not belong in
this set. Everything else is transitively blocked, and the graph is unusually
linear:

```
#20 working modes (done) ──> #22 findings (done) ──> #23 verification, #32 hooks, #35 commit prep
                        │                       └──> #24 repo map (done) ──> #25 symbols
                        │                                               └──> #26 context assembly (also needs #25)
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

**#25 and #26 are the keystones now that #24 has landed.** #25 is the last
thing in front of #26, and the whole tail waits on #26. Preferring a spec that
unblocks nothing over one of these costs more than it looks like it does.

| | Build | Why here |
|---|---|---|
| 1 | **#25 → #26** | The keystone now that #24 is done: #25 is in progress, and #26, which waits only on #25, is the real unlock |
| 2 | #23 verification | Ready now that #22 is done; the last thing in front of #35. Its loop lives in `chat.rs`, so check the parallel rule below before pairing it |
| 3 | #32 hooks | In progress (Tasks 1–4 of 10 done); unblocks nothing, but it is the one Must-tier Phase 4 package that could start |
| 4 | #55 compaction | Also unblocks #49's reuse slice, which is why that slice waits |
| 5 | #27, #54, #51 | The fan-out once #26 has landed |
| 6 | #28 → #29 → #30 | Memory, strictly serial |
| 7 | #35 → #36/#37; #33 → #52 | Delivery and extensibility clusters |
| 8 | #38/#39/#40 | Last, and #40 may legitimately conclude "do not". #38 is deferred; #57 is done, so its stage 1 is due for re-evaluation |

[#56](56_provider_fallback_consent/proposal.md), the opportunistic one, is done
(2026-10-12): it closed the half of #48 that had shipped as only a negative
guarantee.
[#50](50_model_initiated_clarification/proposal.md) is in progress (Tasks 1–3 of 8 done) and unblocks nothing,
which is why it sits off the table rather than above #25.
[#57](57_worker_sessions/proposal.md), added on 2026-10-03, sits off the table
for the same reason: every dependency (#17, #20, #46) is built and nothing
depends on it. It is done (2026-10-16).
[#58](58_header_repo_bar_and_menus/proposal.md), added on 2026-10-13, is done
(2026-10-14). Re-derived then: no `Depends on:` line names #58, so nothing is
promoted. #35 and #37 name it only as a cross-reference: they add *Commit…*
and *Create PR* to its contextual-action slot when they are built.
[#49](49_prompt_cache_accounting_and_reuse/proposal.md)'s accounting slice is
done, off this table for the same reason #56 is: nothing left in it is
unbuilt. Its reuse slice re-enters this table once #55 or #26
unblocks it.
[#59](59_exploration_round_allowance/proposal.md), added on 2026-10-17, sits
off the table like #50 and #57: every dependency (#21, #34, #47) is built and
nothing depends on it, so it joined the ready set beside #23; it is done
(2026-10-23). It rewrites the round loop's counting in `chat.rs`, so it did not
pair with #23 or with #32's `chat.rs` tasks; it could pair with #25, which never
edits `chat.rs`.
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
[#65](65_exploration_memory_within_a_turn.md), added on 2026-10-18 from a
review of #59–#64 and revised on 2026-10-19, is ready: #20, #34, #47 and now
#59 are built. Its requirement 8's budget interaction is settled by #59's
allowance (default 24, ceiling 32, restrict-only). **It is the next priority.**
Investigating a large project is the prerequisite for everything else Damaian
does — an agent that cannot learn a codebase cannot fix, design or implement in
it — and #65 is what makes that investigation stick: #59 gives a turn the
rounds, #61 keeps a stopped turn alive, and #65 keeps what those rounds found in
view once the request window drops them, and carries it into the next turn.
Nothing formally depends on it, but new feature work should not jump ahead of
it. It edits `bounded_messages`, `build_model_prompt` and the turn loop in
`chat.rs`, so it does not pair with another spec that edits that file; #62 has
finished, so nothing holds it back. Build it next, and its per-turn call list is
the one #59's Task 3 added.

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
- **Never together:** #26, #55 and #49's reuse slice. All three rewrite
  `build_model_prompt` and the assembly around it; that shared function is why
  #49's second slice is blocked in the first place. #54 and #55 also both
  restructure the message array.
- **Genuinely independent once #26 lands:** #27, #51 and #23.

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
| 18 | [18_local_evaluation_harness/](18_local_evaluation_harness/proposal.md) | **Done.** Roadmap Phase 1 WP4. New `crates/eval-harness` with a `damaian-eval` binary running seventeen scenarios against fixture repositories across the roadmap metric set; the deterministic tier runs inside the workspace test suite and the live tier is credential-gated. Its first live run on 2026-09-10 found seven defects **in the harness itself**, including two metrics reporting a plausible constant rather than a measurement; all fixed (its §7). |
| 19 | [19_token_and_cost_accounting/](19_token_and_cost_accounting/proposal.md) | **Done, with one measurement outstanding.** Roadmap Phase 1 WP6. Per-run token usage and per-task aggregation, distinguishing provider-measured from estimated; retries, stopped turns and calls lost to a crash all count. Measures only; the enforced ceiling is Phase 2. **Validated against DeepSeek on 2026-09-10** (the `len / 4` estimate ran 2.5–6.3% high); OpenAI remains the outstanding measurement. |
| 20 | [20_working_modes/](20_working_modes/proposal.md) | **Done.** Roadmap Phase 2 WP1. Four session modes (Ask, Plan, Code, Review) as a capability boundary rather than a prompt instruction, enforced in three layers: the tool list via `mode_permits`, the fallback prompt, and orchestrator refusals at nine call sites. Two known gaps stay open in its §7: a rejected proposal can be run by id through `/api/run-command`, and `git diff --output=<file>` passes the read-only check. |
| 21 | [21_task_plan_progress_and_budget/](21_task_plan_progress_and_budget/proposal.md) | **Done.** Roadmap Phase 2 WP2. Ordered plan steps with observable evidence, so a step is never complete because the model said so, plus an enforced per-turn token ceiling using #19's accounting, persisted through #17's event log. `propose_plan` supplies titles only and `complete_step` takes no arguments: the model asks to move on, the engine decides from what it observed. |
| 22 | [22_findings_model_and_panel/](22_findings_model_and_panel/proposal.md) | **Done 2026-10-04.** Roadmap Phase 2 WP6. One `Finding` type shared by compiler, test, lint, browser and (later) review sources, with parsers that degrade to one honest generic finding rather than losing a failure, persisted in #17's session log. A session panel filters, opens, dismisses, or sends a selected subset to the agent as a scoped repair. It also closed #21's deferred `Evidence::Findings`. |
| 23 | [23_verification_loop/](23_verification_loop/proposal.md) | **Not started; split into a folder and planned 2026-10-09.** See `tasks.md` for the ten-task breakdown. [`context.md`](23_verification_loop/context.md) found that a patch is applied after the turn that proposed it has ended, so the loop is a continuation the apply path triggers rather than a step inside a turn. It also found that detection gives a Cargo root `cargo test` and nothing else, so checks are selected by the root that owns each changed file, not by a file-category table. Roadmap Phase 2 WP3. Sequences apply → discover checks → run → find → repair → rerun → report, driven by the orchestrator rather than the model, so a completion report distinguishes verified from assumed. Consumes #21, #22 and #24. |
| 24 | [24_repository_map_and_monorepo_boundaries/](24_repository_map_and_monorepo_boundaries/proposal.md) | **Done 2026-10-09.** Roadmap Phase 3 WP2, built in nine tasks. A deterministic map of project roots, persisted per repository and rebuilt on an input fingerprint, rendered into model context under `repository_map_max_tokens` (default 800, lower-only from a repository). A command discovered in, or requested for, `packages/api` runs there. Allow Always in a sub-root writes a root-qualified grant, and a plain allowlist entry applies at the repository root only. Containment is always checked against the repository root. Roots are corrected through `project_roots_added`/`project_roots_removed` in repository config, from Settings or `damaian repo-root`. Reuses `detect_project_commands`' manifest list per root rather than inventing a second one. `context.md` records where the code contradicted the design. Not built: frameworks and per-root permission restrictions (`proposal.md` §7.1). |
| 25 | [25_symbol_and_relationship_index/](25_symbol_and_relationship_index/proposal.md) | **In progress: Tasks 1–8 of 11 done (started 2026-10-13). Split into a folder and planned 2026-10-09, re-checked against `main` 2026-10-12.** See `tasks.md` for the eleven-task breakdown. [`context.md`](25_symbol_and_relationship_index/context.md) records where the code contradicted the design: spec 46's registry is built, LSP framing is not MCP's, sessions have no close event, and restricted files are indexed. It also records the three pieces that need `chat.rs` and are deferred (§12): an agent-facing symbol tool, task attribution for diagnostics, and symbols in `search_codebase` output. Roadmap Phase 3 WP3. A rescope, not greenfield: `FileRecord.symbols`/`imports` already exist as prefix heuristics. Adds locations, kinds, relationships, and a `Provenance` label separating language-server fact from heuristic guess. Satisfies Phase 2's deferred LSP dependency. |
| 26 | [26_context_assembly.md](26_context_assembly.md) | **Not started.** Roadmap Phase 3 WP4. Replaces the single first-come-first-served budget with per-category allocation, range deduplication, and a manifest recording what was excluded and why. Adds line ranges to `ContextItem`, without which two of its requirements are unexpressible. |
| 27 | [27_context_inspector.md](27_context_inspector.md) | **Not started.** Roadmap Phase 3 WP5. Renders #26's manifest pre- and post-send, and adds pins and path restrictions. Makes "the user can see what context the agent uses" true; Phase 3b's memory arrives as one more category in this view. |
| 28 | [28_memory_model_and_storage.md](28_memory_model_and_storage.md) | **Not started.** Roadmap Phase 3b WP1. The memory record and store, with a `project_key` resolved from the repository's root commit — because `repository_id` is a path hash and cannot give the sharing property the roadmap asks for. Memory refuses secrets rather than redacting them. |
| 29 | [29_memory_creation_and_consent.md](29_memory_creation_and_consent.md) | **Not started.** Roadmap Phase 3b WP2. The consent gate: `MemoryProposal` and `MemoryEntry` are separate types with one conversion requiring a user confirmation, so unconfirmed persistence is unrepresentable. Instruction-shaped candidates from untrusted origins get no proposal at all. |
| 30 | [30_memory_retrieval_and_lifecycle.md](30_memory_retrieval_and_lifecycle.md) | **Not started.** Roadmap Phase 3b WP4. Memory reaches the model only as a `ContextItem` in the lowest-priority category, is visible and removable in #27, and is marked stale by hash comparison against its evidence. Proven by prompt-injection evals, not design argument. |
| 31 | [31_permission_profiles/](31_permission_profiles/proposal.md) | **Done (2026-10-06).** Roadmap Phase 4 WP3, built in nine tasks on top of #34's security subset. Four built-in profiles (Read-only, Safe local, Full, Offline private) plus imported custom ones are applied last and can only narrow; `profile ∩ mode` is checked at every refusal point. Two documented limits: `local` is a command-name heuristic, not a sandbox, and Offline private does not stop model traffic. |
| 32 | [32_hooks/](32_hooks/proposal.md) | **In progress.** Split into a folder and planned on 2026-10-07 in ten tasks; Tasks 1–4 done, the last on 2026-10-23 (see its `tasks.md`). Roadmap Phase 4 WP2. Nine lifecycle events, hooks as external programs returning a verdict — `deny`/`request_approval`/`warn`/`allow`, with no `approve`, so widening is unrepresentable. A repository may define a hook that stays inactive until reviewed at a hash, and may never touch a user hook. |
| 33 | [33_mcp_management_and_deferred_discovery.md](33_mcp_management_and_deferred_discovery.md) | **Not started.** Roadmap Phase 4 WP4. Every enabled server's full schema currently reaches every request, outside #26's accounting. Adds a search tool with on-demand schemas, per-tool enable, lazy startup, and treats a remote read-only claim as an assertion that cannot lower an approval requirement. |
| 34 | [34_repository_config_trust_boundary.md](34_repository_config_trust_boundary.md) | **Done — implemented ahead of #14 onward.** Bug-driven, not a roadmap graduation. `apply_overlay` is scope-blind, so repository config overrides user config: a repo-set `shell` runs its own script for every approved command (verified), a repo-set `model_base_url` exfiltrates the API key and code with no approval, and a repo `command_allowlist` executes commands unprompted. Adds a scope-aware overlay, a forbidden-key list, restrict-only merges, and moves `Allow Always` to user scope keyed by repository. Security subset of #31, extracted so it need not wait for Phase 4. |
| 35 | [35_commit_preparation.md](35_commit_preparation.md) | **Not started.** Roadmap Phase 5 WP1. Damaian's first write to a user's Git repository. Builds the commit through a private index so the user's staging is never touched, runs hooks rather than bypassing them, and closes a trap: `GitService::diff` redacts, so a user could approve a diff showing `[REDACTED]` and commit the real secret. |
| 36 | [36_branch_and_worktree_delivery.md](36_branch_and_worktree_delivery.md) | **Not started.** Roadmap Phase 5 WP2. Branch suggestion, creation, ahead/behind, merge-base, and conflict prediction via `merge-tree --write-tree` so preparation provably cannot execute. Written to work with no worktree support, since its stated dependency (Phase 2 WP4) is Should-tier and unspecified. |
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
| 49 | [49_prompt_cache_accounting_and_reuse/](49_prompt_cache_accounting_and_reuse/proposal.md) | **In progress.** Split into a folder and planned on 2026-09-18. Roadmap Phase 1 WP8. The **accounting slice** (requirements 1, 2, 3, 6, 8, plus requirement 5's guard) is **Done as of 2026-09-23**: `cached_input_tokens` and a cached rate, so cost is no longer overstated for providers that cache automatically. The **reuse slice** (requirement 4) remains blocked, because the conversation section is a **sliding window of the last eight messages** that changes the prefix every turn; it waits on #55 or #26. |
| 50 | [50_model_initiated_clarification/](50_model_initiated_clarification/proposal.md) | **In progress: planned 2026-10-07, Tasks 1–3 of 8 done, the last on 2026-10-14.** Roadmap Phase 2 WP8. An `ask_user` tool whose load-bearing rule is that an answer can never satisfy an approval, enforced by types rather than discipline. It reuses the existing waiting state and reattach machinery, is bounded to two questions per turn, and is absent from the tool list where no user can answer. |
| 51 | [51_external_reference_retrieval.md](51_external_reference_retrieval.md) | **Not started.** Roadmap Phase 3 WP8. Damaian cannot read a page, so every task against a third-party API runs on what the model remembers about an unnamed version of it. `fetch_url` and an optional `search_web`, with fetched content entering as a lowest-priority provenance-labelled `ContextItem` proven non-authoritative by injection evals rather than by argument — #30's treatment of memory, reused. Its §5.3 is the reason it is Should-tier: a URL is the first place the model chooses what leaves the machine, so host approval lives in user scope on the forbidden-for-repository list, a URL carrying repository content or a secret is refused *before* the approval dialog rather than by it, and cross-host redirects are not followed. |
| 52 | [52_mcp_server_mode.md](52_mcp_server_mode.md) | **Not started.** Roadmap Phase 4 WP6. #06 and #33 are client-side only, so Damaian's indexed search, redaction and path policy are reachable only from its own window — the alternative being to give another tool raw filesystem access. Exposes a read-only subset over stdio, with the set computed as the intersection of read-only and profile-permitted rather than listed, so a tool added later is excluded until classified. The repository is fixed at launch and unreachable from the wire; a server has no user, therefore no approval, therefore nothing that writes. |
| 53 | [53_session_search_and_export/proposal.md](53_session_search_and_export/proposal.md) | **Done.** Roadmap Phase 1 WP9. A rescope, not greenfield: listing, renaming and deleting already work end to end, and what is missing is finding anything and getting anything out. A bounded scan through #17's parse-first reader rather than a second index, anchored on `seq` because it survives rewinds. Its sharp edge is inherited from #16: `append_message` stores content verbatim and deliberately does not redact, so that a rewind restores what was actually said — which makes export a display path that writes a file, and redaction on the way out mandatory, with the redaction count stated rather than silently applied. |
| 54 | [54_image_input.md](54_image_input.md) | **Not started.** Roadmap Phase 3 WP9. Lets a user paste a picture into the composer and lets the model see #12's diagnostic screenshots, which today it receives only as a file reference. Its hard part is that **an image defeats `SecretScanner`** and OCR is rejected, so the guarantee becomes informed consent: no unseen image is ever sent, and no setting can make screenshot inclusion automatic. |
| 55 | [55_conversation_compaction.md](55_conversation_compaction.md) | **Not started.** Roadmap Phase 3 WP6 (Must, and in that phase's minimum slice). Conversations are silently **clipped to the last eight messages, each cut to 2,000 characters**, so a constraint stated in the first message is gone at the ninth; compaction appends a `conversation_compacted` event rather than rewriting anything. Its load-bearing rule is that **a model never writes a field the engine can compute**: files changed, plan steps, approvals and validation status come from the session log. |
| 56 | [56_provider_fallback_consent/](56_provider_fallback_consent/proposal.md) | **Done** 2026-10-12. A terminal refusal with a qualifying second provider pauses on a one-shot, refused-by-default offer in the desktop app ("Keep it failed" or "Answer with …", no "always"); approval re-issues the refused call through the fallback and records the switch, and decline fails exactly as #48 does. Split out of #48, it depends on #48's classification and `failureKind` and reuses the pending-approval and resume machinery. |
| 57 | [57_worker_sessions/proposal.md](57_worker_sessions/proposal.md) | **Done 2026-10-16** (Tasks 1–12, started 2026-10-07; acceptance passed with DeepSeek on its re-run, see its §6). Hosts the planner-and-workers workflow behind one user setting, off by default and forbidden to repository config: a *Start worker* button on `worker-prompt` blocks runs the handoff in its own git worktree and process, with a per-project Workers board and pause, resume, stop and steering notes. Its load-bearing rule is that **nothing survives**: a stdin lifeline makes every worker exit when the shell dies, including on `SIGKILL`. |
| 58 | [58_header_repo_bar_and_menus/](58_header_repo_bar_and_menus/proposal.md) | **Done (2026-10-14).** Every acceptance criterion walked in the running app; the full seven-check gate passed in CI. Usability rework of the desktop shell: every "more actions" control becomes one vertical ⋮ menu, the header keeps only title, terminal and ⋮, and a repo bar above the prompt shows folder, branch, `+N −M` and contextual actions such as *Fix N errors*. `githubUrl` is reconstructed from a parsed `origin`, never opened raw, because the remote is repository-controlled. |
| 59 | [59_exploration_round_allowance/](59_exploration_round_allowance/proposal.md) | **Done 2026-10-23** (started 2026-10-18; Tasks 1–6, the last on 2026-10-23; the live re-run completed in 13 model calls with ten read-only rounds and two action rounds, no pause and no approval stop). Specified 2026-10-17 from a reporting-only session that hit *Tool budget exhausted* after eight rounds, five of them read-only. Exception to the graduation rule, like #47, whose read tools made the agent read in more, smaller rounds. A round in which every call is `ActionEffect::ReadOnly` is charged to a separate, clamped allowance (`agent_max_exploration_rounds`, default 24, ceiling 32) instead of `agent_max_tool_rounds`; a round with any command, edit or question, or a read that repeats an earlier one, is charged as today. Reads are not exempted outright because the round cap is the only runaway bound when no token ceiling is set (#21 §5.4). The key is restrict-only, `0` restores today's behaviour, and the pause and stop messages name both counts without assuming the turn was debugging. |
| 60 | [60_worktree_aware_read_tools/](60_worktree_aware_read_tools/proposal.md) | **Done 2026-10-23** (started 2026-10-17; the live re-run of the motivating prompt made no approval stop, see its §7). Adds `list_worktrees`, `read_git_log` and a `worktree` argument on the five existing read tools, because 35 of 40 approval prompts in one session were read-only shell commands in `.claude/worktrees/`. Only worktrees git itself has registered for this repository are reachable, so a symlink a clone ships there is refused. No command-policy change. |
| 61 | [61_continue_the_stopped_turn/proposal.md](61_continue_the_stopped_turn/proposal.md) | **Done 2026-10-23** (started 2026-10-18; Tasks 1–7, the last on 2026-10-23; §6's criteria walked against the tree on 2026-10-23). Specified 2026-10-17: *Continue debugging* starts a new turn that sees only the last eight messages, so a reporting request was lost after one stop and three continuations drifted into debugging a worktree. At the round limit the turn now pauses as a `round_budget` approval, persisted like any `PendingChatTurn`, and *Continue* resumes the same task with every message and plan step, or *Answer now* makes the forced final call. Pausing before the forced call saves the call that was being thrown away. Hard-bounded by `ABSOLUTE_TURN_ROUND_CAP`; completes #47 requirement 6 for users without a token ceiling. The `continue_keeps_state` scenario proves the resumed request still carries the original prompt and the earlier tool results. |
| 62 | [62_approval_prompts_name_the_reason/](62_approval_prompts_name_the_reason/proposal.md) | **Done 2026-10-23** (started 2026-10-18; Tasks 1–7, the last on 2026-10-23; the live re-run named every prompt's cause, see its §6): `command_policy.rs` gains the closed `ApprovalCause` enum and `CommandClassification.causes`, and `reasons` is rendered from it, so every classifier branch and escalation names its cause as a value — including the `Network`/`HighRisk` split and `Blocklisted`/`Destructive`; the `Setting`/`ProfileBlocked` causes name the `require_approval_for_*` key (with its user/repository/profile source, threaded from the `RepositoryConfigReport`) and the `command_access` block respectively. The approval prompt now leads with the deciding cause's plain words, and "sandbox" has left the model-facing command text and the audit status; the pinned Code prompt and its spec-50 fixtures moved with it. The approval card now shows the first cause and the model's stated reason outside the "Why this command" disclosure, in the live, worker and reattached paths. The proposal stores its `causes` (as values, read back tolerantly by older-file rules) and `command_proposal_stored` audits their names in order, so a session can be analysed by cause without changing any decision. The deterministic eval harness now reports each run's approval prompts grouped by their first cause (`approval_prompts_by_cause`, plus a per-run line), and the new `approval_causes` scenario produces two prompts with distinct causes so the grouping is measured. `USER_GUIDE.md`, `TROUBLESHOOTING.md` and `SECURITY.md` now document the card's two lines, how to read a cause, and what runs without asking without claiming a sandbox. Specified 2026-10-17: every approval said "because it cannot run in sandbox mode", and there is no sandbox. The classifier returns structured `ApprovalCause`s, the prompt's first line names the deciding one (which shell control, which keyword, which setting and who set it), the model's own reason is shown beside it, and "sandbox" leaves every user- and model-facing string. Changes no approval decision. |
| 63 | [63_exploratory_command_failures/](63_exploratory_command_failures/proposal.md) | **Done 2026-10-23.** Tasks 1–5 built 2026-10-17/18; Task 6 walked the §6 criteria and documented the purpose split. Commands get a purpose (check, exploration, other) that never influences approval; a failed exploration command records no finding and blocks no plan step, while checks and unknown commands behave as today. Prompted by a zsh glob error and an `ls` of a missing directory becoming open **error** findings; the model is now told its shell and zsh's glob rule. |
| 64 | [64_read_only_command_boundary.md](64_read_only_command_boundary.md) | **Done 2026-10-23.** Found the read-only list is not read-only (`git diff --output=<file>` writes) and replaces the prefix match with a tokenizer, per-program option lists and `PathPolicy` checks on every path argument. It then admits `cd <dir> &&` and `&&`/`|` pipelines of admitted commands into stdin-only filters, and the §5.7 documentation (user guide, security, troubleshooting) was written to close it. |
| 65 | [65_exploration_memory_within_a_turn.md](65_exploration_memory_within_a_turn.md) | **Not started — the next priority now that #59 is done.** Specified 2026-10-18, revised 2026-10-19. `bounded_messages` sends only the last ~7 rounds, so in a long turn most of what the agent read is gone before it answers. Adds a `turn_notes` tool whose bounded text (default 8,000 characters, restrict-only) is pinned after the seed, never elided, and carried into the next turn of the session. Requirement 8, the budget interaction, is settled by #59's exploration allowance (default 24, ceiling 32, restrict-only). |

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
  spec states what to do until it lands — as [#55](55_conversation_compaction.md)
  §5.1 does for the context budget, [#49](49_prompt_cache_accounting_and_reuse/proposal.md)
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
planned since then are folders (12, 17–25, 31, 32, 41–44, 46–50, 53, 56–63),
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

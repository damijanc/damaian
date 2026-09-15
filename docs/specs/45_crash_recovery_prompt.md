# Feature Spec: Crash Recovery Prompt

Status: Done
Order: 45 of 46
Plan: `docs/PLAN/01_phase_1_trust_and_recovery.md`, Phase 1, Work
Package 2 (Must) — the user-facing half. That directory is local-only and not
committed, so the reference is a name rather than a link; this spec is
self-contained.
Depends on: [#17](17_durable_task_state_and_crash_recovery/proposal.md) (the
classification it renders) — built; [#16](16_session_checkpoints_and_rewind.md)
(the checkpoint Inspect links to) — built. Everything else named below is a
cross-reference, not a prerequisite.
Related implementation specs:
[`17_durable_task_state_and_crash_recovery/`](17_durable_task_state_and_crash_recovery/proposal.md)
(supplies the classification and the three engine operations this presents),
[`16_session_checkpoints_and_rewind.md`](16_session_checkpoints_and_rewind.md)
(the checkpoint `Inspect` links to),
[`08_stop_and_progress.md`](08_stop_and_progress.md) (owns the turn-progress UI
this sits beside).

## 1. Motivation

Split out of spec 17 before implementation. Spec 17 establishes *what happened*
after a crash and what may safely be done about it; this spec is how the user
finds out and chooses.

The separation is deliberate rather than administrative. Spec 17's central
guarantee — that no action whose outcome is unknown is ever automatically
repeated — is enforced in the engine, and a `resume` call against an unsafe task
is refused there. This spec cannot widen that. It renders a decision that has
already been constrained, which is why it can be built and reviewed on its own
without weakening anything.

## 2. Current State

Nothing exists. Spec 17 must land first: it provides the classification
(`interrupted` or `unknown_external_outcome`), the dangling action and its
`seq`, whether auto-resume is permitted, and the `resume` / `mark failed` /
`abandon` operations.

`docs/specs/41_ui_density_and_action_hierarchy/` and
[`../UI_STYLE_GUIDE.md`](../UI_STYLE_GUIDE.md) define the button scale and the
action-hierarchy rules this prompt must follow — in particular that an
escalating or destructive action never sits at the same visual weight as a
one-shot one.

## 3. Requirements

1. On launch, a session with recovered tasks presents a prompt naming the
   **specific** in-flight action — "a patch application was in progress and its
   outcome is unknown" — never a generic "session interrupted".
2. The four choices are offered: Resume, Inspect, Mark failed, Abandon. Resume
   is offered **only** when spec 17's classification says it is safe, and its
   absence is explained rather than silent.
3. `Inspect` shows the task, its dangling action, the files it may have touched,
   and links to its checkpoint from spec 16.
4. A task auto-resumed by spec 17 is reported after the fact, not silently
   resumed — the user learns that something was picked up.
5. A pending approval reattached by spec 17 re-presents its approval card, and
   re-presenting it does not re-approve anything.
6. The prompt follows `../UI_STYLE_GUIDE.md`: `Abandon` and `Mark failed` are
   terminal and must not carry the same weight as `Resume`.

## 4. Non-goals

- Deciding what is safe to resume. That is spec 17, and this spec must not
  re-derive or widen it.
- Rewinding files. `Inspect` links to spec 16's checkpoint; performing a restore
  is spec 16's surface.
- A live progress display of the twelve states — [spec 08](08_stop_and_progress.md).

## 5. Design

Written against spec 17's shipped API rather than an imagined one:
`recovery::{classify_all, classify_session, resume, mark_failed, abandon,
resume_allowed, reattach_pending_approvals}` and `RecoveredTask`.

### 5.1 Where the prompt lives

One card per recovered task, at the top of the conversation of the session the
task belongs to, rendered when that session loads.

One consequence, stated rather than discovered: a recovered task is classified
and audited at launch but **presented when its session is opened**. The shell
restores the last session per project, which after a crash is the session that
crashed, so the common case presents immediately. A task in a session the user
never reopens stays classified and unpresented — which is the right trade for a
card in the conversation, and the reason the sweep audits rather than relying on
the card having been seen.

Not a launch-time modal. [`../UI_STYLE_GUIDE.md`](../UI_STYLE_GUIDE.md) §1
requires per-turn information to sit in the turn rather than in chrome, and a
recovered task *is* a turn — it has a prompt, a dangling action and a
checkpoint. A modal would also block the app on a decision the user may not be
ready to make, and `Inspect` inside a modal needs a second layer to show
anything.

### 5.2 The sentence lives in the engine, not the webview

`recovery::headline` maps a `RecoveredTask` to the sentence naming its specific
in-flight action — "A patch application was in progress and its outcome is
unknown", "Reading a file was interrupted" — from the dangling action's name,
with a fallback that prints the raw action name rather than degrading to the
generic "session interrupted" requirement 1 forbids.

It belongs in `recovery.rs` for the same reason `chat::tool_action_label` is not
in the UI: the frontend should never map action names to prose. It is also what
makes requirement 1 testable. The shell has no JS test suite
([`../UI_STYLE_GUIDE.md`](../UI_STYLE_GUIDE.md) §9), so a sentence assembled in
`app.js` could not be asserted against a session-log fixture at all.

`recovery::resume_blocked_reason` returns why `Resume` is absent, for the same
reason: requirement 2 asks for the absence to be *explained*, and an explanation
composed in a webview is a webview's guess at what the engine decided.

### 5.3 Two shell endpoints

**`GET /api/recovery`** runs `classify_all` and, per session,
`reattach_pending_approvals` — **once per shell process**, memoized. Once,
because classification appends a `task_recovered` audit event per task, and
re-sweeping on every session switch would fill the audit log with re-derivations
of a single fact. Once is also correct rather than merely cheap: a crash ends the
process, so no new crash can appear while one is running.

It returns per recovered task: session and task id, `headline`, classification,
the dangling action and its `seq`, `resumeAllowed`, `resumeBlockedReason`, the
task's stored prompt, the files the dangling action may have touched, and the
task's checkpoint id from [spec 16](16_session_checkpoints_and_rewind.md).
Reattached approvals ride along as ready card payloads, or as the reason one was
unavailable.

**`POST /api/recovery-decision`** takes `session_id`, `task_id` and `decision`,
**re-classifies server-side, and passes the engine's own `RecoveredTask`** to
`recovery::resume`, `mark_failed` or `abandon`.

That is load-bearing. The client's fields are an address, never evidence. Were
the handler to rebuild a `RecoveredTask` from posted JSON, a webview could post
`classification: interrupted` for a task whose outcome is unknown and walk
through requirement 5's enforcement point untouched — which is exactly the
arrangement spec 17 §5.4 refused when it put the decision in the engine.

### 5.4 Resume re-runs the turn

`recovery::resume` authorizes and records; it does not re-run anything, because
spec 17 is engine-only and running a turn is the orchestrator's job. So the
decision endpoint, after the engine authorizes, re-submits the task's stored
`user_prompt` through the existing ask-stream path, and the turn genuinely
continues in the conversation.

This needs the prompt back off disk. `read_task_statuses` returns statuses only,
so `SessionStore::read_tasks` is added alongside it, replaying full `Task`
records from `task_created` and `task_status_updated`. `read_task_statuses`
stays as the cheap path for the conversation view, which wants nothing else.

**The resumed task is then recorded as superseded**, by
`SessionStore::mark_task_superseded`, and the sweep skips a superseded task
everywhere it would otherwise consider one. Because the work runs as a *new*
turn with a task of its own, the original never reaches a terminal status —
so without this every later launch classifies it as interrupted again and
offers the identical card, forever. (See §7: this was found by audit, not by
the implementation's own tests, because the in-process snapshot hid it.)

A status is deliberately not written instead. Every terminal one available
would be a lie: `complete` never happened, `failed` did not fail, and
`cancelled` says the turn was not retried when retrying is exactly what
happened. What is true is that the work moved to another turn, so that is the
fact recorded, and the *presentation* filters on it — the same split this spec
already uses, where the engine classifies and the shell decides what to ask
about. Supersession is an active event, so a rewind past the resume brings the
question back, which is correct: the turn that superseded it is gone too.

### 5.5 Auto-resume is authorized automatically and run on request

A deliberate narrowing of requirement 4, stated rather than buried.

The launch sweep calls `recovery::resume` for every task whose classification
says `auto_resume_permitted`, so the status change is automatic and audited as
spec 17 requirement 6 asks. The turn itself does not re-run until the user
presses `Continue` on a card that reports what was picked up and why it was
safe.

The reason is cost and surprise: `waiting_for_model` and `preparing_context` are
auto-resumable, so a literal auto-resume would fire a billed model call the
moment the user opened the app, before they had read anything. Requirement 4's
actual guarantee — that the user learns something was picked up rather than
finding out later — is preserved, and the reporting is no longer after the fact
about work that already ran.

### 5.6 Action hierarchy

Two shapes, following [`../UI_STYLE_GUIDE.md`](../UI_STYLE_GUIDE.md) §4:

| | Disclosure | Visible actions | Overflow `⋯` |
|---|---|---|---|
| Resume allowed | `Inspect` | `Resume` (`.btn-primary`) | `Mark failed`, `Abandon` |
| Resume absent | `Inspect` | `Mark failed` (`.btn-quiet`) | `Abandon` |

`Mark failed` and `Abandon` are terminal and outlive the interaction, which is
the overflow rule, and both keep a confirm dialog — requirement 6.

When `Resume` is absent there is **no primary action at all**. The card states
the blocked reason and leaves `Mark failed` quiet. Filling the only remaining
button would put the loudest control on the surface behind a choice the user is
being asked to make precisely because Damaian cannot tell what happened.

`Inspect` is the §7 disclosure paired with the action row, not a fourth button:
it shows the prompt, the dangling action and its `seq`, the files that action may
have touched, and a link into the existing rewind surface for the task's
checkpoint. Restoring stays spec 16's, per §4.

### 5.7 Re-presented approvals approve nothing

A reattached `CommandProposal`, `ProposedPatch` or plan review re-renders through
the existing `createCommandApprovalPreview`, `createPatchPreview` and
`createPlanReview`, so the card is presented and nothing runs until the user
acts.

**Plan review is the third kind**, added by
[spec 21](21_task_plan_progress_and_budget/proposal.md) after this spec landed.
Until it was handled here, `reattach_pending_approvals` fell through to its
catch-all and failed the task with "unknown pending approval kind plan" — the
card then told the user a turn could not be restored while its paused turn sat
on disk the whole time. A reattached review is assembled from two places,
because that is where spec 21 put them: the plan is replayed from the session
log, and the deferred action comes out of the paused turn via `PausedTurns`, a
read-only view of `chat/pending/`. Read-only on purpose — resuming a paused turn
stays `ChatOrchestrator`'s — and keyed on the data directory rather than on an
orchestrator, which takes thirteen dependencies to build.

A plan review is also the one reattached approval that genuinely *continues*:
its paused turn survived, so it resumes through the ordinary
`/api/resume-plan-stream` and picks the work back up. It therefore needs no
`detached` mode and no closing-out — the turn it resumes reaches a terminal
status on its own. A review whose paused turn is gone is refused rather than
shown, since approving it would have nothing to continue; that fails the task
with the reason stated, exactly as an unreadable command proposal does.

A command approved this way runs standalone: the chat turn that proposed it died
with the process, so there is no model round to feed the result back into. The
card says so rather than implying the conversation will continue.

That is also why the card takes a `detached` flag rather than reusing the live
path unchanged. Its normal decision goes to `/api/resume-command-stream`, which
requires the orchestrator to still hold pending state for the proposal — after a
crash it holds none, and refuses. `detached` sends the decision to
`/api/run-command` and `/api/reject-command`, which do fall back to the stored
proposal, and renders the command's own output in the card instead of into an
answer from a model that was never asked anything.

**An answered approval is closed out**, via
`POST /api/recovery-approval-resolved`, which moves the task to `cancelled` and
records it. This is the one place where this surface could otherwise cause a
side effect *twice*: the task's status is what `reattach_pending_approvals`
keys on, so leaving it `waiting_for_approval` would have the next launch offer
to run a command that had already run. `cancelled` rather than `complete`,
because the command ran but the conversation that asked for it produced no
answer. The patch card closes its task out the same way, once no file is left
pending — a patch is answered per file, so one apply is not the end of the
question.

That endpoint writes a status and audits it; it authorizes nothing, which is why
it does not go through the engine's recovery operations. None of them fit a task
the classifier deliberately skips.

### 5.8 The crashed turn's plan carries onto the resumed one

Spec 21 gave a turn a plan, and `carry_plan_from_a_token_stop` carried it onto
the next task when the previous one stopped at the token ceiling. A crash resume
is the same situation and was not covered: the resumed work is a new task, so
the plan stayed behind and the new turn re-planned from nothing, free to redo
steps whose work had already landed.

It now carries in both cases — the function is
`carry_plan_from_the_previous_turn` — keyed on the same supersession record §5.4
writes. **The crash case is safe for exactly the reason the resume was offered
at all:** a carried plan brings its completed steps and their evidence, which
would be wrong if a step could have half-happened, and it cannot, because a
resume is refused outright for `unknown_external_outcome`. Every crash that
reaches here had nothing side-effecting in flight.

The carry stays narrow either way: only with steps outstanding, and only from a
turn that actually handed its work over, so an unrelated next question never
inherits somebody else's steps.

### 5.9 What the crash cost

[Spec 19](19_token_and_cost_accounting/proposal.md) §5.5 accounts for a model
call lost to a crash — and does it inside *this* spec's sweep, since the
classifier is the only thing that knows a call was lost. The figure went to the
session log and nowhere else, so the one screen that exists to explain a crash
said nothing about what it cost.

The card now carries the task's spend, in the same `task_usage_json` shape a
turn's own response sends, so the frontend reads both with one piece of code.
Two rules come with it, both from spec 19:

- **Absent, not zero.** A task with no usage recorded carries no figures at all.
  A session written before spec 19 has none, and "0 tokens" would read as a
  crash that was free.
- **The estimate is marked.** A lost call's figure is an estimate taken before
  the request went out, so the whole total renders as `~N tokens (estimated)`.

Whether the total includes the lost call is read back from the recorded usage
(`SessionStore::lost_to_crash_task_ids`) rather than inferred from the dangling
marker being a `model_call` — a marker written before spec 19 carries no
estimate, so nothing was added for it, and claiming otherwise would be a
sentence the log does not support.

### 5.10 First launch fails 24 stale approval tasks

This is the first caller of `reattach_pending_approvals`, and spec 17's
implementation notes measured the consequence against real data: 24 tasks left
`waiting_for_approval` by versions that recorded the status but not which
proposal it was waiting on are marked `failed` with a stated reason on first
launch. §5.5 of spec 17 forbids rebuilding an approval card from partial data,
and failing the task destroys nothing — the stored patches stay on disk and stay
applicable. Recorded here because it happens the first time this spec's sweep
runs, and would otherwise be found as a bug.

## 6. Acceptance Criteria

- The prompt names the specific in-flight action, asserted against a session log
  fixture for each of the two recovered classifications.
- `Resume` is absent, with a stated reason, for a task classified
  `unknown_external_outcome`.
- A re-presented approval card approves nothing until the user acts on it.
- `docs/USER_GUIDE.md` explains what happens after a crash, what the choices
  mean, and why Damaian will not retry an action on its own.
- Every quality-gate command from `AGENTS.md` passes.

## 7. Implementation Notes

### The prompt-erasing resume: a latent bug in spec 17's code

Resuming re-sends the prompt the user typed, so §5.4 added
`SessionStore::read_tasks`. Written the obvious way — a later `task_status_updated`
event replaces the earlier record, exactly as `read_task_statuses` overwrites a
status — it returned an empty prompt for every task the sweep had just
authorized.

The cause is that spec 17's `set_status` and `fail_task` construct a `Task` with
only `id` and `session_id` filled in, which is legitimate: tasks are replayed
from events rather than stored as records, and a writer that only wants to move
the status has nothing else to say. Nothing had noticed, because
`read_task_statuses` reads statuses. The first reader of the whole record found
the prompt erased **by the very `resume` meant to bring the task back** — the
failure was in the one path this spec exists for.

`read_tasks` therefore merges: status and completion time from the latest event,
identity and metadata from whichever event recorded them. Blank means "not
recorded on this event" rather than "cleared", which is true of every writer in
the codebase and is asserted by
`a_status_event_without_metadata_does_not_erase_the_prompt`.

Worth noting how it was caught: by an assertion about the *prompt* in a test
about auto-resume reporting, not by a test aimed at `read_tasks`. Its own two
tests passed, because they never wrote a skeleton event.

### Where the guarantee actually sits

Three places, each deliberate:

- The sentence and the reason a resume is absent are in `recovery.rs`, not
  `app.js` (§5.2). The shell has no JS test suite, so this is also the only
  arrangement in which requirement 1 is testable at all.
- `POST /api/recovery-decision` re-classifies before acting (§5.3). The posted
  ids address a task; they never describe it.
- Auto-resume authorizes but does not execute (§5.5). The engine's own status
  change happens automatically; spending money does not.

The second is the one to keep. It is the difference between a guarantee and a
convention: `recovery::resume` refuses an unknown outcome, but only if what
reaches it is what the engine concluded.

### Three bugs, none of which a specimen page could have shown

The first two came from driving the running shell; neither would have appeared
in a static specimen or in the Rust tests. The third came from fixing the
second.

**The sweep raced the bootstrap.** Every `/api/` route is token-gated, and the
token arrives with the Tauri bootstrap. Firing the sweep at load time meant a
401 — and because the request is memoized so it runs once, that single failure
hid every recovered task for the rest of the run. It now awaits
`ensureDesktopApiReady()` and clears the memo on failure, so the next session
opened tries again.

**A reattached approval could not be approved.** The design assumed the existing
approval card's endpoints already fell back to the stored proposal. `/api/run-command`
does; `/api/resume-command-stream`, which is what the card actually posts to,
does not — it requires pending orchestrator state that a crash destroyed, and
answered `No pending chat command for proposal: …`. The card grew a `detached`
mode (§5.7). The lesson is narrow and worth keeping: "reuses the existing
component" is not the same claim as "reuses the existing component's endpoint".

Fixing that surfaced the third, which is the worst of them and was reasoned out
rather than observed: once the approval *could* be answered, nothing closed out
the task it belonged to, so the next launch would reattach the same proposal and
offer to run a command that had already run. A feature whose entire purpose is
that no side effect is repeated would have repeated one. §5.7 says what closes
it, and `an_answered_approval_is_not_reattached_again` holds it.

### What an audit found after specs 19 and 21 landed

Both of these were introduced by *other* specs changing the ground under this
one, and neither broke a test here — the suite stayed green throughout, which is
the point worth keeping.

**A plan review was destroyed rather than re-presented.** Spec 21 added a third
pending-approval kind; `reattach_pending_approvals` knew two, and its catch-all
marked the task `failed` with "unknown pending approval kind plan". The user was
told a turn could not be restored while its paused turn was on disk and
`/api/resume-plan-stream` stood ready to continue it. §5.7 now handles it. The
general lesson is about the shape of that catch-all: it treats an unknown kind
as unrecoverable, which is right for data it cannot read but wrong for a kind
that simply postdates it, and nothing fails loudly when a new kind appears.

**A resumed turn's plan was dropped.** Spec 21 carries a plan across a token
stop; a crash resume left it behind, so the new turn re-planned from scratch.
§5.8 covers both, and the safety argument is written down there rather than
left implicit.

**Two of spec 21's action markers had no phrasing.** `propose_plan` and
`complete_step` fell through `action_subject` to the raw name — "propose_plan
was interrupted before it finished". The fallback did its job, but it exists so
an action this version has *never heard of* still says something specific, not
as a resting place for actions shipped alongside it.

**The crash's cost was recorded and invisible.** §5.9.

**A resumed task was offered again on every later launch.** `recovery::resume`
leaves the task non-terminal by design, the replacement work runs as a new task,
and nothing ever closed the original — so the sweep re-classified it as
interrupted at each start. Within the run that made the decision, the in-process
`forget()` hid it completely, which is exactly why implementation-time testing
missed it: the bug was only visible across a process boundary. §5.4 records
supersession instead, and the test that holds it takes a *fresh* sweep rather
than re-rendering the snapshot.

### One thing kept out: a quadratic launch

The sweep asks four questions of a session log per recovered task — statuses,
tasks, usage, and the two id sets §5.4 and §5.9 added. Each is a full replay, so
asking them per task makes launch cost quadratic in tasks-per-session. That is
the same shape spec 17 measured out of `append_session_event` (17.5s to append
2000 events, 135× better once it stopped rescanning), which is reason enough not
to put it back one caller later. `SessionFacts` reads each log once per session
and the per-task work is map lookups.

### Measured

In the running shell at 1280×800, conversation column 980px — measured, not
estimated:

| Card | Collapsed height |
|---|---|
| Recovery, resume offered | 149px |
| Recovery, resume absent | 132px |
| Recovery, resume offered, with a spend line (§5.9) | 192px |
| Reattached command approval | 216px |
| Reattached plan review, three steps (§5.7) | 313px |

The resume-offered card is the taller of the two because its note explains what
continuing will do; the card without a resume says less because there is less to
offer. Both are cards in the conversation, so they cost nothing on any turn that
did not crash.

### What was not built

- No live progress display of the twelve states — [spec 08](08_stop_and_progress.md)
  owns that, and §4 rules it out here.
- No restore from `Inspect`. It opens spec 16's existing rewind dialog for the
  task's checkpoint; nothing about restoring was reimplemented.
- No JS tests, because there is no suite to add them to. Everything asserted
  about this surface is asserted on the payload the webview receives, which is
  why the payload carries prose rather than codes.

The surface was driven by hand instead, through
`serves_the_ui_for_manual_inspection`, which now seeds both card shapes, a
reattached command approval and a reattached plan review, so the next person
does not have to stage a crash to see them. Both specimen shapes were also added to
[`../ui-style-guide.html`](../ui-style-guide.html).

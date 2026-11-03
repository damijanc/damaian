# Feature Spec: A Worker Can Ask Instead of Guessing

Status: Done 2026-11-03 (Tasks 1–6; scoped local checks are recorded in
[`tasks.md`](tasks.md), and the full seven-check gate is left to CI).
Order: 104 of 106
Plan: none. This is a single-worker capability gap found during the 2026-10-31
investigation.
Depends on: [#50](../50_model_initiated_clarification/proposal.md) (durable
questions), [#57](../57_worker_sessions/proposal.md) (worker process and control
socket), and [#99](../99_worker_task_outcomes.md) (worker outcome UI), all built.
Everything else named below is a cross-reference, not a prerequisite.

## 1. Motivation

`ask_user` is offered only when `ChatTurnOptions.user_present` is true. Worker
turns set it false and instruct the model to proceed under an assumption. That
is useful for harmless ambiguity, but unsafe when the answer changes scope,
architecture, destructive behaviour or acceptance criteria.

A worker already pauses for patch and command decisions and can be opened in
the app. It needs the same durable route for a real clarification instead of
guessing or ending with an unexplained partial result.

## 2. Requirements

1. Worker turns may receive the existing `ask_user` tool even though no user is
   synchronously present. Availability is represented separately from
   `user_present`.
2. Asking records the existing `question_asked` event and `pendingQuestion`
   link, stops the worker run, and writes progress as **Needs your answer**.
3. The worker session and Workers board render the existing question card and
   route its answer through the worker control socket. The worker resumes in
   its own process with the same pending turn and selected provider.
4. A worker question is never an approval. Its answer cannot approve a patch,
   command, check, browser diagnostic, MCP call or budget extension.
5. The existing per-turn question limit applies. Repeating an answered
   question is refused as in an ordinary session.
6. A truly headless caller with no answer route leaves the task waiting. It
   does not convert the question into an assumption or a failed approval.
7. The system prompt continues to prefer a stated assumption for low-impact
   ambiguity, but instructs the model to ask when choosing would materially
   alter scope, public behaviour, destructive actions or acceptance criteria.
8. Reload, worker restart and parent deletion preserve the same recovery and
   ownership rules as other worker decisions.

## 3. Non-goals

- Making workers conversationally synchronous. A question pauses execution.
- Allowing the parent model to answer for the user.
- Treating every ambiguity as a question. The configured question cap and the
  prompt's materiality rule remain.
- Changing ordinary-session question behaviour from #50.

## 4. Design

Replace the single `user_present` offering test with two capabilities:
`question_route_available` and `synchronous_user_present`. Ordinary sessions
set both; app-managed workers set only the route; headless CLI runs may set
neither. The existing question store and pending-turn snapshot remain
authoritative.

Add `question` and `answer_question` worker-control messages parallel to the
existing approval routes, but keep their types distinct through dispatch. The
progress projection carries only question id and redacted display text already
permitted by #50.

## 5. Acceptance

- A worker can ask, become **Needs your answer**, receive an answer after its
  process is idle, and continue the same plan.
- Reloading before the answer redraws exactly one question.
- Supplying an answer through an approval endpoint runs nothing and is refused.
- A worker without a question route is not offered `ask_user` and its prompt
  explains that material ambiguity must stop as incomplete rather than guess.
- The question limit, secret screening and recovery tests from #50 pass for a
  worker session as well as an ordinary one.

## 6. Implementation Notes

The execution order, corrections and scoped-check evidence are in
[`tasks.md`](tasks.md); the decisions planning had to settle are in
[`context.md`](context.md). Where the feature landed:

- **Two capabilities (Tasks 1–2).** `ChatTurnOptions` now carries
  `question_route_available` and `synchronous_user_present`; the old
  `user_present` JSON key is a serde alias for the second so pending turns keep
  loading. `ask_user` is offered when the route exists and the cap is above
  zero; `fetch_url` still requires the synchronous user. A worker-only prompt
  keeps the stated-assumption rule and asks on material ambiguity; a headless
  prompt now says material ambiguity must stop as incomplete rather than guess.
- **Worker host (Task 3).** A worker turn starts with `ChatTurnOptions::worker()`
  and settles `WaitingForApproval` on a question id. The host writes a
  `question` progress line carrying only the id and redacted display text, puts
  the id in its status line, and answers through the distinct
  `answer_question` control command. `approve`/`reject` refuse a question id and
  `answer_question` refuses every other pending kind; the status snapshot uses
  `questionId`, never `proposalId`.
- **Restart (Task 4).** `restore_pending_decision` reattaches a plan review as
  before and now also an unanswered question, writing the same progress and
  settling the same id without a model call. An already-answered question is
  not reattached: spec 50's rule is that an answered question is never asked
  again.
- **Outcome and UI (Task 5).** `WorkerTaskFacts` gained `pending_question`, and
  the engine renders **Needs your answer** with a new `NeedsAnswer` outcome,
  the *Needs you* group, no composer and the review action. The board's
  `doing` uses the same headline. The worker view draws the existing question
  card from the transcript and routes its answer through the control socket;
  *Review* now scrolls to that card too.
- **Tests and documentation (Task 6).** Engine tests cover the two-capability
  split, worker offering and answer routing, the prompt split, and a question
  never satisfying an approval. Desktop tests cover socket parsing, the two
  endpoints refusing each other, live settle/answer, restart reattachment, the
  progress projection and the control route's field names. `USER_GUIDE.md`,
  `TROUBLESHOOTING.md` and the spec README were updated.

No manual browser walk was performed. The card is the spec 50 component
unchanged except for which route its answer takes, and the reload/restart
behaviour is covered by the automated tests above; a visual walk remains the
one uncovered check.

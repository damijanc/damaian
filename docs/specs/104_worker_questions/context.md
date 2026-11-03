# Spec 104 planning context

Status: Done 2026-11-03. This file records the decisions and corrections that
planning found against the code, not the design again; read
[`proposal.md`](proposal.md) first.

## 1. The old flag is also a persisted format

`ChatTurnOptions` is saved with every paused turn. Replacing `user_present`
outright would make an older paused turn resume with both capabilities false:
an ordinary interactive turn would become headless mid-turn, losing its second
question. The field is therefore a serde alias on
`synchronous_user_present`, and `ChatTurnOptions::question_route()` treats a
synchronous user as having the route too. A new driver sets the two capabilities
explicitly; an old one still means what it meant.

## 2. `fetch_url` is not a worker capability

Spec 51's first-host approval is a dialog on the turn's own path, so it cannot
be delivered after the turn has settled. `fetch_url` therefore stays tied to
`synchronous_user_present`. A worker may ask a question but may only fetch a
host the user already allowed. This is deliberate, not an omission.

## 3. Engine-started turns need the worker's route too

Ordinary turns receive their options from a host. Repair, verification
continuation and explicit task continuation are started by the engine itself and
previously hard-coded a headless option set. They now ask
`engine_started_turn_options`, which gives a session with a worker record the
worker route; every other engine-started turn keeps spec 50's headless default.
Without this, a worker that ran into a repair would silently lose the capability
its ordinary turn had.

## 4. The progress file is a projection, not a second question store

The `question` progress line carries only `questionId` and the redacted question
text. The options and the answer live in the session log, which `/api/session`
already joins with `question_state`; the card is drawn from that transcript.
Duplicating the options into the progress file would create a second, stale
copy. The status line carries `questionId` only so the board and strip can tell
a question from an approval without reading the log.

## 5. The host, not the card, owns the pending kind

`DecisionKind::Question` is distinct from `Command`, `Plan`, `RoundBudget` and
`Checks`. The socket enforces the distinction before any job is handed over:

- `approve`/`reject` refuse a question id with “a question is answered with
  `answer_question`”;
- `answer_question` refuses an approval or check id with “that id is not a
  pending question”;
- the status snapshot reports `questionId`, never `proposalId`, because spec 50
  made a question id a different type from a proposal id.

The engine's `resume_after_answer` remains the only resume path, so type
separation and the never-re-ask rule are unchanged. An answer the engine refuses
before consuming the pause (an invalid option, free text where the question
allows only options, or a race) is re-read from durable state like spec 103's
refused plan budget: the host stays waiting on the same question and reports the
refusal, instead of going idle and making the visible card unanswerable.

## 6. Review the task model, not the browser

The board and strip already render engine-owned control. The engine gained one
input, `WorkerTaskFacts.pending_question`, and one outcome, `NeedsAnswer`. The
browser still keeps no status-to-outcome map. `needs_decision` stays true for a
question because the composer must be disabled, but the headline, dot, group
and action are engine data.

## 7. Restart reattaches only an open question

A restart reattaches an unanswered question exactly as spec 103 reattaches a
plan review: the paused turn and the `question_asked` event are on disk, so the
new process writes the card and waits without a model call. An answered question
is deliberately not reattached. Spec 50 already forbids asking it again, and a
disabled card would look answerable while accepting nothing; spec 57's ordinary
restart behaviour is the honest outcome.

## 8. Crash, deletion and parent ownership are unchanged

A live worker is not crash debris (spec 57 C12), so its question is reattached
by the process that owns it. A worker whose process ended while the question was
open is classified as interrupted like any other waiting worker and offers
*Restart worker*, which is what reattaches it. Deleting the parent still sends
`shutdown` through the existing worker rules; the question pause does not add a
second ownership path.

## 9. What was intentionally not changed

- The question store, bounds, cap, redaction and recovery types from spec 50.
- `fetch_url` availability.
- The approval, patch, check, fallback and round-budget routes.
- The session-log answer route for ordinary sessions. Only a worker's card is
  given the control-socket callback.

## 10. Checks run locally

`cargo fmt --all`, scoped `cargo clippy -p workspace-engine -p desktop-shell
--all-targets --locked -- -D warnings`, the focused `clarification`,
`task_control` (inline), worker-host, board-projection and control-route tests,
`node --check crates/desktop-shell/static/app.js`, `npm run lint:web`,
`typos`, `cargo deny check` and `npm run specs:check -- --strict`. The
workspace-wide clippy and test suite were left to CI under `AGENTS.md`.

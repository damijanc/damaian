# Feature Spec: A Responsive Shell While Damaian Works

Status: In progress (specified 2026-11-11; started 2026-11-12; Task 1 done,
Tasks 1b and 1c ready; Tasks 1, 1b, 1c, 2–4 in [tasks.md](tasks.md))
Order: 110 of 110
Plan: none. Written from the owner's spec 33 Task 1 run on 2026-11-11; the
code survey and decisions are in [context.md](context.md).
Depends on: nothing unbuilt; **ready to start**. It changes the desktop
shell's connection handling (`crates/desktop-shell/src/lib.rs`); spec
[#109](../109_fast_verification/proposal.md) Task 6 (Stop for every running
action) builds on it.

## 1. Motivation

While a turn ran, the owner archived an old session from the sidebar and
nothing happened. The archive was not lost: the click waited until the turn
ended and then ran. The desktop shell serves one HTTP request at a time, and a
turn's stream holds the accepting thread for the whole turn, so **every** UI
action (the sidebar, archive, settings, the Workers board, switching or
starting sessions) hangs while Damaian works. The owner: "If a turn locks
everything we have built a useless UI."

The next run (2026-11-12) showed the other half. Damaian was working from a
garbled instruction, and the owner had a correction to give it. The composer
let them send, and the shell refused it as a second turn, shown as raw JSON.
The turn had resumed after a question, so it had no Stop either. The only
choices were to let the turn finish on the wrong premise or to lose it.

## 2. Requirements

1. **The whole UI works while a turn runs** (decided by the owner,
   2026-11-11). Every request other than the session's own work is answered
   while any turn runs: reading and switching sessions, archive, rename,
   delete, settings, the Workers board, the repository bar.
2. **Each session works in its own process, and several can work at once**
   (decided by the owner, 2026-11-11: "We will allow damaian to run multiple
   sessions at the same time, especially if running on different projects";
   "several sessions - each its own process can work on one project too").
   A session's turns, commands, checks, baselines and patches run in a session
   host process of its own; the shell process serves the UI and supervises the
   hosts. One turn at a time per session; a second work request for a session
   that is already working is refused at once with a message, never queued. A
   message to a working session is not a second turn: it joins the running one
   (requirement 7).
   Sessions in other projects and other sessions in the same project run
   independently.
3. **The one exception: archive or delete of a working session** (decided by
   the owner). Archiving or deleting a session while it is working is refused
   with a message telling the user to stop it first; every other action on a
   working session (open it, read it, rename it) is allowed.
4. **Every route is classified** as session work or UI by an exhaustive match,
   so a new route must choose; there is no default.
5. **Running work can be reached** from a UI request: each session host reports
   its running work's label and accepts a cancel, so a Stop request is answered
   mid-turn (used by spec 109 Task 6) and the sidebar can show which sessions
   are working.
6. **Shared state stays correct across processes.** What a single thread
   protected for free — session-log appends and reads, pending-turn
   `load`/`take`, the audit log, the process registry, index and
   repository-map caches, config writes, and two sessions changing the same
   checkout — is made safe or shown to be safe, with a test for each.
7. **A message sent while a session works reaches the running turn** (decided
   by the owner, 2026-11-12). The composer stays usable during a turn. What
   the user sends is accepted at once, shown in the conversation as waiting,
   and given to the model before its next request, as the user's own message.
   It never stops the turn, never answers a question and never approves
   anything ("questions are not approvals"). A message that arrives too late
   for the turn is not lost: it is kept for the turn that resumes after an
   approval or a question, starts the next turn if the turn completed, or goes
   back into the composer if the user stopped the turn (context D7).

## 3. Non-goals

- Running two turns in one session at the same time.
- Changing how workers (spec 57) use worktrees: a session host works in the
  checkout its session belongs to.
- The Stop controls themselves (spec 109 Task 6).
- Coordinating two sessions that edit the same files (patch hash checks still
  refuse a stale patch; nothing new merges their work).

## 4. Acceptance criteria

- With a turn running (a mocked model that waits), `/api/sessions`,
  `/api/session`, archive of another session, rename and settings answer
  within a second, and the sidebar shows the change.
- Two sessions, in two projects and in one project, run turns at the same time,
  each in its own process, and both finish correctly; killing one host leaves
  the other and the UI running, and the killed session recovers as a crash
  (spec 17).
- A second work request for a working session is refused at once; archive or
  delete of a working session is refused with "stop it first".
- A message sent during a turn appears at once as waiting, reaches the model
  before its next request (the request carries it, the session log records it
  as a user message with a `steering_note_added` event), and the turn goes on;
  one sent during the final round starts the next turn; one queued before an
  approval reaches the resumed turn; one sent before a Stop returns to the
  composer. None of them resolves a pending approval or question.
- The route classification is exhaustive and tested; each shared-state item in
  requirement 6 has a concurrency test.
- `USER_GUIDE.md` / `TROUBLESHOOTING.md` describe working in several sessions
  at once and the archive/delete exception.

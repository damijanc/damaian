# Feature Spec: One Pending Decision, and What Typing Does While It Waits

Status: Not started (specified 2026-11-21; four tasks in [tasks.md](tasks.md))
Order: 113 of 113
Plan: none. Written from the owner's use of Damaian on a Snake game repository
on 2026-11-20. The evidence, the code survey and the decisions are in
[context.md](context.md).
Depends on: [#107](../107_visible_waiting_and_working/proposal.md) (pending
decisions and the composer lock for a question),
[#108](../108_interface_cleanup_and_session_archive/proposal.md) (D6: declining
a plan asks how to change it), [#110](../110_responsive_shell_during_work/proposal.md)
(D7: notes reach only a running turn) and
[#112](../112_status_in_one_place/proposal.md) (the composer status slot) —
all built where this spec needs them. **Ready to start.**

## 1. Motivation

A plan review's card vanished (fixed on 2026-11-20). With no buttons, the owner
answered the way the plan seemed to invite: he typed "proceed with the plan",
then "decision 1, go with option A…". Each typed message started a **new task**.
That task carried the unapproved plan forward, met the plan gate again and
paused on a **new** plan review. The older one stayed `WaitingForApproval`
with its file on disk. One session ended with four plan reviews pending at
once, and a restart would have reattached all four.

Three gaps made this possible:

- **Typing while a decision is pending has no defined meaning.** Only a pending
  question locks the composer (spec 107 D2). With a plan, command or patch
  pending, Send starts an ordinary new turn.
- **Nothing settles an older pause.** No code path supersedes a paused turn when
  the user moves on, so pauses pile up.
- **The model offered choices in prose.** The plan asked the owner to pick
  options A/B/C in its text instead of asking with `ask_user`, and the
  plan-review text still says "You can reorder the steps, retitle them…",
  describing the editor that spec 108 removed.

## 2. Requirements

1. **Typing over a pending plan declines it with changes.** While a plan review
   is the session's pending decision, the composer's placeholder reads
   "Describe changes to the plan" and Send (or Enter) sends the typed text as
   the plan's **decline with changes**, through the same route and the same
   audit as the card's *Decline* → *Send* (spec 108 D6). It never approves.
   The plan card's own Approve and Decline are unchanged.
2. **Other pending decisions lock the composer.** While a command, patch, check
   batch, fetch, round-budget, provider-fallback or context-review decision is
   pending, the prompt and Send are disabled, as for a question (spec 107 D2).
   The placeholder names the decision ("Approve or reject the command first").
   The composer status slot's **Show** still leads to the card. A question
   keeps its current lock.
3. **A session waits on one decision at a time.** When a task in a session
   pauses on a decision, every older task of that session still waiting on a
   decision is settled: its pending turn is taken off disk, its status becomes
   `Cancelled`, and an audit event names the newer task. Nothing is approved
   or run. On startup, a session already holding several pending decisions
   keeps only its newest the same way. A worker session is excluded: its
   pauses belong to the worker (spec 57).
4. **The model asks for choices with `ask_user`.** The desktop system prompt
   and the `propose_plan` and `ask_user` descriptions tell the model to ask a
   choice the user must make with `ask_user` (2–4 options) before proposing a
   plan, never as options written into a plan or an answer. The plan-review
   text no longer mentions reordering or retitling steps: it says the user can
   approve the plan or decline it and say what to change.
5. **Rules that do not change.** A typed message never answers a question,
   never approves anything and never resumes a paused command, patch or check
   (AGENTS.md: "a paused turn is resumed only by the route for what it is
   paused for"). A steering note still reaches only a running turn (spec 110
   D7).

## 3. Non-goals

- Changing what a plan, command or patch needs approval for.
- Free-text replies to a command or patch (a reason with a reject). A route
  for that would be its own spec.
- Worker sessions' composer (spec 57 owns it).
- Prompt changes beyond requirement 4.

## 4. Acceptance criteria

- With a plan review pending, typing "keep option A" and pressing Enter posts
  one `/api/resume-plan-stream` with `approved=false` and `changes="keep option
  A"`, and no `/api/ask-stream`; the session holds one pending decision
  afterwards (the revised plan's review, if the model proposes one).
- With a command, patch, check batch, fetch, round-budget, fallback or
  context-review decision pending, `#chat-prompt` and Send are disabled and the
  placeholder names it; answering the card re-enables them.
- Engine test: a session whose task A paused on a plan review and whose new
  task B pauses on a decision leaves A `Cancelled`, A's pending file gone, an
  audit event naming B, and `reattach_pending_approvals` returning only B.
- Engine test: a data dir with one session holding three pending decisions
  reattaches only the newest after the startup sweep, and the other two are
  `Cancelled` with their files gone.
- The plan-review text contains no "reorder" or "retitle"; the desktop system
  prompt and the two tool descriptions carry the `ask_user` guidance (string
  tests), and the deterministic eval tier passes.
- `USER_GUIDE.md` says what typing does while each kind of decision is pending.

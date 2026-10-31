# Feature Spec: A Worker Can Ask Instead of Guessing

Status: Not started
Order: 104 of 106
Plan: none. This is a single-worker capability gap found during the 2026-10-31
investigation.
Depends on: [#50](50_model_initiated_clarification/proposal.md) (durable
questions), [#57](57_worker_sessions/proposal.md) (worker process and control
socket), and [#99](99_worker_task_outcomes.md) (worker outcome UI), all built.
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

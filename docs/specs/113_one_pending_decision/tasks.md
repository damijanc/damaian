# Spec 113 tasks

**Started:** not yet

Implementation plan for [`proposal.md`](proposal.md). The evidence, the code
survey and the decisions are in [`context.md`](context.md).

## Global constraints

- One task, one session (AGENTS.md).
- No change to what needs approval, and no new route that resumes a paused
  turn. A typed message never approves (requirement 5).
- Tasks 1 and 3 both edit `workspace-engine/src/chat/`. Task 2 edits `app.js`.
  Run them one at a time.
- Scoped checks per task; the workspace gate stays with CI. UI changes are
  checked in the running app (`serves_the_ui_for_manual_inspection` on its own
  port and data dir, never 4765, stopped by PID).

| Task | State | Durable result |
|---|---|---|
| 1 · One pending decision per session (req 3, D3, D4) | Not started | |
| 2 · What typing does while a decision waits (req 1, 2, D1, D2) | Not started | |
| 3 · The model asks choices with `ask_user` (req 4, D5) | Not started | |
| 4 · Documentation and close-out | Not started | |

## Task 1 · One pending decision per session (req 3, D3, D4)

- [ ] When a task's status becomes `WaitingForApproval` with its pending link,
      settle every older `WaitingForApproval` task of the same session: take
      its pending turn through the store, set it `Cancelled`, audit
      `pending_decision_superseded` (`taskId`, `supersededBy`, `kind`).
      Exclude worker sessions.
- [ ] The startup sweep applies the same rule to a session with several pending
      decisions before reattaching (keep the newest).
- [ ] Engine tests: the two acceptance criteria for requirement 3, plus a
      worker session left alone and nothing approved or run.
- [ ] Check what reads `WaitingForApproval` (sidebar state, `/api/pending-card`,
      recovery) and record that a settled task drops out of each.

Files: `crates/workspace-engine/src/` (session store, recovery, the pause
path), `crates/desktop-shell/src/recovery.rs` if the sweep needs it, tests.
Checks: scoped clippy for each crate touched; scoped workspace-engine tests;
`cargo nextest run -p desktop-shell`; the deterministic eval tier.

## Task 2 · What typing does while a decision waits (req 1, 2, D1, D2)

- [ ] `sendChatPrompt`: with a pending plan review (not in a worker view), post
      the text as the plan's decline with changes, exactly as the card's
      Decline → Send does, and clear the composer.
- [ ] Generalise the composer lock: every pending kind except `plan` and
      `question` disables `#chat-prompt` and Send, with a placeholder naming
      it. The question lock is unchanged. A plan sets the placeholder
      "Describe changes to the plan".
- [ ] Desktop-shell tests in the `app_js_…` style.
- [ ] Manual check: a pending plan (injected or seeded) with typed text posts
      one `/api/resume-plan-stream` with `approved=false` and the text, and no
      `/api/ask-stream`; a pending command locks the composer.

Files: `crates/desktop-shell/static/app.js`, `style.css` if needed,
desktop-shell tests. Checks: `scripts/quality-gate.sh fmt node lint typos
deny`; `cargo clippy -p desktop-shell --all-targets --locked -- -D warnings`;
`cargo nextest run -p desktop-shell`.

## Task 3 · The model asks choices with `ask_user` (req 4, D5)

- [ ] Desktop system prompt (Code and Plan modes): ask a choice the user must
      make with `ask_user` before proposing a plan; never write options for the
      user into a plan or an answer.
- [ ] `propose_plan` and `ask_user` descriptions: one sentence each to the same
      effect.
- [ ] `plan_review_response`: drop "reorder … retitle … remove"; say the user
      can approve the plan, or decline it and say what to change.
- [ ] String tests for the three; the deterministic eval tier passes (update
      any scenario that asserts the old text).

Files: `crates/workspace-engine/src/chat/{prompt.rs,tool_definitions.rs,plan_flow.rs}`
and tests; eval scenarios if they assert the old text. Checks: scoped clippy;
scoped workspace-engine tests; `cargo run -p eval-harness -- run --tier
deterministic`.

## Task 4 · Documentation and close-out

- [ ] `USER_GUIDE.md`: what typing does while each kind of decision is
      pending; one pending decision per session. `TROUBLESHOOTING.md`: the
      `pending_decision_superseded` audit event and why an older card is gone.
- [ ] Spec 107: a dated note on D2 (the lock now covers every kind but a plan).
- [ ] Close-out per AGENTS.md: Status, this header, the README row and *What to
      build next*, the CHANGELOG *Unreleased* line, `Depends on:` lines naming
      #113.

Checks: `scripts/quality-gate.sh fmt node lint typos deny`; `npm run
specs:check -- --strict`.

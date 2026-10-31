# Feature Spec: Recovery Cards Follow the Conversation

Status: Done 2026-10-31 (specified and built the same day; local checks in §6,
the full gate is left to CI).
Order: 93 of 93
Plan: none. A defect reported in use, like #73–#92.
Depends on: [#17](17_durable_task_state_and_crash_recovery/proposal.md),
built.

## 1. Motivation

On 2026-10-31 the user stopped Damaian mid-session, `main` was reset to
before Damaian's commits, and the new build was opened. The session's first
prompt seemed to be gone. In fact it sat below six "Still waiting for you"
cards, about 3,450 px tall at 1280×900, because recovery prepends its cards
above the conversation. A patch card draws its whole diff.

Three of the four patch cards offered patches the log had already applied
(`patch_file_decided`, `applied: true`). Their tasks still read
`waiting_for_approval`, because the process stopped before the turn moved on.
The reset had put the files back to the patches' base, so each one looked
unapplied, and **Apply** would have restored the changes that had just been
set aside.

## 2. Requirements

1. Recovery does not offer a patch when the log holds a decision for every one
   of its files, applied or rejected.
2. Recovery cards are drawn after the conversation, in the order they were
   built, and the first is scrolled into view. The conversation's start stays
   at the top of the log.
3. Other pending approvals (commands, plans, questions, check batches) are
   reattached as before.

## 3. Non-goals

- Changing the stale task's status. It is left as it is; the sweep only stops
  offering a review that already happened.
- Collapsing a reattached patch card's diff.

## 4. Design

- **`recovery.rs`:** `reattach_pending_approvals` skips a `patch` link when
  `patch_already_decided` finds a decision for every file of the patch. A
  patch it cannot load falls through to the existing *unreadable* refusal.
- **`app.js`:** `renderRecoveryPrompts` appends the cards to `#chat-log`
  instead of prepending them, then scrolls the first into view.

## 5. Acceptance

- `a_patch_the_log_already_decided_is_not_reattached`
  (`tests/crash_recovery.rs`).
- The card order was checked by reading. The shell's web UI has no automated
  layout tests.

## 6. Verification

Local checks, 2026-10-31:
- `scripts/quality-gate.sh fmt node lint typos deny`;
- `cargo clippy -p workspace-engine --all-targets --locked -- -D warnings`;
- `cargo nextest run -p workspace-engine --locked --test crash_recovery`.

The workspace clippy and test suite are left to CI.

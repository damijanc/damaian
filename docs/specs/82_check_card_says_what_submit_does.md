# Feature Spec: The Check Card Says What Its Button Will Do

Status: Done 2026-10-31 (specified and built the same day; local checks in §7,
the full gate is left to CI).
Order: 82 of 82
Plan: none. A defect reported in use, like #73–#80: an exception to the rule
in [`README.md`](README.md).
Depends on: [#23](23_verification_loop/proposal.md) and
[#80](80_worker_verifies_after_its_patch.md), both built.

## 1. Motivation

On 2026-10-30 a worker session (#80) applied its patch and paused on a check
card with two `cargo test` checks. Six seconds after the card appeared, both
were declined, and verification recorded `nothing_verified` (#78). The user
had not meant to decline anything.

The card's boxes start unticked, and its only button read **Submit checks**.
Pressing it sent `approved: false` for every unticked box, which is
`declined_by_user` for each check. The button did not say that it would skip
everything.

## 2. Requirements

1. The boxes still start unticked. Each check is a command that needs
   approval (#23 §5.3). Ticked boxes would turn an unread click into an
   approval, which is the click-through #23 was written to avoid. A card
   reattached after a restart keeps its existing ticked boxes.
2. The button names what it will do with the boxes as they stand, and updates
   whenever a box changes:
   - nothing ticked: **Skip all checks** (**Skip the check** for one check);
   - everything ticked: **Run 2 checks** (**Run 1 check** for one);
   - some ticked: **Run 1, skip 1**.
3. A **Run all** button ticks every box and submits. It is shown only when
   there are two or more checks and not all are ticked, so it never repeats
   the main button.
4. The card's note tells the user to tick the checks to run.
5. The worker's card (#80, drawn with `decide`) behaves the same way. It still
   has no Always allow.

## 3. Non-goals

- Changing what a declined check means, or the `nothing_verified` outcome.
  Those belong to #23 and #78.
- A confirmation step for skipping everything. The label already says what
  will happen, and declining every check is still a valid answer.
- The wire format. The shell's `parse_check_decisions` and the worker's
  `checks` command are unchanged.

## 4. Design

- **`app.js`:**
  - `checkSubmitLabel(runCount, total)` returns the wording in §2.2.
  - `createCheckBatchCard` calls it from `syncSubmit`, which runs once when
    the card is built and again on each box's `change` event. `syncSubmit`
    also hides **Run all** as §2.3 describes.
  - **Run all** ticks every box, dispatches `change` so Always allow is
    enabled as if clicked, and calls the same `submitChecks` the main button
    uses. Both buttons are disabled while the decisions are being sent, and
    enabled again if sending fails.
- **`style.css`:** `.check-batch-actions` gains a gap for the second button.
- **`docs/ui-style-guide.html`:** the sample card shows the new labels.

## 5. Acceptance Criteria

- `the_check_card_button_says_what_it_will_do` (desktop-shell): the label
  helper has the four wordings; the card no longer says "Submit checks";
  boxes start as `reattached`; the label is recomputed on `change`; and the
  card has **Run all**.
- `resume_checks_endpoint_records_declined_checks` still pins that an
  all-declined answer reaches `nothing_verified`, because skipping everything
  remains a valid choice.

## 6. Implementation Notes

There is no JavaScript test harness for the UI (#80 §6), so the Rust test
checks the source shape, as the other JS-only assertions in `lib.rs` do. The
helper's output was also checked by hand with node, for 0–3 ticked out of 1–3
checks.

## 7. Verification

The `desktop-shell` suite and the fast checks passed locally, as did scoped
clippy for `desktop-shell`. The workspace clippy and test suite are left to
CI. The card was not driven in the live app.

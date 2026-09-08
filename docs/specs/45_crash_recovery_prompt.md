# Feature Spec: Crash Recovery Prompt

Status: Not started
Order: 45 of 46
Roadmap: `docs/ROADMAP/01_phase_1_trust_and_recovery.md`, Phase 1, Work
Package 2 (Must) — the user-facing half. That directory is local-only and not
committed, so the reference is a name rather than a link; this spec is
self-contained.
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

To be written when spec 17 lands and its API is real. Writing the design against
an imagined classification shape would produce exactly the kind of guesswork
spec 18's implementation kept catching.

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

To be completed during implementation.

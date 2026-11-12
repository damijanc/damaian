# Feature Spec: Interface Cleanup and Session Archive

Status: In progress (started 2026-11-10; Tasks 1, 3, 4, 5, 6, 8 and 9 done; Tasks 1–9 in [tasks.md](tasks.md))
Order: 108 of 108
Plan: none. Written from the owner's 2026-11-09/10 dogfooding of spec 33; the
decisions and code survey are in [context.md](context.md).
Depends on: [#98](../98_task_scoped_protected_patch_paths.md) (protected paths,
whose manual control this removes), [#94](../94_run_a_plan_in_a_worker.md)
(plan review "Run in a worker", which this removes) and
[#95](../95_one_workers_panel.md) (the Workers board) — all built; **nothing here is unbuilt, so this spec
is ready to start**. It shares `app.js` with
[#107](../107_visible_waiting_and_working/proposal.md) (Done 2026-11-11); build one after the
other, not in parallel.

## 1. Motivation

Using Damaian on a real spec, the owner hit five things that made the window
harder to use without adding anything he needed:

- **Protected paths** sat between the conversation and the composer in every
  session: an empty textarea and a **Protect paths** button taking about a
  fifth of the window, full width. He could not tell what it did; "no one will
  manually type the paths to protect".
- **Run in a worker** appeared on the plan review card of a planning-only task
  that writes docs in the current checkout, where handing off makes no sense.
- **No archive.** Tidying the sidebar means deleting a session and its history.
- **The Git status card prints every path twice** (`D docs/specs/33_….md
  docs/specs/33_….md`).
- **The task keeps its first model.** After switching from v4-flash to v4-pro
  mid-task, every call ran on v4-pro while the task record still named v4-flash.

## 2. Requirements

1. **Remove the manual Protected paths control from the main window**
   (decided by the owner). The engine guarantee of spec 98 stays: a worker
   still receives its parent's protected paths, a patch touching one is still
   refused and the refusal names the path, and the CLI control remains for
   scripted use. Protected paths are shown read-only where they explain
   something: on a worker's card and in a refused patch's message.
2. **Remove Run in a worker from the plan review card** (decided by the owner).
   Handing work to a worker stays available from a `worker-prompt` block's
   **Start worker** and from **New worker**.
3. **Archive a session.** The session ⋮ menu offers **Archive** (decided by the
   owner); an archived session leaves the sidebar (decided by the owner). It is
   listed under a collapsed **Archived** entry from which it can be opened or
   **Unarchived** (context D3). Archiving keeps the history, worktree and
   branch; **Delete** stays the destructive action.
4. **The Git status card names each path once**, with its status.
5. **A task records the model each call used**, and shows every model a task
   used where it shows the model.
6. **The plan review card is read, not edited, and declining asks how to change
   it** (decided by the owner, 2026-11-11). The steps are a numbered, read-only
   list with each step's required check shown as text: no title inputs, no
   ↑ ↓ × controls, no "+ Add a required check" editor. The card offers
   **Approve** and **Decline**. **Decline** opens a question in the card, "How
   should the plan be changed?", with a text field. Sending an answer leaves
   the plan's outstanding steps open, gives Damaian the answer, and Damaian
   must propose a revised plan, which comes back to the user as a new review
   card before anything changes (owner: "and then damaian needs to offer a new
   plan based on the user input"). A quiet **Decline without changes** keeps
   today's decline (nothing changes, the steps are skipped, the turn explains).
   The header no longer shows the deferred tool call as an upper-case chip; at
   most a short muted "Next: …" line.
7. **One decision at a time** (decided by the owner, 2026-11-11: "Asking user
   to approve 2 things at the same time must never happen"). No card asks the
   user for two decisions at once. Spec 107 D3's one-time worker suggestion is
   no longer shown inside the plan review card or on a patch card: at a plan
   review, **Approve** first, and only then, if the engine suggests a worker,
   the same card asks **Run in a worker** or **Continue here**; at a patch, the
   suggestion is asked on its own before the patch card is shown. Declining a
   plan never offers a worker.

## 3. Non-goals

- Changing protected-path enforcement, the handoff, or the CLI.
- Bulk archive, auto-archive or retention rules.
- The waiting/working visibility work — spec 107.

## 4. Acceptance criteria

- No session shows a Protected paths form; spec 98's enforcement tests pass
  unchanged; a worker's card lists its protected paths read-only.
- The plan review card has Approve and Decline only, and its steps are
  read-only; a worker suggestion appears only after Approve, never beside it,
  and never on the same card as a patch decision.
- Decline → "How should the plan be changed?" → an answer produces a revised
  plan on a new review card with no repository change in between (engine test
  and a deterministic eval scenario); Decline without changes behaves as today.
- Archive removes a session from the sidebar and from the Workers board's
  default view; Archived lists it; Unarchive restores it; Delete still deletes.
  Archiving is durable across a reload and a restart.
- The Git status card shows `D path`, not `D path path`.
- A task that switched models mid-turn reports both models.
- `USER_GUIDE.md` and `TROUBLESHOOTING.md` are updated; spec 98 carries a dated
  note that its manual UI control was removed and why.

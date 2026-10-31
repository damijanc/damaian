# Feature Spec: The Commit Draft Names the User's Request

Status: Done 2026-10-31 (specified and built the same day; local checks in §6,
the full gate is left to CI).
Order: 89 of 92
Plan: none. A defect reported in use, like #73–#88.
Depends on: [#35](35_commit_preparation/proposal.md), built.

## 1. Motivation

Two commits made through **Commit…** on 2026-10-31 read:

- `chore: docs/specs/86_… The user reviewed the plan and approved it. The plan is now: 1. Read AGENTS.md, …`
- `chore: crates/workspace-engine/src/chat.rs continue`

The draft took the latest message in the user's role. A plan approval
appends its continuation note in that role, and a typed "continue" names
nothing the commit did. The message was also used whole, however long.

## 2. Requirements

1. The draft's summary is the latest prompt the user typed to start a turn.
   Damaian's own turns (verification and repair), and bare continuations
   ("continue", "go on", "ok", …), are skipped.
2. Only the summary's first line is used, cut at a word to at most 72
   characters.
3. With nothing left to use, the draft falls back to
   `chore: update workspace changes`, as before.

## 3. Non-goals

- The model-written draft (**Draft with model**), which is unchanged.
- The `chore:` prefix and the single-file form, which are unchanged.

## 4. Design

- **`commit.rs`:**
  - `suggest_message` reads `SessionStore::read_tasks`, newest first, instead
    of the messages.
  - It skips prompts that `chat::is_damaian_task_prompt` matches (those that
    start with the verification prompt) and those that `is_bare_continuation`
    matches.
  - `commit_summary` cuts the prompt it chooses.

## 5. Acceptance

- `the_draft_message_names_the_users_request_not_damaians_notes`
  (`tests/commit_preparation.rs`): a plan-approval note and a later
  "continue" leave the draft `chore: a.txt Add a feature`.
- The unit tests `a_commit_summary_is_the_prompts_first_line_cut_at_a_word`
  and `a_bare_continuation_names_nothing`.

## 6. Verification

As #88.

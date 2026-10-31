# Feature Spec: One Check per Place It Runs

Status: Done 2026-10-31 (specified and built the same day; local checks in §6,
the full gate is left to CI).
Order: 90 of 91
Plan: none. A defect reported in use, like #73–#89.
Depends on: [#23](23_verification_loop/proposal.md) and
[#82](82_check_card_says_what_submit_does.md), both built.

## 1. Motivation

On 2026-10-31 a change in `crates/workspace-engine` raised a check card with
two identical rows, `cargo test` and `cargo test`. One ran at the repository
root and one in the member, because #23 §5.2 selects the owning root and every
enclosing root. The card showed only the command. The root is a virtual
workspace, so its `cargo test` already runs the member's tests, and the second
run only repeated work, on a machine where `cargo test` is the slow part.

## 2. Requirements

1. Each row of a check card names where the check runs ("in repository root",
   "in crates/workspace-engine"), as the report already does.
2. A member's `cargo test` is not proposed when a selected enclosing root also
   runs `cargo test` and is known to run the member's tests:
   - the root's `Cargo.toml` has `[workspace]` and no `[package]`;
   - it has no `default-members` and no `exclude`;
   - its `members` patterns match the member;
   - the member's own `Cargo.toml` is not a workspace.
3. In every other case both checks are kept, as before. The rule can drop a
   repeated check, never a needed one.
4. A dropped check is neither proposed nor stored (#23 §5.2: nothing is
   stored that will not run). Both roots are still selected.

## 3. Non-goals

- Other build tools. Only Cargo's rule is read.
- A full TOML parser. The manifest is read line by line, and anything
  unexpected keeps both checks.

## 4. Design

- **`verification.rs`:** before proposing, `propose_checks` runs
  `drop_covered_member_checks` on the restricted map. It removes
  `cargo test` from each root that `cargo_workspace_runs_member` says an
  enclosing root covers.
- **`chat.rs`:** `AgentCommandProposal` carries `directory`, relative to the
  repository root. It is empty at the root and for MCP and browser calls.
- **`desktop-shell`:** `agent_command_json` sends `directory`, and the check
  card prints it under the command. The worker's card uses the same JSON.

## 5. Acceptance

All in `tests/verification_loop.rs`:
- `a_member_change_selects_its_own_root_and_every_enclosing_root` now proves
  the member's `cargo test` is dropped under a virtual workspace;
- `a_member_check_is_kept_when_the_root_does_not_run_it` proves that a root
  that is also a package keeps both;
- `only_the_selected_roots_proposals_are_stored` stores one proposal.

## 6. Verification

As #88.

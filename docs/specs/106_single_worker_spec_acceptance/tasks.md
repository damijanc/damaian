# Spec 106 tasks

**Started:** not yet — planned 2026-11-09

Implementation plan for [`proposal.md`](proposal.md). The design decisions and
the code survey they rest on are in [`context.md`](context.md).

## Global constraints

- The worker, the engine and the completion gate decide; the driver only
  answers decisions the UI would present (context D5). It never edits the
  fixture repository, never runs Git there, and never sends `continue-task`,
  `continue-repair`, a `message` follow-up, a `note` or a round-budget
  approval.
- Success is computed from durable records (context D7), never from model text.
- The live tier is `#[ignore]`d, uses an isolated data directory and a freshly
  materialized fixture, reads its key from an environment variable only, and
  documents its exact manual command in its doc comment.
- Nothing here weakens command policy, patch hash verification, protected
  paths, secret redaction or the question/approval separation.
- Each task is a fresh agent session. Scoped checks per task; the workspace
  clippy and test suite stay with CI under `AGENTS.md`.

| Task | State | Durable result |
|---|---|---|
| 1 · Gate accepts an unchanged pre-existing failure (context D1) | Not started | |
| 2 · `single_worker_spec` fixture | Not started | |
| 3 · Acceptance driver, criteria and report (`eval-harness::acceptance`) | Not started | |
| 4 · Deterministic tier through the in-process worker host | Not started | |
| 5 · Live tier through the real worker binary | Not started | |
| 6 · Three consecutive live acceptance runs | Not started | |
| 7 · Documentation and close-out | Not started | |

## Task 1 · Gate accepts an unchanged pre-existing failure

Requirement 11, context D1. Engine only (`completion.rs`, reading the
provenance records spec 102 already writes).

- [ ] Failing tests first, through the orchestrator: (a) a required floor check
      fails at task start and after the change with the same failure → the task
      completes with a named "pre-existing failure, unchanged" disposition in the
      completion report, not a pass; (b) the same check with an extra introduced
      finding → still blocks (repair first); after the repair → accepted;
      (c) `uncertain` (no task-start baseline) → never accepted; (d) a declined
      baseline → not accepted; (e) generic (unparsed) checks use R10's identity.
- [ ] Add the disposition to `CompletionGateReport` (serde default, so old
      reports load) and render it wherever waivers are rendered (desktop task
      control, CLI, recovery audit).
- [ ] Update specs 101 (the gate and its waiver text), 102 (what provenance now
      decides) and 105 (the completion obligation), each with a dated note.
- [ ] Scoped checks for workspace-engine, desktop-shell, damaian-cli,
      eval-harness; the deterministic eval tier.

## Task 2 · `single_worker_spec` fixture

Requirements 1–4, context §3.

- [ ] `crates/eval-harness/fixtures/single_worker_spec/` with `fixture.toml`
      (version 1), the three crates, docs, the spec record, `AGENTS.md` and the
      worker prompt the acceptance run sends (stored beside the fixture so both
      tiers use the same text).
- [ ] The tie-rounding ambiguity and its two possible answers, with the expected
      test value for each, recorded in the fixture's `acceptance.toml` (question
      key words, scripted answer, expected value, protected path, expected
      command families).
- [ ] A fixture test (not a scenario): materialize it, then confirm the
      workspace-root `cargo test` fails only on `legacy`, the `pricing` and
      `invoice` checks pass, and the protected path parses from the prompt.
- [ ] No scenario count changes.

## Task 3 · Acceptance driver, criteria and report

Context D2, D5–D8. Library code in `eval-harness`, no process or network.

- [ ] A transport trait the two tiers implement (status, approve/reject,
      checks, answer, apply-patch-then-verify), so the driver is shared and has
      no forbidden action to call.
- [ ] The decision policy (D6) and the forbidden list (D5) as data, with unit
      tests: each forbidden action is unreachable; a protected-path proposal
      fails the run with its reason; an unexpected question, an unexpected
      command and an out-of-repository command are each answered or rejected
      and recorded as deviations without failing the run.
- [ ] The report's Deviations section (D9), and a summary shaped for #100's
      worker return so it can be delivered into the parent session.
- [ ] Criteria (requirement 8) evaluated from durable records (D7), each
      reporting the reader and value; unit tests on fabricated session logs for
      each criterion's pass and fail.
- [ ] Report (§5 fields) as JSON plus Markdown under
      `<data_dir>/acceptance/<run-id>/`.

## Task 4 · Deterministic tier through the in-process worker host

Requirement 5, context D3.

- [ ] `desktop-shell` dev-depends on `eval-harness`; check `cargo deny`.
- [ ] One scripted run through `host_in_process` and the shared driver that
      exercises every transition: pre-plan exploration, plan review with
      budget, the clarification question and resume, two mutation batches,
      task-start baselines (pre-existing and introduced), one repair, an
      internal segment boundary, completion verification, the task-record
      update, and final `Complete` with D1's disposition.
- [ ] Negative guards: a scripted protected-path patch fails the run; an
      unexpected command fails it; a run left needing Continue fails with that
      reason.
- [ ] Runs in the workspace test suite; no side effects outside temp dirs.

## Task 5 · Live tier through the real worker binary

Requirement 6, context D4.

- [ ] An `#[ignore]`d `desktop-shell` integration test that materializes the
      fixture, writes `user.conf` for the selected provider, creates a parent
      (main) session and spawns the real worker from it with `Supervisor`,
      drives it with the shared driver over its socket, applies patches the way
      the shell does, writes the report and returns its summary, deviations
      included, to the parent session (D9).
- [ ] Doc comment with the exact command, including exporting the key for one
      run (for DeepSeek, from the Keychain item `deepseek-api-key`).
- [ ] A wiring smoke run with `DAMAIAN_MOCK_MODEL_RESPONSE` proving spawn,
      socket, shutdown and report writing (also `#[ignore]`, it spawns a
      process).

## Task 6 · Three consecutive live acceptance runs

Requirements 8–10. Needs Damijan's choice of provider and model and his
go-ahead for the cost before the first run.

- [ ] Run the live tier until three consecutive successes from fresh state.
      Record each run here: provider/model, duration, rounds, segments,
      compactions, tokens, approvals by kind, questions, repair attempts,
      outcome.
- [ ] A failed run resets the count; its cause is diagnosed from the report and
      recorded, and a product defect becomes its own fix before the next
      attempt.

## Task 7 · Documentation and close-out

- [ ] `USER_GUIDE.md`: what the acceptance gate proves and does not prove;
      `TROUBLESHOOTING.md`: running the live tier and reading its report.
- [ ] `docs/specs/README.md`, `CHANGELOG.md` Unreleased, dependency lines.
- [ ] Status Done only after Task 6's three runs **and** a green CI run
      (requirement 10).

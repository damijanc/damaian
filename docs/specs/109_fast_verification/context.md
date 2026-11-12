# Spec 109 context

**Specified:** 2026-11-11. Line numbers are approximate; re-read before editing.

## Evidence

Spec 33 Task 1 on Damaian v0.69.0, session `session_18e6f2fdaa423e08_14169_15`
(2026-11-11): first patch 10:35:33 (51 lines, `config.rs`); pre-apply baseline
of `cargo test` at the root and in `crates/workspace-engine` (the root run spent
over two minutes just compiling every crate, including `desktop-app`); after the
apply, a per-change "Checks need approval: cargo test in crates/workspace-engine"
card. The plan (9 steps) carried `cargo nextest run -p workspace-engine --test
permission_profiles --test foundation` and scoped clippy as required checks.
Local observations 81 (duplicate member baseline), 83 (full checks per patch;
partly fixed by spec 107 D3), 86 (main-checkout test-process overhead) and 89
(running baseline not visible).

## Survey: what exists today

- **Baseline gate** (`baseline.rs` `BaselineCoordinator::prepare` ≈193): before
  a patch applies, the checks the patch selects plus the completion floor of
  touched roots (`floor_checks`) that have no task-start baseline are put on a
  baseline card ("This patch has not been applied…", ≈270). A check is baselined
  once per task, before its first patch. `prepare_for_command` (≈289, review fix
  R18) does the same before the first mutating-capable command.
- **Provenance** (`provenance.rs` `classify` ≈100): no baseline →
  `UnknownProvenance`; `Passed` → `NewRegression`; `Uncertain` → `Uncertain`;
  `Declined`/`Unavailable` → `UnknownProvenance`; `Failed` → pre-existing /
  unchanged / worsened by comparing termination and findings.
- **Per-change selection** (`chat.rs` ≈2690 and `verification.rs`
  `propose_checks` / `propose_completion_checks`): the detected checks of the
  owning root per change; the floor (owning + enclosing roots) at completion;
  spec 107 Task 5 made a continuing planless turn per-change too.
- **Plan checks** (spec 105): a step's `PlannedCheck { command,
  working_directory, required }`; deferred to its step while open; spec 106 Task
  6 added the engine rerun of a finished step's stale check.
- **Approval:** each check goes through the command policy
  (`ValidationOrchestrator::store_proposal`); "Ask before test, build and lint
  commands" (`require_approval_for_risky_commands`, on by default) makes
  `cargo test` ask every time. "Always allow" is a persistent, user-scope rule
  (spec 34).
- **Recorded runs:** `CheckRun` carries the covered `file_hashes` and the
  mutation `generation`; `is_stale` compares them with the current tree
  (`verification.rs` ≈715).
- **Findings:** `finding/rust_test.rs` parses failing test names and locations
  from `cargo test` output; nextest output is parsed too.

## Decisions

- **D1 — lazy baseline (recommended).** Drop the pre-apply baseline card and
  R18's pre-command baseline. When a per-change or completion check fails and
  the task has no baseline for it, take one then, on the pre-patch tree, and
  classify with it. A passing check never needs one.
- **D2 — the pre-patch tree (to confirm in Task 3).** Build it in a temporary
  git worktree at `HEAD`, then apply the task's state *before* the patch under
  verification: the uncommitted changes that existed when the task started plus
  the task's earlier applied patches (the patch engine stores each applied
  patch's before-hashes and content). Never touch the user's checkout. Remove the
  worktree afterwards. If the pre-patch state cannot be reconstructed exactly
  (hash mismatch), record `CheckBaseline::Unavailable` — the existing
  `UnknownProvenance` path — rather than guess.
- **D3 — targeted rerun (recommended).** For Rust test findings, rerun the
  failing tests by name (`cargo test -- --exact <names>` / nextest `-E`
  filter) in the pre-patch tree; compare the same set. Anything else reruns the
  whole check.
- **D4 — per-change check choice (recommended).** If the plan has a required
  check whose working directory is the patch's owning root (or an enclosing
  root), run that per change instead of the detected `cargo test` for that root.
  The detected check stays in the completion floor and runs once at the end.
  *Resolved in Task 2 (2026-11-11):* "covers" means the plan check's
  `CheckId` directory equals the owning root or one of the map's roots
  enclosing it; a directory that is not a root covers nothing. A covered root
  proposes none of its detected checks per change (a cargo root detects only
  `cargo test`). Only an **approved** plan's `required` checks count, since an
  unapproved plan gates nothing (spec 105). **Deferral wins:** a covering check
  whose own step is still open (spec 105 `plan_defers_check`) still displaces
  the detected checks but is not run by the loop; its step runs it as an
  ordinary command. Running it through the loop would write a `CheckRun`, which
  the gate treats as authoritative over the step's later `CommandExit`, so the
  step's own run would read stale. A per-change verification whose only
  covering check is deferred therefore runs nothing for that root —
  `Selection::Proposed([])`, not "no checks available"; it ends
  `nothing_verified` and the report names each such check and its step
  (`verification_deferred`, "left to its plan step"), never as evidence.
  Reuse (D6) applies to
  the substituted check. The completion branch, the answer-time exact floor
  run and the gate's evidence lookup are unchanged.
  *Fixed 2026-11-12 — which plan check stands in:* per change, only the
  required check of the step the change belongs to (the plan's in-progress
  step) stands in for a covered root. Any approved required check still decides
  whether a root is covered, so a covered root never runs its detected checks
  per change. Task 2 as first landed took every covering required check, and
  only open steps' checks were deferred, so each finished step's check reran
  on every later patch — in the owner's 5-step plan with every check at the
  repository root, several extra runs per patch. The rule now:
  - the current step's check is normally deferred to that still-open step, so
    nothing runs per change and the report says "left to its plan step";
  - when the current step has no check covering the root, nothing runs per
    change either; the root's detected checks are recorded as
    `verification_deferred_to_completion` and the report says "`<check>` is
    left to task completion" (`VerificationReport::deferred_to_completion`,
    the same "Checks deferred" row);
  - a finished step's check is not a per-change check. It is rerun once, when
    stale, at completion: `verify_due` adds spec 106 Task 6's
    `plan_checks_needing_rerun` only to a completion run (all primary steps
    terminal, or the answer-time due), and the gate treats a finished step's
    stale check like the floor — while primary plan work is open or blocked
    it is not reported as `required_check_stale` (a failed or missing one
    still is), so a worker's automatic continuation is not stopped by it. The
    answer-time completion run reruns it with the floor, as before.
- **D5 — approve for this session, for commands that do not change the
  repository (decided by the owner 2026-11-12, definition confirmed the same
  day: "read only + check list but we need to have a configuration entry that
  user can add more").** Replaces the 2026-11-11 per-task
  proposal.
  - *Why "read-only" needs a definition.* Commands the policy admits as
    strictly read-only (`ls`, `git status`, `rg`, … — `is_low_risk_read_only`)
    already run without a card in Code mode, so an option limited to them would
    almost never appear. The commands that ask again and again are checks:
    `cargo test`, `cargo nextest run …`, `cargo clippy …`, `cargo fmt --check`,
    `typos`. They write build output (`target/`) but not the repository's
    files.
  - *Definition: no ecosystem list (owner, 2026-11-12: "we have haskel,
    closure, GO ... point is that it is not a definite list").* Whether a
    command qualifies is decided from what the command policy already says
    about **any** command, plus what the run is seen to do — never from a list
    of languages or tools:
    1. **Why the policy asks.** The policy explains every approval with
       `ApprovalCause`s. A command qualifies only when its causes are limited
       to "Damaian does not know what `<program>` does" (`Unknown`) and the
       ask-before-checks setting (`Setting`). Any other cause disqualifies it
       in every language: `Blocklisted`, `Destructive`, `ShellControl` (so
       `&&`, `|`, `;`, redirects never qualify), `Docker`, `Network`,
       `HighRisk`, `OutsideRepository`, `RestrictedPath`, `ProfileBlocked`.
    2. **No known writer.** `is_source_writing_command` (formatters without
       `--check`, `--fix`, `--write`, …) disqualifies; it is a hint list that
       may grow, not the definition.
    3. **Seen not to change the repository.** The backstop below runs after
       every covered run, so "read-only" is a property each run must keep, not
       a promise from a list: a `stack test`, `lein test`, `go test`,
       `phpunit`, `ctest` or in-house `bin/check` qualifies on the same terms,
       and loses the approval the first time it changes a file.
  - *User additions are spec 111 rules.* The separate
    `session_approvable_commands` setting proposed earlier the same day is
    dropped: spec 111 gives the user ordered `allow`/`ask`/`deny` command rules
    with full authority, so a command the user wants to run without a card is a
    user `allow` rule, and **Approve for this session** writes a session-scoped
    rule of the exact parsed command and directory (spec 111 D2: session rules
    only `allow`, never override a user `deny` or a built-in block). Task 5
    builds on spec 111 Tasks 1–2.
  - *Script runners.* `npm run lint`, `make test`, `rake test` and the like run
    whatever the repository defines; the backstop is what catches a script that
    writes files, and it loses its approval at once.
  - *Backstop.* After each run covered by a session approval, the engine
    compares the repository's state before and after (git status plus hashes of
    changed and untracked files, ignoring git-ignored build output). If anything
    changed, the session approval for that command is revoked, the change is
    reported in the turn and the audit, and the next proposal asks again.
  - *Scope.* Command text and working directory, exactly; this session only,
    including its continuations and resumed turns; a durable session event
    (`command_session_approved`, `command_session_approval_revoked`) so a
    restart keeps it; ended by archive, delete or **Revoke session approvals**
    (session ⋮). Never across sessions, never written to config, never "always
    allow". A worker gets its own session and starts with none.
  - *Trust.* Restrict-only: repository config may turn the option off
    (`session_command_approval=false`) but cannot turn it on or extend the
    list; a declined command is not remembered; a blocked command never gets
    it.
  - *Audit.* `command_approved_for_session` when chosen, each automatic run
    audited with `approvedBy=session_approval`, revocations audited with the
    reason.
  - *UI.* The card's **Approve** button gets a chevron with **Approve for this
    session**, only when the engine says the command qualifies (the card
    payload carries `sessionApprovable` and the reason when not). A run that
    happens under it shows "Approved for this session" in its row. Baseline and
    check batch cards offer it per check.
- **D6 — reuse (recommended).** Before proposing a check, if a `CheckRun` of the
  same identity is not stale, reuse it (no command, no card) and say so in the
  report.
- **D8 — stop anything (decided by the owner, 2026-11-11).** "We need to allow
  the user to stop any ongoing action, even if it is destructive." Today
  **Cancel** exists for baselines (spec 107 Task 4) and Stop for model rounds
  (the turn indicator); the post-apply verification started from the patch card
  (`startVerification`) has neither, and commands, hooks and MCP calls cannot be
  interrupted from the UI. One stop path: the UI's Stop fires the turn's
  `CancelToken`; every runner (command, check, baseline, hook, MCP, browser)
  honours it by ending its process group; a side-effecting action's marker is
  finished with an unknown outcome so recovery and the report say what may have
  happened. No action is exempt.
- **D7 — visibility.** One running-state component for checks and baselines:
  card header indicator, Cancel replacing the run button, the status line and the
  composer line from the same state (fixes OBSERVATIONS 89).
- **D9 — verification with no detected check (owner, 2026-11-12: "it is not a
  definite list").** Damaian detects checks from seven manifest files
  (`PROJECT_MANIFESTS`), so a project in any other ecosystem (Haskell, Clojure,
  PHP, C, C++, C#, …) ends verification "no checks available" (OBSERVATIONS
  95). No fix may rest on listing ecosystems. Instead: (1) when a touched root
  has no detected check, the approved plan's required checks covering it are
  its completion floor, so the checks the model found in the repository's own
  CI files, `Makefile`/`justfile` targets or README when it planned are what
  verify the task; (2) a user-scope setting, `project_checks` (per repository
  path: command and directory), adds a project's checks to its floor, edited in
  Settings; a repository may only narrow it (spec 34); (3) the planning prompt
  asks the model to name the repository's declared checks as single commands,
  not `&&` chains, and to say where it found them. The manifest detection
  stays as a fast path.


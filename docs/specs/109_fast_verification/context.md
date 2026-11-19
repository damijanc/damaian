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
  from `cargo test` output. *Correction (Task 4):* nextest output was not
  parsed — a failed `cargo nextest run` was one generic finding; Task 4 added
  `finding/nextest.rs`.

## Decisions

- **D1 — lazy baseline (recommended).** Drop the pre-apply baseline card and
  R18's pre-command baseline. When a per-change or completion check fails and
  the task has no baseline for it, take one then, on the pre-patch tree, and
  classify with it. A passing check never needs one.
  *Built in Task 3 (2026-11-12).* `finish_verification` (`chat.rs`) calls
  `BaselineCoordinator::prepare_for_failures` before `record_post_patch_provenance`,
  for every selected check whose run failed with a code result
  (`checks_needing_failure_baseline`: not passed, skipped, declined, an
  environment failure, a cancelled run or a browser check) and that has no
  baseline in the task chain. The baseline proposes the same command through
  the ordinary policy (`propose_command_at`, reason "Baseline for a failed
  check…"): blocked is `Unavailable`, auto-run runs at once in the task-start
  tree (D2), and one that needs approval parks the verification on the
  existing check card (`AgentCheckBatch`, now with an optional engine `title`
  and `note`: "This check failed. Run it on the files as they were before this
  task's changes to tell whether the failure is new…"), the pause marker
  `failure_baseline` (R18's `command_baseline` pause, repurposed). The answer
  goes through `resume_after_check_decision` → `resume_failure_baseline`, which
  runs the approved ones with spec 107's streamed `resume_streamed` under the
  turn's cancel token (Stop records the running and unstarted ones `Declined`)
  and then finishes the same verification. `classify` is unchanged. Removed:
  `BaselineCoordinator::prepare`/`prepare_for_command`, `PatchApplyOutcome`
  and `apply_stored_patch_with_baseline` (the apply route returns the applied
  result again), the patch card's baseline card and the
  `/api/resume-baseline(-stream)` routes, the `baseline` pending-decision kind,
  `command_floor_checks`, `RepositoryMap::roots_for_command`,
  `is_read_only_pipeline` and `is_source_writing_command`. `Uncertain` stays:
  a log written before spec 109 still carries it, and a required check whose
  run was cancelled reaches classification without a baseline.
- **D2 — the pre-patch tree (confirmed by the owner 2026-11-11).** Build it in a temporary
  git worktree at `HEAD`, then apply the task's state *before* the patch under
  verification: the uncommitted changes that existed when the task started plus
  the task's earlier applied patches (the patch engine stores each applied
  patch's before-hashes and content). Never touch the user's checkout. Remove the
  worktree afterwards. If the pre-patch state cannot be reconstructed exactly
  (hash mismatch), record `CheckBaseline::Unavailable` — the existing
  `UnknownProvenance` path — rather than guess.
  *Confirmed by the owner 2026-11-11; built in Task 3 (2026-11-12).* What
  Task 3 decided while building it:
  - **Which tree: the task-start tree.** The baseline tree is the state before
    the task chain changed anything, not the state just before the patch under
    verification. A baseline is recorded once per check per task chain and
    reused for every later failure of that check, and a completion-floor
    failure has no single patch under verification, so only the task-start
    state gives one meaning. It is also what spec 102's pre-apply gate
    produced, and it is the safe side: a regression patch 1 introduced but a
    check first saw after patch 3 (D4 makes this common, since the floor runs
    only at completion) still reads `new_regression`, where a "before patch 3"
    tree would have called it `pre_existing`. Test:
    `plan_turn::a_regression_first_seen_after_a_later_patch_is_classified_against_the_task_start_tree`.
  - **Task-start state is not captured at task start; the engine already
    records it.** Every applied patch writes a rollback snapshot of each file's
    pre-apply bytes with the base hash it was reviewed against
    (`patch_engine::read_pre_apply_content`), and each turn's checkpoint holds
    the pre-change bytes and hash of every file a patch or an approved command
    changed (spec 16, spec 85's census). For each path the chain changed (its
    applied patches' paths plus its commands' `verification_due` paths — a
    `Due` with no patch *and no checks*; the answer-time completion due also
    has no patch but names checks), the earliest record is the task-start
    content, verified against its hash. Cost at task start: nothing.
  - **Unavailable, never guessed** when: a changed path has no record; a record
    does not match its hash; a command changed a path and no checkpoint holds it
    from before the command (a checkpoint whose census was truncated,
    `command_effects_covered = false`, counts as no record); the repository is
    not a Git checkout; it has submodules. Recorded as
    `CheckBaseline::Unavailable { termination: unavailable_infrastructure }`,
    audit `verification_baseline_unavailable` with `termination:
    task_start_tree_unavailable` and the reason (paths, never content).
  - **The tree is a shared clone, not `git worktree add`.** `git clone --shared
    --no-checkout` of the common Git directory, then a detached checkout of
    `HEAD`, with `core.hooksPath=/dev/null`: it borrows the user's objects
    through alternates and writes nothing into the user's `.git`, so a crash
    mid-baseline leaves no worktree registration behind. A repository with no
    commit yet starts from an empty `git init`. Over it go every path `git
    status` reports changed or untracked in the user's checkout (copied as it
    is now) except the chain's own changed paths, which get their task-start
    content. The user's checkout is only read (`--no-optional-locks`). The tree
    lives under `<data_dir>/baselines/trees/` (the system temporary directory
    when the data directory is inside the repository, as in development, where
    a nested tree would sit in the checkout) and is removed when the baseline
    run ends, on every path including errors and cancel; a tree older than a
    day left by a crash is removed by the next build.
  - **Known limit: the user's own edits during the task.** A path the task did
    not change is taken as it is now, so an edit the user made by hand during
    the task counts as task-start state, and a failure it causes reads
    `pre_existing`, not the task's (pinned by
    `plan_turn::a_failure_from_a_users_own_edit_during_the_task_is_not_attributed_to_the_task`).
    Ignored files (build output, `node_modules`, `.env`) are not copied; a check
    that needs them fails differently in the tree, which classifies as
    `worsened` (repairable), not as a silent `pre_existing`.
  - **A Damaian-owned cargo cache.** Baselines run with `CARGO_TARGET_DIR` set
    to `<data_dir>/baselines/cargo-target/<16 hex of the canonical root>`, so a
    Rust baseline reuses compiled dependencies instead of building everything in
    each fresh tree, without writing into the user's `target`. It is kept
    between baselines and is not cleaned up automatically (Task 7 documents
    it). The name is hex only: cargo puts the target directory into
    `DYLD_FALLBACK_LIBRARY_PATH`, where the `:` of a `sha256:` repository id
    failed the build (found by the desktop-shell acceptance run).
- **D3 — targeted rerun (recommended).** For Rust test findings, rerun the
  failing tests by name (`cargo test -- --exact <names>` / nextest `-E`
  filter) in the pre-patch tree; compare the same set. Anything else reruns the
  whole check.
  *Built in Task 4 (2026-11-18).* What Task 4 decided while building it:
  - **One filter form for both runners:** the check's own command plus `--
    --exact <names>` (names sorted, deduplicated). nextest accepts libtest's
    `--exact` and name filters after `--` (checked against nextest 0.9.144),
    so no `-E` expression is built; `--exact` keeps `tests::a` from also
    running `tests::a_longer`. It only narrows what the original command runs.
    nextest also gets `--no-tests=pass` before `--`: a test the task added
    does not exist in the task-start tree, and an empty nextest run otherwise
    exits 4 (`error: no tests to run`), which would record a failed baseline
    and read a new regression as `worsened`. libtest passes an empty run.
  - **When it narrows:** the check is `cargo test`/`cargo t` or `cargo nextest
    run`/`r`; the command has no `--` already and no shell control
    characters; every error finding of the failed run is a parsed `Test`
    finding whose name is a plain `::` path of identifiers (a doctest's name
    is not); at least one. Warnings are ignored (the narrowed run builds the
    same targets). A compile error, a generic (unparsed) failure, or anything
    else reruns the whole check. The names come back out of the finding
    summaries (`finding::failed_test_name`, next to the parser that writes
    them).
  - **nextest findings are parsed now** (`finding/nextest.rs`, from real
    output in `finding/fixtures/nextest_failures_stderr.txt`): each failing
    status line (`FAIL`, `TIMEOUT`, `ABORT`, `SIG…`) is one `Test` finding
    shaped exactly like a `cargo test` one, with the panic message and
    location from its indented captured output; the closing `error: test run
    failed` is a cargo summary, not a compiler error. A failed nextest check's
    repair evidence therefore names its tests instead of one generic finding.
  - **The baseline records what it covered.** `verification_baseline` gains
    `tests` (empty: the whole check). A later failure of the same check is
    covered by an earlier baseline only if that baseline ran the whole check,
    or ran every test now failing (a declined, unavailable or uncertain one
    covers everything, as before); otherwise a new baseline runs for the
    union of both sets and replaces it (last record wins, as before).
  - **Policy:** the narrowed command is its own command to the policy and is
    what a baseline card shows. When it would need a card or is blocked but
    the whole check would run without one (the user allowed `cargo test`
    itself), the whole check runs instead, so narrowing never adds a card.
  - **Classification consequence:** spec 102's "fewer failures than at task
    start" fixture now reads `pre_existing`, not `unchanged_failure` — the
    narrowed baseline reruns only the test still failing, which failed then
    too, and never sees the one the patch fixed. Both classes are non-repair
    and the completion gate treats them alike (`completion.rs`); the other
    three fixture classes (new regression, pre-existing, worsened with only
    the new test introduced) are unchanged.
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
- **D10 — a build check after every change (owner, 2026-11-13).** Evidence:
  spec 33 Task 1 restart (session `session_18e7536110829758_18676_13`): step 1
  took nine patches over about two hours and 440 model calls; every
  per-change verification ended `nothing_verified` because the step's check
  (`cargo nextest run -p workspace-engine --test permission_profiles`) was
  deferred to the open step (D4, "deferral wins"); compiled out of band, the
  tree had 7 errors (functions called but never written, a destructure missing
  the new fields). D4 stays: a step's tests still wait for its end. Added
  beside it:
  - **What it is.** A plan step may name a `build` check beside its
    `check`: the same `PlannedCheck` shape (command, working directory,
    purpose), compile or type-check only, expected to take seconds on an
    incremental build. The planning prompt asks for one per step whenever the
    project has such a command, found the way D9 finds checks (CI files, build
    targets, README) and written as one command (`cargo check -p <crate>
    --tests`, `tsc --noEmit`, `go build ./...`, `mvn -q compile`, `make
    build`). Fast path: when the step names none and the touched root has a
    detected Cargo manifest, Damaian uses `cargo check --tests` for the owning
    package; any other detected manifest may add its own later. No language
    list decides anything (D9).
  - **When it runs.** In every per-change verification, first, for each root
    the change touched that has a build check (the current step's, else the
    fast path), whatever D4 then defers or runs. Reuse (D6) applies: an
    unchanged tree does not build twice. It is not run at completion, where
    the floor compiles anyway.
  - **What it records.** Its own event, `verification_build {check,
    outcome, errors}`, and no `CheckRun`, so the completion gate, D4's
    deferral, the step's later `CommandExit` and provenance never read it. The
    report and panel show a "Build" row ("builds", or "does not build: N
    errors").
  - **On failure.** The verification ends with the new outcome
    `build_failed`; no baseline (D1 is for checks whose provenance matters; a
    build error after a change is the change's to fix). The continuation turn
    gets the compiler's errors — secret-redacted, the first errors in full and
    a count of the rest, bounded like command output — with "your change does
    not build"; errors located only in files the task never changed are
    marked as such, since they may be pre-existing. The turn continues; no card
    is shown for a failure.
  - **Approval.** The build command goes through the command policy like any
    check; it asks the first time, and D5's **Approve for this session**
    covers the rest of the session once Task 5 lands (a build writes only
    ignored build output, so it qualifies). Until then it may ask per patch,
    which is still cheaper than two hours without a compiler.
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
  - **As built (Task 8, 2026-11-19).** `DeclaredFloor { plan_checks,
    project_checks }` (`verification.rs`) is what the floor takes beyond the
    detected checks. (1) `plan_floor_of_roots`: a floor root with no detected
    check takes the **approved** plan's required checks covering it by D4's
    rule (the check's directory is the root or encloses it); a root with a
    detected check takes none. `propose_completion_checks` proposes them after
    the detected floor (`ValidationOrchestrator::propose_floor_plan_check`), so
    a completion run of such a root is no longer "no checks available". The
    gate needs no change for them: it already requires every plan check as
    the plan's own. Per change, a covered root with no detected check and no
    current-step check now reports the plan's covering checks as left to
    completion (`verification_deferred_to_completion`) instead of nothing.
    (2) `project_checks.<repository_id>=<entry>|<entry>`, an entry being
    `<command>` (repository root) or `<directory>::<command>`
    (`config::ProjectCheck`; only a first `::` with no whitespace before it
    splits off a directory; absolute or `..` directories are refused). Keyed by
    repository id like the other per-repository decisions (OBSERVATIONS 98's
    naming survey applies). User and admin scope set it; repository scope may
    only narrow it (a new entry is rejected `restrict_only`); a profile is
    refused `user_owned`. A project check joins the floor when the task touched
    a non-documentation file inside its directory, whatever the manifests
    detect: in the gate (`CompletionGateService::with_project_checks`, used by
    every chat path through `ChatOrchestrator::completion_gate` and by
    `WorkspaceEngine::completion_report`; the settlement marker carries them so
    crash recovery recomputes the same floor without the user's config), in
    the completion run (`propose_project_check`), and in the answer-time floor
    run (`propose_checks_exactly` proposes a requested check no root detects
    only when it is a project check; a finished step's stale plan check is
    still added by `with_plan_checks` with its own reason). It is not a
    per-change check. Settings → **Project checks** edits the user's list
    through `GET`/`POST /api/project-checks` (user config only; `|` refused).
    (3) The `propose_plan` tool description asks for `check` and `build`
    commands from the repository's CI files, `Makefile`/`justfile` or README as
    one command each, naming the source in `purpose`. The schema's other
    descriptions were shortened so the tool definition is no longer than
    before: `bounded_worker_long_read_reread` sits a few tokens under its
    context window (as Task 9 found), and a longer description flips it to
    `task_compaction_cannot_fit`.


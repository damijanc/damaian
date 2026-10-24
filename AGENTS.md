# AGENTS.md

Instructions for coding agents working in this repository.

Human contributors should start with [docs/DEVELOPMENT.md](docs/DEVELOPMENT.md). This file focuses on repository rules, invariants, workflow expectations, and mistakes that are particularly easy for coding agents to make.

## What this project is

Damaian is a local-first AI coding assistant for macOS. The repository is a Rust workspace with a native Tauri desktop application. Damaian operates on the user's own Git repository, previews proposed file edits as diffs before applying them, and keeps file access, command execution, and API credentials under the user's control.

Two constraints shape most architectural decisions:

* **macOS only.** The `desktop-app` crate builds against the macOS system webview. Do not add Linux or Windows runtime support unless explicitly requested.
* **No Node.js runtime dependency.** Node.js is used only for build, lint, release, and development tooling. Never introduce Node.js as a dependency of the shipped application.

## Workspace layout

| Crate / directory              | Purpose                                                                                                                                           |
| ------------------------------ | ------------------------------------------------------------------------------------------------------------------------------------------------- |
| `crates/workspace-engine`      | Core services: indexing, context assembly, secret redaction, patch engine, command policy, model adapters, audit log, and most application logic. |
| `crates/damaian-cli`           | Command-line interface over the workspace engine. Binary: `damaian`.                                                                              |
| `crates/desktop-shell`         | Local HTTP shell and web UI on `127.0.0.1:4765`, Keychain access, the terminal PTY, worker sessions. Binary: `damaian-desktop-shell`.             |
| `crates/desktop-app`           | Native Tauri wrapper: embeds the shell; window, folder picker, updater, terminal IPC, worker host. Binary: `damaian-desktop`.                      |
| `crates/eval-harness`          | Evaluation harness over fixture repositories. Binary: `damaian-eval`.                                                                             |
| `crates/desktop-shell/static/` | Vanilla JavaScript and CSS web UI. No framework, bundler, or runtime build step.                                                                  |

## Before you change a feature

`docs/specs/` is the source of truth for implemented features. Every spec has a `Status:` field. Before changing behavior covered by a spec:

1. read the relevant spec;
2. inspect the current implementation it references;
3. update the spec when the design or behavior changes;
4. do not change code while leaving the specification describing the old behavior.

`docs/specs/README.md` lists specs in implementation order. Before selecting a new spec to implement, read the **What to build next** section there. Do not infer build order solely from spec numbers: dependencies, blockers, and concurrency constraints determine what may be implemented next.

### Keep spec and task records truthful

A stale summary is not cosmetic: someone reading the repository later will treat it as truth and may repeat work or make incorrect planning decisions. A folder spec records its status in several places, which must agree:

* the spec's own `Status:` line;
* its row in `docs/specs/README.md`;
* its `tasks.md` progress table;
* the `Started` / `Done` header in `tasks.md`.

Whenever a spec changes status, update all of them in the same change.

`tasks.md` checkboxes and progress records are durable project state, not informal notes. The progress table is the durable record; the checkboxes describe execution progress. Keep both truthful:

* Tick each checkbox when that step actually lands — do not wait until the end of the whole feature, and do not leave completed work represented as incomplete.
* When a task is finished, before starting the next one: tick every completed step, and update the task's row in the progress table with what actually landed, relevant test results, meaningful deviations from the written plan, and implementation discoveries that matter to later tasks.
* Keep the header accurate with actual dates: `**Started:** YYYY-MM-DD` once implementation starts, and `**Started:** YYYY-MM-DD — **Done:** YYYY-MM-DD` when the final task completes. Do not leave placeholder states such as `not yet`, `yes`, or similar once implementation has started.

### When a spec becomes Done

Finishing a spec affects documents outside the spec itself — stale summaries have repeatedly survived after the underlying specification was already correct. When marking a spec Done, in the same change:

1. Update every `Depends on:` entry that references the completed spec. A useful search is:

   ```bash
   grep -rn "<spec-file-or-folder>" docs/specs/*.md docs/specs/*/*.md
   ```

2. Re-evaluate `docs/specs/README.md` → **What to build next**: remove the completed item from the ready set and promote anything whose final unmet dependency has now been satisfied.
3. Remove the corresponding item from the `Unreleased` section of `CHANGELOG.md` (`Unreleased` means specified but not yet built). Do not manually create a release row; the release pipeline handles release rows.
4. Update `docs/USER_GUIDE.md` and `docs/TROUBLESHOOTING.md` when the completed feature changes anything visible to users or changes how failures should be understood.

## Agent sessions and durable state

The repository must remain understandable without access to the conversation that produced it. Code carries implementation state, specifications carry design intent, tests carry executable expectations, and progress records carry implementation history that matters to future work. Agent conversations are temporary working memory: anything important enough for a future task to depend on must graduate from conversation into one of those durable forms. If later work needs information that exists only in an earlier conversation, treat that as missing documentation and record the fact in the relevant spec, progress row, implementation notes, or another appropriate project document.

**One task, one session.** Treat each task in a folder spec as a separate agent session unless the task explicitly requires conversational continuity. When a task is finished: update `tasks.md`, record anything the next task genuinely needs, leave the working tree in the correct state, end the session, and start the next task in a fresh one. Do not carry earlier conversation history forward merely because the next task belongs to the same feature. Within a task, do not reset unnecessarily — history helps with debugging, interpreting test failures, remembering a decision made minutes earlier, iterating on a failing approach, and correlating command output with the code that produced it. The reset boundary is the **task boundary**, not every tool call or edit.

**Durable state between tasks** is the current source code, the feature specification, `tasks.md`, progress-table notes, implementation notes, committed project documentation, and the current working tree where sequential tasks intentionally build upon uncommitted work. The previous agent conversation is **not** durable. The next agent inspects what actually exists rather than inheriting earlier reasoning, failed approaches, temporary hypotheses, stale diffs, large command outputs, resolved test logs, tool-call history, or discussions that no longer affect the implementation.

**A fresh task session reads** only this `AGENTS.md`, the feature specification, the feature's global constraints, the current task in `tasks.md`, sections that task explicitly references, and the source files the work requires. Retrieve additional files only when needed; exploration is demand-driven. Do not preload every earlier task, the complete conversation history, the entire repository, complete test logs, unrelated specs, or all source files mentioned anywhere in the feature. A typical task prompt is about this size — the task specification carries the requirements, the repository carries the implementation state, and the conversation carries only the temporary context needed to finish the current task:

```worker-prompt
Implement Task N from docs/specs/<feature>/tasks.md.

Read AGENTS.md, the feature's global constraints, and the sections
referenced by Task N.

Inspect the current repository state.

Implement only Task N.

Run the scoped checks required by that task.

Update the task record when finished.

Stop after the task is complete.
```

**Final integration, verification, or close-out tasks** may legitimately reread the full feature specification, all acceptance criteria, the final implementation, progress-table entries, implementation notes, test coverage, and quality-gate requirements. They should still normally begin in a fresh agent session: their job is to evaluate the finished repository state, not inherit assumptions made during earlier implementation.

**Why:** reproducibility — a future worker must be able to continue the feature from project state, and correctness must not depend on private conversation history; and context and token efficiency — long sessions accumulate old reasoning, tool output, repeated file contents, obsolete diffs, resolved test failures, earlier tasks, and summaries of earlier summaries, which later model calls retransmit even though the repository already holds the useful result. Task-scoped sessions bound that growth. Do not sacrifice necessary context merely to reduce token usage, but do not retain history that has already been converted into code or documentation.

**Repository state outranks conversational memory.** When the repository disagrees with something said earlier in a conversation, inspect the repository and specification; the current project state is authoritative. Do not preserve an implementation merely because an earlier agent said it existed. Verify it.

## Background product specifications

The two root documents `ai_coding_assistant_specification.md` and `ai_coding_assistant_must_have.md` describe the original product intent. Treat them as background. The numbered files under `docs/specs/` describe the design and behavior that was actually specified and built; when they differ, use the numbered specifications unless explicitly asked to revisit the original product intent.

## Planning documents

Planning information is split across different locations. Do not put information into the wrong layer.

### [CHANGELOG.md](CHANGELOG.md)

Committed and public. It contains released versions, newest first; changes grouped by component; and an `Unreleased` section for functionality that has been specified but not yet built. Moving an implemented feature out of `Unreleased` remains part of finishing that feature.

Do not add release dates. Release rows are generated by the release pipeline, whose release job runs `npm run changelog:update` after a tagged release succeeds. You may run `npm run changelog:update -- --dry-run` to inspect what would be generated, and `npm run changelog:check` to detect tags that are not yet represented. Rows are inserted under the `<!-- releases -->` marker's separator line; do not remove that marker. A tag with no commits in its range is omitted. Generated release rows may be edited afterward; the updater must not overwrite an existing release row.

### `docs/PLAN/`

Local-only planning state, intentionally not committed. Do not link to files under `docs/PLAN/` from committed documentation. It may contain phases, work packages, execution dashboards, `OBSERVATIONS.md`, deferred decisions, and local planning context.

### `docs/specs/`

Committed project truth. A work package becomes a numbered specification when the design has become concrete enough to implement. Where relevant, specs carry a `Plan:` line identifying their planning origin.

## Picking up a piece of work

Before writing code for a specification, follow this order.

1. Read the specification. Check its `Status:`, Current State, relevant file references, Implementation Notes, dependencies, and acceptance criteria.
2. If available, inspect `docs/PLAN/README.md`. Confirm work-package state, blockers, priority, and dependencies.
3. If available, read `docs/PLAN/OBSERVATIONS.md` for known problems touching the same part of the system.
4. When you discover something outside the current task's scope, record it in `OBSERVATIONS.md` rather than silently expanding the task. Follow that file's own rules for evidence, falsifiability, and disposition.

If `docs/PLAN/` is missing, assume the checkout simply does not contain local planning state. Do not recreate it from memory, and do not infer that no planning context exists; say that it is unavailable if it is required. `docs/specs/` and `CHANGELOG.md` are sufficient for understanding committed product state, but they do not necessarily describe ideas that were explicitly rejected, deferred, or parked.

## Quality gate

CI runs seven checks. All seven must pass before work is claimed complete — scoped local checks change **when** the complete gate runs, not whether it runs.

```bash
cargo fmt --all -- --check
cargo clippy --workspace --all-targets --locked -- -D warnings
cargo nextest run --workspace --locked
node --check crates/desktop-shell/static/app.js
npm run lint:web
typos
cargo deny check
```

`npm run specs:check` is an additional advisory check: it reports mismatches between dependency declarations and specification statuses and intentionally does not fail by default (`npm run specs:check -- --strict` makes it blocking locally). Read its output when finishing a specification.

### Clippy

Clippy warnings are errors. Fix warnings instead of suppressing them. When a lint is genuinely inappropriate, a targeted `#[allow(...)]` with a comment explaining why is acceptable. Do not add broad suppression merely to make the gate pass.

### The full gate runs on CI, not locally

Policy since 2026-10-15: do not run the full seven-check gate locally. The workspace-wide `cargo clippy --workspace` and `cargo nextest run --workspace` take minutes on CI and anywhere from minutes to over an hour on the development Mac, because process launches and file writes stall intermittently there, most likely because of its endpoint security software (see [docs/DEVELOPMENT.md](docs/DEVELOPMENT.md#the-quality-gate-is-slow-locally)). Waiting on them locally buys nothing CI does not already give. This overrides any task in a `tasks.md` that says to run the full gate locally, including final close-out tasks. Likewise, when implementing a multi-task feature, do not run the entire workspace gate after every task unless the task explicitly requires it; use scoped checks while iterating, and the complete gate runs on CI once the feature reaches its final integrated state.

Before handing work back, run locally:

* the fast checks: `scripts/quality-gate.sh fmt node lint typos deny`;
* scoped clippy for every crate the change touches: `cargo clippy -p <crate> --all-targets --locked -- -D warnings`;
* scoped tests for the behavior the change touches: `cargo nextest run -p <crate> --test <test-file>`, or `-p <crate>` when the change is crate-wide.

Then report which local checks ran and passed, and say plainly that the workspace clippy and test suite are left to CI. Do not describe the work as having passed the quality gate until CI's **Quality** workflow is green on the commit. The user pushes; agents never push.

When CI passes, record it where the work is tracked, the way spec 58 did: "The full seven-check gate passed in CI on YYYY-MM-DD; scoped checks passed locally: …". When CI fails, the fix is ordinary work: reproduce the failing check with a scoped local command, fix it, and hand it back for another CI run.

The user may still ask for a local full run, for example to diagnose the machine. Then run it with `scripts/quality-gate.sh` and follow the next section if it is slow.

Local timings vary far more than the work does. On the developer Mac (2026-10-14), at a good moment, `cargo clippy --workspace --all-targets` into an empty target directory took about 45s and `cargo nextest run --workspace --locked` about 33s — close to CI. The same suite has also taken 8–12 minutes, and runs of an hour or more have been seen, because process launches stall intermittently (most likely the machine's endpoint security software; see below). Do not budget from either extreme, and do not treat a slow local run as a regression in the code.

### When the gate is slow

`scripts/quality-gate.sh` runs the seven checks, or the ones named, with per-step timing and a machine-state sampler, and warns once a step passes ten minutes. Prefer it to running the commands by hand.

If any local check or scoped test run takes longer than ten minutes, do not keep waiting and do not treat it as normal. Find out why first, following [docs/DEVELOPMENT.md](docs/DEVELOPMENT.md#the-quality-gate-is-slow-locally): read the step's `cpu/wall` ratio and the sampler log, check for concurrent builds from other worktrees, and when the step is waiting rather than computing, check for stalled process launches with `scripts/time-git-calls.sh`. Report what you found to the user. Security software and system settings are the user's (and their administrators') to change, not yours.

### Tools and fixes

Installing `cargo-nextest`, `typos-cli`, and `cargo-deny` (CI installs them automatically) is covered in [docs/DEVELOPMENT.md](docs/DEVELOPMENT.md#quality-checks). Agent-specific rules:

* **Web lint:** `npm run lint:web:fix` fixes many formatting issues automatically; run `npm ci` first when `node_modules` is missing. Biome is pinned through the lock file — do not update it as an unrelated side effect.
* **nextest:** `cargo nextest` is required locally. Use nextest for the workspace suite; plain `cargo test` remains acceptable when iterating on a single test.
* **typos:** prefer fixing prose rather than adding dictionary exceptions. When a term is intentionally unusual, add it to `_typos.toml` with a comment explaining why.
* **cargo-deny:** scoped to `aarch64-apple-darwin` in `deny.toml`. When adding a dependency with a new license, update the license allow-list only after verifying and documenting why that license is acceptable.

## Testing

Most tests are inline Rust `#[test]` modules, with integration tests under workspace test directories. Add tests near the behavior you change. Do not maintain a hard-coded global test count in this document; it will become stale.

### Filesystem watcher rule

A test that creates a temporary repository should normally configure `enable_index_watcher: false`. Registering the macOS filesystem watcher carries significant startup overhead and provides no benefit to a fixture that is indexed once and discarded. Reuse the existing test configuration helpers where possible. Tests whose purpose is specifically to verify watcher behavior may enable it deliberately.

### Diagnose slow tests before optimizing builds

When a suite appears slow, compare wall-clock time with CPU time before reaching for build-cache or linker optimizations, for example with `/usr/bin/time -p cargo nextest run --workspace --locked`. If `user + sys` is far below `real`, the process is mostly waiting rather than compiling or executing CPU-heavy work. Use process inspection tools to identify the blocking syscall before optimizing the wrong subsystem.

### Eval harness

Run `cargo run -p eval-harness -- run --tier deterministic` after changing behavior related to prompts, context assembly, retrieval, tool dispatch, path policy, patch behavior, recovery, or model-dependent workflows. The deterministic tier exercises the system end to end against fixture repositories. It is already covered by the normal workspace test gate, but running it directly may be useful while iterating. See [docs/DEVELOPMENT.md](docs/DEVELOPMENT.md#evaluation-harness).

### Tests with real side effects

Tests that open the user's desktop, launch Finder, spawn a real login shell, bind a real port, access the network, call a real model provider, deliberately terminate processes, or inspect the user's own checkout must be `#[ignore]` unless there is a compelling reason otherwise. Every ignored test with side effects must include a doc comment explaining the command needed to run it manually. Keep this convention.

Known examples: `serves_the_ui_for_manual_inspection` (serves the real web UI on port 4899 with a known API token), `live_tier_runs_one_scenario_against_a_real_provider` (calls a real model provider over the network), and `a_real_sigkill_mid_action_leaves_a_readable_log_and_an_unknown_outcome` (spawns a child process and `SIGKILL`s it, plus the helper test it re-executes into), the two tests in `workspace-engine/tests/semantic_search.rs` (download the embedding model from huggingface.co on first run), alongside tests that open Finder, spawn a real login shell, or read your own checkout to measure census cost.

### Mock model responses and test data

Use `DAMAIAN_MOCK_MODEL_RESPONSE="..."` for model-dependent code paths when possible; prefer this over mocking HTTP infrastructure.

Use `DAMAIAN_DATA_DIR=.damaian` during local testing where application data would otherwise be written into the user's real application-support directory. Never modify the user's real Damaian state as a side effect of automated tests.

## Security boundaries

The following are product guarantees. Do not weaken them to make implementation easier. See [SECURITY.md](SECURITY.md) for the complete security model.

* **Secret redaction.** Detected credentials must be removed from model context, command output, diffs, and other data leaving the protected boundary. Do not add a code path that bypasses secret scanning.
* **Command execution policy.** The command policy determines what may execute without approval. Read-only commands may be eligible for automatic execution; commands with side effects require explicit approval according to policy. Do not widen the allow-list or reduce a risk classification without an explicit specification change.
* **API keys.** API credentials belong in the macOS Keychain or in explicitly referenced environment variables. `model_api_key_env` may contain a Keychain reference such as `keychain:model-api-key` or an environment-variable name; it must never contain a raw API key. Never write credentials into source code, configuration files, tests, fixtures, logs, or committed documentation.
* **Patch safety.** Patch application verifies file hashes so that an approved patch cannot silently overwrite changes made after the preview was produced. Do not remove or bypass that protection.
* **Questions are not approvals.** An answer to a question never satisfies an approval, and a paused turn is resumed only by the route for what it is paused for.

### Repository configuration is untrusted input

Configuration stored inside a cloned repository is controlled by that repository. It may tighten restrictions; it must not weaken global safety controls. Repository configuration must not be allowed to override protected settings such as shell configuration, data directory, allowed roots, secret patterns, audit behavior, generated-secret blocking, model credentials or model configuration, and command allow-lists.

Keep overlay classification exhaustive. When a new configuration field is added, force the compiler or validation layer to make an explicit decision about whether repository configuration may control it. Do not add wildcard handling that silently accepts new fields.

The same principle applies to repository-supplied agent instructions: repository content may constrain an agent, but must not widen the operating mode granted by the user.

## When something is broken

Read [docs/TROUBLESHOOTING.md](docs/TROUBLESHOOTING.md) before adding instrumentation or guessing about runtime behavior. It documents configuration locations, session storage, patch storage, audit behavior, what is and is not logged, CLI reproduction paths, and application data locations. Do not assume there is a log file when the application only writes to stdout/stderr. Prefer reproducing a problem through the CLI when that isolates the engine more cleanly than the desktop UI.

## Traps

The following mistakes have already cost time.

* **Do not kill Damaian processes by name.** Never use commands such as `pkill -f damaian-desktop-shell` to clean up a process you started. The user may already be running a legitimate Damaian instance with the same process name. Track the PID of the process you started and terminate only that PID.
* **Packaged preview builds and Gatekeeper.** A development package may be ad-hoc signed rather than Developer ID signed and notarized, so Gatekeeper blocking first launch is not automatically a packaging bug. Read [docs/DEVELOPMENT.md](docs/DEVELOPMENT.md#build-a-macos-dmg) (local builds) and [docs/MACOS_INSTALLATION.md](docs/MACOS_INSTALLATION.md) (releases) before attempting to change signing behavior.
* **Vendored terminal assets.** Files matching `crates/desktop-shell/static/xterm*` are vendored and minified, and intentionally excluded from normal linting and formatting. Do not edit or reformat them.
* **Port 4765.** The desktop shell normally uses `127.0.0.1:4765`. If that port is occupied, assume the user's own Damaian instance may be running. Do not kill it and do not take over the port. Use a different port for your own process, and a separate `DAMAIAN_DATA_DIR` so your test instance does not share user data.

## Style

* **Rust:** Rust 2024 edition conventions and default `rustfmt`; format with `cargo fmt --all`. Do not introduce a custom formatting configuration without an explicit reason.
* **Web assets:** formatted and linted with Biome. Follow the repository's existing rules, including 2-space indentation, approximately 100-column lines where practical, and existing vanilla-JavaScript conventions.
* **Comments:** match surrounding code. Comments should normally explain **why** something is necessary, surprising, or constrained. Do not add comments that merely restate obvious code.
* **Scope:** keep changes focused on the current task. Do not opportunistically refactor unrelated code merely because you noticed it. Record out-of-scope findings in the appropriate planning or observation document.
* **Commit messages:** short and descriptive of the user-visible or architectural effect — prefer `Track cached input tokens in usage accounting` over descriptions of the editing process itself.

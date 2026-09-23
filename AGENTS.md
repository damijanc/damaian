# AGENTS.md

Instructions for coding agents working in this repository.

Human contributors should start with [docs/DEVELOPMENT.md](docs/DEVELOPMENT.md). This file focuses on repository rules, invariants, workflow expectations, and mistakes that are particularly easy for coding agents to make.

## What this project is

Damaian is a local-first AI coding assistant for macOS.

The repository is a Rust workspace with a native Tauri desktop application. Damaian operates on the user's own Git repository, previews proposed file edits as diffs before applying them, and keeps file access, command execution, and API credentials under the user's control.

Two constraints shape most architectural decisions:

* **macOS only.** The `desktop-app` crate builds against the macOS system webview. Do not add Linux or Windows runtime support unless explicitly requested.
* **No Node.js runtime dependency.** Node.js is used only for build, lint, release, and development tooling. Never introduce Node.js as a dependency of the shipped application.

## Workspace layout

| Crate / directory              | Purpose                                                                                                                                           |
| ------------------------------ | ------------------------------------------------------------------------------------------------------------------------------------------------- |
| `crates/workspace-engine`      | Core services: indexing, context assembly, secret redaction, patch engine, command policy, model adapters, audit log, and most application logic. |
| `crates/damaian-cli`           | Command-line interface over the workspace engine. Binary: `damaian`.                                                                              |
| `crates/desktop-shell`         | Local HTTP shell and web UI on `127.0.0.1:4765`. Binary: `damaian-desktop-shell`.                                                                 |
| `crates/desktop-app`           | Native Tauri wrapper: folder picker, Keychain integration, updater, PTY terminal. Binary: `damaian-desktop`.                                      |
| `crates/desktop-shell/static/` | Vanilla JavaScript and CSS web UI. No framework, bundler, or runtime build step.                                                                  |

## Before you change a feature

`docs/specs/` is the source of truth for implemented features.

Every spec has a `Status:` field. Before changing behavior covered by a spec:

1. read the relevant spec;
2. inspect the current implementation it references;
3. update the spec when the design or behavior changes;
4. do not change code while leaving the specification describing the old behavior.

`docs/specs/README.md` lists specs in implementation order.

Before selecting a new spec to implement, read the **What to build next** section there.

Do not infer build order solely from spec numbers. Dependencies, blockers, and concurrency constraints determine what may be implemented next.

### Keep all spec status records synchronized

A folder spec may describe its status in several places:

* the spec's own `Status:` line;
* its row in `docs/specs/README.md`;
* its `tasks.md` progress table;
* the `Started` / `Done` header in `tasks.md`.

These records must agree.

Whenever a spec changes status, update all relevant records in the same change.

A stale summary is not cosmetic. Someone reading the repository later will treat it as truth and may repeat work or make incorrect planning decisions.

### When a spec becomes Done

Finishing a spec affects documents outside the spec itself.

When marking a spec Done, perform the following in the same change.

1. Find every `Depends on:` entry that references the completed spec and update it.

   A useful search is:

   ```bash
   grep -rn "<spec-file-or-folder>" docs/specs/*.md docs/specs/*/*.md
   ```

2. Re-evaluate `docs/specs/README.md` → **What to build next**.

   Remove the completed item from the ready set and promote anything whose final unmet dependency has now been satisfied.

3. Remove the corresponding item from the `Unreleased` section of `CHANGELOG.md`.

   `Unreleased` means specified but not yet built.

   Do not manually create a release row. The release pipeline handles release rows.

4. Update `docs/USER_GUIDE.md` and `docs/TROUBLESHOOTING.md` when the completed feature changes anything visible to users or changes how failures should be understood.

These checks exist because stale summaries have repeatedly survived after the underlying specification was already correct.

The spec is not the only record readers use.

## Closing out a task in a folder spec

A folder spec's `tasks.md` tracks implementation steps.

Treat those checkboxes and progress records as durable project state, not informal notes.

While implementing a task:

* tick each checkbox when that step actually lands;
* do not wait until the end of the whole feature;
* do not leave completed work represented as incomplete.

When a task is finished, complete all of the following before starting the next task.

1. Tick every completed step in that task.

2. Update the task's row in the progress table.

   Record:

   * what actually landed;
   * relevant test results;
   * meaningful deviations from the written plan;
   * implementation discoveries that matter to later tasks.

3. Keep the task header accurate.

   Use actual dates:

   ```text
   **Started:** YYYY-MM-DD
   ```

   and when the final task completes:

   ```text
   **Started:** YYYY-MM-DD — **Done:** YYYY-MM-DD
   ```

Do not leave placeholder states such as `not yet`, `yes`, or similar once implementation has started.

The progress table is the durable record. The checkboxes describe execution progress.

Keep both truthful.

## Agent session lifecycle for multi-task specs

Treat each task in a folder spec as a separate agent session unless the task explicitly requires conversational continuity.

A feature may contain many sequential tasks, but those tasks should not normally share one ever-growing conversation.

When one task is finished:

1. update `tasks.md`;
2. record anything the next task genuinely needs;
3. leave the working tree in the correct state;
4. end the current agent session;
5. start the next task in a fresh session.

### Durable state belongs in the repository

The durable state between tasks is:

* the current source code;
* the relevant feature specification;
* `tasks.md`;
* progress-table notes;
* implementation notes;
* committed project documentation;
* the current working tree where sequential tasks intentionally build upon uncommitted work.

The previous agent conversation is **not** durable project state.

Do not carry earlier conversation history forward merely because the next task belongs to the same feature.

The next agent should inspect what actually exists in the repository rather than inherit:

* earlier reasoning;
* failed approaches;
* temporary hypotheses;
* stale diffs;
* large command outputs;
* test logs already resolved;
* tool-call history;
* discussions that no longer affect the implementation.

If later work requires information that exists only in an earlier conversation, that information should have been written into the repository.

Treat that as missing documentation.

Record the necessary fact in the relevant spec, progress row, implementation notes, or another appropriate project document instead of preserving hidden conversational state.

### What a fresh task session should read

A new task session should normally begin with only:

* this `AGENTS.md`;
* the relevant feature specification;
* the feature's global constraints;
* the current task in `tasks.md`;
* sections explicitly referenced by that task;
* the current source files required to perform the work.

Retrieve additional files only when needed.

Do not preload:

* every earlier task;
* the complete conversation history;
* the entire repository;
* complete test logs;
* unrelated specs;
* all source files mentioned anywhere in the feature.

Repository exploration should be demand-driven.

### Keep the initial task prompt small

A typical task prompt should be sufficient at roughly this level:

```text
Implement Task N from docs/specs/<feature>/tasks.md.

Read AGENTS.md, the feature's global constraints, and the sections
referenced by Task N.

Inspect the current repository state.

Implement only Task N.

Run the scoped checks required by that task.

Update the task record when finished.

Stop after the task is complete.
```

The task specification should carry the requirements.

The repository should carry the implementation state.

The conversation should carry only the temporary context needed to finish the current task.

### Keep context during one task

Do not reset the session unnecessarily while actively implementing one task.

Within a task, conversation history can be valuable for:

* debugging;
* interpreting test failures;
* remembering an implementation decision made minutes earlier;
* iterating on a failing approach;
* correlating command output with the code that produced it.

The reset boundary is normally the **task boundary**, not every tool call or every edit.

### Final integration tasks are broader

A final integration, verification, or close-out task may legitimately need broader context.

It may reread:

* the full feature specification;
* all acceptance criteria;
* the final implementation;
* progress-table entries;
* implementation notes;
* test coverage;
* quality-gate requirements.

It should still normally begin in a fresh agent session.

Its job is to evaluate the finished repository state, not inherit assumptions made during earlier implementation.

### Why task-scoped sessions are required

This policy exists for two reasons.

#### Reproducibility

A future worker should be able to understand and continue the feature from project state.

Correctness must not depend on private conversation history.

#### Context and token efficiency

Long-running coding-agent sessions accumulate:

* old reasoning;
* tool output;
* repeated file contents;
* obsolete diffs;
* resolved test failures;
* earlier tasks;
* summaries of earlier summaries.

Later model calls may repeatedly transmit large portions of that history even though the repository already contains the useful result.

Task-scoped sessions bound that growth.

Do not sacrifice necessary context merely to reduce token usage, but do not retain conversational history that has already been converted into code or documentation.

### Repository state outranks conversational memory

When the current repository disagrees with something said earlier in a conversation, inspect the repository and specification.

The current project state is authoritative.

Do not preserve an implementation merely because an earlier agent said it existed.

Verify it.

## Background product specifications

The two root documents:

```text
ai_coding_assistant_specification.md
ai_coding_assistant_must_have.md
```

describe the original product intent.

Treat them as background.

The numbered files under `docs/specs/` describe the design and behavior that was actually specified and built.

When they differ, use the current numbered specifications unless explicitly asked to revisit the original product intent.

## Planning documents

Planning information is split across different locations.

Do not put information into the wrong layer.

### [CHANGELOG.md](CHANGELOG.md)

Committed and public.

It contains:

* released versions, newest first;
* changes grouped by component;
* an `Unreleased` section for functionality that has been specified but not yet built.

Do not add release dates.

Release rows are generated by the release pipeline.

The release job runs:

```bash
npm run changelog:update
```

after a tagged release succeeds.

You may run:

```bash
npm run changelog:update -- --dry-run
```

to inspect what would be generated.

Use:

```bash
npm run changelog:check
```

to detect tags that are not yet represented.

Rows are inserted under the `<!-- releases -->` marker's separator line; do not remove that marker. A tag with no commits in its range is omitted.

Generated release rows may be edited afterward. The updater must not overwrite an existing release row.

Moving an implemented feature out of `Unreleased` remains part of finishing that feature.

### `docs/PLAN/`

Local-only planning state.

It is intentionally not committed.

Do not link to files under `docs/PLAN/` from committed documentation.

It may contain:

* phases;
* work packages;
* execution dashboards;
* `OBSERVATIONS.md`;
* deferred decisions;
* local planning context.

### `docs/specs/`

Committed project truth.

A work package becomes a numbered specification when the design has become concrete enough to implement.

Where relevant, specs carry a `Plan:` line identifying their planning origin.

## Picking up a piece of work

Before writing code for a specification, follow this order.

1. Read the specification.

   Check:

   * its `Status:`;
   * Current State;
   * relevant file references;
   * Implementation Notes;
   * dependencies;
   * acceptance criteria.

2. If available, inspect `docs/PLAN/README.md`.

   Confirm work-package state, blockers, priority, and dependencies.

3. If available, read `docs/PLAN/OBSERVATIONS.md`.

   Look for known problems touching the same part of the system.

4. When you discover something outside the current task's scope, record it in `OBSERVATIONS.md` rather than silently expanding the task.

   Follow that file's own rules for evidence, falsifiability, and disposition.

If `docs/PLAN/` is missing, assume the checkout simply does not contain local planning state.

Do not recreate it from memory.

Do not infer that no planning context exists.

Say that it is unavailable if it is required.

`docs/specs/` and `CHANGELOG.md` are sufficient for understanding committed product state, but they do not necessarily describe ideas that were explicitly rejected, deferred, or parked.

## Quality gate

CI runs the following checks.

All seven must pass before work is claimed complete.

```bash
cargo fmt --all -- --check
cargo clippy --workspace --all-targets --locked -- -D warnings
cargo nextest run --workspace --locked
node --check crates/desktop-shell/static/app.js
npm run lint:web
typos
cargo deny check
```

An additional advisory check is:

```bash
npm run specs:check
```

It reports mismatches between dependency declarations and specification statuses.

It intentionally does not fail by default.

To make it blocking locally:

```bash
npm run specs:check -- --strict
```

Read its output when finishing a specification.

### Clippy

Clippy warnings are errors.

Fix warnings instead of suppressing them.

When a lint is genuinely inappropriate, a targeted `#[allow(...)]` with a comment explaining why is acceptable.

Do not add broad suppression merely to make the gate pass.

### Per-task checks versus the full gate

When implementing a multi-task feature, do not run the entire workspace gate after every task unless the task explicitly requires it.

Use scoped checks while iterating.

Examples:

```bash
cargo nextest run -p <crate> --test <test-file>
```

and:

```bash
cargo clippy -p <crate> --all-targets --locked -- -D warnings
```

Run the complete quality gate once the feature reaches its final integrated state.

This changes **when** the complete gate runs, not whether it runs.

Nothing is complete until all seven checks pass.

Measured on a developer Mac (2026-09-18): `cargo clippy --workspace --all-targets` cold takes about 18m20s, `cargo clippy -p workspace-engine --all-targets` warm takes about 75s, and `cargo nextest run --workspace --locked` takes about 5 minutes, mostly idle on the checkpoint store's `git` subprocesses. A multi-task plan that runs the unscoped pair after every task burns hours waiting on nothing.

Do not read that ~5 minute local nextest figure as a regression against the 28s in [docs/specs/09_release_quality_gate.md](docs/specs/09_release_quality_gate.md) §7 — that number is CI-runner only and has never described a developer machine.

### Web linting

If web assets fail lint:

```bash
npm run lint:web:fix
```

can fix many formatting issues automatically.

Run:

```bash
npm ci
```

first when `node_modules` is missing.

Biome is pinned through the lock file.

Do not update it as an unrelated side effect.

### cargo-nextest

`cargo nextest` is required locally.

Install it with:

```bash
cargo install cargo-nextest --locked
```

CI installs it automatically.

Use nextest for the workspace suite.

Plain `cargo test` remains acceptable when iterating on a single test.

### typos

Install with:

```bash
cargo install typos-cli
```

Prefer fixing prose rather than adding dictionary exceptions.

When a term is intentionally unusual, add it to `_typos.toml` with a comment explaining why.

### cargo-deny

`cargo deny` is scoped to `aarch64-apple-darwin` in `deny.toml`.

When adding a dependency with a new license, update the license allow-list only after verifying and documenting why that license is acceptable.

## Testing

Most tests are inline Rust `#[test]` modules, with integration tests under workspace test directories.

Add tests near the behavior you change.

Do not maintain a hard-coded global test count in this document. It will become stale.

### Filesystem watcher rule

A test that creates a temporary repository should normally configure:

```rust
enable_index_watcher: false
```

Registering the macOS filesystem watcher carries significant startup overhead and provides no benefit to a fixture that is indexed once and discarded.

Reuse the existing test configuration helpers where possible.

Tests whose purpose is specifically to verify watcher behavior may enable it deliberately.

### Diagnose slow tests before optimizing builds

When a suite appears slow, compare wall-clock time with CPU time before reaching for build-cache or linker optimizations.

For example:

```bash
/usr/bin/time -p cargo nextest run --workspace --locked
```

If `user + sys` is far below `real`, the process is mostly waiting rather than compiling or executing CPU-heavy work.

Use process inspection tools to identify the blocking syscall before optimizing the wrong subsystem.

### Eval harness

Run:

```bash
cargo run -p eval-harness -- run --tier deterministic
```

after changing behavior related to:

* prompts;
* context assembly;
* retrieval;
* tool dispatch;
* path policy;
* patch behavior;
* recovery;
* model-dependent workflows.

The deterministic evaluation tier exercises the system end to end against fixture repositories.

It is already covered by the normal workspace test gate, but running it directly may be useful while iterating. See [docs/DEVELOPMENT.md](docs/DEVELOPMENT.md#evaluation-harness).

### Tests with real side effects

Tests that:

* open the user's desktop;
* launch Finder;
* spawn a real login shell;
* bind a real port;
* access the network;
* call a real model provider;
* deliberately terminate processes;
* inspect the user's own checkout;

must be `#[ignore]` unless there is a compelling reason otherwise.

Known examples: `serves_the_ui_for_manual_inspection` (serves the real web UI on port 4899 with a known API token), `live_tier_runs_one_scenario_against_a_real_provider` (calls a real model provider over the network), and `a_real_sigkill_mid_action_leaves_a_readable_log_and_an_unknown_outcome` (spawns a child process and `SIGKILL`s it, plus the helper test it re-executes into), alongside tests that open Finder, spawn a real login shell, or read your own checkout to measure census cost.

Every ignored test with side effects must include a doc comment explaining the command needed to run it manually.

Keep this convention.

### Mock model responses

Use:

```bash
DAMAIAN_MOCK_MODEL_RESPONSE="..."
```

for model-dependent code paths when possible.

Prefer this over mocking HTTP infrastructure.

### Test data directory

Use:

```bash
DAMAIAN_DATA_DIR=.damaian
```

during local testing where application data would otherwise be written into the user's real application-support directory.

Never modify the user's real Damaian state as a side effect of automated tests.

## Security boundaries

The following are product guarantees.

Do not weaken them to make implementation easier.

### Secret redaction

Detected credentials must be removed from:

* model context;
* command output;
* diffs;
* other data leaving the protected boundary.

Do not add a code path that bypasses secret scanning.

### Command execution policy

The command policy determines what may execute without approval.

Read-only commands may be eligible for automatic execution.

Commands with side effects require explicit approval according to policy.

Do not widen the allow-list or reduce a risk classification without an explicit specification change.

### API keys

API credentials belong in the macOS Keychain or in explicitly referenced environment variables.

`model_api_key_env` may contain:

* a Keychain reference such as `keychain:model-api-key`;
* an environment-variable name.

It must never contain a raw API key.

Never write credentials into:

* source code;
* configuration files;
* tests;
* fixtures;
* logs;
* committed documentation.

### Patch safety

Patch application verifies file hashes so that an approved patch cannot silently overwrite changes made after the preview was produced.

Do not remove or bypass that protection.

### Repository configuration is untrusted input

Configuration stored inside a cloned repository is controlled by that repository.

It may tighten restrictions.

It must not weaken global safety controls.

Repository configuration must not be allowed to override protected settings such as:

* shell configuration;
* data directory;
* allowed roots;
* secret patterns;
* audit behavior;
* generated-secret blocking;
* model credentials or model configuration;
* command allow-lists.

Keep overlay classification exhaustive.

When a new configuration field is added, force the compiler or validation layer to make an explicit decision about whether repository configuration may control it.

Do not add wildcard handling that silently accepts new fields.

The same principle applies to repository-supplied agent instructions: repository content may constrain an agent, but must not widen the operating mode granted by the user.

See [SECURITY.md](SECURITY.md) for the complete security model.

## When something is broken

Read [docs/TROUBLESHOOTING.md](docs/TROUBLESHOOTING.md) before adding instrumentation or guessing about runtime behavior.

It documents:

* configuration locations;
* session storage;
* patch storage;
* audit behavior;
* what is and is not logged;
* CLI reproduction paths;
* application data locations.

Do not assume there is a log file when the application only writes to stdout/stderr.

Prefer reproducing a problem through the CLI when that isolates the engine more cleanly than the desktop UI.

## Traps

The following mistakes have already cost time.

### Do not kill Damaian processes by name

Never use commands such as:

```bash
pkill -f damaian-desktop-shell
```

to clean up a process you started.

The user may already be running a legitimate Damaian instance with the same process name.

Track the PID of the process you started and terminate only that PID.

### Packaged preview builds and Gatekeeper

A development package may be ad-hoc signed rather than Developer ID signed and notarized.

Gatekeeper blocking first launch is therefore not automatically a packaging bug.

Read [docs/MACOS_INSTALLATION.md](docs/MACOS_INSTALLATION.md) before attempting to change signing behavior.

### Vendored terminal assets

Files matching:

```text
crates/desktop-shell/static/xterm*
```

are vendored and minified.

They are intentionally excluded from normal linting and formatting.

Do not edit or reformat them.

### Port 4765

The desktop shell normally uses:

```text
127.0.0.1:4765
```

If that port is occupied, assume the user's own Damaian instance may be running.

Do not kill it and do not take over the port.

Use a different port for your own process.

Also use a separate `DAMAIAN_DATA_DIR` so your test instance does not share user data.

## Style

### Rust

Use Rust 2024 edition conventions and default `rustfmt`.

Format with:

```bash
cargo fmt --all
```

Do not introduce a custom formatting configuration without an explicit reason.

### Web assets

Web code is formatted and linted with Biome.

Follow the repository's existing rules, including:

* 2-space indentation;
* approximately 100-column lines where practical;
* existing vanilla-JavaScript conventions.

### Comments

Match surrounding code.

Comments should normally explain **why** something is necessary, surprising, or constrained.

Do not add comments that merely restate obvious code.

### Scope

Keep changes focused on the current task.

Do not opportunistically refactor unrelated code merely because you noticed it.

Record out-of-scope findings in the appropriate planning or observation document.

### Commit messages

Keep commit messages short and descriptive of the user-visible or architectural effect.

Prefer:

```text
Track cached input tokens in usage accounting
```

over descriptions of the editing process itself.

## General agent principle

The repository must remain understandable without access to the conversation that produced it.

Code carries implementation state.

Specifications carry design intent.

Tests carry executable expectations.

Progress records carry implementation history that matters to future work.

Agent conversations are temporary working memory.

Anything important enough for a future task to depend on must graduate from conversation into one of those durable forms.

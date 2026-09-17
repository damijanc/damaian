# AGENTS.md

Instructions for coding agents working in this repository. Human contributors
should read [docs/DEVELOPMENT.md](docs/DEVELOPMENT.md) first; this file covers
the parts that are easy for an agent to get wrong.

## What this project is

Damaian is a local-first AI coding assistant for macOS: a Rust workspace with a
native Tauri desktop app. It operates on the user's own Git repository, previews
every file edit as a diff before applying it, and keeps file access, command
execution, and API keys under the user's control.

Two constraints follow from that and shape most decisions:

- **macOS only.** The `desktop-app` crate builds against the macOS system
  webview. Do not add Linux or Windows support paths unless asked.
- **No Node.js at runtime.** Node is used only for build, release, and lint
  scripts. Never introduce a Node dependency into the shipped application.

## Workspace layout

| Crate | Purpose |
|-------|---------|
| `crates/workspace-engine` | Core services: indexing, context assembly, secret redaction, patch engine, command policy, model adapters, audit log. Most logic lives here. |
| `crates/damaian-cli` | Command-line front end over the workspace engine (binary: `damaian`). |
| `crates/desktop-shell` | Local HTTP shell and web UI on `127.0.0.1:4765` (binary: `damaian-desktop-shell`). |
| `crates/desktop-app` | Native Tauri wrapper: folder picker, Keychain, updater, PTY terminal (binary: `damaian-desktop`). |

The web UI is vanilla JavaScript and CSS in `crates/desktop-shell/static/` —
no framework, no bundler, no build step.

## Before you change a feature

`docs/specs/` is the source of truth for the implemented features, and each
spec carries a `Status:` line. Read the relevant spec before changing behaviour
it covers, and update the spec when you change the design — not just the code.
`docs/specs/README.md` lists them in implementation order.

**A spec is summarised in four places, and they move together.** Its own
`Status:` line, its row in `docs/specs/README.md`, and — for a folder spec — the
progress table in its `tasks.md` and that file's `**Started:** … — **Done:** …`
header. When you change what a spec says or what is true of it, update all four
in the same change. Leaving one behind is the easiest way to make this directory
lie.

The reason is measured, not theoretical. On 2026-09-15 three such summaries were
found asserting the **opposite** of what their own specs recorded: the index
still said the eval harness's live tier was "not yet verified against a real
provider" and that "no provider has been tested for usage reporting", five days
after both had been done and written up in those specs' §7. The proposals were
correct the whole time; only the summaries around them were not. The index is
where a reader starts, so a stale row is not a cosmetic problem — it is work
someone repeats.

### Closing out a task in a folder spec

A folder spec's `tasks.md` tracks each task as a list of `- [ ]` steps. **Tick
the box the moment the step lands, not at the end of the task**, and when the
task is finished do all three of these before moving to the next one:

1. **Tick every remaining box in that task.** An unticked box is a claim that the
   step was never done, and the next person will redo it or, worse, trust it.
2. **Fill in the task's row in the Progress table** — what actually landed, the
   test count, and any deviation from the plan. That row is the record; the
   checkboxes only say how far the work got.
3. **Set the header.** `**Started:** YYYY-MM-DD` when you begin the first task,
   and `— **Done:** YYYY-MM-DD` when the last one lands. Never leave it as
   `not yet` or `yes` once work has started: a date is checkable and a word is
   not.

This one is measured too. On 2026-09-17 a survey of all 56 specs found **446
unticked boxes across nine plans that were fully Done** — spec 46 had all 73
unticked on the day it shipped — and four headers still reading `not yet` or
`yes`, one of them for work finished ten days earlier. Every Progress table
was accurate, so the checkboxes were a second tracker saying the opposite of
the first. They were ticked retroactively from the Progress tables, which is
reconstruction, not a record: do it as you go instead.

The two root documents `ai_coding_assistant_specification.md` and
`ai_coding_assistant_must_have.md` are the original product spec. Treat them as
background: they describe intent, and the `docs/specs/` files describe what was
actually built.

Planning is split across three documents, and writing in the wrong one is the
easy mistake:

- [`CHANGELOG.md`](CHANGELOG.md) — committed, public. Every release newest
  first as a two-column table — version on the left, what changed on the right,
  grouped by component (Engine / App / CLI / Evaluation / Release) — plus an
  `Unreleased` table listing what is specified but not built. No dates anywhere:
  a released version needs no date to be findable, and a specification is a
  decision rather than a schedule. It replaced a themed roadmap document on
  2026-09-15, because the changelog says what actually happened and what is
  written down — the same information without the guesswork.

  **The release pipeline writes the release row.** `macos-dmg.yml`'s
  `update-changelog` job runs `npm run changelog:update` once a tagged release
  has published, and commits the result to `main`. Run the command yourself only
  to catch up a tag pushed before that job existed, or with `--dry-run` to see
  what it would add; `npm run changelog:check` exits non-zero when a tag is
  undocumented and is a local command, called by no workflow. It adds a row for
  every tag the file does not yet document, from the commit subjects in that
  tag's range, and it never rewrites an existing row — so edit a generated row
  freely, and re-running leaves it alone. Rows are inserted under the
  `<!-- releases -->` marker's separator line; do not remove that marker. A tag
  with no commits in its range is omitted. **Moving an entry out of
  `Unreleased` when it ships is still yours** — nothing automates that
  judgement, so a release can leave `Unreleased` naming work that already
  shipped.
- `docs/PLAN/` — **local-only and not committed**, so never link to it from a
  committed file; name it instead. Phases, work packages, the execution
  dashboard, and `OBSERVATIONS.md`, the inbox for things noticed but not yet
  decided.
- `docs/specs/` — committed. What was decided and built. A work package
  graduates into a numbered spec carrying a `Plan:` line naming its phase.

### Picking up a piece of work

Before writing code for a spec, in this order:

1. **Read the spec.** Its `Status:` line says whether it is built. Its §2
   Current State names the files, and its §7 Implementation Notes may already
   record what the last person learned.
2. **Check the plan's dashboard** (`docs/PLAN/README.md`) for that work
   package's status, priority and dependencies. A spec can exist for work that
   is blocked on something unspecified.
3. **Read `docs/PLAN/OBSERVATIONS.md` for open entries touching the same
   area.** This is the step most easily skipped and the one that wastes the most
   time. Entries there are known, evidenced problems that have deliberately not
   been decided yet, and one of them may change what you should build — or tell
   you that the thing you are about to measure is already known to be blocked by
   something else. Skipping it means re-deriving a finding someone already paid
   for.
4. **Add an entry yourself** for anything you notice that is out of scope,
   rather than widening the change or letting it evaporate when the session
   ends. The file states its own rules: a falsifiable one-sentence claim,
   evidence a reader can check, and a disposition. There is no quality bar for
   adding one.

**If `docs/PLAN/` is not present, you are working from a clone that does not
have it** — the directory is deliberately not committed. Do not conclude there
is no planning context and do not reconstruct it: say it is missing, and ask.
`docs/specs/` and `CHANGELOG.md` are committed and are enough to understand what
exists, but not what was decided against, deferred, or noticed and parked.

## Quality gate

CI (`.github/workflows/quality.yml`) runs exactly these. Run them before you
claim work is done — all seven must pass:

```sh
cargo fmt --all -- --check
cargo clippy --workspace --all-targets --locked -- -D warnings
cargo nextest run --workspace --locked
node --check crates/desktop-shell/static/app.js
npm run lint:web
typos
cargo deny check
```

Notes:

- **Clippy warnings are errors.** Fix them rather than suppressing them. Where a
  lint is genuinely wrong for the code, `#[allow(...)]` with a comment
  explaining why is acceptable — see the `too_many_arguments` allowances on the
  dependency-injection constructors.
- `npm run lint:web:fix` auto-fixes most web-asset findings.
- `cargo nextest` needs `cargo install cargo-nextest --locked`; CI installs it
  with `taiki-e/install-action`. It replaced `cargo test` because it runs test
  binaries concurrently instead of one after another, and reports the slowest
  tests so a suite cannot quietly rot into a forty-five-minute one again
  (`--final-status-level slow`). Nothing is lost in the swap: nextest does not
  run doctests, and this workspace has none. Plain `cargo test` still works for
  a single test while you iterate.
- `typos` needs `cargo install typos-cli`; CI pins the `crate-ci/typos` action
  to the same version. Prefer fixing the prose. Where the word is deliberate,
  add it to `_typos.toml` with a comment saying why — as `mis` is, since the
  hyphenated `mis-` prefix reads as a bare `mis` once typos splits on the
  hyphen.
- `npm ci` first if `node_modules` is missing. Biome is pinned in
  `package-lock.json`; do not bump it as a side effect of another change.
- `cargo deny` is scoped to `aarch64-apple-darwin` in `deny.toml`. If you add a
  dependency with a new license, add it to the allow-list there with a reason.

## Testing

- All tests pass by default: mostly inline `#[test]` modules, plus integration
  tests in `crates/workspace-engine/tests/` and `crates/eval-harness/tests/`.
  Add tests next to the code you change. (No count here on purpose — a number in
  this file goes stale on the next commit that adds a test, and a stale one is
  worse than none: it invites "close enough" when the real total differs.)
- **A test that builds a throwaway repository must set
  `enable_index_watcher: false` on its `Config`.** Registering the filesystem
  watcher is a synchronous handshake with macOS `fseventsd` that costs ten to
  fifteen seconds per repository — not work this project controls — and a test
  that indexes a fixture once and discards it buys nothing with it. Copy
  `foundation`'s `test_config` or the eval harness's `base_config`. Omitting it
  is what made this suite take forty-five minutes on 2026-09-16: `foundation`
  and the eval harness held 29 of those minutes between them, at 92% idle.
  `index_cache_picks_up_file_changes_via_watcher_without_full_rescan` is the
  one test that needs a watcher, and turns it back on deliberately.
- **If the suite feels slow, measure wall time against CPU time before reaching
  for build tooling.** `/usr/bin/time -p cargo nextest run --workspace --locked`
  settles it: when `user + sys` is a small fraction of `real`, the suite is
  waiting on something, and no amount of `sccache`, `mold` or cache tuning will
  move it. `sample <pid>` on a stalled test names the blocking syscall. That is
  how the `fseventsd` cost above was found, after a build-caching guide had sent
  the search in the wrong direction.
- `cargo run -p eval-harness -- run --tier deterministic` evaluates Damaian end
  to end against fixture repositories — scenarios covering retrieval, patch
  proposal, restricted paths, secret redaction, approval denial, the retry bound
  and crash recovery. Run it after changing prompt, context-assembly,
  tool-dispatch, path-policy or recovery code: a regression there passes the
  unit tests and fails here. Its deterministic tier is already part of
  `cargo nextest run --workspace --locked`, so it adds no quality-gate
  command. See
  [`docs/DEVELOPMENT.md`](docs/DEVELOPMENT.md#evaluation-harness).
- Some tests are `#[ignore]`d because they have real side effects: one opens
  Finder, one spawns a real login shell, one reads your own checkout to measure
  census cost, `serves_the_ui_for_manual_inspection` serves the real web UI on
  port 4899 with a known API token so the desktop UI can be looked at without a
  Tauri build (its doc comment has the two console lines you need),
  `live_tier_runs_one_scenario_against_a_real_provider` calls a real provider
  over the network and needs credentials, and
  `a_real_sigkill_mid_action_leaves_a_readable_log_and_an_unknown_outcome`
  spawns a child process and `SIGKILL`s it, together with the helper test it
  re-executes into. Every one carries its manual command in its doc comment.
  Keep that convention: anything that touches the user's desktop, spawns a
  shell or a process, binds a port, reaches the network, or reads their
  repository should be `#[ignore]`d with a doc comment saying how to run it
  manually.
- `DAMAIAN_MOCK_MODEL_RESPONSE="..."` makes model-dependent paths testable with
  no API key and no network. Prefer it over mocking HTTP.
- `DAMAIAN_DATA_DIR=.damaian` keeps app data inside the workspace instead of
  `~/Library/Application Support/DamaianClient`. Use it so you never write to
  the user's real data while testing.

## Security boundaries

These are product guarantees, not implementation details. Do not weaken them to
make something easier:

- **Secret redaction** removes detected credentials from model context, command
  output, and diffs. Never add a path that bypasses the scanner.
- **Command policy** decides what runs without approval. Read-only commands may
  run automatically; anything else requires explicit user approval. Do not widen
  the allowlist or downgrade a risk classification without a spec change.
- **API keys** live in the macOS Keychain. `model_api_key_env` holds a Keychain
  reference (`keychain:model-api-key`) or an environment variable name — never a
  raw key. Never write a raw key into config, code, tests, or logs.
- **Patch application** verifies file hashes so it cannot overwrite work the user
  changed after the preview was generated. Keep that check.
- **Repository config is untrusted input.** `<repo>/.damaian/config.conf` arrives
  with a clone, so it may add restrictions and never remove one. A repository
  cannot set `shell`, `data_dir`, `allowed_roots`, `secret_patterns`,
  `audit_enabled`, `block_generated_secrets`, any `model_*` key, or
  `command_allowlist`. `Config::apply_overlay_scoped` classifies every overlay
  field in one exhaustive destructuring, so a field added to `ConfigOverlay`
  without being classified fails to compile — keep it that way rather than
  adding a catch-all. The same rule already applies to `AGENTS.md`, which
  cannot widen a working mode.

See [SECURITY.md](SECURITY.md) for the full model.

## When something is broken

[docs/TROUBLESHOOTING.md](docs/TROUBLESHOOTING.md) is the diagnostic reference:
where config, sessions, patches, and the audit log are written, what is and is
not logged (there is no log file — only stdout/stderr), how to read the audit
trail, and how to reproduce a failure with the CLI instead of the UI. Read it
before you start instrumenting code to find out what happened.

## Traps

Things that have actually cost time here:

- **Never `pkill -f damaian-desktop-shell`** (or match any Damaian binary by
  name) to clean up a process you started. The user's own running app shares
  those names, and you will kill their session. Track the PID you spawned and
  kill that.
- **A packaged build refusing to launch is expected, not a bug.** The developer
  preview is ad-hoc signed, not Developer ID signed or notarized, so macOS
  Gatekeeper blocks first launch. The workaround is in
  [docs/MACOS_INSTALLATION.md](docs/MACOS_INSTALLATION.md). Do not "fix" it.
- **`crates/desktop-shell/static/xterm*` is vendored and minified.** It is
  excluded from linting and formatting. Never edit or reformat it.
- The desktop shell defaults to port `4765`. If that port is already in use, the
  user's app is probably running — do not assume it is stale and do not take the
  port. Pass `--port` to run your own instance somewhere else, and set
  `DAMAIAN_DATA_DIR` so it does not share their data.

## Style

- Rust 2024 edition, default `rustfmt`. No custom formatting config, so just run
  `cargo fmt --all`.
- Web assets are formatted and linted by Biome (`biome.json`): 2-space indent,
  100-column lines.
- Match the surrounding code. Comments in this codebase explain *why* something
  is done, not what the line does — follow that.
- Keep commit messages short and descriptive of the user-visible effect.

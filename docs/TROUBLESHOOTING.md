# Troubleshooting

This is the full reference for diagnosing problems with Damaian, for users and
developers alike: where state is written, what is logged and what is not, what
each failure means, and how to reproduce one outside the UI. The
[User Guide](USER_GUIDE.md) explains how to use the app; building, testing and
releasing it, including a slow local quality gate, are in
[Development](DEVELOPMENT.md); repository conventions for contributors and
coding agents are in [AGENTS.md](../AGENTS.md).

## Contents

- [Symptom index](#symptom-index)
- [Conventions](#conventions)
- [Triage order](#triage-order)
- [Process model](#process-model)
- [Where state lives](#where-state-lives)
- [Logs](#logs)
- [Turns, recovery and workers](#turns-recovery-and-workers)
- [Inspecting configuration](#inspecting-configuration)
- [Reproducing outside the UI](#reproducing-outside-the-ui)
- [Common failures](#common-failures)
- [Resetting state](#resetting-state)
- [What to collect for a bug report](#what-to-collect-for-a-bug-report)

## Symptom index

| Symptom | Section |
|---------|---------|
| The app refuses to start: port 4765 is in use | [Port 4765](#port-4765) |
| The data directory is refused at startup | [The data directory is refused at startup](#the-data-directory-is-refused-at-startup) |
| macOS blocks the first launch | [The packaged app will not launch](#the-packaged-app-will-not-launch) |
| Something works from Terminal but not from the app (a key, a command, an MCP server) | [The app does not see your shell's environment](#the-app-does-not-see-your-shells-environment) |
| Model calls fail | [Model calls fail](#model-calls-fail) |
| The provider refused a call (rate limit, quota, auth) | [Provider refusal classifications](#provider-refusal-classifications) |
| A refusal offered no switch to another provider | [A refusal offered no switch to my other provider](#a-refusal-offered-no-switch-to-my-other-provider) |
| A turn stopped early, or asks *Answer now* / *Continue* | [A turn stopped early](#a-turn-stopped-early-which-budget-ran-out) |
| A crash-recovery card, or a task marked unknown outcome | [After a crash: what recovery decided](#after-a-crash-what-recovery-decided) |
| Processes left over after a crash | [After a crash: what was still running](#after-a-crash-what-was-still-running) |
| A worker won't start | [A worker will not start](#a-worker-will-not-start) |
| A worker shows *Interrupted* or *Ended* | [A worker shows Interrupted, or Ended](#a-worker-shows-interrupted-or-ended) |
| The worker board stops updating | [The board or the control strip stops updating](#the-board-or-the-control-strip-stops-updating) |
| No *Start worker* button on handoff prompts | [An answer has handoff prompts but no Start worker](#an-answer-has-handoff-prompts-but-no-start-worker) |
| Worktrees left over after a crash | [Leftover worktrees after a crash](#leftover-worktrees-after-a-crash) |
| Rewind did not restore a file, or reports a conflict | [Checkpoints and rewind](#checkpoints-and-rewind) |
| The assistant replies but never proposes a patch | [The assistant replies but never proposes a patch](#the-assistant-replies-but-never-proposes-a-patch) |
| Applying a patch fails with a conflict | [Patch apply fails with a conflict](#patch-apply-fails-with-a-conflict) |
| An `edit_file` proposal is refused | [An `edit_file` proposal is refused](#an-edit_file-proposal-is-refused) |
| `Refused: …` from a mode, a profile or the command policy | [The assistant was refused](#the-assistant-was-refused-the-session-mode-the-permission-profile-or-the-command-policy) |
| `Refused by hook …`, or a hook blocked, warned, timed out or did not run | [A hook refused, blocked, or did not run](#a-hook-refused-blocked-or-did-not-run) |
| A command that used to run unasked now asks | [A command that used to run unasked now asks](#a-command-that-used-to-run-unasked-now-asks) |
| A setting will not stick, or a repository config key has no effect | [Configuration layers](#configuration-layers), [Why a rule is in force](#why-a-rule-is-in-force) |
| Memory will not save, a project's memory is missing (including a shallow clone), or the store was refused | [Memory](#memory) |
| A permission profile key has no effect | [A permission profile key had no effect](#a-permission-profile-key-had-no-effect) |
| A file read or search result was cut short | [A file read or search result was truncated](#a-file-read-or-search-result-was-truncated) |
| A page the agent tried to read was not fetched, or a fetch approval did not appear after a reload | [A fetch was refused](#a-fetch-was-refused) |
| A file is missing from the model's context | [A file the model should see is missing from context](#a-file-the-model-should-see-is-missing-from-context) |
| Why a file was not consulted, or a pin or restriction is not taking effect | [The context inspector: why a file was not consulted](#the-context-inspector-why-a-file-was-not-consulted) |
| Why a file or search hit was excluded, or what a turn's context actually held | [Reading a turn's context manifest](#reading-a-turns-context-manifest) |
| A `worktree` argument is refused | [A `worktree` argument is refused](#a-worktree-argument-is-refused) |
| `list_worktrees` shows none from a worktree session | [`list_worktrees` shows no worktrees from a worktree session](#list_worktrees-shows-no-worktrees-from-a-worktree-session) |
| A project root is missing or wrong, or a command ran in the wrong directory | [Repository roots and the repository map](#repository-roots-and-the-repository-map) |
| Symbol search shows only `likely` results, or a configured language server will not start | [Symbol search shows only likely results, or a language server will not start](#symbol-search-shows-only-likely-results-or-a-language-server-will-not-start) |
| A language-server diagnostic never appears | [Symbol search shows only likely results, or a language server will not start](#symbol-search-shows-only-likely-results-or-a-language-server-will-not-start) |
| Findings are missing, stale, or cannot be opened | [Findings](#findings) |
| A browser diagnostic looks wrong | [Browser diagnostics](#browser-diagnostics) |
| Command output looks cut short | [Command output](#command-output) |
| MCP server not working | [MCP server not working](#mcp-server-not-working) |
| A Docker command fails | [Docker command problems](#docker-command-problems) |
| Semantic search does nothing | [Semantic search does nothing](#semantic-search-does-nothing) |
| The terminal panel fails to start | [Terminal panel problems](#terminal-panel-problems) |
| Token figures always read "estimated", or are missing | [Token figures are always "estimated"](#token-figures-are-always-estimated), [A turn shows no token line at all](#a-turn-shows-no-token-line-at-all) |
| Cache hit rate reads "not reported", or seems low | [Cache hit rate](#cache-hit-rate-reads-not-reported-or-seems-low) |
| The app forgot my projects | [Web UI state (localStorage)](#web-ui-state-localstorage) |
| `curl` against `/api/*` is refused | [The API token blocks curl](#the-api-token-blocks-curl) |
| A `Desktop API unavailable` banner | [Debugging the web UI](#debugging-the-web-ui) |
| The quality gate is slow locally (contributors) | [Development](DEVELOPMENT.md#the-quality-gate-is-slow-locally) |

## Conventions

The recipes below use these shell variables. Set the ones a recipe needs
first:

```bash
# The data directory: $DAMAIAN_DATA_DIR if you set it, otherwise the default.
DATA_DIR="${DAMAIAN_DATA_DIR:-$HOME/Library/Application Support/DamaianClient}"

# One conversation's session log; this picks the most recently written one.
SESSION_FILE=$(ls -t "$DATA_DIR"/sessions/*.jsonl | grep -v '\.progress\.jsonl$' | head -1)

# The repository you are working in. CLI examples write it as /path/to/repo.
REPO=/path/to/repo
```

- `REPO_ID` — the repository's id, the directory name under
  `$DATA_DIR/checkpoints/` and `$DATA_DIR/repository-map/` and the
  `repositoryId` field on audit events. `ls "$DATA_DIR/checkpoints"` lists the
  repositories that have checkpoints.
- `CHECKPOINT_ID` — one checkpoint, named by the files under
  `$DATA_DIR/checkpoints/$REPO_ID/manifests/` and by the `checkpointId` field on
  `checkpoint_created` audit events.
- `TOKEN` — the API token for `curl`. Only the manual-inspection server accepts
  one from outside the app, and its token is fixed:
  `TOKEN=damaian-ui-inspection-token` (see
  [The API token blocks curl](#the-api-token-blocks-curl)).

Audit events are flat objects, and **every field value is a string**, numbers
and booleans included (`AuditLog::record` in
[audit.rs](../crates/workspace-engine/src/audit.rs)). Compare with
`"true"`, not `true`, and convert with `tonumber` before arithmetic, as the
timeline recipe in [Audit log](#audit-log) does with `timestampMs`.

## Triage order

Work top to bottom. Each step is cheap and rules out a whole class of causes.

1. **Reproduce with the process attached to a terminal.** The app has no log
   file — see [Logs](#logs). If you cannot see stderr, you are guessing.
2. **Read the audit log tail.** `$DATA_DIR/audit/events.jsonl`
   records every consequential action with a timestamp. It tells you how far the
   turn got before it failed.
3. **Read the session log** for the affected conversation. It holds the actual
   prompts, replies, and task status transitions.
4. **Dump the effective config.** Three files overlay each other; the value you
   are looking at in one may be overridden by another. Use `config-show`.
5. **Reproduce with the CLI.** `damaian-cli` drives the same engine without the
   HTTP shell, the webview, or the Tauri wrapper. If the CLI reproduces it, the
   bug is in `workspace-engine`. If it does not, the bug is in the shell or UI.

## Process model

Four kinds of process can be involved, and knowing which one owns a symptom
saves most of the time. In the packaged app the first two are one process: the
shell runs as a thread (`damaian-shell`) inside `damaian-desktop`, so a shell
panic or a shell message shows up under `damaian-desktop`.

| Process | Binary | Role |
|---------|--------|------|
| Native wrapper | `damaian-desktop` | Tauri app: window, menus, folder picker, Keychain, updater, PTY terminal. Spawns the shell in a background thread. |
| Local shell | `damaian-desktop-shell` standalone; a thread inside `damaian-desktop` in the app | HTTP server on `127.0.0.1:4765` serving the web UI and the `/api/*` surface. |
| CLI | `damaian` | Same workspace engine, no HTTP, no UI. |
| Worker | `damaian-desktop --worker` (or `damaian-desktop-shell --worker` in development) | One per running worker session ([spec 57](specs/57_worker_sessions/proposal.md)): the app's own binary re-launched by the shell, with no window and no HTTP server. See [Worker sessions](#worker-sessions). |

The webview loads `http://127.0.0.1:4765/` — the UI is a normal web page talking
to a local HTTP server, so browser-side debugging applies (see
[Debugging the web UI](#debugging-the-web-ui)).

> **Never `pkill -f damaian-desktop-shell`** or match any Damaian binary by
> name. The user's own running app shares those names. Track the PID you
> spawned and kill that one.

Each of them spawns children of its own — MCP stdio servers, `curl` model
calls, PTY shells, shell commands, hook executables, worker processes and
language servers — and records them under `processes/` so a crash cannot leak
them. That registry is the mechanical form of the rule above:
it kills by PID and start time, never by name. See
[After a crash: what was still running](#after-a-crash-what-was-still-running).

## Where state lives

### Global data directory

Default location, resolved in `Config::default_data_dir()`
([config.rs](../crates/workspace-engine/src/config.rs)):

```text
~/Library/Application Support/DamaianClient
```

`DAMAIAN_DATA_DIR` overrides it wholesale. Everything below is relative to that
root, so a session run with `DAMAIAN_DATA_DIR=.damaian` writes a parallel,
isolated tree — that is the intended way to test without touching real data.

| Path | Contents | Written by |
|------|----------|------------|
| `config/user.conf` | User-scope config overlay | `Config::user_config_path` ([config.rs](../crates/workspace-engine/src/config.rs)) |
| `config/admin.conf` | Admin-scope overlay (highest precedence) | `Config::admin_config_path` ([config.rs](../crates/workspace-engine/src/config.rs)) |
| `audit/events.jsonl` | Append-only audit trail, secret-redacted | `AuditLog::record` ([audit.rs](../crates/workspace-engine/src/audit.rs)) |
| `sessions/<session-id>.jsonl` | Per-conversation event log: sessions, tasks, messages, and session-scope memory (`memory_recorded`) | `SessionStore` ([session.rs](../crates/workspace-engine/src/session.rs)) |
| `memory/schema.conf` | The memory store's own schema version (`schema_version=1`), separate from the data-directory marker | `MemoryStore` ([memory.rs](../crates/workspace-engine/src/memory.rs)) |
| `memory/project/<project-key>.jsonl` | Append-only project memory, one record per line. **Not safe to share** | `MemoryStore` ([memory.rs](../crates/workspace-engine/src/memory.rs)) |
| `memory/user.jsonl` | User-scope memory, present only after the scope is enabled. **Not safe to share** | `MemoryStore` ([memory.rs](../crates/workspace-engine/src/memory.rs)) |
| `patches/pending/` | Proposed patches awaiting approval | `PatchStore::save` ([edit.rs](../crates/workspace-engine/src/edit.rs)) |
| `patches/rejected/` | Rejected patches, kept for inspection | `PatchStore::mark_rejected` ([edit.rs](../crates/workspace-engine/src/edit.rs)) |
| `rollback/<patch-id>/` | Pre-edit file copies backing patch rollback, **unredacted** | `PatchEngine::apply_patch` ([patch_engine.rs](../crates/workspace-engine/src/patch_engine.rs)) |
| `commits/<prepare-id>.json` | A prepared commit’s record: candidates, exact blob ids, expected old `HEAD`, prepared commit, hooks and state. Redacted diffs only, no file content | `CommitEngine::save_record` ([commit.rs](../crates/workspace-engine/src/commit.rs)) |
| `commits/<prepare-id>/` | The private index and message file used for one attempt; removed after a known outcome | `CommitEngine` ([commit.rs](../crates/workspace-engine/src/commit.rs)) |
| `checkpoints/<repo-id>/objects/` | Session checkpoint content, a Damaian-owned Git object store, **unredacted** | [checkpoint.rs](../crates/workspace-engine/src/checkpoint.rs) |
| `checkpoints/<repo-id>/manifests/<checkpoint-id>.json` | What one checkpoint covers: paths, hashes, origins. No file content | [checkpoint.rs](../crates/workspace-engine/src/checkpoint.rs) |
| `commands/pending/<id>.dcmd` | Command proposals awaiting approval | `CommandStore::save_proposal` ([validation.rs](../crates/workspace-engine/src/validation.rs)) |
| `commands/output/<exec-id>/` | `stdout.log`, `stderr.log`, `summary.dcmd` per execution | `CommandStore::save_execution` ([validation.rs](../crates/workspace-engine/src/validation.rs)) |
| `commands/rejected/<id>.dcmd` | Rejected command proposals | `CommandStore::mark_rejected` ([validation.rs](../crates/workspace-engine/src/validation.rs)) |
| `chat/pending/<proposal-id>.json` | Suspended chat turn awaiting a command decision | `PendingCommandStore` ([chat.rs](../crates/workspace-engine/src/chat.rs)) |
| `sessions/<session-id>.progress.jsonl` | A worker's live progress, the board's and control strip's source. Secret-redacted, truncated when the worker starts, deleted with the session | [worker_progress.rs](../crates/workspace-engine/src/worker_progress.rs) |
| `workers/<hash>.sock` | A running worker's control socket (`0600` in a `0700` directory), removed when it exits cleanly | [worker.rs](../crates/desktop-shell/src/worker.rs) |
| `processes/<pid>-<start-time-us>.json` | One live child process this instance spawned, written **before** the child is returned to its caller and unlinked on clean exit | `ProcessRegistry::register` ([process_registry.rs](../crates/workspace-engine/src/process_registry.rs)) |
| `web-diagnostics/<session-id>/<task-id>/run-<ms>/` | Browser diagnostic screenshots, copied out of the browser server's own output. Images are **not** secret-scanned; they show whatever the page showed | `materialize_browser_artifacts` ([desktop-shell/src/lib.rs](../crates/desktop-shell/src/lib.rs)) |
| `repository-map/<repo-id>.json` | The repository map: roots, their evidence and commands, and excluded paths. Derived, safe to delete | `RepositoryMapStore` ([repository_map.rs](../crates/workspace-engine/src/repository_map.rs)) |
| `symbol-index/<repo-id>.json` | The symbol index: declarations, relations, per-file hashes and the on-demand reference cache. Derived, safe to delete | `SymbolIndexStore` ([symbol_index.rs](../crates/workspace-engine/src/symbol_index.rs)) |
| `vector-index/<repo-id>.bin` | Semantic-search embeddings cache | `vector_index_path` ([vector_index.rs](../crates/workspace-engine/src/vector_index.rs)) |
| `models/all-MiniLM-L6-v2/` | Downloaded embedding model (semantic search only) | `EmbeddingModel::load` ([embeddings.rs](../crates/workspace-engine/src/embeddings.rs)) |

Not on disk: the **repository index** is an in-memory cache with a file watcher
([index_cache.rs](../crates/workspace-engine/src/index_cache.rs)), rebuilt on a
5-minute full-rescan interval. Restarting the app clears it — which is why a
restart "fixes" stale-index symptoms.

### Configuration layers

Three files overlay onto the built-in defaults, then the repository's
permission profile, applied in this order. Later wins — **except at repository
and profile scope**. Repository scope is untrusted because that file arrives
with a clone, and a profile exists only to narrow, so both can only make the
policy stricter:

| Order | Scope | Path | Can weaken? | Override |
|-------|-------|------|-------------|----------|
| 1 | User | `<data-dir>/config/user.conf` | yes | — |
| 2 | Repository | `<repo>/.damaian/config.conf` | **no** | — |
| 3 | Admin | `<data-dir>/config/admin.conf` | yes | `DAMAIAN_ADMIN_CONFIG` |
| 4 | Profile | built in, or `<data-dir>/config/profiles/<name>.conf` | **no** | selected by `permission_profile.<repository_id>` in user config |

With no profile selected, nothing is applied at step 4 (Full repository
development).

A missing file is skipped silently, not an error. The format is flat
`key=value`, one per line; blank lines and `#` comments are ignored
(`ConfigOverlay::parse` in
[config.rs](../crates/workspace-engine/src/config.rs)). List-valued keys use
`|` as the separator. Nested keys are dotted:

```conf
model_provider=deepseek
model_provider.deepseek.base_url=https://api.deepseek.com
model_provider.deepseek.supports_native_tools=true
model_provider.deepseek.max_output_tokens=8192
mcp_server.playwright-mcp.enabled=true
command_allowlist=npm test|cargo test
```

Provider and MCP-server entries are **upserted by id**, so a repo-scope file can
change one field of a provider defined at user scope without restating the rest.

The desktop Settings UI only ever writes **user** scope
(`desktop_settings_config_path` in
[lib.rs](../crates/desktop-shell/src/lib.rs)). If a setting will not stick,
something at admin scope is overriding it, or repository scope is adding a
restriction on top of it.

#### A repository config key had no effect

That is usually deliberate. Repository config may add restrictions but never
remove one
([SECURITY.md](../SECURITY.md#repository-config-trust-boundary), spec
[34](specs/34_repository_config_trust_boundary.md)):

| Key | In repository config |
|-----|----------------------|
| `shell`, `data_dir`, `allowed_roots`, `secret_patterns`, `audit_enabled`, `block_generated_secrets`, `enable_worker_sessions`, `language_server.<lang>`, `checkpoint_retention_days`, `checkpoint_max_total_bytes`, `checkpoint_census_max_paths`, any `model_*` (including `model_provider.<id>.*`) | Ignored, audited, reported to the user once per repository |
| `command_allowlist`, `command_allowlist.<repo_id>`, `permission_profile.<repo_id>`, `hook_review.<repo_id>.<hook_id>` | Never honoured: they are the user's decisions, and `Allow Always` writes user scope instead |
| `hook.<id>.*` | A new hook is kept, and runs only after you review it; one that names a hook you or an admin defined is ignored |
| `restricted_patterns`, `ignore_patterns`, `command_blocklist` | Added to the user's list, never replacing it |
| `require_approval_for_*` | May be set `true`, never `false` |
| `allow_file_edits`, `allow_browser_diagnostics`, `allow_mutating_mcp_tools`, `command_access` | May be turned down, never up |
| `mcp_enabled`, `mcp_server.<id>.enabled` | May be turned off, never on |
| `mcp_server_allowlist` | May be narrowed, never widened |
| `mcp_server.<id>.*` for a server the user already configured | Ignored: it would redirect a process the user trusts |
| `command_timeout_secs`, `max_read_lines`, `max_list_entries`, `max_search_matches`, `max_match_line_chars`, `repository_map_max_tokens`, `agent_max_task_tokens`, `agent_max_turn_messages`, `agent_max_exploration_rounds`, `agent_max_turn_notes_chars` | May be lowered, never raised |
| Everything else (`max_file_bytes`, `max_command_output_bytes`, `audit_retention_days`, `enable_semantic_search`, `agent_max_tool_rounds`, `agent_web_debug_max_tool_rounds`, `agent_tool_retry_limit`, `agent_max_questions_per_turn`, `project_roots_added`, `project_roots_removed`) | Applied as written |

Ask which keys a repository asked for and did not get — this also records them
in the audit log, so it answers once per repository and then reports nothing
new:

```bash
cargo run -p damaian-cli -- config-review /path/to/repo
```

`config-set repo` prints the same note immediately after writing a key that
repository scope will ignore.

#### Why a rule is in force

Read the attributed view rather than the raw files. In the desktop app it is
`Settings` › `General` › `Effective policy` with the repository open; from the
CLI:

```bash
cargo run -p damaian-cli -- config-show --sources /path/to/repo
```

The desktop table names each rule in plain words with its config key
underneath; the CLI prints the key only. Every rule names its source:
`default`, `user config`, `repository config`,
`admin config`, `profile: <id>`, or `this repository (Allow Always)`. List keys
name a source per entry. When two scopes agree, the source is the last one
whose value holds, so a repository that repeats your own restriction takes the
attribution. `widened by admin config` marks a rule an admin value loosened. An
admin value on a key with no direction (`audit_enabled`, `allowed_roots`,
`secret_patterns`, `block_generated_secrets`, `command_allowlist`, …) shows
`admin config` as its source and is never tagged.

If a command, edit or diagnostic is refused and the message names a setting,
for example `(command_access=local)`, find that key in the view: its source says
whether the profile, your config, the repository or admin set it. The view
describes the loaded config. A provider you pick in the composer for one
request is not reflected.

#### A permission profile key had no effect

A profile only narrows. A profile key has no effect when:

- **It would loosen what is already in force.** For example `command_access=all`
  in a custom profile over a user config that set `local`. Nothing is reported
  for this at load; import lists such keys under "Written but no effect here".
- **A profile may not carry it.** `shell`, `data_dir`, `allowed_roots`,
  `secret_patterns`, `audit_enabled`, `block_generated_secrets`, every `model_*`
  key, the preference keys (`max_file_bytes`, `enable_semantic_search`, the
  `agent_*` round limits, …), `command_allowlist`, a
  `permission_profile.<id>` selection, and an MCP server definition. These
  appear in the view as `Refused: the permission profile tried to change
  “<rule>” (<key>). …`, or in `config-show --sources` as `refused: the
  permission profile asked to set <key> (<class>)`. Each is audited once per
  key per profile.

If the selected custom profile's file is missing, the load fails with an error
naming it rather than silently running as Full. Pick another profile, or
restore the file.

`local` blocks every `npm`, `pnpm` and `yarn` command, validation scripts
included, because it matches command names. That is expected, not a
misclassification; see the user guide's "What `local` and Offline private
cannot promise".

#### Where `Allow Always` entries live

In **user** config, keyed by repository:

```conf
command_allowlist.repo_sha256:9f2c1a7b=cargo test|npm ci
```

The id is derived from the working folder's canonical path, so a grant does not
carry over to another clone of the same project. The plain `command_allowlist`
key still works at user and admin scope and applies to every repository.

If a repository already carried a `command_allowlist` when you opened it, the
app lists those commands once and asks which to keep; nothing from that list
runs without approval until you answer. The CLI equivalent — no commands means
discard everything:

```bash
cargo run -p damaian-cli -- config-allowlist-keep /path/to/repo "cargo test"
```

Either way the repository's own file is left exactly as it is.

### Memory

Memory is a small, local, append-only store of confirmed one-sentence facts. It
lives under `<data dir>/memory/`, and the directory and its `project/`
subdirectory are mode `0700`.

```text
memory/schema.conf                  schema_version=1
memory/project/<project-key>.jsonl  project scope
memory/user.jsonl                   user scope, only after enabling it
```

Session-scope memory is not here: it is `memory_recorded` events in that
session's log, so it is deleted with the session.

**Read one.** Each line is a complete record; the last record for an `id` wins,
which is how an edit, supersession, expiry or deletion is represented without
rewriting the file. A torn final line is ignored on read. To see the current
project entries:

```bash
jq -r 'select(.state=="active") | "\(.category)\t\(.created_at_ms)\t\(.statement)"' \
  "$DATA_DIR"/memory/project/*.jsonl | sort -t $'\t' -k2
```

The fields are the entry `id`, `scope`, `project_key`, `session_id`, `category`,
`statement`, `provenance`, `verification`, `sensitivity`, `state`,
`created_at_ms`, `updated_at_ms`, `supersedes` and `expiry`. `provenance` is
one of `userRequest`, `suggestedAndConfirmed` or `repositoryEvidence`; there is
no provenance for an unconfirmed model inference, so one cannot be stored.

**Which project a file belongs to.** The file name is the `project_key`, and its
prefix names the tier that produced it:

| Prefix | Resolved from | Survives |
|---|---|---|
| `root_<40-hex>` | `git rev-list --max-parents=0 HEAD` | clone, move, rename, remote change, fork |
| `remote_<16-hex>` | normalized `origin` URL (host + path) hashed | clone, move; not a remote rename |
| `path_repo_sha256_<hex>` | the existing path-derived `repository_id`, with its colon mapped to `_` | nothing; per-checkout fallback |

The root commit is preferred because it belongs to the repository's history
rather than to where it sits. A fork shares its upstream's root commit and
therefore its memory — deliberate, since the conventions usually still apply. A
**shallow clone** (`git clone --depth 1`) has no true root: Git reports the
grafted boundary, whose value depends on the depth and changes on
`git fetch --unshallow`, so the root-commit tier is skipped and the remote is
used. Two shallow clones of one repository therefore share through `origin`,
and a shallow clone with no portable remote falls to the path hash. A `file:`
remote is a local path with no portable host and also falls through — two
unrelated repositories can share a path tail, so treating it as identity would
over-share. A repository with no commit and no remote falls to the path hash,
and the failure mode there is isolation, not sharing. The UI reports the tier
rather than the raw key.

**Why a memory was not created.** A proposal that is not in the card list was
refused before it could be shown, not lost. The audit log has the reason:

| Audit event | Meaning |
|---|---|
| `memory_proposal_created` | The candidate became a card. It is still not an entry. |
| `memory_proposal_refused` | No card was shown. `reason` is `secret`, `instruction_shaped`, `evidence_restricted`, `evidence_ignored`, `evidence_not_relative`, `statement_too_long` or `suggestion_cap_reached`. A `path` field is present only when the candidate cited evidence. |
| `memory_proposal_confirmed` | You pressed Remember and the entry was written. |
| `memory_proposal_dismissed` | You dismissed the card. Nothing was written. |
| `memory_proposal_failed` | The proposal could not be recorded at all; the turn continued. |

A non-user suggestion is capped at three per task. The fourth produces
`reason=suggestion_cap_reached` rather than a card. An explicit "remember that…"
from you is not capped.

The pending cards themselves are session-log events, `memory_proposal_proposed`
and `memory_proposal_decided`, so a reload or restart shows what is still
undecided and a dismissed proposal stays visible in the log as history. A
proposal never enters the memory files and never enters the model's
conversation.

**A refused secret write.** A statement (or an evidence path) that the secret
scanner recognises is refused with *"That looks like a credential; Damaian will
not store it."* Nothing is written — not even a `[REDACTED_…]` placeholder,
because a redacted memory is recalled forever and records that a credential
exists. The same path refuses evidence under `restricted_patterns` or excluded
by a `.gitignore`, with a message naming which of the two applied but not the
protected path.

Pattern matching cannot see every credential. That is why the confirmation step
shows the exact sentence that will persist, and why a stored memory will be
redacted again on its way into a request once recall ships.

**A newer schema.** Opening a store written by a newer build fails with the
marker path and both versions and changes nothing. Upgrade Damaian, or point
`DAMAIAN_DATA_DIR` somewhere else. An older store (files with no marker) is
migrated by appending normalized records and then writing the marker; the
pre-migration lines stay for audit.

**Lifecycle events** appear in `audit/events.jsonl` as `memory_created`,
`memory_confirmed`, `memory_recalled`, `memory_edited`, `memory_superseded`,
`memory_expired`, `memory_deleted` and `memory_state_changed`, plus
`memory_schema_migrated` for the version boundary. They carry the entry id,
scope, category, provenance type, sensitivity and the state transition. The
statement appears only for `Ordinary` sensitivity: a personal or environmental
statement in a retained audit line is the exposure this rule prevents.

**Memory files are not safe to share.** They can name a machine, a path, a port
or a personal preference, exactly like a session log. Do not attach them to a
bug report or a shared diagnostic bundle.

### Secrets

API keys live in the macOS Keychain under service `DamaianClient`
(`KEYCHAIN_SERVICE` in [keychain.rs](../crates/desktop-shell/src/keychain.rs)).
Config holds only a reference: `keychain:<account>`, or the name of an
environment variable — never a raw key.

A loopback endpoint such as Ollama (local) does not need a key. When the
reference resolves to nothing on a loopback endpoint, Damaian sends no
`Authorization` header instead of refusing the request. A missing key is
tolerated only for a loopback endpoint; every remote provider still requires
one.

```bash
security find-generic-password -s DamaianClient -a model-api-key -w
```

That prints the secret, so prefer checking existence only. The UI's
`/api/model-key-status` and the Settings `Model API Key` field report `Saved`
without revealing the value. If `model_api_key_env` names an environment
variable instead, the variable must be set **in the environment the app was
launched from**; see
[The app does not see your shell's environment](#the-app-does-not-see-your-shells-environment).

#### What is not safe to share

Two places under the data directory hold your files **exactly as they were**,
credentials included, because their whole job is to put a file back byte for
byte (`docs/specs/16_session_checkpoints_and_rewind.md` §5.2):

- `checkpoints/<repo-id>/objects/` — session checkpoint content.
- `rollback/<patch-id>/` — pre-apply copies backing patch rollback.

Redaction covers what leaves your control — model context, command output,
diffs, the audit log — and deliberately does not cover these. Do not attach
either to a bug report. Session logs are not safe to share either: they hold
prompts, answers, and file content.

Checkpoint *manifests* are safe by design: they carry paths, hashes, and counts,
never content.

### Web UI state (localStorage)

Project list, last folder, and per-repo preferences are browser state, not
config. Keys (the `…StorageKey` and `…StoragePrefix` constants in
[`app.js`](../crates/desktop-shell/static/app.js)):

| Key | Contents |
|-----|----------|
| `damaian:projects` | Known project paths |
| `damaian:lastRepository` | Folder restored on launch |
| `damaian:projectDisplayNames` | Renamed projects |
| `damaian:expandedProjects`, `damaian:projectsCollapsed` | Sidebar UI state |
| `damaian:lastSession:<repo>` | Last open conversation per repo |
| `damaian:pinnedContextFiles:<repo>:<session>` | Pinned context files (`draft` before a session exists) |
| `damaian:chatModelPrefs:<repo>` | Selected provider/model per repo (`global` with no repository open) |
| `damaian:dismissedWorkers` | Finished workers dismissed from the board, by session id. The sessions themselves stay |

Physically stored in the WebKit data store for the bundle id, not in the data
directory:

```text
~/Library/WebKit/com.damaian.client
```

A "the app forgot my projects" report is almost always this store, not
`user.conf`. It is also why a fresh `DAMAIAN_DATA_DIR` still shows the old
project list.

### Repository roots and the repository map

The map of a repository's project roots
([User Guide](USER_GUIDE.md#repository-roots)) is stored per repository:

```text
$DATA_DIR/repository-map/<repository_id>.json
```

`<repository_id>` is the same `repo_…` id as `command_allowlist.<repository_id>`.
The map is derived from the in-memory index plus config. The stored file is
reused only while its `fingerprint` still matches the inputs: the indexed
paths, the content of every manifest and `AGENTS.md`, the root overrides, the
command allowlist and blocklist, and the excluded paths. Anything else
rebuilds it: a changed manifest, a new grant, an unreadable file, or another
schema version. **To force a rebuild, delete the file.** It is built again on
the next request or map read. A deleted file is rebuilt silently. A stale,
corrupt or version-mismatched one is rebuilt with an audit event.

Read it without the UI. `--json` prints `{"load":{…},"map":{…}}`, the same
shape the Settings section reads from `/api/repository-map`:

```bash
DAMAIAN_DATA_DIR=/tmp/damaian-debug cargo run -p damaian-cli -- repo-map /path/to/repo
DAMAIAN_DATA_DIR=/tmp/damaian-debug cargo run -p damaian-cli -- repo-map /path/to/repo --json | jq '.map.roots[] | {path, detectedBy, userOverride}'
```

The first line of the summary says what happened: `map reused (inputs
unchanged)`, `map built (none stored)`, or `map rebuilt: …` with the reason.
In the JSON, `load.outcome` is `reused`, `built` or `rebuilt`, and a rebuild
carries `reason` (`corrupt`, `schemaMismatch` with `foundSchemaVersion`, or
`stale`).

**Reading `detectedBy`.** Each root says why it is one:

| `detectedBy.kind` | Meaning |
|---|---|
| `repositoryRoot` | The working folder, which is always a root, holding none of the known manifests |
| `manifest` | `detectedBy.path` names the manifest that made it a root. With several in one directory, the first of `package.json`, `pyproject.toml`, `pytest.ini`, `pom.xml`, `build.gradle`, `go.mod`, `Cargo.toml` is named, so a root with `package.json` and `Cargo.toml` reads `package.json` |
| `userOverride` | Added by `project_roots_added`, with no manifest of its own |

`userOverride` on a root (`added` or `removed`) records a correction. A
removed root keeps its manifest evidence. Its lists are empty, it proposes no
commands, and it is not shown to the model. Its files belong to the nearest
enclosing root.

**Reading `excluded`.** Only three kinds of path are listed. Other ignored
paths were never candidates and are not listed:

| `reason` | Meaning |
|---|---|
| `vendor` | A `node_modules`, `vendor`, `target`, `dist`, `build`, `.venv` or `venv` directory. Never a root, even when indexed |
| `belowDepthCeiling` | A manifest directory more than 6 segments deep (`packages/api` is 2). Not a root. If it should be one, add it: added roots are not held to the ceiling |
| `invalidOverride` | A `project_roots_added` or `project_roots_removed` entry that was not applied. Possible causes: it is not a directory holding an indexed file; it is absolute or contains `..`; it is under a vendor directory or a `restricted_patterns` match; it is in both lists; it removes the repository root; or it removes a directory that is not a root. Fix or delete the entry |

**A root is missing.** Check, in order:

1. Does the directory hold one of the seven manifests? A `setup.py` alone is not one.
2. Is it `.gitignore`d, under an `ignore_patterns` match, or under a vendor directory?
3. Is it listed in `excluded`?
4. Has an override removed it? The override can come from the repository's own
   `.damaian/config.conf`, which may be committed, or from your
   `config/user.conf`, where the keys apply to every repository.
   `config-show` prints both lists.

The index is in memory and updated by the file watcher. A manifest created a
moment ago may need the watcher to catch up, or a restart.

**A directory is wrongly a root.** The usual cause is a stray manifest in an
example or a fixture. Remove it in Settings › General › Repository roots, or
from the CLI:

```bash
cargo run -p damaian-cli -- repo-root /path/to/repo remove examples/demo
```

`add` and `clear` work the same way. All three write
`<repo>/.damaian/config.conf`, which `git status` then shows. If that file
already has a line the parser rejects, the write is refused with the parse
error and the file is left alone.

**A root has no commands, or not the expected ones.** Commands come only from
the `package.json` scripts named exactly `test`, `lint`, `typecheck`, `build`
or `format`, and from one fixed command per other manifest. A script named
`lint:web` is not picked up. A Cargo workspace root and each member all show
`cargo test`, each in its own directory.

**A command ran in the wrong directory.** The approval card's **Runs in**
line shows where a command runs. So do `workingDirectory` and `repositoryRoot`
in the stored proposal (`commands/pending/<id>.dcmd`, or
`/api/command-proposal`). A proposal written before roots existed has no
`REPOSITORY_ROOT` line and is read as running at the repository root.

**The audit event.** A rebuild of a stored map writes
`repository_map_rebuilt` with `repositoryId`, `reason` (`corrupt`,
`schemaMismatch` or `stale`) and, for a mismatch, `foundSchemaVersion`. A
first build and a reuse write nothing.

```bash
jq -c 'select(.eventType=="repository_map_rebuilt")' "$DATA_DIR/audit/events.jsonl"
```

**What the model saw.** The map reaches the model as one context item of
kind `repository_map`. It is limited by `repository_map_max_tokens`: the
default is 800, `0` turns it off, and a repository may only lower it. When the
map is cut to fit, its last line starts `Abridged to fit:` and says what was
dropped. If the map cannot be stored, for example because the data directory
is read-only, it is left out of the request and the turn goes on.

### Symbol search shows only likely results, or a language server will not start

**Check the index first.** The symbol index is stored per repository:

```text
$DATA_DIR/symbol-index/<repository_id>.json
```

`<repository_id>` is the same `repo_…` id as `command_allowlist.<repository_id>`
and `repository-map/`. The file is derived from the in-memory index. **To force a
rebuild, delete it**; the next symbol search builds it again. A missing file is
built silently. A stale, corrupt, version-mismatched or foreign file is rebuilt
and audited as `symbol_index_rebuilt` with `repositoryId`, `reason` (`corrupt`,
`schemaMismatch` with `foundSchemaVersion`, or `stale`), `fileCount`,
`totalFiles` and `durationMs`. A first build and a reuse write nothing.

```bash
jq -c 'select(.eventType=="symbol_index_rebuilt")' "$DATA_DIR/audit/events.jsonl"
```

**Read it without the UI.** Each command takes a scratch `DAMAIAN_DATA_DIR` so
they do not touch your real state:

```bash
DAMAIAN_DATA_DIR=/tmp/damaian-debug cargo run -p damaian-cli -- symbols /path/to/repo upload_retry
DAMAIAN_DATA_DIR=/tmp/damaian-debug cargo run -p damaian-cli -- symbols /path/to/repo upload_retry --json | jq '.matches[] | {name: .symbol.name, path: .symbol.path, line: .symbol.line}'
DAMAIAN_DATA_DIR=/tmp/damaian-debug cargo run -p damaian-cli -- symbol-references /path/to/repo src/upload.rs:42 upload_retry
```

`symbols` prints `name  kind  path:line  root  provenance` and says whether the
index was built, reused or rebuilt. `symbol-references` prints either the
unavailable reason, or each `path:line  name  kind  provenance`. Neither prints a
heuristic result without `provenance`.

**A configured server shows `idle`, `unavailable` or `crashed`.**
Settings › General › Language servers lists every configured language;
`GET /api/language-servers?repo=…` returns the same. `idle` means configured but
not started. `ready` carries the PID. `unavailable` carries the reason: the spawn
failed, or `initialize` failed or timed out. `crashed` carries `restarts`; after
the one restart it stays down until the entry expires or Damaian restarts.

- **The command is not found.** A GUI app does not inherit your shell's `PATH`.
  Use an absolute path in `language_server.<language>`, and test that exact
  command from Terminal with the repository as the working directory. Damaian's
  spawn sends the server's stderr to `/dev/null`, so the app keeps no server
  log; run the command yourself to see its errors.
- **The key has no effect.** It must be in user config, not
  `.damaian/config.conf`: a repository's value is ignored and audited as
  `repository_config_key_rejected`. A typo in the language name, or an empty
  command, is a parse error naming the key. A permission profile cannot carry
  it.
- **Which processes are ours.** A running server has one
  `processes/<pid>-<start-time-us>.json` entry with
  `"kind":"language_server"`. On a sweep, an entry whose recorded start time no
  longer matches is refused rather than killed, and the refusal is audited as
  `orphan_process_kill_refused` with `"kind":"language_server"`. Damaian never
  kills a process by name, so your editor's own `rust-analyzer` is not at risk.

**Why results stay `likely`.** The first symbol search for a repository starts
the background pass, but answers from the snapshot it read, so that response is
heuristic. A later search after the pass finishes shows the server's
definitions. If results are still `likely`, check the reason in Settings ›
General › Language servers: `idle` means no search has started the server yet,
and `unavailable`/`crashed` carry why. A configured server can also leave a file
heuristic when it reports nothing for it, when the file is restricted, or when
its language has no configured server. Diagnostics appear after a search has let
the server open files, on the next findings refresh.

## Logs

**There is no application log file.** Nothing writes a rolling log. Diagnostics
come from four separate places, and confusing them wastes time:

| Source | Contains | Persistent |
|--------|----------|------------|
| stdout / stderr | Startup, bind failures, shell/navigation errors, panics | No |
| `audit/events.jsonl` | What happened, with metadata — not message bodies | Yes |
| `sessions/*.jsonl` | Prompts, replies, task status transitions | Yes |
| `commands/output/*/` | Full stdout and stderr of executed commands | Yes |

### Capturing stdout and stderr

Every runtime error the wrapper reports goes to stderr via `eprintln!` (in
`main` and its `setup` closure,
[desktop-app/src/main.rs](../crates/desktop-app/src/main.rs)) and is lost if
nothing is attached to the process.

Development — output appears in the terminal:

```bash
DAMAIAN_REPO=/path/to/repo npm run desktop:dev
```

Packaged app — launch the binary directly instead of double-clicking. The
executable is the `damaian-desktop` Cargo binary, not the product name:

```bash
/Applications/Damaian.app/Contents/MacOS/damaian-desktop
```

Already-running GUI instance — read the unified log, which captures stderr from
GUI-launched processes:

```bash
log show --predicate 'process == "damaian-desktop"' --last 30m --info
```

Standalone shell, no Tauri wrapper, on a port of your own so you do not collide
with the user's app:

```bash
DAMAIAN_DATA_DIR=/tmp/damaian-debug cargo run -p desktop-shell -- --repo /path/to/repo --port 4900
```

Messages worth recognising on startup. The shell's own lines come from
`run_server_with_ready` and the functions it calls in
[desktop-shell/src/lib.rs](../crates/desktop-shell/src/lib.rs), and are printed
to stdout; the wrapper's come from the `setup` closure in
[desktop-app/src/main.rs](../crates/desktop-app/src/main.rs), on stderr:

| Message | Meaning |
|---------|---------|
| `Damaian desktop shell listening at http://127.0.0.1:<port>` | Shell is up (stdout) |
| `Default repository: <path>` | The repository the shell was started with (stdout) |
| `Data directory <path> adopted at schema version 1` | An existing data directory with no `schema.conf` was marked as the current layout; see [The data directory is refused at startup](#the-data-directory-is-refused-at-startup) |
| `Orphan sweep: killed <n>, refused <m> on a start-time mismatch` | The launch sweep cleaned up after a crash; see [After a crash: what was still running](#after-a-crash-what-was-still-running). Printed only when it killed or refused something |
| `Damaian cannot use its data directory: <error>` | The data directory's schema marker is one this build cannot read. The app also shows a dialog and quits |
| `...refuses to start because 127.0.0.1:4765 is already in use` | Port conflict; see [Port 4765](#port-4765) |
| `Damaian shell server stopped: bind 127.0.0.1:<port>: <error>` together with `Damaian shell did not start: …` | The shell could not bind its port, or failed another startup step before it was ready |
| `Damaian shell did not report readiness` | Shell did not bind within the 2-second startup timeout |
| `Damaian shell server stopped: <error>` | Server thread died after startup |

### Audit log

Append-only JSONL, one event per line, every field passed through the secret
scanner before being written (`AuditLog::record` in
[audit.rs](../crates/workspace-engine/src/audit.rs)).
Records **metadata about actions** — file paths, commands, exit codes, token
estimates, provider and model names, status — deliberately **not** prompt or
response bodies. Disabled entirely when `audit_enabled=false`.

Retention (`AuditLog::cleanup_retention`) runs before every append. It deletes
each file in `audit/` whose mtime is older than `audit_retention_days`
(default 90); `0` turns pruning off. It deletes whole files and never trims one.
While Damaian is in use, `events.jsonl` is appended to and its mtime stays
current, so the active log grows without bound. After an idle period longer
than `audit_retention_days`, though, its mtime is past the cutoff, and the next
event deletes the **whole** `events.jsonl` before starting a new one. Copy the
file first if you need history from before a long break.

Every line carries `eventId`, `timestampMs`, `userId` (`local_user`), and
`eventType`.

A repository-sourced config key that was refused appears as
`repository_config_key_rejected` with `key` and `class`
(`forbidden`, `restrict_only`, or `user_owned`) — never the refused value,
which is repository-controlled text:

```bash
jq -r 'select(.eventType=="repository_config_key_rejected") | "\(.repositoryId) \(.key) \(.class)"' "$DATA_DIR/audit/events.jsonl"
```

Permission profiles leave three events, none of which carries a refused or
imported value:

- `permission_profile_set` — a selection, with `repositoryId`,
  `repositoryPath`, `from` and `to`.
- `permission_profile_key_rejected` — a key the selected profile may not carry,
  with `profileId`, `repositoryId`, `key` and `class`. Once per key per profile;
  replacing a custom profile by import resets that, so the new file's refusals
  are audited afresh.
- `permission_profile_imported` — with `profileId`, `replaced`, and the counts
  and key names of what was not carried and what would loosen.

```bash
jq -c 'select(.eventType|startswith("permission_profile"))' "$DATA_DIR/audit/events.jsonl"
```

How a command or browser diagnostic refused by the session mode or the profile
is recorded is in
[The assistant was refused](#the-assistant-was-refused-the-session-mode-the-permission-profile-or-the-command-policy).

```bash
tail -20 "$DATA_DIR/audit/events.jsonl" | jq .
```

Last 50 events as a readable timeline:

```bash
jq -r '"\(.timestampMs|tonumber/1000|strftime("%H:%M:%S")) \(.eventType) \(.status // .exitCode // .files // "")"' "$DATA_DIR/audit/events.jsonl" | tail -50
```

Which event types have ever fired, and how often:

```bash
jq -r .eventType "$DATA_DIR/audit/events.jsonl" | sort | uniq -c | sort -rn
```

Event types grouped by what they tell you:

| Area | Event types |
|------|-------------|
| Model turn | `context_assembled`, `model_request_prepared`, `model_response_completed`, `edit_model_request_prepared`, `chat_turn_cancelled`, `model_usage_reporting_unsupported`, `turn_continued` |
| Round budget | `round_budget_paused`, `round_budget_decision`, `turn_budget_extended` |
| Plan review | `plan_review_decision` |
| Patches | `patch_proposed`, `edit_patch_ready_for_approval`, `edit_failed`, `patch_applied`, `patch_hunks_rejected`, `patch_rollback_started`, `patch_rolled_back`, `stored_patch_applied`, `stored_patch_files_rejected`, `stored_patch_rejected`, `stored_patch_rolled_back` |
| Commands | `command_proposed`, `command_proposal_stored`, `command_executed`, `stored_command_executed`, `stored_command_rejected`, `command_allowlisted`, `repository_allowlist_migrated` |
| Files & repo | `file_read`, `file_modified`, `file_restored`, `directory_listed`, `content_searched`, `repository_indexed`, `repository_map_rebuilt`, `symbol_index_rebuilt`, `git_status_read`, `git_diff_read`, `git_log_read`, `git_worktrees_listed` |
| Checkpoints | `checkpoint_created`, `checkpoint_creation_failed`, `checkpoint_files_skipped`, `checkpoint_paths_added`, `checkpoint_restored`, `checkpoint_conflicted`, `checkpoint_cleanup` |
| MCP | `mcp_tools_listed`, `mcp_tool_called`, `mcp_tool_call_failed`, `mcp_discovery_failed` |
| Provider refusals | `provider_refusal`, `provider_fallback_offered`, `provider_fallback_decision` |
| Browser diagnostics | `browser_diagnostic_approval_decision`, `browser_diagnostic_session_approval_used` |
| Hooks | `hook_invoked`, `hook_widening_attempt`, `hook_depth_exceeded`, `hook_command_refused`, `hook_reviewed` |
| Config and profiles | `repository_config_key_rejected`, `permission_profile_set`, `permission_profile_key_rejected`, `permission_profile_imported` |
| Crash recovery | `task_recovered`, `task_recovery_decision`, `session_log_truncated_tail`, `pending_approval_unavailable`, `reattached_approval_resolved`, `task_usage_recorded_for_lost_call` |
| Workers | `worker_spawned`, `worker_spawn_failed`, `worker_restarted`, `worker_terminated`, `worker_worktree_removed` |
| Orphan sweep | `orphan_process_killed`, `orphan_process_kill_refused`, `orphan_process_already_exited`, `process_registry_entry_unreadable` |

Useful reads:

- `model_request_prepared` carries `contextFiles`, `tokenEstimate`,
  `toolRound`/`toolRoundLimit` and `explorationRounds`/`explorationLimit` —
  the fastest way to confirm what the model actually saw, how many action and
  read-only rounds a turn took, and where its budget stood.
- `model_response_completed.status` distinguishes `complete`,
  `complete_with_command`, `command_approval_required`, and
  `patch_proposal_ready`. A turn that "did nothing" usually ended `complete`
  when you expected `patch_proposal_ready`.
- `command_proposed` carries `risk` and `requiresApproval` — check here before
  assuming the command policy is wrong.
- `command_proposal_stored` carries `causes`, the approval reasons as variant
  names in classifier order, comma-joined (`unknown,outside_repository`), with
  the `command` they explain. This is the field to read when asking why a prompt
  appeared; it is written when the proposal is stored, before any decision.
- A `patch_proposed` with no following `patch_applied` means the user never
  approved, or apply failed on a hash conflict.
- An `edit_model_request_prepared` followed by `edit_failed` is a proposal that
  died after the model answered: `edit_failed.status` and `.error` say why. The
  common causes are a patch naming a path in `restricted_patterns` (asking for a
  `.env` does this) and a model that replied in prose instead of the
  `DAMAIAN_EDIT_V1` envelope. An `edit_model_request_prepared` with *neither*
  `edit_failed` nor `edit_patch_ready_for_approval` means the process died
  mid-request.
- `git_status_read` rows come in pairs: every `/api/git-status` request (each
  repo-bar refresh) runs `git status` and then the repo-bar summary, which is
  marked `scope: summary`. The summary row never records the remote URL. In a
  folder that is not a repository both rows read `status: failed` with git's
  exit code, and the bar shows the folder alone; that is expected, not an
  error. To check the bar's `+N −M`, compare it with
  `git diff --numstat HEAD` plus the line counts of untracked text files
  (files over 1 MiB or with a NUL byte in the first 8 KiB are not counted).
- `context_assembled` is the one event that says what a turn's context held and
  what it left out: [Reading a turn's context manifest](#reading-a-turns-context-manifest).

### The context inspector: why a file was not consulted

The first place to look when an answer missed a file is the **Context** control
in the conversation header, not the audit log. It opens the panel described in
the [user guide](USER_GUIDE.md#inspecting-and-controlling-context) and shows the
same manifest, without a shell.

- **Expand Excluded.** It holds one row per omitted item with the reason. The
  common answers are `restricted_path` (the path policy refused it),
  `category_budget_exhausted` (its category was full), `too_large`,
  `path_not_selected` (the session's path restriction does not include it) and
  `missing` (a pinned file no longer reads).
- **A pin is not in the request.** Look under Excluded for that path. A pin that
  became restricted or whose file was deleted stays pinned and appears there
  with the reason; restoring the file restores the pin. A pin whose path is not
  in Excluded was included — expand **Pinned** to confirm.
- **Only a few files are being considered.** Check the line under the pin list
  that says the session is restricted to N paths, and clear it with **Clear
  paths** if that was not what you meant. The restriction never overrides
  `restricted_patterns`, so selecting a restricted path does not include it.
- **An item you removed is still listed.** On a paused turn, an item removed in
  the panel appears under Excluded as `user_removed` and the token total falls.
  Removal is per turn; a pin or restriction is what changes later turns.
- **Something is missing from the panel.** `Required` cannot be removed by
  design: it is your own message and Damaian's required context, and the API
  refuses its removal even if a client asks. `AGENTS.md` is removable for a
  paused turn, but a path restriction does not remove it — repository
  instructions are policy, not one of the content files a restriction selects.
- **An earlier turn.** Each assistant message with a recorded manifest has a
  **Context · N tokens** link. It opens the read-only view of what that turn
  actually sent, including any removal made before it was sent. The post-send
  record is an audit event: with `audit_enabled=false`, or after audit retention
  has removed it, an old turn has no link. The live pre-send view is unaffected.

### Reading a turn's context manifest

Each model turn records one `context_assembled` event before its first
`model_request_prepared`, and each one-shot edit records one before
`edit_model_request_prepared`. It carries the manifest — every included item by
path, range and owning root, every item left out with a reason, and what each
context category was allowed and used. **The manifest holds no file content**, so
it is safe to paste into a bug report.

```bash
# The latest turn's manifest, pretty-printed.
jq -c 'select(.eventType=="context_assembled")' "$DATA_DIR/audit/events.jsonl" \
  | tail -1 | jq -r '.manifest | fromjson'
```

All fields can be read straight off the event:

```bash
jq -r 'select(.eventType=="context_assembled")
  | "\(.timestampMs) task=\(.taskId) tokens=\(.tokenEstimate)"' "$DATA_DIR/audit/events.jsonl" | tail -5
```

The event itself holds `actor` (`system`), `sessionId`, `repositoryId`,
`taskId`, `tokenEstimate` (the manifest's `totalTokens`) and `manifest`, the
whole manifest as a JSON string. Every audit field value is a string, so the
manifest is parsed with `fromjson` rather than read field by field.

Inside the manifest:

| Field | Meaning |
|---|---|
| `repositoryId`, `taskId` | Which checkout and which task the assembly was for. |
| `assembledAtMs` | When it was assembled. The only time-dependent field; it never reaches the prompt. |
| `limit` | The token limit categories divided, and where it came from — see [Declared and observed limits](#declared-and-observed-limits). |
| `totalTokens` | Sum of the included items' estimated tokens. |
| `categories` | One entry per category **in budget order**: `category`, `allocated` (after redistribution from earlier categories), `used`, `items`, `truncated`, `estimated`. `estimated` is always `true` for assembly: the figure is the estimator's, never a provider measurement. |
| `included` | One entry per item: `category`, `kind`, `path`, `lineRange`, `tokens`, `rootPath`, `provenance`. `kind` is the source label the prompt header shows (`explicit_file`, `retrieved_file`, `agent_instruction`, `project_rule`, `hook_context`, `repository_map`, `symbols`, `plan_state`); your own message is listed as `user_prompt` with no path. `category` is the budget bucket, which is not always the same thing: a search hit is `kind: retrieved_file` but `category: search_results`. The entry records where the item came from and how big it was, never its text. |
| `excluded` | One entry per item left out or shortened: `reason`, `path`, `lineRange`, `category`, `detail`. |
| `conversationSummary` | The #55 summary this turn sent, when it has one: `tokens`, `messages`, `throughSeq`, `factsOnly`. It is not an `included` item because it travels inside `Recent conversation:`; `null` when there is no summary. |
| `symbolProvenance` | For the symbols block, `total`, `languageServer` and `heuristic`. The one `included` entry for the block can only carry the weakest provenance; these are the counts behind "12 symbols (4 heuristic)". |

`rootPath` is the repository root that owns the path: `""` means the repository
root itself, `"packages/api"` a nested root from the repository map. It is
`null` for content with no file identity (the map and symbols blocks, hook
context, a carried plan) and for a file whose owning root could not be resolved,
including when no repository map could be loaded.

`lineRange` is `null` for every item today. The field and the range-merge
machinery exist, but no collection source produces a line range yet: a search
hit contributes the whole file, and the symbols block is one rendered block.
Until a source does, `deduplicated_into_range` records cannot appear either.

`provenance` is the weakest source behind an included item: `language_server`
when a language server supplied the symbols the block shows, `heuristic` when
only source-text matching did, and `null` otherwise. Only the symbols block
carries one today; a file item, an instruction file and hook context are `null`.

#### Exclusion reasons

| Reason | What it means | Produced today? |
|---|---|---|
| `restricted_path` | The path matched `restricted_patterns`, at collection or at read time. It was never opened. | Yes |
| `category_budget_exhausted` | The item did not fit its category's share. Once one item in a category does not fit, every lower-priority item in that category is dropped too, so the dropped set is the tail of the category's priority order, not a scatter. | Yes |
| `truncated` | The item **is** included, but shorter: the repository map or the symbols block was re-rendered at a smaller ceiling to fit the category's share. `detail` names the ceiling. | Yes |
| `too_large` | A whole-file read was refused because the file exceeds `max_file_bytes`. | Yes |
| `binary` | A whole-file read was refused because the file looks binary. | Yes |
| `deduplicated_into_range` | A line range was merged into another range for the same path, or absorbed by a whole-file item. | Not yet: nothing produces a line range. |
| `user_removed` | You removed an optional item in a paused pre-send review, or removed the conversation summary. It applies to that turn only. | Yes, spec 27 |
| `path_not_selected` | The session's path restriction admits only selected paths, and this one is not among them. `AGENTS.md` is exempt by design. | Yes, spec 27 |
| `missing` | A pinned file no longer reads: it was deleted or became unreadable. The pin stays in the session. | Yes, spec 27 |

`secret_bearing` and `total_budget_exhausted` are not produced by this version;
they arrive with the stages that generate them. A file that is simply missing or
unreadable, and a path that resolves outside the repository root, produce **no**
record unless it is a pin — the enum has no reason for an ordinary missing file,
and inventing one would be worse than the omission. A pinned file is different:
its absence is the user's standing request failing, so it is recorded as
`missing`.

#### Declared and observed limits

`limit.source` says how `limit.tokens` was decided:

- `declared` — `context_token_budget` for the selected provider and model, used
  unchanged: `safetyMarginBps` is `0` and `window` is `null`. **This is what
  every turn records today.** `context_token_budget` is a packing budget, not a
  provider's context window, so subtracting a safety margin from it would only
  shrink a number that is already well below the window.
- `observed` — a measured provider window: `window` is the window, and
  `limit.tokens` is that window minus `safetyMarginBps` of it
  (`context_budget.safety_margin`, 10% by default). No capability profile is
  measured yet, so nothing produces this.

The limit bounds the assembled repository context and your message only. The
system prompt, the tool definitions and the `Recent conversation:` block are
added outside it, so the request body the provider sees can be larger than
`limit.tokens`.

#### When content is `category_budget_exhausted`

That reason means the category ran out of its share, not that the file was
refused. Check `categories` first: if that category's `used` equals its
`allocated`, the share is what bound it; if `allocated` is already above the
category's nominal percentage, earlier categories released their remainder and
it still was not enough.

Any of these changes it, in the order that is usually cheapest:

1. **Name the file in the prompt.** A file that search found is
   `category: search_results`; naming it (or adding it with **+**) collects it
   as `category: file_ranges`, which has its own, larger share. This is the one
   fix that needs no config edit.
2. **Raise that category's share** in your user config, lowering another by the
   same amount so the shares still sum to at most `1.00`. A repository can only
   lower a share and raise the margin, so this edit belongs in your own config,
   not in `.damaian/config.conf`.
3. **Raise `context_token_budget`** for the provider and model. This raises
   every share proportionally, and it raises what every turn costs, because the
   context is re-sent and re-billed each turn.
4. **Set a share to `0`** to turn a category off deliberately. Its items then
   appear as `category_budget_exhausted` and its whole share is released to the
   categories after it.

If the dropped item is an `AGENTS.md` file, remember the truncation order: the
most specific nested file is kept and the root file or a root project rule is
dropped first, because nested instructions override broader ones. If it is a
semantic search hit, expect it to be dropped before a keyword hit.

### Session logs

One JSONL file per conversation, `sessions/<session-id>.jsonl`, appended as
events. This is where **message content** lives.

Event types: `session_created`, `session_renamed`, `task_created`,
`task_status_updated`, `task_superseded`, `task_usage_recorded`,
`message_appended`, `steering_note_added`, `action_started`, `action_finished`,
`browser_diagnostics_approval_updated`, `conversation_rewound`, `plan_created`,
`plan_revised`, `plan_resumed`, `plan_step_updated`, `plan_approved`,
`question_asked`, `question_answered`, `provider_fallback`,
`session_mode_set`, `web_diagnostic_recorded`, `finding_recorded`,
`finding_status_changed`, `worker_spawned`.

Every event carries a monotonic `seq`. Events written before that field existed
are numbered by line order on read, which is their append order, so old
sessions need no rewrite.

Find the most recently touched session:

```bash
ls -t "$DATA_DIR"/sessions/*.jsonl | grep -v '\.progress\.jsonl$' | head -1
```

Replay a conversation:

```bash
jq -r 'select(.eventType=="message_appended") | "[\(.payload.role)] \(.payload.content)"' "$SESSION_FILE"
```

Follow task state, which is how you spot a turn that died mid-flight — a task
whose last status is not `complete`, `failed`, `cancelled`,
`tool_budget_exhausted` or `token_budget_exhausted` never finished. The
exception is `waiting_for_approval`: a task paused on a proposal (a command, a
plan review, a provider fallback, a question the assistant asked, or a
`round_budget` pause whose file is `chat/pending/roundbudget_…`, see
[A turn stopped early](#a-turn-stopped-early-which-budget-ran-out)) is waiting
for you, not dead:

```bash
jq -r 'select(.eventType=="task_status_updated") | "\(.payload.task.status // .payload.status) \(.payload.task.id // .payload.id)"' "$SESSION_FILE"
```

The `.task //` fallbacks are not defensive padding: `task_status_updated`
genuinely appears in two shapes. `await_approval` wraps the task alongside the
proposal it is waiting for, and the plain status write does not. Both are
current.

**A waiting question reads as a different link, not a different status.** Both
an approval and a question leave the task in `waiting_for_approval`; the newest
status event says which by the key it carries. `pendingApproval` has `kind` and
`proposalId`, where `kind` is `command`, `patch`, `plan`, `browser_diagnostic`,
`mcp_tool`, `provider_fallback` or `round_budget`. `pendingQuestion` has only
`questionId`, and nothing else. A
question never writes `pendingApproval`, and `/api/run-command` or
`/api/reject-command` with a question id runs nothing:

```bash
jq -r 'select(.eventType=="task_status_updated") | .payload |
  if .pendingQuestion then "question \(.pendingQuestion.questionId)"
  elif .pendingApproval then "approval \(.pendingApproval.kind) \(.pendingApproval.proposalId)"
  else "nothing" end' "$SESSION_FILE"
```

The question itself lives in two events, not in the status link.
`question_asked` is written once where the card appears and carries
`questionId`, the question, `options` and `allowFreeText`; `question_answered`
is written once with `answer` and `selectedOption` (absent for free text). An
asked question with no answer event is open; an answered pair is never re-asked,
and a second answer is refused. Both are read through the active events, so a
rewind past the question drops them:

```bash
jq -r 'select(.eventType=="question_asked" or .eventType=="question_answered") |
  "\(.seq) \(.eventType) \(.payload.questionId)"' "$SESSION_FILE"
```

Every line is parsed as JSON on read, and a line that does not parse is
**discarded rather than half-read** — a truncated tail cannot turn into a
phantom message. The count of discarded lines is available
(`SessionStore::unreadable_event_count`) and the recovery classifier audits it
as `session_log_truncated_tail`, so a torn write shows up as evidence of the
crash rather than as silent corruption. `list_sessions` still returns an empty
list rather than failing when the directory is unreadable
(`SessionStore::list_sessions` in
[session.rs](../crates/workspace-engine/src/session.rs)), so an
unreadable *directory* is the case that stays quiet. To check a file yourself:

```bash
jq -e . "$SESSION_FILE" > /dev/null
```

### Plans and step evidence

A turn that needed more than one piece of work writes a plan. Five event types
carry it, and reading them in order is how you reconstruct what the panel
showed:

| Event | Written when |
|-------|--------------|
| `plan_created` | the model proposed the plan |
| `plan_revised` | the user edited it at the review gate |
| `plan_resumed` | a plan was carried onto a new task after a token stop, a tool-budget stop, or a crash-recovery `Continue` |
| `plan_step_updated` | one step changed status or gained evidence |
| `plan_approved` | the user cleared the review gate |

The full-plan events carry the whole step list; `plan_step_updated` carries one
step under `.payload.step`. On replay the step list comes **only** from the
first three — an update naming a step the plan does not have is ignored, so the
log cannot grow a plan nobody wrote.

See every plan event in order:

```bash
jq -r 'select(.eventType | startswith("plan_")) | "\(.seq) \(.eventType)"' "$SESSION_FILE"
```

Read the final state of each step, which is what the panel renders:

```bash
jq -r 'select(.eventType=="plan_step_updated") | "\(.payload.step.id) \(.payload.step.status) evidence=\(.payload.step.evidence | length)"' "$SESSION_FILE"
```

A step's `evidence` array is what Damaian itself observed, never what the model
claimed. Four kinds:

- `{"kind":"commandExit","markerId":…,"exitCode":0}` — a command ran. **A
  missing `exitCode` is not a zero**: the process was killed or signalled and
  reported no code, which blocks the step rather than passing it.
- `{"kind":"patchApplied","markerId":…,"files":[{"path":…,"appliedHash":…}]}` —
  a patch landed. `appliedHash` is what was actually written, which differs
  from the proposal's hash when the user accepted only some hunks.
- `{"kind":"fileRead","path":…,"hash":…}` — a file was read.
- `{"kind":"findings","refs":[…],"failing":2}` — the findings a check produced
  (see [Findings](#findings)), recorded beside the `commandExit` it explains.
  `failing` counts the `error` ones. It never decides the step's status; the
  exit code does.

`markerId` ties evidence back to the `action_started` / `action_finished` pair
for that call, so you can find the command's stored output under
`commands/output/`.

A step showing `"status":"completed"` with `"evidence":[]` is **completed
unverified** — it finished with nothing observable behind it. That is a
reported outcome, not a missing record. A step showing `"status":"blocked"` had
a command that did not succeed; the steps after it stay `pending` on purpose.

Rewinding takes plans and approvals with it: `read_task_plan` and
`read_plan_approved` both read only events after the last
`conversation_rewound`, unlike token usage, which is read from every event
because what was billed was billed regardless of where the conversation now
sits.

### Verification: which checks ran, and why one was skipped

After a patch is applied in Code mode, the verification loop writes these
session events:

| Event | Meaning |
|-------|---------|
| `verification_due` | a settled Code-mode patch needs verification; names the task and patch |
| `verification_started` | one attempt, with its number and the selected checks |
| `verification_check` | one check's outcome, or its skip reason, plus the hash of every changed file when it ran |
| `verification_finished` | the attempt's outcome (`passed`, `failed`, `repair_limit_exhausted`, `no_repair_applied`, `no_checks_available`, `only_documentation_changed`) |

A check is identified by its command and root, so `cargo test` at the repository
root and in `crates/engine` are two checks. Each appears in exactly one of
passed, failed, skipped or stale in the report; **an unrun check is never
passed**.

**Where the full output lives.** A `verification_check` event is a summary. The
findings carry an `originRef`, and the command's complete `stdout.log`,
`stderr.log` and summary are under `commands/` in the data directory (see
[Command output](#command-output)). The report itself lists only findings and
exit codes.

**A check says `stale`.** The files it saw changed after it ran: a verified file
was rolled back, a later patch touched it, or you edited it outside Damaian. The
loop records the hash of every changed file with each check run and compares it
when the report is built. A stale check is not rerun automatically and is not
reported as passed; run the verification again once the files have settled.

**The check card keeps asking.** The loop proposes each check through the
normal command policy, so the fix is the same as for any command: choose
**Allow Always** for that check (exact command, root-qualified in a sub-root) or
add the command to `command_allowlist` for the repository. A repository with
`require_approval_for_all_commands` keeps asking by design.

**A browser check was skipped.** `no running target` means no stored diagnostic
target existed, or the page could not be reached. Start the app and inspect the
page once, then run verification again. Without a session grant for browser
diagnostics the check is skipped with the policy reason instead of running
unasked.

**The task failed with the repair limit exhausted.** The report lists what still
fails; the agent's repairs did not fix it within `agent_tool_retry_limit`
(default 2). Raise the limit if the work genuinely needs more, or fix the
finding and apply a patch — verification runs again when that patch settles.

### The plan says a step is still open after the turn ended

That is the recorded truth, not a stuck turn. When a turn ends while a step is
still open, Damaian attaches the evidence the step accrued and leaves its status
alone: a step is never closed because the model stopped talking. The panel calls
it **not finished** and summarises the plan as "Turn ended with N steps not
marked done." The answer the turn produced is unaffected.

Two audit records explain what happened:

| Event | Meaning |
|-------|---------|
| `plan_reminder_sent` | the engine sent its one reminder at a completion that would otherwise have ended with open steps; `openSteps` is the count it named |
| `plan_left_open` | the turn truly ended with open steps; `openSteps`, `reminderSent` and `stopReason` (`answered`, `tool_budget`, `token_budget`, `cancelled` or `failed`) say why |

The reminder is also an ordinary message in the session log with
`"kind":"plan_reminder"` on its `message_appended` payload. It is a user-role
message in the request so the model reads it at the end, but it is not something
the user typed: the conversation view renders it as a system line, and the next
turn's prompt seed skips it.

A plan you declined at the review gate has its remaining steps marked `skipped`,
which is why a declined plan produces neither record and never gets a reminder:
the user's decision, not the model's claim, settled it.

### Browser diagnostics

Each `inspect_web_page` or `run_web_scenario` that returned a report appends
one `web_diagnostic_recorded` event. Its payload is the whole record the card
renders: `id`, `taskId`, `tool`, `url`, `recordedAtMs`, and `report`
(`text`, `artifacts`, `is_error`, `details`, `via`). The envelope is
camelCase and the report is snake_case. The session log is otherwise stored
unredacted, but this record is built from the **redacted** report, so what
you see here is what the model and the UI saw. A runner that returned an
error instead of a report writes no record, only the tool message.

```bash
jq -c 'select(.eventType=="web_diagnostic_recorded") | .payload | {tool, url, via: .report.via, is_error: .report.is_error, page_errors: .report.details.page_errors}' "$SESSION_FILE"
```

Replay goes through the same active-event window as plans, so a rewound turn's
diagnostics disappear from the UI with it. They stay in the file.

Screenshots are under `web-diagnostics/<session-id>/<task-id>/run-<ms>/` in
the data directory. The record's artifact `path` is relative to the data
directory. **Reveal in Finder** (`POST /api/reveal-web-diagnostic-artifact`)
and the thumbnail endpoint only resolve canonical paths inside
`web-diagnostics/`. An absolute path, a `..`, or anything else is refused.

**Two shapes of runner.** A browser server that returns the companion's JSON
object (`page_errors`, `console`, `failed_requests`, `dom_summary`, …) gets
typed `details`. The model reads Damaian's own summary, `Browser diagnostic
found …`, and the card lists each problem. Any other browser server's result
is kept as text: `details` is `null`, the model reads the raw text as before,
and the card has only a header, any screenshots and a **Diagnostic output**
disclosure. **To tell them apart, check the card for problem lists**, or check
`.payload.report.details` for `null`. Nothing is wrong with the second shape.
It is the fallback for a server Damaian does not know.

**"Found" is not "failed".** A header that says *found N page errors* means
the diagnostic worked and the page is broken. *Browser diagnostic failed* means
the tool itself failed: MCP `is_error`, or the companion's `"error": true`
(for example, a scenario step with invalid arguments). Only the second counts
toward `agent_tool_retry_limit`, so a model that keeps inspecting a broken
page is not stopped for it.

Known quirks:

- **Each load-time error is listed two or three times after a scenario.** The
  legacy `run_scenario` adapter adds a `goto` to the steps, and the companion
  and the model may also navigate, so the page loads more than once and each
  load throws again. The count in the header is inflated. The error is real.
- **The diagnostic's arguments are not recorded.** For an approval-gated call
  they are in the approval message (`message_appended`, "The assistant wants
  to run …"). A loopback inspection runs without approval, so for that call
  they are not written anywhere. That matters when a report is missing its
  screenshot or DOM summary, because the model may have turned `capture` off.

### Findings

A command the assistant runs inside a turn (one Damaian ran without asking, or
one you approved) and every browser diagnostic turn their failures into findings,
which the **Findings** panel shows — except an exploration command (see below).
A command run from outside a conversation (`/api/run-command` with no session)
records none.

**Parsed structurally:**

| Check | Source |
|-------|--------|
| `cargo build`, `cargo check`, `cargo clippy` | `compiler`, or `lint` for `clippy::` lints |
| `cargo test` (its compile errors too) | `test`, plus `compiler` for compile errors |
| `biome check`/`lint`/`ci`/`format`, `npm`/`pnpm`/`yarn run lint` and `run lint:*` | `lint` |
| browser diagnostics with the companion's typed report | `browser_console`, `browser_network`, `browser_scenario` |

**Everything else falls back to one generic finding per failed run**,
including `cargo nextest run`, an ESLint `npm run lint`, and a parsed check
whose parser found no error in output that failed. **How to tell:** source
`command`, shown in the panel as **Unparsed checks**, is the generic fallback
and nothing else is. Its summary names the command and its first output line,
and its details are the last 40 lines of stderr then stdout. A cancelled check
gets no generic finding, because it never reached a verdict.

**A generic command finding is not always a code defect.** It often means the
command could not run at all — a missing tool, a `PATH` difference between your
shell and the app (see [The app does not see your shell's environment](#the-app-does-not-see-your-shells-environment)),
or a timeout. When every finding you ask to repair is a `command` finding with
no location, the repair confirmation says so before sending. Read the prompt
and cancel if the fix belongs to your environment rather than the repository.

**A failed command recorded no finding.** Its purpose was **exploration** — a
search, a listing, a text read, or a git read, including pipelines of those.
`grep` exits 1 when nothing matches and `ls -d` exits 1 for a path that does
not exist; Damaian treats either as the answer the assistant asked for, keeps
the output in the tool result, and files no finding. Until 2026-10-24 a loop
(`for …; do …; done`) or a leading `set +e` made such a command **other**, so
older sessions may still hold an **Unparsed checks** error that is only a
failed look-around; dismiss it. The list of commands that
count as exploration is in [USER_GUIDE.md](USER_GUIDE.md#findings). The
purpose is decided once, when the command is proposed, and stored: `purpose`
on the `command_executed` and `stored_command_executed` audit events, the
`PURPOSE` field of the stored proposal and `summary.dcmd`, and the plan's
`CommandExit` evidence. A `check` or `other` command still records findings,
and a record written before purposes existed reads as `other`, so it keeps
the old behaviour. To see what one execution was recorded as:

```bash
jq -c 'select(.eventType=="command_executed") | {command, exitCode, purpose}' "$DATA_DIR/audit/events.jsonl"
```

**Where full output lives.** A finding's `details` is a bounded excerpt (at
most 4096 bytes, redacted). For a command, `originRef` is the execution id,
and the stored output is
`$DATA_DIR/commands/output/<originRef>/stdout.log` and `stderr.log`. That is
the runner's tail-truncated, redacted output, the most Damaian keeps. For a
browser finding, `originRef` is the `web_diagnostic_recorded` record's `id`.

**The two events.** `finding_recorded` carries the whole finding as its
payload (camelCase: `id`, `source`, `severity`, `summary`, `details`, `range`,
`taskId`, `originRef`, `status`, `code`, `fileHash`, `createdAtMs`).
`finding_status_changed` carries `{findingId, status}` when the user dismisses
or restores one.

```bash
jq -c 'select(.eventType=="finding_recorded" or .eventType=="finding_status_changed") | {seq, timestampMs, eventType, id: (.payload.id // .payload.findingId), source: .payload.source, status: .payload.status, summary: .payload.summary}' "$SESSION_FILE"
```

**`stale` is never in the log.** It is derived on read: an `open` finding
whose `fileHash` no longer matches its file, or whose file is gone, reads as
`stale`. Reverting the file makes it `open` again, with no event. A dismissed
or fixed finding keeps its status whatever happens to the file, and a finding
with no `fileHash` is never stale. Setting `stale` through
`/api/finding-status` is refused. Reads use the same active-event window as
plans, so a rewind takes the findings recorded after its point with it.

**The findings list cannot be opened.** The list is reached from the repo
bar's **Fix N errors** / **Fix N warnings** chevron, which exists only while
the session has an open error or warning. With every finding dismissed, fixed,
stale or informational there is nothing to open, so a dismissed finding cannot
be restored from the UI until a check records a new open one. Nothing is lost:
the `finding_recorded` and `finding_status_changed` events above still list
every finding and its status.

**A location is missing.** A finding gets a `range` only when the tool printed
one, it is relative and inside the repository, and the file exists when the
finding is recorded. The printed path is tried against the directory the
command ran in first, then each parent up to the repository root, and the
first existing file wins. So a check run in a sub-root (see
[Repository roots and the repository map](#repository-roots-and-the-repository-map))
keeps its locations. An absolute path (the standard library, a registry
crate) or a `..` path is dropped. A browser console location maps to a file
only for a loopback URL whose path matches exactly one repository file outside
`node_modules`. Percent-encoded paths are not decoded and never match.

### Command output

Approved command executions store their full output on disk
(`CommandStore::save_execution` in
[validation.rs](../crates/workspace-engine/src/validation.rs)):

```text
commands/output/<execution-id>/stdout.log
commands/output/<execution-id>/stderr.log
commands/output/<execution-id>/summary.dcmd
```

Both streams are secret-redacted and truncated to `max_command_output_bytes`
before being stored, so a suspiciously short log may be a truncation, not an
early exit. Check `exitCode` on the matching `command_executed` audit event.

A command that was killed rather than exited reports `exitCode: -1` on that
audit event and a `TERMINATION` of `timed_out` or `cancelled` on its
`summary.dcmd`. It is killed when it passes `command_timeout_secs` (default
600) or when the turn is stopped; `command_timeout_secs` is restrict-only, so a
repository may lower that deadline but not raise it. While it runs, its output
is streamed to the interface as it arrives and redacted a line at a time; the
stored log above is still the authoritative whole-output redaction, so treat a
missing secret in the live view as expected rather than as proof.

## Turns, recovery and workers

How to read what a turn, a crash, a worker or a rewind left behind. Each
section names the log events it relies on; [Logs](#logs) describes the logs
themselves.

### A turn stopped early: which budget ran out

What happens at `agent_max_tool_rounds` (default 8) depends on whether a token
ceiling is set ([spec 61](specs/61_continue_the_stopped_turn/proposal.md)).
That budget counts **action rounds**. A round in which every call was a read —
`read_file`, `list_directory`, `search_content`, `search_codebase`,
`read_git_status`, `read_git_diff`, `list_worktrees` or `read_git_log` — and no
call repeats an earlier one is a **read-only round**, charged to
`agent_max_exploration_rounds` (default 24, ceiling 32) instead
([spec 59](specs/59_exploration_round_allowance/proposal.md)). Spending that
allowance never pauses a turn; later reads are action rounds.
`agent_max_exploration_rounds=0` restores the behaviour where every round is
charged to `agent_max_tool_rounds`.

- **No `agent_max_task_tokens` (the default).** When the turn reaches its
  action-round budget and the model still wants a tool, the turn **pauses**
  instead of stopping. The task goes to `waiting_for_approval` on a
  `chat/pending/roundbudget_<id>.json` proposal with
  `PendingApprovalRef.kind = "round_budget"`, and the conversation names how
  many action rounds it used and how many read-only rounds it had used.
  **Continue — N more** grants the same turn another
  `agent_max_tool_rounds` and replays its saved messages, tool results and
  plan; **Answer now** makes one final call with tools dropped. A turn never
  runs past 64 action rounds in all: at that cap only Answer now is offered.
- **With `agent_max_task_tokens` set.** The turn continues on its own,
  another `agent_max_tool_rounds` at a time, and records `turn_continued` each
  time, until the token ceiling stops it or it reaches 16 action rounds. At 16
  rounds it pauses as above.

### Notes: what the assistant kept, and why a later turn knows something

A turn can write short `turn_notes` during an investigation (spec 65). The
notes are pinned into every later request of that turn, even after older rounds
have been dropped, and they carry into the next turn of the session. That is why
an answer may say it relied on its notes, and why a later turn can name a file
or a function it did not read again. The notes are bounded by
`agent_max_turn_notes_chars` (default 8,000, `0` off); an append over the bound
is refused with the current size and a suggestion to `replace`.

Two model-facing blocks explain what left the request window:

- `Earlier rounds of this turn were elided … Calls already made, oldest first`
  names the calls that were dropped. A call into a registered worktree names
  that worktree, so the same path read in two checkouts appears twice.
- `The window is full. With your next request, round N leaves it` names the
  oldest round still in view and its calls. The model gets one round to record
  anything it still needs.

Both are normal, not errors: they are how the model avoids re-reading what it
already saw. A read it repeats anyway is still charged as an action round.

In the session log, `turn_notes` events carry `taskId`, `replace`, `carried`,
`text` (already redacted), `chars` and `totalChars`; the latest event for a task
is that task's notes. A carry into a later turn is another `turn_notes` event
with `carried: true`, written before that turn's first request. The audit log
has `turn_notes_recorded` (sizes only, never text) and `turn_notes_carried`
(the source task id and size). A turn that took no notes is indistinguishable
from one where the model chose not to; check `agent_max_turn_notes_chars=0` if
you expected the tool and it was not offered at all.

Two terminal statuses look alike in the UI and mean different things:

- `tool_budget_exhausted` — written only after **Answer now**, when the final
  call still asked for a tool instead of answering. The UI shows **Tool budget
  exhausted** with **Ask again**, which starts a new turn from the stopped
  task's request, and the reply names both the action and read-only counts.
- `token_budget_exhausted` — the turn reached `agent_max_task_tokens`, which is
  **per turn**, not per session. Nothing is wrong; it cost more than the cap
  allows.

A paused turn is in the audit log as `round_budget_paused` (`proposalId`,
`roundsUsed`, `roundsTotal`, `explorationRounds`, `explorationLimit`). The
answer is `round_budget_decision` with `decision` `continue` or `answer_now`,
and a Continue also writes `turn_budget_extended` with the rounds granted and
the running total:

```bash
jq -c 'select(.eventType|test("^(round_budget_|turn_budget_extended|turn_continued)"))' "$DATA_DIR/audit/events.jsonl"
```

A pause survives a restart: crash recovery reattaches the `roundbudget_`
proposal rather than treating the task as interrupted. If its file under
`chat/pending/` is gone, recovery fails the task with that reason
(`pending_approval_unavailable` in the audit log).

Tell the terminal statuses apart in the log:

```bash
jq -r 'select(.eventType=="task_status_updated") | .payload.task.status // .payload.status' "$SESSION_FILE" | sort | uniq -c
```

A stop is recoverable without losing work. After a token stop, raise the cap
and ask again; after a tool-budget stop, press **Ask again**. That button sends
only the stopped task's id, and the engine builds the new turn from the task's
own `user_prompt` as `Continue this earlier request: <that request>` — the
client never supplies the text, and a fixed "continue debugging" prompt is not
sent. The next turn reads the stopped task's plan, carries it forward as
`plan_resumed`, and keeps the evidence the earlier steps had already gathered.
The same carry happens when crash recovery's `Continue` supersedes an
interrupted task. Confirm the carry happened:

```bash
jq -r 'select(.eventType=="plan_resumed") | "\(.payload.taskId) resumed from \(.payload.resumedFrom)"' "$SESSION_FILE"
```

If no `plan_resumed` appears, the previous task either had no plan or had
nothing outstanding — a plan whose steps all finished has nothing to resume.

### The assistant seems to have forgotten something you said earlier

Damaian no longer clips a conversation to its last eight messages silently. When
a conversation grows past its **conversation window**, older messages are
represented by a summary (or, with automatic compaction off, left out with an
explicit notice). Two things can make the assistant lose a constraint:

1. **The summary does not mention it.** The transcript shows a boundary marker
   where the model's view begins — *"N earlier messages are represented by a
   summary the model receives instead"*. Open it and read the **Requirements and
   constraints** and **Decisions** sections. If the constraint is missing, the
   summarisation call did not record it; restate the constraint in your next
   message. The summary is a record, not an authority over what you say now.
2. **Automatic compaction is off.** The marker says the earlier messages were
   **not sent and not summarised**. The request also carries an
   `[Damaian: … earlier messages … were not sent …]` line. Compact by hand from
   the marker's **Compact now** button, or turn automatic compaction back on in
   **Settings › General › Conversation**.

Find out what happened from the session log. `$SESSION_FILE` is resolved at the
top of this document:

```bash
# Every compaction boundary, newest last.
jq -r 'select(.eventType=="conversation_compacted") | "seq \(.seq): through \(.payload.throughSeq), \(.payload.messageCount) messages, format v\(.payload.formatVersion), run \(.payload.modelRunId // "none")"' "$SESSION_FILE"

# A compaction that could not run, and why.
jq -r 'select(.eventType=="conversation_compaction_failed") | "seq \(.seq): \(.payload.reason)"' "$SESSION_FILE"
```

The same events appear in the audit log as `conversation_compacted` (ids,
counts, the run id, token figures and whether the summary is facts-only) and
`conversation_compaction_failed` (the reason). The audit log never holds the
summary's text.

**The facts in a summary are not the model's recollection.** Files changed, plan
steps, approval decisions and validation results are read from the session log;
if one of those is wrong, the log is the authority and the summary is a rendering
of it. Check the plan panel, the approvals in the transcript, or
`.damaian/chat/` for the stored proposal.

**A compaction failure never blocks a turn.** If the summarisation call is
refused or fails, Damaian keeps a facts-only summary and leaves the
model-written half empty; the marker shows the summary with no narrative. If even
the fact half cannot be written, the request uses the window with the elision
notice. Nothing is lost from storage in either case — the full transcript is
still in the session, and rewinding past the boundary restores the model's view
of every message.

### After a crash: what recovery decided

A task that was mid-action when the process died leaves an `action_started`
with no matching `action_finished`. That pairing is the crash signature, and it
is what the classifier reads. Find the dangling ones:

```bash
jq -rs 'map(select(.eventType=="action_finished").payload.markerId) as $finished
        | map(select(.eventType=="action_started"
                     and (.payload.markerId | IN($finished[])) == false))
        | .[] | "seq=\(.seq) \(.payload.action) ref=\(.payload.ref) sideEffecting=\(.payload.sideEffecting) task=\(.payload.taskId)"' "$SESSION_FILE"
```

Pair by **`markerId`, not by action name.** A session that ran the same tool
twice has two `run_command` markers, and one can be closed while the other is
not — matching on the name alone reports the wrong one, or reports a completed
action as dangling.

To see the raw pairing instead:

```bash
jq -r 'select(.eventType=="action_started" or .eventType=="action_finished")
       | "\(.eventType) \(.payload.markerId) \(.payload.action) sideEffecting=\(.payload.sideEffecting|tostring)"' "$SESSION_FILE"
```

`|tostring` there is not cosmetic. `jq`'s `//` operator treats `false` as
absent, so the obvious `.payload.sideEffecting // "-"` prints the same thing
for a read-only action as for a missing field — on the one field that decides
whether the action can be retried. `action_finished` does not carry the field
at all and prints `null`.

`sideEffecting` on the `action_started` line is what decides everything. Two
outcomes, per
[spec 17](specs/17_durable_task_state_and_crash_recovery/proposal.md) §5.4:

- **`interrupted`** — nothing observable outside the process was in flight, so
  the task can be picked up again. A cut model call lands here: the call may
  have been billed, but repeating it costs another call rather than repeating an
  effect.
- **`unknown_external_outcome`** — a side-effecting action was in flight and
  **there is no way to find out whether it landed.** A command may have deleted
  files, a patch may have written some of them, an MCP call may have posted
  something. Damaian will not repeat it, and will not repeat it even if you ask:
  `resume` refuses this classification outright. Inspect what actually happened
  and then mark the task failed or abandon it.

That refusal is deliberate and lives in the engine rather than the UI. Nothing
you can be told makes an unknown outcome knowable, so there is no informed
choice to offer — probing the external system to find out is explicitly out of
scope, because the probe itself can have effects.

Recovery decisions are in the **audit log**, not the session log:

```bash
jq -r 'select(.eventType|startswith("task_recover")) | "\(.eventType) \(.taskId) \(.classification // "") \(.decision // "") \(.outcome // "") \(.reason // "")"' "$DATA_DIR/audit/events.jsonl"
```

`task_recovered` carries the classification and the evidence it was made on
(`danglingAction`, `danglingRef`, `danglingSeq`, `autoResumePermitted`);
`task_recovery_decision` carries what was then done about it and whether it was
allowed or refused.

**The sweep runs once per app start**, on the first `GET /api/recovery` from
the web UI ([spec 45](specs/45_crash_recovery_prompt.md) §5.3). Once, because
each pass appends a `task_recovered` event per non-terminal task, and a crash
ends the process anyway. It classifies every session, reattaches pending
approvals, and authorizes — but does not run — the tasks the classification
marked safe. Every card the UI then shows is `GET /api/recovery` output;
`POST /api/recovery-decision` applies one choice, re-classifying first, so a
decision cannot be made on a claim the webview supplied.

The running app's API cannot be reached from curl (see
[The API token blocks curl](#the-api-token-blocks-curl)), so read a real
session's verdict from the two audit events above. To watch the sweep's JSON,
start the manual-inspection server described there, which is seeded with three
tasks left mid-flight, and in another terminal:

```bash
curl -s -H "x-damaian-api-token: $TOKEN" http://127.0.0.1:4899/api/recovery | jq
```

A task that was resumed shows as `preparing_context` afterwards. That is
authorization, not execution: the turn re-runs only when the user presses
`Continue`, which re-sends the recorded prompt as an ordinary turn. It also
gets a `task_superseded` event, which is what stops the next launch asking about
it again — the task never becomes terminal, because its work moved to the new
turn:

```bash
jq -r 'select(.eventType=="task_superseded") | "\(.payload.taskId) — \(.payload.reason)"' "$SESSION_FILE"
```

One consequence of the first launch after upgrading: a task left awaiting
approval by a version older than spec 17 recorded its status but not *which*
proposal it was waiting on. There is nothing to reattach and nothing safe to
guess, so it is failed with that stated reason rather than shown as an approval
card rebuilt from partial data. **No stored proposal is deleted** — the patches
and commands themselves remain on disk and stay usable.

### A commit was refused, or an interrupted commit's outcome is unknown

A commit refusal is a value, not a guess. The dialog shows the sentence and the
code, and nothing was committed unless the commit object is reported as created:

- **`commit_head_moved`** — `HEAD` moved after the preview. Nothing was committed; prepare the commit again. The same code covers a branch switch and a ref change during the commit.
- **`commit_content_changed`** — a selected file no longer has the bytes you reviewed, or its mode changed (a chmod after the preview). Nothing was committed. Look at what changed the file or its mode (a formatter, a command, another editor), prepare again, and review the new diff.
- **`commit_rename_incomplete`** — a rename's target was selected without its source deletion (or the reverse). Select both paths, or neither; committing only the new path would leave the old file behind. In the dialog the pair's checkboxes move together, so this usually means a client sent the selection itself.
- **`commit_hooks_changed`** — a Git hook was added or edited after the preview, including a same-length edit: the report stores each hook file's SHA-256. Prepare again so the hooks you approve are the hooks that run.
- **`commit_hook_failed`** — the named hook exited non-zero; the response carries its redacted, truncated output. A failing `pre-commit`, `prepare-commit-msg` or `commit-msg` aborts before a commit object exists. A failing `post-commit` does **not** undo the commit; the SHA is reported with a warning.
- **`commit_message_rewritten`** — a message hook changed your text. The rewritten text is returned and prefilled; review it and approve again. Nothing was committed on the first attempt.
- **`commit_secrets_require_override`** — the real bytes contain something the scanner flags. The displayed diff is redacted, so the match may not be visible; review the file and tick the override to commit it anyway. The override applies to that one commit and is audited.
- **`commit_not_allowed`** — the session is not in Code mode, or the permission profile has `allow_git_commits=false` (Read-only does). Switching mode does not help a profile refusal.
- **`commit_restricted_path`** — the named path is outside the checkout or matches `restricted_patterns`. It cannot be committed.
- **`commit_not_supported`** — the path is a submodule or a nested repository. It is shown with that label and its checkbox stays off.

The audit trail records `commit_prepared`, `commit_refused`, `commit_secret_override` and `commit_created` in `audit/events.jsonl`. `commit_created` carries the SHA, branch, path and hunk counts, message length, hook outcomes and whether the secret override was used — never the message or file content.

A **successful** commit can still carry a warning. The most common one is a `pre-commit` hook that reformatted files and staged them (lint-staged). Damaian commits the bytes you reviewed and leaves the hook's edit in your working tree; the warning names the hook. This is deliberate: including the hook's staging would commit bytes you did not review, and dropping it silently would lose the hook's work. If a hook leaves the private index unreadable, the warning says the staged changes could not be checked and the commit still contains the reviewed bytes.

If the app stops while a commit was in progress, the task classifies as an unknown external outcome and recovery **does not re-commit**. Open the recovery card's **Inspect**: the “Commit in flight” row shows the prepared commit, the expected old `HEAD`, the current `HEAD`, and whether the prepared commit is where `HEAD` is. If it is, the commit landed; if `HEAD` is still the old value, it did not. The durable record is `commits/<prepare-id>.json`; the marker is an `action_started` with `"action":"commit"` and `"sideEffecting":true` in the session log.

### After a crash: what was still running

A separate question from the one above, and a separate sweep. That one asks what
a *task* was doing; this one asks which **child processes** outlived the crash —
MCP stdio servers, `curl` model calls, PTY shells, shell commands, hooks,
workers and language servers (`kind` is `mcp_server`, `model_call`, `terminal`,
`command`, `hook`, `worker` or `language_server`). Damaian records each one at
spawn under `processes/`, and kills the leftovers at the
next launch ([spec 46](specs/46_process_registry_and_orphan_sweep/proposal.md)).

Each file is named `<pid>-<start-time-us>.json`, and **both halves matter**. A
PID is reused, so the start time is the evidence that the number still names the
process that was recorded. What is currently registered:

```bash
for f in "$DATA_DIR"/processes/*.json; do jq -r '"\(.kind) pid=\(.pid) pgid=\(.pgid) session=\(.sessionId) owner=\(.ownerPid)"' "$f"; done
```

A file here does **not** mean something is leaking. The normal state during a
session is one file per live child; they are unlinked when the child exits
cleanly. A file left behind after Damaian is closed means the process died
without unlinking — which is exactly the case the next launch's sweep is for.

`ownerPid` is the Damaian instance that spawned the child, and it is why a
second running instance is safe: an entry whose owner is still alive is skipped,
not killed. Two Damaian instances can run at once, so "recorded but not mine" is
not evidence of a crash.

Sweep decisions are in the **audit log**, one event per entry:

```bash
jq -r 'select((.eventType|startswith("orphan_process_")) or .eventType=="process_registry_entry_unreadable") | "\(.eventType) pid=\(.pid // "-") kind=\(.kind // "-") \(.recordedStartTimeUs // "") \(.actualStartTimeUs // "")"' "$DATA_DIR/audit/events.jsonl"
```

The parentheses around `startswith` are not cosmetic. Written as
`select(.eventType|startswith(…) or .eventType==…)`, the `or` binds *inside* the
pipe, where `.` is already the event-type string — so `.eventType` indexes a
string and `jq` aborts on the first line that is not an orphan event. It appears
to work on a log that contains nothing else, because `or` short-circuits before
evaluating the broken half.

Audit fields sit at the top level of each event, not under a `payload` object —
unlike the session log, where they are nested.

| Event | What happened |
|---|---|
| `orphan_process_killed` | The PID was alive and its start time matched. Killed, entry removed |
| `orphan_process_kill_refused` | The PID was alive and its start time **differed** — it is a different process now, so it was left alone. Both start times are recorded so the refusal is checkable after the fact |
| `orphan_process_already_exited` | Nothing to kill; the entry was removed |
| `process_registry_entry_unreadable` | A half-written entry, meaning the crash landed between creating the file and writing it. Audited rather than ignored, because it is evidence about the crash |

**A refusal is the system working, not a failure.** It means a recorded PID had
already been recycled by the operating system and now belongs to something else.
Killing it would have taken out an unrelated process — a worse outcome than the
leftover this sweep exists to clean up.

If you expected a cleanup and got nothing, check in this order: the entry's
`ownerPid` is still alive (so the entry is deliberately skipped); the sweep runs
at launch and on `SIGINT`/`SIGTERM`/`SIGHUP`, so a `SIGKILL`ed Damaian cleans up
on its *next* start rather than immediately (workers are the exception: they
end themselves at once, see [Worker sessions](#worker-sessions)); and a process started outside
Damaian was never registered, so it is not the sweep's to kill.

### Worker sessions

A worker ([spec 57](specs/57_worker_sessions/proposal.md)) is a session hosted
by its own process, the app binary re-launched with `--worker`. Its stderr is
the shell's, so a worker's refusals appear in the shell's own output (see
[Capturing stdout and stderr](#capturing-stdout-and-stderr)). Nothing is written
to a separate worker log.

**Where a worker's files are:**

| What | Where |
|---|---|
| Worktree and branch | `<source repo>/.damaian/worktrees/<slug>` on `damaian/<slug>`; `.damaian/worktrees/` is a line in the source's `.git/info/exclude` |
| What created them | the worker's own session log, first event `worker_spawned` (`gitCommands`, `baseCommit`, `includedUncommitted`, `parentSessionId`), and the same event in the audit log |
| Its conversation | `sessions/<session-id>.jsonl`, keyed by the **worktree's** repository id, not the source's. The parent's log has a `worker_spawned` event naming it |
| Live progress | `sessions/<session-id>.progress.jsonl` |
| Control socket | `workers/<first 16 hex of sha256(session id)>.sock` |
| Its process | a `processes/*.json` entry with `kind` `worker`, owned by the shell. The worker's own commands and model calls are entries owned by the worker's pid |

The board reads `GET /api/workers?repo=<source repo>`, which the running app
does not serve to curl (see
[The API token blocks curl](#the-api-token-blocks-curl)). In the app, read the
board itself, or call the route from the webview console. On the
manual-inspection server, against its own seeded repository and data
directory, the same route is:

```bash
curl -s -H "x-damaian-api-token: $TOKEN" \
  "http://127.0.0.1:4899/api/workers?repo=$(jq -rn --arg r "$REPO" '$r|@uri')" \
  | jq -r '.workers[] | "\(.group) \(.state) \(.taskStatus // "-") \(.sessionId) \(.branch)"'
```

`$REPO` there is the `repository` path the server printed. It has no workers
of its own, so expect an empty list unless you start one from its UI with
Worker sessions turned on.

Worker lifecycle events in the audit log:

```bash
jq -r 'select(.eventType|startswith("worker_")) | "\(.eventType) \(.sessionId // "-") \(.error // .pid // "")"' "$DATA_DIR/audit/events.jsonl"
```

`worker_spawned`, `worker_spawn_failed`, `worker_restarted`,
`worker_terminated` (a `SIGTERM` from the stop escalation or at quit, with the
registry's pid and start time) and `worker_worktree_removed`.

#### The board or the control strip stops updating

The shell answers one request at a time, and a turn streaming in another
session holds it for that turn's whole length. The board and the worker's
control strip then keep their last rows. After 3 seconds the board says how old
they are and that the shell answers once the turn ends; the strip shows nothing.
Workers keep running meanwhile, and only the view lags. When the other turn
ends, polling resumes within a second. Only a request that **fails** shows
"Could not refresh". This is accepted behaviour (correction C10 in
[`tasks.md`](specs/57_worker_sessions/tasks.md)). Answering the worker routes
while a turn streams is a follow-up, proposal §7. The sidebar's dots and badge
have no timer of their own, so with the board closed they can be stale until a
project is drawn, a session loads or the window regains focus.

#### An answer has handoff prompts but no *Start worker*

*Start worker* appears only on a block whose fence's info string is exactly
`worker-prompt`, in an assistant message that has finished streaming. A block
that opens with a bare fence and has `worker-prompt` as its first line inside
is deliberately not matched, so that no button lands on the wrong thing
(proposal §4). The handoff paragraph shows the model the literal opening fence
for this reason. If a model still writes it wrongly, ask it to rewrite the
prompts as `worker-prompt` blocks, or paste the text into **New worker**. If
the request produced a patch preview instead of an answer, it went to the
one-shot patch flow, whose prompt has no handoff paragraph. That happens to a
request with an edit verb and a code word (`looksLikeEditRequest` in
`app.js`) unless Worker sessions is on and the request also says *split*,
*plan*, *tasks*, *hand off*, *parallel*, *worker* or *session*. Reword it with
one of those.

#### A worker shows *Interrupted*, or *Ended*

The worker's process has gone and its session has not. *Interrupted* means the
process ended part way through a turn. Its task was then classified by crash
recovery ([After a crash: what recovery decided](#after-a-crash-what-recovery-decided))
as `interrupted` or, when a command was running, `unknown_external_outcome`.
*Ended* means it ended between turns. Common causes: the app quit or crashed
(workers never outlive it), the worker was stopped, or its session's parent was
deleted.

**Restart worker** starts a new, idle process for the same session and worktree
(`worker_restarted`). It does not continue the old turn: a worker session is
never auto-resumed or resumed in the shell (correction C5), because its
repository is a worktree and its host is gone. An approval the old process was
waiting for ended with it and its command did not run. Its
`chat/pending/<proposal-id>.json` snapshot stays on disk. Restart is refused
while a live worker holds the worktree, and once the worktree was removed.

The shell's crash-recovery card is never shown for a worker session. A **live**
worker's in-flight task is not crash debris either. `GET /api/recovery` skips a
session that has a worker record and a live `ProcessKind::Worker` registry
entry with a matching start time (correction C12), so a running worker gets no
`task_recovered` event, no `lost_to_crash` usage and no recovery card. If you
see `task_recovered` for a worker's task, its process had already ended when
the sweep ran.

#### A worker will not start

- "the worker socket path is N bytes": the data directory is too long for a
  Unix socket path (macOS allows 103 bytes, and the socket adds about 30). It
  only happens with a long `DAMAIAN_DATA_DIR`, and it is refused before
  anything is created.
- A refusal naming a symlink, an existing path or a tracked path under
  `.damaian/worktrees`: the creation checks treat `.damaian/` as untrusted
  ([`SECURITY.md`](../SECURITY.md#worker-sessions)). Nothing was changed.
- `worker_spawn_failed` with an error after `worktree add` (copying an
  untracked file under *Include*): the worktree and its branch are left in
  place, and the event does not list the commands that ran. Look for the
  directory under `.damaian/worktrees/` and `git branch --list 'damaian/*'`.
- 403 "Worker sessions are turned off": the setting is off. Every worker route
  except `GET /api/workers` refuses then, `stop` and `shutdown` included, so a
  worker left running when you turned the setting off is controlled again by
  turning it on, or ends when the app quits.

#### Leftover worktrees after a crash

Workers die with the app, and the process registry backs that up at the next
launch, but **worktrees and branches are kept on purpose**: they hold the
worker's work. After a crash, a worker's worktree is still listed by
`git -C "$REPO" worktree list` and its session still shows in the sidebar,
with *Restart worker*. Remove a worktree you no longer want from the board's
*Remove worktree*, which refuses anything that is not a worker worktree and
confirms uncommitted changes first, or by hand with
`git worktree remove .damaian/worktrees/<slug>`. Damaian never deletes the
`damaian/<slug>` branch. If a directory was deleted by hand, `git worktree
prune` clears Git's record of it.

#### `DAMAIAN_MOCK_MODEL_RESPONSE` is set in the shell's environment

The desktop shell builds its model adapter the same way for its own turns and
for workers (`chat_model_adapter`), and like the CLI it answers from
`DAMAIAN_MOCK_MODEL_RESPONSE` when that variable is set. A shell started from a
terminal with it set therefore answers every turn, its own and its workers',
from the variable, without calling the provider. The app started from Finder
does not ([why](#the-app-does-not-see-your-shells-environment)).

### Checkpoints and rewind

One checkpoint per turn, under `checkpoints/<repo-id>/`. The manifest says what
the turn covered and what it deliberately did not:

```bash
jq '{summary, fileCount: (.files|length), excluded, commandEffectsCovered}' \
  "$DATA_DIR/checkpoints/$REPO_ID/manifests/$CHECKPOINT_ID.json"
```

Common questions:

- **"Rewind says a file changed since the checkpoint."** That is a conflict, and
  nothing was written — not part of the tree, not one file out of several. The
  file no longer holds what the turn left there, so Damaian cannot tell your
  edit from the agent's. Resolve it by hand, or rewind the conversation only.
- **"A file was not restored."** Look at `excluded` in the manifest. A path
  matched `restricted_patterns` (`.env` and friends), resolved outside the
  repository, or lives inside Damaian's own data directory — the last one
  matters when `DAMAIAN_DATA_DIR` is inside the repository you are working on.
- **"Command effects were not covered."** `commandEffectsCovered: false` means
  the working-tree census exceeded `checkpoint_census_max_paths`, or the
  repository is not a git checkout. Damaian says so rather than implying
  coverage it does not have.
- **"The conversation went back but the files did not."** Those are two
  independent switches, and the rewind dialog offers them separately.
- **"A rewound message is still in the session log."** Correct: rewind appends
  a `conversation_rewound` marker rather than truncating the log, so the history
  stays auditable. Readers treat events after the marker's `throughEventSeq` as
  inert.

Retention is `checkpoint_retention_days` (default 90) plus a
`checkpoint_max_total_bytes` ceiling (default 1 GiB). Cleanup drops expired
manifests, deletes their refs, and lets `git gc` collect the content, but never
drops the newest checkpoint of the session you are in. None of the three keys
can be set by repository config.

## Inspecting configuration

Effective config for a repository, with all three overlays applied:

```bash
cargo run -p damaian-cli -- config-show /path/to/repo
```

Omit the repo argument to see user + admin only, without repo scope. Add
`--sources` to name the scope behind every value and list refused requests
([Why a rule is in force](#why-a-rule-is-in-force)). In the desktop app the
attributed form is the **Effective policy** table. `GET /api/config` returns
the plain values, and `GET /api/effective-policy?repo=<path>` the attributed
form, both as JSON.

`command_allowlist` in `config-show` output is the effective list for that
repository: the user and admin machine-wide entries plus the `Allow Always`
grants for that working folder. Other folders' grants are not shown, because
they do not apply there — read `user.conf` to see them all.

Write to a specific scope — this reports the exact file it touched, which is
also a quick way to confirm where a scope resolves:

```bash
cargo run -p damaian-cli -- config-set user model_name deepseek-v4-flash
```

To check overlay precedence, print the raw files rather than the merged view:

```bash
cat "$DATA_DIR/config/user.conf"
cat "$REPO/.damaian/config.conf"
cat "$DATA_DIR/config/admin.conf"
```

Relevant environment variables:

| Variable | Effect |
|----------|--------|
| `DAMAIAN_DATA_DIR` | Relocates the whole data directory |
| `DAMAIAN_ADMIN_CONFIG` | Relocates `admin.conf` |
| `DAMAIAN_REPO` | Default repository at launch, used only when no folder was previously saved |
| `DAMAIAN_DESKTOP_PORT` | Port for standalone `desktop-shell` (**not** read by the Tauri app, which is pinned to 4765) |
| `DAMAIAN_MOCK_MODEL_RESPONSE` | Returns a canned model reply — no API key, no network |
| `SHELL` | Login shell for the PTY terminal; falls back to `/bin/zsh` |

## Reproducing outside the UI

### The API token blocks curl

Every `/api/*` path requires the `x-damaian-api-token` header
(`require_api_token` in
[lib.rs](../crates/desktop-shell/src/lib.rs)). The token is 32 random
bytes generated per process, kept in memory, handed to the webview over the
Tauri IPC command `damaian_desktop_bootstrap`, and **never written to disk or
printed**. You cannot curl a running app's API from outside it, and there is no
flag to set the token.

So: use `damaian-cli` to reproduce engine behaviour, and the webview console to
reproduce shell behaviour. Do not spend time trying to authenticate curl
against the app.

The one server curl can reach is the ignored test
`serves_the_ui_for_manual_inspection`
([desktop-shell/src/lib.rs](../crates/desktop-shell/src/lib.rs)). It serves the
real UI on `127.0.0.1:4899` with its own isolated data directory and a seeded
repository, and prints `apiToken = "damaian-ui-inspection-token"`, a fixed test
token, with the repository and data paths:

```bash
cargo test -p desktop-shell --lib -- --ignored --nocapture serves_the_ui
```

It runs until you stop it. Pass that token as `x-damaian-api-token` (the
recipes call it `$TOKEN`; see [Conventions](#conventions)), or in the
browser console set `apiToken` to it and call `setRepository` with the printed
repository path. It shows only its own seeded state, never the user's.

### CLI reproduction

The CLI bypasses the HTTP shell entirely, which is exactly what makes it useful
for splitting engine bugs from UI bugs:

```bash
cargo run -p damaian-cli -- index /path/to/repo
cargo run -p damaian-cli -- search /path/to/repo "some query"
cargo run -p damaian-cli -- git-status /path/to/repo
cargo run -p damaian-cli -- classify-command "rm -rf build"
cargo run -p damaian-cli -- propose-command /path/to/repo "npm test"
cargo run -p damaian-cli -- repo-map /path/to/repo --json
cargo run -p damaian-cli -- symbols /path/to/repo upload_retry
cargo run -p damaian-cli -- symbol-references /path/to/repo src/upload.rs:42 upload_retry
cargo run -p damaian-cli -- propose-validations /path/to/repo
DAMAIAN_MOCK_MODEL_RESPONSE="Mock answer" cargo run -p damaian-cli -- ask /path/to/repo "What does auth do?"
cargo run -p damaian-cli -- propose-edit /path/to/repo "Make the change"
cargo run -p damaian-cli -- show-patch /path/to/repo <patch-id>
```

Run `damaian --help` for the full list. Always pair CLI reproduction with
`DAMAIAN_DATA_DIR` pointed somewhere disposable so you do not write into the
user's real sessions, patches, or audit trail:

```bash
DAMAIAN_DATA_DIR=/tmp/damaian-debug cargo run -p damaian-cli -- ask /path/to/repo "..."
```

`DAMAIAN_MOCK_MODEL_RESPONSE` also accepts a full `DAMAIAN_EDIT_V1` or
`DAMAIAN_COMMAND_V1` envelope, so patch-proposal and command-approval paths can
be driven deterministically without a provider.

### Debugging the web UI

The UI is vanilla JS at `crates/desktop-shell/static/app.js` — no bundler, no
source maps, so line numbers in console errors are the real ones.

In `npm run desktop:dev`, right-click the window and choose **Inspect Element**
to open the WebKit inspector. Or point a browser at
`http://127.0.0.1:4900/` for a standalone shell
([Capturing stdout and stderr](#capturing-stdout-and-stderr)) — the page loads and static
assets work, but every `/api/*` call fails with `Desktop API bootstrap is
available in the desktop app`, because the Tauri IPC that supplies the token
does not exist in a plain browser. That message is expected outside the app,
not a bug.

Error-surfacing behaviour worth knowing:

- A `401` on a protected path triggers exactly one automatic re-bootstrap and
  retry (the `401` branch of `api()` in `app.js`). A second failure surfaces to the user.
- Failures reach the user as a toast and a `Desktop API unavailable` banner
  above the conversation (`#desktop-api-banner`), which clears on the next
  successful `/api/*` response; the underlying message is in the console.
- The CSP in [tauri.conf.json](../crates/desktop-app/tauri.conf.json) restricts
  `connect-src` to `http://127.0.0.1:4765`. A shell on any other port cannot be
  reached from the packaged webview — this is why the app refuses to fall back
  to another port.

## Common failures

### Port 4765

The Tauri app probes the port before starting and **refuses to start** if it is
taken (`shell_port_for_status` in
[desktop-app/src/main.rs](../crates/desktop-app/src/main.rs)),
because the CSP and Tauri capability are scoped to that exact origin. There is
no fallback by design.

```bash
lsof -nP -iTCP:4765 -sTCP:LISTEN
```

If that PID is a Damaian shell, the user's app is probably already running. Do
not take the port and do not kill by name. Run your own instance on another
port with its own data directory, using the standalone shell command in
[Capturing stdout and stderr](#capturing-stdout-and-stderr).

### The data directory is refused at startup

The data directory carries a schema marker, `schema.conf`, holding one line,
`schema_version=1` ([data_schema.rs](../crates/workspace-engine/src/data_schema.rs),
`ensure_data_dir_schema`). It is checked before the shell serves anything. A
directory with content and no marker is adopted as version 1 and the startup
line says so. A marker that names a newer version, `0`, or something that is
not a number, or has no `schema_version` line, is refused, and nothing in the
directory is touched:

```text
The data directory at <path> declares data schema version "<found>", and this version of Damaian supports version 1. Nothing in it was changed. …
```

The app shows this in a dialog titled `Damaian cannot use its data directory`
and quits; the standalone shell prints it and exits. The usual cause is running
an older build against a directory a newer build wrote. Run a build that
supports it, or point `DAMAIAN_DATA_DIR` somewhere else. Do not delete the
directory or edit the marker to get past it: the refusal exists because an
older build writing over a newer layout loses sessions.

### The app does not see your shell's environment

A macOS app opened from Finder, the Dock or Launchpad does not read your shell
profile, so it does not get the variables or the `PATH` your Terminal has. That
one fact is behind several symptoms:

- For a remote provider, `model_api_key_env` names an environment variable
  that is set in your shell and missing in the app, so
  [model calls fail](#model-calls-fail). A `keychain:` reference avoids this.
  A loopback endpoint such as Ollama (local) does not need a key at all.
- An MCP stdio server's `command`, or `docker`, resolves in Terminal and not in
  the app. Use an absolute path to the executable.
- A variable such as `DAMAIAN_MOCK_MODEL_RESPONSE` or `DAMAIAN_DATA_DIR`
  changes the behaviour of a shell started from a terminal and not of the app
  started from Finder.

To see what the app does with your shell's environment, launch it from a
terminal instead, as described in
[Capturing stdout and stderr](#capturing-stdout-and-stderr).

### Model calls fail

Check in this order:

1. For a remote provider, is the key `Saved`? [Secrets](#secrets) shows how to
   check without printing it. A loopback endpoint such as Ollama (local) does
   not need one.
2. If `model_api_key_env` names an environment variable rather than a
   `keychain:` reference, is that variable set in the launching environment?
   See [The app does not see your shell's environment](#the-app-does-not-see-your-shells-environment).
   This does not apply to a loopback endpoint.
3. `config-show` — is `model_base_url` the provider you think it is? A repo or
   admin overlay may be redirecting it.
4. The audit log — a `model_request_prepared` with no matching
   `model_response_completed` means the request left and the response never
   arrived. Compare against stderr for the transport error.

A provider refusal is audited as `provider_refusal` carrying its classification;
a refused turn's task state also carries a `failureKind`. See
[Provider refusal classifications](#provider-refusal-classifications).

### Provider refusal classifications

A provider can refuse a call. The refusal is classified from the HTTP **status
first** and the error object's structured `code`/`type` field second — never
from a substring of the message. The classification decides whether Damaian
retries, and it is recorded as the task's `failureKind` and in the audit log.

| Classification | Status | Retried? | What to do |
|---|---|---|---|
| `provider_rate_limited` | 429 | Yes, up to 4 attempts / 90s | Wait, or retry later by resuming the task. |
| `provider_overloaded` | 500, 502, 503, 504 | Yes, up to 4 attempts / 90s | Wait; it is the provider, not your request. |
| `provider_quota_exhausted` | 402; another 4xx (not 400/404/422) whose error `code` or `type` names quota, billing or balance; or such an error object with no classifying status | No | Top up the account; the provider's message says what is owed. |
| `provider_auth_failed` | 401, 403 | No | The key is wrong, expired, or lacks access. |
| `provider_bad_request` | 400, 404, 422 | No | The request will not succeed if repeated. |
| `provider_refused` | other 4xx/5xx | No | Recognised as a refusal but not classified further; treated as permanent. |

A refused task keeps its plan, so resuming it after the provider recovers (or
after you fix your account) re-runs the same work rather than re-asking.

### A refusal offered no switch to my other provider

The desktop app offers to answer a refused turn with another configured
provider only when that provider qualifies. The engine picks the **first**
entry in `model_providers`, in configuration order, that:

- has an id different from the active provider's — a second model on the same
  provider is never offered;
- has a non-empty `api_key_env`. Only the name is checked. A name with no key
  behind it still gets an offer, and approving it then fails with
  `<NAME> is required` or a Keychain error, unless its endpoint is loopback
  (Ollama local), where no key is needed. The offer stays open, so
  **Keep it failed** still works;
- has the same `supports_native_tools` as the active provider, because the
  paused turn's messages were built for that contract;
- for `provider_quota_exhausted` only, has an `api_key_env` **different** from
  the active provider's. Two ids reading one key are one account.

There is also no offer when one was already made this turn. If the fallback
also refuses, the turn fails with that refusal's `failureKind`, so a loop of
offers is impossible. A transient refusal that a retry fixed never reaches the
offer at all. The CLI never offers: its model calls do not go through a chat
turn that could pause.

To see what happened to an offer:

```bash
grep -h '"provider_fallback' "$DATA_DIR/audit/events.jsonl" | jq -c .
```

- `provider_fallback_offered`: the turn paused. Carries `proposalId`,
  `refusedProvider`, `fallbackProvider` and `classification`. The usual
  `provider_refusal` row is written first, as it is for any refusal.
- `provider_fallback_decision`: `decision` is `declined` or `approved`, and an
  approval also carries `fallbackModel`. A decline fails the task with the
  refusal's `failureKind`, exactly as if no offer had been made.

An approved switch also leaves a `provider_fallback` event in the session log
(`refusedProvider`, `fallbackProvider`, `approvedBy`) and a `system` message in
the transcript ("Switched to … for this turn after … refused it (…)"). A
rewind keeps the event: it does not change which model answered a call that
was already billed. While an offer is open, its paused turn is stored under
`chat/pending/<proposalId>.json`. If that file is gone after a crash, recovery
fails the task instead of showing the offer again.

Nothing records the decision for later. The next turn starts on the configured
provider, and a refusal there gets a fresh offer.

### The assistant replies but never proposes a patch

Usually one of three things, distinguishable from the logs:

- **Native tool-calling is off** for the provider. It is on by default, so this
  usually means an explicit `model_provider.<id>.supports_native_tools=false`.
  `propose_patch` and MCP tools require it; the fallback text envelopes are less
  reliable.
- **The response was truncated** at the output ceiling, and a truncated
  `propose_patch` call leaves unusable partial JSON. With no
  `model_provider.<id>.max_output_tokens` set, Damaian sends a built-in
  ceiling: 65,536 for `deepseek-v4-flash` and `deepseek-v4-pro`, and 8,192 for
  the retired `deepseek-chat`/`deepseek-reasoner` aliases and any other
  DeepSeek model. A provider with no ceiling, configured or built in, gets
  none, and its own default applies, which can be low. Set
  `model_provider.<id>.max_output_tokens` to raise it.
- **The turn ended `complete`** rather than `patch_proposal_ready` — the model
  simply answered in prose. Confirm with `model_response_completed.status`.

### Patch apply fails with a conflict

`ClientError::PatchConflict` means file hashes changed between preview and
apply — the file was edited after the diff was generated. This check is a
product guarantee, not a bug. Regenerate the proposal. Pre-edit copies for
already-applied patches are under `rollback/<patch-id>/`.

### An `edit_file` proposal is refused

When the assistant proposes a snippet edit (`edit_file`) and it is refused, the
model quotes the reason back. Two refusals are expected, not bugs:

- **`old_text did not match`** — the file changed since the model read it. Ask
  the assistant to re-read the file and propose again; it is working from a
  stale view. This refusal exists precisely so a stale view lands in a refusal
  instead of a change to the wrong region.
- **`old_text matched N times`** — the snippet is not unique enough to anchor
  on. Ask the assistant to include more surrounding context in `old_text` so it
  matches exactly once.

Neither refusal writes anything to disk. A refused edit goes back to the model
as a tool result, so it can retry within the same turn.

### A command that used to run unasked now asks

Damaian used to admit a command as read-only when its text merely *started with*
`pwd`, `ls`, `git status`, `git diff`, `git log` or `git show`. That prefix
match let through commands that were not read-only at all, so it was replaced by
a tokenizer and a closed option list. Three kinds of command that used to run
without asking now ask, and each is deliberate:

- **`git diff`, `git show` and `git log -p`.** They print file contents, which a
  command line cannot bound or keep restricted paths out of. The assistant reads
  changes and commit content through `read_git_diff` and `read_git_log`, which do
  both, so nothing is lost — the shell form just asks now.
- **An option outside the admitted list.** `git diff --output=README.md` and
  `git log --output=x` used to write files unasked, and `git log
  --since=yesterday` used to run. Any option not in the admitted list asks.
- **A command that expands to something else, or reads outside the repository.**
  `ls $HOME/.ssh`, `ls ~`, `ls *` and `git diff --no-index $HOME/x README.md`
  used to slip past the prefix match. `$`, `~` and unquoted globs now ask,
  because the shell would run something other than what was checked.

If a command that looks harmless asks, check it against the admitted table in
the user guide's [Command Approval](USER_GUIDE.md#command-approval). The first
line of the approval card names the deciding cause — for shell control and
paths it names the construct or path, and for anything else it names the
keyword, setting or program that refused it. **Why this command** repeats every
reason the classifier recorded. This is the boundary working, not a regression.

### Reading an approval's cause

Every approval prompt leads with the deciding cause, in plain words, and the
approval card shows the same line beside the risk badge. The shapes are:

- **Asks first: uses …** — shell control (`&&`, `|`, `;`, `>`, a newline). The
  prompt names each construct it found.
- **Asks first: may use the network (…)** — a network keyword such as `curl` or
  `wget`. This is separate from the generic high-risk reason, so a network
  command reads as a network command.
- **Asks first: Docker commands always ask** — every Docker command.
- **Asks first: `chmod` changes your system or repository** — a high-risk
  keyword, named. `git commit` reads the same way.
- **Asks first: Damaian does not know what … does** — a command outside every
  admitted list and every keyword rule.
- **Asks first: reads …, which the policy restricts** or **Asks first: refers
  to …, outside the repository** — the path is named.
- **Asks first: *Ask before every command* is on (set by user)** — a setting,
  with the scope that set it: `user`, `repository`, `profile`, `admin` or
  `default`. The same shape names *Ask before test, build and lint commands*
  when `require_approval_for_risky_commands` is on.
- **Asks first: hook `<id>` requested it: `<reason>`** — a
  [`before_tool`](USER_GUIDE.md#hooks) hook returned `request_approval` for a
  command that would otherwise have run unasked. Its audit cause name is
  `hook_requested`. A hook can add this reason; it can never remove one.
- **Blocked: …** — a `command_blocklist` rule, a destructive pattern, or the
  permission profile's `command_access`. These offer no approval.

**The agent says: …**, when present, is the model's own stated purpose, not the
policy's reason. **Why this command** keeps the full prompt, including every
cause when more than one fired.

For a session already logged, `command_proposal_stored` carries `causes`: the
variant names in classifier order, comma-joined (for example
`unknown,outside_repository`). That is the field to group or count when asking
which reasons produce the most prompts.

### The assistant was refused: the session mode, the permission profile, or the command policy?

They are different boundaries (see the user guide's Working Modes and
Permission Profiles sections), and the wording tells them apart:

- **A mode refusal** always names two modes: `Refused: <mode> mode does not
  allow this. Switch to <mode> mode to allow it.` It appears as a grey tool-result
  bubble in the conversation, directly above the model's own explanation. No
  approval card is shown, because the mode forbids the action whatever you
  would answer. When a stored patch is refused at apply, the same sentence
  comes back as an `access_denied` error rather than `policy_blocked`.
- **A profile refusal** names a setting and no mode: `Refused: the permission
  profile does not allow this (<key>=<value>). Switching mode will not allow
  it.` The setting is the resolved value, which your own config or the
  repository's may also have set; [Why a rule is in force](#why-a-rule-is-in-force)
  finds its source. When both mode and profile refuse, the mode is named.
  A command `command_access` blocks is refused before any proposal or card in a
  turn. A stored proposal run by id is re-classified and refused as blocked by
  policy; the reason its classification carries reads `Blocked: the
  permission profile allows <level> commands only`.
- **A command-policy refusal** names no mode. It reads `local policy blocks this
  command`, or `Command proposal is blocked by policy` at run time, and it
  happens in Code mode too.

If a command you allowlisted with `Allow Always` is refused, that is a mode
refusal and it is expected. The allowlist removes the prompt in Code. It does
not make a command read-only in Plan or Review, and Ask runs no commands at all.

To see which mode a session was in, and when it changed, read its
`session_mode_set` events (a session with none is in Code):

```bash
jq -c 'select(.eventType=="session_mode_set") | {seq, timestampMs, mode: .payload.mode, setBy: .payload.setBy}' "$SESSION_FILE"
```

The newest event wins. A turn uses the mode in force when it started, so a
change landing mid-turn shows up from the next turn. Approvals are the
exception: approving a paused command, MCP call or browser diagnostic, or
applying a stored patch, is checked against the mode *at that moment*. A
rewind does not undo a mode change, because the mode is a setting, not part of
the conversation.

Refusals leave a trace in the audit log (`audit/events.jsonl`). A command
refused when you approve it is recorded as `stored_command_rejected` with
`rejectedBy: "mode_policy"`. Its `actor` field still reads `user`, which is a
known wart. A browser diagnostic refused the same way is recorded with
`decision: "refused_by_mode"`. The profile equivalents are
`rejectedBy: "profile_policy"` and `decision: "refused_by_profile"`. Profile
switches are `permission_profile_set` audit events, not session events, and a
profile change takes effect at the same points as a mode change. A refusal inside a turn, before any approval
card was shown, leaves no proposal at all. What it leaves is a failed
`action_finished` in the session log, next to the tool-result message.

### A hook refused, blocked, or did not run

A hook ([user guide](USER_GUIDE.md#hooks)) can stop an action, turn a command
into an approval card, add a warning, or fail closed. Its wording names the
hook, which is how it is told apart from the other boundaries:

- **`Refused by hook <id>: <reason>`** — a `deny` verdict or a mandatory hook's
  failure. At `before_tool` it is a grey tool-result bubble and the action is
  not dispatched: no proposal is stored, no action marker is opened and no
  approval card appears. At `session_start` it refuses the turn before a
  session exists; at `before_context` it fails the task with `hook_denied` and
  shows the reason as the assistant message. At `before_patch_apply` it is the
  apply error and writes nothing; the file is unchanged and no apply marker
  exists. At `before_task_complete` it goes to the model as a user-role note,
  the task does not complete, and the model gets another round within the
  existing budget.
- **`Hook <id>: <reason>`** — an `after_tool` denial. It is appended to the
  tool result the model sees and recorded as an error finding. Nothing is
  undone: the command or patch already happened. Roll it back with
  [Rewind](USER_GUIDE.md#rewind) if you agree with the hook. An
  `after_patch_apply` or `after_check` denial is only an error finding (and,
  for a patch, a warning on the result).
- **A warning** is a finding with source **Hook** in the [findings
  panel](USER_GUIDE.md#findings), and at patch apply it is also on the patch
  card. An **optional** hook's failure (non-zero exit, timeout, malformed
  output, version mismatch, output too large, a spawn failure, or a changed
  review) is a warning and the action proceeds. A **mandatory** hook's failure
  blocks, so a rule you rely on cannot be disabled by a typo.

The same wording is not used for a mode, profile or command-policy refusal;
those are in [The assistant was refused](#the-assistant-was-refused-the-session-mode-the-permission-profile-or-the-command-policy).

**Where to look in the audit log.** Every invocation writes one `hook_invoked`
record with `hookId`, `event`, `sessionId`, `taskId`, `classification`
(`mandatory`/`optional`), `source`, the effective `verdict`, `outcome`
(`ok`/`failed`), `durationMs`, and on failure a `failureKind`:

```bash
jq -c 'select(.eventType=="hook_invoked") | {event: .event, hookId: .hookId, verdict: .verdict, outcome: .outcome, failureKind: .failureKind, durationMs: .durationMs}' "$DATA_DIR/audit/events.jsonl" | tail -20
```

The failure kinds are `spawn_failed`, `non_zero_exit`, `timeout`,
`malformed`, `version_mismatch`, `output_too_large` and
`changed_since_review`. A denial or a request also carries its `reason`; a
successful `deny` response is `outcome: ok`. Three related records explain the
rest:

- `hook_widening_attempt` carries `field` or `verdict` and the *name* only,
  never the value. It is written when a hook returns an unknown field or an
  unrecognised verdict; the response is then treated as malformed, so a
  mandatory hook denies.
- `hook_command_refused` carries the redacted `command` and the `reason` when a
  command a hook asked Damaian to run was not run. A hook never raises an
  approval card, so only a command policy would run unasked is run; Ask mode
  and a narrower profile refuse it.
- `hook_reviewed` records `repositoryId`, `hookId`, `decision` and the approved
  `hash`. Pending, declined and disabled hooks are not invoked and write no
  per-event record.

**Debug the hook itself.** The shell's **Settings › Hooks** page has a **Test**
button with an event picker; the CLI has the same run and the redacted stderr:

```bash
damaian hooks /path/to/repo
damaian hook-run /path/to/repo no_migrations before_tool
```

`hook-run` prints one hook's own verdict, failure kind, duration and redacted
stderr against a representative payload for the event. A test run writes no
`hook_invoked` record because it is not a session invocation; only a review
writes an audit record (`hook_reviewed`). Both `hook-run` and the Test button
**refuse a repository hook that is pending, changed since review, declined or
disabled**, with a message saying which state it is in — make it active first.
A user's own disabled hook can still be tested.

**A repository hook does not run.** Repository hooks arrive
**pending review** and stay inactive until you approve the exact definition
shown in the consent dialog. The review is stored in **your** user config as
`hook_review.<repository_id>.<hook_id>`; the repository's own
`.damaian/config.conf` cannot record or replace it. Approving at a stale hash
is refused with "changed since it was shown to you". If the definition or a
script inside the repository changes after approval, the hook is **changed
since review**: it stops running and fails under its own classification
(mandatory blocks, optional warns) rather than executing the new code.
Declining keeps it inactive and is reversible from **Settings › Hooks**;
declined hooks are not shown in the consent notice again. A user or
administrator hook cannot be changed, disabled, retimed or made optional by a
repository, and a profile cannot carry a hook at all.

**A depth-exceeded warning.** A hook's requested command can itself trigger
`before_tool` and `after_tool`, including other hooks. Nesting is capped at
two levels; past that Damaian skips the nested dispatch, still runs the
command, and records a warning finding plus `hook_depth_exceeded` (`hookId`,
`event`, `depth`). The outer action is not blocked: the nested command is one
the hook deliberately asked for. A rule that needs a chain deeper than two
hooks is asking Damaian to run more than the guard allows; rewrite the rule to
run its check inside the hook and return the verdict from there.

**A hook works in Terminal but not from Damaian.** The hook process has a
cleared environment — only `PATH`, `HOME`, `USER`, `LANG`, `TMPDIR` if set,
plus `DAMAIAN_HOOK_ID`, `DAMAIAN_HOOK_EVENT` and `DAMAIAN_REPOSITORY_ROOT` —
and runs in the repository root, not your shell's current directory. It does
not read your shell profile, and it never gets the model API key or an MCP
token. Put an absolute shebang/executable path in `command` (or `args`), or
ensure the program is on the inherited `PATH`.

**Deleting a session runs a `session_end` hook.** `/api/session-delete` takes
the repository as well as the session id, and refuses a root that is not the
session's own repository:
`Session <id> does not belong to repository <root>`. The session is left in
place and no hook runs. The desktop app sends the open folder; a `curl` or
script caller must send `repo` too.

**The command line shown for a hook joins its arguments with spaces.** The
review dialog and the hook listings render `command` followed by `args`
separated by single spaces, so an argument that itself contains a space cannot
be told apart from two arguments in that display. The approved review hash
covers the exact argument list (`args` is `|`-separated in config), so the
decision still applies to exactly the definition shown; it is the display that
is ambiguous, not the review. Known limitation, recorded in spec 32 §7.

### A file read or search result was truncated

A large file read reports a line range with the file's real length ("lines
1–400 of 4420"), and a search or directory listing says how many results were
cut ("50 of 231 matches"). A result that was cut shorter than requested adds a
`(truncated by lines|bytes)` notice. This is deliberate: a truncated result that
reads as complete is the failure the caps exist to prevent. The caps are
`max_read_lines` (400), `max_list_entries` (200), `max_search_matches` (50), and
`max_match_line_chars` (500); a repository config may lower one but never raise
it.

### A fetch was refused

`fetch_url` is the agent's page reader. Several different things can stop a
fetch, and only one of them is the approval dialog. Find the wording in the tool
result in the conversation; the audit log records the outcome separately (see
[Audit log](#audit-log)).

| What the model sees | Cause | What to do |
|---|---|---|
| A card headed with a host, then **Allow once** / **Don't fetch** | The host has no approval for this checkout. **Nothing has been sent.** | Read the URL. Approve once, choose *Always allow this host in this project* in the overflow, or decline. Declining makes no request and tells the model the host was refused. |
| `Cannot fetch from <host>: the URL carries something that looks like a credential (<category>); it is refused before any approval is asked` | The URL contains a secret-shaped string. The category is named; the matched value is not repeated. | Remove the credential from the URL. This refusal happens **before** the dialog, so approving cannot override it. |
| `Cannot fetch from <host>: the URL carries content from <path>, a file read earlier in this task; it is refused before any approval is asked` | The query or fragment contains a line of at least 16 bytes from a file `read_file` returned in this task, raw, percent-decoded or base64-decoded. | Do not put repository content in a URL. Files read through `search_content`, `read_git_diff` or a command are deliberately outside this check. |
| `Cannot fetch from <host>: the URL names <host>, which is not a public internet address; a fetch only reaches public addresses` | The host is a loopback, private, link-local or reserved address literal, including an IPv4-mapped IPv6 form. No dialog appears. | Use a public URL. A name that resolves privately is refused at request time with `… resolves to <ip>, which is not a public internet address`; the same rule applies. |
| `Cannot fetch that URL: <reason>. Nothing was requested` | The URL did not parse: not `http`/`https`, no host, userinfo, whitespace or a control character, or an IPv4 address written as a bare number, hex or a short form (`2130706433`, `0x7f000001`, `127.1`). | Supply a plain absolute `http(s)` URL, with an IPv4 address written as four decimal octets. |
| `The page answered with a redirect to <target>. This tool does not follow redirects. …` | A 3xx response, including a same-host one. The request count is one, and the target is named. | Fetch the named URL as a new call; its host needs its own approval. The audit event is `web_fetch_redirect`. |
| `A fetch was not made: this turn has already fetched N of M pages` | The per-turn cap, `web_fetch_max_per_turn` (default 8, hard ceiling 20). | The turn must answer with what it has. The cap is not a bug. |
| `Cannot fetch from <host>: <error>. Check the URL and try again` | The request or its timeout failed (connect 15 s, stall 30 s, total 60 s). | Check the URL; a stop during a fetch appears as `The fetch was stopped.` |
| `No text is shown: this content type is not one the fetch tool converts (text/*, application/json, application/xhtml+xml)` | The response is not convertible text. Its type and size are still reported. | Nothing to fix; the body is not returned and was not cached. |
| `The page contained no text` | A convertible type with an empty body, or a page whose text was all inside scripts, styles or tags. | A JavaScript-rendered page is expected to land here: nothing is rendered and no sub-resource is loaded. |

**Where an approval lives.** *Allow always* writes
`web_domain_allowlist.<repository id>` to **user** config. The machine-wide
`web_domain_allowlist` is also user-scoped. A repository file that sets either
key is rejected as a forbidden key, recorded as `repository_config_key_rejected`
with the key and class only, and reported once; a permission profile cannot
carry it either. A host and the same host on a non-default port are different
approvals.

**`Redaction: clean` beside a `[REDACTED_…]` placeholder.** The raw page is
scanned and redacted in the fetch transport before conversion, and the converted
text is scanned again. By then the credential is already a placeholder, so the
second scan finds nothing and the result prints `Redaction: clean` and
`redaction_status()` / the session event say `clean`. **The secret is gone**;
only the reported count is wrong. This is a known reporting defect, recorded in
[spec 51](specs/51_external_reference_retrieval/proposal.md) §7.3; trust the
placeholder, not the count.

**Reloading while the dialog is open re-presents the card.** A crash or reload
during a fetch approval no longer fails the task. The next launch reads the
paused turn and shows the same host-and-URL card; *Allow once*, *Allow always*
and *Don't fetch* go through the same route as the live dialog, and *Allow
always* still writes only the user-scope per-checkout key. Nothing was requested
before the crash, so the re-presented card is the same question rather than a
resumed side effect.

### A file the model should see is missing from context

Two independent filters, both visible in `config-show`:

- `ignore_patterns` — excluded from indexing (defaults include `.git/`,
  `node_modules/`, `target/`, `.damaian/`).
- `restricted_patterns` — access denied by policy (defaults include `.env*`,
  `*.pem`, `*.key`, `**/secrets/**`), surfacing as `ClientError::AccessDenied`.

Confirm what was actually sent by reading `contextFiles` on the
`model_request_prepared` event rather than reasoning about the patterns. When a
turn did run and a specific file is absent, the `excluded` list on its
`context_assembled` event names the reason — a restricted path, a category
share, or a read refusal — in
[Reading a turn's context manifest](#reading-a-turns-context-manifest).

### A `worktree` argument is refused

The read tools take an optional `worktree` argument, but only a worktree git has
registered for the repository is accepted (compared by canonical path). A
refusal is a failed tool call of the form `Worktree <x> <reason>; reachable
worktrees: <paths>`, and it lists the worktrees that *would* be accepted so the
assistant can correct itself in the same round. The reasons the code produces:

- `does not exist` — the argument does not resolve to an existing path.
- `is not a registered worktree of this repository` — it resolves, but git has
  not registered it. A directory under `.claude/worktrees/` or
  `.damaian/worktrees/` that only looks like a worktree, and a symlink planted
  there pointing elsewhere, both land here.
- ``is this session's own checkout; omit `worktree` to read it`` — the argument
  is the checkout the session already works in; read it without `worktree`.
- `is a bare repository with no working tree` — a `bare` entry has nothing to
  read.
- `is prunable: git no longer considers it a valid worktree` — git lists it but
  would prune it (its administrative files or directory are gone).
- `does not exist on disk` — git lists it, but its path no longer resolves to a
  directory.
- ``is outside the repository root and no `allowed_roots` entry contains it`` — a
  registered worktree outside the repository root is reachable only when an
  `allowed_roots` entry contains it.

Worktrees inside the repository root are reachable by default; ones outside it
need an `allowed_roots` entry. A worktree is still subject to the session's own
policy once reached: a restricted or ignored path inside it is refused or
omitted exactly as in the main checkout.

### `list_worktrees` shows no worktrees from a worktree session

`list_worktrees` lists only the worktrees that are **reachable from the session's
own checkout**, not every registration git knows about. The containment root is
the checkout the session opened. A session running in a linked worktree — a
worker's `.damaian/worktrees/<slug>`, or a development worktree under
`.claude/worktrees/` — therefore treats its own entry as the session checkout
and the main checkout and every sibling as outside it. With the default empty
`allowed_roots`, the reachable set is empty and the tool result is:

```
Worktrees (0 listed, ahead/behind vs `main`):
```

It does not enumerate the entries it withheld, so the assistant can read that as
“no worktrees are registered”. They are registered; the tool cannot reach them.

Check the truth with `git -C <repo> worktree list` or the audit event
`git_worktrees_listed`: its `count` is the number listed and its `resourcePath`
is the session root that decided reachability. A `count: 0` with a `resourcePath`
inside a worktree is this case, not an empty repository.

To survey the repository from a worktree session, either add the main checkout
to `allowed_roots` (then the main and its siblings are reachable, while the
session's own checkout is still addressed by omitting `worktree`), or run the
tool from the main checkout. This is deliberate (spec 60 §5.1, correction C1),
pinned by `from_a_linked_worktree_list_worktrees_is_empty_without_allowed_roots`;
changing it would be a spec change, not a configuration fix.

### MCP server not working

`POST /api/mcp-test` performs a live connect-and-list and returns either
`toolCount` and the tool names or an error string; the UI exposes it as the
test button in MCP settings. Then check:

- `mcp_enabled` is not `false` at admin scope — it is a global kill-switch.
- `mcp_server_allowlist`, if non-empty, must include the server id.
- The server's own `enabled=true` — new servers are off by default.
- Audit events `mcp_discovery_failed` and `mcp_tool_call_failed` for the
  underlying error.

For stdio servers the `command` must be resolvable in the app's environment,
which for a GUI launch is not your shell's `PATH` — prefer an absolute path
([why](#the-app-does-not-see-your-shells-environment)).

### Docker command problems

Docker commands are intentionally approval-gated. Use
`classify-command "docker ps"` or `propose-command /path/to/repo "docker ps"`
from the CLI to inspect policy decisions without a model call.

If an approved Docker command fails:

- `docker` may be missing from the app's `PATH`; use an absolute Docker
  executable path, or launch Damaian from a terminal
  ([why](#the-app-does-not-see-your-shells-environment)).
- Docker Desktop or the Docker daemon may not be running or reachable.
- `docker compose` requires the Compose plugin. If the machine uses the legacy
  binary, try `docker-compose`.
- Check audit events `command_proposed`, `command_executed`, and
  `stored_command_executed` for the proposed command, risk, exit status, and
  output references.

### Semantic search does nothing

`enable_semantic_search` is off by default because enabling it downloads an
embedding model on first use. The download uses `curl`
(`download_if_missing` in
[embeddings.rs](../crates/workspace-engine/src/embeddings.rs)) and
writes to `models/all-MiniLM-L6-v2/`. If that directory is missing or partial,
the first search after enabling is doing the fetch. Delete the directory to
force a clean re-download.

### Terminal panel problems

The PTY terminal spawns a login shell from `$SHELL` (falling back to
`/bin/zsh`) on a pseudo-terminal. The PTY code is in the desktop-shell crate
(`terminal::open` in
[terminal.rs](../crates/desktop-shell/src/terminal.rs)) and runs inside the
desktop app's process, which the webview reaches through the Tauri
`terminal_*` commands. Failures surface
as `failed to start shell: <error>`. Sessions are held in a process-global map,
so they do not survive a restart. `crates/desktop-shell/static/xterm*` is
vendored and minified — never edit or reformat it.

### Token figures are always "estimated"

The per-turn line reads `~12,714 tokens (estimated)` and never drops the
marker. That means no provider-reported usage reached the run.

An OpenAI-compatible streaming API omits usage unless asked, so Damaian sends
`stream_options: {"include_usage": true}` on every streaming request. Three
things stop a measured figure coming back:

1. **The provider rejects the field.** It answers with an error body naming
   `stream_options` or `include_usage`. Damaian retries the request **once**
   without the field and remembers the answer for the rest of the process, so
   the probe costs one extra call per launch, not one per turn. That retry is
   deliberately *not* counted as a retry attempt — it is a capability probe.
   The observation is audited as `model_usage_reporting_unsupported`; grep the
   audit log for it.
2. **The provider accepts the field and reports nothing anyway.** No audit
   event, because nothing failed. The figure is simply an estimate.
3. **`provider_reports_usage` is off.** Then not even the probe runs.

To stop probing a provider you know does not support it:

```text
model_provider.<id>.provider_reports_usage=false
```

Repository config cannot set this, or any other `model_provider.*` key — see
[the trust boundary spec](specs/34_repository_config_trust_boundary.md).

An estimate is `payload length / 4`, the same approximation the context budget
uses. It is deliberately the same one: a second, better estimate would give
two different token counts for the same turn.

### A turn shows no token line at all

Sessions written before token accounting shipped carry no usage events, and
those turns render nothing rather than a zero — a zero would read as "this
turn was free". Only new turns have figures. Check the session log for
`task_usage_recorded`:

```bash
grep -c task_usage_recorded "$SESSION_FILE"
```

### Cache hit rate reads "not reported", or seems low

None of these are a malfunction:

- **The provider never reports a cache split.** Most OpenAI-compatible
  providers do not; the usage row's tooltip says so instead of showing 0%,
  which would read as "caching is broken" when the truth is "we cannot see
  it". See [Prompt cache hit rate](USER_GUIDE.md#prompt-cache-hit-rate).
- **The system prompt or the enabled tool set changed.** A prompt cache
  matches on an exact prefix
  ([spec 49](specs/49_prompt_cache_accounting_and_reuse/proposal.md) §5.7);
  toggling a tool or anything that edits the system prompt changes the first
  bytes of every request after it, and the cache starts over from there.
- **A long turn outgrew its message window.** Within one turn, a request
  carries at most `agent_max_turn_messages` (default 24) of the turn's
  messages, so once a turn passes that, earlier rounds drop out of the request.
  That changes the request's prefix, which the provider then has to warm
  again. The session log still holds every round.
- **The provider expired the cached prefix on its own schedule.** Providers
  hold a cached prefix for a limited window — commonly a few minutes of
  inactivity — and nothing in Damaian controls or extends it.
- **The conversation is short.** A turn or two rarely has enough of a stable
  prefix ahead of the volatile part to be worth a hit.

None of this is a request Damaian is failing to make on your behalf: this
slice measures whatever hit rate the provider already produces under today's
request order. It does not yet reorder a request to make a hit more likely —
that half is specified but blocked on a conversation-window redesign
([spec 49](specs/49_prompt_cache_accounting_and_reuse/proposal.md) §5.8), not
on anything you configured.

### The packaged app will not launch

Release DMGs are Developer ID signed and notarized, so they open without a
Gatekeeper prompt. A blocked first launch usually means a DMG built locally or
by a manually triggered preview run, which is ad-hoc signed. That is expected,
not a bug; the workaround is in [Development](DEVELOPMENT.md#build-a-macos-dmg). Do
not try to "fix" it in code.

## Resetting state

Ordered least to most destructive. **Look at what you are deleting first** —
sessions and audit records are the user's history and are not recoverable.

Clear the in-memory repository index: restart the app.

Clear semantic-search caches (regenerated on demand):

```bash
rm -rf "$DATA_DIR/vector-index"
```

Clear stale pending state — proposals the user never resolved:

```bash
ls "$DATA_DIR/patches/pending"
ls "$DATA_DIR/commands/pending"
ls "$DATA_DIR/chat/pending"
```

Reset UI state only, keeping config and history — clear localStorage from the
webview console, or remove `~/Library/WebKit/com.damaian.client`.

Start from a fully clean tree without touching the real one:

```bash
DAMAIAN_DATA_DIR=/tmp/damaian-clean npm run desktop:dev
```

Prefer that last option to deleting anything. It reproduces first-run behaviour
and is reversible.

## What to collect for a bug report

Enough for someone else to work the problem without the machine in front of
them:

1. Version from the About dialog, and whether it is a dev run or a packaged DMG.
2. Steps to reproduce, and which of the processes in
   [Process model](#process-model) is involved.
3. stderr from a terminal-attached run, captured as described in
   [Logs](#logs).
4. The audit timeline around the failure — the `jq` timeline command in
   [Audit log](#audit-log), trimmed to the relevant window.
5. The affected session file, or the `message_appended` extract. **Review it
   before sharing:** it contains prompts and file content from the user's
   repository.
6. `config-show` output for the repository. It contains no secrets —
   `model_api_key_env` is a reference, not a key — but confirm that before
   pasting.
7. Provider and model name, and whether native tool-calling is enabled.

Never include Keychain values, raw API keys, or the API token in a report.

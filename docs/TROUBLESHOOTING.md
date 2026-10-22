# Troubleshooting

How to diagnose a misbehaving Damaian build: where state is written, what is
logged and what is not, and how to reproduce a failure outside the UI.

Written for both humans and coding agents. For end-user symptoms and fixes see
the [User Guide](USER_GUIDE.md#troubleshooting); for build and release mechanics
see [Development](DEVELOPMENT.md); for repository conventions and traps see
[AGENTS.md](../AGENTS.md).

## Triage order

Work top to bottom. Each step is cheap and rules out a whole class of causes.

1. **Reproduce with the process attached to a terminal.** The app has no log
   file — see [Logs](#logs). If you cannot see stderr, you are guessing.
2. **Read the audit log tail.** `~/Library/Application Support/DamaianClient/audit/events.jsonl`
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
| `sessions/<session-id>.jsonl` | Per-conversation event log: sessions, tasks, messages | `SessionStore` ([session.rs](../crates/workspace-engine/src/session.rs)) |
| `patches/pending/` | Proposed patches awaiting approval | `PatchStore::save` ([edit.rs](../crates/workspace-engine/src/edit.rs)) |
| `patches/rejected/` | Rejected patches, kept for inspection | `PatchStore::mark_rejected` ([edit.rs](../crates/workspace-engine/src/edit.rs)) |
| `rollback/<patch-id>/` | Pre-edit file copies backing patch rollback, **unredacted** | `PatchEngine::apply_patch` ([patch_engine.rs](../crates/workspace-engine/src/patch_engine.rs)) |
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
| `command_timeout_secs`, `max_read_lines`, `max_list_entries`, `max_search_matches`, `max_match_line_chars`, `repository_map_max_tokens`, `agent_max_task_tokens`, `agent_max_turn_messages`, `agent_max_exploration_rounds` | May be lowered, never raised |
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

### Secrets

API keys live in the macOS Keychain under service `DamaianClient`
(`KEYCHAIN_SERVICE` in [keychain.rs](../crates/desktop-shell/src/keychain.rs)).
Config holds only a reference: `keychain:<account>`, or the name of an
environment variable — never a raw key.

```bash
security find-generic-password -s DamaianClient -a model-api-key -w
```

That prints the secret, so prefer checking existence only. The UI's
`/api/model-key-status` and the Settings `Model API Key` field report `Saved`
without revealing the value. If `model_api_key_env` names an environment
variable instead, the variable must be set **in the environment the app was
launched from** — a GUI launch from Finder does not inherit your shell profile.

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
jq -r 'select(.eventType=="repository_config_key_rejected") | "\(.repositoryId) \(.key) \(.class)"' ~/Library/Application\ Support/DamaianClient/audit/events.jsonl
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
jq -c 'select(.eventType|startswith("permission_profile"))' ~/Library/Application\ Support/DamaianClient/audit/events.jsonl
```

A command or browser diagnostic the profile refused when you approved it is
recorded as `stored_command_rejected` with `rejectedBy: "profile_policy"`, or
with `decision: "refused_by_profile"` (see
[the refusal section](#the-assistant-was-refused-the-session-mode-the-permission-profile-or-the-command-policy)).

```bash
tail -20 ~/Library/Application\ Support/DamaianClient/audit/events.jsonl | jq .
```

Last 50 events as a readable timeline:

```bash
jq -r '"\(.timestampMs|tonumber/1000|strftime("%H:%M:%S")) \(.eventType) \(.status // .exitCode // .files // "")"' ~/Library/Application\ Support/DamaianClient/audit/events.jsonl | tail -50
```

Which event types have ever fired, and how often:

```bash
jq -r .eventType ~/Library/Application\ Support/DamaianClient/audit/events.jsonl | sort | uniq -c | sort -rn
```

Event types grouped by what they tell you:

| Area | Event types |
|------|-------------|
| Model turn | `model_request_prepared`, `model_response_completed`, `edit_model_request_prepared`, `chat_turn_cancelled`, `model_usage_reporting_unsupported`, `turn_continued` |
| Round budget | `round_budget_paused`, `round_budget_decision`, `turn_budget_extended` |
| Plan review | `plan_review_decision` |
| Patches | `patch_proposed`, `edit_patch_ready_for_approval`, `edit_failed`, `patch_applied`, `patch_hunks_rejected`, `patch_rollback_started`, `patch_rolled_back`, `stored_patch_applied`, `stored_patch_files_rejected`, `stored_patch_rejected`, `stored_patch_rolled_back` |
| Commands | `command_proposed`, `command_proposal_stored`, `command_executed`, `stored_command_executed`, `stored_command_rejected`, `command_allowlisted`, `repository_allowlist_migrated` |
| Files & repo | `file_read`, `file_modified`, `file_restored`, `directory_listed`, `content_searched`, `repository_indexed`, `repository_map_rebuilt`, `symbol_index_rebuilt`, `git_status_read`, `git_diff_read`, `git_log_read`, `git_worktrees_listed` |
| Checkpoints | `checkpoint_created`, `checkpoint_creation_failed`, `checkpoint_files_skipped`, `checkpoint_paths_added`, `checkpoint_restored`, `checkpoint_conflicted`, `checkpoint_cleanup` |
| MCP | `mcp_tools_listed`, `mcp_tool_called`, `mcp_tool_call_failed`, `mcp_discovery_failed` |
| Provider refusals | `provider_refusal`, `provider_fallback_offered`, `provider_fallback_decision` |
| Browser diagnostics | `browser_diagnostic_approval_decision`, `browser_diagnostic_session_approval_used` |
| Config and profiles | `repository_config_key_rejected`, `permission_profile_set`, `permission_profile_key_rejected`, `permission_profile_imported` |
| Crash recovery | `task_recovered`, `task_recovery_decision`, `session_log_truncated_tail`, `pending_approval_unavailable`, `reattached_approval_resolved`, `task_usage_recorded_for_lost_call` |
| Workers | `worker_spawned`, `worker_spawn_failed`, `worker_restarted`, `worker_terminated`, `worker_worktree_removed` |
| Orphan sweep | `orphan_process_killed`, `orphan_process_kill_refused`, `orphan_process_already_exited`, `process_registry_entry_unreadable` |

Useful reads:

- `model_request_prepared` carries `contextFiles`, `tokenEstimate`, and
  `toolRound` — the fastest way to confirm what the model actually saw, and how
  many tool rounds a turn took.
- `model_response_completed.status` distinguishes `complete`,
  `complete_with_sandbox_command`, `command_approval_required`, and
  `patch_proposal_ready`. A turn that "did nothing" usually ended `complete`
  when you expected `patch_proposal_ready`.
- `command_proposed` carries `risk` and `requiresApproval` — check here before
  assuming the command policy is wrong.
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
ls -t ~/Library/Application\ Support/DamaianClient/sessions/*.jsonl | head -1
```

Replay a conversation:

```bash
jq -r 'select(.eventType=="message_appended") | "[\(.payload.role)] \(.payload.content)"' "$SESSION_FILE"
```

Follow task state, which is how you spot a turn that died mid-flight — a task
whose last status is not `complete`, `failed`, `cancelled`,
`tool_budget_exhausted` or `token_budget_exhausted` never finished. The
exception is `waiting_for_approval`: a task paused on a proposal (a command, a
plan review, a provider fallback, or a `roundbudget_…` round-budget pause, see
[A turn stopped early](#a-turn-stopped-early-which-budget-ran-out)) is waiting
for you, not dead:

```bash
jq -r 'select(.eventType=="task_status_updated") | "\(.payload.task.status // .payload.status) \(.payload.task.id // .payload.id)"' "$SESSION_FILE"
```

The `.task //` fallbacks are not defensive padding: `task_status_updated`
genuinely appears in two shapes. `await_approval` wraps the task alongside the
proposal it is waiting for, and the plain status write does not. Both are
current.

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

A check the assistant runs inside a turn (a sandbox command, or one you
approved) and every browser diagnostic turn their failures into findings,
which the **Findings** panel shows. A command run from outside a conversation
(`/api/run-command` with no session) records none.

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

**Where full output lives.** A finding's `details` is a bounded excerpt (at
most 4096 bytes, redacted). For a command, `originRef` is the execution id,
and the stored output is
`<data dir>/commands/output/<originRef>/stdout.log` and `stderr.log`. That is
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

### Repository roots and the repository map

The map of a repository's project roots
([User Guide](USER_GUIDE.md#repository-roots)) is stored per repository:

```text
<data dir>/repository-map/<repository_id>.json
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
jq -c 'select(.eventType=="repository_map_rebuilt")' ~/Library/Application\ Support/DamaianClient/audit/events.jsonl
```

**What the model saw.** The map reaches the model as one context item of
kind `repository_map`. It is limited by `repository_map_max_tokens`: the
default is 800, `0` turns it off, and a repository may only lower it. When the
map is cut to fit, its last line starts `Abridged to fit:` and says what was
dropped. If the map cannot be stored, for example because the data directory
is read-only, it is left out of the request and the turn goes on.

### A turn stopped early: which budget ran out

What happens at `agent_max_tool_rounds` (default 8) depends on whether a token
ceiling is set ([spec 61](specs/61_continue_the_stopped_turn/proposal.md)):

- **No `agent_max_task_tokens` (the default).** When the turn reaches its round
  budget and the model still wants a tool, the turn **pauses** instead of
  stopping. The task goes to `waiting_for_approval` on a
  `chat/pending/roundbudget_<id>.json` proposal, and the conversation shows
  `Used N of M tool rounds.` with **Answer now** and **Continue — N more**.
  Continue grants the same turn another `agent_max_tool_rounds` rounds; Answer
  now makes one final call with tools dropped. A turn never runs past 64 rounds
  in all: at that cap only Answer now is offered.
- **With `agent_max_task_tokens` set.** The turn continues on its own,
  another `agent_max_tool_rounds` at a time, and records `turn_continued` each
  time, until the token ceiling stops it or it reaches 16 rounds. At 16 rounds
  it pauses as above.

Two terminal statuses look alike in the UI and mean different things:

- `tool_budget_exhausted` — written only after **Answer now**, when the final
  call still asked for a tool instead of answering. The UI shows **Tool budget
  exhausted** with **Ask again**, which starts a new turn from the stopped
  task's own request.
- `token_budget_exhausted` — the turn reached `agent_max_task_tokens`, which is
  **per turn**, not per session. Nothing is wrong; it cost more than the cap
  allows.

A paused turn is in the audit log as `round_budget_paused` (`proposalId`,
`roundsUsed`, `roundsTotal`). The answer is `round_budget_decision` with
`decision` `continue` or `answer_now`, and a Continue also writes
`turn_budget_extended` with the rounds granted and the running total:

```bash
jq -c 'select(.eventType|test("^(round_budget_|turn_budget_extended|turn_continued)"))' ~/Library/Application\ Support/DamaianClient/audit/events.jsonl
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
and ask again; after a tool-budget stop, press **Ask again**. The next turn
reads the stopped task's plan, carries it forward as `plan_resumed`, and keeps
the evidence the earlier steps had already gathered. The same carry happens
when crash recovery's `Continue` supersedes an interrupted task. Confirm the
carry happened:

```bash
jq -r 'select(.eventType=="plan_resumed") | "\(.payload.taskId) resumed from \(.payload.resumedFrom)"' "$SESSION_FILE"
```

If no `plan_resumed` appears, the previous task either had no plan or had
nothing outstanding — a plan whose steps all finished has nothing to resume.

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
jq -r 'select(.eventType|startswith("task_recover")) | "\(.eventType) \(.taskId) \(.classification // "") \(.decision // "") \(.outcome // "") \(.reason // "")"' ~/Library/Application\ Support/DamaianClient/audit/events.jsonl
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
use the manual-inspection server, which is seeded with three tasks left
mid-flight and prints a fixed token:

```bash
cargo test -p desktop-shell --lib -- --ignored --nocapture serves_the_ui
# in another terminal, with the apiToken it printed:
curl -s -H "x-damaian-api-token: damaian-ui-inspection-token" http://127.0.0.1:4899/api/recovery | jq
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
curl -s -H "x-damaian-api-token: damaian-ui-inspection-token" \
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
does not inherit a terminal's variables.

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
cat ~/Library/Application\ Support/DamaianClient/config/user.conf
cat /path/to/repo/.damaian/config.conf
cat ~/Library/Application\ Support/DamaianClient/config/admin.conf
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

It runs until you stop it. Pass that token as `x-damaian-api-token`, or in the
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
`http://127.0.0.1:4900/` for a standalone shell — the page loads and static
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
not take the port and do not kill by name. Run your own instance elsewhere:

```bash
DAMAIAN_DATA_DIR=/tmp/damaian-debug cargo run -p desktop-shell -- --port 4900 --repo /path/to/repo
```

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

### Model calls fail

Check in this order:

1. `GET /api/model-key-status`, or the Settings `Model API Key` field — is it
   `Saved`?
2. If `model_api_key_env` names an environment variable rather than a
   `keychain:` reference, is that variable set in the launching environment? A
   Finder launch does not read your shell profile.
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
  `<NAME> is required` or a Keychain error. The offer stays open, so
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
grep -h '"provider_fallback' ~/Library/Application\ Support/DamaianClient/audit/events.jsonl | jq -c .
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

- **Native tool-calling is off** for the provider. `propose_patch` and MCP tools
  require it; the fallback text envelopes are less reliable. Check
  `model_provider.<id>.supports_native_tools`.
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

### A file read or search result was truncated

A large file read reports a line range with the file's real length ("lines
1–400 of 4420"), and a search or directory listing says how many results were
cut ("50 of 231 matches"). A result that was cut shorter than requested adds a
`(truncated by lines|bytes)` notice. This is deliberate: a truncated result that
reads as complete is the failure the caps exist to prevent. The caps are
`max_read_lines` (400), `max_list_entries` (200), `max_search_matches` (50), and
`max_match_line_chars` (500); a repository config may lower one but never raise
it.

### A file the model should see is missing from context

Two independent filters, both visible in `config-show`:

- `ignore_patterns` — excluded from indexing (defaults include `.git/`,
  `node_modules/`, `target/`, `.damaian/`).
- `restricted_patterns` — access denied by policy (defaults include `.env*`,
  `*.pem`, `*.key`, `**/secrets/**`), surfacing as `ClientError::AccessDenied`.

Confirm what was actually sent by reading `contextFiles` on the
`model_request_prepared` event rather than reasoning about the patterns.

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
which for a GUI launch is not your shell's `PATH` — prefer an absolute path.

### Docker command problems

Docker commands are intentionally approval-gated. Use
`classify-command "docker ps"` or `propose-command /path/to/repo "docker ps"`
from the CLI to inspect policy decisions without a model call.

If an approved Docker command fails:

- `docker` may be missing from the GUI app environment. A Finder-launched macOS
  app does not necessarily inherit your Terminal `PATH`; use an absolute Docker
  executable path or launch Damaian from a shell in development.
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
`/bin/zsh`) on a pseudo-terminal owned by the Tauri wrapper
(`terminal::open` in
[terminal.rs](../crates/desktop-shell/src/terminal.rs)). Failures surface
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
grep -c task_usage_recorded ~/Library/Application\ Support/DamaianClient/sessions/<session>.jsonl
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

### The quality gate is slow locally

CI runs the whole gate in a few minutes. On this project's development Mac the
same runs have ranged from that to well over an hour, on the same code, minutes
apart. A fresh worktree's cold `target/` is not the cause: on 2026-10-14 a
clippy run into an empty target directory took 45s, and
`cargo nextest run --workspace` 33s. When a gate step runs past
about ten minutes, stop waiting and find out why.

**Measure it with `scripts/quality-gate.sh`.** It runs the same seven checks
(or the ones named, e.g. `scripts/quality-gate.sh clippy nextest`), times each
step, and samples load, swap, power source, concurrent cargo processes and the
top CPU consumers every 30 seconds into
`$CARGO_TARGET_DIR/gate-logs/<timestamp>/` (`target/` when unset). It warns
on stderr once a step passes `GATE_SLOW_SECS` (default 600). Read the
summary's `cpu/wall` column first: well above 1 means the step was compiling
on several cores, well below 1 means it was mostly *waiting*, and no build
cache or linker change will help a waiting step.

**A waiting test step is most likely endpoint security software stalling
process launches.** The tests that dominate a slow run are the ones that start
many processes — the checkpoint store's `git` calls, the eval-harness
scenarios, the mode-refusal tests that run commands and MCP servers. On
2026-10-15 a full `nextest` took 457s–690s at `cpu/wall` 0.1, and the
hundred-turn checkpoint test alone took 196s and then 49s a few minutes later.
Timing every `git` call that test made showed two clusters: the median call
took 14 ms, but about 9% took a fixed ~280 ms each and accounted for 68% of all
`git` time. A plain launch of `/usr/bin/true` took 2 ms and of `git` 8 ms
when measured in isolation. That shape — most launches fast, a share held for a
fixed interval, worse at some moments than others, absent on CI — is what an
agent that inspects every launch before letting it run produces. The Mac in
question runs two, installed by its administrators: Sophos (`SophosAntiVirus`,
`SophosCBR`) and SentinelOne (`sentineld`). Which of them is responsible has
not been isolated; that needs their admin consoles.

Check for the signature with `scripts/time-git-calls.sh`, which runs a command
with every `git` call timed and prints the distribution:

```bash
scripts/time-git-calls.sh cargo nextest run -p workspace-engine --test checkpoints --locked \
  -E 'test(a_hundred_turn_session_stays_within_its_documented_storage_bound)'
```

A healthy machine shows p99 within a few times p50 and no `>=150 ms` cluster.
The remedy is outside this repository: ask the administrators for scanning
exclusions covering the repositories (`~/development`), `~/.cargo`,
`~/.rustup`, the `git` binary (`/opt/homebrew/bin/git`), the per-user temp
directory the tests create repositories in (`$TMPDIR`, under `/var/folders`),
and the target directories. Re-run the check afterwards; the slow cluster should
be gone. Do not try to fix this in code — a test that is slow only on one
machine is not a code defect.

**Ruled out, with measurements, on 2026-10-14 and 2026-10-15:**

- **The Developer Tools exemption** (System Settings → Privacy & Security →
  Developer Tools). Gatekeeper's `syspolicyd` was often busy during slow runs,
  but back-to-back runs launched from exempt Terminal.app and from an exempt
  Claude Code process were equally slow (690s and 457s). One fast run after
  adding an exemption was coincidence. If you try it anyway, note that the
  exemption follows the *responsible* process, which is not always the parent
  you see — `sudo launchctl procinfo $$ | grep -i responsible` names it.
- **`git` itself and raw file I/O.** A checkpoint-style turn (five `git` calls)
  cost about 55 ms in `$TMPDIR`, in a scratch directory and under
  `~/development` alike.
- **Parallel contention.** The hundred-turn test was as slow running alone as
  inside the full suite.

**Other causes to rule out, in the sampler log:**

- **Concurrent builds.** The cargo/rustc process count covers every worktree.
  Two gates at once compete for the same eight cores, and runs sharing one
  target directory serialise on its lock (`Blocking waiting for file lock`).
  Give each worktree its own `CARGO_TARGET_DIR`; never kill another session's
  build to free the machine.
- **Swap.** `vm.swapusage` close to its total while `rustc` processes are large
  means the build is paging.
- **Battery.** A run on battery or in Low Power Mode is slower by design.

If none of these shows up, `sample <pid> 5` on the slowest process names the
call it is blocked in; record what you find before changing anything.

## Resetting state

Ordered least to most destructive. **Look at what you are deleting first** —
sessions and audit records are the user's history and are not recoverable.

Clear the in-memory repository index: restart the app.

Clear semantic-search caches (regenerated on demand):

```bash
rm -rf ~/Library/Application\ Support/DamaianClient/vector-index
```

Clear stale pending state — proposals the user never resolved:

```bash
ls ~/Library/Application\ Support/DamaianClient/patches/pending
ls ~/Library/Application\ Support/DamaianClient/commands/pending
ls ~/Library/Application\ Support/DamaianClient/chat/pending
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
4. The audit timeline around the failure — the `jq` timeline command above,
   trimmed to the relevant window.
5. The affected session file, or the `message_appended` extract. **Review it
   before sharing:** it contains prompts and file content from the user's
   repository.
6. `config-show` output for the repository. It contains no secrets —
   `model_api_key_env` is a reference, not a key — but confirm that before
   pasting.
7. Provider and model name, and whether native tool-calling is enabled.

Never include Keychain values, raw API keys, or the API token in a report.

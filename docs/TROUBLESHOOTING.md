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

Three processes can be involved, and knowing which one owns a symptom saves
most of the time:

| Process | Binary | Role |
|---------|--------|------|
| Native wrapper | `damaian-desktop` | Tauri app: window, menus, folder picker, Keychain, updater, PTY terminal. Spawns the shell in a background thread. |
| Local shell | `damaian-desktop-shell` | HTTP server on `127.0.0.1:4765` serving the web UI and the `/api/*` surface. Also runnable standalone. |
| CLI | `damaian` | Same workspace engine, no HTTP, no UI. |

The webview loads `http://127.0.0.1:4765/` — the UI is a normal web page talking
to a local HTTP server, so browser-side debugging applies (see
[Debugging the web UI](#debugging-the-web-ui)).

> **Never `pkill -f damaian-desktop-shell`** or match any Damaian binary by
> name. The user's own running app shares those names. Track the PID you
> spawned and kill that one.

## Where state lives

### Global data directory

Default location, resolved in `Config::default_data_dir()`
([config.rs:182](../crates/workspace-engine/src/config.rs:182)):

```text
~/Library/Application Support/DamaianClient
```

`DAMAIAN_DATA_DIR` overrides it wholesale. Everything below is relative to that
root, so a session run with `DAMAIAN_DATA_DIR=.damaian` writes a parallel,
isolated tree — that is the intended way to test without touching real data.

| Path | Contents | Written by |
|------|----------|------------|
| `config/user.conf` | User-scope config overlay | [config.rs:233](../crates/workspace-engine/src/config.rs:233) |
| `config/admin.conf` | Admin-scope overlay (highest precedence) | [config.rs:237](../crates/workspace-engine/src/config.rs:237) |
| `audit/events.jsonl` | Append-only audit trail, secret-redacted | [audit.rs:58](../crates/workspace-engine/src/audit.rs:58) |
| `sessions/<session-id>.jsonl` | Per-conversation event log: sessions, tasks, messages | [session.rs:244](../crates/workspace-engine/src/session.rs:244) |
| `patches/pending/` | Proposed patches awaiting approval | [edit.rs:129](../crates/workspace-engine/src/edit.rs:129) |
| `patches/rejected/` | Rejected patches, kept for inspection | [edit.rs:104](../crates/workspace-engine/src/edit.rs:104) |
| `rollback/<patch-id>/` | Pre-edit file copies backing patch rollback | [patch_engine.rs:315](../crates/workspace-engine/src/patch_engine.rs:315) |
| `commands/pending/<id>.dcmd` | Command proposals awaiting approval | [validation.rs:108](../crates/workspace-engine/src/validation.rs:108) |
| `commands/output/<exec-id>/` | `stdout.log`, `stderr.log`, `summary.dcmd` per execution | [validation.rs:67](../crates/workspace-engine/src/validation.rs:67) |
| `commands/rejected/<id>.dcmd` | Rejected command proposals | [validation.rs:93](../crates/workspace-engine/src/validation.rs:93) |
| `chat/pending/<proposal-id>.json` | Suspended chat turn awaiting a command decision | [chat.rs:859](../crates/workspace-engine/src/chat.rs:859) |
| `vector-index/<repo-id>.bin` | Semantic-search embeddings cache | [vector_index.rs:149](../crates/workspace-engine/src/vector_index.rs:149) |
| `models/all-MiniLM-L6-v2/` | Downloaded embedding model (semantic search only) | [embeddings.rs:27](../crates/workspace-engine/src/embeddings.rs:27) |

Not on disk: the **repository index** is an in-memory cache with a file watcher
([index_cache.rs](../crates/workspace-engine/src/index_cache.rs)), rebuilt on a
5-minute full-rescan interval. Restarting the app clears it — which is why a
restart "fixes" stale-index symptoms.

### Configuration layers

Three files overlay onto the built-in defaults, applied in this order. Later
wins — **except at repository scope**, which is untrusted because that file
arrives with a clone and can only make the policy stricter:

| Order | Scope | Path | Can weaken? | Override |
|-------|-------|------|-------------|----------|
| 1 | User | `<data-dir>/config/user.conf` | yes | — |
| 2 | Repository | `<repo>/.damaian/config.conf` | **no** | — |
| 3 | Admin | `<data-dir>/config/admin.conf` | yes | `DAMAIAN_ADMIN_CONFIG` |

A missing file is skipped silently, not an error. The format is flat
`key=value`, one per line; blank lines and `#` comments are ignored
([config.rs:699](../crates/workspace-engine/src/config.rs:699)). List-valued
keys use `|` as the separator. Nested keys are dotted:

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
([lib.rs:1048](../crates/desktop-shell/src/lib.rs:1048)). If a setting will not
stick, something at admin scope is overriding it, or repository scope is adding
a restriction on top of it.

#### A repository config key had no effect

That is usually deliberate. Repository config may add restrictions but never
remove one
([SECURITY.md](../SECURITY.md#repository-config-trust-boundary), spec
[34](specs/34_repository_config_trust_boundary.md)):

| Key | In repository config |
|-----|----------------------|
| `shell`, `data_dir`, `allowed_roots`, `secret_patterns`, `audit_enabled`, `block_generated_secrets`, any `model_*` (including `model_provider.<id>.*`) | Ignored, audited, reported to the user once per repository |
| `command_allowlist` | Never honoured — `Allow Always` writes user scope instead |
| `restricted_patterns`, `ignore_patterns`, `command_blocklist` | Added to the user's list, never replacing it |
| `require_approval_for_*` | May be set `true`, never `false` |
| `mcp_enabled`, `mcp_server.<id>.enabled` | May be turned off, never on |
| `mcp_server_allowlist` | May be narrowed, never widened |
| `mcp_server.<id>.*` for a server the user already configured | Ignored: it would redirect a process the user trusts |
| Everything else (budgets, `enable_semantic_search`, `agent_*`) | Applied as written |

Ask which keys a repository asked for and did not get — this also records them
in the audit log, so it answers once per repository and then reports nothing
new:

```bash
cargo run -p damaian-cli -- config-review /path/to/repo
```

`config-set repo` prints the same note immediately after writing a key that
repository scope will ignore.

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
([keychain.rs:3](../crates/desktop-shell/src/keychain.rs:3)). Config holds only
a reference: `keychain:<account>`, or the name of an environment variable —
never a raw key.

```bash
security find-generic-password -s DamaianClient -a model-api-key -w
```

That prints the secret, so prefer checking existence only. The UI's
`/api/model-key-status` and the Settings `Model API Key` field report `Saved`
without revealing the value. If `model_api_key_env` names an environment
variable instead, the variable must be set **in the environment the app was
launched from** — a GUI launch from Finder does not inherit your shell profile.

### Web UI state (localStorage)

Project list, last folder, and per-repo preferences are browser state, not
config. Keys (`app.js:31`):

| Key | Contents |
|-----|----------|
| `damaian:projects` | Known project paths |
| `damaian:lastRepository` | Folder restored on launch |
| `damaian:projectDisplayNames` | Renamed projects |
| `damaian:expandedProjects`, `damaian:projectsCollapsed` | Sidebar UI state |
| `damaian:lastSession:<repo>` | Last open conversation per repo |
| `damaian:pinnedContext:<repo>:<session>` | Pinned context files |
| `damaian:chatModelPrefs:<repo>` | Selected provider/model per repo |

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

Every runtime error the wrapper reports goes to stderr via `eprintln!`
([desktop-app/src/main.rs:29](../crates/desktop-app/src/main.rs:29)) and is lost
if nothing is attached to the process.

Development — output appears in the terminal:

```bash
DAMAIAN_REPO=/path/to/repo npm run desktop:dev
```

Packaged app — launch the binary directly instead of double-clicking:

```bash
/Applications/Damaian.app/Contents/MacOS/Damaian
```

Already-running GUI instance — read the unified log, which captures stderr from
GUI-launched processes:

```bash
log show --predicate 'process == "Damaian"' --last 30m --info
```

Standalone shell, no Tauri wrapper, on a port of your own so you do not collide
with the user's app:

```bash
DAMAIAN_DATA_DIR=/tmp/damaian-debug cargo run -p desktop-shell -- --repo /path/to/repo --port 4900
```

Messages worth recognising on startup
([desktop-app/src/main.rs:60](../crates/desktop-app/src/main.rs:60)):

| Message | Meaning |
|---------|---------|
| `Damaian desktop shell listening at http://127.0.0.1:<port>` | Shell is up |
| `...refuses to start because 127.0.0.1:4765 is already in use` | Port conflict; see [Port 4765](#port-4765) |
| `Damaian shell did not report readiness` | Shell did not bind within the 2-second startup timeout |
| `Damaian shell server stopped: <error>` | Server thread died after startup |

### Audit log

Append-only JSONL, one event per line, every field passed through the secret
scanner before being written ([audit.rs:42](../crates/workspace-engine/src/audit.rs:42)).
Records **metadata about actions** — file paths, commands, exit codes, token
estimates, provider and model names, status — deliberately **not** prompt or
response bodies. Disabled entirely when `audit_enabled=false`.

Retention prunes files in `audit/` whose mtime is older than
`audit_retention_days` (default 90). Because `events.jsonl` is appended to, its
mtime is always current, so in practice the active log grows without bound and
is never pruned. Do not expect it to self-trim.

Every line carries `eventId`, `timestampMs`, `userId` (`local_user`), and
`eventType`.

A repository-sourced config key that was refused appears as
`repository_config_key_rejected` with `key` and `class`
(`forbidden`, `restrict_only`, or `user_owned`) — never the refused value,
which is repository-controlled text:

```bash
jq -r 'select(.eventType=="repository_config_key_rejected") | "\(.repositoryId) \(.key) \(.class)"' ~/Library/Application\ Support/DamaianClient/audit/events.jsonl
```

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
| Model turn | `model_request_prepared`, `model_response_completed`, `edit_model_request_prepared` |
| Patches | `patch_proposed`, `edit_patch_ready_for_approval`, `edit_failed`, `patch_applied`, `patch_hunks_rejected`, `patch_rollback_started`, `patch_rolled_back`, `stored_patch_applied`, `stored_patch_files_rejected`, `stored_patch_rejected`, `stored_patch_rolled_back` |
| Commands | `command_proposed`, `command_proposal_stored`, `command_executed`, `stored_command_executed`, `stored_command_rejected` |
| Files & repo | `file_read`, `file_modified`, `file_restored`, `repository_indexed`, `git_status_read`, `git_diff_read` |
| MCP | `mcp_tools_listed`, `mcp_tool_called`, `mcp_tool_call_failed`, `mcp_discovery_failed` |

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

### Session logs

One JSONL file per conversation, `sessions/<session-id>.jsonl`, appended as
events. This is where **message content** lives.

Event types: `session_created`, `session_renamed`, `task_created`,
`task_status_updated`, `message_appended`.

Find the most recently touched session:

```bash
ls -t ~/Library/Application\ Support/DamaianClient/sessions/*.jsonl | head -1
```

Replay a conversation:

```bash
jq -r 'select(.eventType=="message_appended") | "[\(.payload.role)] \(.payload.content)"' "$SESSION_FILE"
```

Follow task state, which is how you spot a turn that died mid-flight — a task
left at `running` or `waiting_for_approval` never reached `complete` or
`failed`:

```bash
jq -r 'select(.eventType=="task_status_updated") | "\(.payload.status) \(.payload.id)"' "$SESSION_FILE"
```

Session files are read by scanning and parsing lines, and `list_sessions`
returns an empty list rather than failing when the directory is unreadable
([session.rs:151](../crates/workspace-engine/src/session.rs:151)). A truncated
or hand-edited line therefore degrades quietly — if a conversation renders
oddly, validate the file:

```bash
jq -e . "$SESSION_FILE" > /dev/null
```

### Command output

Approved command executions store their full output on disk
([validation.rs:67](../crates/workspace-engine/src/validation.rs:67)):

```text
commands/output/<execution-id>/stdout.log
commands/output/<execution-id>/stderr.log
commands/output/<execution-id>/summary.dcmd
```

Both streams are secret-redacted and truncated to `max_command_output_bytes`
before being stored, so a suspiciously short log may be a truncation, not an
early exit. Check `exitCode` on the matching `command_executed` audit event.

## Inspecting configuration

Effective config for a repository, with all three overlays applied:

```bash
cargo run -p damaian-cli -- config-show /path/to/repo
```

Omit the repo argument to see user + admin only, without repo scope. In the
desktop app the same data is the **Effective Policy** view, and
`GET /api/config` returns it as JSON.

`command_allowlist` in `config-show` output is the effective list for that
repository: the user and admin machine-wide entries plus the `Allow Always`
grants for that working folder. Other folders' grants are not shown, because
they do not apply there — read `user.conf` to see them all.

Write to a specific scope — this reports the exact file it touched, which is
also a quick way to confirm where a scope resolves:

```bash
cargo run -p damaian-cli -- config-set user model_name deepseek-chat
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
([lib.rs:1740](../crates/desktop-shell/src/lib.rs:1740)). The token is 32 random
bytes generated per process, kept in memory, handed to the webview over the
Tauri IPC command `damaian_desktop_bootstrap`, and **never written to disk or
printed**. You cannot curl a running app's API from outside it, and there is no
flag to set the token.

So: use `damaian-cli` to reproduce engine behaviour, and the webview console to
reproduce shell behaviour. Do not spend time trying to authenticate curl.

### CLI reproduction

The CLI bypasses the HTTP shell entirely, which is exactly what makes it useful
for splitting engine bugs from UI bugs:

```bash
cargo run -p damaian-cli -- index /path/to/repo
cargo run -p damaian-cli -- search /path/to/repo "some query"
cargo run -p damaian-cli -- git-status /path/to/repo
cargo run -p damaian-cli -- classify-command "rm -rf build"
cargo run -p damaian-cli -- propose-command /path/to/repo "npm test"
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
  retry (`app.js:157`). A second failure surfaces to the user.
- Failures reach the user as a toast and a `Desktop API unavailable` chat
  status; the underlying message is in the console.
- The CSP in [tauri.conf.json](../crates/desktop-app/tauri.conf.json) restricts
  `connect-src` to `http://127.0.0.1:4765`. A shell on any other port cannot be
  reached from the packaged webview — this is why the app refuses to fall back
  to another port.

## Common failures

### Port 4765

The Tauri app probes the port before starting and **refuses to start** if it is
taken ([desktop-app/src/main.rs:290](../crates/desktop-app/src/main.rs:290)),
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

`ClientError::is_retryable` treats rate limits, `429`, timeouts, connection
failures, and DNS failures as transient
([error.rs:43](../crates/workspace-engine/src/error.rs:43)); everything else,
including auth failures, is permanent. If a failure is being retried, it was
classified transient — the message text drives that decision.

### The assistant replies but never proposes a patch

Usually one of three things, distinguishable from the logs:

- **Native tool-calling is off** for the provider. `propose_patch` and MCP tools
  require it; the fallback text envelopes are less reliable. Check
  `model_provider.<id>.supports_native_tools`.
- **The response was truncated** at the provider's output limit. DeepSeek
  defaults to 4096 tokens, and a truncated `propose_patch` call leaves unusable
  partial JSON. Set `model_provider.<id>.max_output_tokens`.
- **The turn ended `complete`** rather than `patch_proposal_ready` — the model
  simply answered in prose. Confirm with `model_response_completed.status`.

### Patch apply fails with a conflict

`ClientError::PatchConflict` means file hashes changed between preview and
apply — the file was edited after the diff was generated. This check is a
product guarantee, not a bug. Regenerate the proposal. Pre-edit copies for
already-applied patches are under `rollback/<patch-id>/`.

### A file the model should see is missing from context

Two independent filters, both visible in `config-show`:

- `ignore_patterns` — excluded from indexing (defaults include `.git/`,
  `node_modules/`, `target/`, `.damaian/`).
- `restricted_patterns` — access denied by policy (defaults include `.env*`,
  `*.pem`, `*.key`, `**/secrets/**`), surfacing as `ClientError::AccessDenied`.

Confirm what was actually sent by reading `contextFiles` on the
`model_request_prepared` event rather than reasoning about the patterns.

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
([embeddings.rs:117](../crates/workspace-engine/src/embeddings.rs:117)) and
writes to `models/all-MiniLM-L6-v2/`. If that directory is missing or partial,
the first search after enabling is doing the fetch. Delete the directory to
force a clean re-download.

### Terminal panel problems

The PTY terminal spawns a login shell from `$SHELL` (falling back to
`/bin/zsh`) on a pseudo-terminal owned by the Tauri wrapper
([terminal.rs:49](../crates/desktop-shell/src/terminal.rs:49)). Failures surface
as `failed to start shell: <error>`. Sessions are held in a process-global map,
so they do not survive a restart. `crates/desktop-shell/static/xterm*` is
vendored and minified — never edit or reformat it.

### The packaged app will not launch

Expected, not a bug. The developer preview is ad-hoc signed, not Developer ID
signed or notarized, so Gatekeeper blocks first launch. The workaround is in
[macOS Installation](MACOS_INSTALLATION.md). Do not try to "fix" it in code.

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
2. Steps to reproduce, and which of the three processes is involved.
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

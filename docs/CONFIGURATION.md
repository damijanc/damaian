# Configuration reference

This page lists every key Damaian reads from its configuration files: what it does, its default, the values it accepts, and whether a repository's own config can change it. The [User Guide](USER_GUIDE.md) explains most of these features in their own sections; the tables link there instead of repeating them.

## Where configuration lives

Your settings are in one file:

```text
~/Library/Application Support/DamaianClient/config/user.conf
```

When `DAMAIAN_DATA_DIR` is set, the file is `config/user.conf` inside that directory instead ([Local Data](USER_GUIDE.md#local-data)). **Settings › General › User configuration** edits the same file, and so does every Settings page that saves something (a provider, a key reference, an MCP server). Editing the file by hand does exactly what editing it in Settings does.

Two other files can change the same keys:

- **Repository config**, `.damaian/config.conf` inside a repository. It arrives with a clone, so it is untrusted: it can only make Damaian more careful, never less. The last column of every table below says what a repository may do with each key. A key it may not set is ignored, and Damaian tells you once per repository and records it in the audit log ([Settings](USER_GUIDE.md#settings)).
- **Admin config**, `config/admin.conf` next to `user.conf`, or the file `DAMAIAN_ADMIN_CONFIG` names. It is read after the repository config and is trusted like your own file.

A [permission profile](USER_GUIDE.md#permission-profiles) selected for a checkout is applied last. It narrows exactly as far as a repository may, and sets none of the keys a repository may set freely. The [effective policy view](USER_GUIDE.md#reading-the-effective-policy-view) shows the value in force for every key and which file it came from.

## Syntax

- One `key=value` entry per line. Spaces around the key and the value are trimmed.
- A line that starts with `#` is a comment, and blank lines are ignored. A `#` later in a line is part of the value, not a comment.
- A later line for the same key replaces an earlier one.
- Lists are separated by `|`, for example `ignore_patterns=dist/|*.min.js`. Empty entries are dropped.
- Booleans are `true` or `false`, in lower case.
- Shares (`conversation_window_share` and the `context_budget.*` keys) are decimals from `0` to `1` with at most four decimal places, so `0.25` is a quarter.
- A key Damaian does not know, or a value it cannot parse, makes your `user.conf` fail to load, with an error naming the key. In repository config the same line is skipped and reported instead.
- When Settings saves a single key (a provider, a key reference, *Allow Always*), it rewrites the whole file in a fixed order, so comments and your own line order are not kept. Saving the text in **Settings › General › User configuration** writes it as you typed it.

**Credentials never go in this file.** A key that names an API key or a token (`model_api_key_env`, `model_provider.<id>.api_key_env`, `mcp_server.<id>.auth_token_env`) takes a reference, never the secret itself:

- `keychain:<account>` reads the secret from macOS Keychain under that account. This is the way to give the desktop app a key ([The Model API Key panel](USER_GUIDE.md#the-model-api-key-panel)).
- An environment variable name, such as `DEEPSEEK_API_KEY`, reads the secret from that variable. This works only for the command-line tool and a development build started from a shell ([Environment variables](USER_GUIDE.md#environment-variables-cli-and-development-only)).

Anything else, such as a pasted key, is refused.

## What a repository may change

The last column of every table uses these words:

| Entry | Meaning |
|---|---|
| **No** | Ignored in repository config, and reported to you |
| **Yes** | Applied as written, in either direction. A permission profile cannot set it |
| **Lower only** | A repository may set a smaller value than yours, never a larger one |
| **Raise only** | A repository may set a larger value than yours, never a smaller one |
| **Turn on only** | A repository may set it to `true`, never to `false` |
| **Turn off only** | A repository may set it to `false`, never to `true` |
| **Adds to yours** | A repository's entries are added to your list; none of yours are removed |
| **Narrow only** | A repository may keep fewer of your entries, never add one |

## Model and providers

See [Model Providers and API Keys](USER_GUIDE.md#model-providers-and-api-keys).

| Key | Default | Values | What it does | Repository may change it? |
|---|---|---|---|---|
| `model_provider` | `openai` | A provider id: `openai`, `deepseek`, `openrouter`, `openai-compatible`, or the id of a provider you defined. Case does not matter, `_` reads as `-`, and `open-ai`, `deep-seek`, `open-router` and `custom` are accepted spellings | The default provider: what a project with no choice of its own starts on, and what workers and commit-message drafts use. Choosing a built-in provider also sets `model_name`, `model_base_url` and `model_api_key_env` from its defaults ([Configuration examples](USER_GUIDE.md#configuration-examples)) | No |
| `model_name` | `gpt-4.1`, or `OPENAI_MODEL` when that variable is set | A model id the provider serves | The model used with the default provider | No |
| `model_base_url` | `https://api.openai.com`, or `OPENAI_BASE_URL` when set | A URL | Where model requests are sent | No |
| `model_api_key_env` | `OPENAI_API_KEY` | `keychain:<account>` or an environment variable name | Where the API key is read from. A provider's own `model_provider.<id>.api_key_env` keychain reference wins over it ([The Model API Key panel](USER_GUIDE.md#the-model-api-key-panel)) | No |
| `model_reasoning_level` | `default` | `default` (also `auto`), `none`, `minimal`, `low`, `medium`, `high`, `max` | Reasoning effort. A level the provider does not accept is not sent ([Choosing the model and effort](USER_GUIDE.md#choosing-the-model-and-effort)) | No |

### Provider definitions: `model_provider.<id>.<field>`

One line per field, for each provider you configure. `<id>` is lower-case letters, digits, `.` and `-`. Setting a field for a built-in provider (`openai`, `deepseek`, `openrouter`, `openai-compatible`) changes only that field and keeps the rest of the built-in definition. **A repository cannot set any of these fields.**

| Key | Default | Values | What it does | Repository may change it? |
|---|---|---|---|---|
| `model_provider.<id>.label` | The built-in provider's name, otherwise the id | Text | The name Settings and the model picker show | No |
| `model_provider.<id>.base_url` | The built-in provider's URL, otherwise empty | A URL; a trailing `/` is removed | The OpenAI-compatible API address | No |
| `model_provider.<id>.api_key_env` | The built-in provider's variable (`DEEPSEEK_API_KEY`, for example), otherwise empty | `keychain:<account>` or an environment variable name | Where this provider's API key is read from | No |
| `model_provider.<id>.models` | The built-in provider's model list, otherwise empty | Model ids, separated by `\|` | The models offered for this provider. The first is its default model | No |
| `model_provider.<id>.supports_native_tools` | `true` | `true`, `false` | Use the provider's native tool-calling API ([Native tool-calling](USER_GUIDE.md#native-tool-calling)) | No |
| `model_provider.<id>.max_output_tokens` | Per model ([Output and Context Limits](USER_GUIDE.md#output-and-context-limits)) | A whole number, 1 or more | Largest reply the model may generate, in tokens (**Max output tokens**) | No |
| `model_provider.<id>.context_token_budget` | Per model, otherwise `16000` | A whole number, 1 or more | How much repository content is packed into each request, in tokens (**Context per turn**) | No |
| `model_provider.<id>.context_window` | Per model, otherwise unknown | A whole number, 1 or more | How many input tokens the model accepts at all (**Model context window**) | No |
| `model_provider.<id>.provider_reports_usage` | `true` | `true`, `false` | Ask the provider to report token usage on a streamed reply. `false` skips the request and the one-time probe for a provider that rejects it | No |
| `model_provider.<id>.price_per_million_input_tokens` | None | A number, 0 or more | Your price per million input tokens, used to show what a turn cost. Both this and the output price are needed for a cost ([What a Turn Cost](USER_GUIDE.md#what-a-turn-cost)) | No |
| `model_provider.<id>.price_per_million_output_tokens` | None | A number, 0 or more | Your price per million output tokens | No |
| `model_provider.<id>.price_per_million_cached_input_tokens` | None | A number, 0 or more | Your price per million input tokens served from the provider's prompt cache. Without it, a cost with cached tokens is shown as an upper bound ([Prompt cache hit rate](USER_GUIDE.md#prompt-cache-hit-rate)) | No |
| `model_provider.<id>.supports_explicit_cache_breakpoints` | `false` | `true`, `false` | Mark prompt-cache breakpoints explicitly, for a provider that needs them. No built-in provider does | No |

## Context and limits

See [Output and Context Limits](USER_GUIDE.md#output-and-context-limits) and [Agent tools for reading and navigating](USER_GUIDE.md#agent-tools-for-reading-and-navigating).

| Key | Default | Values | What it does | Repository may change it? |
|---|---|---|---|---|
| `max_file_bytes` | `1048576` (1 MiB) | Bytes | Largest file Damaian reads whole, indexes, or scans for secrets before a commit | Yes |
| `max_read_lines` | `400` | 1 or more | Lines one unranged file read returns before it is cut short | Lower only |
| `max_list_entries` | `200` | 1 or more | Entries one folder listing returns | Lower only |
| `max_search_matches` | `50` | 1 or more | Matches one content search returns | Lower only |
| `max_match_line_chars` | `500` | 1 or more | Characters kept of each matching line | Lower only |
| `repository_map_max_tokens` | `800` | 0 or more; `0` leaves the map out | Tokens the [repository map](USER_GUIDE.md#the-map-the-assistant-sees) may use per turn | Lower only |
| `symbol_context_max_tokens` | `1600` | 0 or more; `0` leaves symbol hints out | Tokens the symbol hints may use when a prompt names a symbol ([Symbol Search and Language Servers](USER_GUIDE.md#symbol-search-and-language-servers)) | Lower only |
| `conversation_auto_compact` | `true` | `true`, `false` | Summarise older conversation automatically when it outgrows its window ([Long conversations and summaries](USER_GUIDE.md#long-conversations-and-summaries)) | No |
| `conversation_window_share` | `0.25` | A share from `0.0001` to `1` | How much of the context budget the conversation may fill before older messages are summarised | Lower only, and not below `0.05` |
| `pause_for_context_review` | `false` | `true`, `false` | Hold each turn before its first model call so you can review its context (**Pause before sending to review context**, [Inspecting and controlling context](USER_GUIDE.md#inspecting-and-controlling-context)) | No |
| `agent_max_turn_messages` | `24` | 1 or more | Most messages one model request carries inside a turn; older rounds beyond it are dropped from the request, not from the session ([Capping What One Turn May Spend](USER_GUIDE.md#capping-what-one-turn-may-spend)) | Lower only |
| `agent_max_turn_notes_chars` | `8000` | 0 to 32000; a larger value is reduced to 32000; `0` turns notes off | Characters of notes the assistant may keep per turn ([Notes kept during and between turns](USER_GUIDE.md#notes-kept-during-and-between-turns)) | Lower only |

### Context shares: `context_budget.<category>`

Each share is the part of the context budget one category may use, as a decimal. The shares together may add up to at most `1`; more is a configuration error. The categories and their order are explained in [What goes into context](USER_GUIDE.md#what-goes-into-context).

| Key | Default | Values | What it does | Repository may change it? |
|---|---|---|---|---|
| `context_budget.instructions` | `0.10` | A share | `AGENTS.md` files and project rules | Lower only |
| `context_budget.hook_context` | `0.05` | A share | Context added by [hooks](USER_GUIDE.md#hooks) | Lower only |
| `context_budget.pinned` | `0.15` | A share | Files you named or added | Lower only |
| `context_budget.plan_state` | `0.05` | A share | A plan carried into the turn | Lower only |
| `context_budget.repository_map` | `0.05` | A share | The repository map | Lower only |
| `context_budget.symbols` | `0.10` | A share | Symbol hints | Lower only |
| `context_budget.search_results` | `0.10` | A share | Keyword and semantic search hits | Lower only |
| `context_budget.file_ranges` | `0.25` | A share | File content | Lower only |
| `context_budget.findings` | `0.02` | A share | [Findings](USER_GUIDE.md#findings) | Lower only |
| `context_budget.conversation_summary` | `0.05` | A share | The summary of older conversation | Lower only |
| `context_budget.external_reference` | `0.05` | A share | Pages fetched in earlier turns ([Reading an external page](USER_GUIDE.md#reading-an-external-page)) | Lower only |
| `context_budget.memory` | `0.03` | A share | Remembered facts ([Memory](USER_GUIDE.md#memory)) | Lower only |
| `context_budget.safety_margin` | `0.10` | A share | Part of a measured provider window held back for estimate error | Raise only |

## Agent rounds and budgets

See [While a turn runs, and how it ended](USER_GUIDE.md#while-a-turn-runs-and-how-it-ended).

| Key | Default | Values | What it does | Repository may change it? |
|---|---|---|---|---|
| `agent_max_tool_rounds` | `16` | 1 to 16 | Action rounds before a turn pauses at a checkpoint | Yes |
| `agent_web_debug_max_tool_rounds` | `16` | 1 to 16 | The same limit for a turn that debugs a web page or uses a [browser diagnostic](USER_GUIDE.md#browser-diagnostics) tool | Yes |
| `agent_max_exploration_rounds` | `24` | 0 or more; a value above 32 is reduced to 32; `0` counts every read as an action round | Read-only rounds before reads start counting as action rounds | Lower only |
| `agent_tool_retry_limit` | `16` | 1 to 32 | How often a failing tool call may be retried before the model is told to change approach, and the repair attempts after a failing check ([Verification](USER_GUIDE.md#verification)) | Yes |
| `agent_max_questions_per_turn` | `2` | 0 to 4; `0` stops the assistant asking | Questions the assistant may ask you in one turn ([When the assistant asks you a question](USER_GUIDE.md#when-the-assistant-asks-you-a-question)) | Yes |
| `agent_max_task_tokens` | None (no cap) | 1000 or more | Tokens one turn may spend before it stops ([Capping What One Turn May Spend](USER_GUIDE.md#capping-what-one-turn-may-spend)) | Lower only; a repository may also set one where you have none |

## Workers

See [Worker Sessions](USER_GUIDE.md#worker-sessions). The `worker_task_budget_*` totals also bound how far an ordinary Code-mode turn carries on by itself.

| Key | Default | Values | What it does | Repository may change it? |
|---|---|---|---|---|
| `enable_worker_sessions` | `false` | `true`, `false` | Let a session hand work to a worker session | No |
| `worker_preplan_exploration_rounds` | `72` | 1 to 384 | Read-only rounds a worker may spend before it must propose a plan | Lower only |
| `worker_task_budget_action_rounds` | `96` | 1 to 256 | Total action rounds a plan review proposes for a task | Lower only |
| `worker_task_budget_exploration_rounds` | `144` | 1 to 384 | Total read-only rounds a plan review proposes | Lower only |
| `worker_task_budget_input_tokens` | `60000000` | 1 to 64000000 | Total input tokens a plan review proposes | Lower only |
| `worker_task_budget_output_tokens` | `512000` | 1 to 4000000 | Total output tokens a plan review proposes | Lower only |
| `worker_task_budget_max_action_rounds` | `256` | 1 to 256 | Ceiling on a task's action rounds | Lower only |
| `worker_task_budget_max_exploration_rounds` | `384` | 1 to 384 | Ceiling on a task's read-only rounds | Lower only |
| `worker_task_budget_max_input_tokens` | `64000000` | 1 to 64000000 | Ceiling on a task's input tokens | Lower only |
| `worker_task_budget_max_output_tokens` | `4000000` | 1 to 4000000 | Ceiling on a task's output tokens | Lower only |

## Commands and approvals

See [Command Approval](USER_GUIDE.md#command-approval).

Five of these keys are **planned to be replaced (spec 111)** by one list of command rules: `command_allowlist`, `command_allowlist.<repository_id>`, `command_blocklist`, `require_approval_for_risky_commands` and `command_access`. They work as described here until that change ships.

| Key | Default | Values | What it does | Repository may change it? |
|---|---|---|---|---|
| `shell` | `$SHELL`, otherwise `/bin/zsh` | A path to a shell | The shell that runs commands | No |
| `command_path_from_interactive_shell` | `true` | `true`, `false` | Commands find programs on the `PATH` your Terminal has ([Which programs a command finds](USER_GUIDE.md#which-programs-a-command-finds)) | No |
| `command_timeout_secs` | `600` | 1 or more | Seconds a command may run before it is stopped and reported as timed out | Lower only |
| `max_command_output_bytes` | `1048576` (1 MiB) | Bytes | Command output kept, per stream | Yes |
| `require_approval_for_all_commands` | `false` | `true`, `false` | Ask before every command, read-only ones included | Turn on only |
| `require_approval_for_risky_commands` | `true` | `true`, `false` | Ask before test, build and lint commands. Planned to be replaced (spec 111) | Turn on only |
| `command_allowlist` | Empty | Commands, separated by `\|` | Commands that run without asking, matched exactly. Planned to be replaced (spec 111) | No |
| `command_allowlist.<repository_id>` | None | Commands, separated by `\|` | The commands you allowed with *Always allow in this project* for one repository ([Allow Always](USER_GUIDE.md#allow-always)). Damaian writes these lines. Planned to be replaced (spec 111) | No |
| `command_blocklist` | Empty | Commands, separated by `\|` | Commands that are always blocked: any command that starts with an entry. Planned to be replaced (spec 111) | Adds to yours |
| `command_access` | `all` | `none`, `read_only`, `local`, `all` | Which commands may run at all ([Permission Profiles](USER_GUIDE.md#permission-profiles)). Planned to be replaced (spec 111) | Lower only (toward `none`) |

## Files and paths

| Key | Default | Values | What it does | Repository may change it? |
|---|---|---|---|---|
| `data_dir` | `DAMAIAN_DATA_DIR`, otherwise `~/Library/Application Support/DamaianClient` | A path | Where Damaian keeps sessions, audit records, checkpoints and other data ([Local Data](USER_GUIDE.md#local-data)). It does not move `user.conf`, which is always read from the default location | No |
| `allowed_roots` | Empty (any folder) | Paths, separated by `\|` | Folders Damaian may open as a repository. Also the folders outside a repository whose worktrees the assistant may read ([Agent tools for reading and navigating](USER_GUIDE.md#agent-tools-for-reading-and-navigating)) | No |
| `ignore_patterns` | `.git/`, `node_modules/`, `vendor/`, `.venv/`, `venv/`, `dist/`, `build/`, `target/`, `coverage/`, `.damaian/`, `*.min.js`, `*.map` and `.gitignore` | Patterns, separated by `\|` | Files left out of the index. Setting it replaces the built-in list | Adds to yours |
| `restricted_patterns` | `.env` files, `*.pem`, `*.key`, `*.p12`, `*.pfx`, SSH private keys, and `secrets/` and `credentials/` directories | Patterns, separated by `\|` | Files Damaian never reads or sends. Setting it replaces the built-in list, so repeat the built-in entries you want to keep | Adds to yours |
| `secret_patterns` | Empty | Text, separated by `\|` | Extra text redacted as a secret wherever it appears, on top of the built-in detectors. Each entry is matched literally | No |
| `block_generated_secrets` | `true` | `true`, `false` | Stop a change that adds a hardcoded secret ([Hardcoded secret warnings](USER_GUIDE.md#hardcoded-secret-warnings)) | No |
| `require_approval_for_file_edits` | `true` | `true`, `false` | Ask before changing files ([File Changes](USER_GUIDE.md#file-changes)) | Turn on only |
| `project_roots_added` | Empty | Repository-relative folders, separated by `\|` | Folders you made project roots ([Repository Roots](USER_GUIDE.md#repository-roots)). Settings writes this to the repository's own config | Yes |
| `project_roots_removed` | Empty | Repository-relative folders, separated by `\|` | Detected roots you said are not projects | Yes |

## Permissions and Git

See [Permission Profiles](USER_GUIDE.md#permission-profiles), [Commits](USER_GUIDE.md#commits) and [Branches](USER_GUIDE.md#branches).

| Key | Default | Values | What it does | Repository may change it? |
|---|---|---|---|---|
| `permission_profile.<repository_id>` | None (the Full profile) | `read_only`, `safe_local`, `full`, `offline_private`, or a custom profile name | The permission profile you chose for one checkout. Damaian writes these lines when you pick a profile | No |
| `allow_file_edits` | `true` | `true`, `false` | Whether file changes are allowed at all | Turn off only |
| `allow_git_commits` | `true` | `true`, `false` | Whether Damaian may create commits | Turn off only |
| `allow_git_branches` | `true` | `true`, `false` | Whether Damaian may create, switch or delete branches | Turn off only |
| `git_branch_prefix` | `damaian/` | Text, at most 200 characters | The prefix of a suggested branch name | Yes |
| `allow_browser_diagnostics` | `true` | `true`, `false` | Whether [browser diagnostics](USER_GUIDE.md#browser-diagnostics) may run | Turn off only |
| `allow_mutating_mcp_tools` | `true` | `true`, `false` | Whether MCP tools that are not marked read-only may be offered | Turn off only |

## Memory

See [Memory](USER_GUIDE.md#memory) and [How memory reaches the model](USER_GUIDE.md#how-memory-reaches-the-model).

| Key | Default | Values | What it does | Repository may change it? |
|---|---|---|---|---|
| `memory_user_scope_enabled` | `false` | `true`, `false` | Let memory hold facts that apply across projects | No |
| `memory_max_entries` | `5` | 0 or more; `0` recalls nothing | Most remembered entries recalled into one turn | Lower only |
| `memory_retention_days` | `0` (no expiry) | Days | Days after which a remembered entry expires | No |

## Checkpoints and audit

See [Rewind](USER_GUIDE.md#rewind) and [Local Data](USER_GUIDE.md#local-data).

| Key | Default | Values | What it does | Repository may change it? |
|---|---|---|---|---|
| `audit_enabled` | `true` | `true`, `false` | Turns the local audit log on or off | No |
| `audit_retention_days` | `90` | Days | How long audit records are kept | Yes |
| `checkpoint_retention_days` | `90` | Days | How long session checkpoints are kept | No |
| `checkpoint_max_total_bytes` | `1073741824` (1 GiB) | Bytes | Most checkpoint storage for one repository; the oldest checkpoints are removed past it | No |
| `checkpoint_census_max_paths` | `1000` | A whole number | Changed or untracked files a command checkpoint may cover; past it, a command's effects are reported as not covered | No |

## MCP servers

See [MCP Servers](USER_GUIDE.md#mcp-servers). `allow_mutating_mcp_tools` is under [Permissions and Git](#permissions-and-git).

| Key | Default | Values | What it does | Repository may change it? |
|---|---|---|---|---|
| `mcp_enabled` | `true` | `true`, `false` | Turns every MCP server on or off | Turn off only |
| `mcp_server_allowlist` | Empty (no restriction) | Server ids, separated by `\|` | When set, only the listed servers are offered, even if others are enabled | Narrow only; a repository may also set one where you have none |

### Server definitions: `mcp_server.<id>.<field>`

`<id>` is lower-case letters, digits, `.` and `-`. A repository may suggest a new server: it is created switched off and asking before each call, whatever the file says, so it does nothing until you turn it on. A repository cannot change a server you configured, except to switch it off or make it ask.

| Key | Default | Values | What it does | Repository may change it? |
|---|---|---|---|---|
| `mcp_server.<id>.label` | The id | Text | The name Settings shows | Only for a new server |
| `mcp_server.<id>.transport` | `stdio` | `stdio` (also `local`), `http` (also `https`, `sse`, `streamable-http`, `remote`) | Start a local program, or connect to a remote URL | Only for a new server |
| `mcp_server.<id>.command` | Empty | An executable, or a whole command line when `args` is empty | stdio: the program to start | Only for a new server |
| `mcp_server.<id>.args` | Empty | Arguments, separated by `\|` | stdio: its arguments | Only for a new server |
| `mcp_server.<id>.env` | Empty | `NAME=value` pairs, separated by `\|` | stdio: extra environment variables | Only for a new server |
| `mcp_server.<id>.url` | Empty | A URL; a trailing `/` is removed | http: the server's address | Only for a new server |
| `mcp_server.<id>.auth_token_env` | Empty (no token) | `keychain:<account>` or an environment variable name | http: where the bearer token is read from | Only for a new server |
| `mcp_server.<id>.enabled` | `false` | `true`, `false` | Offer this server's tools to the assistant. The Settings form ticks it for a new server | Turn off only |
| `mcp_server.<id>.require_approval` | `true` | `true`, `false` | Ask before each tool call on this server | Turn on only |

## Hooks

See [Hooks](USER_GUIDE.md#hooks), and [Configuring a hook](USER_GUIDE.md#configuring-a-hook) for an example.

`<id>` is 1 to 40 lower-case letters, digits or `_`. A hook needs at least `command` and `events`. A repository may define a hook of its own, which runs only after you review it ([Repository hooks and review](USER_GUIDE.md#repository-hooks-and-review)), but cannot change any field of a hook you or an admin defined. A permission profile carries no hooks.

| Key | Default | Values | What it does | Repository may change it? |
|---|---|---|---|---|
| `hook.<id>.command` | None (required) | An executable | The program the hook runs, directly and not through `shell` | Its own hooks only |
| `hook.<id>.args` | Empty | Arguments, separated by `\|` | Its arguments | Its own hooks only |
| `hook.<id>.events` | None (required) | One or more of `session_start`, `before_context`, `before_tool`, `after_tool`, `before_patch_apply`, `after_patch_apply`, `after_check`, `before_task_complete`, `session_end`, separated by `\|` | When the hook runs ([The nine events](USER_GUIDE.md#the-nine-events)) | Its own hooks only |
| `hook.<id>.mode` | `mandatory` | `mandatory`, `optional` | Whether a failing hook blocks the action or only warns | Its own hooks only |
| `hook.<id>.order` | `0` | An integer | Hooks for one event run by `order`, then by id | Its own hooks only |
| `hook.<id>.timeout_ms` | Per event: 2000, 5000 or 10000 | 1 to 60000 | Milliseconds the hook may run | Its own hooks only |
| `hook.<id>.enabled` | `true` | `true`, `false` | Whether the hook runs | Its own hooks only |
| `hook_review.<repository_id>.<hook_id>` | None | `sha256:<hex>` or `declined` | Your decision about one repository's hook. Damaian writes these lines when you review a hook | No |

## Language servers and search

See [Semantic search](USER_GUIDE.md#semantic-search) and [Language servers](USER_GUIDE.md#language-servers).

| Key | Default | Values | What it does | Repository may change it? |
|---|---|---|---|---|
| `enable_semantic_search` | `false` | `true`, `false` | Add embedding-based search. Downloads a small model on first use | Yes |
| `language_server.<language>` | None | A command line. `<language>` is one of `go`, `java`, `javascript`, `kotlin`, `php`, `python`, `rust`, `typescript` | The language server Damaian starts for that language | No |

## Web

See [Reading an external page](USER_GUIDE.md#reading-an-external-page). `allow_browser_diagnostics` is under [Permissions and Git](#permissions-and-git).

| Key | Default | Values | What it does | Repository may change it? |
|---|---|---|---|---|
| `web_fetch_max_per_turn` | `8` | 0 to 20; `0` withholds the tool | Pages the assistant may fetch in one turn | Lower only |
| `web_domain_allowlist` | Empty | Host names, separated by `\|` | Hosts pages may be read from without asking, in every repository | No |
| `web_domain_allowlist.<repository_id>` | None | Host names, separated by `\|` | The hosts you allowed with *Always allow this host in this project* for one repository. Damaian writes these lines | No |

## Per-repository keys

Four key families above are keyed by a repository id, such as `repo_3f9a…`: `command_allowlist.<repository_id>`, `web_domain_allowlist.<repository_id>`, `permission_profile.<repository_id>` and `hook_review.<repository_id>.<hook_id>`. They hold your own decisions about one repository, so they belong in your `user.conf`, and Damaian writes them when you make the decision. A repository's config can never set them.

## Example

A user configuration with DeepSeek as the default provider, OpenRouter as a second provider, and one local MCP server. Store the two keys in Keychain from **Settings › Providers** first; only the references appear here.

```text
# Default provider and model
model_provider=deepseek
model_name=deepseek-v4-flash

# DeepSeek: key in Keychain, and prices so each turn shows its cost
model_provider.deepseek.api_key_env=keychain:deepseek-api-key
model_provider.deepseek.price_per_million_input_tokens=0.27
model_provider.deepseek.price_per_million_output_tokens=1.10

# OpenRouter: a shorter model list and an output ceiling
model_provider.openrouter.api_key_env=keychain:openrouter-api-key
model_provider.openrouter.models=anthropic/claude-sonnet-5.5|anthropic/claude-opus-5.5
model_provider.openrouter.max_output_tokens=32000

# A local MCP server, switched on, asking before each call
mcp_server.notes.label=Notes
mcp_server.notes.transport=stdio
mcp_server.notes.command=/opt/homebrew/bin/npx
mcp_server.notes.args=-y|@modelcontextprotocol/server-filesystem|/Users/me/notes
mcp_server.notes.enabled=true
mcp_server.notes.require_approval=true

# Stop a command after five minutes
command_timeout_secs=300
```

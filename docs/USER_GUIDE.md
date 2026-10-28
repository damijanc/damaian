# Damaian User Guide

Damaian is a local-first AI coding assistant for macOS. It indexes a local Git repository, prepares focused context for coding questions, previews generated edits as diffs, and records local audit data.

This build is a developer preview. Release builds are Developer ID signed and notarized, and update themselves from GitHub Releases. The feature set is still moving, and configuration and stored data may change shape between releases.

## Quick start

1. Install Damaian and open `Damaian.app`. See [macOS Installation](MACOS_INSTALLATION.md#install).
2. Open Settings with `⌘,`. It opens on **Providers**.
3. Under **Popular providers**, select **+ Connect** beside your provider, or select **New** beside **Configured providers** for any other OpenAI-compatible endpoint. Check **Base URL** and **Models** in **Provider details**.
4. Paste your key into **API Key**. While **API Key Reference** starts with `keychain:` (Damaian fills one in), the key is stored in macOS Keychain and only the reference is written to your config.
5. Leave **Native tool-calling** on (the default), then select **Save Provider**. Without it the assistant can answer questions but cannot propose file changes ([why](#native-tool-calling)).
6. Close Settings. In the model picker below the prompt, choose your provider under **Provider** ([Choosing the model and effort](#choosing-the-model-and-effort)).
7. Select **+** beside **Projects** and pick the Git repository you want to work on.
8. Ask a question in the prompt box and press `Enter`. To change code, ask for the change, review the patch preview in the conversation, and select **Apply N** ([File Changes](#file-changes)).

## Contents

**Setup**

- [Quick start](#quick-start)
- [Updates](#updates)
- [Settings](#settings)
- [Model Providers and API Keys](#model-providers-and-api-keys)
- [Output and Context Limits](#output-and-context-limits)

**Daily use**

- [Working Folder](#working-folder)
- [Terminal](#terminal)
- [Chat](#chat)
- [File Changes](#file-changes)
- [Verification](#verification)
- [Commits](#commits)
- [Rewind](#rewind)
- [The Plan Panel](#the-plan-panel)
- [Memory](#memory)
- [Search and Export](#search-and-export)
- [Symbol Search and Language Servers](#symbol-search-and-language-servers)
- [After a Crash](#after-a-crash)

**Control**

- [Working Modes](#working-modes)
- [Permission Profiles](#permission-profiles)
- [Command Approval](#command-approval)
- [Repository Roots](#repository-roots)

**Advanced**

- [Worker Sessions](#worker-sessions)
- [MCP Servers](#mcp-servers)
- [Hooks](#hooks)
- [Browser Diagnostics](#browser-diagnostics)
- [Findings](#findings)
- [Provider Limits and Retries](#provider-limits-and-retries)
- [Capping What One Turn May Spend](#capping-what-one-turn-may-spend)
- [What a Turn Cost](#what-a-turn-cost)

**Reference**

- [Local Data](#local-data)
- [The Command-Line Tool](#the-command-line-tool)
- [Safety Model](#safety-model)
- [Troubleshooting](#troubleshooting)

## Updates

Damaian checks for updates when the desktop app starts, and on demand from **Damaian › Check for Updates...**. If a newer GitHub Release is available, the button at the bottom of the sidebar changes to **Update &lt;version&gt;**. Select it and confirm **Install** to download and install the signed update, then quit and reopen Damaian to finish.

The first installed version must already include the updater. If you installed an older developer-preview DMG before automatic updates were added, download and install one newer DMG manually once.

## Settings

Open Settings with `⌘,` or **Damaian › Settings...**; Escape or **×** closes it. It opens on **Providers**, and its other pages are **General**, **Shortcuts**, **Checkpoints**, **MCP Servers**, **Hooks** and **Models**. Your user configuration is under **General › User configuration**: edit it and select **Save**, or select **Load** to re-read the file from disk.

These are your user-scope settings, and they are kept in one file:

```text
~/Library/Application Support/DamaianClient/config/user.conf
```

Editing that file by hand does the same thing as editing it in Settings, which is how you configure Damaian for the [command-line tool](#the-command-line-tool), which has no Settings window. Note the `config/` directory in the middle of that path, and the `.conf` name: a file dropped next to it as `config.conf` is not user config and is read by nothing.

Configuration uses one `key=value` entry per line. Edit values directly and select **Save**. Delete a line and save to remove that user-level override.

`model_api_key_env` is a reference field. The app rejects raw API keys in this field; use the **Model API Key** controls to store the secret in Keychain (see [Model Providers and API Keys](#model-providers-and-api-keys)).

Common keys:

| Key | Default | What it does | Can a repository change it? |
|---|---|---|---|
| `restricted_patterns` | `.env` files, `*.pem`, `*.key`, SSH private keys, `secrets/` and `credentials/` directories, and similar | Files Damaian never reads or sends, separated by `\|` | Adds to yours; cannot remove any |
| `ignore_patterns` | Built-in list | Files Damaian does not index | Adds to yours; cannot remove any |
| `audit_enabled` | `true` | Turns the local audit log on or off | No |
| `model_provider` | `openai` | The provider used when the model picker has no choice of its own | No |
| `model_name` | `gpt-4.1` | Model identifier | No |
| `model_base_url` | `https://api.openai.com` | OpenAI-compatible API base URL | No |
| `model_api_key_env` | `OPENAI_API_KEY` | API key reference: `keychain:model-api-key` for the desktop Keychain flow, or an environment variable name for [CLI and development](#environment-variables-cli-and-development-only) | No |
| `model_reasoning_level` | `default` | Reasoning effort: `default`, `minimal`, `low`, `medium` or `high` | No |
| `model_provider.<id>.supports_native_tools` | on | Use the provider's native tool-calling API ([Native tool-calling](#native-tool-calling)) | No |
| `model_provider.<id>.max_output_tokens` | Per model | Largest reply the model may generate, in tokens ([Output and Context Limits](#output-and-context-limits)) | No |
| `model_provider.<id>.context_token_budget` | Per model | How much repository content is sent with each request, in tokens | No |
| `enable_semantic_search` | `false` | Embedding-based retrieval ([Semantic search](#semantic-search)) | Yes |
| `enable_worker_sessions` | `false` | [Worker Sessions](#worker-sessions) | No |
| `agent_max_tool_rounds` | `8` | Action rounds before a turn pauses ([While a turn runs](#while-a-turn-runs-and-how-it-ended)) | Yes |
| `agent_max_exploration_rounds` | `24` | Read-only rounds before reads start counting as action rounds ([While a turn runs](#while-a-turn-runs-and-how-it-ended)) | Lower only |
| `agent_max_turn_notes_chars` | `8000` | Characters of notes the assistant may keep per turn ([Notes](#notes-kept-during-and-between-turns)) | Lower only |
| `agent_max_task_tokens` | none | Token cap per turn ([Capping What One Turn May Spend](#capping-what-one-turn-may-spend)) | Lower only |
| `command_timeout_secs` | `600` | Deadline for a command Damaian runs | Lower only |
| `require_approval_for_file_edits` | `true` | Ask before changing files | Turn on only |
| `require_approval_for_all_commands` | `false` | Ask before every command, read-only ones included | Turn on only |
| `checkpoint_retention_days` | `90` | How long [checkpoints](#rewind) are kept | No |
| `audit_retention_days` | `90` | How long audit records are kept | Yes |
| `repository_map_max_tokens` | `800` | Size of the [repository map](#the-map-the-assistant-sees) | Lower only |
| `symbol_context_max_tokens` | `1600` | Size of the symbol hints added when a prompt names one ([Symbol Search and Language Servers](#symbol-search-and-language-servers)) | Lower only |
| `context_budget.<category>` | Per category | Share of the context budget one category may use ([Output and Context Limits](#output-and-context-limits)) | Lower only |
| `context_budget.safety_margin` | `0.1000` | Share of a measured provider window held back for estimate error | Raise only |
| `language_server.<language>` | none | Command for an optional language server ([Language servers](#language-servers)) | No |

Repository-scoped settings are not edited from Settings. Put repository defaults in `.damaian/config.conf` inside the selected repository. Repository settings are included in the [effective policy view](#reading-the-effective-policy-view).

Because that file arrives with a clone, a repository can only make Damaian *more* careful, never less:

- `shell`, `data_dir`, `allowed_roots`, `secret_patterns`, `audit_enabled`, `block_generated_secrets`, `enable_worker_sessions`, `checkpoint_retention_days`, `checkpoint_max_total_bytes`, `checkpoint_census_max_paths`, every `language_server.<language>` key, and every `model_*` key are ignored in repository config. Damaian tells you once, per repository, when a repository tried to set one, and records it in the audit log.
- `restricted_patterns`, `ignore_patterns`, and `command_blocklist` are added to yours rather than replacing them.
- The `require_approval_for_*` flags can be turned on by a repository, not off. MCP and individual MCP servers can be turned off, not on.
- `command_allowlist` is never taken from repository config; *Allow Always* writes to your own config instead. See [Allow Always](#allow-always).
- Budgets and preferences — `max_file_bytes`, `max_command_output_bytes`, `audit_retention_days`, `enable_semantic_search`, `agent_max_tool_rounds`, and the root overrides `project_roots_added` and `project_roots_removed` ([Repository Roots](#repository-roots)) — apply as written.
- The caps on what one agent tool returns — `max_read_lines` (400), `max_list_entries` (200), `max_search_matches` (50), and `max_match_line_chars` (500) — and the repository map's `repository_map_max_tokens` (800) can be lowered by a repository but never raised. The same holds for `command_timeout_secs`, `agent_max_task_tokens`, `agent_max_turn_messages`, `agent_max_exploration_rounds`, and `agent_max_turn_notes_chars`. The first three bound how much one request may carry; `agent_max_exploration_rounds` bounds how many read-only rounds a turn may take; `agent_max_turn_notes_chars` bounds how much the assistant's notes add to every later request. Raising one would widen what leaves your machine or how long a turn may run. A repository that tries is told to you like any other rejected key.
- Each `context_budget.<category>` share can be lowered by a repository but never raised, and `context_budget.safety_margin` can be raised but never lowered. A share decides how much of the repository's own content is packed, so a clone may tighten it and may not widen it ([Output and Context Limits](#output-and-context-limits)).

### Semantic search

`enable_semantic_search` is off by default. Off, Damaian finds relevant files for a request, and answers the assistant's `search_codebase` tool, by keyword and term overlap. On, it also uses an embedding model for that search, which can find files that use different words for the same idea.

Turning it on downloads the embedding model on first use, into `models/all-MiniLM-L6-v2/` inside the [data directory](#local-data), so the first search afterwards is slower. If it seems to do nothing, see [Troubleshooting](TROUBLESHOOTING.md#semantic-search-does-nothing).

### Language servers

A language server is optional. Configured, it adds exact symbol locations to later searches, answers **Find references** exactly, and publishes diagnostics into [Findings](#findings). Unconfigured, symbols come from Damaian's source-text matching and every result is marked `likely`.

Add one line per language to your user config ([Settings](#settings)):

```text
language_server.rust=/absolute/path/to/rust-analyzer
language_server.typescript=/absolute/path/to/typescript-language-server --stdio
```

The key accepts the languages Damaian indexes: `rust`, `typescript`, `javascript`, `python`, `go`, `java`, `kotlin` and `php`. Any other language, or an empty command, is a configuration error naming the key. The value is a command line, split the way an MCP server's command is. An app opened from Finder or the Dock may not see your shell's `PATH`, so use an absolute path: a bare name that works in Terminal can fail in the app.

**A repository cannot set this key.** It names a program Damaian starts, so it has the same power as `shell`. A `language_server.<language>` line in `.damaian/config.conf` is ignored and recorded in the audit log as a rejected repository key; your own value stands. A permission profile cannot carry it either.

**What is sent.** The server is a local program you named, started with the repository root as its working directory. Damaian sends it the content of files it indexes (`textDocument/didOpen`), the `documentSymbol`, `references`, `implementation` and `definition` requests, and the lifecycle messages. It sends no prompts, model traffic or credentials; the server reads the same repository from disk anyway. Everything coming back — symbol names, locations and diagnostic text — is secret-scanned before Damaian stores or shows it, and a restricted file is never sent to a server.

**Lifetime.** A server starts on the first lookup that needs it and stops after ten idle minutes. At quit, a server sees its input close, which most servers treat as a request to exit, and the next launch's sweep removes anything left; the standalone shell also shuts its servers down explicitly. A server that crashes is restarted once; if it crashes again it stays down until its entry expires or Damaian restarts. A server that fails to start, or never answers `initialize`, is unavailable, and Damaian carries on with the heuristic symbols. Each server's PID and start time are recorded, and a process whose start time no longer matches is never killed. Nothing is ever killed by name, so your editor's own `rust-analyzer` is safe.

**Settings › General › Language servers** lists every configured language: `idle` before it has started, `ready` with its PID while it runs, or `unavailable`/`crashed` with the reason. The same state is served at `GET /api/language-servers`.

The first symbol search for a repository starts the pass in the background, and the search itself answers from the index as it was, so it returns at once. Once the pass finishes, later searches show the server's exact definitions, and diagnostics it published while opening those files appear in [Findings](#findings) on the next refresh. See [Troubleshooting](TROUBLESHOOTING.md#symbol-search-shows-only-likely-results-or-a-language-server-will-not-start) if results stay `likely` or a server will not start.

## Model Providers and API Keys

Damaian uses OpenAI-compatible chat APIs. Configure providers under **Settings › Providers**, but do not paste the API key into the configuration file.

### Adding a provider

The **Providers** page lists your **Configured providers**, then **Popular providers** (OpenAI, DeepSeek, OpenRouter, OpenAI compatible, Ollama (local) and Ollama Cloud) that you have not configured yet. **+ Connect** on a popular provider, or **New**, fills the **Provider details** form below:

- **Name** and **ID** identify the provider. Built-in providers keep their ID.
- **Base URL** is the OpenAI-compatible API address.
- **API Key Reference** and **API Key**: when the reference starts with `keychain:`, a key pasted into **API Key** is stored in macOS Keychain under that account when you save. Only the reference is written to your config. A key cannot be saved against any other kind of reference.
- **Models** lists the model ids to offer, one per line.
- **Native tool-calling** ([below](#native-tool-calling)).
- **Max output tokens** and **Context budget** ([Output and Context Limits](#output-and-context-limits)).

Select **Save Provider** to write it to your user config, or **Remove** to delete it. **Settings › Models** lists each configured provider's models, with **Configure** to open it here. Saving a provider does not make it the one chat uses: pick it in the composer's [model picker](#choosing-the-model-and-effort), or set `model_provider`.

**Ollama (local)** uses `http://localhost:11434` and needs the [Ollama app](https://ollama.com/download) installed and running on this Mac. It does not check API keys, so you do not need to store one: leave **API Key** blank and keep the filled-in Keychain reference. **Ollama Cloud** uses `https://ollama.com`; create a key under **Keys** in your [ollama.com account settings](https://ollama.com/settings/keys), then paste it into **API Key**.

### The Model API Key panel

The **Model API Key** panel at the bottom of **Providers** stores the key named by `model_api_key_env`:

1. Enter a Keychain account name in **Keychain Account**, such as `model-api-key`.
2. Paste the API key into **API Key**.
3. Select **Save Key**.

Damaian stores the secret in macOS Keychain and writes only this reference to config:

```text
model_api_key_env=keychain:model-api-key
```

Damaian keeps a process-local in-memory copy after a successful Keychain save or read. You may be asked by macOS the first time the app accesses the key after launch, but repeated chat, edit, or command-assisted answers in the same app run should not require another password prompt.

Use **Remove Key** to delete the stored secret from Keychain. Saving a new key with the same account replaces the previous value.

If the [effective policy view](#reading-the-effective-policy-view) still shows a different `model_api_key_env` after saving the key, a repository or admin config is overriding the user setting. Remove or update that override before retrying chat.

### Native tool-calling

**Native tool-calling** is on by default for built-in and user-defined providers (or set `model_provider.<id>.supports_native_tools=false` to turn it off). With it the assistant can propose file changes, make plans, and use [MCP](#mcp-servers) and the other tools. Without it the assistant can answer questions but cannot do any of those. Turn it off only for a provider that does not implement the OpenAI tool-calling API reliably.

### Configuration examples

Every example below is user-scope configuration: enter it in Settings, or write it to [`config/user.conf`](#settings) directly. From a source checkout, the [command-line tool](#the-command-line-tool) writes that file for you and prints the path it used:

```sh
cargo run -p damaian-cli -- config-set user model_provider deepseek
```

Naming a built-in provider fills in most of the setup. `model_provider=deepseek`, `model_provider=openrouter`, or `model_provider=openai` sets the base URL, the default model, and the key variable for that provider; set the other keys only to override one of those. [Native tool-calling](#native-tool-calling) is on by default.

Example DeepSeek configuration:

```text
model_provider=deepseek
model_name=deepseek-v4-flash
model_base_url=https://api.deepseek.com
model_api_key_env=keychain:model-api-key
```

The same pattern applies to OpenAI, OpenRouter, or any OpenAI-compatible provider. OpenRouter defaults to `https://openrouter.ai/api/v1` with `anthropic/claude-sonnet-5.5` and a starter list of tool-capable models from Anthropic, OpenAI, Google, DeepSeek, xAI, Moonshot, Z.ai and Qwen. Any other model OpenRouter serves can be added under **Models** (one id per line) or selected with `model_name`.

If an older configuration still names `deepseek-chat` or `deepseek-reasoner`, update it. Those were compatibility aliases that DeepSeek retired on 24 July 2026; the current models are `deepseek-v4-flash` and `deepseek-v4-pro`.

### Environment variables (CLI and development only)

Keychain is the way to give the desktop app a key. An app opened from Finder or the Dock does not see variables exported in your shell, so an environment-variable reference only works for the command-line tool and for a development build started from a shell.

In that mode, `model_api_key_env` is the name of an environment variable that contains the key. It is not the key itself:

```text
model_provider=deepseek
model_name=deepseek-v4-flash
model_base_url=https://api.deepseek.com
model_api_key_env=DEEPSEEK_API_KEY
```

Then set the variable in the shell that runs the command-line tool or the development build (see [DEVELOPMENT.md](DEVELOPMENT.md)):

```sh
export DEEPSEEK_API_KEY="your-deepseek-api-key"
```

To check that the settings were picked up before spending anything, print the merged configuration. This makes no model call:

```sh
cargo run -p damaian-cli -- config-show | grep '^model'
```

If `model_api_key_env` names a variable you did not choose, the configuration was not read at all and the built-in default is standing. That is a wrong path or a wrong filename, not a wrong key.

## Output and Context Limits

Two settings control how many tokens each request may use. They are independent: one bounds what the model writes, the other bounds what it reads. Both live in Settings under **Providers**, in the **Provider details** panel, and both apply per provider.

- **Max output tokens** caps the reply the model generates in a single response. Set too low, a large multi-file patch is cut off mid-way. Damaian detects this, tells the model its call was truncated, and asks it to retry with fewer files per patch, so the request recovers instead of failing silently.
- **Context budget** caps how much repository content Damaian packs into each request. Raising it lets the assistant see more of a large repository, which can improve answers. This content is re-sent and re-billed on every turn, so a higher budget increases cost and latency on all requests, not just large ones.

Leave a field blank to use the built-in default for the selected model. The defaults are:

- `deepseek-v4-flash` and `deepseek-v4-pro`: 65536 output tokens, 64000 context budget.
- `deepseek-chat` and `deepseek-reasoner` (retired aliases): 8192 output tokens, 16000 context budget.
- Other DeepSeek models: 8192 output tokens, 16000 context budget.
- All other models: no output cap is sent, and the context budget is 16000.

The built-in context budgets are deliberately well below what these models technically accept. DeepSeek's V4 models allow up to 384000 output tokens and a 1M-token context window, but requesting the maximum on every turn is rarely worth the cost. Raise the values if your repository or your patches need more.

A value entered in the panel always wins over the built-in default. To give a V4 model its full output range:

```text
model_provider.deepseek.max_output_tokens=384000
```

Setting a value higher than the model actually allows causes the provider to reject the request outright, so prefer the built-in defaults unless you know the model's real limit.

### What goes into context

Every request is assembled the same way. The collected items are sent in one `Repository context:` block, followed by the conversation under `Recent conversation:` — a summary of older messages when one exists, then the most recent messages that fit the conversation window (each cut to 2,000 characters) — followed by your message under `User request:`. Your message appears once. Within the repository block each item is one section headed `--- <source>: <path> ---`; a fetched page has no path, so its header names the URL and fetch time instead. The source labels are listed in [Reading a turn's context manifest](TROUBLESHOOTING.md#reading-a-turns-context-manifest). Items are added in a fixed order: the files you named or added, the `AGENTS.md` files that apply to the collected paths, the root project rules, the repository map, symbol hints, hook context, a carried plan, keyword and semantic search hits, and last any page fetched in an earlier turn. A line range would append `:start-end` to the path; no source produces one yet, so every item is a whole file or a rendered block. A one-shot edit request sends the same items under its own `User edit request:` heading, which comes first because that request makes a single call.

Your message is secret-scanned before it is sent, and so is every earlier message replayed under `Recent conversation:`. Sessions store messages as you typed them, so this is what keeps a credential from an older turn out of a later request: a detected value is replaced by Damaian's redaction marker rather than sent in cleartext.

**Each category has a share of the budget.** `context_token_budget` is divided by category. The order below is the order the budget is shared, and the order content is dropped when it runs short:

| Category | Default share | What it holds |
|---|---|---|
| `required` | none | Your message. Never dropped and never truncated; it is subtracted before any category is allocated. |
| `instructions` | 10% | The `AGENTS.md` files for the collected paths, root first and each nested file after it, then the root project rules (`README.md`, `CONTRIBUTING.md`, `.editorconfig`, `package.json`, `pyproject.toml`, `Cargo.toml`, `go.mod`, `pom.xml`, `build.gradle`). When this category runs short the most specific `AGENTS.md` survives; the root project rules are dropped first, then broader instruction files. |
| `hook_context` | 5% | Text a [hook](#hooks) returned for this turn. |
| `pinned` | 15% | Files and line ranges you pinned for the session in the [context inspector](#inspecting-and-controlling-context). |
| `plan_state` | 5% | The plan a continued or resumed turn carries forward, as numbered steps with their status and detail. |
| `repository_map` | 5% | The [repository map](#the-map-the-assistant-sees) block, still capped by `repository_map_max_tokens`. |
| `symbols` | 10% | The symbol hints block, still capped by `symbol_context_max_tokens`. |
| `search_results` | 10% | Files found by search: exact keyword hits first, then semantic-similarity hits. Semantic hits are dropped first when this category runs short. |
| `file_ranges` | 25% | Files you named in the prompt and files you added with **+** ([Chat](#chat)); explicit paths are ordered ahead of mentioned ones. |
| `findings` | 5% | Reserved for the [findings](#fix-n-errors-and-the-findings-list) repair flow. The assistant still receives only the findings you select for a repair request, so nothing fills this automatically today and its share is released. |
| `conversation_summary` | 5% | The summary shown in the inspector's **Summary** row. It travels inside `Recent conversation:` rather than as a context item, so this share is released. |
| `external_reference` | 5% | Pages read with `fetch_url`, shown in the turn **after** the fetch. It is the last category declared, so a crowded context drops a fetched page before it drops repository content. |

A category that uses less than its share releases the remainder to the categories after it. In a repository with no hook context, no carried plan, no pinned context, no findings, no conversation summary and no fetched page, the six unused shares add up to 40% of the budget released to the categories that do have content. A category that wants more than its share is shortened within itself — its lowest-priority items are dropped, or the map or symbols block is re-rendered smaller — and it never takes from another category.

Each share is its own key in your user config ([Settings](#settings)), written as a decimal fraction with at most four places:

```text
context_budget.instructions=0.1000
context_budget.hook_context=0.0500
context_budget.pinned=0.1500
context_budget.plan_state=0.0500
context_budget.repository_map=0.0500
context_budget.symbols=0.1000
context_budget.search_results=0.1000
context_budget.file_ranges=0.2500
context_budget.findings=0.0500
context_budget.conversation_summary=0.0500
context_budget.external_reference=0.0500
context_budget.safety_margin=0.1000
```

The defaults sum to exactly `1.00`; a set that sums above `1.00` is a configuration error naming the key family rather than a silent scale-down. `damaian config-show` prints the resolved values. `context_budget.safety_margin` is reserved from a *measured* provider window and never from the declared budget; Damaian does not measure a window today, so the limit is `context_token_budget` used unchanged. See [Troubleshooting](TROUBLESHOOTING.md#reading-a-turns-context-manifest) for what a turn's manifest records.

A repository's `.damaian/config.conf` may lower a share and raise the margin, and never the other way: a share steers traffic toward or away from the repository's own content, and a clone may tighten a bound but not widen one. A raise there is refused and reported like any other rejected key. A permission profile has the same one-way limit.

### Inspecting and controlling context

The **Context** control in the conversation header shows this turn's assembled
context as `tokens / limit`. Open it to see what Damaian is about to send, or
what an earlier turn sent:

- **Every category**, with the tokens it used and how many items it holds.
  Expanding a category lists its items by path, and a line range where one
  exists. The **Required** row is marked **locked**: it is your own message and
  Damaian's required context, and nothing in this view can remove it.
- **The symbols row** also says how many of the shown symbols are heuristic, for
  example `12 symbols (4 heuristic)`. A language-server symbol is a fact; a
  heuristic one is Damaian's guess.
- **The Summary row**, when the conversation is compacted, with its token cost
  and how many messages it covers.
- **Excluded**, collapsed by default, with one row per omitted item and the
  reason: a restricted path, a secret-bearing file, a category or total budget
  exhausted, a range merged into a wider one, a truncated block, a file too
  large, a binary file, an item you removed, a path outside this session's
  selection, or a pinned file that no longer exists.
- **The limit and its source** — declared from `context_token_budget`, or
  observed from a measured provider window. Damaian does not measure a window
  today, so every limit reads **declared**.

Every path in the panel, including in **Excluded**, opens in Visual Studio Code.
That includes a restricted path: the panel refuses to *send* its content, but you
may still read your own file. The panel itself only ever shows paths, ranges and
counts — never file content, and never a secret.

**Pins.** A pin is a file or a line range Damaian includes on every later turn
of the session, in the **Pinned** category. Use the pin control on any item row,
or the path/range form at the bottom of the panel (a range looks like `40-120`).
Pins are stored in the session, so they survive a restart. **Clear pins** removes
them. A pin that later becomes restricted, or whose file is deleted, stays in the
pin list and appears in **Excluded** with the reason, so restoring the file
restores the pin without you re-pinning it. Pins made in the panel differ from
the **+** composer chips: a chip is an attachment for the request you are typing
and is not carried into later turns.

**Path restriction.** Enter one path per line and choose **Apply paths** to make
only those files eligible for context in this session. The field shows the
session's current restriction when the panel opens. The restriction narrows; it
never widens. `restricted_patterns` and the path policy still apply on top, so
selecting a restricted path does not include it. The `AGENTS.md` hierarchy and
your own message are still included: repository instructions are policy rather
than one of the content files you are choosing between, and your message is
required. The **symbols** row is not filtered either: it is a lookup from the
identifiers in your message, not file content, so it can still name a symbol in
a file outside your selection. An empty list is a restriction that admits no
file content, not "no restriction": applying an empty field is refused, and
**Clear paths** removes the restriction.

**Reviewing before sending.** By default a turn sends as soon as its context is
assembled; the panel shows the manifest while the turn runs. Turn on **Pause
before sending to review context** in **Settings › Conversation** to hold every
turn at that point instead. The turn card then offers **Review context**,
**Send** and **Decline**. While it is paused, expanding a category gives each
optional item a **×** to remove it from this turn only; the removal appears under
**Excluded** as **removed by you**, and the token total falls. **Send** sends the
amended context, and **Decline** sends nothing at all. `Required` items have no
**×** and the API refuses their removal even if a client asks. The setting is
yours alone: a repository or a permission profile cannot turn it on or off.

**Past turns.** Each assistant message whose turn recorded a manifest has a
**Context · N tokens** link. It opens the same panel for that turn, read-only.
The link shows the manifest that was actually sent, including any item you
removed while the turn was paused.

### Long conversations and summaries

A long conversation cannot be sent whole on every turn. Damaian keeps a
**conversation window** of recent messages — 25% of the context budget by
default, so roughly 4,000 tokens on a 16,000-token budget — and represents
everything older with one **summary**. The window is measured in tokens rather
than messages, so a conversation of short messages keeps more turns in view than
one of long messages.

The summary has two halves, and only one is written by a model:

- **Read from your session, never guessed:** the original objective (quoted from
  your first message, not paraphrased), the files changed by accepted patches,
  the plan steps and their status, the approval decisions you made, and the
  validation results Damaian observed.
- **Written by the model:** the constraints you stated, the decisions and their
  rationale, approaches that failed, and what is still uncertain.

That split is deliberate. "Which files did we change" and "which checks passed"
are exactly the facts a model reconstructs plausibly and the engine already knows
exactly, and a summary is re-sent on every later turn, so an error in it would
become a belief the assistant holds for the rest of the session.

**When it runs.** Compaction happens at a turn boundary — before a request is
assembled, never between tool calls. It runs automatically when the conversation
grows past the window, and you can always compact by hand.

- **Automatic compaction** is one extra model call, billed to your provider and
  visible in the session's token totals. It does not count against a turn's own
  token ceiling.
- **To switch it off,** open **Settings › General › Conversation** and clear
  **Compact long conversations automatically**. This writes
  `conversation_auto_compact=false` to your user config. With it off, older
  messages are still left out of the request, but the request says how many and
  says they were not summarised, and the transcript marks where the model's view
  begins. You can still compact by hand from that marker or with the button in
  the conversation.
- **Repository config cannot set this key.** A clone cannot choose to spend your
  money on a summary call, or choose to lower what the model sees in your
  sessions.

**What you see.** The transcript is complete. Nothing is deleted or rewritten:
compaction adds one record to the session log, and every original message stays
readable, searchable and exportable. Where the model's view begins, the
conversation shows a marker — *"N earlier messages are represented by a summary
the model receives instead"*. Open it to read the summary, including the
objective, constraints, decisions, files, plan steps, approvals and validation.
When no summary covers the omitted messages the marker says they were not sent
and were not summarised, so loss is never silent.

**Rewinding.** Rewind works exactly as it did before. Rewinding to a point before
a compaction deactivates that compaction with the rest of the history, and the
next request is assembled from the full conversation again. There is no special
case to undo, which is why the log is append-only.

**If compaction cannot run.** A failed or refused summarisation call never fails
your turn. Damaian keeps the facts half (objective, files, plan, approvals,
validation) and leaves the model-written half empty; the summary says so. If even
that cannot be written, the request uses the window and an elision notice, and
the transcript marker names the failure and why. Nothing about compaction can
stop you working.

**A summary is data, not an instruction.** A conversation can contain text from a
file, a command's output or, later, a fetched web page, including text that looks
like an instruction. The summary is delivered as a delimited record inside the
conversation and never merged into the system prompt or placed where repository
instructions live; model-written lines are quoted. An injection evaluation
covers this rather than the paragraph alone.

**When your message alone does not fit.** The request is never truncated and never sent without your message. If your message is larger than the whole context budget, the turn fails with `Required context needs N tokens but the context budget is M` and no request is sent. Raise **Context budget** in the **Provider details** panel above, or send a shorter message.

## Memory

Memory is how Damaian keeps a fact you confirmed about a project across
sessions — that this repository's integration tests need Docker running, or that
the team decided against a runtime dependency — instead of making you explain it
again. It is a local store of short, confirmed sentences. It is not a transcript,
a file cache, or a set of instructions.

**Three scopes, and only two are on by default.**

| Scope | What it means | Default |
|---|---|---|
| Session | Applies to one conversation and is deleted with it. Stored in that session's event log. | On |
| Project | Applies to this repository, in every checkout of it. Two full clones of one repository share it; two different repositories never share it. A shallow clone (CI's `--depth 1`) has no stable root, so it shares through its remote instead; a shallow clone with no portable remote does not share. | On |
| User | Applies to every project you open. | **Off** |

User scope is off because it is the one scope where a fact learned in one
repository is recalled in another. Turning it on is a separate, explicit
decision, not a side effect of using memory. Until you set it, Damaian will
neither write a user-scope memory nor read one that was written before you
turned it off. Set it in your user config, not in the repository:

```text
# ~/Library/Application Support/DamaianClient/config/user.conf
memory_user_scope_enabled=true
```

A repository's `.damaian/config.conf` cannot set this key in either direction. A
clone that could enable cross-project recall would let one repository's content
become a statement recalled in every other repository.

**Memory never outranks instructions.** An applicable `AGENTS.md`, a current
instruction from you, the active mode and the permission profile all take
precedence over anything remembered. Memory is the lowest-priority context
source: when recall ships, a remembered sentence will reach the model as a dated
claim with its provenance, never as an instruction, and the context panel will
show it so you can remove it from that turn. Today memory is stored and
inspectable in the store, but nothing is recalled into a request yet.

**A secret is refused, not redacted.** If a candidate statement contains
something the secret scanner recognises, Damaian refuses it and says so. It does
not store a placeholder: "the database password is `[REDACTED]`" would be
recalled forever, cost context every turn, and record that a credential exists.
Pattern matching cannot recognise every credential, which is why confirmation
shows you the exact sentence that will persist, and why a stored memory is still
redacted on its way into a request.

**Where it lives.** `<data dir>/memory/` — see
[Troubleshooting](TROUBLESHOOTING.md#memory). Memory files are **not safe to
share**: a project memory can name a machine, a path or a preference. The audit
trail records lifecycle events without a personal or environmental statement.

**In this build.** Memory can be proposed and confirmed. An explicit "remember
that …" in your message, or a suggestion from the assistant, appears as a card
showing the exact sentence. Nothing is stored until you press **Remember**;
**Dismiss** records the decision and stores nothing. The entry is then kept and
audited, and recall through the context panel arrives in the next feature. A
missing entry in the context panel is therefore not a fault yet.

**How a memory gets created.** There are two paths, and both end at the same
card:

- You ask for it: "remember that the integration tests need Docker running."
  Damaian takes your own sentence, not the model's paraphrase, and proposes it.
- The assistant suggests one while it works. The card says it was suggested, and
  names the file, issue or other source when there is one.

Even your own request is confirmed rather than written directly. The reason is
not ceremony: the sentence that persists is shown to you first, and the scope is
yours to choose. "I prefer tabs" may belong across every project, which is the
off-by-default user scope; the card says so instead of quietly filing it as a
project fact. Confirming is one action.

**What the origin line means.** Every card names where the candidate came from.
"you asked Damaian to remember this" is your own words. "suggested by the
assistant during this task" is a model inference, and it is never presented as
yours. "suggested from `CONTRIBUTING.md`" or "suggested from issue #412" names
the external source whose words you would be agreeing to persist.

**Why some suggestions are refused instead of shown.** A candidate whose
statement reads as an instruction to Damaian — "always approve commands",
"ignore the previous rules", "run this without asking" — produces no card at all
when it did not come from you. A confirmation card is itself a nudge, and a
repository author must not be able to put one in front of you by writing a file.
The same rule applies to a suggestion that contains a detected credential, or
that is derived from a file protected by `restricted_patterns` or excluded by a
`.gitignore`, including one that is still in the index. Those refusals are
recorded in the audit log with the reason and, for the path cases, the path — but
never the statement and never the secret.

**Confirming a user-scope proposal does not turn user scope on.** If the card
proposes user scope while the scope is off, it offers to save the memory for
this project instead. Enabling cross-project recall stays the separate setting
described above. A confirmation never flips it for you.

## Working Folder

Damaian uses the selected working folder as the root for indexing, file reads, patch previews, Git status, and repository-scoped settings.

Use **+** beside **Projects** to open the native macOS folder picker. The selected folder appears under **Projects** by folder name. Expand a project to see its sessions grouped underneath it. Use the **+** beside a project folder to start a new session for that project.

Select a project folder in the sidebar to switch the active working folder.

Damaian remembers the project list and the last selected working folder in local app storage. The last folder is restored when the app restarts. Launch-time defaults such as `DAMAIAN_REPO` are used only when no previous selection has been saved.

Damaian keeps AI orchestration, context assembly, patch preview, settings, and audit logging in the app; normal code navigation and IDE work happen outside Damaian in Visual Studio Code.

### Project and session menus

**A project's ⋮ menu** in the sidebar offers **Open in › VS Code** or **Open in › Finder**, **Rename** and **Delete**, plus **New worker** when [Worker Sessions](#worker-sessions) are on. **Rename** changes only the name shown in the sidebar; an empty name goes back to the folder name. **Delete** asks first, then removes the project from the list only. Nothing on disk is deleted, and you can add the folder again later.

**A session's ⋮ menu**, shown on the open session and when you hover over or tab to a row, has **Rename**, **Export as Markdown** and **Delete** (which asks for confirmation). Double-clicking a session also renames it. Select an existing session to reload its conversation.

**The conversation header** holds only the session title, the terminal toggle and a ⋮ menu. The ⋮ menu's **Open in** submenu opens the selected working folder in **VS Code**, reveals it in **Finder**, or opens the **Terminal** panel there. Below it are **Rename session**, **Export as Markdown** and, last, **Delete session**, which asks for confirmation: the same actions as the open session's own menu.

### The repo bar

A strip directly above the prompt shows where the next turn will work:

- **The folder button** names the working folder. It opens a menu: **Show in Finder**, **Open repository on GitHub**, **Copy path**, **Change folder…** (the same folder picker as the **+** beside **Projects**) and **Open in terminal**. **Open repository on GitHub** appears only when the folder's `origin` remote is on github.com. Damaian rebuilds the address as `https://github.com/<owner>/<repo>` from the remote rather than opening the remote's text as it stands, because a cloned repository controls that text.
- **The branch** follows it in grey. On a detached HEAD it shows the short commit hash instead.
- **The change count**, `+N −M` on the right, is the working tree's lines added and removed against the last commit, with new untracked text files counted as added lines. It is hidden when there is nothing to count. It is information only; clicking it does nothing yet.
- **A contextual action** may appear at the far right. Today that is **Commit…** when the session has changes ([Commits](#commits)), or **Fix N errors** / **Fix N warnings** ([Findings](#fix-n-errors-and-the-findings-list)); errors and warnings take priority.

In a folder that is not a Git repository, the bar shows the folder button alone, and its menu still works. The bar refreshes when you open a session, after every turn, after you apply or roll back a patch, after a rewind, and when you return to the window (at most once every five seconds). It never polls on a timer.

## Terminal

Use the terminal icon in the conversation header to show or hide the bottom terminal panel. **Open in › Terminal** in the header's ⋮ menu and **Open in terminal** in the repo bar's folder menu open the same panel. The terminal opens in the selected working folder. If no folder is selected yet, it opens in your home directory.

Commands are entered manually by the user and run directly on the local machine. Use `cd` to change the panel's working directory, and `clear` to clear the panel output.

## Chat

Type in the prompt box at the bottom of the window to ask questions about the selected repository. Damaian retrieves relevant local files, redacts detected secrets, streams the answer, and shows the context files used for the response.

Press `Enter` to send a chat message. Press `Shift+Enter` to insert a new line.

Questions stream read-only answers. Code and file change requests, such as `create a test file` or `update the README`, generate an inline patch preview in the conversation. With Worker sessions on, a request to plan, split or hand off work stays in the conversation instead (see [Worker Sessions](#worker-sessions)).

To pin specific repository files into the next chat request, select **+** at the bottom left of the prompt and then **Add file**, or drag files onto the window. Pinned files appear as chips above the prompt and are included before automatic retrieval. Select **×** on a chip to remove that file.

If you name a file in your prompt, such as `USER_GUIDE.md` or `docs/USER_GUIDE.md`, Damaian attempts to include that file in the model context. A filename without a directory must uniquely match one file in the selected repository.

Damaian also reads `AGENTS.md` instruction files when present. A root `AGENTS.md`
applies across the selected repository. Nested `AGENTS.md` files apply to files
under their directory, and more specific nested instructions override broader
ones. These instructions are sent for chat answers and edit proposals, but they
do not override your request or Damaian's safety policy.

If the assistant needs a local command result to answer a question, it can ask Damaian to run commands. Read-only ones run without asking; everything else asks you first. See [Command Approval](#command-approval).

If the app cannot reach its own background service, a red banner above the conversation says **Desktop API unavailable**. It stays until a request succeeds again.

Context file buttons open the referenced file in Visual Studio Code.

### Choosing the model and effort

Below the prompt, after the **+** button and the [mode](#working-modes) pill, two buttons show the model and the reasoning effort the next turn will use. Either one opens the same menu:

- **Provider** lists your configured providers.
- **Model** lists that provider's models, with a **Custom model** field and **Apply** for a model id that is not listed.
- **Effort** offers **Default**, **Minimal**, **Low**, **Medium** and **Extra High**.
- **Reset to default** goes back to the provider, model and effort in your configuration (`model_provider`, `model_name` and `model_reasoning_level`).

The choice is remembered in the app for each working folder and sent with each request. It is not written to your config file.

### While a turn runs, and how it ended

Progress is shown inside the reply, not in the header. While a turn runs, a line in the assistant's message shows what it is doing ("Waiting for model", a tool's own label, "Streaming", "round 2/8", "round 2/8 · 12 reads" when the turn has also taken read-only rounds, "· notes 2.1k" when it has kept notes, or "Generating patch" for a file-change request), how long it has been running, and **Stop** when that kind of turn can be stopped. A chat message you send can be stopped. A turn that resumes after you approve a command, a plan or a fallback offer, and a patch being generated, show the line without Stop. When a turn finishes normally, the line disappears.

A round in which the assistant only reads — `read_file`, `list_directory`, `search_content`, `search_codebase`, `read_git_status`, `read_git_diff`, `list_worktrees` or `read_git_log`, with no repeated call — is a *read-only round*. It is charged to `agent_max_exploration_rounds` (24 by default) instead of `agent_max_tool_rounds`. A round that runs a command, proposes a change, asks a question or repeats an earlier read is an *action round* and is charged as before. Spending the read-only allowance never pauses a turn by itself: later reads are simply charged as action rounds. Set `agent_max_exploration_rounds=0` to charge every round the way Damaian did before this allowance existed.

### Notes kept during and between turns

To keep a long investigation from losing what it found, the assistant can write **notes** with a `turn_notes` tool: short plain text such as paths, what a component does, how pieces connect, and open questions. On every later request of that turn the notes are sent in full, even after older rounds have been dropped from the request to keep it bounded. They also carry into the next turn of the same session, so a turn that investigates and a turn that acts share one record instead of starting over.

The notes are bounded by `agent_max_turn_notes_chars` (8,000 characters by default, ceiling 32,000; a repository may lower it but never raise it). An append that would go over the bound is refused and the assistant is told the current size and to rewrite the record shorter with `replace`. Set it to `0` to turn the feature off completely: the tool is not offered and requests are exactly what they were before notes existed.

A turn that has notes shows a collapsed **Notes** disclosure under its answer; when some of the text came from an earlier turn the label says **Notes (carried from the previous turn)**. The same text is included in a [session export](#exporting-a-session). While such a turn runs, the progress line adds `· notes 2.1k` beside the round and read counts.

When older rounds are dropped, the request also names the calls that left the window and, once the window is full, warns which round leaves next. Both are for the assistant: they tell it what it has already looked at so it does not re-read it. A read it makes anyway still counts as a repeat, exactly as before.

A turn that reaches its action-round limit (`agent_max_tool_rounds`, 8 by default) while the assistant still wants a tool does not stop: it pauses, and the reply names the action rounds it used against that limit and how many read-only rounds it had used. Select **Continue — N more** to give the same turn another segment of action rounds, keeping every message, tool result and plan step it had, or **Answer now** to have the assistant answer with what it has, with no tools. The pause comes before that final call, so nothing is spent until you choose. A turn can be continued up to 64 action rounds in total; at that cap only **Answer now** is offered. The pause is saved, so after a restart the same offer is still waiting. With a token cap set, the turn continues on its own instead of pausing (see [Capping What One Turn May Spend](#capping-what-one-turn-may-spend)).

A turn that ends any other way leaves a mark on its message: **Stopped by you**, **Tool budget exhausted** (after **Answer now**; the reply names the action and read-only rounds it had used, and **Ask again** starts a new turn from the stopped task's own request, carrying its plan forward), **Token budget exhausted**, **Stopped before finishing**, or the error itself when the request failed. The stop and budget marks are still there when you reopen the session. **Stopped before finishing** and a failure's error are shown only in the window where the turn ran; after a reload, a stopped turn reads **Stopped by you** and a failed one has no mark.

### Intermediate steps and the answer

A turn records each action it takes — a file read, a search, a command, a note — as an assistant summary followed by its result. Those consecutive steps are folded into one collapsed **Activity (N steps)** row rather than left as a full bubble per action. Open it to see each action and its exact tool result; the content scrolls at a cap, and the answer stays its own message at the end of the turn. This is the same on reload: the activities are rebuilt from the session log, not stored as a second record.

The plan panel keeps its summary visible and puts its steps behind a collapsed **Steps (N)** row. Open it when you want the full plan, each step's outcome, and the evidence behind it; leave it closed when you only need the answer. See [The Plan Panel](#the-plan-panel).


### Agent tools for reading and navigating

With [native tool-calling](#native-tool-calling) enabled, Damaian offers the assistant tools that need
no approval and never change anything: `read_file` (with an optional line
range), `list_directory`, `search_content`, `search_codebase`,
`read_git_status`, `read_git_diff`, `list_worktrees`, and `read_git_log`. They
only read, so they can never change a file or run a command. A further tool,
`edit_file`, proposes a change to a snippet of a file, but nothing is written to
disk — the proposal appears as a patch preview and waits for your approval, on
the same terms as a whole-file `propose_patch`. A large file read is reported as
a range with the file's real line count ("lines 1–400 of 4420"), so a truncated
result says so rather than reading as the whole file.

`ask_user` is offered alongside them. It lets the assistant ask you one short
question in the middle of a turn and carry on with your answer; it changes
nothing by itself, and it is available only where a user can answer (see
[When the assistant asks you a question](#when-the-assistant-asks-you-a-question)).

The agent can also read the repository's git worktrees without asking.
`list_worktrees` lists every worktree git has registered for the repository —
each one's path, branch (or `detached`), short `HEAD`, count of changed files,
and how far it is ahead of and behind the main branch — and `read_git_log` reads
the commits a worktree's branch has that a base branch does not, as one-line
summaries. The five read tools (`read_file`, `list_directory`, `search_content`,
`read_git_status` and `read_git_diff`) each take an optional `worktree`
argument: pass a path `list_worktrees` returned, and the tool reads that
worktree instead of the main checkout. `read_git_diff` can also be limited to
given paths, return a `--stat` summary, or compare against a base branch.

Only worktrees git itself has registered are reachable. A directory that merely
lives under `.claude/worktrees/` or `.damaian/worktrees/` without being
registered, a symlink planted there, or a registered worktree outside the
repository root and your `allowed_roots`, is refused with the reachable
worktrees listed, so the assistant can correct itself. Reads inside a worktree
apply the same policy as reads in the main checkout: restricted and ignore
patterns, the read and search caps, and secret redaction. Git reads in a
worktree or the main checkout exclude restricted paths and are bounded the same
way.

A session whose own checkout is a linked worktree — a worker, or a development
worktree — can reach only that worktree by default: the main checkout and its
siblings count as outside it. `list_worktrees` then shows 0 even though
`git worktree list` shows the repository's worktrees, because it lists only
reachable ones. Add the main checkout to `allowed_roots`, or run the tool from
the main checkout, to survey them.

When one model message asks for several read-only tools, they run together and
their results are reported in the order the model asked for them.

### Reading an external page

With native tool-calling enabled and the session in **Code** mode, the
assistant also has `fetch_url`. It reads one absolute `http` or `https` URL that
the model names — a library's documentation, a changelog, an RFC — and returns
it as text. It is one request for one page, never a crawl:

- **A host is read only after you allow it.** The first fetch of a host pauses
  the turn and shows a card with the host as the headline and the URL exactly as
  the model wrote it, wrapped rather than shortened. A query or fragment longer
  than 128 bytes is broken out and marked, because a long opaque parameter is
  the shape exfiltration takes. The card offers *Allow once*, *Don't fetch*, and
  *Always allow this host in this project* in the overflow. *Allow always*
  writes `web_domain_allowlist.<repository id>` to your **user** config; a
  repository's own config can never add a host, and neither can a permission
  profile.
- **Nothing has been sent when the card appears.** The request carries no API
  key, no cookie and no other credential, and it does not reuse the model
  connection. A redirect is never followed: a 3xx comes back as a result naming
  the target, and fetching that URL is a new call with its own approval.
- **A URL carrying something private is refused before you are asked.** If the
  URL contains something that looks like a credential, or content from a file
  the assistant read earlier in the same task, the fetch is refused and the
  model is told why — no dialog appears, because an approval is not the last
  line of defence. The query and fragment are checked raw, percent-decoded and
  base64-decoded.
- **Only text comes back.** `text/*`, `application/json` and
  `application/xhtml+xml` are converted to plain text; scripts, styles, tags,
  attributes, images and other sub-resources are dropped and never loaded.
  Anything else returns its content type and size without its bytes. A response
  is capped at 200 KiB; a larger one is cut short and the result says so.
- **Nothing behind a login is reachable.** There are no cookies or sessions, so
  a page that requires sign-in returns its login page or an error, not the
  content.
- **One session, one cache.** Re-reading the same URL in the same session is
  served from a redacted cache and says so; a new session fetches it again, so a
  stale page is never presented as current.
- **At most eight pages per turn** by default (`web_fetch_max_per_turn`, hard
  ceiling 20). The cap is a refusal the model can read, not a silent stop.

The fetched text is **data, not an instruction**. It enters the model's context
on the **next turn** as the *Reference pages* row in the [context
inspector](#inspecting-and-controlling-context), labelled with its URL and fetch
time; the immediate tool result is the same text inside a fixed frame and quoted
line by line. It is the lowest-priority context category, so a crowded context
drops a fetched page before it drops repository code. A page that looks like it
is trying to issue instructions is marked in the result (instruction override,
impersonation, fake tool call, exfiltration request); the text is still shown,
quoted, and is not obeyed.

### When the assistant asks you a question

If the request is ambiguous and guessing would spend the turn on the wrong
reading, the assistant can pause and ask you one short question — "Did you mean
the CLI or the shell?" — instead of ending the turn. The question appears as a
card in the assistant's message with two to four suggested answers, plus a
free-text field when the question allows one. Answer by clicking an option or
typing a reply and selecting **Send**; the turn then continues with your answer,
carrying everything it had already read.

- **Answering a question authorises nothing.** It is not an approval. A command,
  a patch or a configuration change that follows still asks for its own approval
  on the usual terms, even when your answer was "yes, go ahead".
- **You can leave it unanswered.** Nothing times out and Damaian never picks an
  answer for you. The task stays waiting — including after a reload or a crash —
  until you answer it.
- **The question and its options are redacted** before they are stored or shown,
  on the same terms as other model output, so a question quoting a config file
  cannot carry a key into the transcript. What you type in reply is redacted
  before it is recorded and sent back to the model. Your own prompt to Damaian
  is still stored and sent as you typed it, as before.
- **A question appears only in an interactive desktop conversation.** The
  command-line tool, the evaluation harness, worker sessions and any unattended
  run have no one to answer, so `ask_user` is not offered there at all and the
  assistant proceeds under a stated assumption instead.
- **Asking is bounded.** Two questions per turn by default
  (`agent_max_questions_per_turn`, ceiling 4), and each one uses a tool round. A
  third question in the same turn is refused, and the assistant is told so and
  carries on. An unanswered question simply waits; an answered one is never
  asked again.

## File Changes

Use the conversation box to request file changes.

1. Enter a request such as `create a test file for the config parser`.
2. Review the inline patch preview returned by the assistant.
3. Keep checked only the files you want to act on. Each file's hunks have their own checkboxes, so you can also leave out part of a file.
4. Select **Apply N** to write the checked changes, or **Reject** to record the checked files as rejected without changing the workspace.

Damaian checks file hashes before applying a stored patch. If a target file changed after preview, that file is blocked instead of overwriting newer local work.

### Hardcoded secret warnings

Damaian scans generated content for credentials before writing it. If a selected file is flagged, nothing is written: the preview shows which file tripped the check and what was detected, and you choose.

Detection is not perfect. Setup instructions and placeholder values can resemble credentials, so review the diff before deciding. Select **Apply Anyway** to accept the change as written, or **Cancel** to leave the workspace untouched. The decision applies to that one apply and is not remembered; each apply asks again, and the override is recorded in the audit log.

From the CLI, `damaian apply-patch` prints the same findings and re-runs with `--allow-generated-secrets` to accept them.

After files are applied, Damaian prints a concise Git status summary in the conversation.

## Verification

After you apply a patch in **Code** mode, Damaian checks the change instead of
leaving that to you. It discovers the project's own checks, runs the ones you
approve, turns failures into findings, lets the agent repair within a limited
number of attempts, reruns, and reports what was actually verified.

**Which checks run.** Detection is per project root. A change selects the root
that owns each changed file plus every root that encloses it, because a member
change is a workspace change. On this repository, changing `crates/engine`
selects `cargo test` in `crates/engine` and at the repository root. A
documentation-only change (`.md`, `.txt`, `.rst`, `.adoc`) selects nothing and
says so. A Cargo root currently detects one check, `cargo test`; detection finds
no separate clippy or build command, so the report cannot claim one ran.

**Approval.** Checks that would need approval arrive on **one card**, not one
prompt each. Unchecked checks are declined and reported as skipped; a blocked
check never reaches the card and is reported with the policy's reason.
**Allow Always** stays per check, exact-command, and is qualified with the root
when the check runs in a sub-root. A session with
`require_approval_for_all_commands` gets a card even for a check that would
otherwise run without asking.

**Repair.** A failing check becomes findings, and the agent may propose a repair
patch within `agent_tool_retry_limit` (default 2). The patch still goes through
the normal preview and approval path, and the check reruns once it is applied.
A repair that breaks a previously passing check is reported as a regression. If
a repair turn produces no patch, the loop stops rather than spending the
remaining attempts; at the limit the task is marked failed.

**The report** lists:

- **Files changed** — the paths this task applied.
- **Checks passed** — only checks that ran and exited zero. A killed or
  signalled command is never here.
- **Checks failed** — with the exit code or the browser diagnostic's verdict.
- **Checks skipped** — declined by you, blocked by policy, or no running target.
- **Checks stale** — the files a check saw changed after it ran (a rollback, a
  later patch, or an edit outside Damaian). A stale check is not rerun silently
  and is not reported as passed.
- **Behaviour verified** — browser checks that ran and recorded no error.
- **Unverified steps**, **Regressions**, **Remaining**, and **User action
  needed**.

**"No checks were available" is not "checks passed".** A project with no
detected checks, or a documentation-only change, says so in its own words.

**Browser checks.** When a changed path is a web asset and the session already
has a diagnostic target, the loop reruns an inspection at that page under the
same session allowance. With no target, or a page that is not reachable, the
check is skipped with `no running target`; it is never inferred as verified.

Verification runs only in Code mode. Ask, Plan and Review have nothing applied
to verify. From the command line, `damaian verify <repo> <session>` runs it and
prints the report; `--decline-checks` answers an approval card with every check
declined. This checks that the project's own tests pass — it does not prove the
change is correct, and the report is written to avoid implying that.

## Commits

**Commit…** in the repo bar prepares a commit from the changes Damaian applied and you accepted in this session. It is the first action that writes to your Git repository, so it is the most explicit one:

- **Only accepted files start selected.** A file Damaian applied and you have not edited since is checked. A file you edited by hand after Damaian changed it, a file an approved command changed, and any other change appear with a label and start unchecked — the commit is the set you reviewed, and nothing is added behind your back.
- **A rename moves both paths together.** If you renamed a file with `git mv`, the review shows the new path and the old path's deletion; the checkboxes move as a pair, and committing only the new path is refused with `commit_rename_incomplete`, because that would leave the old file behind. A copy does not produce a deletion.
- **The mode is part of the change.** A chmod-only change, or a file replaced by a symlink, is shown as a candidate and committed with the mode that is on disk (`100755` or `120000`), not the old mode from `HEAD`.
- **Your staging area is not used.** Damaian builds the commit through a private index, so anything you had staged stays staged and your working tree is not touched. If the commit fails, there is nothing to restore.
- **The displayed diff is redacted; the commit is not.** The scanner redacts credentials in the preview. The dialog says so, and before the commit Damaian scans the real content again. If it finds something, you must tick the override for that commit; the override is recorded in the audit log. The bytes are also checked against the preview, so a file edited after you looked refuses the commit rather than committing something you did not see.
- **Repository hooks run for real.** The dialog lists `pre-commit`, `prepare-commit-msg`, `commit-msg` and `post-commit`, where they are and whether they will run. Damaian never passes `--no-verify`. A failing `pre-commit` aborts, a failing `post-commit` is reported after the commit, and a hook that rewrites the message asks you to approve the rewritten text before anything is created. A hook that reformats and stages files — lint-staged, for example — does not change what is committed: the commit keeps the bytes you reviewed, and a warning names the hook and says its staged changes remain uncommitted in your working tree.
- **One approval per commit.** There is no “always allow”. Committing is available in **Code** mode only, and only under a permission profile whose `allow_git_commits` is on; **Read-only** turns it off. A repository may turn it off and never on.
- **The result is reported.** The dialog shows the short commit SHA and the message, and the audit log records the SHA, the file and hunk counts, the message length, the hooks and whether the secret override was used.

The commit message starts as a template from your request. **Draft with model** asks the configured model for a subject and body using the redacted diff and the plan; if there is no model, the call fails or the answer is unusable, the template stays. You can always edit the message, and your text is what is committed.

If Damaian is interrupted while committing, the recovery card offers **Inspect**: it shows the prepared commit, the `HEAD` it was prepared on, and where `HEAD` is now. It does not resolve the commit for you.

## Rewind

Damaian takes a checkpoint before every turn, so a turn that went the wrong way can be undone in one action. Each of your messages in the conversation carries a **Rewind** control, which offers three choices:

- **Files and conversation** — puts back the files Damaian changed in that turn and moves the conversation to just before your message.
- **Files only** — restores the files and leaves the conversation as it is.
- **Conversation only** — moves the conversation back and leaves your files alone.

What a checkpoint covers is what Damaian itself changed in that turn: files from patches you applied, and files an approved command created, modified, or deleted. Files you edited yourself are not covered.

Some things are deliberately left out, and the dialog says so before you confirm:

- Paths excluded by policy — `.env` and anything else matching `restricted_patterns` — are never snapshotted and never restored.
- Files an approved command changed are not covered when the repository is not a Git checkout, or when its working tree is larger than `checkpoint_census_max_paths`.
- Effects that are not files — a pushed commit, a network call, a database write, container state — cannot be undone by a rewind.

If a file changed after the turn ran, the rewind stops and restores nothing. That is deliberate: Damaian cannot tell your later edit from the agent's change, so it reports the conflict and leaves everything alone rather than overwriting work it does not understand. Resolve the file by hand, or rewind the conversation only.

A rewind never rewrites history you can audit. The conversation moves back by appending a marker, so the full session log — including the messages that left the conversation — is still there.

**Checkpoints are session recovery, not version control.** They cover Damaian's own changes to one repository, they expire (`checkpoint_retention_days`, 90 days by default), and they are no substitute for a commit. Commit anything you would be unhappy to lose.

### The Checkpoints page

**Settings › Checkpoints** lists the open repository's checkpoints, one per turn, newest first. Each row shows the turn's summary, when it was taken, how many files it holds, the session it belongs to, and whether you have already rewound to it. A row also says when the checkpoint does not cover command effects, or how many paths policy excluded. **Rewind** on a row opens the same rewind dialog as the control on a message.

## The Plan Panel

When a request needs more than one piece of work, the assistant writes a plan
before starting and a panel appears under its answer. Its one-line summary is
always visible; the steps are behind a collapsed **Steps (N)** row, so a long
plan does not push the answer off the screen. Open that row to see every step
and what became of it.

Short requests get no plan. A question answered by reading one file is a single
step, and gets no panel.

Exactly one step runs at a time. If the panel ever shows two as running, it
says so in red: that is a fault in Damaian, not something you caused.

### What each step says

Every finished step carries one of four outcomes:

- **Confirmed** — the step finished and something Damaian watched confirms it: a
  command that exited cleanly, or a patch that was applied.
- **Completed, unverified** — the step finished, and nothing observable stands
  behind it. "Understand the retry logic" is a real step with nothing to
  measure. Damaian says so rather than implying a confirmation it does not
  have.
- **Blocked** — a check or other command the step ran did not succeed. The
  steps after it stay pending: continuing would build on work that did not
  happen. A look-around command that fails, such as a `grep` with no match or an
  `ls` of a missing path, is kept as evidence but does not block the step.
- **Skipped** — the step was not needed.

The one-line summary at the top of the panel **never says "complete" while any
step is blocked or still open**. A plan where four steps passed and one failed
is not a plan that worked.

Open **What confirms this** on any step to see the evidence itself — the command
and its exit code, or the files a patch wrote. This is what Damaian observed,
never what the model said about its own work. A step whose command exited 3 is
blocked no matter how confidently the reply describes it as done.

### When the turn ends with steps still open

The model does not get to mark a step done by saying so. If it answers while its
plan still has steps open, Damaian sends it **one** reminder to call
`complete_step` for the ones it finished. If it answers again without closing
them, the turn ends and they stay open.

That is not a stuck turn. An open step in an ended turn is shown as **not
finished**, and the summary reads **"Turn ended with N steps not marked done."**
Damaian knows what it observed; it does not know that the work was left undone,
which is why the wording is not "failed". The answer is still the answer, and
everything the step ran is still listed under **What confirms this**.

A plan you declined at the review gate is different: its steps are shown as
**skipped**, because your decision settled them. Damaian will not ask the model
to complete work you refused.

The panel survives a reload, so you can reopen an old session and still see
what a turn came to.

### Reviewing a plan before it runs

Before the first step that changes anything, Damaian stops and shows you the
plan. Nothing has happened yet at that point — no file written, no command run.
You can:

- **Reorder** steps with the arrows.
- **Retitle** a step by typing in its box.
- **Delete** a step with **×**.
- **Approve** the plan as it stands, or **Decline** it.

A plan whose steps only read things — listing files, searching, reading source
— is never put up for approval, because there is nothing to undo.

A step that has already run is shown but has no controls. Its result is tied to
the state of your repository, and deleting it would leave a plan whose history
no longer describes what actually happened. If you want a genuinely different
plan mid-task, stop the task and start another — [Rewind](#rewind) can take the
repository back first.

Approving is remembered for that task, so you are asked once, not before every
edit. A [rewind](#rewind) past the point where you approved takes the approval
with it: you are asked again, because you have moved the conversation back to
before you saw the plan.

## Search and Export

The search icon in the sidebar, beside the **+** next to **Projects**, opens a search box that finds text across the sessions of the current working folder; Escape, the button again, or a click elsewhere closes it. Type and matches appear live, each with the session title and a short snippet. Choosing one opens that session and scrolls to the matching message. Search is a plain text search — it is not code search and it is not semantic — and it only searches the current folder's sessions, because a session from another folder can contain that folder's file contents. Search only ever reads the session log; it never changes a session.

**Export as Markdown**, in a session's ⋮ menu ([Project and session menus](#project-and-session-menus)), saves the conversation as Markdown. Exports contain the title, the time range, the conversation in order, what each task came to, the plan steps and their evidence, and token and cost totals.

**Exports are redacted.** A session stores exactly what was said, so that a rewind can restore it faithfully; that same fidelity is why the stored log is not safe to copy out as-is. Every export runs the secret scanner over the conversation and states at the top how many secrets it removed. The number is not silently applied: if a transcript says it removed three secrets, three secrets were replaced with `[REDACTED_…]` and are not in the file. Searching finds a secret-shaped string without showing it — the snippet is redacted too.

An export is a one-way record. It cannot be imported back, and it never leaves your machine on its own: the file is saved wherever your browser's save dialog puts it, and no request is made to anywhere but Damaian itself.

## Symbol Search and Language Servers

The `{ }` button in the thread header, beside the terminal control, opens symbol search for the current folder. It searches declarations rather than text. Each result gives the symbol's name, its kind (function, method, struct, class, and so on) and an exact `path:line`. The `path:line` is clickable and opens the file in Visual Studio Code at that line and column, the same way a file reference in a reply does. A result in a project root other than the repository root shows that root after its path.

Results put a language-server answer before a guess, then order exact names before prefix matches and prefix matches before fuzzy ones, and show at most 50. The index is local and built from source text, so search needs no setup, no network and no language server.

**What `likely` means.** Every symbol and relationship carries a provenance. A result Damaian inferred by matching source text carries a `likely` marker; hover it to read "Derived from source text rather than a language server; may be inaccurate." A result a language server reported has no marker. A heuristic result's file and line are still real, and the link works: the marker says the *identification* is a guess, not that the location is missing. The assistant's context uses the same distinction, as `heuristic — may be inaccurate` (below).

**Find references.** Each result offers **Find references**, which asks the language server configured for that file's language ([Language servers](#language-servers)). Without one, or if it is unavailable, slow or crashed, the panel says so and shows nothing: references are reported only where they are reliable, never guessed from source text. A successful answer lists each reference as a clickable `path:line`. The answer is cached until a file it names changes.

**What the assistant sees.** When your prompt names an identifier the index knows, the turn gets a `symbols` context item after the repository map: one line per symbol with its kind, `path:line`, and the hedge for a heuristic entry. `symbol_context_max_tokens` caps the item at 1,600 estimated tokens by default; `0` turns it off, and a repository may lower it but not raise it. When the item is cut to fit, its last line says how many of the matching symbols were shown. If the index cannot be read, the item is left out and the turn continues.

**The first search starts the background pass.** It answers from the index as it was, so that first response is the heuristic tier and returns immediately. Once the pass has run, later searches show the language server's definitions without the `likely` marker, and diagnostics the server published while opening the files appear in [Findings](#findings) on the next refresh. The full effect, the settings that show a server's state, and the audit event that reports a rebuild are in [Troubleshooting](TROUBLESHOOTING.md#symbol-search-shows-only-likely-results-or-a-language-server-will-not-start).

## After a Crash

If Damaian stops while a turn is in flight — a crash, a force quit, a machine that lost power — the turn is not lost silently. When you reopen the session, a card at the top of the conversation says what was actually happening: "A patch application was in progress and its outcome is unknown", "Reading a file was interrupted before it finished". It never just says the session was interrupted, because what was in flight is what decides what can safely be done about it.

Each card offers up to four choices:

- **Resume** or **Continue** — sends your original request again as a new turn, carrying over the plan the interrupted turn had made so the steps it already finished are not redone. Offered only when Damaian can account for everything that was in flight.
- **Inspect** — expands to show your request, the action that never finished, the files it may have touched, and the checkpoint taken before the turn, so you can rewind if you want to.
- **Mark failed** — closes the turn out and records that its outcome was unknown.
- **Abandon** — closes the turn without retrying it.

If the turn had already spent anything, the card says so — including the model call that was in flight when Damaian stopped, which the provider billed even though its answer was lost. That figure is an estimate and is marked as one. A turn with nothing recorded shows no figure at all rather than a zero, which would read as a crash that was free.

Neither **Mark failed** nor **Abandon** changes or undoes anything on disk. They settle what the *turn* is, not what happened to your files. Putting files back is **Rewind**, which is what the checkpoint link in **Inspect** opens.

**Damaian will not retry an action on its own.** Where a turn was reading files or waiting on the model, there is nothing outside Damaian that could have half-happened, so the turn is marked ready to pick up again and the card tells you it was — but it still waits for you to press **Continue**, because resuming means paying for another model call and you should be the one to decide that.

Where something was running that could have changed the world — a command, a patch being written to disk, a call out to an MCP server — **Resume** is not offered at all, and the card says why. There is no way to find out whether a command finished, whether the patch wrote three files or ten, or whether the remote call went through. Damaian will not run it again and hope, and it will not do so even if you ask: the refusal is in the engine, not in a button that could be worked around. Look at what actually happened with **Inspect**, put files back with **Rewind** if you need to, and then mark the turn failed or abandon it.

If a turn was waiting for your approval when Damaian stopped, that approval comes back: the command, the patch, or the plan is re-presented exactly as it was, and nothing runs until you approve it. A command approved this way runs on its own — the conversation that proposed it ended with the crash, so its output is not fed back to the model. A plan is the exception: the turn that asked you to review it was saved, so approving the plan continues that turn from where it stopped. Occasionally an approval cannot be restored, because an older version of Damaian recorded that a turn was waiting without recording what it was waiting for. That turn is closed with the reason stated rather than shown as a rebuilt approval card: approving a command Damaian is guessing at would be worse than losing the turn. Your stored patches and commands are untouched either way.

## Working Modes

Every session has a working mode, which decides what the assistant is *able* to
do in that session. The four modes:

- **Ask** — reads your repository and answers questions. Nothing else.
- **Plan** — reads, runs read-only commands, and produces a plan. It cannot
  change a file.
- **Code** — the full set: file changes, commands and validation, each on the
  approval terms described in [Command Approval](#command-approval) and
  [File Changes](#file-changes). This is the default.
- **Review** — reads, runs read-only commands and browser diagnostics, and
  reports findings. It cannot change a file or make a plan.

| The assistant wants to… | Ask | Plan | Code | Review |
|---|---|---|---|---|
| Read files, list folders, search, read Git status and diffs | yes | yes | yes | yes |
| Propose a file change (`propose_patch`, `edit_file`) | no | no | yes | no |
| Make or advance a plan (`propose_plan`, `complete_step`) | no | yes | yes | no |
| Run a read-only command that needs no approval, such as `ls` or `git status` | no | yes | yes | yes |
| Run any other command | no | no | yes | no |
| Inspect a web page or run a browser scenario | no | no | yes | yes |
| Call an MCP tool its server marks read-only | yes | yes | yes | yes |
| Call any other MCP tool | no | no | yes | no |

A mode is a boundary, not a request to the assistant. A tool the mode does not
allow is never offered to the model. If the model tries to use it anyway, Damaian
refuses before anything happens: no patch is stored, no approval card appears,
no command runs. Instructions in a repository's `AGENTS.md` cannot widen a mode,
because an `AGENTS.md` arrives with whatever you clone. The model cannot change
the mode either. Only you can.

Ask offers no commands at all, not even read-only ones, so you can be sure
nothing happens. In Plan and Review, a command that would need your approval is
refused outright rather than shown as an approval card, because approving it
would let you click your way out of the mode you chose.

**An allowlisted command is still refused in Ask, Plan and Review.** An
allowlisted `npm run build` ([Allow Always](#allow-always)) runs without a
prompt in Code and is refused in the other three modes. Plan and Review decide
what counts as read-only from the command itself, never from your allowlist.

MCP tools are read-only only when their server says so. Most servers don't mark
their tools at all, and an unmarked tool counts as able to change things, so it
is only offered in Code.

**Switching modes.** The mode is the pill below the prompt, after the **+**
attach button and before the model, labelled with the current mode (**Code**,
for example). Click it, or tab to it and press Enter, and pick a mode; the
current one is marked **Current**. It takes effect from the next turn. If the
change cannot be saved, the pill goes back to the mode still in force and the
error is shown.
A turn that is already running finishes in the mode it started with. The control
is disabled until a session is open, and a new session starts in Code. No
confirmation is asked in either direction. A plan made in Plan mode stays as it
is when you switch to Code to carry it out.

A prompt that reads like a file-change request, such as `fix the bug in the
config file`, goes to the patch-preview flow described under
[File Changes](#file-changes) rather than to the assistant's tools. That flow
follows the open session's mode too: outside Code it is refused before the model
is asked anything.

Mode is also checked when you act on something the assistant asked for earlier.
Approving a command, MCP call or browser diagnostic that was waiting for you,
or applying a stored patch, is refused if the session's *current* mode does not
allow it, even though the mode allowed it when it was proposed.

When something is refused, the conversation says which mode blocked it and which
would allow it, for example: "Refused: Ask mode does not allow this. Switch to
Code mode to allow it."

Known limit: the CLI's `propose-edit` runs outside any session, so no mode
applies to it. The repository's permission profile still does: under Read-only
it is refused before the model is asked anything.

## Permission Profiles

A permission profile decides what Damaian may do in one repository, whatever
the session is doing. A mode is what this session is doing; a profile is what
you allow in this checkout. You choose one per repository:

| Profile | File edits | Commands | Browser diagnostics | MCP tools | Also |
|---|---|---|---|---|---|
| **Full repository development** | as configured | as configured | yes | yes | Narrows nothing. The default |
| **Safe local development** | always need your approval | `local` only | yes | yes | |
| **Offline private** | as configured | `local` only | no | none (MCP off) | Audit and checkpoint retention lowered to 7 days |
| **Read-only** | no | none | no | read-only tools only | |
| A custom profile | | | | | One you imported (see below) |

"As configured" means the profile leaves that key to your own settings.

**Choosing one.** In **Settings › General**, open a repository, then pick from
**Permission profile for this repository**. The choice is stored in your user
config as `permission_profile.<repository_id>=<profile>`, keyed by the working
folder the way *Allow Always* is, so it does not carry over to another clone. A
repository cannot choose its own profile in either direction. From the CLI:

```bash
damaian profile-set /path/to/repo safe_local
```

The ids are `read_only`, `safe_local`, `full` and `offline_private`, or the name
of a custom profile.

**A profile only narrows.** It is applied after your user config, the
repository's config and admin config, and it can only make the result stricter.
A profile that sets something looser than what is already in force changes
nothing; a profile that tries to set a key it may not carry, such as `shell` or
a `model_*` key, has that key ignored and reported. So a profile can never undo
a restriction you or the repository set, and admin config cannot undo a
narrower profile you picked.

**When it takes effect.** At the next turn, at the next resume after you approve
something, and at the next command run by id. Anything already running finishes
under the profile it started with, exactly as with a mode switch.

### Profile and mode together

Both must allow an action:

```text
what the session may do = profile ∩ mode
```

Neither can widen the other. Ask mode under Full repository development cannot
edit a file, because Ask does not allow it; Code mode under Read-only cannot
either, because Read-only does not.

The refusal says which one refused, because the fix is different:

- A mode refusal names the mode and the mode that would allow it (see
  [Working Modes](#working-modes)).
- A profile refusal: "Refused: the permission profile does not allow this
  (allow_file_edits=false). Switching mode will not allow it."

The profile refusal names the setting rather than the profile, because the same
setting can also come from your own config or the repository's. The effective
policy view says which.

When both refuse, the mode is named first. In Ask under Read-only, an edit is
refused with "Switch to Code mode"; in Code it is then refused by the profile.

Read-only still lets Code mode make and advance a plan (`propose_plan`,
`complete_step`): planning writes nothing, so no profile setting covers it.

### What `local` and Offline private cannot promise

**`local` means no command Damaian recognises as networked.** That is a name
heuristic, not a security boundary. Damaian recognises `curl`, `wget`, `npm`,
`pnpm`, `yarn`, `pip`, `docker`, and `git pull`/`push`/`fetch`/`clone`. A
command it does not recognise, such as `python -c` with `urllib`, can still
reach the network. Because the check reads the command name, `local` also blocks every
`npm`, `pnpm` and `yarn` command, `npm test` and `npm run …` included, while
`cargo test` is allowed.

A custom profile may also use `command_access=read_only`, which allows only the
low-risk read-only commands Plan mode allows. The built-in Read-only profile
uses `none`, which allows no commands at all.

**Offline private does not stop model traffic.** Your prompts and the code in
context are network traffic to the model provider, and no profile can change
where the model provider is. When Offline private is selected and the
provider's base URL is not a loopback address, Settings shows a warning saying
so. It does not refuse to run. For no model traffic leaving the machine, use a
local provider (see [Model Providers and API Keys](#model-providers-and-api-keys)).

### Reading the effective policy view

**Settings › General › Effective policy** shows the policy in force for the
open repository. Open a repository first; without one the picker is disabled
and the table shows the global policy.

The header names the profile, and the mode when a session is open, for example
`Safe local development ∩ Code mode`.

Each row is one rule. The rule is named in plain words, such as `Ask before
changing files`, with its config key (`require_approval_for_file_edits`)
underneath in small type. Refusal messages and this guide use the key. Each row
also shows the rule's value and its **source**:

| Source | Meaning |
|---|---|
| `default` | Built-in default; nothing set it |
| `user config` | Your `user.conf` |
| `repository config` | The repository's `.damaian/config.conf` |
| `admin config` | `admin.conf` or `DAMAIAN_ADMIN_CONFIG` |
| `profile: <id>` | The selected profile |
| `this repository (Allow Always)` | A command you allowed always in this repository |

A list such as `restricted_patterns` shows a source per entry, grouped where
consecutive entries share one. Where several scopes agree on a value, the source
is the last scope whose value holds.

A rule an admin value loosened carries the tag `widened by admin config`. Admin
config is the one scope allowed to loosen what came before it. Some keys have no
looser or stricter direction, such as `audit_enabled`, `allowed_roots` or
`secret_patterns`; an admin value for one of those shows `admin config` as its
source but is never tagged. Read `admin config` as "your administrator decided
this", which may be looser than your own setting.

Below a rule, a `Refused:` line is a request that was not applied:

```text
Refused: repository config tried to change “Ask before changing files”
(require_approval_for_file_edits). It may only make this stricter.
```

It names who asked (`repository config` or `the permission profile`), the rule,
and why:

- "It may never set this": that scope can never set the key.
- "It may only make this stricter": this request would have loosened it.
- "Only your own config may set this": for example `command_allowlist`.
- "The line could not be read".

The refused value is never shown, because it is text the repository or the
profile file controls. A refusal of a key with no row of its own is listed under
`Other refused requests`.

Under `command_access=local` or `read_only`, a note row repeats what that level
can and cannot promise.

The CLI prints the same view:

```bash
damaian config-show --sources /path/to/repo
```

### Exporting and importing profiles

**Settings › General › Permission profile files**.

**Export** writes the selected profile as a short config file, starting with a
`# Damaian permission profile: <id>` comment. It contains only the keys a
profile may carry, so it never contains `model_api_key_env`, an MCP server's
`auth_token_env`, or any other credential reference, and it is safe to paste
into an issue.

**Import** treats the file as untrusted. Paste it, name it, and press **Review
import**. Nothing is written yet. The review lists:

- **Will write** — the keys the profile will carry.
- **Not written, a profile cannot set these** — keys such as `shell`, `model_*`
  or `allowed_roots`. They are dropped.
- **Written but no effect here** — keys that would loosen this repository's
  config. They are kept in the file, but a profile only narrows, so they do
  nothing while your config is stricter. If you later loosen your own config,
  they keep the profile's narrowing.

There is nothing to approve in the third list: no profile can widen anything,
so an imported one cannot either. Then press **Import** or **Cancel**. A name that
is a built-in, or that an existing custom profile already uses, is refused
unless you tick **Replace a custom profile with this name**.

**An import never selects the profile.** It adds it to the picker; choosing it
is a separate step, so importing a file cannot change what any repository runs
under.

From the CLI:

```bash
damaian profile-export safe_local my-profile.conf
```

```bash
damaian profile-import my_profile my-profile.conf --repo /path/to/repo --review
```

`--review` prints the three lists and writes nothing. Without it the lists are
printed and the profile is written. `--repo` judges "would loosen" against that
repository's config. `--replace` overwrites an existing custom profile of the
same name. Then select it with `damaian profile-set <repo> <name>`.

Imported profiles live in `config/profiles/<name>.conf` inside the data
directory. Custom names are lowercase letters, digits and `_`, up to 40
characters.

## Command Approval

When the assistant needs a local command result, it asks Damaian to run the command. Read-only commands Damaian recognises, such as `pwd`, `ls`, `git status`, and `git log`, run without asking, in the selected working folder. They run directly on your machine, which is why only commands Damaian classifies as read-only qualify. Simple pipelines made only of such commands, filtered through `head`, `tail`, `wc`, `sort`, `uniq`, `cut`, or `grep`, also run without asking. Damaian redacts the output and sends it back to the model so it can finish answering.

### What runs without asking

A command runs without asking only when Damaian can prove it is read-only. A
shell tokenizer splits it into words with no expansion — no `$`, no backtick, no
unquoted `*`, `?`, `[`, `{` or `~` — so what was checked is exactly what the
shell will run. Each program then takes only a closed list of options, and every
path argument is checked against the repository. Anything outside that shape
asks for approval.

| Program | Admitted form |
|---|---|
| `pwd` | alone |
| `ls` | `-a`, `-l`, `-1`, `-d`, `-h`, `-R`, and paths, alone or combined; `-R` only with a path that is not the repository root |
| `git status` | `--short`/`-s`, `--porcelain`, `--porcelain=v1`, `--branch`/`-b`, `--untracked-files=no` |
| `git log` | `--oneline`, `-n N` (or `-nN`, `-N`), `--max-count=N`, `--stat`, `--name-only`, `--name-status`, `--format=…`, `--pretty` or `--pretty=…`, `--decorate`, `--graph`, revisions and paths — but not `-p`, `--patch` or `-u` |
| `git diff`, `git show` | not admitted — they print file contents a command line cannot bound or keep restricted paths out of; the assistant reads them through the `read_git_diff` tool instead |
| `git` (global) | `--no-pager` only |

Every path in an admitted command is resolved against the repository and
`restricted_patterns`: a path outside the repository, behind a symlink, or
inside a restricted directory such as `secrets/` asks for approval, and so does
a command whose working directory is restricted. `ls -R` also needs a path that
is not the repository root, so a recursive listing cannot walk into restricted
directories unasked. A restricted entry inside a directory you named is still
listed by name — `ls` is not policed per entry the way the `list_directory` tool
is.

### Read-only pipelines

A pipeline runs without asking when it is built only from those commands and a
small set of filters, joined by `&&` and `|`. It may begin with `cd <dir> &&`,
where `<dir>` is inside the repository, not restricted, and exists. Each command
before a `|` must itself be an admitted command from the table, checked against
the directory it will run in. Each command after a `|` must be a filter that
reads only its input, with no file argument:

- `head -n N`, `tail -n N`
- `wc` with `-l`, `-w` or `-c`
- `sort` with `-r`, `-n` or `-u`
- `uniq` with `-c`
- `cut` with `-d` and `-f`
- `grep` with `-i`, `-v`, `-n`, `-c`, `-E` or `-F` and one pattern

So `git log --oneline | grep -i fix | wc -l` and
`cd crates && git status --short | head -n 3` run without asking. `git status;
rm x`, `git log > out.txt` and `ls | xargs cat` do not: `;`, `>` and a filter
with a file argument are outside the admitted shape.

Every other command appears as an approval card in the conversation. Its first line names why Damaian is asking — which shell-control characters, network keyword, restricted path, setting, or [hook](#hooks) — and the same reason appears on the card beside the risk badge. **Why this command** holds the full prompt with every reason, and, when the agent gave one, **The agent says: …** is the agent's own stated purpose for the command. Review the command, risk, working directory and both reasons, then select **Approve** or **Reject**. Destructive commands blocked by policy cannot be approved from the UI.

Docker commands, including `docker ps`, `docker compose ...`, and `docker-compose ...`, always ask for approval. They can inspect or change a privileged local daemon, use the network, start background services, mount host paths, expose ports, and mutate images or volumes, so Damaian never treats them as read-only.

A command Damaian runs is stoppable and has a deadline: its output is streamed
to the turn while it runs, **Stop** ends it, and `command_timeout_secs` (600 by
default) kills it if it runs too long. A repository may shorten that deadline,
never lengthen it.

Which commands may run at all also depends on the session's [working mode](#working-modes) and the repository's [permission profile](#permission-profiles). A command can run in a directory inside the repository rather than the working folder; see [Where a command runs](#where-a-command-runs).

### Allow Always

**Always allow in this project**, in the approval card's ⋯ menu, runs the command and records it in your own config as `command_allowlist.<repository_id>`, so that command stops asking. The rest of this guide, and the policy view, call this grant *Allow Always*. It means "stop asking me about this exact command", not "this command is read-only".

The allowance covers the exact command only: allowing `npm run test:unit` does not allow `npm run test:unit --watch`, and allowing `git push` does not allow `git push --force`. It applies to that working folder alone, and not to another clone of the same project at a different path. In a root other than the working folder it applies to that root only ([`Allow Always` in a root](#allow-always-in-a-root)). Remove an entry by editing the `command_allowlist.<repository_id>` line in `~/Library/Application Support/DamaianClient/config/user.conf`.

The decision is stored in your config rather than in the repository because Damaian could not otherwise tell a command you allowed from one that arrived with the clone. A `command_allowlist` in a repository's `.damaian/config.conf` is never honoured; if a repository you open already has one, Damaian shows the exact commands once and asks which to keep.

*Allow Always* is not offered for commands blocked by policy, for commands containing shell control syntax such as `|`, `&&`, or `>`, when `require_approval_for_all_commands` is set, or on a [worker's](#worker-sessions) command card. In the first three cases an allowlist entry would have no effect, so the app does not offer to write one. An allowlisted command is still refused in Ask, Plan and Review modes ([Working Modes](#working-modes)).

## Repository Roots

A repository can hold several projects: a Cargo workspace and its member crates, or `packages/api` (Node) beside `packages/web`. Damaian calls each one a *root*. Each root has its own languages, manifests, entry points, test directories, `AGENTS.md` files and commands. A root's commands run in that root's directory, so `npm test` found in `packages/api` runs in `packages/api`, where its `package.json` is.

### How roots are found

A directory is a root when it contains one of the manifests Damaian already knows how to run: `package.json`, `pyproject.toml`, `pytest.ini`, `pom.xml`, `build.gradle`, `go.mod` or `Cargo.toml`. The working folder itself is always a root, even with no manifest, so every repository has at least one. Nested roots are all kept: a Cargo workspace root is right for `cargo test --workspace`, and each member is right for its own `cargo test`.

Some directories are never roots:

- anything `.gitignore`d or matched by `ignore_patterns`, because Damaian never indexes it;
- `node_modules`, `vendor`, `target`, `dist`, `build`, `.venv` and `venv`, wherever they appear, even if you have removed them from `ignore_patterns`;
- anything more than six directories deep. Damaian records a manifest found there rather than ignoring it silently (see [Troubleshooting](TROUBLESHOOTING.md#repository-roots-and-the-repository-map)).

When a directory holds more than one manifest, the root names the first in the list above as its evidence. A repository root with both `package.json` and `Cargo.toml` therefore reads "because `package.json` exists".

A root's commands come from its manifest: `npm run <name>` for each of the scripts `test`, `lint`, `typecheck`, `build` and `format` in `package.json` (plus `npm test` when there is a `test` script), and one fixed command for each other manifest, such as `cargo test` for `Cargo.toml`. Only those exact script names count: a script called `lint:web` or `check` is not offered.

### Seeing and correcting roots

**Settings › General** has a **Repository roots** section for the selected working folder. Each row shows the root's path (`.` is the working folder itself), its languages, how many commands it has, and whether it was "detected", "added by you" or "removed by you". Select a row to see why it is a root ("Treated as a root because `packages/api/package.json` exists."), its instruction files, and each command with the directory it runs in.

- **Add a root** makes a directory a root although it has no manifest. It must be a directory inside the repository that holds at least one indexed file, and not under a vendor directory or a `restricted_patterns` match.
- **Remove root** stops treating a detected directory as a project. Its files then belong to the nearest enclosing root, and it proposes no commands. It stays in the list, marked "removed by you", so the decision is visible.
- **Undo** clears your override for that directory.

The working folder itself cannot be removed.

Add, remove and undo write `project_roots_added` and `project_roots_removed` to `.damaian/config.conf` **inside the repository**. That file is in your working tree, so `git status` shows it, and you can commit it to share the correction or leave it uncommitted. Damaian asks before it writes. If that file already has a line Damaian cannot parse, the write is refused with the error, so a hand-edited file is never rewritten. You can also edit the two keys by hand, as `|`-separated lists:

```text
project_roots_added=tools/scripts
project_roots_removed=examples/legacy|examples/old
```

An entry Damaian cannot apply (a path that does not exist, a file, `..`, the working folder in `project_roots_removed`, a path in both lists) is not applied, and the section lists it under "Overrides Damaian did not apply". Overrides survive every rescan.

The same keys are accepted in your user config (`config/user.conf`). There they apply to every repository you open. Nothing in Damaian writes them there.

From the CLI, `damaian repo-root <repo> add|remove|clear <path>` makes the same edit, and `damaian repo-map <repo>` prints the roots.

### Where a command runs

A command the assistant asks for runs in the working folder unless it asks for a directory inside the repository, normally one of the roots on the map. When it does, the approval card reads **Runs in `packages/api`**. No line means the command runs in the working folder itself. A root changes only the directory. It never changes whether a command needs your approval, and never lets a command reach a path outside the repository: `cat ../web/src/index.ts` from `packages/api` is still inside the repository, and a directory outside it, behind a symlink, or under `restricted_patterns` is refused before anything is proposed.

Error locations a command prints (`src/x.ts:3:5`) are matched to files relative to the directory it ran in first, then its parent directories up to the working folder. So findings from a command in `packages/api` point at `packages/api/src/x.ts`, and `cargo` output from a member crate, which prints workspace-relative paths, still resolves.

Elsewhere the UI names the root when a file is not in the working folder's own root: a pinned file chip reads `index.ts · packages/api`, and each file in a patch preview is tagged with its root. A turn that touches two packages shows both.

### `Allow Always` in a root

[*Allow Always*](#allow-always) on a command that runs in a root other than the working folder records the grant for that root only. For `npm test` in `packages/api`, your config gets:

```text
command_allowlist.<repository_id>=cd packages/api && npm test
```

That entry lets `npm test` run unprompted in `packages/api` and nowhere else. `npm test` in `packages/web` may do something quite different, so it still asks. The entry cannot be matched by typing it as a command, because a command containing `&&` is never allowlisted.

The reverse also holds. A plain `command_allowlist` entry, whether from *Allow Always* at the working folder or one you wrote yourself at user or admin scope, applies **only at the repository root**. Your machine-wide `command_allowlist=npm test` does not let `npm test` run unprompted in `packages/api`.

### The map the assistant sees

Each request includes a compact *repository map*: every root with its languages and commands and, when there is room, its manifests, entry points, test directories, major directories and instruction files. The map tells the assistant where things are without searching. It sits after `AGENTS.md` instructions and before retrieved files, so under a tight context budget it can displace retrieved files but never instructions. It is secret-scanned like every other context item.

The map is limited by `repository_map_max_tokens`, 800 by default. When a repository's map is larger, Damaian drops detail in a fixed order: entry points and test directories (largest root first), then major directories, then generated and vendor paths, which become a count, and finally whole roots. Whenever anything is left out, the map ends with a line saying what, such as "3 of 7 roots shown". Set the key to `0` to leave the map out of requests entirely. The roots, the Settings section and the CLI work either way.

A repository's `.damaian/config.conf` can lower `repository_map_max_tokens` or set it to `0`, but cannot raise it above yours, because the map is repository content sent to the model on every request.

## Worker Sessions

A **worker** is a session that runs on its own: it gets a prompt, works in a new Git worktree of its own, and is hosted by a separate Damaian process, so you can keep talking to the session that started it. Workers are for handing off self-contained tasks, several at once if they can run in parallel.

**Turning them on.** Workers are off by default. Turn on **Worker sessions** at the top of **Settings › General**. A repository cannot turn them on for you. While they are on, every session that is not itself a worker is told how to write a handoff: a prompt in a fenced block whose opening line is ```` ```worker-prompt ````. A request that asks to plan, split or hand off work (it says *split*, *plan*, *tasks*, *hand off*, *parallel*, *worker* or *session*) then always goes to the conversation, even when it also asks for an edit, rather than to a one-shot patch preview, which could only answer with a diff. Off, nothing of this section is visible and nothing is added to what the model is sent.

**Starting a worker.** A `worker-prompt` block in an assistant's answer gets a **Start worker** button once the answer has finished. No other block gets one. Clicking it opens the New worker dialog with the block's text, which you can still edit. Confirm with **Start worker**. The session holding the block becomes the worker's parent: the block then shows "Started *‹title›* →", and the conversation gets a one-line "Spawned worker" link. **New worker** on a project's ⋮ menu or on the Workers board opens the same dialog empty, for a worker with no parent. A worker cannot start workers of its own.

**Uncommitted changes.** A worker starts from your last commit, so it cannot see work you have not committed — often the very task file you planned it from. If your tree has uncommitted changes, the dialog says how many and names the files the prompt mentions first. Choose **Include uncommitted changes** (selected by default), which copies your tracked edits and untracked files into the worker's tree, or **Start from last commit**. Your own tree is not changed either way.

**What a worker creates.** A branch `damaian/<name>` and a worktree at `<repository>/.damaian/worktrees/<name>`, the name taken from the prompt's first line. Damaian adds `.damaian/worktrees/` to `.git/info/exclude` once, so worktrees never show as untracked files. `.gitignore` is never edited. Clicking **Start worker** is your approval of those Git commands. They are recorded in the audit log and in the worker's session.

**The Workers board.** **Workers** at the top of each project in the sidebar opens the board, and its badge counts the workers that need you. Workers are also listed in the sidebar under their parent, with a status dot. The board groups workers under *Needs you* (waiting for approval, or paused), *Running* and *Finished*, newest first. Each row shows the planner it came from, its rounds, what it is doing and how long it has run. Finished rows show files changed and commits ahead of where the worker started. Each row has one action — **Review** for an approval, **Pause** while running, **Open** otherwise — and a ⋯ menu with **Stop**, **Reveal worktree in Finder**, **Remove worktree** and **Dismiss from board**. Dismissing only hides the row. The session stays in the sidebar.

**In a worker's session** a control strip under the header shows its state and round, with **Pause** and **Stop** while it runs, **Resume** and **Stop** while it is paused, and **Stop** while it waits or sits idle. The header says which session it came from and which branch it is on.

- **Notes.** While a worker runs, the composer's button reads **Send note**. A note joins the conversation as your message at the worker's next round, before its next model request. While paused it reads **Resume with note**, and when the worker is idle, **Send message** starts a new turn. A note sent during the turn's final round, after which the turn finishes without another model request, does not reach that turn.
- **Pause** takes effect at the next round boundary. A running command or model call is never cut short.
- **Stop** ends the turn at its next safe point. If it has not ended within 10 seconds Damaian asks the worker to shut down, and 5 seconds after that it terminates the process. Stopping a worker that waits for approval or sits idle shuts it down.
- **Approvals** look exactly as they do in your own sessions and continue in the same worker process without restarting it. A worker's command card has no *Allow Always*, and its plan steps cannot be edited before approval. Patch proposals and provider-switch offers end the turn as usual and are answered from the worker's session.
- **Restart worker.** A worker whose process ended shows *Interrupted* if it ended mid-turn, or *Ended*, and offers **Restart worker**. That starts a new, idle process for the same session and worktree, and the composer comes back. An approval the old process was waiting for ended with it, and its command did not run.

**Nothing outlives Damaian.** When Damaian quits, crashes or is force-quit, every worker exits within seconds and takes every command it started with it. Deleting a session stops its workers, and a worker's own session too. Their worktrees, branches and session logs are kept.

**Removing a worktree.** **Remove worktree** on a finished worker's ⋯ menu removes `.damaian/worktrees/<name>`, after a confirmation. If the worktree has uncommitted changes, a second confirmation names them. **The branch is never deleted**: merging or deleting it is yours, through ordinary Git or by asking a session to.

**Turning workers off** hides every entry point, the board and the controls. Workers that are already running keep running until they finish their turn or Damaian quits. To pause, steer or stop one, turn the setting on again.

## MCP Servers

**Settings › MCP Servers** connects Model Context Protocol servers. Their tools are offered to the assistant alongside the built-in ones. MCP tools need [native tool-calling](#native-tool-calling) on the provider.

Select **New** beside **Configured servers**, then fill in **Server details**: a **Name** and an **ID**, and a **Transport**:

- **Local (stdio)** starts a program on your machine. **Command** is the executable, or a full command line if you leave **Arguments** empty; quote paths that contain spaces. **Arguments** takes one argument per line, and **Environment** one `NAME=value` per line.
- **Remote (HTTP)** connects to a **URL**. For a server that needs a token, set **Auth Token Reference** to a `keychain:` reference and paste the token into **Auth Token**: it is stored in macOS Keychain when you save, and only the reference is written to your config.

Two checkboxes follow. **Enabled — offer this server's tools to the assistant** turns the server on, and **Require approval before each tool call** makes every call wait for your approval. Both start ticked for a new server. **Test Connection** connects with the form as it stands and reports how many tools the server offers, or the error. **Save Server** writes it to your user config, as `mcp_server.<id>.*` keys, and **Remove** deletes it.

Which MCP tools a session may call also depends on its [working mode](#working-modes) and the repository's [permission profile](#permission-profiles): a tool its server does not mark read-only is offered only in Code mode. A repository can turn MCP or a server off, never on.

## Hooks

A **hook** is a program that runs at one of nine points in a session and
answers with a verdict. It is how you enforce a local rule Damaian does not
know about: "never touch `migrations/`", "run `cargo fmt --check` before a task
is called complete", "ask me before a patch adds a dependency". `AGENTS.md` can
*ask* the model to follow a rule; a hook can refuse the action when it does
not.

**A hook can only narrow.** It may allow, warn, ask you to approve, or deny an
action. There is deliberately no verdict that grants anything, so a hook cannot
undo a working mode or a permission profile. A broken **mandatory** hook
blocks the action; it is never skipped. Hooks run in every mode and under every
profile, because a rule like "never touch `migrations/`" must keep working in
Ask mode too.

A hook is an external program, not a model instruction and not an in-process
plugin. It receives paths, ids and metadata, never file content, patch content
or your prompt. The model never sees a hook's stderr.

### The nine events

| Event | When it runs | What a hook sees |
|---|---|---|
| `session_start` | Before a new session is created. A denial refuses the turn and creates nothing | `{}` |
| `before_context` | Before context is assembled, in a chat turn and in a one-shot edit | `{ flow, explicitPaths }` |
| `before_tool` | Before one tool call is dispatched. A denial refuses that call and the turn continues | `{ tool, paths }`, plus the command, MCP server/tool or diagnostic URL where they apply |
| `after_tool` | After one tool call or approval resume finishes. It observes; nothing is undone | The same fields, plus `outcome`, and for a command `exitCode` and `termination` |
| `before_patch_apply` | When you apply a previewed patch, before anything is written | `{ patchId, files }` |
| `after_patch_apply` | After a patch is applied and recorded. It observes; nothing is rolled back | `{ patchId, files, appliedFiles }` |
| `after_check` | After a validation command's findings are recorded (not for the standalone Run Command endpoint, which records none) | `{ command, exitCode, termination, findingIds }` |
| `before_task_complete` | When the model has answered and the task is about to complete, not at a pause | `{ rounds, filesChanged }` |
| `session_end` | When you delete a session. It only observes; a denial is a warning and does not stop the delete | `{}` |

`before_task_complete` is not dispatched when you answer **Answer now** at the
round budget, because that is your own decision to end the turn and needs no
extra round. `before_tool` is the one event a question (`ask_user`) sees, and
`request_approval` there refuses the question: an answer is not an approval.

Some events have nothing to approve or nothing left to undo. What each verdict
means at each event is in [What a verdict means](#what-a-verdict-means) below.

### Configuring a hook

Hooks use the same flat key format as the rest of Damaian's config. Put them in
your user config (`<data_dir>/config/user.conf`) or in a repository's
`.damaian/config.conf`:

```text
hook.no_migrations.command=/bin/sh
hook.no_migrations.args=/Users/me/bin/no-migrations.sh
hook.no_migrations.events=before_patch_apply|before_tool
hook.no_migrations.mode=mandatory
hook.no_migrations.order=10
hook.no_migrations.timeout_ms=2000
hook.no_migrations.enabled=true
```

| Key | Meaning |
|---|---|
| `hook.<id>.command` | The executable. Required. A user hook is an absolute path or a name found on `PATH`; a repository hook's command must resolve inside the repository or be an absolute system path |
| `hook.<id>.args` | Arguments, separated by `\|`. An argument may contain a space |
| `hook.<id>.events` | One or more of the nine events, separated by `\|`. Required |
| `hook.<id>.mode` | `mandatory` (default) or `optional` |
| `hook.<id>.order` | An integer, default `0`. Hooks run by `order`, then by id |
| `hook.<id>.timeout_ms` | A deadline in milliseconds, `1`–`60000`. Defaults: 2,000 for `session_start`, `before_context` and `before_tool`; 5,000 for `session_end`; 10,000 for the other five |
| `hook.<id>.enabled` | `true` (default) or `false`. A repository may switch off a hook it defined; it cannot switch off yours |

The id is `[a-z0-9_]{1,40}`. Every configured hook appears in **Settings ›
Hooks**, which shows its source, events, command line, whether it is active,
and whether a failure blocks or warns. A user hook has an **Enabled** toggle
there; a repository hook is switched by review instead.

### Writing a hook

The hook is run directly, not through your login shell. It gets the event as
one JSON object on standard input:

```json
{
  "event": "before_patch_apply",
  "hookApiVersion": 1,
  "sessionId": "session_...",
  "taskId": "task_...",
  "mode": "code",
  "profile": "full",
  "repositoryRoot": "/Users/me/work/app",
  "hookDepth": 0,
  "payload": {
    "patchId": "patch_...",
    "files": [{ "path": "migrations/003_add_index.sql", "status": "modified" }]
  }
}
```

It answers on standard output with exit status 0:

```json
{
  "hookApiVersion": 1,
  "verdict": "deny",
  "reason": "migrations/ is applied by the release process, not by hand",
  "findings": [],
  "context": []
}
```

- Empty output (whitespace only) is `allow`. Any other output must be one JSON
  object with `hookApiVersion: 1`.
- `verdict` is `allow`, `warn`, `request_approval` or `deny`; absent means
  `allow`. `reason` is required for `deny` and `request_approval`.
- `findings` are ordinary Damaian findings with source **Hook**. `context` is
  accepted only at `session_start` and `before_context`, where it enters the
  turn as a bounded `hook_context` item. `commands` asks Damaian to run a local
  command (see below).
- The response is a closed schema. An unknown field, an unrecognised verdict,
  an unknown `hookApiVersion`, malformed JSON, output over 64 KiB, a non-zero
  exit or a timeout is a failure, not a quiet `allow`. A hook that tries to
  widen is recorded as a widening attempt and then treated as malformed.
- Output is bounded and redacted before Damaian shows or stores it: 64 KiB of
  stdout, a 1,000-character `reason`, 20 findings, 4 context entries of 4,000
  characters each, and 3 commands. stderr is kept to its last 8 KiB, redacted, and shown only to you.

The process gets a **cleared environment**: `PATH`, `HOME`, `USER`, `LANG` and
`TMPDIR` if set, plus `DAMAIAN_HOOK_ID`, `DAMAIAN_HOOK_EVENT` and
`DAMAIAN_REPOSITORY_ROOT`. It does not get your model API key or any MCP
token. It runs in the repository root, whatever the session's working mode or
profile is.

### What a verdict means

Each event maps the four verdicts onto what it can actually do:

| Event | `deny` | `request_approval` | `warn` |
|---|---|---|---|
| `session_start` | The turn is refused before a session exists, and the reason is shown | Treated as `deny` | Proceed, warning finding |
| `before_context` | The turn fails with the reason as the assistant message | Treated as `deny` | Proceed, warning finding |
| `before_tool`, command | No proposal is stored; the tool result names the hook and reason | The command is forced to an approval card, with the hook's reason on it; a blocked command stays blocked | Proceed, warning finding |
| `before_tool`, MCP call or diagnostic | The call is refused and the turn continues | The call takes an approval card even if it would have run unasked | Proceed, warning finding |
| `before_tool`, patch or edit | The call is refused; no patch is stored | Satisfied by the existing review step; recorded as a warning | Proceed, warning finding |
| `before_tool`, any other tool | The call is refused | Treated as `deny`: there is no approval step to move it to. A question (`ask_user`) is refused | Proceed, warning finding |
| `before_patch_apply` | The apply is refused; nothing is written | Satisfied, because applying is your own action; recorded as a warning | Warning on the result; the apply proceeds |
| `before_task_complete` | The task does not complete; the reason goes to the model, which gets another round within the existing budget | Treated as `deny` | Complete, warning finding |
| `after_tool`, `after_patch_apply`, `after_check` | Nothing is undone. An error finding; at `after_tool` the reason is also appended to the tool result | Treated as `warn` | Warning finding |
| `session_end` | Treated as `warn`. Audit only | Treated as `warn` | Audit only |

A failure follows the same direction: a **mandatory** hook's failure (non-zero
exit, timeout, malformed output, version mismatch, output too large, a spawn
failure, or `changed_since_review`) becomes `deny`; an **optional** hook's
failure becomes a warning and the action proceeds.

`deny` short-circuits the remaining hooks for that event. The others run, and
the strongest verdict wins: `deny` > `request_approval` > `warn` > `allow`,
regardless of order. So a later hook can still deny an action an earlier one
merely asked about.

### Mandatory is the default

A user who writes a hook to enforce a rule wants the rule enforced when the
hook itself is broken, not silently skipped. So `mode` defaults to
`mandatory`, and a timeout, a typo in the script, a crash or malformed output
blocks the action. Use `mode=optional` for advice: a linter, a notifier, or
anything that should never stop your work.

The choice is per hook and shown as **If it fails: blocks** or **If it fails:
warns** in Settings, so "what happens if this breaks?" is answerable without
reading the config.

### A worked example: `cargo fmt` before completion

The useful way to require a check is for the hook to run it and deny when it
fails. The denial sends the reason back to the model, which gets another round
and can fix the problem:

```sh
#!/bin/sh
# ~/bin/no-unformatted.sh
if cargo fmt --all -- --check >/dev/null 2>&1; then
  printf '%s\n' '{"hookApiVersion":1,"verdict":"allow"}'
else
  printf '%s\n' '{"hookApiVersion":1,"verdict":"deny","reason":"cargo fmt --check failed; run cargo fmt --all"}'
fi
```

```text
hook.fmt.command=/bin/sh
hook.fmt.args=/Users/me/bin/no-unformatted.sh
hook.fmt.events=before_task_complete
```

No executable bit is needed because `/bin/sh` reads the script. The check runs
as your own trusted program, outside the mode and profile; the hook's verdict
is all that comes back. Do not ask Damaian to run `cargo fmt` through the
`commands` field instead: a command with side effects needs approval, a hook
never raises an approval card, and the command's result would not change the
hook's verdict anyway.

### Repository hooks and review

A repository may ship a hook definition in its `.damaian/config.conf`, but it
arrives **pending**, not running. Damaian shows the full command line, events
and fail direction in a consent dialog; **Decline** is focused by default, and
dismissing the dialog records no decision and asks again next time. Approving
records a hash of the exact definition, including the contents of any script
inside the repository, in your user config as
`hook_review.<repository_id>.<hook_id>`.

- If the repository later changes the definition or the script, the hook
  becomes **changed since review** and stops running until you approve the new
  definition. A mandatory hook fails closed while it is in that state.
- A **declined** hook never runs. It is not listed in the consent notice
  again, but **Settings › Hooks** still shows it with **Approve**, so a
  mistaken decline is reversible.
- A repository can never change, disable, retime or make optional a hook that
  your config or an administrator's config defines. Those keys are refused
  and shown in the audit log by key and class.
- A profile cannot carry a hook, so importing a permission profile cannot add
  one.

### Reviewing, enabling and testing from the CLI

The command-line tool reads the same config and data directory as the app:

```bash
damaian hooks /path/to/repo
damaian hook-review /path/to/repo no_migrations approve
damaian hook-review /path/to/repo no_migrations decline
damaian hook-run /path/to/repo no_migrations before_tool
```

`hooks` lists every hook with its source, events, fail direction, activation,
command line and, for a repository hook, its review hash. `hook-review` writes
only your user config and
audits the decision as `hook_reviewed`; an approval at a hash that is no longer
current is refused. `hook-run` runs one hook against a representative event
payload and prints its verdict, failure kind, duration and redacted stderr —
the fastest way to debug a script.

A test run does not count as an invocation in the audit log. The shell's
**Test** button and `damaian hook-run` both refuse a repository hook that is
pending, changed since review, declined or disabled; make it active first. A
user's own disabled hook can still be tested.

### What a hook may ask Damaian to run

A hook's `commands` field asks Damaian to run up to three local commands and
turn their output into findings. It is deliberately narrow:

- Only a command policy would run **unasked** in this session's mode and
  profile is run. Ask mode refuses them; a command that needs approval, or
  that the profile blocks, is refused. A hook never raises an approval card.
- Each run is bracketed by an action marker, so [crash
  recovery](#after-a-crash) sees it, and its findings belong to the session.
- A requested command's result does not change the hook's verdict. A rule that
  depends on a check runs the check itself, as the `cargo fmt` example does.
- A hook never sees its own requested command's `before_tool`/`after_tool`
  event. Requested commands can nest hooks, but only to a depth of two; past
  that the nested dispatch is skipped, the command still runs, and a warning
  finding and `hook_depth_exceeded` audit record say so.

## Browser Diagnostics

When a browser MCP server is configured, such as a Playwright server under
[**Settings › MCP Servers**](#mcp-servers), the assistant can look at a web page the way you see it.
It can open a URL and report what the page did, or run a short scenario:
fill a field, click a button, wait, take a screenshot. This is how it finds
problems that `curl` cannot see, such as a script that throws on load and
leaves a button with nothing attached to it.

### What needs your approval

- **Inspecting a local page** (`localhost`, `127.0.0.1`, `[::1]`) runs
  without asking. It only opens the page and reads it.
- **A scenario on a local page** asks first, because it types and clicks. The
  prompt shows the steps and the target origin. **Allow for this session**, in
  the card's ⋯ menu, covers later local scenarios in the same session.
- **Anything outside your machine**, whether an inspection or a scenario, asks
  every time and shows the origin it will reach. The session option never
  covers a remote URL.

### The diagnostic card

Each diagnostic that ran leaves a card under the assistant's answer:

- **The header** counts what went wrong, for example *Browser diagnostic
  found 1 page error, 2 console warnings.* A page with no problems reads
  *found no page errors, console problems, or failed requests.*
- **The target line** says *Inspected* or *Scenario*, and the URL.
- **The page line** gives the final URL, the HTTP status and the page title.
- **Page errors** are uncaught exceptions: the first thing to read when
  something "does nothing".
- **Console** lists errors and warnings in the order the page logged them,
  with the source file and line when the browser gave one.
- **Failed requests** lists requests that could not be sent, and responses of
  400 or above.
- **Failed steps** lists the scenario steps that did not succeed.
- **Screenshots** are thumbnails. **Reveal in Finder** selects the image file
  on disk.
- **Page summary** opens the page's buttons, form fields, status text and a
  visible-text excerpt.

A list longer than five items shows the first five and a **Show all** toggle.

**A page with errors is a successful diagnostic.** The header says *found*,
and the assistant is meant to read the errors, not retry. Only a header that
says **Browser diagnostic failed** means the diagnostic itself did not run:
the browser could not start, a step's arguments were rejected, or it timed
out. Repeating a failed call counts toward the retry limit; finding page
errors does not.

What the assistant reads is the same summary as the header, as text.
Everything in it has been through the secret scanner first. Screenshots are
for you; they are not sent to the model.

Cards survive a reload and a restart. A browser server that is not
Playwright-shaped still works, but its card has no problem lists: only the
header, any screenshots, and a **Diagnostic output** disclosure with the raw
result (see
[TROUBLESHOOTING.md](TROUBLESHOOTING.md#browser-diagnostics)).

## Findings

A **finding** is one problem a check reported: a compiler error at a file and
line, a failing test, a lint rule, a browser console error or failed request, a
hook's own finding or warning, a diagnostic a language server published, or a
whole check that failed in a way Damaian could not take apart. Findings come
from the checks the assistant runs during a conversation, from its browser
diagnostics, from [hooks](#hooks), and from a configured
[language server](#language-servers). They belong to the session.

Every command the assistant runs has a **purpose**, and the purpose decides
whether a failure becomes a finding.

- **Check** commands produce findings. A check is a detected project
  validation (`cargo test`, `npm run lint`), a command the command policy
  knows as a validation, or one whose output a finding parser reads
  (`cargo build`, `cargo check`, `cargo clippy`, Biome).
- **Exploration** commands do not. These are the assistant looking around:
  searches (`grep`, `rg`, `find`), listings (`ls`, `pwd`, `du`, `stat`,
  `file`), text reads (`cat`, `head`, `tail`, `wc`, `sed` without `-i`, `awk`,
  `sort`, `uniq`, `cut`, `echo`, `true`, `read`), and git reads (`status`,
  `diff`, `log`, `show`, `rev-list`, `rev-parse`, `branch --list`,
  `worktree list`, `ls-files`, `grep`), including pipelines and `&&`/`||`/`;`
  chains of those, with a leading `cd <dir>` or `set -e`/`set +e` ignored. A
  `for`, `while` or `until` loop counts when every command it runs does, so a
  loop that reads files is exploration and one that deletes them is not. A
  command with `$(…)` or backticks never counts. A search that matched nothing or a path
  that does not exist is an answer, not a problem in your code, so its output
  still reaches the assistant and no finding is recorded.
- **Other** commands, including any command Damaian does not recognise,
  produce findings as a check does.

### Fix N errors, and the findings list

When the session has open findings, the [repo bar](#the-repo-bar) above the
prompt offers to fix them. With open errors it reads **Fix 2 errors** (in red);
with no open errors but open warnings, **Fix 1 warning** (in amber); otherwise
there is no button. Pressing it starts one repair turn covering every open
finding of that severity, exactly as if you had ticked them all and pressed
**Fix selected** below. It is disabled while a turn is running.

The chevron beside it opens the findings list, upward over the conversation.
Escape, the chevron again, or a click elsewhere closes it. Opened from **Fix N
warnings**, the list shows errors and warnings rather than only errors, so it
is never empty.

**The list is reachable only while there are open errors or warnings.** With
nothing open (every finding dismissed, fixed, stale, or only informational)
there is no button and no chevron, so a dismissed finding cannot be restored
from the app until a check reports something new. The findings are still in
the session; [TROUBLESHOOTING.md](TROUBLESHOOTING.md#findings) shows how to
read them from the session log.

The list groups findings by source (Compiler, Tests, Lint, Security, Hook,
Browser console, Network, Browser scenario, Code review, Language server,
Unparsed checks), then by file. Two filters choose
what is shown:

- **Severity:** Errors (the default), Errors and warnings, or All severities.
- **Status:** Open (the default), Open and stale, or All statuses.

"Showing 2 of 5" tells you how much the filters hide.

Each row shows the summary, the location when the tool reported one, and a
collapsed **Details** with the tool's own excerpt. Click the location to open
the file at that line, the same way a file reference in a reply does. A
finding whose tool gave no location has none. Damaian does not guess one.

### Dismiss and Restore

**Dismiss** hides a finding you have decided not to act on. It stays in the
session, and **Restore** (under the *All statuses* filter) brings it back,
as long as the list can still be opened (see above).
Dismissing is not suppression: if a later check reports the same problem, it
is a new finding and it appears again.

### Fix selected

Tick the findings you want repaired and press **Fix selected**. The assistant
gets a request naming exactly those findings, with their locations and
excerpts, not the raw tool log, and runs an ordinary turn that can read, edit
and re-run the check.

Only **open** findings are sent. A stale, dismissed or fixed finding in the
selection is left out, and a note says how many were left out and why. If
nothing is left, nothing is sent.

Both **Fix selected** and the repo bar's **Fix N errors** show the exact repair
prompt in a confirmation before sending. **Send** starts the turn; **Cancel**
sends nothing and leaves your selection. The prompt is the whole request: the
finding ids, their summaries and the excerpts the assistant will see. If every
selected finding is a generic command failure with no file location, the
confirmation says it is likely an environment or `PATH` problem rather than a
code defect, so a missing tool is not turned into a code change by accident.

### What stale means

A finding records a fingerprint of its file when the check ran. If the file
has changed since, the finding is **stale**: the line it points at may no
longer hold the problem, or may hold different code. A stale finding cannot be
selected for a fix, because repairing against a location that has moved is
how an edit lands on the wrong lines. Re-run the check to get current
findings. If the file is changed back, the finding is open again.

A finding without a location, or whose file could not be read when it was
recorded, never goes stale.

### Known limits

- **Commands you run outside a conversation record no findings.** Only the
  checks the assistant runs inside a turn, including ones you approved, are
  recorded.
- **Some checks are not taken apart.** `cargo build`, `check`, `clippy` and
  `test`, Biome (`npm run lint:web` and similar), and browser diagnostics are
  parsed into individual findings. `cargo nextest` and any other check give one
  **Unparsed checks** finding per failed run, with the end of its output in
  Details.
- **A browser location opens a file only when exactly one file matches.** A
  bundled script, a file served from another origin, or a path that matches
  two files in your repository is shown without a location.
- **A language-server diagnostic has no task.** It is recorded when the findings
  panel refreshes, with the source **Language server** and its range, but with
  no task id: it is never attached to a plan step and never blocks one by
  itself. A diagnostic that a later edit republishes unchanged is recorded
  again, and the earlier one turns [stale](#what-stale-means). Diagnostics
  arrive after the first symbol search has let a configured server open the
  files
  ([details](TROUBLESHOOTING.md#symbol-search-shows-only-likely-results-or-a-language-server-will-not-start)).

Where a finding's full output lives, and how to read findings from the
session log, is in
[TROUBLESHOOTING.md](TROUBLESHOOTING.md#findings).

## Provider Limits and Retries

A model provider can refuse a call — rate limiting ("you are going too fast"), a
transient overload, or an exhausted quota or balance. When the refusal is
transient, Damaian retries automatically: the progress indicator says how long
it is waiting (for example "Provider refused — retrying in 12s"), and you can
stop the turn at any point during that wait. Retrying gives up after a bounded
number of attempts and at most 90 seconds of total waiting.

If the provider still refuses, the turn ends with a message that names the
reason and carries the provider's own words — which is where you learn what to
top up. The task keeps its plan, so you can resume the same work once the
provider recovers or after you fix your account.

Damaian never switches to a different provider on its own. If you have a second
provider configured with its own API key, a refused turn instead pauses on an
offer that names both providers: **Answer this turn with …?** Choose:

- **Keep it failed** — the default, and the focused button. The turn fails
  exactly as it would have without the offer.
- **Answer with …** — the same request is sent to the other provider, the turn
  continues there, and its plan carries over unchanged. A note in the
  conversation ("Switched to … for this turn after … refused it") marks where
  the other model took over, and it stays there after a reload.

The offer is for that one turn only. There is no "always" option, nothing is
written to your configuration, and your next turn starts on your configured
provider again — if it refuses too, you are asked again. If the other provider
also refuses, the turn fails; you are not offered a third choice. If Damaian
stops while the offer is open, it is shown again under **Still waiting for
you** when you return.

Only the desktop app makes this offer. For a quota or balance refusal, the other
provider must use a different API key, since two providers reading one key are
the same account.

## Capping What One Turn May Spend

`agent_max_task_tokens` stops a turn once it has spent that many tokens, rather
than letting a stuck agent run up a bill:

```text
agent_max_task_tokens=120000
```

**The cap is per turn, not per session or per day.** One turn is one thing you
asked for — one message and everything Damaian does to answer it, including
every tool call and every model call it makes along the way. Ten turns with a
120000 cap can spend 1.2M tokens between them. Read it as "no single request
may cost more than this", and set it against what one large request should
reasonably cost, not against your budget for the week.

There is no default: without this setting, nothing caps a turn's tokens and
`agent_max_tool_rounds` and `agent_max_exploration_rounds` are the only bounds.
The minimum accepted value is 1000 — below that, a turn would stop before it
could do anything.
With the cap set, a turn that reaches its action-round limit continues on its own
instead of pausing for **Continue** ([While a turn runs](#while-a-turn-runs-and-how-it-ended)),
and the messages it sends are bounded by `agent_max_turn_messages` so a long
turn does not grow without limit.

When a turn stops this way you see **Token budget exhausted** under the answer,
and the reply names what was finished and what was left. Nothing is lost:
raise the cap and ask again, and the turn picks the plan up where it stopped,
keeping the evidence its earlier steps had already gathered.

This is different from **Tool budget exhausted**, which means the turn used its
action rounds and you chose **Answer now**. One ran out of rounds; the other ran
out of money.

A repository's own `.damaian/config.conf` may only tighten this cap: it can
lower one you set, or introduce one where you had none, but it can never raise
or remove yours.

## What a Turn Cost

Every assistant reply carries a line underneath it saying what the turn used:

```text
12,714 tokens · 3 model calls
```

The count covers the whole turn, not the last request. A turn that used tools
makes several model calls, and each one re-sends the assembled context, so
three calls cost roughly three times what one does. The line survives a reload,
so you can look back at what an old conversation spent.

**"Estimated" means Damaian counted the tokens itself** rather than being told.
Most providers report exact usage and the figure is then shown without the `~`
or the marker. It falls back to an estimate when:

- the provider does not report usage, or rejects the request for it;
- you stopped the turn mid-answer, so no final report arrived;
- the app crashed while a call was in flight, and the call is counted from what
  was sent rather than lost.

A turn mixing both is marked estimated overall — a total is only as reliable as
its least reliable part. The estimate is a size-based approximation and is not
exact; treat it as a rough scale, not a bill.

### Showing a cost

Damaian ships no price list. Prices change, and a stale table would report
confident wrong numbers. If you want a cost figure, give it your own rates per
million tokens. The figure is always shown with a `$` sign, so enter your
rates in US dollars:

```text
model_provider.deepseek.price_per_million_input_tokens=0.27
model_provider.deepseek.price_per_million_output_tokens=1.10
```

Both are required; one alone would produce a number that quietly omits half the
spend. The result is labelled `at your rates`, because it is your arithmetic
rather than the provider's invoice. Anything under a hundredth of a cent shows
as `<$0.0001` rather than rounding to zero.

**One rate per direction may not fit your provider's pricing.** Some providers
vary by time of day — DeepSeek, for instance, doubles both its cache-hit and
cache-miss rates during weekday peak hours — and Damaian has no way to tell
which price window applied, so pick the rate matching how you mostly work and
read a time-varying provider's figure as an order of magnitude rather than an
amount owed. Cache pricing itself has its own dedicated rate; see
[Prompt cache hit rate](#prompt-cache-hit-rate) below. The token counts
themselves are exact wherever they are not marked estimated.

Where a provider does report a cost of its own, that figure is shown instead
and carries no such label.

### Prompt cache hit rate

Where a provider serves part of a request's input tokens from its own prompt
cache and reports the split, the turn's usage line adds a hit rate:

```text
12,714 tokens · 3 model calls · at most $0.0018 at your rates · 75% cached
```

**The hit count is measured only — never estimated.** There is no way to
estimate a cache hit from a payload's size, so a turn with no provider-reported
usage shows the estimated token total described above but no cache figure at
all, rather than a guessed one.

**"Not reported" is not 0%.** Hovering the usage line explains why no
percentage shows: either this provider never reports a cache split, or it
reported one but the turn had no input tokens to measure it against. Both read
as "the split cannot be seen from here," never as "nothing was cached" — an
actual 0% only appears when the provider explicitly reports zero cached tokens
for a call that had some to report on.

If some of a turn's model calls reported a split and others did not, the
percentage covers only the calls that did, and the label says how many:
`75% cached (of 2 of 3 calls)`.

**Cost.** Configure a cached rate to price the split exactly:

```text
model_provider.deepseek.price_per_million_input_tokens=0.27
model_provider.deepseek.price_per_million_output_tokens=1.10
model_provider.deepseek.price_per_million_cached_input_tokens=0.028
```

With a cached rate set and a split reported, the cached and uncached tokens are
billed at their own rates. Without one, Damaian still shows a figure — computed
at the full input rate on every input token, and labelled **at most** — rather
than nothing, because the error can only run one direction: charging the
uncached rate on tokens that were actually cheaper can only overstate the cost,
never understate it. A figure that could understate the true cost is
suppressed instead, the same rule the missing-base-rate case above follows.

A hit rate reflects what today's conversation happened to hit in the
provider's own cache, not a setting you can raise, and Damaian does not yet
change how a request is assembled to make a hit more likely — see
[Cache hit rate reads "not reported", or seems low](./TROUBLESHOOTING.md#cache-hit-rate-reads-not-reported-or-seems-low)
if a rate you expected to see is missing or lower than you expected.

## Local Data

Damaian stores audit records, sessions, and patch proposals locally. By default, global app data is stored under:

```text
~/Library/Application Support/DamaianClient
```

`DAMAIAN_DATA_DIR` is an optional override, not the default. During development you can set:

```sh
DAMAIAN_DATA_DIR=.damaian
```

This keeps Damaian data inside the current working directory. If you prefer a home-directory dotfolder, launch with:

```sh
DAMAIAN_DATA_DIR=~/.damaian
```

Your own settings live inside that directory too, at `config/user.conf`, so `DAMAIAN_DATA_DIR` moves them along with everything else. A run with the variable set reads the `user.conf` under *that* directory and not your usual one — which is what makes it a clean way to try a different provider without touching your real configuration.

Repository-scoped config remains separate and lives at `.damaian/config.conf` inside the selected repository.

Two parts of that directory hold copies of your files exactly as they were, credentials included, because their job is to put a file back byte for byte: `checkpoints/` (session checkpoints) and `rollback/` (patch rollback). They never leave your machine, and they are not safe to attach to a bug report. Session logs hold your prompts and file content, so the same applies to them.

`symbol-index/<repository_id>.json` holds the derived symbol index and cached reference answers. It is safe to delete, and Damaian rebuilds it on the next symbol search; [Troubleshooting](TROUBLESHOOTING.md#symbol-search-shows-only-likely-results-or-a-language-server-will-not-start) shows how to force that.

## The Command-Line Tool

This guide mentions `damaian` commands, such as `damaian config-show` or `damaian profile-set`. The command-line tool is not included in `Damaian.app`. It is built from a source checkout of Damaian, where you run it as `cargo run -p damaian-cli -- <command>`; see [DEVELOPMENT.md](DEVELOPMENT.md#cli). It reads and writes the same [user config](#settings) and [data directory](#local-data) as the app.

## Safety Model

Damaian keeps the local app in control of important effects:

- The model does not read files directly.
- The model does not write files directly.
- File edits are previewed before application.
- Assistant commands run without approval only when Damaian classifies them as read-only; they run directly on your machine.
- Every other command requires your approval before it runs, unless you [allowed that exact command always](#allow-always) for that repository.
- Restricted files and detected secrets are redacted or blocked by policy.
- A [permission profile](#permission-profiles) per repository can narrow all of this further, and never widen it.
- Important actions are recorded in a local audit trail.

## Troubleshooting

If the app shows "Repository is required", select **+** beside **Projects** and pick a working folder.

If model calls fail, open Settings and confirm the **Model API Key** status is **Saved**, or, for the command-line tool or a development build, confirm that `model_api_key_env` names an environment variable and that the variable is set before launching ([Environment variables](#environment-variables-cli-and-development-only)).

If the assistant announces a file change, such as `Let me create all the necessary files:`, but no patch appears, its reply was cut off at the model's output limit. Ask it to create fewer files at a time, or raise **Max output tokens** in **Provider details**. See [Output and Context Limits](#output-and-context-limits).

If the assistant answers questions but never proposes file changes or uses MCP tools, check that [Native tool-calling](#native-tool-calling) is still on for the provider.

If macOS warns that the app is from an unidentified developer, see [macOS Installation](./MACOS_INSTALLATION.md#install). A release build should never show that warning.

For anything not covered here — where configuration and logs are stored, how to read the audit trail, and how to diagnose a failure in detail — see [Troubleshooting](./TROUBLESHOOTING.md).

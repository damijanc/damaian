# Damaian User Guide

Damaian is a local-first AI coding assistant for macOS. It indexes a local Git repository, prepares focused context for coding questions, previews generated edits as diffs, and records local audit data.

This build is a developer preview. It is usable for local workflows and can offer signed in-app updates from GitHub Releases, but it is not yet Developer ID signed or notarized.

## First Run

1. Open `Damaian.app`.
2. Select `+` beside `Projects`.
3. Pick the local Git repository or working folder you want Damaian to inspect.
4. Select the Visual Studio Code icon when you want to open the same folder in Visual Studio Code, or the terminal icon to open the bottom terminal panel.

## Updates

Damaian checks for updates when the desktop app starts. If a newer GitHub Release is available, an `Update <version>` button appears in the conversation header. Select it to download the signed update, install it, and restart the app.

The first installed version must already include the updater. If you installed an older developer-preview DMG before automatic updates were added, download and install one newer DMG manually once.

## Working Folder

Damaian uses the selected working folder as the root for indexing, file reads, patch previews, Git status, and repository-scoped settings.

Use `+` beside `Projects` to open the native macOS folder picker. The selected folder appears under `Projects` by folder name. Expand a project to see its sessions grouped underneath it. Use the `+` beside a project folder to start a new session for that project.

Select a project folder in the sidebar to switch the active working folder.

Damaian remembers the project list and the last selected working folder in local app storage. The last folder is restored when the app restarts. Launch-time defaults such as `DAMAIAN_REPO` are used only when no previous selection has been saved.

Use the Visual Studio Code icon in the conversation header to open the selected working folder in Visual Studio Code. Damaian keeps AI orchestration, context assembly, patch preview, settings, and audit logging in the app; normal code navigation and IDE work happen outside Damaian in Visual Studio Code.

## Terminal

Use the terminal icon in the conversation header to show or hide the bottom terminal panel. The terminal opens in the selected working folder. If no folder is selected yet, it opens in your home directory.

Commands are entered manually by the user and run directly on the local machine. Use `cd` to change the panel's working directory, and `clear` to clear the panel output.

## Chat

Use the `Chat` tab to ask questions about the selected repository. Damaian retrieves relevant local files, redacts detected secrets, streams the answer, and shows the context files used for the response.

Press `Enter` to send a chat message. Press `Shift+Enter` to insert a new line.

Questions stream read-only answers. Code and file change requests, such as `create a test file` or `update the README`, generate an inline patch preview in the conversation.

Use `+ File` above the prompt to pin specific repository files into the next chat request. Pinned files appear as chips above the prompt and are included before automatic retrieval. Use the `x` on a chip to remove one file, or `Clear` to remove all pinned files.

If you name a file in your prompt, such as `USER_GUIDE.md` or `docs/USER_GUIDE.md`, Damaian attempts to include that file in the model context. A filename without a directory must uniquely match one file in the selected repository.

If the assistant needs a local command result to answer a question, it can request one command from Damaian. Sandbox-safe read-only commands, such as `pwd`, `ls`, `git status`, `git diff`, `git log`, and `git show`, run automatically in the selected working folder. Damaian redacts the output and sends it back to the model so it can finish answering.

Commands that cannot run in sandbox mode appear as an approval card in the conversation. Review the command, risk, working directory, and reason, then select `Approve Run` or `Reject`. Destructive commands blocked by policy cannot be approved from the UI.

Sessions are shown under their project folder in the sidebar. Select an existing session to reload its conversation. Double-click a session to rename it, or use the `-` beside a session to delete it.

Context file buttons open the referenced file in Visual Studio Code.

## File Changes

Use the conversation box to request file changes.

1. Enter a request such as `create a test file for the config parser`.
2. Review the inline patch preview returned by the assistant.
3. Keep checked only the files you want to act on.
4. Select `Apply Selected` to write those files, or `Reject Selected` to record selected files as rejected without changing the workspace.

Damaian checks file hashes before applying a stored patch. If a target file changed after preview, that file is blocked instead of overwriting newer local work.

After files are applied, Damaian prints a concise Git status summary in the conversation.

## Settings

Use the `Settings` tab to inspect and edit user configuration values, then select `Load`.

Configuration uses one `key=value` entry per line. Edit values directly and select `Save`. Delete a line and save to remove that user-level override.

`model_api_key_env` is a reference field. The app rejects raw API keys in this field; use the `Model API Key` controls to store the secret in Keychain.

Common keys:

- `restricted_patterns`: File patterns Damaian should avoid reading, separated by `|`.
- `audit_enabled`: Set to `true` or `false`.
- `model_base_url`: OpenAI-compatible API base URL.
- `model_name`: Model identifier.
- `model_api_key_env`: API key reference. Use `keychain:model-api-key` for the desktop Keychain flow, or an environment variable name for CLI/dev workflows.
- `model_provider.<id>.max_output_tokens`: Largest reply the model may generate, in tokens. Omit to use the built-in default for the selected model. See [Output and Context Limits](#output-and-context-limits).
- `model_provider.<id>.context_token_budget`: How much repository content is sent with each request, in tokens. Omit to use the built-in default for the selected model.
- `model_provider.<id>.supports_native_tools`: Set to `true` to use the provider's native tool-calling API. Required for MCP tools, patch proposals, and other agentic actions.

Repository-scoped settings are not edited from the UI. Put repository defaults in `.damaian/config.conf` inside the selected repository. Repository settings are included in `Effective Policy` and can override user settings.

## Model Providers and API Keys

Damaian uses OpenAI-compatible chat APIs. Configure the provider URL and model in Settings, but do not paste the API key into the configuration file.

In the desktop app, use the `Model API Key` controls in Settings:

1. Enter a Keychain account name, such as `model-api-key`.
2. Paste the API key into `API Key`.
3. Select `Save Key`.

Damaian stores the secret in macOS Keychain and writes only this reference to config:

```text
model_api_key_env=keychain:model-api-key
```

Damaian keeps a process-local in-memory copy after a successful Keychain save or read. You may be asked by macOS the first time the app accesses the key after launch, but repeated chat, edit, or command-assisted answers in the same app run should not require another password prompt.

Use `Remove Key` to delete the stored secret from Keychain. Saving a new key with the same account replaces the previous value.

If `Effective Policy` still shows a different `model_api_key_env` after saving the key, a repository or admin config is overriding the user setting. Remove or update that override before retrying chat.

Environment variables remain supported for CLI and development workflows. In that mode, `model_api_key_env` is the name of an environment variable that contains the key. It is not the key itself.

Example DeepSeek configuration:

```text
model_provider=deepseek
model_name=deepseek-v4-flash
model_base_url=https://api.deepseek.com
model_api_key_env=keychain:model-api-key
```

For environment-variable based development, use:

```text
model_provider=deepseek
model_name=deepseek-v4-flash
model_base_url=https://api.deepseek.com
model_api_key_env=DEEPSEEK_API_KEY
```

Then launch the app from a shell where that environment variable is set:

```sh
export DEEPSEEK_API_KEY="your-deepseek-api-key"
npm run desktop:dev
```

Or set it for one launch:

```sh
DEEPSEEK_API_KEY="your-deepseek-api-key" npm run desktop:dev
```

The same pattern applies to OpenAI or any OpenAI-compatible provider.

If an older configuration still names `deepseek-chat` or `deepseek-reasoner`, update it. Those were compatibility aliases that DeepSeek retired on 24 July 2026; the current models are `deepseek-v4-flash` and `deepseek-v4-pro`.

## Output and Context Limits

Two settings control how many tokens each request may use. They are independent: one bounds what the model writes, the other bounds what it reads. Both live in `Settings` under `Providers`, in the `Provider details` panel, and both apply per provider.

- `Max output tokens` caps the reply the model generates in a single response. Set too low, a large multi-file patch is cut off mid-way. Damaian detects this, tells the model its call was truncated, and asks it to retry with fewer files per patch, so the request recovers instead of failing silently.
- `Context budget` caps how much repository content Damaian packs into each request. Raising it lets the assistant see more of a large repository, which can improve answers. This content is re-sent and re-billed on every turn, so a higher budget increases cost and latency on all requests, not just large ones.

Leave a field blank to use the built-in default for the selected model. The defaults are:

- `deepseek-v4-flash` and `deepseek-v4-pro`: 65536 output tokens, 64000 context budget.
- `deepseek-chat` and `deepseek-reasoner` (retired aliases): 8192 output tokens, 16000 context budget.
- All other models: no output cap is sent, and the context budget is 16000.

The built-in context budgets are deliberately well below what these models technically accept. DeepSeek's V4 models allow up to 384000 output tokens and a 1M-token context window, but requesting the maximum on every turn is rarely worth the cost. Raise the values if your repository or your patches need more.

A value entered in the panel always wins over the built-in default. To give a V4 model its full output range:

```text
model_provider.deepseek.max_output_tokens=384000
```

Setting a value higher than the model actually allows causes the provider to reject the request outright, so prefer the built-in defaults unless you know the model's real limit.

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

Repository-scoped config remains separate and lives at `.damaian/config.conf` inside the selected repository.

## Safety Model

Damaian keeps the local app in control of important effects:

- The model does not read files directly.
- The model does not write files directly.
- File edits are previewed before application.
- Sandbox-safe assistant command requests are limited to read-only local commands.
- Commands outside the sandbox require user approval before execution.
- Restricted files and detected secrets are redacted or blocked by policy.
- Important actions are recorded in a local audit trail.

## Troubleshooting

If the app shows `Repository is required`, select `+` beside `Projects` and pick a working folder.

If model calls fail, open Settings and confirm the `Model API Key` status is `Saved`, or confirm that `model_api_key_env` names an environment variable and that the variable is set before launching the app.

If the assistant announces a file change, such as `Let me create all the necessary files:`, but no patch appears, its reply was cut off at the model's output limit. Ask it to create fewer files at a time, or raise `Max output tokens` in `Provider details`. See [Output and Context Limits](#output-and-context-limits).

If the assistant answers questions but never proposes file changes or uses MCP tools, enable `Native tool-calling` for the provider in `Provider details`. Patch proposals and MCP tools require it.

If macOS warns that the app is from an unidentified developer, see [macOS Installation](./MACOS_INSTALLATION.md).

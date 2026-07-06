# Damaian User Guide

Damaian is a local-first AI coding assistant for macOS. It indexes a local Git repository, prepares focused context for coding questions, previews generated edits as diffs, runs approved commands, and records local audit data.

This build is a developer preview. It is usable for local workflows, but it does not yet include Keychain-backed API key storage, automatic updates, code signing, or notarization.

## First Run

1. Open `Damaian.app`.
2. Select `Choose` beside the `Repository` field.
3. Pick the local Git repository or working folder you want Damaian to inspect.
4. Select `Status` to confirm Damaian can inspect the repository.
5. Select the Visual Studio Code icon when you want to open the same folder in Visual Studio Code.

You can also enter an absolute path manually:

```text
/Users/your-name/development/my-project
```

## Working Folder

Damaian uses the selected working folder as the root for indexing, file reads, patch previews, command execution, Git status, and repository-scoped settings.

In the packaged desktop app, use `Choose` to open the native macOS folder picker. In browser-only development mode, type the absolute path manually.

If you switch folders, use `Status` to verify the new root before asking questions, applying edits, or running commands.

Damaian remembers the last selected working folder in local app storage and restores it when the app restarts. Launch-time defaults such as `DAMAIAN_REPO` are used only when no previous selection has been saved.

Use the Visual Studio Code icon in the conversation header to open the selected working folder in Visual Studio Code. Damaian keeps AI orchestration, context assembly, patch preview, command approval, settings, and audit logging in the app; normal code navigation and IDE work happen outside Damaian in Visual Studio Code.

## Chat

Use the `Chat` tab to ask questions about the selected repository. Damaian retrieves relevant local files, redacts detected secrets, streams the answer, and shows the context files used for the response.

Press `Enter` to send a chat message. Press `Shift+Enter` to insert a new line.

If you name a file in your prompt, such as `USER_GUIDE.md` or `docs/USER_GUIDE.md`, Damaian attempts to include that file in the model context. A filename without a directory must uniquely match one file in the selected repository.

Use `New`, `Rename`, and `Delete` to manage project-scoped chat sessions. Selecting an existing session reloads its conversation and future questions continue with recent prior messages as context.

Context file buttons open the referenced file in Visual Studio Code.

## Edits

Use the `Edits` tab to preview and apply generated file changes.

1. Enter a short request in `Describe the change`.
2. Paste a model edit envelope into the larger text area.
3. Select `Preview` to generate a diff.
4. Review each file diff.
5. Keep checked only the files you want to act on.
6. Select `Apply Selected` to write those files, or `Reject Selected` to record selected files as rejected without changing the workspace.

Damaian checks file hashes before applying a stored patch. If a target file changed after preview, that file is blocked instead of overwriting newer local work.

## Commands

Use the `Commands` tab to run validation commands through Damaian's command policy.

1. Enter a command such as `npm test` or `cargo test`.
2. Select `Propose`.
3. Review the approval prompt and risk classification.
4. Select `Run` to execute the stored proposal, or `Reject` to record the rejection.

Destructive or shell-control-heavy commands are blocked or approval-gated by default.

## Settings

Use the `Settings` tab to inspect and edit configuration values. Choose `User` for global settings or `Repository` for settings stored in the selected working folder, then select `Load`.

Configuration uses one `key=value` entry per line. Edit values directly and select `Save`. Delete a line and save to remove that override from the selected scope.

Common keys:

- `command_allowlist`: Commands that may run with lower friction, separated by `|`.
- `restricted_patterns`: File patterns Damaian should avoid reading, separated by `|`.
- `audit_enabled`: Set to `true` or `false`.
- `model_base_url`: OpenAI-compatible API base URL.
- `model_name`: Model identifier.
- `model_api_key_env`: Environment variable name used for the API key.

Repository-scoped settings are stored at `.damaian/config.conf` inside the selected repository. User-scoped settings apply across repositories.

## Model Providers and API Keys

Damaian uses OpenAI-compatible chat APIs. Configure the provider URL and model in Settings, but do not paste the API key into the configuration file.

`model_api_key_env` must be the name of an environment variable that contains the key. It is not the key itself.

Example DeepSeek configuration:

```text
model_provider=deepseek
model_name=deepseek-v4-flash
model_base_url=https://api.deepseek.com
model_api_key_env=DEEPSEEK_API_KEY
```

Launch the app from a shell where that environment variable is set:

```sh
export DEEPSEEK_API_KEY="your-deepseek-api-key"
npm run desktop:dev
```

Or set it for one launch:

```sh
DEEPSEEK_API_KEY="your-deepseek-api-key" npm run desktop:dev
```

The same pattern applies to OpenAI or any OpenAI-compatible provider. For example, use `model_api_key_env=OPENAI_API_KEY` and set `OPENAI_API_KEY` before launching.

## Local Data

Damaian stores audit records, sessions, command proposals, and patch proposals locally. By default, global app data is stored under:

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
- Commands are proposed before execution.
- Restricted files and detected secrets are redacted or blocked by policy.
- Important actions are recorded in a local audit trail.

## Troubleshooting

If the app shows `Repository is required`, enter an absolute repository path in the sidebar.

If model calls fail, confirm that `model_api_key_env` names an environment variable and that the variable is set before launching the app.

If a command is blocked, inspect the command proposal text. Update `command_allowlist` only for commands you trust in that repository.

If macOS warns that the app is from an unidentified developer, see [macOS Installation](./MACOS_INSTALLATION.md).

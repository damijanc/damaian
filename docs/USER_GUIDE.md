# Damaian User Guide

Damaian is a local-first AI coding assistant for macOS. It indexes a local Git repository, prepares focused context for coding questions, previews generated edits as diffs, runs approved commands, and records local audit data.

This build is a developer preview. It is usable for local workflows, but it does not yet include Keychain-backed API key storage, automatic updates, code signing, or notarization.

## First Run

1. Open `Damaian.app`.
2. Select `Choose` beside the `Repository` field.
3. Pick the local Git repository or working folder you want Damaian to inspect.
4. Select `Status` to confirm Damaian can inspect the repository.
5. Use `Search` to verify indexing works for the selected project.

You can also enter an absolute path manually:

```text
/Users/your-name/development/my-project
```

## Working Folder

Damaian uses the selected working folder as the root for indexing, file reads, patch previews, command execution, Git status, and repository-scoped settings.

In the packaged desktop app, use `Choose` to open the native macOS folder picker. In browser-only development mode, type the absolute path manually.

If you switch folders, use `Status` to verify the new root before asking questions, applying edits, or running commands.

## Chat

Use the `Chat` tab to ask questions about the selected repository. Damaian retrieves relevant local files, redacts detected secrets, and sends the prompt plus context to the configured model provider.

For local testing without a model key, enter text in `Mock response` and select `Ask`. The app will run the same orchestration path but return the mock response instead of calling a model API.

## Edits

Use the `Edits` tab to preview and apply generated file changes.

1. Enter a short request in `Describe the change`.
2. Paste a model edit envelope into the larger text area.
3. Select `Preview` to generate a diff.
4. Review the diff.
5. Select `Apply` to write the approved files, or `Reject` to record the rejection.

Damaian checks file hashes before applying a stored patch. If a target file changed after preview, the patch is blocked instead of overwriting newer local work.

## Commands

Use the `Commands` tab to run validation commands through Damaian's command policy.

1. Enter a command such as `npm test` or `cargo test`.
2. Select `Propose`.
3. Review the approval prompt and risk classification.
4. Select `Run` to execute the stored proposal, or `Reject` to record the rejection.

Destructive or shell-control-heavy commands are blocked or approval-gated by default.

## Settings

Use the `Settings` tab to inspect and update policy values.

Common keys:

- `command_allowlist`: Commands that may run with lower friction, separated by `|`.
- `restricted_patterns`: File patterns Damaian should avoid reading, separated by `|`.
- `audit_enabled`: Set to `true` or `false`.
- `model_base_url`: OpenAI-compatible API base URL.
- `model_name`: Model identifier.
- `model_api_key_env`: Environment variable name used for the API key.

Repository-scoped settings are stored at `.damaian/config.conf` inside the selected repository. User-scoped settings apply across repositories.

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

If model calls fail, use `Mock response` for local testing or confirm that the environment variable named by `model_api_key_env` is set before launching the app.

If a command is blocked, inspect the command proposal text. Update `command_allowlist` only for commands you trust in that repository.

If macOS warns that the app is from an unidentified developer, see [macOS Installation](./MACOS_INSTALLATION.md).

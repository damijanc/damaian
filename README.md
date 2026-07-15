# Damaian Client

Local-first AI coding assistant client foundation.

This repository currently implements the workspace-engine slice from the product specification:

- repository indexing with default exclusions and `.gitignore` support
- controlled file reads scoped to selected repository roots
- context assembly with secret redaction
- structured patch preview and safe apply with hash checks
- terminal command risk classification and approval-gated execution
- basic Git status/diff wrappers
- append-only local audit logs
- provider-isolated model adapter interfaces
- dependency-free local desktop shell prototype served over localhost
- native Tauri desktop wrapper with macOS folder picker
- Projects sidebar with folder-grouped chat sessions
- one-click handoff from the selected working folder to Visual Studio Code
- embedded bottom terminal panel for user-run commands
- session-aware desktop chat with streamed responses and context file links
- sandbox-safe assistant command requests for local facts such as Git history
- per-file edit diff review with selected-file apply/reject
- macOS Keychain-backed model API key storage from desktop settings

The macOS desktop shell layers on top of these services while keeping AI file edits behind explicit preview/apply approval. When the assistant needs local command output, Damaian only runs sandbox-safe read-only commands automatically. Commands outside that sandbox are shown in the conversation for user approval before execution.

## Commands

```sh
npm test

# Command-line Rust implementation
cargo test
cargo run -p damaian-cli -- config-show /path/to/repo
cargo run -p damaian-cli -- config-set user command_allowlist "npm test|cargo test"
cargo run -p damaian-cli -- config-set repo /path/to/repo restricted_patterns ".env|*.pem|private/**"
cargo run -p damaian-cli -- config-set admin audit_retention_days 30
cargo run -p damaian-cli -- propose-command /path/to/repo "npm test"
cargo run -p damaian-cli -- propose-validations /path/to/repo
cargo run -p damaian-cli -- run-command command_proposal_id --approve
cargo run -p damaian-cli -- reject-command command_proposal_id
DAMAIAN_MOCK_MODEL_RESPONSE="Mock answer" cargo run -p damaian-cli -- ask /path/to/repo "What does auth do?"
OPENAI_API_KEY=... cargo run -p damaian-cli -- ask /path/to/repo "Explain the project"
DAMAIAN_MOCK_MODEL_RESPONSE=$'DAMAIAN_EDIT_V1\nSUMMARY: Update file\nFILE: src/app.js\nSTATUS: modified\nCONTENT:\n...\nEND_FILE\nEND_PATCH\n' cargo run -p damaian-cli -- propose-edit /path/to/repo "Make the change"
cargo run -p damaian-cli -- apply-patch /path/to/repo patch_id_from_preview
cargo run -p damaian-cli -- reject-patch patch_id_from_preview

# Local desktop shell prototype
cargo run -p desktop-shell -- --repo /path/to/repo --port 4765

# Native Tauri desktop wrapper
DAMAIAN_REPO=/path/to/repo npm run desktop:dev

# Command-line Node reference implementation
node ./bin/damaian-client.js index /path/to/repo
node ./bin/damaian-client.js git-status /path/to/repo
node ./bin/damaian-client.js classify-command "npm test"
```

## Assistant Command Sandbox

Chat providers do not receive direct shell access. If the assistant needs local facts that are not in the indexed file context, such as the latest Git commit or current uncommitted changes, it must request one command using Damaian's command envelope.

Sandbox-safe read-only commands, including `pwd`, `ls`, `git status`, `git diff`, `git log`, and `git show`, can run automatically in the selected working folder. Damaian sends the redacted command output back to the model so it can answer the original question.

Commands that cannot run in sandbox mode, such as validation scripts or commands with unknown side effects, require explicit user approval from an inline command card in the conversation. Policy-blocked destructive commands cannot be approved from the UI.

By default, Damaian stores global app data under:

```text
~/Library/Application Support/DamaianClient
```

Set `DAMAIAN_DATA_DIR` only when you want to override that location. For example, use `DAMAIAN_DATA_DIR=.damaian` to keep CLI audit and rollback data inside the current workspace during local development, or `DAMAIAN_DATA_DIR=~/.damaian` if you prefer a home-directory dotfolder.

Repository-scoped config is separate from the global data directory and is stored at `.damaian/config.conf` inside the selected repository.

Desktop settings can store the model API key in macOS Keychain. The user config file stores only a reference:

```text
model_api_key_env=keychain:model-api-key
```

Environment-variable references such as `model_api_key_env=OPENAI_API_KEY` remain supported for CLI and development workflows.

Repository config can still be provided manually at `.damaian/config.conf` and is reflected in the desktop Effective Policy view.

Do not put raw API keys in config files. `model_api_key_env` must be a Keychain reference or an environment variable name.

Example override:

```sh
DAMAIAN_DATA_DIR=~/.damaian npm run desktop:dev
```

## Build macOS DMG

Build the native macOS app and DMG installer with:

```sh
npm run desktop:build
```

The generated artifacts are written to paths like:

- `target/release/bundle/macos/Damaian.app`
- `target/release/bundle/dmg/Damaian_<version>_aarch64.dmg`

The developer-preview package is ad-hoc signed for bundle integrity but is not Developer ID signed or notarized. macOS may require the `Privacy & Security` `Open Anyway` flow described in [macOS Installation](docs/MACOS_INSTALLATION.md).

## Automatic Updates

Packaged desktop builds can check GitHub Releases for updates at startup. When a newer signed release is available, Damaian shows an `Update <version>` button in the conversation header. Selecting it downloads the update, verifies its Tauri updater signature, installs it, and restarts the app.

The updater uses this static manifest URL:

```text
https://github.com/damijanc/damaian/releases/latest/download/latest.json
```

The first installed build must already include the updater. Older DMGs built before this feature must be replaced manually once.

## GitHub macOS Release Build

The repository includes a GitHub Actions workflow at `.github/workflows/macos-dmg.yml`.

Before creating updater-capable releases, generate a Tauri updater signing key and add these GitHub repository secrets:

- `TAURI_UPDATER_PUBKEY`: The public key printed by the signer command. This is compiled into the app.
- `TAURI_UPDATER_PRIVATE_KEY`: The private key file contents. This is used only in GitHub Actions to sign updater artifacts.
- `TAURI_UPDATER_PRIVATE_KEY_PASSWORD`: Optional, only if the private key was generated with a password.

Example key generation:

```sh
cargo tauri signer generate -w ~/.tauri/damaian-updater.key
```

Keep the private key out of Git. If the private key is lost, existing updater-enabled clients cannot verify artifacts signed with a replacement key.

To build DMGs manually:

1. Open the repository on GitHub.
2. Go to `Actions`.
3. Select `Build macOS DMG`.
4. Select `Run workflow`.
5. Enter a version such as `0.1.3`; this stamps the app About dialog, DMG filename, and updater manifest.
6. Download the `Damaian-macOS-arm64-DMG` artifact from the completed run.

To create a GitHub Release with DMG assets, push a version tag:

```sh
git tag v0.1.0
git push origin v0.1.0
```

The workflow builds an ad-hoc signed Apple Silicon DMG, creates signed Tauri updater artifacts, then attaches all release assets to the GitHub Release for that tag. For tag builds, the workflow derives the app version from the tag. For example, tag `v0.1.3` produces app metadata version `0.1.3`, a `Damaian_0.1.3_aarch64.dmg` installer, a signed `Damaian.app.tar.gz` updater archive, its `.sig` file, and `latest.json`.

## User Documentation

- [Damaian User Guide](docs/USER_GUIDE.md)
- [macOS Installation](docs/MACOS_INSTALLATION.md)
- [Security Policy](SECURITY.md)

No Node.js runtime is required to run the packaged macOS app.

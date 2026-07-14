# macOS Installation

This document covers installing the current Damaian developer-preview build on macOS.

## System Requirements

- macOS 14 Sonoma or newer recommended.
- Git installed and available on the command line.
- Network access only when calling a configured model provider.
- A local Git repository you want Damaian to inspect.

## Install From DMG

Download the DMG from the GitHub Release page, or use a DMG you built locally.

1. Open the `Damaian_0.1.0_*.dmg` file.
2. Drag `Damaian.app` into `Applications`.
3. Eject the mounted disk image.
4. Open `Damaian` from `Applications`.

## Developer Preview Signing

The developer-preview package is ad-hoc signed for bundle integrity, but it is not Developer ID signed or notarized. On first launch, macOS may show a warning that the app cannot be opened because the developer cannot be verified.

For this developer-preview package:

1. Open `System Settings`.
2. Go to `Privacy & Security`.
3. Find the blocked `Damaian` launch message.
4. Select `Open Anyway`.
5. Confirm the launch prompt.

Only do this for builds you created yourself or received from a trusted source.

If macOS says the application is damaged, verify that you are using a release built after the ad-hoc signing fix. Older DMGs had an invalid bundle signature and should be replaced by a new release build.

## Selecting a Working Folder

Select `+` beside `Projects` to open the native macOS folder picker. Pick the local Git repository or working folder you want Damaian to inspect. The folder appears under `Projects`, and its chat sessions are grouped underneath it. Use the `+` beside a project folder to start a new session for that project.

Select the Visual Studio Code icon in the conversation header to open the selected working folder in Visual Studio Code. Damaian keeps AI orchestration and safety controls in the app while code navigation and IDE work happen in VS Code.

Select the terminal icon in the conversation header to open the bottom terminal panel. It starts in the selected working folder, or in your home directory when no folder is selected yet.

## Model API Key

The packaged desktop app can store model credentials in macOS Keychain from Settings.

1. Open Settings.
2. Configure `model_base_url` and `model_name`.
3. Enter a Keychain account name such as `model-api-key`.
4. Paste the API key into `API Key`.
5. Select `Save Key`.

Damaian stores the secret in Keychain and writes only a reference such as this to user config:

```text
model_api_key_env=keychain:model-api-key
```

Environment variables are still supported for CLI and development workflows. In that mode, set `model_api_key_env` to an environment variable name, such as `OPENAI_API_KEY`, before launching.

Repository config can still be provided manually at `.damaian/config.conf`. If repository config sets `model_api_key_env`, it overrides the user Keychain reference shown above.

Do not paste raw API keys into config files. `model_api_key_env` must be a Keychain reference or an environment variable name.

## Uninstall

1. Quit Damaian.
2. Delete `Damaian.app` from `Applications`.
3. Delete any local Damaian data directory you configured for testing, such as `.damaian` in a repository.

## Package Contents

The installer contains:

- `Damaian.app`: Native Tauri desktop wrapper.
- Local workspace engine: Repository indexing, file access, patch preview/apply, Git status, and audit logging.
- Embedded static UI: Chat, inline patch preview, terminal panel, settings, and Visual Studio Code handoff.

## Current Limitations

- The package is unsigned and not notarized.
- Repository selection uses a native folder picker from the Projects sidebar.
- The app uses a local in-process server on `127.0.0.1:4765` as the temporary bridge between Tauri and the workspace engine.

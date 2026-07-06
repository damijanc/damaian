# macOS Installation

This document covers installing the current Damaian developer-preview build on macOS.

## System Requirements

- macOS 14 Sonoma or newer recommended.
- Git installed and available on the command line.
- Network access only when calling a configured model provider.
- A local Git repository you want Damaian to inspect.

## Install From DMG

1. Open the `Damaian_0.1.0_*.dmg` file.
2. Drag `Damaian.app` into `Applications`.
3. Eject the mounted disk image.
4. Open `Damaian` from `Applications`.

## Unsigned Build Warning

The current package is not code signed or notarized. On first launch, macOS may show a warning that the app cannot be opened because the developer cannot be verified.

For this developer-preview package:

1. Open `System Settings`.
2. Go to `Privacy & Security`.
3. Find the blocked `Damaian` launch message.
4. Select `Open Anyway`.
5. Confirm the launch prompt.

Only do this for builds you created yourself or received from a trusted source.

## Selecting a Working Folder

Select `Choose` beside the `Repository` field to open the native macOS folder picker. Pick the local Git repository or working folder you want Damaian to inspect.

You can still enter an absolute path manually:

```text
/Users/your-name/development/my-project
```

Then select `Status` to confirm Damaian can inspect the repository.

Select the Visual Studio Code icon in the conversation header to open the selected working folder in Visual Studio Code. Damaian keeps AI orchestration and safety controls in the app while code navigation and IDE work happen in VS Code.

## Model API Key

The packaged preview reads model credentials from the environment variable configured by `model_api_key_env`. The default is usually `OPENAI_API_KEY`.

Desktop chat requires a configured model key. Launch Damaian from an environment where the key is available, or use the CLI mock-response mode for local engine testing while native Keychain storage is still pending.

## Uninstall

1. Quit Damaian.
2. Delete `Damaian.app` from `Applications`.
3. Delete any local Damaian data directory you configured for testing, such as `.damaian` in a repository.

## Package Contents

The installer contains:

- `Damaian.app`: Native Tauri desktop wrapper.
- Local workspace engine: Repository indexing, file access, patch preview/apply, command approval, Git status, and audit logging.
- Embedded static UI: Chat, edit preview, command approval, settings, and Visual Studio Code handoff.

## Current Limitations

- The package is unsigned and not notarized.
- Repository selection uses a native folder picker, with manual path entry still available as a fallback.
- Model API keys are not stored in Keychain yet.
- The app uses a local in-process server on `127.0.0.1:4765` as the temporary bridge between Tauri and the workspace engine.

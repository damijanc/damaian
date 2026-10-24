# macOS Installation

This document covers installing a released build of Damaian. To build it from
source instead, see [Development](DEVELOPMENT.md).

## System Requirements

- A Mac with Apple Silicon. Release builds are made for Apple Silicon only.
- macOS 14 Sonoma or newer.
- Git installed and available on the command line.
- An API key for a supported model provider (OpenAI, DeepSeek, OpenRouter,
  Ollama Cloud, or any OpenAI-compatible endpoint), or the Ollama app for local
  models.

## Install

1. Download `Damaian_<version>_aarch64.dmg` from the
   [latest release](https://github.com/damijanc/damaian/releases/latest).
2. Open the DMG and drag `Damaian.app` into `Applications`.
3. Eject the mounted disk image.
4. Open `Damaian` from `Applications`.

Release builds are Developer ID signed and notarized, so macOS opens them
without a Gatekeeper prompt. If macOS warns that the developer cannot be
verified, or says the app is damaged, do not work around it: download the DMG
again from the release page, and if the warning persists, report it as an
issue.

## First Steps

1. Open Settings with `⌘,` (or **Damaian › Settings...**). It opens on
   **Providers**.
2. Pick one of the **Popular providers**, or select **New** for another
   OpenAI-compatible endpoint. Under **Provider details**, paste your key into
   **API Key**; Damaian stores it in the macOS Keychain and keeps only a
   reference to it in its config. **Native tool-calling** is on by default;
   leave it on, because without it the assistant can answer questions but
   cannot propose file changes or use tools. Select **Save Provider**.
3. Close Settings, select `+` beside **Projects**, and pick the local Git
   repository you want to work in.
4. Ask a question in the prompt box.

The [User Guide](USER_GUIDE.md) covers everything from there.

## Updates

Damaian checks GitHub Releases for updates when it starts, and on demand from
**Damaian › Check for Updates...**. When a newer release is available, the
button at the bottom of the sidebar changes to `Update <version>`. Select it and
confirm **Install**: Damaian downloads the update, verifies its signature, and
installs it. Quit and reopen Damaian to finish.

## If Damaian Does Not Start

- **"refuses to start because 127.0.0.1:4765 is already in use"**: Damaian
  serves its interface on that local port, and another program, or another
  copy of Damaian, already holds it. Quit the other program and try again.
- **"Damaian cannot use its data directory"**: the data directory was written
  by a newer version of Damaian. Install the latest release.

[Troubleshooting](TROUBLESHOOTING.md) covers other failures.

## Uninstall

1. Quit Damaian.
2. Delete `Damaian.app` from `Applications`.
3. To remove your sessions, settings and audit log too, delete
   `~/Library/Application Support/DamaianClient`.
4. To remove stored API keys, open **Keychain Access** and delete the items
   whose name is `DamaianClient`.

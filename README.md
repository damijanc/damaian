# Damaian

A local-first AI coding assistant for macOS. Damaian works on a Git repository
on your machine: it answers questions about your code, previews every edit as a
diff before touching a file, and keeps file access, command execution, and your
API keys under your control.

![Damaian desktop app](./docs/screenshots/UI-screen.png)

> **Developer preview.** Damaian is signed, notarized, and usable for everyday
> local work, and it updates itself from GitHub Releases. It is still a preview:
> the feature set is moving, and configuration and stored data may change shape
> between releases.

## What you can do

- **Chat with your codebase.** Ask questions and get streamed answers. Each turn
  lists the repository files it read, and any file it mentions opens in your
  editor in one click.
- **Review edits before they land.** File-change requests come back as a diff
  preview. Apply or reject changes per file — or per individual hunk — and
  Damaian checks file hashes so it never overwrites work you changed after the
  preview.
- **Undo a whole turn.** Every turn leaves a checkpoint covering both the files
  it changed and your position in the conversation, so a wrong direction is one
  Rewind away rather than a manual unpick. Checkpoints are session recovery, not
  version control, and no substitute for a commit.
- **Stay in control of the terminal.** When the assistant needs a local fact, it
  can run read-only commands (like `git status` or `git log`) on its own.
  Anything that writes, installs, or reaches the network waits for your approval
  in the conversation, showing the full command before you decide. Approve once,
  or allow that command for the project. Docker is handled as its own
  approval-gated family, with only sandbox-safe read-only calls running
  unprompted.
- **Stop a run that is going wrong.** Long turns report what they are doing and
  how long they have been doing it, and stop on `Escape` or the Stop button.
- **Give the assistant project rules.** `AGENTS.md` files are read as scoped
  repository instructions, including nested ones for subdirectories.
- **Extend it with MCP servers.** Connect local (stdio) or remote (HTTP) Model
  Context Protocol servers in Settings, with per-server approval requirements.
- **Organize work by project.** A Projects sidebar groups chat sessions by
  folder, and your project list and last-used folder are remembered between
  launches.
- **Hand off to your editor.** Open the current folder in Visual Studio Code, or
  use the built-in bottom terminal panel, in one click.
- **Keep secrets safe.** Detected credentials are redacted from context, command
  output, and diffs, and your model API key is stored in the macOS Keychain.
- **Update in place.** When a newer signed release is available, an update button
  appears in the app.

## Requirements

- macOS 14 (Sonoma) or newer
- Git installed and on your `PATH`
- An API key for an OpenAI-compatible model provider (e.g. OpenAI, or a local
  provider such as Ollama)

No Node.js runtime is required to run the packaged app.

## Install

1. Download the latest `Damaian_<version>_aarch64.dmg` from the GitHub Releases
   page (Apple Silicon).
2. Open the DMG and drag `Damaian.app` into `Applications`.
3. Launch it. Builds are Developer ID signed and notarized, so macOS should open
   them without a Gatekeeper prompt — see
   [macOS Installation](docs/MACOS_INSTALLATION.md) if yours does not.

## Quick start

1. Open Damaian and select `+` beside **Projects** to pick a local Git
   repository.
2. Open **Settings** (`⌘,`), add your model provider details, and use the
   **Model API Key** controls to store your key in the Keychain.
3. Ask a question about the repository in the conversation, or request a change
   such as "add a test for the config parser" and review the diff preview.

The [Damaian User Guide](docs/USER_GUIDE.md) walks through each of these in
detail.

## Documentation

- [User Guide](docs/USER_GUIDE.md) — day-to-day usage, settings, model providers
- [macOS Installation](docs/MACOS_INSTALLATION.md) — install and first-launch steps
- [Troubleshooting](docs/TROUBLESHOOTING.md) — config and log locations, diagnosing failures
- [Security Policy](SECURITY.md) — safety model and vulnerability reporting
- [Development](docs/DEVELOPMENT.md) — building from source, the CLI, and releases
- [UI Style Guide](docs/UI_STYLE_GUIDE.md) — the desktop shell's visual language, with a rendered specimen page you open from a checkout
- [Feature specs](docs/specs/README.md) — what is built, what is planned, and in what order
- [AGENTS.md](AGENTS.md) — conventions and constraints for coding agents

## License

See [LICENSE](LICENSE).

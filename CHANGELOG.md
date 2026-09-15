# Changelog

Every release of Damaian, newest first, grouped by the part of the product each
change touched. Damaian is a local-first AI coding assistant for macOS — it
works on your own Git repository, on your machine, with your API key.

**Unreleased** lists work that has been specified but not built. Those entries
carry no version because nothing has shipped: a written specification is a
decision about what to build, not a date or a promise. They are ordered by
intended sequence. See [`docs/specs/`](docs/specs/README.md) for the full
specification behind each one.

A tag that introduced no commits is omitted: it recorded no change, so it is
not a release worth a row. `v0.8.0` is the one such tag so far.

The last row is the work before the first numbered release. It was briefly
tagged `v1.0.0` and `v1.1.0`; those tags were retracted on 2026-09-15 because a
1.0 states a stability promise Damaian has not made yet, and versioning
restarted at `v0.1.2` the day after they were cut. No release was ever published
from either tag, so only the numbers were withdrawn, never the work.

Components: **Engine** is the core (indexing, context, patches, commands,
policy, providers), **App** is the desktop application and its interface,
**CLI** is the `damaian` command-line tool, **Evaluation** is the offline test
harness, and **Release** covers packaging, signing and distribution.

## Unreleased

| | |
|---|---|
| **Next** | - Wait out provider rate limits and retry instead of ending the task, keeping the plan so work can resume.<br>- Report cached prompt tokens separately from uncached ones, so cost reflects what was actually billed.<br>- Four session modes — Ask, Plan, Code and Review — enforced in the engine, so a read-only session cannot write a file by any route.<br>- Turn failing tests, compiler errors, lint warnings and browser console errors into one kind of addressable finding you can click, filter and select for repair.<br>- Run your project's own checks, repair what failed, re-run, and report which results were verified rather than assumed.<br>- Let the agent ask a short clarifying question mid-task — where answering one authorises nothing.<br>- Give the agent ranged file reads, listing and content search, and region-bounded edits, so a large task fits in one sitting.<br>- Clean up processes left behind by a crash, matching both process id and start time so a stranger's process is never killed.<br>- Finish first-class browser diagnostics for troubleshooting a local web app *(in progress)*. |
| **After that** | - Map project roots so a command found in one package runs in that package, and monorepos stop being a special case.<br>- Find where a symbol is defined and what uses it, rather than searching for a word.<br>- Allocate context by category with a record of what was excluded and why.<br>- Show exactly what went into each request, with the ability to pin and exclude.<br>- Compact long conversations — keeping the objective, constraints, decisions and failed approaches — so a long task continues instead of stopping.<br>- Fetch a page the model names, under per-host approval, treated as reference material and never as instructions.<br>- Accept a pasted screenshot, and let the agent look at the screenshots its own browser diagnostics already capture.<br>- Search across past sessions and export one as a document.<br>- Remember facts about a project across sessions — each confirmed by you, visible, removable, and never authoritative over the code. |
| **Later** | - Permission profiles, lifecycle hooks, and per-tool MCP management, where nothing installed can widen what you allowed.<br>- Prepare a commit from exactly the changes you accepted, create a branch, and open a pull request — each its own explicit approval.<br>- Let another tool read your repository through Damaian, read-only, with the same redaction and path rules.<br>- Run several agents in parallel, only if measurement shows it beats a single agent. Abandoning it is a legitimate outcome. |

## Releases

<!-- releases -->

| | |
|---|---|
| **v0.34.0** | **Engine**<br>- Record durable task state and action markers, so a crash can be classified rather than guessed at.<br>- Classify what a crash left behind, and refuse to automatically resume an action whose outcome is unknown.<br>- Reattach a pending approval on restart, or fail the task with a stated reason.<br>- Load sessions written before durable task state and classify their running tasks.<br>- Assert recovery for every task state against every crash shape.<br><br>**App**<br>- After a crash, say what was interrupted and let you choose what happens to it.<br><br>**Evaluation**<br>- Measure crash recovery end to end, turning the recovery metric from a placeholder into a measured value. |
| **v0.33.0** | **Engine**<br>- Replay `reasoning_content` on every assistant turn, not only on native tool calls — required by providers that reject a thinking-mode turn without it.<br><br>**App**<br>- Move the composer controls out of the input into a slim action row, reclaiming the right gutter every line previously paid for.<br><br>**Evaluation**<br>- Add the evaluation harness: a deterministic tier that runs in CI, and a credential-gated live tier.<br>- Record the first reviewed metric baseline.<br>- Restrict a top-level `secrets/` directory, found while building the harness. |
| **v0.32.1** | **Release**<br>- Packaging fix. |
| **v0.32.0** | **App**<br>- Rewind the workspace and conversation to a previous checkpoint.<br>- Restructure command approval and visually demote persistent grants, so a one-off approval and an "always" are no longer identical.<br>- Size patch and settings actions to their content instead of stretching them across the column.<br>- Fold context files into the turn that read them, hide redundant role labels, and show the active folder and session in the thread header.<br>- Grow the composer from two rows instead of reserving four.<br>- Bring type onto a documented scale and slim the terminal chrome. |
| **v0.31.0** | **Release**<br>- Sign binaries with a Developer ID, notarize and staple them, so the app launches without a Gatekeeper workaround.<br><br>**Engine**<br>- Close a trust-boundary hole where repository configuration could override user configuration — a committed config could otherwise redirect the model endpoint or run its own shell for approved commands. |
| **v0.30.0** | **Engine**<br>- Support Docker as an approval-gated command family with its own risk messaging.<br><br>**App**<br>- Improve communication with the browser automation runner. |
| **v0.29.0** | **Engine, App**<br>- Improve web debugging and troubleshooting for local applications. |
| **v0.28.0** | **App**<br>- Remove files from the review list once their patch is accepted. |
| **v0.27.0** | **Engine**<br>- Improve `AGENTS.md` support: repository instructions load as part of the system prompt, with nested files resolved by scope. |
| **v0.26.0** | **Engine, App, CLI**<br>- Add "always allow" for a specific command, so the same approval stops being asked on every run. |
| **v0.25.0** | **Release**<br>- Gate releases on the Quality workflow, so a tagged build cannot publish while checks are red. |
| **v0.24.0** | **Engine, App**<br>- Interrupt a running conversation. |
| **v0.23.0** | **Engine, App, CLI**<br>- Fix the agent blocking legitimate changes. |
| **v0.22.0** | **Engine, App**<br>- Handle a prompt that requires creating a guardrail-protected file, and explain the outcome instead of failing quietly. |
| **v0.21.0** | **Engine, App, CLI, Release**<br>- Add `AGENTS.md` support and wire the quality checks into the pipeline. |
| **v0.20.0** | **App**<br>- Interface improvements. *(The commit records no detail beyond this.)* |
| **v0.19.0** | **Engine**<br>- Send reasoning context on every round, as some providers require. |
| **v0.18.0** | **Engine, App**<br>- Fix accepted patches not actually being applied. |
| **v0.17.0** | **Engine, App**<br>- Fix a silent file-creation failure.<br>- Add model-aware limits, configurable in the interface. |
| **v0.16.0** | **App**<br>- Integrate a real terminal. |
| **v0.15.0** | **Release**<br>- Generate release notes from commit messages rather than baked-in text. |
| **v0.14.0** | **Engine, App**<br>- Improve MCP configuration, with a switch to enable external tool calls. |
| **v0.13.0** | **Engine, App**<br>- Add MCP support. |
| **v0.12.0** | **Engine, App, CLI**<br>- Apply patches at hunk level rather than whole files.<br>- Make file references in messages clickable. |
| **v0.11.0** | **Engine, App**<br>- Switch tool calling to the provider's native API. |
| **v0.10.1** | **App**<br>- Fix renaming the project. |
| **v0.10.0** | **Engine, App, CLI**<br>- Interface improvements. *(The commits record no detail beyond this.)* |
| **v0.9.0** | **Engine, App, CLI**<br>- Allow adding files from outside the repository, chosen by the user. |
| **v0.7.0** | **App**<br>- Add drag-and-drop. |
| **v0.6.0** | **App**<br>- Improve adding files to the context. |
| **v0.5.0** | **Engine**<br>- Allow chained tool calls.<br>- Stream model responses. |
| **v0.4.0** | A security-focused release alongside the feature work.<br><br>**Engine**<br>- Keep the model API key out of the `curl` argument list.<br>- Redact secrets from diffs and from patch rollback snapshots.<br>- Expand secret scanner coverage.<br>- Reject symlinked write targets that resolve outside the repository.<br>- Detect line breaks in shell commands, and prevent an allowlist bypass using shell control characters.<br><br>**App**<br>- Deliver the desktop API token over Tauri IPC instead of exposing it at bootstrap.<br>- Scope the desktop shell origin.<br><br>**Engine, App, CLI**<br>- Add patch rollback, hunk-level diffs, incremental indexing, and native tool calling.<br><br>**Release**<br>- Remove the Node reference implementation. |
| **v0.3.1** | **App**<br>- Fix auto-update. |
| **v0.3.0** | **Engine, App, CLI**<br>- Support multiple LLM providers, with model selection. |
| **v0.2.1** | **Release**<br>- Build fix. |
| **v0.2.0** | **App, Release**<br>- Add auto-update. |
| **v0.1.4** | **Release**<br>- Stamp the macOS release version. |
| **v0.1.3** | **Release**<br>- macOS build fix. |
| **v0.1.2** | **Release**<br>- Update the GitHub build. |
| **Initial development** | **Engine, App, CLI**<br>- First implementation: chat with repository context, diff preview, command execution, and the CLI.<br>- Response streaming and context files.<br>- The tool-call chain, and toolchain support in development mode.<br>- Keychain integration for API keys.<br>- Configuration editor, native folder picker, and VS Code integration.<br>- Terminal support.<br><br>**Release**<br>- macOS build and packaging. |

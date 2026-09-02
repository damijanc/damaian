# AGENTS.md

Instructions for coding agents working in this repository. Human contributors
should read [docs/DEVELOPMENT.md](docs/DEVELOPMENT.md) first; this file covers
the parts that are easy for an agent to get wrong.

## What this project is

Damaian is a local-first AI coding assistant for macOS: a Rust workspace with a
native Tauri desktop app. It operates on the user's own Git repository, previews
every file edit as a diff before applying it, and keeps file access, command
execution, and API keys under the user's control.

Two constraints follow from that and shape most decisions:

- **macOS only.** The `desktop-app` crate builds against the macOS system
  webview. Do not add Linux or Windows support paths unless asked.
- **No Node.js at runtime.** Node is used only for build, release, and lint
  scripts. Never introduce a Node dependency into the shipped application.

## Workspace layout

| Crate | Purpose |
|-------|---------|
| `crates/workspace-engine` | Core services: indexing, context assembly, secret redaction, patch engine, command policy, model adapters, audit log. Most logic lives here. |
| `crates/damaian-cli` | Command-line front end over the workspace engine (binary: `damaian`). |
| `crates/desktop-shell` | Local HTTP shell and web UI on `127.0.0.1:4765` (binary: `damaian-desktop-shell`). |
| `crates/desktop-app` | Native Tauri wrapper: folder picker, Keychain, updater, PTY terminal (binary: `damaian-desktop`). |

The web UI is vanilla JavaScript and CSS in `crates/desktop-shell/static/` —
no framework, no bundler, no build step.

## Before you change a feature

`docs/specs/` is the source of truth for the implemented features, and each
spec carries a `Status:` line. Read the relevant spec before changing behaviour
it covers, and update the spec when you change the design — not just the code.
`docs/specs/README.md` lists them in implementation order.

The two root documents `ai_coding_assistant_specification.md` and
`ai_coding_assistant_must_have.md` are the original product spec. Treat them as
background: they describe intent, and the `docs/specs/` files describe what was
actually built.

## Quality gate

CI (`.github/workflows/quality.yml`) runs exactly these. Run them before you
claim work is done — all five must pass:

```sh
cargo fmt --all -- --check
cargo clippy --workspace --all-targets --locked -- -D warnings
cargo test --workspace --locked
npm run lint:web
cargo deny check
```

Notes:

- **Clippy warnings are errors.** Fix them rather than suppressing them. Where a
  lint is genuinely wrong for the code, `#[allow(...)]` with a comment
  explaining why is acceptable — see the `too_many_arguments` allowances on the
  dependency-injection constructors.
- `npm run lint:web:fix` auto-fixes most web-asset findings.
- `npm ci` first if `node_modules` is missing. Biome is pinned in
  `package-lock.json`; do not bump it as a side effect of another change.
- `cargo deny` is scoped to `aarch64-apple-darwin` in `deny.toml`. If you add a
  dependency with a new license, add it to the allow-list there with a reason.

## Testing

- 265 tests pass by default: mostly inline `#[test]` modules, plus integration
  tests in `crates/workspace-engine/tests/`. Add tests next to the code you
  change.
- Two tests are `#[ignore]`d because they have real side effects (one opens
  Finder, one spawns a real login shell). Keep that convention: anything that
  touches the user's desktop or spawns a shell should be `#[ignore]`d with a doc
  comment saying how to run it manually.
- `DAMAIAN_MOCK_MODEL_RESPONSE="..."` makes model-dependent paths testable with
  no API key and no network. Prefer it over mocking HTTP.
- `DAMAIAN_DATA_DIR=.damaian` keeps app data inside the workspace instead of
  `~/Library/Application Support/DamaianClient`. Use it so you never write to
  the user's real data while testing.

## Security boundaries

These are product guarantees, not implementation details. Do not weaken them to
make something easier:

- **Secret redaction** removes detected credentials from model context, command
  output, and diffs. Never add a path that bypasses the scanner.
- **Command policy** decides what runs without approval. Read-only commands may
  run automatically; anything else requires explicit user approval. Do not widen
  the allowlist or downgrade a risk classification without a spec change.
- **API keys** live in the macOS Keychain. `model_api_key_env` holds a Keychain
  reference (`keychain:model-api-key`) or an environment variable name — never a
  raw key. Never write a raw key into config, code, tests, or logs.
- **Patch application** verifies file hashes so it cannot overwrite work the user
  changed after the preview was generated. Keep that check.
- **Repository config is untrusted input.** `<repo>/.damaian/config.conf` arrives
  with a clone, so it may add restrictions and never remove one. A repository
  cannot set `shell`, `data_dir`, `allowed_roots`, `secret_patterns`,
  `audit_enabled`, `block_generated_secrets`, any `model_*` key, or
  `command_allowlist`. `Config::apply_overlay_scoped` classifies every overlay
  field in one exhaustive destructuring, so a field added to `ConfigOverlay`
  without being classified fails to compile — keep it that way rather than
  adding a catch-all. The same rule already applies to `AGENTS.md`, which
  cannot widen a working mode.

See [SECURITY.md](SECURITY.md) for the full model.

## When something is broken

[docs/TROUBLESHOOTING.md](docs/TROUBLESHOOTING.md) is the diagnostic reference:
where config, sessions, patches, and the audit log are written, what is and is
not logged (there is no log file — only stdout/stderr), how to read the audit
trail, and how to reproduce a failure with the CLI instead of the UI. Read it
before you start instrumenting code to find out what happened.

## Traps

Things that have actually cost time here:

- **Never `pkill -f damaian-desktop-shell`** (or match any Damaian binary by
  name) to clean up a process you started. The user's own running app shares
  those names, and you will kill their session. Track the PID you spawned and
  kill that.
- **A packaged build refusing to launch is expected, not a bug.** The developer
  preview is ad-hoc signed, not Developer ID signed or notarized, so macOS
  Gatekeeper blocks first launch. The workaround is in
  [docs/MACOS_INSTALLATION.md](docs/MACOS_INSTALLATION.md). Do not "fix" it.
- **`crates/desktop-shell/static/xterm*` is vendored and minified.** It is
  excluded from linting and formatting. Never edit or reformat it.
- The desktop shell defaults to port `4765`. If that port is already in use, the
  user's app is probably running — do not assume it is stale and do not take the
  port. Pass `--port` to run your own instance somewhere else, and set
  `DAMAIAN_DATA_DIR` so it does not share their data.

## Style

- Rust 2024 edition, default `rustfmt`. No custom formatting config, so just run
  `cargo fmt --all`.
- Web assets are formatted and linted by Biome (`biome.json`): 2-space indent,
  100-column lines.
- Match the surrounding code. Comments in this codebase explain *why* something
  is done, not what the line does — follow that.
- Keep commit messages short and descriptive of the user-visible effect.

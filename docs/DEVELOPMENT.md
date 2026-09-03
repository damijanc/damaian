# Development

Technical reference for building, running, and releasing Damaian. For end-user
instructions see the [Damaian User Guide](USER_GUIDE.md); for diagnosing a
misbehaving build see [Troubleshooting](TROUBLESHOOTING.md).

Damaian is a Rust workspace with a native Tauri desktop app. No Node.js runtime
is required to run the packaged macOS app; Node is only used for the build,
release, and lint scripts below.

If you are working with a coding agent, point it at [AGENTS.md](../AGENTS.md) —
it records the conventions, security boundaries, and pitfalls specific to this
repository.

## Workspace layout

| Crate | Purpose |
|-------|---------|
| `crates/workspace-engine` | Core services: indexing, context assembly, secret redaction, patch engine, command policy, model adapters, audit log. |
| `crates/damaian-cli` | Command-line front end over the workspace engine. |
| `crates/desktop-shell` | Local HTTP shell and web UI served on `127.0.0.1:4765`. |
| `crates/desktop-app` | Native Tauri wrapper (macOS folder picker, Keychain, updater). |

## Run locally

```sh
# Run the test suite
cargo test

# Native Tauri desktop app (development)
DAMAIAN_REPO=/path/to/repo npm run desktop:dev

# Local desktop shell prototype (no Tauri wrapper)
cargo run -p desktop-shell -- --repo /path/to/repo --port 4765
```

## Quality checks

Every pull request and every push to `main` runs
[`.github/workflows/quality.yml`](../.github/workflows/quality.yml). The same
checks run locally:

```sh
cargo fmt --all -- --check                                   # formatting
cargo clippy --workspace --all-targets --locked -- -D warnings  # lints (warnings fail)
cargo test --workspace --locked                              # test suite
npm run lint:web                                             # app.js / style.css
cargo deny check                                             # advisories + licenses
```

`npm run lint:web:fix` applies the web-asset fixes automatically. `npm ci` first
if you have not installed the Node dev dependencies; Biome is the only one, and
it is pinned in `package-lock.json`.

Two tools are needed only for the last check and are installed on demand in CI:

```sh
cargo install cargo-deny --locked
cargo install typos-cli --locked   # spell check, also run in CI
```

Configuration lives in [`deny.toml`](../deny.toml) (scoped to
`aarch64-apple-darwin`, the only shipped target), [`biome.json`](../biome.json),
and [`_typos.toml`](../_typos.toml). The vendored `xterm*` files under
`crates/desktop-shell/static/` are excluded from all of it.

The workflow splits into a fast Linux job for the checks that do not compile
Rust and a macOS job for Clippy and tests, since `desktop-app` only builds
against the macOS webview.

## CLI

The `damaian-cli` crate exposes the workspace engine on the command line:

```sh
cargo run -p damaian-cli -- config-show /path/to/repo
cargo run -p damaian-cli -- config-set user command_allowlist "npm test|cargo test"
cargo run -p damaian-cli -- config-set repo /path/to/repo restricted_patterns ".env|*.pem|private/**"
cargo run -p damaian-cli -- config-set admin audit_retention_days 30
cargo run -p damaian-cli -- propose-command /path/to/repo "npm test"
cargo run -p damaian-cli -- propose-validations /path/to/repo
cargo run -p damaian-cli -- run-command command_proposal_id --approve
cargo run -p damaian-cli -- run-command command_proposal_id --approve --always
cargo run -p damaian-cli -- reject-command command_proposal_id
DAMAIAN_MOCK_MODEL_RESPONSE="Mock answer" cargo run -p damaian-cli -- ask /path/to/repo "What does auth do?"
OPENAI_API_KEY=... cargo run -p damaian-cli -- ask /path/to/repo "Explain the project"
cargo run -p damaian-cli -- propose-edit /path/to/repo "Make the change"
cargo run -p damaian-cli -- apply-patch /path/to/repo patch_id_from_preview
cargo run -p damaian-cli -- reject-patch patch_id_from_preview
```

## Data directories

Global app data (audit records, sessions, patch proposals) is stored under:

```text
~/Library/Application Support/DamaianClient
```

Set `DAMAIAN_DATA_DIR` to override that location. This is useful during
development to keep data inside the current workspace:

```sh
DAMAIAN_DATA_DIR=.damaian npm run desktop:dev   # inside the current workspace
DAMAIAN_DATA_DIR=~/.damaian npm run desktop:dev # home-directory dotfolder
```

Repository-scoped config is separate from the global data directory and lives at
`.damaian/config.conf` inside the selected repository. It is reflected in the
desktop **Effective Policy** view.

Do not put raw API keys in config files. `model_api_key_env` must be a Keychain
reference (`keychain:model-api-key`) or the name of an environment variable.

## Build a macOS DMG

```sh
npm run desktop:build:preview
```

Artifacts are written to:

- `target/release/bundle/macos/Damaian.app`
- `target/release/bundle/dmg/Damaian_<version>_aarch64.dmg`

The developer-preview package is ad-hoc signed for bundle integrity but is not
Developer ID signed or notarized. See [macOS Installation](MACOS_INSTALLATION.md)
for the first-launch steps macOS requires.

`npm run desktop:build` is the stable path and does **not** default the signing
identity. With no `APPLE_SIGNING_IDENTITY` in the environment it produces an
unsigned build rather than quietly falling back to an ad-hoc signature, so a
release that is missing its credentials fails instead of shipping something that
looks signed and is not.

## Automatic updates

Packaged builds check GitHub Releases at startup and offer an in-app update when
a newer signed release is available. The updater reads a static manifest:

```text
https://github.com/damijanc/damaian/releases/latest/download/latest.json
```

Updates are verified against the app's compiled-in Tauri updater public key. The
first installed build must already include the updater; older DMGs built before
this feature must be replaced manually once.

## GitHub macOS release build

The workflow at `.github/workflows/macos-dmg.yml` builds Apple Silicon DMGs and
signed Tauri updater artifacts.

### Release channels

The channel follows the trigger, not a repository setting:

| Trigger | Channel | Signing | Publishes |
|---|---|---|---|
| Tag push `v*` | `stable` | Developer ID, notarized, stapled, verified — or the job fails | DMG, `latest.json`, GitHub Release |
| `workflow_dispatch`, `channel: stable` | `stable` | Same as above | Workflow artifact only |
| `workflow_dispatch` (default) | `preview` | Ad-hoc | DMG, `preview.json`; never a Release |
| Pull request / branch CI | n/a | n/a | Nothing; Quality only |

Only the stable channel writes `latest.json`, which is the file the shipped
updater endpoint reads. A preview install is therefore never offered a stable
update it did not opt into, and a stable install is never offered a preview
build. The channel is compiled in with `DAMAIAN_RELEASE_CHANNEL` and shown next
to the version in the About panel.

### Apple signing credentials

A stable build needs these repository secrets. The job checks that every one is
present before it starts building, and names any that are missing. Values are
passed through `env:` only, so GitHub's log masking applies.

| Secret | Purpose |
|---|---|
| `APPLE_CERTIFICATE` | Base64-encoded Developer ID Application `.p12` |
| `APPLE_CERTIFICATE_PASSWORD` | Password for that `.p12` |
| `APPLE_SIGNING_IDENTITY` | Full identity, e.g. `Developer ID Application: Name (TEAMID)` |
| `APPLE_TEAM_ID` | Apple Developer team identifier |
| `APPLE_API_ISSUER` | App Store Connect API issuer UUID |
| `APPLE_API_KEY` | App Store Connect API key ID |
| `APPLE_API_KEY_CONTENT` | Base64-encoded `.p8` private key |

`APPLE_API_KEY_PATH` is not a secret. The workflow decodes `APPLE_API_KEY_CONTENT`
into the runner's temp directory, points that variable at it, and deletes the
file in an `if: always()` step so a cancelled job leaves no key behind.

App Store Connect API-key authentication is used rather than an Apple ID and
app-specific password because it does not tie releases to one person's Apple ID
and survives that person changing their two-factor settings.

A stable build fails closed: if signing, notarization, stapling, or verification
fails, or a credential is missing, the job stops before any artifact is uploaded
and before any Release is created. Verification is not nominal — it asserts a
`Developer ID Application` signing authority, the hardened-runtime flag,
`stapler validate` on both the `.app` and the `.dmg`, and `spctl` acceptance of
both.

### Updater signing keys

Before creating updater-capable releases, generate a signing key:

```sh
cargo tauri signer generate -w ~/.tauri/damaian-updater.key
```

Add these GitHub repository secrets:

- `TAURI_UPDATER_PUBKEY` — public key printed by the signer command; compiled into the app.
- `TAURI_UPDATER_PRIVATE_KEY` — private key file contents; used only in CI to sign updater artifacts.
- `TAURI_UPDATER_PRIVATE_KEY_PASSWORD` — optional, only if the key was generated with a password.

Keep the private key out of Git. If it is lost, existing updater-enabled clients
cannot verify artifacts signed with a replacement key.

### Manual workflow run

1. Open the repository on GitHub → **Actions**.
2. Select **Build macOS DMG** → **Run workflow**.
3. Enter a version such as `0.1.3`. This stamps the About dialog, DMG filename, and updater manifest.
4. Choose a channel. `preview` needs no Apple credentials; `stable` exercises the full signing and notarization path without creating a Release.
5. Download the `Damaian-macOS-arm64-<channel>-DMG` artifact from the completed run.

A preview run with no updater secrets configured still produces a DMG — the
updater artifacts are dropped rather than the build, so a fork is not blocked.

### Tag-triggered release

Push a version tag to build and attach release assets automatically:

```sh
git tag v0.1.0
git push origin v0.1.0
```

The workflow derives the app version from the tag. For example, tag `v0.1.3`
produces version `0.1.3`, a `Damaian_0.1.3_aarch64.dmg` installer, a signed
`Damaian.app.tar.gz` updater archive with its `.sig` file, and `latest.json`.

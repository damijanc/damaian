# Feature Spec: Developer ID Signing and Notarization

Status: Implemented; pending the validation runs in §7.
Order: 14 of 15
Roadmap: `docs/ROADMAP/00b_phase_0_distributable_build.md`, Phase 0, Work
Package 1 (Must). That directory is local-only and not committed, so the
reference is a name rather than a link; this spec is self-contained.
Related spec sections: none in `ai_coding_assistant_specification.md`. This is
release engineering, not product behaviour. Related implementation specs:
[`09_release_quality_gate.md`](09_release_quality_gate.md) (the Quality gate this
pipeline already depends on) and
[`15_install_and_update_verification.md`](15_install_and_update_verification.md)
(the verification half of the same phase).

## 1. Motivation

Damaian's packaged build is ad-hoc signed. `.github/workflows/macos-dmg.yml:116`
sets `APPLE_SIGNING_IDENTITY: "-"`, so macOS Gatekeeper blocks first launch on
any machine other than the build machine, and
[`docs/MACOS_INSTALLATION.md`](../MACOS_INSTALLATION.md) documents a manual
`Open Anyway` / `xattr -dr com.apple.quarantine` workaround. `AGENTS.md` records
this as expected rather than a bug — correctly, for a developer preview.

It stops being acceptable the moment the roadmap depends on user feedback. Every
later phase is justified by claims about what users need — "dependable enough for
daily use", "trusted daily driver", "usability". None of those claims can be
tested while installing Damaian requires a terminal command and a Gatekeeper
override.

A secondary defect compounds it. Ad-hoc signing churns the code-signing hash on
every build, which invalidates the Keychain item's partition list and produces
two password prompts per launch. `crates/desktop-shell/src/keychain.rs` is
correct; the unstable signing identity is the cause.

The current pipeline is also silently permissive rather than fail-closed. The
`desktop:build` npm script defaults `APPLE_SIGNING_IDENTITY` to `-` when the
variable is unset, and `cargo tauri build` skips notarization without error when
Apple credentials are absent. A tagged release with a missing or expired secret
therefore publishes an unsigned artifact and reports success.

## 2. Current State

- `.github/workflows/macos-dmg.yml:116` hardcodes `APPLE_SIGNING_IDENTITY: "-"`
  in the `Build DMG` step. No Apple certificate, App Store Connect key, team ID,
  or notarization credential is passed to the build.
- `package.json` `desktop:build` runs
  `APPLE_SIGNING_IDENTITY="${APPLE_SIGNING_IDENTITY:--}" cargo tauri build`.
  The `:-` default means an unset variable silently produces an ad-hoc signature
  instead of failing.
- `.github/workflows/macos-dmg.yml:122-125` (`Verify macOS bundle`) runs
  `codesign --verify --deep --strict` and `hdiutil verify`. Both pass for ad-hoc
  signatures, so the step proves bundle integrity and nothing about
  distributability.
- There is no notarization step, no stapling step, and no `spctl` assessment.
- `crates/desktop-app/tauri.conf.json` has no `bundle.macOS` section: no
  entitlements file, no `minimumSystemVersion`, no hardened-runtime
  configuration. Tauri enables the hardened runtime automatically when it signs
  with a real identity, but nothing verifies that it did.
- `scripts/enable-updater-artifacts.mjs` writes `bundle.createUpdaterArtifacts`
  and the updater `pubkey` and `endpoints` into `tauri.conf.json` at build time.
  The endpoint is a single hardcoded URL
  (`https://github.com/damijanc/damaian/releases/latest/download/latest.json`).
- `scripts/create-updater-manifest.mjs` reads the `.sig` file Tauri produced and
  writes `target/release/bundle/updater/latest.json`. It never verifies that the
  signature validates against `TAURI_UPDATER_PUBKEY`.
- `scripts/sync-version.mjs` stamps the version into `package.json`,
  `crates/desktop-app/tauri.conf.json`, and `Cargo.toml`.
- There is exactly one release channel. Nothing in the artifact, the updater
  manifest, or the app distinguishes a preview build from a stable one.
- `crates/desktop-app/src/main.rs:176-183` builds the macOS About panel from
  `AboutMetadata { name, version }`. It shows a version number with no channel.
- [Spec 09](09_release_quality_gate.md) already makes the Quality workflow a
  required dependency of the release job (`.github/workflows/macos-dmg.yml:22-28`).
  That gate covers code quality, not artifact distributability.

## 3. Requirements

1. Stable releases are signed with a Developer ID Application certificate
   supplied through CI secrets, imported into a temporary keychain that is
   created and destroyed within the job. No certificate, password, App Store
   Connect key, or team identifier is committed to the repository.
2. The `.app` bundle is notarized through `notarytool` with the hardened runtime
   enabled, and the notarization ticket is stapled to both the `.app` and the
   `.dmg`.
3. The stable release path fails closed. If any of signing, notarization,
   stapling, or verification fails — or if a required credential is missing,
   empty, or invalid — the job fails before any artifact is uploaded and before
   any GitHub Release is created or edited.
4. Verification is real, not nominal. After stapling, the workflow asserts that
   the signing authority is a Developer ID Application certificate (not ad-hoc),
   that the hardened runtime flag is set, that `spctl --assess` accepts both the
   app and the DMG, and that `stapler validate` passes for both. Any rejection
   is a build failure.
5. Updater artifacts are signed, and the workflow verifies the signature it just
   produced against the configured public key before publishing.
6. The developer-preview path is preserved. A build without signing credentials
   still produces an ad-hoc DMG, clearly labelled as a preview, so contributors
   and forks are not blocked. A preview build never publishes to the stable
   updater channel.
7. Stable and preview builds are distinguishable in the artifact name, in the
   updater manifest, and in the application's About panel.
8. Release prerequisites are documented for both local and CI runs, naming every
   required secret without recording any value.
9. No secret value appears in workflow logs, in the repository, in the built
   artifact, or in the updater manifest.
10. Application behaviour is unchanged apart from the channel label in the About
    panel and the channel-scoped updater endpoint.

## 4. Non-goals

- Mac App Store distribution.
- Sandboxing, or hardened-runtime entitlement changes beyond what notarization
  requires.
- Auto-update UX redesign. The existing `Check for Updates...` menu item and
  header button keep their current behaviour.
- Homebrew, MacPorts, or any third-party distribution channel.
- Intel (`x86_64`) or universal builds. The pipeline stays Apple Silicon only.
- Linux or Windows packaging.
- A general-purpose feature-flag or channel-management system. Channel is a
  build-time property, not a runtime setting.
- Adding a Node.js dependency to the shipped application. Node stays build-time
  only, and the new verification script uses only the Node standard library.
- Automatic certificate renewal or secret rotation tooling.

## 5. Design

### 5.1 Release channels

Channel is decided by the workflow trigger, not by a repository setting:

| Trigger | Channel | Signing | Publishes |
|---|---|---|---|
| Tag push `v*` | `stable` | Developer ID, notarized, stapled, verified — or the job fails | Stable DMG, `latest.json`, GitHub Release |
| `workflow_dispatch` with `channel: stable` | `stable` | Same as above | Workflow artifact only; no GitHub Release |
| `workflow_dispatch` (default) | `preview` | Developer ID if credentials are present, otherwise ad-hoc | Preview DMG, `preview.json`; never `latest.json` |
| Pull request / branch CI | n/a | n/a | Nothing. Quality runs; no packaging |

Add a `channel` input to the `workflow_dispatch` trigger (`stable` | `preview`,
default `preview`) and resolve `RELEASE_CHANNEL` in the existing
`Stamp release version` step alongside `RELEASE_VERSION` and `RELEASE_TAG`.

The stable channel is the only one that may write `latest.json`, which is the
file the shipped updater endpoint points at. Preview builds write `preview.json`
and configure the preview endpoint, so a preview install can never be offered a
stable update it did not opt into, and a stable install is never offered a
preview build.

### 5.2 Credentials

Required repository secrets for a stable build. Values are never echoed, never
written to a file that is uploaded, and never interpolated into a `run:` script
body — they are passed through `env:` only, so GitHub's log masking applies:

| Secret | Purpose |
|---|---|
| `APPLE_CERTIFICATE` | Base64-encoded Developer ID Application `.p12` |
| `APPLE_CERTIFICATE_PASSWORD` | Password for that `.p12` |
| `APPLE_SIGNING_IDENTITY` | Full identity string, e.g. `Developer ID Application: Name (TEAMID)` |
| `APPLE_TEAM_ID` | Apple Developer team identifier |
| `APPLE_API_ISSUER` | App Store Connect API issuer UUID (notarization) |
| `APPLE_API_KEY` | App Store Connect API key ID (notarization) |
| `APPLE_API_KEY_PATH` | Written from `APPLE_API_KEY_CONTENT` into the job's runner temp directory, never into the workspace |
| `APPLE_API_KEY_CONTENT` | Base64-encoded `.p8` private key |

Already present and unchanged: `TAURI_UPDATER_PUBKEY`,
`TAURI_UPDATER_PRIVATE_KEY`, `TAURI_UPDATER_PRIVATE_KEY_PASSWORD`.

App Store Connect API-key authentication is chosen over Apple ID plus
app-specific password because it does not tie the release pipeline to one
person's Apple ID and it survives that person enabling or changing two-factor
settings.

The temporary keychain is Tauri's: `cargo tauri build` creates one, imports
`APPLE_CERTIFICATE`, and deletes it when the build finishes. Do not hand-roll a
second `security create-keychain` flow. Add an `if: always()` cleanup step that
removes the `.p8` file from the runner temp directory, so a cancelled or failed
job does not leave a key on a self-hosted runner.

### 5.3 Preflight gate

Tauri skips signing and notarization silently when credentials are absent, and
`desktop:build` turns an unset `APPLE_SIGNING_IDENTITY` into `-`. A stable build
must therefore assert its inputs before it starts, rather than discovering the
problem at verification time — or not at all.

Add a `Check release credentials` step that runs when `RELEASE_CHANNEL == stable`
and, for each required secret, fails with a message naming the missing secret if
the value is empty. It checks presence only; it never prints a value or a length.

Change `desktop:build` in `package.json` to stop defaulting the identity:

```json
"desktop:build": "cd crates/desktop-app && cargo tauri build",
"desktop:build:preview": "cd crates/desktop-app && APPLE_SIGNING_IDENTITY=- cargo tauri build"
```

A local developer build with no `APPLE_SIGNING_IDENTITY` set is unsigned rather
than ad-hoc signed, which is fine for `desktop:dev`. The preview release path
uses `desktop:build:preview` and gets the ad-hoc signature it needs for bundle
integrity. The stable path uses `desktop:build` with the identity supplied from
secrets, so a missing identity is a build failure instead of a silent downgrade.

### 5.4 Notarization and stapling

Tauri notarizes the `.app` and staples the ticket to it, then builds the DMG from
the stapled app. The DMG itself is not notarized or stapled by Tauri, and an
unstapled DMG shows a Gatekeeper dialog on a machine that is offline or behind a
network that blocks Apple's OCSP responder. The workflow therefore submits the
DMG separately after the build:

```sh
xcrun notarytool submit "$DMG_PATH" \
  --key "$APPLE_API_KEY_PATH" \
  --key-id "$APPLE_API_KEY" \
  --issuer "$APPLE_API_ISSUER" \
  --wait
xcrun stapler staple "$DMG_PATH"
```

`notarytool submit --wait` returns a non-zero exit status when the submission is
rejected, and `set -euo pipefail` turns that into a job failure. On rejection the
step fetches and prints the notarization log
(`xcrun notarytool log <submission-id>`), which contains no secrets, so the
failure is diagnosable without re-running.

### 5.5 Verification gate

Replace the current `Verify macOS bundle` step. For a stable build every command
below must exit 0, and the step runs before the artifact upload:

```sh
set -euo pipefail
APP_PATH=target/release/bundle/macos/Damaian.app
DMG_PATH="$(find target/release/bundle/dmg -name '*.dmg' -print -quit)"

# Signature is a real Developer ID identity, not ad-hoc.
codesign -dvvv "$APP_PATH" 2>&1 | grep -q '^Authority=Developer ID Application'

# Hardened runtime is on; notarization requires it.
codesign -d --verbose "$APP_PATH" 2>&1 | grep -q 'flags=.*runtime'

codesign --verify --deep --strict --verbose=4 "$APP_PATH"
hdiutil verify "$DMG_PATH"

xcrun stapler validate "$APP_PATH"
xcrun stapler validate "$DMG_PATH"

spctl --assess --type execute --verbose=4 "$APP_PATH"
spctl --assess --type open --context context:primary-signature --verbose=4 "$DMG_PATH"
```

The `Authority=Developer ID Application` assertion is the check that makes the
gate meaningful: it is the one condition an ad-hoc signature cannot satisfy, and
its absence is exactly the failure mode that shipped today's artifacts.

For a preview build the step asserts bundle integrity only (`codesign --verify`
and `hdiutil verify`), skips the Developer ID, stapler, and `spctl` assertions,
and prints a line stating that this is an unsigned preview artifact.

### 5.6 Updater artifact verification

`scripts/create-updater-manifest.mjs` copies the `.sig` file into the manifest
without checking it. Add `scripts/verify-updater-signature.mjs`, run immediately
after the manifest is created and before upload, which verifies the signature
against `TAURI_UPDATER_PUBKEY`:

- Both the public key and the signature are minisign blobs, base64-encoded as a
  whole. Decode, then read the inner lines: comment, payload, and — for a
  signature — a `trusted comment:` line and a second base64 payload.
- The public-key payload is a 2-byte algorithm tag, an 8-byte key ID, and a
  32-byte Ed25519 public key (42 bytes). The signature payload is the same tag
  and key ID followed by a 64-byte signature (74 bytes).
- **The two algorithm tags are not the same value, and must not be compared.**
  A minisign public key is always tagged `Ed`. A signature is tagged `Ed` when it
  signs the file bytes and `ED` when it signs a BLAKE2b-512 hash of them, and
  the Tauri CLI pinned in this workflow (2.11.4) emits `ED`. Verified against
  real `cargo tauri signer generate` and `cargo tauri signer sign` output: the
  public key reads `Ed`, the signature reads `ED`. An implementation that
  requires the tags to match, or that assumes the raw-bytes case, rejects every
  genuine signature. Accept both tags and let the *signature's* tag select
  whether to hash first; `node:crypto` supports `blake2b512`.
- Fail if the key IDs do not match. That means the release was signed with a
  different private key than the one shipped in the app, which would brick the
  updater for every existing install — it must fail the build, not reach users.
- Verify with `node:crypto`'s Ed25519 support, wrapping the raw 32-byte key in
  the fixed SPKI DER prefix `302a300506032b6570032100`. No third-party
  dependency; Node is already required at build time and nothing here reaches the
  shipped application.
- Also verify the trusted comment, which carries the file name and timestamp the
  updater displays. minisign covers it with a second signature over
  `signature_bytes || trusted_comment`, so it can be checked rather than trusted.
- Reject a malformed blob explicitly. `Buffer.from(value, "base64")` silently
  skips characters it does not recognise, so a corrupt signature decodes to
  something short instead of throwing; validate the base64 and the payload
  lengths before verifying.

The script also asserts that the `url` in the generated manifest points at the
tag being released, so a manifest stamped with the wrong version cannot direct
installs at another release's binary.

### 5.7 Channel-aware updater configuration

`scripts/enable-updater-artifacts.mjs` currently hardcodes one endpoint. Give it
a channel:

```js
const channel = (process.env.RELEASE_CHANNEL || "preview").trim();
const manifestName = channel === "stable" ? "latest.json" : "preview.json";
const updaterEndpoint =
  `https://github.com/damijanc/damaian/releases/latest/download/${manifestName}`;
```

`scripts/create-updater-manifest.mjs` writes to the same channel-derived filename
and adds a `"channel"` field to the manifest JSON. The upload and release steps
in the workflow glob `latest.json` and `preview.json` rather than `latest.json`
alone, and the preview channel never attaches assets to a GitHub Release.

The repository owner/name stays hardcoded in `enable-updater-artifacts.mjs`
because it is baked into the shipped binary and must not vary with a fork's
`GH_REPO`; a fork that wants its own updater channel edits that constant
deliberately. Add a comment saying so, since the neighbouring manifest script
does read `GH_REPO` and the inconsistency otherwise looks like a bug.

### 5.8 Channel in the application

The channel is a compile-time property. In `crates/desktop-app/src/main.rs`:

```rust
// Set by the release workflow. A build with no channel is a local
// developer build, which is a preview by definition.
const RELEASE_CHANNEL: &str = match option_env!("DAMAIAN_RELEASE_CHANNEL") {
    Some(channel) => channel,
    None => "preview",
};
```

Pass it to the About panel through `AboutMetadata.short_version`, which macOS
renders as `Version <version> (<short_version>)`:

```rust
let about_metadata = AboutMetadata {
    name: Some(app_name.clone()),
    version: Some(app.package_info().version.to_string()),
    short_version: Some(RELEASE_CHANNEL.to_string()),
    ..Default::default()
};
```

The workflow exports `DAMAIAN_RELEASE_CHANNEL` for the build step. Because
`option_env!` is evaluated at compile time, the value is fixed in the binary and
cannot be altered by the environment the user launches the app in.

### 5.9 Artifact naming

The uploaded workflow artifact becomes
`Damaian-macOS-arm64-${RELEASE_CHANNEL}-DMG`, so a preview artifact downloaded
from a workflow run cannot be mistaken for a release build.

### 5.10 Documentation

- `docs/MACOS_INSTALLATION.md`: [spec 15](15_install_and_update_verification.md)
  owns the rewrite of the install instructions. This spec only removes the
  "ad-hoc signed but not Developer ID signed or notarized" line from `Current
  Limitations` once stable signing lands.
- `docs/DEVELOPMENT.md`: add a release section naming every secret from §5.2 and
  what it is for, with no values, plus how to run a preview build locally
  (`npm run desktop:build:preview`).
- `AGENTS.md`: [spec 15](15_install_and_update_verification.md) owns the change
  to the "packaged build refusing to launch is expected" trap.

## 6. Acceptance Criteria

- A tagged release produces a Developer ID signed, notarized, stapled DMG, or
  produces no artifact at all.
- `codesign -dvvv` on the released app reports
  `Authority=Developer ID Application`, and the hardened runtime flag is set.
- `spctl --assess --type execute` accepts the app and
  `spctl --assess --type open --context context:primary-signature` accepts the
  DMG, inside the workflow.
- `xcrun stapler validate` passes for both the `.app` and the `.dmg`.
- A stable job with any required signing or notarization secret missing or empty
  fails at the preflight step, before the build runs, and names the missing
  secret.
- A stable job whose notarization is rejected fails, prints the notarization log,
  and uploads no artifact and creates no release.
- A `workflow_dispatch` build with no credentials still produces an ad-hoc
  preview DMG, labelled preview in the artifact name and in the About panel.
- A preview build writes `preview.json` and never `latest.json`.
- The updater signature is verified against `TAURI_UPDATER_PUBKEY` before upload;
  a signature produced by a different key fails the build.
- The generated manifest's `url` points at the tag being released.
- No secret value appears in workflow logs, the repository, the DMG, or the
  updater manifest.
- The About panel shows the channel next to the version.
- `npm run desktop:build` with no `APPLE_SIGNING_IDENTITY` set no longer silently
  ad-hoc signs.
- The five quality-gate commands from `AGENTS.md` pass. Application behaviour is
  otherwise unchanged.

## 7. Implementation Notes

Implemented 2026-09-03. Changes: `.github/workflows/macos-dmg.yml` (channel
input, preflight gate, API-key handling, split stable/preview build steps, DMG
notarization and stapling, real verification gate, channel-scoped artifact
name), `package.json` (`desktop:build` no longer defaults the identity, plus
`desktop:build:preview` and `updater:verify-signature`),
`scripts/enable-updater-artifacts.mjs` and `scripts/create-updater-manifest.mjs`
(channel-scoped manifests), the new `scripts/verify-updater-signature.mjs`,
`crates/desktop-app/src/main.rs` and `build.rs` (compiled-in channel), and the
release sections of `docs/DEVELOPMENT.md`.

Three deviations from the design above, each deliberate:

1. **§5.2's table lists `APPLE_API_KEY_PATH` as a required secret.** It is not
   one, as its own description says: the workflow decodes `APPLE_API_KEY_CONTENT`
   into `RUNNER_TEMP` and exports the path itself. Seven Apple secrets are
   required, not eight. The table is left as written; this note is the
   correction.
2. **`build.rs` gained `rerun-if-env-changed=DAMAIAN_RELEASE_CHANNEL`.** §5.8 is
   right that `option_env!` fixes the channel at compile time, but Cargo does
   not track that variable, and the workflow caches `target/`. Without this, a
   preview build followed by a stable build at the same version could reuse a
   binary carrying the wrong channel. The neighbouring
   `rerun-if-env-changed=TAURI_UPDATER_PUBKEY` set the pattern.
3. **Requirement 6 conflicted with the existing updater gate.** The
   `Configure updater release artifacts` step failed hard when
   `TAURI_UPDATER_PUBKEY` was absent, which would have blocked exactly the
   credential-less fork build that requirement 6 promises. It now fails closed
   for `stable` and, for `preview`, skips the updater artifacts and continues.
   `UPDATER_ARTIFACTS` carries that decision to the manifest and verification
   steps.

Two implementation details worth keeping:

- The verification gate captures `codesign -dvvv` output into a variable instead
  of piping it to `grep -q`. `grep -q` exits at the first match and closes the
  pipe, `codesign` then dies of SIGPIPE, and `set -o pipefail` reports a
  successful verification as a failure.
- §5.6's warning about the algorithm tags is correct and was confirmed against
  real `cargo tauri signer` 2.11.4 output: the public key reads `Ed`
  (`RWTg…`), the signature reads `ED` (`RUTg…`). One further detail the design
  does not mention: minisign's global signature covers the trusted comment
  **without** the separator space after `trusted comment:`. Including it makes
  every genuine signature fail.

`scripts/verify-updater-signature.mjs` was checked against a throwaway keypair
generated with the pinned CLI version, covering the happy path plus five
rejections: a signature from a different key, a bundle modified after signing, a
tampered trusted comment, a malformed base64 blob, and a manifest URL pointing at
the wrong tag.

Not done here, deliberately: `docs/MACOS_INSTALLATION.md` still says the build is
ad-hoc signed and not notarized, because that remains true of every artifact
published so far. Remove that line when the first stable release ships, per
§5.10.

Sequencing note: the workflow changes cannot be fully exercised on a pull
request, because a tag-triggered stable path is what they gate. Validate in this
order, and record the run URLs here:

1. `workflow_dispatch` with `channel: preview` and no Apple secrets — confirms
   the preview path still builds and that the preflight gate does not fire.
2. `workflow_dispatch` with `channel: stable` — exercises signing, notarization,
   stapling, and the full verification gate without creating a GitHub Release.
3. A deliberately broken run: `channel: stable` with one Apple secret temporarily
   cleared — confirms the preflight gate fails closed and uploads nothing.
4. A real tag push — the first stable release.

Step 3 is the one worth not skipping. The defect this spec exists to fix is a
pipeline that reports success while shipping an unsigned artifact, and the only
evidence that it is fixed is a run that fails when it should.

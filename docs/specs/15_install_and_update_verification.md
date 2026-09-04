# Feature Spec: Install and Update Verification

Status: Partially done, remainder skipped. The data-directory schema version
(§5.2, §5.3) shipped; the updater fixture tests, the update rehearsal, the
Keychain measurement, the documentation rewrite, and the second-person install
were skipped as verified in practice. See §7.
Order: 15 of 15
Roadmap: `docs/ROADMAP/00b_phase_0_distributable_build.md`, Phase 0, Work
Package 2 (Must). That directory is local-only and not committed, so the
reference is a name rather than a link; this spec is self-contained.
Related spec sections: none in `ai_coding_assistant_specification.md`. This is
release engineering, not product behaviour. Depends on
[`14_developer_id_signing_and_notarization.md`](14_developer_id_signing_and_notarization.md);
related to [`09_release_quality_gate.md`](09_release_quality_gate.md).

## 1. Motivation

[Spec 14](14_developer_id_signing_and_notarization.md) makes the release
pipeline produce a signed, notarized artifact. It does not prove that the
artifact installs, launches, and updates cleanly for someone who is not the
author. Those are different claims, and only the second one is what Phase 0 is
for: the exit condition for the phase is a second person's successful first
launch, because every later phase's usability claim rests on being able to put a
build in someone else's hands.

Three specific risks are unproven today:

- **The updater accepts what it should reject.** The desktop app auto-updates
  from GitHub Releases (`crates/desktop-app/src/main.rs:40`). Nothing tests that
  a manifest signed with the wrong key is refused. An updater that fails open is
  a remote code execution path into every install, and it is the single highest-
  severity item in this phase.
- **Nothing tests the data directory across a version boundary.** The data
  directory holds `config/`, `sessions/`, `audit/`, `patches/`, `rollback/`, and
  `models/`, and no file in it records a schema version. An update that changes a
  persisted format would silently lose a user's sessions, and there is no
  artefact to detect it with.
- **The Keychain double-prompt is explained but not verified.**
  `crates/desktop-shell/src/keychain.rs` is believed correct, with unstable
  ad-hoc code-signing hashes blamed for the two password prompts per launch.
  That explanation predicts one prompt under a stable Developer ID signature.
  If two prompts survive, the explanation was wrong and the Keychain code has a
  real defect.

Alongside those, the documentation currently leads with the workaround.
[`docs/MACOS_INSTALLATION.md`](../MACOS_INSTALLATION.md) devotes its
`Developer Preview Signing` section to `Open Anyway` and
`xattr -dr com.apple.quarantine`, and `AGENTS.md` tells agents that a packaged
build refusing to launch is expected rather than a bug. Both become actively
wrong the moment stable builds are signed, and the `AGENTS.md` line would train
agents to dismiss a real Gatekeeper regression.

## 2. Current State

- `crates/desktop-app/src/main.rs:40` registers `tauri_plugin_updater`;
  `main.rs:48` checks for updates at startup and `main.rs:284` re-checks from the
  `Check for Updates...` menu item. Signature verification is the plugin's, using
  the `pubkey` baked in by `scripts/enable-updater-artifacts.mjs`. No test in the
  workspace exercises the rejection path.
- `Config::default_data_dir` (`crates/workspace-engine/src/config.rs:190-200`)
  resolves `DAMAIAN_DATA_DIR`, falling back to
  `~/Library/Application Support/DamaianClient`.
- The data directory has no version marker. `config/user.conf` and
  `config/admin.conf` (`config.rs:243-249`), `sessions/`
  (`session.rs:163`), `audit/` (`audit.rs:58`), `patches/rejected/`
  (`edit.rs:107`), `rollback/<patch-id>/` (`patch_engine.rs:414`), and
  `models/all-MiniLM-L6-v2` (`embeddings.rs:27`) are read by convention. The only
  reference to migration anywhere in the workspace is a comment at
  `config.rs:1104` about an "un-migrated config", and there is no migration code
  behind it.
- `crates/desktop-shell/src/keychain.rs` stores the model API key through
  `SecKeychain*` calls. `model_api_key_env` holds `keychain:<account>` and never
  a raw key.
- [`docs/MACOS_INSTALLATION.md`](../MACOS_INSTALLATION.md) documents the
  quarantine workaround as the normal path, and lists "ad-hoc signed but not
  Developer ID signed or notarized" under `Current Limitations`.
- `AGENTS.md` "Traps" states that a packaged build refusing to launch is expected
  and instructs agents not to fix it.
- Two tests are already `#[ignore]`d for having real side effects, per the
  convention recorded in `AGENTS.md`.

## 3. Requirements

1. An automated test proves the updater rejects a manifest or archive signed with
   a key other than the configured `pubkey`, and that the application stays on
   its current version after the rejection.
2. An automated test proves the updater rejects a manifest whose archive bytes
   have been altered after signing.
3. The data directory carries an explicit schema version, written on first use
   and read on startup, so a version boundary is detectable rather than assumed.
4. A newer application refuses to silently operate on a data directory written by
   a schema version it does not understand. It reports the mismatch and leaves
   the data untouched.
5. An automated test covers the data directory across at least one version
   boundary — same-version load, older-version load, and unknown-newer-version
   refusal — using `DAMAIAN_DATA_DIR` so no real user data is touched.
6. A manual update rehearsal from the previous stable version to the new one
   confirms that sessions, config, and the stored Keychain reference survive, and
   its result is recorded in this spec's implementation notes.
7. The Keychain prompt count under a Developer ID signature is measured and
   recorded. One prompt per launch is the pass condition. Two prompts is a real
   defect that gets its own spec, and the ad-hoc explanation is retired either
   way.
8. [`docs/MACOS_INSTALLATION.md`](../MACOS_INSTALLATION.md) describes the normal
   signed install path first, and the preview-build workaround second, clearly
   scoped to unsigned preview builds.
9. `AGENTS.md`'s Gatekeeper trap is scoped to preview builds. A signed stable
   build refusing to launch is a bug from this phase onward.
10. A person who has never built Damaian from source installs the stable DMG and
    reaches a working first-run screen without consulting a workaround. Their
    result is recorded.

## 4. Non-goals

- A general-purpose migration framework. This spec adds a version marker, a
  refusal path, and one tested boundary — not a migration engine for formats that
  have not changed yet.
- Rewriting or reformatting any existing persisted data.
- Downgrade support. An older application meeting newer data refuses and reports;
  it does not attempt to convert.
- Changing the updater's transport, endpoint scheme, or UX. Channel handling
  belongs to [spec 14](14_developer_id_signing_and_notarization.md).
- Replacing the Keychain implementation. This spec measures the prompt count and
  records the finding; a defect, if found, is scoped separately.
- Automated end-to-end install testing on a fresh machine. The second-person
  install is a human check by design — an automated one on the build account's
  own machine would not test the thing that matters.
- Any change to application behaviour beyond the schema-version refusal path.

## 5. Design

### 5.1 Updater rejection tests

The updater signature check lives in `tauri_plugin_updater`, not in this
workspace, so the test target is the contract rather than the plugin's internals:
a manifest this pipeline could plausibly produce, verified against the public key
this application ships.

`scripts/verify-updater-signature.mjs` from
[spec 14](14_developer_id_signing_and_notarization.md) §5.6 is the same Ed25519
verification the plugin performs, in the pipeline's own code. Test it directly,
in a Node test file run by the existing `npm` tooling, over fixtures generated
once and committed:

| Fixture | Expectation |
|---|---|
| Archive signed with the release key | Accepted |
| Archive signed with a second, unrelated key | Rejected, key-ID mismatch reported |
| Archive signed with the release key, then one byte flipped in the archive | Rejected, signature mismatch reported |
| Signature blob truncated or not valid base64 | Rejected with a parse error, not a crash |

The keys used to generate fixtures are throwaway keys created for the test. The
real release private key never leaves CI secrets and is never used to build a
fixture.

For the application side, add an `#[ignore]`d integration test — per the
`AGENTS.md` convention for tests with real side effects — that points a built app
at a local manifest signed with the wrong key and asserts that the app remains on
its current version and surfaces an update error. Its doc comment states how to
run it manually, and its result is recorded in §7 rather than in CI.

This split is deliberate. The bytes-level check runs on every CI run and catches
the regression that would actually happen — a pipeline change that ships an
unverifiable artifact. The end-to-end check runs by hand and confirms the plugin
behaves as documented.

### 5.2 Data directory schema version

Write a marker at the root of the data directory:

```text
<data_dir>/schema.conf
```

```text
schema_version=1
```

The file uses the same flat `key=value` format as `config/user.conf`, so it needs
no new parser and stays readable and hand-editable.

Behaviour on startup, in `workspace-engine`, at the point the data directory is
first resolved:

| Marker state | Behaviour |
|---|---|
| Missing, directory empty or absent | Create the directory and write the current version. Normal first run |
| Missing, directory has existing content | Treat as version 1 and write the marker. This is every existing install, and it must not be disruptive |
| Equal to the current version | Proceed |
| Older than the current version | Run the migration for that boundary if one exists; otherwise proceed and update the marker |
| Newer than the current version, or unparsable | Refuse. Report the data directory path, the version found, and the version supported. Change nothing |

The refusal path matters more than the migration path. The realistic failure is a
user who updates, hits a problem, reinstalls the previous version, and has that
older build write over data the newer one had already reorganised. Refusing is
cheap; recovering the sessions is not.

Surface the refusal in the desktop shell as a clear startup error naming the
path, not as a silent empty state — an empty projects list would read as data
loss and prompt exactly the destructive recovery attempt this is meant to prevent.

Version 1 is defined as the current on-disk layout. No migration is written in
this spec; the migration hook exists so the next format change has somewhere to
go and a test harness already pointed at it.

### 5.3 Migration boundary test

Add integration tests in `crates/workspace-engine/tests/` that build a data
directory under a temporary `DAMAIAN_DATA_DIR` and assert:

- A directory with no marker and existing `sessions/` content is adopted as
  version 1, the marker is written, and the sessions still load.
- A directory with `schema_version=1` loads unchanged.
- A directory with `schema_version=999` is refused, the error names the path and
  both versions, and no file in the directory is created, modified, or deleted.
- A directory with a malformed marker (`schema_version=banana`) is refused the
  same way rather than being treated as version 0 or as missing.

The "no file was modified" assertion is the load-bearing one: compare a snapshot
of paths and modification times before and after. A refusal that still writes is
not a refusal.

### 5.4 Manual update rehearsal

Run against a throwaway data directory, never the real one:

```sh
DAMAIAN_DATA_DIR=.damaian-migration-test /Applications/Damaian.app/Contents/MacOS/damaian-desktop
```

Procedure:

1. Install the previous stable DMG. Launch it with the throwaway data directory,
   create a project and at least one session, apply a patch so `rollback/` and
   `audit/` have content, and store an API key so `model_api_key_env` holds a
   `keychain:` reference.
2. Quit. Snapshot the data directory.
3. Update through the in-app updater, not by replacing the `.app` by hand — the
   updater path is what users take and what this spec is verifying.
4. Relaunch with the same throwaway data directory. Confirm the project, the
   session and its history, the config, and the stored key reference are intact,
   and that `schema.conf` now exists.

Do not use `pkill -f` to clean up anything started here. The user's own app
shares the binary name; track the PID and kill that, per `AGENTS.md`.

### 5.5 Keychain prompt measurement

With a Developer ID signed build installed, launch the app cold three times and
count password prompts per launch. Record the counts in §7.

- One prompt per launch, or none after the first grant: the ad-hoc explanation
  held. Record it in §7. The condition is not currently described in
  [`docs/TROUBLESHOOTING.md`](../TROUBLESHOOTING.md), so nothing needs removing
  there; the claim this result supersedes is in the roadmap phase file's
  motivation.
- Two prompts per launch: the explanation was wrong. Record the finding, add a
  [`docs/TROUBLESHOOTING.md`](../TROUBLESHOOTING.md) entry describing it, open a
  separate spec against `keychain.rs`, and do not close Phase 0 on the assumption
  that signing fixed it.

Either outcome is a result. The failure mode to avoid is closing the work package
without measuring, leaving a wrong explanation in the documentation.

### 5.6 Documentation

[`docs/MACOS_INSTALLATION.md`](../MACOS_INSTALLATION.md):

- `Install From DMG` becomes the normal path and states plainly that the release
  DMG is Developer ID signed and notarized, so it opens with a double-click and
  no security override.
- The `Developer Preview Signing` section is retitled to make its scope explicit
  — it applies to unsigned preview builds and local builds only — and moves below
  the normal path. The `xattr -dr com.apple.quarantine` instruction keeps its
  existing warning about only doing this for builds you trust.
- `Updates` gains a sentence on channels: a stable install follows the stable
  channel and is never offered a preview build.
- `Current Limitations` drops the ad-hoc signing line.

`AGENTS.md`, Traps: rewrite the "packaged build refusing to launch is expected"
entry to scope it to preview and local builds, and state that a signed stable
build refusing to launch is a bug to investigate rather than a known condition.

[`docs/TROUBLESHOOTING.md`](../TROUBLESHOOTING.md): add entries for an update
rejected by signature verification (what the user sees, that staying on the
current version is the correct outcome, and that a rejection is a reason to
distrust the artifact rather than to retry), and for the schema-version refusal
(what the message means, where the data directory is, and that the fix is to run
a matching application version rather than to delete the directory).

## 6. Acceptance Criteria

- An updater manifest signed with the wrong key is rejected, and the application
  stays on its current version.
- An archive altered after signing is rejected.
- A malformed signature blob is rejected with a clear error and no crash.
- A data directory with no marker and existing content is adopted as version 1
  without disruption, and its sessions still load.
- A data directory with an unknown-newer or malformed schema version is refused,
  the error names the path and both versions, and no file in the directory is
  created, modified, or deleted.
- The desktop shell shows the refusal as an explicit startup error, not an empty
  project list.
- An update from the previous stable version through the in-app updater preserves
  projects, sessions, audit and rollback content, config, and the stored Keychain
  reference.
- The Keychain prompt count under a Developer ID signature is measured and
  recorded in §7, and the documentation matches the measurement.
- `docs/MACOS_INSTALLATION.md` leads with the signed install path, and the
  workaround is scoped to preview builds.
- `AGENTS.md`'s Gatekeeper trap is scoped to preview builds.
- A person other than the author installs the stable DMG and reaches a working
  first-run screen without consulting a workaround, and their result is recorded
  in §7.
- The five quality-gate commands from `AGENTS.md` pass. No new runtime Node.js
  dependency is added; the signature-verification test is build-time only.

## 7. Implementation Notes

Partially implemented 2026-09-04, and the remainder deliberately skipped. What
follows is the record of what actually ran, not what was intended.

### Implemented: the data directory schema version (§5.2, §5.3)

New `crates/workspace-engine/src/data_schema.rs` writes and reads
`<data_dir>/schema.conf` (`schema_version=1`, the same flat `key=value` format
as `config/user.conf`) and implements the table in §5.2:

| Marker state | Behaviour |
|---|---|
| Missing, directory empty or absent | Created, marker written, `Initialized` |
| Missing, directory has content | Adopted as version 1, marker written, `Adopted` |
| Equal to the current version | `Current`, nothing written |
| Older | `migrate()` (no boundary exists yet) then marker rewritten, `Upgraded` |
| Newer, `0`, or unparsable | Refused. Nothing created, modified, or deleted |

Two deviations from the design, both deliberate:

1. **`schema_version=0` is refused rather than treated as older.** §5.2 says
   "older than the current version" migrates, but no build ever wrote `0`, so a
   `0` marker is a corrupt or hand-edited file, not an old install. Refusing is
   consistent with the unparsable case.
2. **The refusal is surfaced as a native error dialog, not a page in the web
   UI.** The shell is what would serve an error page, and the shell is what
   refuses to start, so there is nothing to serve it from. `desktop-app`'s
   `setup` calls `desktop_shell::verify_data_dir_schema()` before it spawns the
   shell thread and, on refusal, shows a `MessageDialogKind::Error` dialog
   naming the path and both versions, then exits. The dialog is shown from a
   spawned thread because `blocking_show` must not run on the main thread.

Wired into all three entry points, so no front end can write over data it does
not understand: `desktop_shell::run_server_with_ready` (before it binds the
port), `damaian-cli`'s `main` (before any command runs), and `desktop-app`'s
`setup` (before the shell starts).

Twelve tests, written before the code:

- `crates/workspace-engine/tests/data_schema.rs` — the four §5.3 cases plus
  first run. The load-bearing assertion is the before-and-after snapshot of
  every path, length, and mtime under the directory: a refusal that still
  writes is not a refusal.
- `crates/workspace-engine/src/data_schema.rs` inline tests — marker parsing,
  including a commented-out and an absent version.
- `crates/desktop-shell/src/lib.rs` — startup refuses a `999` directory naming
  the path and the version found, and marks a fresh one.
- `crates/damaian-cli/tests/data_schema_refusal.rs` — the built binary exits
  non-zero with the refusal on stderr, and marks a fresh directory. This one
  runs the real binary, so it also proves the wiring, not just the module.

The quality gate passes: `cargo fmt --all -- --check`,
`cargo clippy --workspace --all-targets --locked -- -D warnings`,
`cargo test --workspace --locked`, `npm run lint:web`, and `cargo deny check`.

Not verified: the desktop-app dialog itself. The refusal it displays is tested,
the display path is not — it needs a packaged build launched by hand against a
`schema_version=999` directory.

### Skipped

Skipped on the maintainer's decision, 2026-09-04, on the grounds that install
and update work as expected in practice. Each is a real gap, listed so a later
phase can pick it up rather than assume it was covered:

- **Requirements 1, 2 — updater rejection fixtures.** No Node test file, no
  committed fixtures, and no `#[ignore]`d end-to-end test. The pipeline's
  `scripts/verify-updater-signature.mjs` was exercised by hand against a
  throwaway keypair during [spec 14](14_developer_id_signing_and_notarization.md)
  (its §7 records the happy path plus five rejections, including a wrong key, a
  bundle modified after signing, and a malformed base64 blob), so the behaviour
  is evidenced — but nothing runs it in CI, so a pipeline change that ships an
  unverifiable artifact is still undetected.
- **Requirement 6 — the update rehearsal.** Not run. No recorded evidence that
  sessions, config, and the `keychain:` reference survive an in-app update
  across a version boundary.
- **Requirement 7 — the Keychain prompt count.** Not measured as three cold
  launches under this spec. Spec 14 §7 records the outcome from the first stable
  release: with a stable Developer ID identity, clicking **Always Allow** once
  makes the grant persist and later launches prompt zero times, and the first
  launch after any identity change still prompts. That puts the project on the
  first branch of §5.5 — the ad-hoc explanation held — on spec 14's evidence
  rather than this spec's.
- **Requirements 8, 9 — the documentation rewrite.**
  `docs/MACOS_INSTALLATION.md` still leads with the `Open Anyway` /
  `xattr -dr com.apple.quarantine` workaround and still lists ad-hoc signing
  under `Current Limitations`, and `AGENTS.md`'s Gatekeeper trap still says a
  packaged build refusing to launch is expected. Both were true of every
  artifact before v0.31.0 and are wrong for a signed stable build. This is the
  cheapest outstanding item and the one most likely to cause harm, because it
  trains an agent to dismiss a real Gatekeeper regression.
- **Requirement 10 — the second-person install.** Not done, so Phase 0's exit
  condition is not evidenced. v0.31.0 was installed and checked on a real
  machine with no `xattr` command and no Gatekeeper override (spec 14 §7), but
  that machine was the author's.

# Feature Spec: Release Quality Gate

Status: Not started
Order: 9 of 9
Related spec sections: none. This is a release-engineering defect, not a product gap.

## 1. Motivation

A tagged release builds and publishes even when the Quality workflow is
failing. This is not a race or a flake — the two workflows are simply not
connected, and nothing in the release path would ever notice.

**Quality never runs on tags.** `.github/workflows/quality.yml` triggers on
`pull_request` and `push` to `main`. A tag push matches neither, so pushing
`v0.24.0` produces no Quality run at all.

**The release path has no dependency on Quality.**
`.github/workflows/macos-dmg.yml` triggers on `push: tags: v*`. Its `build` job
has no `needs:`, and `publish-release` declares only `needs: build`. There is
no edge, direct or transitive, from the release to any quality signal. GitHub
Actions provides no cross-workflow blocking, so adding a tag trigger to
`quality.yml` would not help: the two runs would proceed independently and the
release would still publish.

**The release job's own checks are a thin subset.** `build` runs
`node --check crates/desktop-shell/static/app.js` and `cargo test --locked`.
It does not run `cargo fmt --check`, `cargo clippy -- -D warnings`, `typos`,
`cargo-deny`, or `npm run lint:web`. It also omits `--workspace` from its test
invocation, so it tests less than Quality does even in the one area they
overlap.

The observed instance: `typos` failed on three misspellings in
`crates/workspace-engine/tests/foundation.rs`, and releases continued to ship.
The typos are trivial. The gap that let a red workflow coexist with a published
release is not.

## 2. Requirements

1. A tagged release must not build or publish if any Quality check fails.
2. Manual `workflow_dispatch` builds must be gated the same way. A build is a
   build regardless of how it was triggered.
3. Quality must have exactly one definition. PRs, `main`, and tags must all run
   the same checks, so a check cannot pass on one path and be absent on another.
4. The existing `pull_request` and `push: main` behaviour of `quality.yml` must
   be unchanged.
5. The three `typos` findings must be resolved so the gate passes on the
   current tree.

## 3. Non-goals

- Sharing a Cargo cache between the Quality job and the release build. They use
  different cache keys today; unifying them is a performance change with its own
  correctness questions, and is out of scope.
- Adding a `push: tags` trigger to `quality.yml`. It would run the same checks a
  second time per tag, and both runs would land in the same
  `quality-${{ github.ref }}` concurrency group with `cancel-in-progress: true`
  — so the standalone run and the gating run would cancel each other. Explicitly
  rejected.
- Branch protection or repository rulesets. They govern merges into branches,
  not tag pushes, and cannot express "this workflow must pass before that one".
- Fixing `unappliable` in `SECURITY.md` and
  `crates/workspace-engine/src/secret_scanner.rs`. The `typos` dictionary does
  not flag that variant, so it does not affect the gate.

## 4. Design

### 4.1 Quality becomes a reusable workflow

Add `workflow_call:` to the `on:` block of `quality.yml`. Nothing else in the
file changes — same two jobs (`checks` on Linux, `rust` on macOS), same steps,
same `permissions: contents: read`, which carries over to callers.

This is the whole of requirement 3: one file defines quality, and every path
that wants it calls the same file.

### 4.2 The release pipeline calls it

`macos-dmg.yml` gains a `quality` job:

```yaml
quality:
  name: Quality
  uses: ./.github/workflows/quality.yml
```

`build` gains `needs: quality`. `publish-release` keeps `needs: build`. The
resulting chain is:

```
quality → build → publish-release
```

A failure anywhere in `quality` fails the job, which blocks `build` on its
`needs`, which blocks `publish-release` on its. The pipeline fails closed: no
DMG is produced and no GitHub Release is created or edited. Because the
`quality` job carries no `if:` condition, it gates `workflow_dispatch` runs as
well as tag pushes, satisfying requirement 2.

Quality runs against the tag's own commit, not against whatever `main` happened
to be, so the gate reflects exactly what is being shipped.

### 4.3 Redundant steps are removed from `build`

Two steps in `build` become dead weight once `quality` runs first:

- `Check desktop JavaScript` — `quality.checks` runs the identical
  `node --check` on the same file.
- `Run Rust tests` (`cargo test --locked`) — `quality.rust` runs
  `cargo test --workspace --locked`, a strict superset.

Both are deleted. Every other step in `build` (version stamping, updater
artifact configuration, Tauri CLI install, the build itself, bundle
verification, manifest creation, artifact upload) is release-specific and stays.

### 4.4 Accepted cost

`quality.rust` and `build` use different Cargo cache keys
(`…-quality-${{ hashFiles('Cargo.lock') }}` and
`…-cargo-${{ hashFiles('Cargo.lock') }}`), so they share no compilation. A tag
pipeline now runs a full macOS clippy-and-test pass to completion before the
release build starts.

This serialization is the gate. Overlapping the two would mean starting a build
before knowing whether it should exist, which is the defect being fixed. The
added wall-clock time is accepted deliberately.

## 5. Typo fixes

Three findings in `crates/workspace-engine/tests/foundation.rs`, all genuine
misspellings:

| Line | Current | Fixed |
|------|---------|-------|
| 795 | `appliable` (doc comment) | `applicable` |
| 2674 | `fn propose_edit_records_failure_when_model_output_is_unparseable` | `…_is_unparsable` |
| 2675 | `temp_dir("edit-unparseable-failure")` | `temp_dir("edit-unparsable-failure")` |

Both `unparseable` occurrences are inside the same test. The test function has
no callers, and the string is only a scratch-directory name, so neither rename
affects anything outside those two lines.

## 6. Acceptance criteria

1. `typos` reports no findings on the working tree.
2. `cargo test --workspace --locked` passes, including the renamed test.
3. `quality.yml` declares `workflow_call` alongside its existing
   `pull_request` and `push` triggers, with its jobs and steps otherwise
   unchanged.
4. `macos-dmg.yml` contains a `quality` job that uses
   `./.github/workflows/quality.yml`, and `build` declares `needs: quality`.
5. `build` no longer contains the `Check desktop JavaScript` or `Run Rust tests`
   steps.
6. Both workflow files parse as valid GitHub Actions workflows.
7. On a tag push where any Quality check fails, no DMG artifact is uploaded and
   no GitHub Release is created or modified.

Criterion 7 cannot be observed without pushing a tag against a failing tree.
Criteria 3–6 establish the wiring statically; criterion 7 is what that wiring
is for.

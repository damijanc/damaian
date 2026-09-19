# Feature Spec: Release Quality Gate

Status: Done
Order: 9 of 9
Depends on: nothing in this directory.
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

(A fourth job was appended to this chain on 2026-09-17; see §4.5 and §7.)

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
- `Run Rust tests` (`cargo test --locked`) — `quality.rust` runs the same tests
  across the whole workspace, a strict superset. (That step was
  `cargo test --workspace --locked` when this was written; the runner changed
  on 2026-09-16, see §7.)

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

### 4.5 The changelog row is written after publishing

Added 2026-09-17. `CHANGELOG.md` is generated from tags by
`scripts/update-changelog.mjs`, which can only run once the tag exists. That
made it a manual step after tagging, recorded in `AGENTS.md`, and a forgotten
run left the file behind the releases it documents while the GitHub Release
itself stayed correct. The same pipeline that creates the tag's release can
write the row.

`macos-dmg.yml` gains a fourth job:

```yaml
update-changelog:
  name: Update changelog
  needs: publish-release
  runs-on: ubuntu-latest
  permissions:
    contents: write
```

The chain becomes:

```
quality → build → publish-release → update-changelog
```

`needs: publish-release` carries that job's
`if: startsWith(github.ref, 'refs/tags/')` transitively, so a
`workflow_dispatch` build never writes a row — there is no tag for it to
document.

**The job runs after publishing, not before.** A bookkeeping commit stays off
the critical path: the DMG and the release notes are already out before the job
starts, so a push that fails cannot withhold a build from users. The job still
fails the run, so a rejected push is visible rather than silent.

The job checks out `main` rather than the tag, which is detached and cannot be
pushed to, with `fetch-depth: 0` — the script reads every tag and counts
reachable commits to order them, so a shallow clone would produce a wrong
ordering rather than an error. Node is set up without `npm ci`, because
`update-changelog.mjs` imports only `node:fs/promises` and
`node:child_process`. The job runs `npm run changelog:update`, exits quietly
when `CHANGELOG.md` is unchanged, and otherwise commits as `github-actions[bot]`
with a single-line subject naming the tag being released, rebasing onto `main`
before pushing so a concurrent push does not lose the race. The subject names
the tag rather than the rows written, which differ only when the job is catching
up more than one undocumented tag; deriving the real list would mean parsing the
script's stdout, and that coupling is not worth the accuracy.

**Existing rows are never touched, and that guarantee is the script's rather
than the workflow's.** `update-changelog.mjs` splices new rows under the
`<!-- releases -->` separator, carries the remainder of the file through byte
for byte, and refuses to write if that remainder would have changed. A version
already present in the table is skipped entirely. CI running the same script
inherits all of it, so a row edited by hand after an earlier release survives
every later run, and a re-run is a no-op.

The bot's own commit touches only `CHANGELOG.md`, which the script files under
the `Docs` component, and `Docs` commits produce no bullets. The automation does
not appear in the next release's row.

**What stays manual.** Moving an entry out of the `Unreleased` table when it
ships. That is a judgement about whether a shipped change matches what was
specified, the script has never made it, and this does not change that. After a
release, `Unreleased` can still name work that just shipped until someone edits
it.

**`AGENTS.md` becomes wrong and is corrected.** Its planning section currently
carries the instruction **"After tagging, run `npm run changelog:update`"** as
the reader's responsibility. That sentence is replaced by one stating that the
release pipeline runs it, that the command remains available for a local run
against tags already pushed, and that `Unreleased` is still edited by hand.
Everything the existing passage says about the script's behaviour — rows
inserted under the `<!-- releases -->` marker, the marker not to be removed,
existing rows never rewritten, a tag with no commits omitted — is still true and
stays.

Two things are deliberately not done. The GitHub Release notes still come from
`git log` over the tag range rather than from the changelog row: the two texts
have different audiences and different lifetimes — notes are frozen at
publication, a row is edited afterwards — and merging them is a separate
decision. And `npm run changelog:check` remains uncalled by any workflow; with
the row written automatically there is nothing left for it to catch, and it
stays a local command.

Accepted cost: pushing to `main` triggers `quality.yml` on `push`, so every
release spends a full Quality run on a one-file commit. Suppressing it would
mean either a `[skip ci]` marker in a subject line that appears in GitHub's
release feed, or a `paths-ignore` on `quality.yml` that would also stop Quality
running on genuine documentation-only commits. Neither trade is worth a few
minutes per release.

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

This document is added to `extend-exclude` in `_typos.toml`. It has to quote
the misspellings in order to specify their removal, and `typos` has no
inline-ignore directive, so a file-scoped exclusion is the only way to state
the fix and pass the check that motivated it. The exclusion is deliberately
narrow: one path, not a word added to a repo-wide allow list.

## 6. Acceptance criteria

1. `typos` reports no findings on the working tree, with this document excluded
   in `_typos.toml`.
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

Criteria 8–11 cover §4.5 and were added on 2026-09-17:

8. `macos-dmg.yml` contains an `update-changelog` job declaring
   `needs: publish-release` and `permissions: contents: write`, and the file
   still parses as a valid workflow.
9. Running `npm run changelog:update` against a tree whose `CHANGELOG.md`
   already documents every tag makes no change to the file and no commit.
10. On a tag push, `CHANGELOG.md` on `main` gains one row for that tag, and the
    text of every row already present is byte-identical afterwards.
11. `AGENTS.md` no longer instructs the reader to run the command after tagging.

Criterion 10 is the append-only requirement, and it is enforced by the script's
own refusal to write a changed tail rather than by the workflow. Criterion 9 is
the part of it that can be checked locally without pushing a tag.

## 7. Implementation notes

**2026-09-19 — an advisory check, `npm run specs:check`.** `quality.lint` now
runs `scripts/check-spec-status.mjs`, which reports a spec's `Depends on:` line
disagreeing with the depended-on spec's own `Status:` line, in either direction.
It exists because [`README.md`](README.md)'s "What to build next" is *derived*
from those lines, so one stale line makes the directory's own sequencing advice
wrong, and re-deriving it cannot repair the source it reads. The failure has
occurred twice and neither occurrence was caught by review — see `AGENTS.md`,
"Finishing a spec goes further than those four".

**It warns and exits 0, so the gate is still the seven blocking checks.** The
trade was considered and refused: a documentation inconsistency that stops a
build stops the person who just finished the spec, which is precisely the moment
you want someone marking things Done rather than avoiding it. Under GitHub
Actions each finding becomes a `::warning` annotation with a file and line, so
the report is visible on the run and the pull request without gating it.
`--strict` exits 1 and is the switch if that trade ever changes — a flag rather
than an edit, so flipping it is a decision and not a rewrite. Node built-ins
only, so it needs no `npm ci`.

**2026-09-16 — the test step became `cargo nextest run`.** `quality.rust` now
runs `cargo nextest run --workspace --locked --final-status-level slow`, with
`cargo-nextest` installed by a pinned `taiki-e/install-action`. The gate is
still the same seven checks and still a strict superset of what `build` used to
run; only the runner changed. Criterion 2 above records what was verified when
this spec was executed and is left as written.

The reason was cost, not preference. The suite had reached about forty-five
minutes, and compilation was not the cause: it ran at roughly 92% idle — 51s of
user time against 1307s of wall — blocked in `register_with_server`, waiting on
macOS `fseventsd`. `IndexCache::get_or_build` registered a filesystem watcher
synchronously, and because every test builds its own throwaway repository, each
one paid ten to fifteen seconds for freshness it never used. Two test binaries
alone held 29 minutes of it.

Registration now happens off the calling thread, which also removes the same
stall from opening a repository in the app, and the suites that index-and-
discard set `Config.enable_index_watcher: false`. Nextest compounds that by
running the test binaries concurrently instead of one after another — which
pays off here precisely because the residual time is wait rather than compute.

Measured on the runner: 651 tests in 28s, the whole `rust` job in 2m44s, of
which the Cargo cache restore is now the largest step at 72s. Nothing was lost
in the swap: nextest does not run doctests and this workspace has none, and the
14 skipped tests are the pre-existing `#[ignore]`d ones, so the 665 total is
unchanged. `--final-status-level slow` prints the slowest tests, so the same
drift shows up in the log next time rather than only in the job duration.

**2026-09-17 — the pipeline writes the changelog row.** The release pipeline
gained a fourth job, specified in §4.5, and §4.2's chain diagram was corrected
to match. Criteria 8–11 were added for it; criteria 1–7 record what was verified
when this spec was first executed and are left as written.

It is recorded here rather than as a new numbered spec because it extends the
pipeline this document already defines, and splitting one pipeline across two
specs would leave neither describing it completely. There is no `docs/PLAN/`
work package behind it: it came from reading the release path and noticing that
the one step in it still performed by hand had no reason to be.
`OBSERVATIONS.md` held no open entry touching the release path when this was
written.

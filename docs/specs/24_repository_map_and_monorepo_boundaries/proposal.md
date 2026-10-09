# Feature Spec: Repository Map and Monorepo Boundaries

Status: In progress. Split into a folder and planned on 2026-10-06. Design
unchanged from the original flat spec. Corrections where it no longer matches
the code are in [`context.md`](context.md), not inlined here, the way specs 20,
22 and 31 kept theirs. The most important ones:
- commands already carry a working directory, but Allow Always and the
  outside-root check both read it as the repository root;
- the allowlist is per repository, so §5.5's per-root test fails today;
- the index is in memory only, so roots are looked up rather than stamped on
  `FileRecord`;
- the map reaches the model as a context item, so `build_model_prompt` is
  never edited and only Task 7 touches `chat.rs`.

Read `context.md` before starting any task in [`tasks.md`](tasks.md).
Order: 24 of 27
Plan: `docs/PLAN/03_phase_3_code_understanding.md`, Phase 3, Work
Package 2 (Must). That directory is local-only and not committed, so the
reference is a name rather than a link; this spec is self-contained.
Depends on: [#21](../21_task_plan_progress_and_budget/proposal.md) (plan state) —
built; [#22](../22_findings_model_and_panel/proposal.md) (findings) — built.
Everything else named below is a cross-reference, not a prerequisite.
Related spec sections: `ai_coding_assistant_specification.md` section 7.2
(project indexer), section 7.4 (command approval). Related implementation specs:
[`02_semantic_search.md`](../02_semantic_search.md),
[`11_agents_md_support.md`](../11_agents_md_support.md) (the per-root precedent this
extends; instruction precedence is defined there and not redefined here),
[`13_docker_command_support.md`](../13_docker_command_support.md),
[`23_verification_loop.md`](../23_verification_loop.md) (runs the commands this
work package assigns to roots).

## 1. Motivation

Damaian treats a repository as one flat thing with one root.

`CommandPolicy::detect_project_commands`
(`crates/workspace-engine/src/command_policy.rs:189`) looks for `package.json`,
`Cargo.toml`, `pyproject.toml`, `go.mod`, `pom.xml`, and `build.gradle` — in
**one** directory, the one it is handed. In a monorepo with `packages/api`
(Node) and `crates/engine` (Rust), pointing Damaian at the top level finds
whichever manifests happen to sit there and misses the rest. Worse, a test
command discovered anywhere runs with the repository root as its working
directory, so `npm test` for `packages/api` runs where there is no
`package.json` and fails for a reason that has nothing to do with the code.

There is also no high-level view. The agent learns about a repository by
searching it, one query at a time, which means every session rediscovers the
same structure and spends context doing it. A compact map — where the roots are,
what languages they use, where the tests live, what is generated — is cheap to
build and answers questions that searching answers expensively.

The two problems are one work package because detection and enforcement are the
same problem: knowing `packages/api` is a root is only useful if commands,
instructions, and search results respect that boundary afterwards.

## 2. Current State

- **Project-command detection is single-root.**
  `detect_project_commands(root_path)` (`command_policy.rs:189-230`) checks
  `root.join("package.json")` for `test`, `lint`, `typecheck`, `build`, and
  `format` scripts, then a fixed table of
  `(pyproject.toml → pytest)`, `(pytest.ini → pytest)`, `(pom.xml → mvn test)`,
  `(build.gradle → gradle test)`, `(go.mod → go test ./...)`,
  `(Cargo.toml → cargo test)`. It does not descend, and it returns
  `ProjectCommand { name, command, risk }` with no working directory.
- **Ancestor walking already exists, for instructions.**
  `agent_instruction_paths` (`crates/workspace-engine/src/context_manager.rs:280-309`)
  takes the context paths and walks each one's directory ancestors, emitting an
  `AGENTS.md` candidate per level, rejecting absolute paths and `../` traversal.
  This is the mechanism [spec 11](../11_agents_md_support.md) delivered, and it is
  the precedent for per-root behaviour.
- **The index is flat and path-keyed.** `RepositoryIndex { repository_id,
  root_path, indexed_at_ms, files, skipped }`
  (`crates/workspace-engine/src/indexer.rs:52-58`), where each `FileRecord`
  carries a repository-relative `path` (`indexer.rs:22-33`). Paths are unique
  strings, so `packages/a/index.ts` and `packages/b/index.ts` are already
  distinct — but nothing groups them by root or scopes a search to one.
- **There is no map of any kind.** No structure summary, no entry points, no
  generated-path list, no per-root metadata.
- **`language.rs::detect_language`** (`crates/workspace-engine/src/language.rs:3`)
  maps a path to a language name, which is the raw material for per-root
  language detection.
- **The vector index is persisted per repository** at
  `<data_dir>/vector-index/<repository_id>.bin`
  (`crates/workspace-engine/src/vector_index.rs:149-152`), with `load` and
  `save` (`vector_index.rs:22-40`). This is the existing per-repository
  persistence pattern.
- **Nothing is user-overridable** about structure. There is no place to say "this
  directory is a root" or "this one is not".

## 3. Requirements

1. A compact, deterministic repository map records: project and package roots;
   languages and frameworks; important manifests; entry points; major
   directories; test locations; generated and vendor paths; local instruction
   files; and available validation commands.
2. The map refreshes incrementally and stays under a documented token ceiling,
   enforced by test.
3. Each detected root is associated with its `AGENTS.md` hierarchy, language and
   package metadata, validation commands, index and symbol namespaces, working
   directory, and optional permission restrictions.
4. One task may touch several roots, and the UI shows which.
5. Commands run from the correct root. A test command discovered in
   `packages/api` runs with `packages/api` as its working directory.
6. Context preserves path identity across roots. Two files named `index.ts` in
   different packages are never conflated.
7. Root detection is explainable and manually overridable, and a correction
   persists.
8. Instruction precedence follows [spec 11](../11_agents_md_support.md). This work
   package does not invent a second precedence rule.

## 4. Non-goals

- Parsing manifests for dependency graphs, workspace member resolution, or
  version constraints. Detection is by manifest presence and a small set of
  well-known fields, not by understanding a build system.
- Supporting every monorepo tool's conventions (Nx, Turborepo, Bazel, Lerna,
  pnpm workspaces). §5.2 detects roots by manifest presence, which covers these
  incidentally where they use standard manifests, and does not special-case any
  of them.
- Running commands for a root the user did not approve. Roots change the working
  directory of an approved command; they do not change the approval boundary.
- Cross-root refactoring or dependency-aware task ordering.
- Remote or organization-wide repository mapping.
- A visual repository browser. The map is context and metadata; the UI surface is
  a root list and a per-root detail view.
- Replacing `detect_project_commands`. It is called per root instead of once.

## 5. Design

### 5.1 The map

```rust
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RepositoryMap {
    pub repository_id: String,
    pub schema_version: u32,
    pub generated_at_ms: u128,
    /// Hash of the inputs the map was derived from, for staleness detection.
    pub fingerprint: String,
    pub roots: Vec<ProjectRoot>,
    /// Directories excluded from the map and why.
    pub excluded: Vec<ExcludedPath>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProjectRoot {
    /// Repository-relative. "" is the repository root itself.
    pub path: String,
    /// Why this was treated as a root — requirement 7's explainability.
    pub detected_by: RootEvidence,
    pub languages: Vec<String>,
    pub manifests: Vec<String>,
    pub entry_points: Vec<String>,
    pub test_paths: Vec<String>,
    pub generated_paths: Vec<String>,
    pub instruction_files: Vec<String>,
    pub commands: Vec<RootCommand>,
    /// Set when the user corrected detection. Never overwritten by a rescan.
    pub user_override: Option<RootOverride>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RootCommand {
    pub name: String,
    pub command: String,
    pub risk: CommandRisk,
    /// Repository-relative directory the command runs in. Requirement 5.
    pub working_directory: String,
}
```

`RootCommand` is `ProjectCommand` plus `working_directory`. That one field is
requirement 5: today `ProjectCommand` (`command_policy.rs:200-204`) has no
directory, so the caller supplies one and supplies the wrong one.

`detected_by` carries the actual evidence — which manifest file, at which path —
rather than a boolean. Requirement 7's "explainable" means the UI can say
"treated as a root because `packages/api/package.json` exists", which is a
sentence a user can agree or disagree with.

### 5.2 Root detection

Walk the repository honouring the existing ignore, size, and path rules, and
mark a directory as a root when it contains one of the manifests
`detect_project_commands` already knows: `package.json`, `Cargo.toml`,
`pyproject.toml`, `pytest.ini`, `go.mod`, `pom.xml`, `build.gradle`.

Reusing that exact list is deliberate. A root is useful here precisely because it
has commands and metadata, and the list of things that give a directory commands
is already written down in one place. A second, longer list of "things that look
like a project" would drift from the first and produce roots with nothing to run.

Three rules keep the result sane on real repositories:

- **The repository root is always a root**, even with no manifest, so a
  single-language repository with no manifest still has one entry and the
  map is never empty.
- **Nested roots are kept, not flattened.** A Cargo workspace root with member
  crates yields the workspace root and each member. Members are the
  correct working directory for a member's own test command, and the workspace
  root is correct for `cargo test --workspace`. Both are true at once.
- **Vendor and generated directories are never roots.** `node_modules`,
  `vendor`, `target`, `dist`, `build` and the configured ignore patterns are
  excluded, and recorded in `excluded` with the reason. Without this, a
  repository with dependencies vendored in yields hundreds of roots.

A depth ceiling bounds the walk. Roots below it are not detected, and the fact
is recorded in `excluded` rather than being silent — a monorepo nested deeper
than the ceiling should tell the user, not quietly lose half its packages.

### 5.3 Determinism

Requirement 2's determinism (two runs over an unchanged repository produce
identical output) is not automatic: a directory walk yields entries in
filesystem order, which varies.

Every collection in the map is sorted before it is written — roots by path,
languages, manifests, entry points, test paths, commands by name. `generated_at_ms`
is excluded from the comparison, and `fingerprint` is computed over the sorted
content, so an unchanged repository produces an unchanged fingerprint. The
determinism test compares two full serialisations with the timestamp field
removed.

### 5.4 The token ceiling

Requirement 2's ceiling is what stops the map from becoming the thing it exists
to avoid. `repository_map_max_tokens` in `Config`, with a documented default,
enforced when the map is rendered for model context.

Rendering degrades in a fixed order rather than truncating arbitrarily, so the
most useful information survives a large repository:

1. Roots, their languages, and their commands — always included.
2. Entry points and test paths — dropped per root, largest root first.
3. Major directories — dropped entirely.
4. Generated and vendor paths — reduced to a count.

When anything is dropped, the rendered map says so explicitly ("12 of 47 roots
shown"), because a silently abridged map is a map the agent will draw wrong
conclusions from. The full map remains available to the UI and the context
inspector ([spec 27](../27_context_inspector.md)); only the rendered-for-model form
is bounded.

### 5.5 Per-root command execution

`detect_project_commands` is called once per detected root, with that root's
path, and each resulting command records `working_directory: root.path`.

The verification loop ([spec 23](../23_verification_loop.md)) and every other
command path use `working_directory` when executing. Requirement 5 is then a
property of the data rather than a rule each call site must remember.

Two consequences to be explicit about:

- **Risk classification is per root.** `ProjectCommand.risk` comes from
  `self.classify(&command, root)` (`command_policy.rs:202`), which already takes
  the root. Calling it per root means the same command text can classify
  differently in two roots, which is correct — and `command_allowlist` matching
  stays exact-command, so allowlisting `npm test` in one root does not silently
  authorise it in another where it does something else. This is worth a test.
- **Path policy applies per root**, and `path_policy.rs` is evaluated against
  the repository root as it is today. A root is a working directory, not a new
  policy scope; a root cannot widen what paths are readable.

### 5.6 Path identity

Requirement 6 is largely already satisfied: `FileRecord.path`
(`indexer.rs:24`) is repository-relative, so `packages/a/index.ts` and
`packages/b/index.ts` are distinct keys today.

What is missing is *scoping*, and the requirement is really about display and
retrieval. Each `FileRecord` gains a `root_path` field naming the nearest
enclosing root, so search results can be grouped and filtered by root and a
result can be shown as `index.ts` in `packages/a` rather than as a bare
filename. The index namespace stays one per repository — a second index per root
would multiply the cold-index cost this phase is trying to reduce, and the
`root_path` field gives the same grouping for free.

Anywhere a file is displayed by basename, the root qualifies it. This is where
requirement 6's "never conflated" is actually at risk: not in the index, but in
a UI list showing two rows called `index.ts`.

### 5.7 User override

`RootOverride` records a user decision — add a root at a path, or remove a
detected one — persisted in repository config so it travels with the
repository and is reviewable in Git:

```text
project_roots_added=tools/scripts
project_roots_removed=examples/legacy
```

A rescan re-derives detection but never discards an override, and the map's
`detected_by` shows `UserOverride` for those entries so the UI can explain a
root the evidence does not support.

The flat `key=value` format matches the existing config style, so no new parser
is needed and a user can edit it by hand.

### 5.8 Persistence and refresh

The map persists per repository following the existing vector-index pattern
(`vector_index.rs:149-152`):

```text
<data_dir>/repository-map/<repository_id>.json
```

`schema_version` and `fingerprint` are recorded. A version mismatch, a corrupt
file, or a fingerprint mismatch rebuilds the map — a rebuild is cheap, and a
stale map is worse than no map because the agent trusts it.

Refresh is incremental: a change under a root updates that root, and only a
change to a manifest, an instruction file, or a directory structure triggers
re-detection. `RepositoryMap` is derived from the index, so it refreshes on the
same signals the index already receives from the watcher in `index_cache.rs`.

Note that `vector_index.rs` has `load`/`save` but no schema version of its own.
The map does not copy that omission.

### 5.9 UI

A roots list showing each root's path, languages, command count, and whether it
was detected or overridden. Selecting a root shows its evidence, its instruction
files, and its commands with their working directories.

Requirement 4's "the UI shows which roots a task touched" is satisfied by
grouping the task's changed files and command runs by `root_path` in the
completion report ([spec 23](../23_verification_loop.md)).

### 5.10 Documentation

`docs/USER_GUIDE.md`: what a root is, how detection works, how to correct it,
and why a command runs where it does. `docs/TROUBLESHOOTING.md`: where the map
is stored, how to force a rebuild, what to do when a root is missed or a
directory is wrongly detected, and how to read `detected_by`.

## 6. Acceptance Criteria

- Two runs over an unchanged repository produce identical map output, ignoring
  `generated_at_ms` — asserted by comparing full serialisations.
- The map stays under `repository_map_max_tokens` on a large fixture, and states
  what it dropped when it degrades.
- In a multi-root fixture, a validation command discovered in `packages/api`
  runs with `packages/api` as its working directory.
- Nested roots are both detected: a Cargo workspace root and its member crates
  each appear, each with their own commands.
- `node_modules`, `vendor`, `target`, `dist`, and `build` never appear as roots,
  and appear in `excluded` with a reason.
- A repository with no manifest still produces a map with one root.
- Roots below the depth ceiling are recorded in `excluded`, not silently
  dropped.
- Nested `AGENTS.md` files resolve per root, matching
  [spec 11](../11_agents_md_support.md)'s rules, with no second precedence rule
  introduced.
- A misdetected root corrected by the user persists across a rescan, and its
  `detected_by` reports the override.
- The same command text in two roots classifies per root, and an exact-command
  allowlist entry in one root does not authorise it in another — asserted by
  test.
- Search results and file displays qualify a basename by its root, so two
  `index.ts` files are distinguishable.
- A corrupt or version-mismatched map file rebuilds cleanly and reports that it
  did.
- A root cannot widen `path_policy.rs`.
- Every quality-gate command from `AGENTS.md` passes, and the
  [spec 18](../18_local_evaluation_harness/proposal.md) baseline shows no
  regression.

## 7. Implementation Notes

Written in Task 9 (2026-10-09). The per-task record is
[`tasks.md`](tasks.md)'s progress table. The decisions that replaced parts of
§5 are in [`context.md`](context.md).

### 7.1 Acceptance criteria and the tests that prove them

Engine tests are in `crates/workspace-engine/tests/repository_map.rs` unless
another file is named. Shell tests are in the `tests` module of
`crates/desktop-shell/src/lib.rs`.

| §6 criterion | Proved by | State |
|---|---|---|
| Two runs over an unchanged repository produce identical output, ignoring `generated_at_ms` | `two_builds_of_an_unchanged_repository_serialise_identically` (full serialisations with `generatedAtMs` removed, one build from a reversed index); `detection_does_not_depend_on_input_order` | Met |
| The map stays under `repository_map_max_tokens` on a large fixture and states what it dropped | `sixty_long_roots_stay_under_the_default_ceiling_and_say_what_was_dropped`; `each_degradation_step_fires_in_order_as_the_ceiling_falls`; `a_small_map_is_rendered_whole_with_no_degradation_line` | Met |
| A validation command discovered in `packages/api` runs with `packages/api` as its working directory | `detected_validations_are_proposed_and_run_at_their_own_root`; `a_proposal_in_a_sub_root_runs_in_that_directory`; `a_requested_working_directory_routes_the_proposal_to_that_root` (the model-requested path, Task 7) | Met |
| Nested roots are both detected, each with its own commands | `nested_roots_are_kept_and_each_names_its_manifest`; `nested_roots_each_carry_their_own_metadata_and_commands` | Met |
| `node_modules`, `vendor`, `target`, `dist`, `build` never appear as roots and appear in `excluded` with a reason | `vendor_and_build_output_directories_are_never_roots_and_are_recorded` | Met |
| A repository with no manifest produces a map with one root | `a_repository_with_no_manifest_has_exactly_one_root` | Met |
| Roots below the depth ceiling are recorded in `excluded` | `roots_below_the_depth_ceiling_are_recorded_not_dropped` | Met |
| Nested `AGENTS.md` files resolve per root by spec 11's rules, with no second precedence rule | `instruction_files_resolve_per_root_broadest_first`, which reads spec 11's own `agent_instruction_paths` (`context.md` §14) | Met |
| A user correction persists across a rescan and `detected_by` reports it | `overrides_survive_a_rebuild_after_the_repository_changed`; `an_added_override_makes_a_root_the_user_named` (`UserOverride` evidence); `a_removed_root_stays_visible_and_its_files_go_to_the_enclosing_root`; shell `repository_roots_endpoint_writes_and_re_reads_an_added_root` | Met. A removed root keeps its manifest evidence and reports the correction in `userOverride` (Task 4's decision), so the UI can say "found by `package.json`, removed by you" |
| The same command classifies per root, and an exact allowlist entry in one root does not authorise it in another | `a_root_qualified_grant_authorises_its_directory_only`; `a_plain_allowlist_entry_matches_only_at_the_repository_root`; `a_roots_command_risk_honours_a_grant_qualified_with_that_root`; `allow_always_in_a_sub_root_writes_a_qualified_grant_under_the_repository_id`; `a_root_qualified_grant_authorises_the_command_in_its_root_only` (through the chat loop) | Met, by root-qualified grants (`context.md` §2), not by the per-repository allowlist §5.5 assumed |
| Search results and file displays qualify a basename by its root | `root_for_path_is_the_longest_root_that_is_a_segment_prefix` (the rule `app.js`'s `rootForPath` copies). Model-facing results already carry full paths (`context.md` §4) | Met, but the UI half has **no automated test**: the web UI has no JS test harness. Task 8 verified the chips (`index.ts · packages/api`), patch root tags and "Runs in" by hand in the running app |
| A corrupt or version-mismatched map file rebuilds cleanly and reports that it did | `a_garbage_map_file_is_rebuilt_as_corrupt_and_audited`; `a_map_from_another_schema_version_is_rebuilt_as_a_mismatch`; `a_manifest_change_rebuilds_the_map_as_stale` | Met |
| A root cannot widen `path_policy.rs` | `paths_resolve_from_the_working_directory_and_are_bounded_by_the_repository`; `a_working_directory_outside_the_repository_matches_nothing_and_needs_approval`; `a_refused_working_directory_never_stores_a_proposal`; shell `a_root_override_is_never_written_outside_the_allowed_roots` | Met |
| Every quality-gate command passes, and the spec 18 baseline shows no regression | The Task 9 gate run, recorded in `tasks.md` | **Not met on 2026-10-09.** `cargo nextest run --workspace --locked` fails: 25 `desktop-shell` tests abort with SIGABRT at the default test-thread stack (`OBSERVATIONS.md` #26, pre-existing, being fixed separately). `token_ceiling::a_turn_stops_before_the_call_that_would_cross_the_ceiling` also failed because of this spec, and is now fixed (§7.3). The other six gate commands pass. The eval tier passes, and its token rise is exactly the one Tasks 6 and 7 recorded |

Two requirements have no design in §5 and were **not built**
(`context.md` §12):

- **Frameworks** (requirement 1). Not met. Detecting a framework means reading
  a manifest's dependencies, which §4 rules out. `ProjectRoot` records
  `languages` and `manifests` and has no `frameworks` field.
- **Optional permission restrictions per root** (requirement 3). Not met. A
  permission profile is per repository (spec 31), and no scope narrower than a
  repository exists. A per-root restriction needs a new config scope, which is
  a spec 31 design change.

### 7.2 Recording questions

**Depth ceiling.** `MAX_ROOT_DEPTH = 6` path segments, a constant rather than a
config key (`context.md` §11). Added roots are not held to it.

**Root count and map size on the largest repository tested** (this one, the
only real repository measured). A fresh `damaian repo-map` on 2026-10-09, with
a scratch `DAMAIAN_DATA_DIR`, found **7 roots**: `.`, the five crates, and
`crates/eval-harness/fixtures/rust-workspace`. `excluded` held `node_modules`
and `target`, both `vendor`. The stored file is **6,710 bytes**. Task 5
measured 277 indexed files, a `Built` load of about 0.38 ms and a `Reused`
load of about 0.17 ms in release builds, so the on-demand design in
`context.md` §3 needed no revisit.

**Token default and degradation.** `repository_map_max_tokens` defaults to
**800**, spec 26's planned 5% share of the 16,000-token default context budget
(`context.md` §7). On this repository the map rendered whole at 465 tokens in
Task 6. Task 7 lengthened only the header sentence, so it still fits well
inside 800, and **no degradation step fired at the default**. Every step was
exercised only below the default or on synthetic fixtures. On this repository,
300 tokens drops entry points, test paths and major directories, and 150
tokens shows 3 of 7 roots (Task 6). The 60-root fixture fires every step at
800. No repository large enough to degrade at the default was available, so
the order of the steps is tested but not tuned on real data.

**Surprising roots.**

- **A test fixture is a root.** `crates/eval-harness/fixtures/rust-workspace`
  has its own `Cargo.toml`, so it is detected and proposes `cargo test`. That
  is the stray-manifest case §7 anticipated. It is correct by the detection
  rule, and `project_roots_removed` exists to correct it. No fixture heuristic
  was added. Such a rule would be a second notion of "project", which §5.2
  warns against.
- **The repository root's evidence reads `package.json`, not `Cargo.toml`.**
  Both sit at the root, and `package.json` is first in `PROJECT_MANIFESTS`.
  That order is `detect_project_commands`' own. The evidence is true but less
  telling than `Cargo.toml` would be for a Rust workspace.
- **The root's npm scripts are missed.** `detect_project_commands` matches only
  the script names `test`, `lint`, `typecheck`, `build` and `format`. This
  repository names its lint script `lint:web`, so `.` offers only `cargo test`.
  This is behaviour that predates the spec, and §4 says the function is called
  per root, not replaced.
- **The Cargo workspace root and each member all propose `cargo test`.** Seven
  identical command strings, each at its own directory. They are kept
  deliberately (Task 6). Spec 23 should expect overlapping per-root proposals.

**Does the manifest list need refining?** Not the list itself. Nothing beyond
the fixture was a false root, and nothing real was missed. The gap is in what a
manifest *yields*: exact script-name matching misses common variants
(`lint:web`, `test:unit`). Refining that changes `detect_project_commands` for
every caller, so it is left for a later spec rather than done here.

### 7.3 The `token_ceiling` failure this spec caused

`crates/workspace-engine/tests/token_ceiling.rs`,
`a_turn_stops_before_the_call_that_would_cross_the_ceiling`, asserts that a
turn under a 1,000-token ceiling makes exactly 3 model calls. With this spec,
it makes 2 (`left: 2, right: 3`). Every request now carries the
`repository_map` item and the longer `run_command` schema (Tasks 6 and 7), so
the estimated usage reaches 1,000 one call sooner. The ceiling still stops
*before* the call that would cross it, which is the property the test exists
for. Only the call count it was measured at has moved. Confirmed on
2026-10-09: with `repository_map_max_tokens: 0` in that test's config, all 13
`token_ceiling` tests pass. No per-task scoped run included this test file,
which is why Tasks 6 and 7 did not see it.

**Fixed in Task 9** by setting `repository_map_max_tokens: 0` in
`engine_with_ceiling`, the fixture every `token_ceiling` test uses. The test
was not changed to expect 2, for three reasons. The test is about where the
ceiling stops, not what a request costs. The eval tier already records the
map's per-request cost. And a count tied to the map's size would move again
whenever the rendering changed. The exact count of 3 still guards
`context.md` §3.3 of spec 21.

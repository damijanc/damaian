# Context: Repository Map and Monorepo Boundaries

Background for [`tasks.md`](tasks.md). This file records the places where
[`proposal.md`](proposal.md) (the flat spec, unchanged in substance) no longer
matches the code, checked on 2026-10-06. It also records the decisions the
proposal leaves open. Read it before Task 1. Do not "fix" any of these back to
the proposal's wording without re-reading the code they cite.

The proposal was written before specs 22, 31, 47 and 34 landed. Most of its
file references still point at the right functions, but at drifted lines, and
four of its claims about how the code behaves are wrong. Those four are §1–§4
below, and each one changes a task.

## 1. Commands already carry a working directory, and two things conflate it with the repository root

The proposal says `ProjectCommand` "has no directory, so the caller supplies
one and supplies the wrong one" (§5.1). That is true of `ProjectCommand`
(`command_policy.rs:50`), but the type that actually reaches execution already
has a directory:

- `CommandProposal.working_directory: String` (`validation.rs:17`), absolute,
  is stored with the proposal (`serialize_proposal`, `validation.rs:391`).
- `CommandRunner::run(command, cwd, ..)` (`command_runner.rs:187`) runs the
  shell with `.current_dir(cwd)` (`:234`) and records it on
  `CommandExecution.working_directory`.

So requirement 5 does not need new plumbing to the process. Every producer
passes the repository root today (`chat.rs:2312`, desktop
`/api/propose-command`, CLI `propose-command`, and
`propose_detected_validations`). What it needs is fixes for two places that
read the working directory *as* the repository root. Both are silent the day
a proposal's directory is a sub-root:

- **Allow Always would write the grant under the wrong repository.**
  `allow_command_always` derives the repository id from
  `PathBuf::from(&proposal.working_directory)` (`validation.rs:281`). For a
  command in `packages/api`, that is `repository_id_for_root(<repo>/packages/api)`.
  Config is loaded with the id of the repository root
  (`apply_repository_allowlist`, `config.rs:1771`), so nothing would ever
  read the grant. It fails safe, but the user's click does nothing.
- **The outside-root heuristic treats the directory as the boundary.**
  `CommandPolicy::classify(command, working_directory)` (`command_policy.rs:73`)
  passes the directory to `references_path_outside_root` (`:421`). That
  function joins relative tokens to it and checks containment against **it**
  (`token_escapes_root`, `:434`). In `packages/api`, `cat ../web/index.ts`
  would be flagged as "outside the selected repository" and escalated to
  approval, although it is inside the repository.

**Decision (Task 2):** a command location has two parts, the repository root
and the working directory, and every classifier and runner takes both.

- `CommandPolicy::classify_at(command, repository_root, working_directory)`
  is the new classifier. `classify(command, wd)` stays, as
  `classify_at(command, wd, wd)`, so every existing caller behaves exactly as
  today.
- Tokens resolve against the working directory. Containment is checked
  against the repository root. That is proposal §5.5's "a root cannot widen
  what paths are readable", stated as code: the boundary is always the
  repository root, never the root the command runs in.
- `CommandProposal` gains `repository_root: String`. A stored proposal without
  the field (every proposal written before this spec) loads with
  `repository_root = working_directory`. That equals today's meaning, because
  every existing proposal ran at the root.
- `CommandRunner::run_at` and `run_proposal` re-classify at the proposal's
  pair. `allow_command_always` takes the repository id from `repository_root`.

## 2. The allowlist is per repository, not per root, so §5.5's test fails today

Proposal §5.5 says `command_allowlist` "stays exact-command, so allowlisting
`npm test` in one root does not silently authorise it in another", and the
acceptance criteria require a test of it. The premise is wrong:

- matching is `configured_exact_matches(&config.command_allowlist, command)`
  (`command_policy.rs:133`, helper `:300`), on the command text alone;
- the per-repository grants (`command_allowlist.<repository_id>`) are folded
  into that flat list when config is loaded (`config.rs:1771`).

So once commands run in sub-roots, `npm test` allowed in `packages/api` would
also run unprompted in `packages/web`, where it may do something else. That is
the failure §5.5 describes as the thing it prevents.

**Decision (Task 2): a grant made in a sub-root is stored root-qualified, and
a plain entry matches only at the repository root.**

- Allow Always for a proposal whose working directory is `packages/api` stores
  the entry `cd packages/api && npm test` under the repository's id.
- At a working directory other than the repository root, `classify_pattern`
  matches only the entry qualified with that directory, and never a plain
  entry.
- At the repository root, matching is exactly today's.

Why this representation:

- **It cannot match by accident.** It contains `&&`, which
  `contains_shell_control` (`command_policy.rs:304`) rejects before the
  allowlist is consulted (`:119` precedes `:133`). So no command typed as
  `cd packages/api && npm test` can ever match the entry literally.
- **It means what it says.** It reads in the Settings policy view (spec 31)
  as the command it authorises, and needs no new key, no new config syntax
  and no change to spec 34's classes.

**What this narrows:** a machine-wide `command_allowlist` entry no longer
applies in a sub-root. That narrows a user-owned setting, but only for a
situation that does not exist today, because no command runs outside the
repository root before this spec. Nothing that runs unprompted today starts
prompting. The user guide must say this (Task 9).

## 3. The index is in memory only, and roots cannot be stamped on `FileRecord`

Proposal §5.6 adds `root_path` to `FileRecord`. §5.8 says the map persists
"following the existing vector-index pattern" and "refreshes on the same
signals the index already receives". Checked against the code:

- `RepositoryIndex` and `FileRecord` (`indexer.rs:22`, `:52`) derive no serde
  and are never written to disk. `IndexCache` (`index_cache.rs`) keeps them in
  a process-wide registry, rebuilt in full every five minutes
  (`get_or_build`, `:43`) and patched per path by the watcher
  (`apply_single_path_change`, `:147`). There is no subscriber hook.
- Roots are derived *from* the index. If each `FileRecord` carried its root,
  adding one `package.json` would have to re-stamp every record beneath it.
  The incremental path, `index_single_file` (`indexer.rs:222`), handles one
  file and knows no roots, so it cannot do that.

**Decisions:**

- **No `root_path` on `FileRecord` (Task 3).** `RepositoryMap::root_for_path(&self, path) -> &str`
  returns the nearest enclosing root by longest path-segment prefix. Display
  and grouping call it at the point of use. That gives proposal §5.6 its
  grouping, at the cost of one lookup, and needs no index change. Spec 26
  plans a `ContextItem.root_path` "from spec 24" (its line 166). That field
  should be filled by this function.
- **The map is a pure function of the index plus config (Tasks 3 and 5).**
  `RepositoryMap::build(&RepositoryIndex, ..)` reads `files` and `skipped`.
  It is called on demand from the cached index, which is already incremental.
  The persisted file is reused when its input fingerprint matches, and rebuilt
  otherwise. This replaces §5.8's per-root incremental update, which would
  need a watcher subscriber that does not exist. Task 5 measures the build on
  this repository and records the time, so the decision can be revisited with
  a number.
- **`fingerprint` hashes the inputs, not the output.** §5.1's field comment
  says "inputs", §5.3 says "computed over the sorted content". The input hash
  is the one that detects staleness. It covers the sorted indexed paths, the
  content hashes of every manifest and instruction file, the override keys and
  the schema version. Determinism (requirement 2) is tested separately, on the
  serialised output with `generatedAtMs` removed, as §5.3 says.
- **The walk already sorts** (`tree_walk.rs:67`). The map still sorts every
  collection itself, as §5.3 requires. It does not rely on the walker's
  order, because the watcher appends patched records at the end of `files`
  (`index_cache.rs:170-173`).

## 4. Search results to the model already carry full paths; basenames appear only in the UI

Requirement 6's risk is a display problem, as proposal §5.6 says. Checked:

- Every search result reaching the model carries the repository-relative path:
  `format_search_results` (`chat.rs:4687`), `search_content` matches
  (`path:line: text`), and context items (`--- kind: path ---`,
  `chat.rs:3639-3652`). Two `index.ts` files are already distinct there.
  Filtering a search by root is already possible through `search_content`'s
  `path_glob` (`navigation.rs:172`). **Decision:** no change to any
  model-facing search structure, and no change to `chat.rs` for requirement 6.
- In `app.js`, a file is shown by basename in one place only:
  `renderPinnedContextFiles` uses `fileBaseName` for the composer's pinned
  chips (around `app.js:925-957`). Everything else (the "Read N files"
  disclosure, findings, recovery and checkpoint lists, patch summaries) prints
  the full path. **Decision (Task 8):** a chip whose file lies in a root
  other than `""` reads `index.ts · packages/a`.

## 5. Where the map enters model context, and what touches `chat.rs`

The two questions that decide pairing are whether a task must edit
`build_model_prompt` and whether one must edit `chat.rs`.

**`build_model_prompt` — no task edits it.** It renders every
`ContextItem` generically as `--- {kind}: {path} ---` plus content
(`chat.rs:3612`, loop at `:3639-3652`). `ContextManager::build_context`
(`context_manager.rs:66`) produces those items for both the chat flow
(`chat.rs:753`) and the edit flow (`edit.rs:404`).

**Decision (Task 6):** the rendered map is one more `ContextItem` of kind
`repository_map`, added inside `build_context`:

- it goes after `agent_instruction` and `project_rule` and before
  `retrieved_file`, so under a tight budget it displaces retrieved files and
  never instructions;
- it is bounded by `repository_map_max_tokens` before it reaches the flat
  budget.

The system prompt is untouched, so the byte-identical system-prompt test and
spec 49's prefix guard (`tests/prompt_cache.rs`) are unaffected. `ContextManager`
gains the map builder at construction, in `workspace_engine.rs:66`. Neither
`chat.rs` nor `edit.rs` changes for this.

This collides with nothing that runs in parallel. Spec 26 will rewrite
`build_context`, but #26 depends on #24, so it starts after this spec is Done
and absorbs the `repository_map` item as its planned `ContextCategory::RepositoryMap`.
#55 and #49's reuse slice rewrite `build_model_prompt`, which this spec does
not touch.

**`chat.rs` — exactly one task edits it: Task 7.** It has three jobs, and none
of them can live elsewhere:

1. **The model cannot choose where a command runs.** The `run_command` tool
   takes only `command` and `reason` (`chat.rs:3993`). `CommandRequest`
   (`:3713`), `command_request_from_tool_call` (`:4421`) and the
   `DAMAIAN_COMMAND_V1` parser (`:4440`) carry nothing else. A map that tells
   the model "`npm test` runs in `packages/api`" without a way to say so
   invites `cd packages/api && npm test`. That is shell control, so it is High
   risk and always prompts. Task 7 adds an optional, repository-relative
   `working_directory` to the tool schema, the envelope (`WORKING_DIRECTORY:`)
   and `CommandRequest`.
2. **Two classification call sites read the directory as the root.** The
   resume path re-classifies with `Path::new(&proposal.working_directory)`
   (`chat.rs:955`), and `action_permission` classifies with
   `repository_root` (`:3158-3161`). Both move to `classify_command_at`. The
   proposal call at `:2312` moves to `propose_command_at`.
3. **Finding paths resolve against the repository root.** `record_findings`
   (`chat.rs:1343`) resolves `repository_root.join(&range.path)` (`:1355`).
   A command in `packages/api` printing `src/x.ts:3:5` would lose its range,
   or bind to the wrong file if `<repo>/src/x.ts` exists. See §8 for the
   rule. Task 7 calls a helper that lives in `finding.rs`.

Everything Task 7 needs from outside `chat.rs` lands earlier:
`classify_command_at` and `propose_command_at` in Task 2, and the path
resolution helper in Task 7 itself, in `finding.rs` and `validation.rs`. That
keeps the `chat.rs` diff to call sites and argument plumbing.

## 6. User overrides: the keys and their class

Proposal §5.7 persists overrides in repository config
(`project_roots_added=…`, `project_roots_removed=…`) so they "travel with the
repository". Since spec 34 and spec 31, repository config is untrusted input,
and every new overlay field must be classified explicitly
(`AGENTS.md`, "Repository configuration is untrusted input"). The proposal
predates that and never classifies them.

**Decision (Task 4): both keys are preference keys (Free at repository scope).**
A root grants nothing that a committed manifest does not already grant. A
repository can make `tools/scripts` a root today by committing
`tools/scripts/package.json`, which costs the same as committing the config
line. A root changes which directory an approved command runs in. It never
changes whether approval is needed (§1, §2), and never what paths are
readable (§1). So the keys meet spec 31's preference rule
(`31_permission_profiles/context.md` §2).

Consequences the task must handle:

- **Spec 31's partition tests pin the Free set exactly.**
  `the_preference_keys_are_exactly_spec_34s_free_keys` (`tests/permission_profiles.rs:103`)
  and `every_preference_key_applies_from_repository_scope` (`:436`) list
  seven keys. Task 4 adds the two keys to both, and a `PreferenceCase` each.
  This is a legitimate change to the Free set, not a weakening. The file
  `repository_config_trust.rs` stays unmodified.
- `classify_overlay_fields!` (`config.rs:2165`), both `to_policy_text`s,
  `ConfigOverlay::set`, `apply_overlay_scoped`'s destructure, and
  `split_profile_keys` (`profile.rs:322`, a profile never carries a
  preference key) are all exhaustive, so each fails to compile until it is
  handled. `rule_label` (`effective_policy.rs`) needs a plain-language name
  for each key, or `every_rule_has_a_human_name` fails.
- **Values are validated when the map is built, not when config is parsed.**
  An entry must be repository-relative, normalised, free of `..`, not under a
  vendor directory, not restricted by `restricted_patterns`, and must name a
  directory that holds at least one indexed file. Anything else is not
  applied. It is recorded in `excluded` with reason `invalidOverride`. That
  makes it visible to the user, where a parse error would only reach spec 34's
  notice.
- **The repository root cannot be removed.** `project_roots_removed=` naming
  `""` or `.` is recorded as `invalidOverride`, because §5.2 says the map is
  never empty.
- **The UI and CLI write repository config**, `<repo>/.damaian/config.conf`
  (`Config::repository_config_path`, `config.rs:768`), as the proposal says.
  That file is in the user's working tree, so `git status` shows it. The UI
  must say so before writing. If the existing file has a line the strict
  parser rejects, the write is refused with that error, so a hand-edited file
  is never rewritten. The keys are also accepted at user scope, where they
  apply to every repository. That is allowed, and the user guide says so.
  Nothing writes them there.

## 7. `repository_map_max_tokens` is a capability key

§5.4 adds `repository_map_max_tokens` to `Config` without classifying it.
Spec 34 §5.2 classed the seven read and search caps as lower-wins because
"they bound how much of the repository leaves the machine per call", and
spec 31 made them capability keys for that reason. The map ceiling bounds
exactly that.

**Decision (Task 6):** a capability key, lower-wins at repository scope, using
the same merge as `max_read_lines`. It also gets a Task 1-style weakening case
in `tests/permission_profiles.rs` and a `classify_overlay_fields!` entry. The
default is **800**, which is spec 26's planned 5% share
(`context_budget.repository_map=0.05`) of `DEFAULT_CONTEXT_TOKEN_BUDGET`
(16,000, `config.rs:13`). A value of `0` turns the map off for model context.
A repository can therefore disable the map, which is a narrowing, but cannot
enlarge it. The map stays available to the UI and CLI either way.

## 8. Finding paths from a command run in a sub-root

The spec 22 parsers keep the printed path as-is. `workspace_range`
(`finding/rust_diagnostics.rs:136`) only rejects absolute paths and `..`. They
do not know the working directory, and they should not: the printed path's
base differs by tool.

- `npm`/Biome in `packages/api` print `src/x.ts`, relative to the working
  directory.
- `cargo` in a workspace member prints paths relative to the **workspace
  root**, because cargo runs rustc from there. In `crates/engine`, the path
  is `crates/engine/src/lib.rs`, not `src/lib.rs`.

**Decision (Task 7):** `resolve_finding_path(repository_root, working_directory, printed) -> Option<String>`,
in `finding.rs`. It tries the printed path against the working directory, then
against each ancestor up to and including the repository root, and takes the
first that names an existing file inside the repository. It returns that
file's repository-relative path, which becomes the finding's range path.
`None` keeps today's behaviour (`without_range`). Cwd-first is right for tools
that print cwd-relative paths. The ancestor walk covers cargo, and a
false match would need `crates/engine/crates/engine/src/lib.rs` to exist.

## 9. Spec 23 consumes `propose_detected_validations`, which becomes per-root

Spec 23 (not started) reuses `ValidationOrchestrator::propose_detected_validations(working_directory)`
(`validation.rs:199`) as its only discovery mechanism, by its own non-goal
("reused, not replaced or supplemented"). It assumes one working directory,
and its line citations are stale (`:167` there, `:199` now).

**Decision (Task 5):** `propose_detected_validations(repository_root, &RepositoryMap)`
proposes every root's commands, each at its own working directory. Spec 23
then gets per-root checks by calling the function it already planned to call.
It must pass a map. The CLI `propose-validations` and the
`foundation.rs` test move to the new signature. `CommandPolicy::detect_project_commands(root)`
keeps its single-directory meaning for the CLI's `detect-commands`. Its body is
shared with the per-root detection, so there is still one manifest table
(proposal §4, last non-goal).

If spec 23 is planned or built in parallel with this spec, agree which side
adapts to the signature change. `README.md` "What to build next" records this.

## 10. The manifest list is shared by construction, not by convention

§5.2 says roots use "the manifests `detect_project_commands` already knows".
Today that knowledge is inline: a `package.json` branch and a six-row table
(`command_policy.rs:221-249`).

**Decision (Task 1):** extract `pub const PROJECT_MANIFESTS: [&str; 7]`
into `command_policy.rs`. Root detection reads it. A test asserts that every
entry yields at least one command from `detect_project_commands` (a
`package.json` fixture with a `test` script). So adding a manifest to the root
list without giving it a command fails, which is the drift §5.2 warns about.

## 11. Exclusions: what is recorded, and what is not

§5.2 says vendor and generated directories "and the configured ignore
patterns" are excluded and recorded in `excluded` with the reason. The walker
reports an ignored entry as `SkippedFile { reason: "ignored" }` and does not
descend (`tree_walk.rs:82-88`). It reports directories and files the same way,
and `.gitignore` rules are applied per directory, so a large repository has
thousands of ignored entries.

**Decision (Task 1):** `excluded` records three things and nothing else:

- `vendor`: an ignored entry whose last segment is one of
  `VENDOR_DIRECTORIES` (`node_modules`, `vendor`, `target`, `dist`, `build`,
  `.venv`, `venv`). Also any manifest found beneath such a segment, which
  happens when a user has removed `node_modules/` from `ignore_patterns`. That
  manifest is recorded once, under the vendor directory's path.
  `VENDOR_DIRECTORIES` is a fixed list, independent of `ignore_patterns`,
  because §5.2 says those directories are *never* roots.
- `belowDepthCeiling`: a manifest directory deeper than `MAX_ROOT_DEPTH`.
- `invalidOverride`: Task 4's rejected override entries (§6).

Other ignored paths are not listed. They were never candidates, and listing
them would make `excluded` grow with the repository, which requirement 2's
ceiling exists to prevent. The user guide says that `.gitignore`d directories
are never roots (Task 9).

**Depth ceiling:** `MAX_ROOT_DEPTH = 6` directory segments (`""` is depth 0,
`packages/api` is 2). It is a constant, not a config key, so no key has to be
classified. Six is twice the depth of the deepest conventional layout
(`packages/<scope>/<name>`). Proposal §7 asks for the chosen value and the
measured root count, which Task 9 records.

## 12. What the proposal asks for that its design does not provide

Two items in requirements 1 and 3 have no design in §5. Neither is built
here. They are named so the close-out reports them as unmet, rather than
leaving them quietly absent (`docs/specs/README.md`, "A dependency marked
*not built* must be answered", applied to requirements):

- **"Frameworks"** (requirement 1). Detecting a framework means reading a
  manifest's dependencies, and proposal §4 rules out parsing manifests for
  dependency graphs. `ProjectRoot` has `languages` and `manifests`, and no
  `frameworks` field.
- **"Optional permission restrictions"** per root (requirement 3). A profile
  is per repository (spec 31, `permission_profile.<repository_id>`), and no
  scope narrower than a repository exists. A per-root restriction needs a new
  config scope, which is a spec 31 design change.

One field the design omits is added, because requirement 1 lists it and §5.4
degrades it. `ProjectRoot` gains `major_directories: Vec<String>`: the
root's immediate child directories that are not themselves roots, sorted.

## 13. Requirement 4 without spec 23's completion report

§5.9 meets requirement 4 ("the UI shows which roots a task touched") by
grouping in spec 23's completion report, which does not exist. Spec 23 is a
cross-reference here, not a declared dependency, so this spec must not wait
for it.

**Decision (Task 8):** every action the UI already shows names its root:

- the command approval card shows "Runs in `packages/api`" whenever the
  working directory is not the repository root;
- each file in a patch review is tagged with its root when that root is not
  `""`.

A turn that touches two roots therefore shows both on the cards it produces.
Spec 23's report will roll these up later.

## 14. Instruction files per root reuse spec 11's function

Requirement 8 and the criterion "nested `AGENTS.md` files resolve per root,
matching spec 11's rules" need one source of truth.

- Spec 11's ancestor walk is `agent_instruction_paths` (`context_manager.rs:284-312`).
  The proposal cites `:280-309`.
- It is private. Precedence ("more specific overrides broader") is stated only
  in the system prompt (`chat.rs:3580`), not enforced by merging.

**Decision (Task 3):** make `agent_instruction_paths` `pub(crate)`. A root's
`instruction_files` are its result for a path inside that root, kept when the
index holds the file, in the order it returns (broadest first). Context
loading is unchanged: instructions still enter context through spec 11's
context-path rule. The map lists a root's files so the model and the user can
see them. It does not add them to context, which would be a second inclusion
rule.

## 15. Things the proposal says that were checked and hold

- `detect_project_commands` checks `package.json` scripts
  `test`/`lint`/`typecheck`/`build`/`format` by substring, then the six-row
  table, in one directory, without descending (`command_policy.rs:215-260`).
- Risk comes from `self.classify(&command, root)` (`:228`, `:237`, `:254`).
  The root only feeds the outside-root heuristic (§1). Everything else in the
  classification ignores it.
- `vector_index_path` is `<data_dir>/vector-index/<repository_id>.bin`
  (`vector_index.rs:149-153`), with `load`/`save` at `:22`/`:29` and no schema
  version.
- `detect_language(path) -> &'static str` (`language.rs:3`) matches on the
  extension and falls back to `text`.
- `FileRecord.path` is repository-relative and `/`-separated (`indexer.rs:24`),
  so two `index.ts` files are distinct keys.
- Path policy evaluates against the repository root
  (`PathPolicy::canonical_root`, `path_policy.rs:41`; `resolve_existing`,
  `:58`). No command path consults it today. Task 7's `working_directory`
  argument is resolved through `resolve_existing`, so a requested directory
  outside the repository, through a symlink or into a restricted path is
  refused before any proposal is stored.
- Config lists are `|`-separated, and a repeated key overwrites the earlier
  one (`split_list`, `config.rs:3440`). The proposal's one-value examples are
  valid. Several roots are `project_roots_added=a|b`.

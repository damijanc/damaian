# Client comparison: Damaian against opencode

How we compare Damaian with an established coding client on the same model, and
what we measure. This page is the method; results go in the results log
described at the end. It is written so the comparison can be repeated on any
Mac, by anyone, without the conversation that designed it.

Agreed on 2026-11-16. Nothing here has been run yet; the first run will correct
whatever this page gets wrong, and the correction belongs here.

## Why opencode, and what "as good as" means

The goal is for Damaian to be as good as an established client.
[opencode](https://opencode.ai) is the reference because it can run **the same
model** as Damaian. With the model held constant, any gap belongs to the
harness: system prompt, tool set, tool output, context assembly and loop
control. opencode is open source, so when a gap shows up, its tools and prompts
show one way to close it. Study them for ideas and write Damaian's own version;
do not copy its code.

Damaian deliberately does things opencode may not: approval stops, secret
redaction, patch hash checks and the command policy. Parity is judged on:

- **Outcome**: the task is done, with meaningful tests, inside its scope.
- **Efficiency**, with the cost of those guarantees measured separately so that
  it is visible and cannot hide a real inefficiency.

A gap is never closed by weakening a guarantee. If an approval stop is
expensive, make it cheaper rather than removing it.

## Controls

These hold for every run on both sides.

- **Same model, same settings.** Record the exact model ID as each client names
  it, plus temperature and reasoning effort where they are configurable.
  Provider model names drift; a bare "DeepSeek" is not enough.
- **Pinned opencode version.** Record it with every run, and do not upgrade in
  the middle of a comparison round, or the reference moves.
- **Same base commit and same prompt.** Use the neutral worker prompt from
  [AGENTS.md](../AGENTS.md) ("Implement Task N from …"). Both clients read the
  same `AGENTS.md`.
- **One isolated checkout per run.** Damaian also gets its own
  `DAMAIAN_DATA_DIR`, so it never reads or writes your real sessions.
- **Several runs per side.** Model output is not deterministic. One run per
  side is an anecdote; plan on at least three.
- **Approvals.** Approve whatever the task legitimately needs, and count each
  stop (see [Human cost](#human-cost)). Do not count your time spent
  answering a stop as Damaian's time.

## Choosing tasks

- **One task from a `tasks.md`, not a whole spec.** A medium task touching two
  to four files, with its own scoped tests, gives a clearer signal than a
  whole spec, where one early wrong turn can decide the result.
- **Replayed tasks.** Re-run a task that is already Done, starting from the
  commit just before it landed. The landed change is a reference solution.
  These form a fixed set, used as a regression check between rounds of fixes.
- **Live tasks.** Run both clients on the next ready spec task (see
  [What to build next](specs/README.md#what-to-build-next-and-what-may-run-in-parallel)).
  Keep one result, normally Damaian's if it is acceptable, so the work still
  moves forward. Discard the other after scoring. Live tasks keep Damaian from
  being tuned to the replay set.

### Replays must not see the answer

A worktree of this repository can still reach `main`, so either client could
find the reference solution with `git log` or `git show`. Replays run in a
fresh clone that contains nothing after the base commit:

```sh
git clone --no-local --single-branch --branch main <this-repo> "$DIR"
git -C "$DIR" reset --hard <base-commit>
git -C "$DIR" reflog expire --expire=now --all
git -C "$DIR" gc --prune=now
git -C "$DIR" remote remove origin
```

Before starting the run, confirm that `git -C "$DIR" log --all --oneline` ends
at the base commit.

## What we measure

One scorecard per run. Every metric is recorded on both sides.

### Outcome

- **Passes**: scoped tests, scoped clippy and fmt pass on the result.
- **Rework**: the edits you would have to make before accepting the diff.
- **Scope**: files touched outside the task, unrelated refactors, unrequested
  changes.
- **Records**: `tasks.md` checkboxes and progress rows, and the spec, are
  updated and truthful.
- **Replays only**: how the diff compares with the reference solution.

### Tests the client wrote

A client that only re-runs existing tests can look disciplined while leaving
the new behavior unchecked, which has happened before.

- **Written**: tests added or changed. If none, did the task need any? Its
  acceptance criteria usually say.
- **Coverage**: each acceptance criterion maps to a test that exercises it.
  Count the criteria with none.
- **Discriminating**: run the new tests against the pre-change code after the
  run. A test that passes there proves nothing.
- **Weakened**: loosened assertions, added `#[ignore]`, deleted tests, or
  expected values changed to match the new output. Count these separately;
  each one is a red flag.
- **Conventions**: `enable_index_watcher: false` for temporary repositories,
  `#[ignore]` with a run command for tests with side effects, tests placed
  next to the behavior they cover.

### Test and build runs

Their **time is excluded** from the timing below; their **count and judgment**
are recorded. For each run, note what changed since the previous one, how wide
it was (workspace, crate, test file or single test), the result, and whether
the client's next action used it. Then classify it:

- **Needed**: the first run after a meaningful edit to code the tests cover; a
  run confirming a fix for a failure already seen; the final check before
  claiming completion.
- **Redundant**: nothing changed since the previous run; the change could not
  affect it (only docs or `tasks.md` edited); the output was ignored or
  truncated and never investigated.
- **Too broad**: a workspace run where one crate would do, or a crate where one
  test file would do. `AGENTS.md` asks for scoped runs, so this also measures
  whether the client follows repository instructions.
- **Missing**: places where a run should have happened and did not, such as
  claiming completion with no run after the last edit. Without this count,
  doing fewer runs can hide checking less.

`cargo clippy` and `cargo build` are treated the same way.

### Effort

- **Time**: wall time minus test and build time, split into model time and
  tool time.
- **Rounds**: model requests.
- **Tokens**: input, output and cached, and the cache hit rate. The cache rate
  depends on how stable the prompt prefix is and changes the cost
  considerably.
- **Peak context**: the largest single request, and whether compaction ran.
- **Exploration before the first edit**: rounds and tokens spent before
  anything is changed, against those spent after.
- **Redundant reads**: the same file, or the same range, read twice.

### Tools against the model

- **Done by tools against done by the model**: what each client did through a
  purpose-built tool, and what the model had to do itself.
- **Failed tool calls**: bad arguments, patch-apply failures, hash mismatches,
  commands blocked by policy. Each costs a round.
- **Retry loops**: test, fail and fix cycles before the tests pass.
- **Shell fallbacks**: where a client used the shell (`grep`, `sed`, `cat`)
  instead of a tool.

Sort every gap into one of four kinds, because each has a different fix:

1. **Missing**: Damaian has no such tool. This makes a candidate new tool.
2. **Unused**: the tool exists but the model works around it. This is a tool
   description or prompt problem.
3. **Blocked**: the tool exists but the command policy gets in the way.
4. **Wasteful**: the tool is used but returns too much or too little, such as
   a whole file where a range would do, or untruncated test output.

### Human cost

- **Interventions**: approval stops, questions to you, and the times you had to
  unstick the client.
- **Stalls**: the run stopped, or claimed it was finished before it was.

## Running a round

1. **Check capture.** Before the first round on a machine, confirm that each
   client's session record yields every metric above: Damaian's session and
   audit records (locations in [Troubleshooting](TROUBLESHOOTING.md)), and
   opencode's session export. Write down here how each metric is extracted the
   first time it is worked out. Any metric Damaian cannot produce is itself a
   gap to fix.
2. **Pilot.** Run one or two replays to test the scorecard rather than the
   clients. Fix metrics that cannot be captured or judged consistently.
3. **Head to head.** Run live tasks one at a time, both clients from the same
   base commit with the same prompt.
4. **Fix and repeat.** Fix the largest gap, then re-run the replay set and the
   latest live task. A fix that does not move the metric it targeted was a
   wrong diagnosis. Spec 47 shows how a plausible fix can make things worse.

Do not pause the comparison to finish other specs first: the next spec tasks
are the benchmark. Only a defect that would decide the comparison by itself,
such as a worker stall or crash, is worth fixing before measuring.

## Running on another Mac

- Install opencode at the version pinned for the round, not the latest.
- Provider credentials go in the macOS Keychain or an environment variable,
  never in a file in this repository (see [SECURITY.md](../SECURITY.md)).
- Compare token counts, rounds and test-run judgments across machines. Do not
  compare wall time across machines; it is only meaningful between the two
  clients on the same machine in the same session. On the development Mac,
  process launches stall intermittently (see
  [the quality gate is slow locally](DEVELOPMENT.md#the-quality-gate-is-slow-locally)),
  which is one reason test and build time is excluded.

## Results

Each round's scorecards go in a results log next to this page, one dated
section per round. Each section records the machine, both client versions, the
model ID and settings, the tasks, and the scorecards. Identify the measured
Damaian version by release version or branch, and a replay's base commit by the
subject line of the commit it precedes, not by hash: hashes break when history
is rewritten. Gaps found in a round go into the local `OBSERVATIONS.md` when it
exists, and into a spec once a fix is concrete, so they outlive the round.

The rubric is versioned by this page's history. When a metric is added, earlier
rounds remain comparable on the metrics they share.

The same task set and scorecard are meant to be reused for the provider axis of
the evaluation harness's live tier, when more providers are added to Damaian.

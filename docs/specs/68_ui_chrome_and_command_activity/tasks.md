# Quieter Chrome and Readable Command Activity — Task List

**Implements:** [`proposal.md`](proposal.md) · agreed look in [`mockup.html`](mockup.html)
**Style reference:** [`../../UI_STYLE_GUIDE.md`](../../UI_STYLE_GUIDE.md) · rendered at `docs/ui-style-guide.html`
**Started:** 2026-10-29 — **Done:** 2026-10-29

> **For agentic workers:** use superpowers:subagent-driven-development or
> superpowers:executing-plans to work through this list task by task. Steps use
> checkbox (`- [ ]`) syntax; tick each step when it lands (AGENTS.md).

**Goal:** Build proposal §3's eighteen requirements: a turn's usage row is drawn
once, command steps are readable, and the sidebar, header and composer show
only chrome that has something to say.

**Architecture:** The work is all client-side presentation in
`crates/desktop-shell/static/{app.js,style.css,index.html}`. The one Rust edit
is test-only: it seeds the `serves_the_ui_for_manual_inspection` fixture so
every new state can be inspected. Nothing that the engine stores or sends
changes.

**Tech stack:** vanilla JS (no modules, no bundler; top-level functions are
globals), CSS custom properties, Biome, Rust test fixtures.

## Global constraints

- **Each task is its own agent session** (AGENTS.md). Read this file's Global
  constraints, the task, and the proposal sections it names.
- **Order:** Task 1 is Rust-only and may run beside Task 2. Tasks 2 → 6 all edit
  `app.js` and `style.css`, so they run strictly in sequence. Task 7 runs last.
- **Presentation only** (proposal §1, §4). No API route, stored message, setting
  key, approval decision or engine string changes. If a step seems to need one,
  stop and record it in the Progress row instead.
- **There is no JS test suite** (style guide §9), and this spec does not add one
  (#67 §4). A "test" step here is a set of `console.assert` lines, run in the
  Browser pane's console against the inspection app. A step passes when the
  console prints no `Assertion failed`. The functions are globals, so the
  console can call them.
- **Running the inspection app:**
  `cargo test -p desktop-shell --lib -- --ignored --nocapture serves_the_ui`,
  run in the background with local binding allowed. It serves port 4899 with
  its own data dir. In the console:
  `apiToken = "damaian-ui-inspection-token"; setRepository("<printed repo>", false)`.
  Static assets are `include_str!`-embedded, so after every asset edit, stop the
  test by its task/PID and start it again. Never touch port 4765, and never kill
  a process by name.
- **Scoped checks before a task is done:** `node --check
  crates/desktop-shell/static/app.js`, `npm run lint:web`, `typos`. Task 1 also
  runs `cargo clippy -p desktop-shell --all-targets --locked -- -D warnings`.
  The full seven-check gate runs on CI (AGENTS.md, policy 2026-10-15).
- **Style guide in step with the code.** A task that changes a rule the guide
  states updates that guide section and its specimen in the same change.
- **Tokens only** (style guide §2): no literal colours except `rgba(0,0,0,.05)`
  for icon-button hover, which the guide gains in Task 5.
- **Commits:** ask the user before committing (they decide). The message is one
  subject line describing the effect. Never push.

## Review focus

The inputs most likely to bite that no requirement names. Each is pinned by a
test in the task that owns the code.

1. **A command whose text contains `` ` — `` or backticks.** Reason extraction
   must strip the exact `` Ran `<command>` — `` prefix, not split on the first
   `` ` — ``. → Task 3, `commandReason` asserts.
2. **stdout that itself contains `\n\nSTDERR:\n`** (for example `cat` of a log
   that holds a result). The split is at the *last* marker, so stdout keeps
   that text. → Task 3, `parseCommandResult` asserts.
3. **stderr whose last line starts `` Hook ` `` but was written by the
   command**, not a hook. This is accepted: proposal §5.1 strips only a single
   trailing line after `\n\n`. A mid-stream `Hook` line stays in stderr.
   → Task 3 asserts both.
4. **Exit `-1` and `none (cancelled by user)`.** They read `exit -1` and
   `cancelled`, never `✓`. → Task 3 asserts.
5. **The same command run twice in one turn**, first approved, then auto-run.
   Only the step immediately after the request is marked *approved*.
   → Task 1 fixture, Task 3 walk.

---

### Task 1: Inspection fixture for command activity

**Proposal:** §2, §3 req. 18.

**Files:**
- Modify: `crates/desktop-shell/src/lib.rs`, inside `fn
  serves_the_ui_for_manual_inspection` (near line 8303). It is test-only.

**Interfaces:**
- Produces a session titled **"Command activity"** in the fixture repository,
  printed as `command activity session = <id>`. Task 3 and Task 7 walk it.

- [x] **Step 1: Seed one task with these messages, in order**, using the
  fixture's existing `append_message(&session.id, Some(&task.id), role,
  content)` pattern. `<root>` is the fixture repo path; `<cause>` is
  `ApprovalCause::ShellControl { constructs: "`|`".into() }.plain_words()`.
  1. `user`: `List the build output and check the crate.`
  2. `assistant`: ``Ran `ls target` — See what the last build left.``
  3. `tool`: `Command: ls target\nWorking directory: <root>\nExit code: 0\n\nSTDOUT:\ndebug\n\nSTDERR:\n`
  4. `assistant`: `<cause>\n\n` + `` `cargo check 2>&1 | tail -5` ``. This is
     the approval request; it must contain the command verbatim.
  5. `assistant`: ``Ran `cargo check 2>&1 | tail -5` — Compile the crate.``
  6. `tool`: `Command: cargo check 2>&1 | tail -5\nWorking directory: <root>\nExit code: 0\n\nSTDOUT:\nFinished dev\n\nSTDERR:\n`
  7. `assistant`: ``Ran `cargo check 2>&1 | tail -5` — Check again.``
  8. `tool`: same as 6. This is review focus 5.
  9. `assistant`: `<cause>\n\n` + `` `rm -rf target | cat` ``
  10. `assistant`: ``Ran `rm -rf target | cat` — Clean the build.``
  11. `tool`: ``The user declined to run `rm -rf target | cat`. Do not request it again; answer using what you already know, noting the limitation if it matters.``
  12. `assistant`: ``Ran `sleep 999` — Wait for the server.``
  13. `tool`: `Command: sleep 999\nWorking directory: <root>/sub\nExit code: none (timed out)\n\nSTDOUT:\n\n\nSTDERR:\nslow\n\n`, followed by ``Hook `fmt-guard`: refused after the action.``
  14. `assistant`: ``Ran `echo "a ` — b"` — Quote test.``
  15. `tool`: `Command: echo "a ` — b"\nWorking directory: <root>\nExit code: 1\n\nSTDOUT:\nx\n\nSTDERR:\nHalf\n\nSTDERR:\nreal`
  16. `assistant`: `` Read `src/lib.rs`. ``
  17. `tool`: `fn main() {}`. This does not parse, so it falls back.
  18. `assistant`: `The crate builds; the clean was declined.` This is the answer.
- [x] **Step 2: Record usage on the task** the way the fixture already does
  for "Add retry to the upload client", so the usage row exists.
- [x] **Step 3:** `cargo clippy -p desktop-shell --all-targets --locked -- -D
  warnings` passes, and the test starts and prints the new session id.
- [x] **Step 4:** In the running app, as it is today: the session shows
  requests 4 and 9 as bubbles, and the usage row repeats under each of them.
  Note the count in the Progress row as the "before" figure.

### Task 2: Usage row once per turn, with duration

**Proposal:** §3 req. 1 and 6, §5.3.

**Files:** `crates/desktop-shell/static/app.js`: `renderMessages` (~6177),
`markMessageUsage` (~9271), and the live `done` handlers that call
`markMessageUsage` (~11221, 11411, 11473, 11929, 12097, 12220, 12464, 13123,
13810, 13944, 14049).

**Interfaces:**
- Produces `turnDurationMs(taskId: string, messages: Message[], pausedTaskIds:
  Set<string>) -> number | null`.
- Produces `markMessageUsage(target, usage, durationMs = null)`. It appends
  `` · ${formatElapsed(durationMs)} `` when `durationMs` is a number.
- Produces `pausedTaskIds`, computed in `renderMessages`: tasks holding a
  `question` message, or an assistant message that is an approval request.
  Until Task 3 lands, a request is any assistant message, not `endsTheTurn`,
  that is followed in the same task by an assistant starting ``Ran ` ``. Task 3
  replaces this with its absorption predicate.

- [x] **Step 1: Console asserts (expected to fail before the change)**

```js
const m = (taskId, createdAtMs, role = "assistant") => ({ taskId, createdAtMs, role });
console.assert(turnDurationMs("t", [m("t", 1000, "user"), m("t", 135000)], new Set()) === 134000);
console.assert(turnDurationMs("t", [m("t", 1000)], new Set()) === null);
console.assert(turnDurationMs("t", [m("t", 1000, "user"), m("t", 9000)], new Set(["t"])) === null);
console.assert(turnDurationMs("t", [m("t", undefined, "user"), m("t", 9000)], new Set()) === null);
```

- [x] **Step 2: Implement `turnDurationMs`.** Move the `renderMessages` call to
  `markMessageUsage` inside the `endsTheTurn` block, and pass the duration.
- [x] **Step 3: Live paths.** Pass the indicator's elapsed time only when the
  turn was not resumed from a pause; resume paths pass `null`. Expose the
  started-at time from the indicator (`startedAt`, ~5909) rather than
  re-timing.
- [x] **Step 4:** Reload the Task 1 session. Exactly one usage row shows, under
  message 18, and it has no duration because the task paused. "Add retry to the
  upload client" also shows one row.
- [x] **Step 5:** Run the scoped checks, then fill in the Progress row.

### Task 3: Command steps, hook notes and absorbed approvals

**Proposal:** §3 req. 2–5, §5.1, §5.2.

**Files:** `app.js`: `isGroupableAction` (~6125), `renderActivityGroup`
(~6137), and the grouping walk in `renderMessages` (~6243). Add the CSS in
`style.css`, next to the `.activity-*` rules.

**Interfaces:**
- Consumes `createDisclosure(label, panel)`, `repo()` and Task 2's
  `pausedTaskIds`. Every stored string enters the DOM through `textContent`,
  except the action title, which keeps #67's `renderMarkdown`.
- Produces `parseCommandResult(text: string) -> { command, workingDirectory,
  exit: { state: "ok"|"failed"|"timed_out"|"cancelled", code: number|null },
  stdout, stderr, hookNote: string|null } | null`.
- Produces `commandReason(actionContent: string, command: string) -> string`.
  It returns the text after the exact prefix `` Ran `${command}` — ``, or `""`.
- Produces `approvalRequestFor(message, next, nextResult) -> boolean`,
  proposal §5.2's four conditions. Task 2's interim predicate is replaced by:
  a task is paused if it holds a `question`, an absorbed request, or a request
  bubble that matched every condition except the parse.
- CSS: `.activity-command`, `.activity-command-text`,
  `.exit-mark[data-state="ok"|"failed"|"timed_out"|"cancelled"]`,
  `.activity-approved`, `.activity-hook-note`, `.activity-request`.

- [x] **Step 1: Console asserts**

```js
const R = (cwd, exit, out, err) =>
  `Command: c\nWorking directory: ${cwd}\nExit code: ${exit}\n\nSTDOUT:\n${out}\n\nSTDERR:\n${err}`;
let p = parseCommandResult(R("/r", "0", "a", ""));
console.assert(p.exit.state === "ok" && p.exit.code === 0 && p.stdout === "a" && p.stderr === "");
p = parseCommandResult(R("/r", "-1", "", ""));
console.assert(p.exit.state === "failed" && p.exit.code === -1);
console.assert(parseCommandResult(R("/r", "none (timed out)", "", "")).exit.state === "timed_out");
console.assert(parseCommandResult(R("/r", "none (cancelled by user)", "", "")).exit.state === "cancelled");
p = parseCommandResult(R("/r", "1", "x", "Half\n\nSTDERR:\nreal"));
console.assert(p.stdout === "x\n\nSTDERR:\nHalf" && p.stderr === "real");
p = parseCommandResult(R("/r", "0", "", "slow\n\nHook `g`: refused after the action."));
console.assert(p.stderr === "slow" && p.hookNote === "Hook `g`: refused after the action.");
p = parseCommandResult(R("/r", "0", "", "Hook `g` mid\nmore"));
console.assert(p.hookNote === null && p.stderr === "Hook `g` mid\nmore");
console.assert(parseCommandResult("fn main() {}") === null);
console.assert(parseCommandResult("The user declined to run `x`.") === null);
console.assert(commandReason('Ran `echo "a ` — b"` — Quote test.', 'echo "a ` — b"') === "Quote test.");
console.assert(commandReason("Read `x`.", "x") === "");
```

Review focus 2 needs the last-marker split, and that only works with a greedy
STDOUT group. The `parseCommandResult(R(…, "x", "Half\n\nSTDERR:\nreal"))`
assert pins it.

- [x] **Step 2: Implement `parseCommandResult` and `commandReason`.** Use
  proposal §5.1's pattern with a greedy STDOUT group, then the hook-note split.
- [x] **Step 3: The command step.** In `renderActivityGroup`, a parsed pair
  becomes a `.activity-command` row: caret, `$ command` (ellipsis, full text in
  `title`), `.exit-mark` (`✓ 0`, `exit N`, `timed out`, `cancelled`), and the
  muted reason. The row is the `createDisclosure` trigger. Its panel holds:
  - the working directory, only when it is not `repo()`;
  - one `<pre>` per non-empty stream, labelled `stdout` / `stderr`;
  - `No output` when both streams are empty;
  - the `.activity-hook-note`.

  An unparsed pair keeps #67's title and `<pre>`.
- [x] **Step 4: Absorption.** In the grouping walk, an assistant message for
  which `approvalRequestFor` holds starts or joins the run. Its step gets
  `.activity-approved` ("approved" after the exit mark), and its text goes at
  the top of the panel under a quiet *Approval request* label
  (`.activity-request`). Every other message renders as today.
- [x] **Step 5: Walk the Task 1 session.** Open the Activity:
  - `ls target` reads `✓ 0`, has no working directory, and has no stderr
    section.
  - The first `cargo check` reads *approved*; the second does not (review
    focus 5).
  - `rm -rf` stays a request bubble before the Activity, and its step shows
    the declined text in a `<pre>`.
  - `sleep 999` reads `timed out`, shows `<root>/sub`, and carries the hook
    note under the output, not in stderr.
  - The quote test's reason reads `Quote test.`
  - The `src/lib.rs` step is the plain `<pre>`.
- [x] **Step 6: Style guide.** In §7 (or §4 beside Activity), add the command
  step: exit-mark states, and the rule that a hook note is never stderr. Add a
  command-step specimen with an approved and a failed row to
  `ui-style-guide.html`.
- [x] **Step 7:** Run the scoped checks, then fill in the Progress row.

### Task 4: Sidebar shows only what has something to say

**Proposal:** §3 req. 7–10, §5.4.

**Files:** `app.js`: `renderWorkersEntry` (~8003) and its caller (~10314),
`dismissWorker` (~8202), the project menu (~1128–1141), `scheduleUpdateCheck`,
`resetUpdateButton`, `checkForAppUpdate` and `installAppUpdate` (~3365–3470).
`index.html`: `.brand` (~23–26), and Settings → General (~518). `style.css`:
`.project-workers-entry`, `.update-button`.

**Interfaces:**
- Produces `renderWorkersEntry(projectPath, rows) -> HTMLElement | null`.
- Produces `showRestartNotice()`: the footer is shown, the button hidden, and
  the text reads *Restart Damaian to finish the update*.
- Produces the Settings button `#check-updates-btn` (`.btn-quiet`).

- [x] **Step 1: Console asserts**

```js
console.assert(renderWorkersEntry("/p", []) === null);
dismissedWorkers.add("w1");
console.assert(renderWorkersEntry("/p", [{ sessionId: "w1", group: "finished" }]) === null);
const e = renderWorkersEntry("/p", [{ sessionId: "w2", group: "running" }, { sessionId: "w3", group: "needs_you" }]);
console.assert(e.querySelector(".worker-dot").dataset.state === "needs-you");
console.assert(e.textContent.includes("1 running") && e.querySelector(".workers-badge").textContent === "1");
dismissedWorkers.delete("w1");
```

- [x] **Step 2: Implement.** Make the entry conditional, give it the dot (most
  urgent: `needs-you` › `running` › `paused` › `failed` › `finished`) and the
  `· N running` label. Make `dismissWorker` also redraw the project list.
- [x] **Step 3: Project menu.** Add *Workers board* after *New worker*, both
  `hidden: !workersEnabled`, calling `openWorkersBoard(projectPath)`.
- [x] **Step 4: Update footer.** `resetUpdateButton` hides `#update-app-footer`.
  The available and installing branches show it, as now. A successful install
  calls `showRestartNotice()`. Add a *Check for updates* row to General that
  calls `checkForAppUpdate(true)`.
- [x] **Step 5: Brand.** Add `hidden` to `#repo-state`; `setRepoState` is
  unchanged.
- [x] **Step 6: Walk.** Turn worker sessions on in Settings: a project with no
  workers shows no entry, and its ⋮ opens *Workers board*. Outside the desktop
  app, stub `tauriUpdater` from the console to return `null`: no footer shows,
  and Settings → *Check for updates* toasts "up to date".
- [x] **Step 7: Style guide.** Update §4 Worker sessions' *Sidebar* paragraph:
  the entry is conditional, and its dot and running count. Update the
  `.workers-badge` specimen if it shows the old entry.
- [x] **Step 8:** Run the scoped checks, then fill in the Progress row.

### Task 5: One icon set; header and composer

**Proposal:** §3 req. 13–16, §5.5.

**Files:** `app.js` (new `icons`/`icon` near the other DOM helpers, plus
`renderContextButton` ~2786), `index.html` (`.thread-actions` ~93–138 and
`.composer-actions` ~325–440), `style.css` (`.topbar-button`, `.symbol-icon`,
`.terminal-icon`, `.terminal-button`, `.attach-icon`,
`.context-inspector-button`, the triggers).

**Interfaces:**
- Produces `icon(name: "code"|"terminal"|"paperclip"|"folderPlus"|"compose")
  -> SVGSVGElement`: 24×24 viewBox, `stroke="currentColor"`,
  `stroke-width="1.7"`, `fill="none"`, round caps and joins, `aria-hidden`,
  `class="icon"` (16px). The path data is the mockup's (`mockup.html`).
- Produces `.icon-btn`: 28×28, transparent, `--muted`, hover
  `rgba(0,0,0,.05)`, the §3 focus ring, and `[aria-pressed="true"]` with the
  accent tint.
- Produces `contextMeterText(totalTokens, limitTokens) -> { percent: number |
  null, label: string }`.

- [x] **Step 1: Console asserts**

```js
console.assert(icon("code").getAttribute("aria-hidden") === "true");
console.assert(contextMeterText(12000, 32000).percent === 38);
console.assert(contextMeterText(12000, 32000).label === "38% of budget");
console.assert(contextMeterText(12000, 0).percent === null);
```

- [x] **Step 2: Header.** Symbol search and terminal become `.icon-btn` with
  `icon("code")` and `icon("terminal")`. Their existing ids, `aria-label`s and
  `title`s stay, so every listener still binds. *Context* is a borderless chip
  left of a `.thread-actions-divider`, holding a `.context-meter`
  (36×4px, `--accent` on `--line`) and the label. Its title adds `; model
  window <n>` when `lastContextManifest.limit.window` is known.
- [x] **Step 3: Composer.** Add `<span class="trigger-key">Mode</span>` and
  `<span class="trigger-key">Effort</span>` before the values (muted, weight
  500), and make attach use `icon("paperclip")`. `#session-mode-label` and
  `#effort-menu-summary` keep their ids.
- [x] **Step 4: Remove** the CSS for the replaced glyphs, only where no other
  element uses them (`grep` each class first; the terminal tab bar uses
  `.terminal-icon`, so keep it there).
- [x] **Step 5: Walk.** Tab reaches each header control in order. Every
  control shows its tooltip on hover. Pressing the terminal tints it. The
  composer reads *Mode Code* and *Effort Default*. Measure the header height
  before and after and record both.
- [x] **Step 6: Style guide.** In §3 add `.icon-btn` to the button table. In §4
  *Footer control rows*, add the key-label rule for `.btn-trigger`. Add specimens
  for the icon buttons (rest, hover, pressed) and the context chip.
- [x] **Step 7:** Run the scoped checks, then fill in the Progress row.

### Task 6: Project rows and session times

**Proposal:** §3 req. 11–12.

**Files:** `app.js`: `renderProjectList` (~10226; the row is built ~10260–10302),
the session-row builder it calls, and the Projects heading in `index.html`
(~42–61). `style.css`: `.project-row`, `.project-add-btn`,
`.project-menu-btn`.

**Interfaces:**
- Consumes `icon("compose")` and `icon("folderPlus")` (Task 5).
- Produces `formatRelativeTime(ms: number, now = Date.now()) -> string`.

- [x] **Step 1: Console asserts**

```js
const now = Date.parse("2026-10-29T12:00:00");
console.assert(formatRelativeTime(now - 30_000, now) === "now");
console.assert(formatRelativeTime(now - 2 * 60_000, now) === "2m");
console.assert(formatRelativeTime(now - 3 * 3_600_000, now) === "3h");
console.assert(formatRelativeTime(Date.parse("2026-10-27T09:00:00"), now) === "Tue");
console.assert(formatRelativeTime(Date.parse("2026-10-20T09:00:00"), now) === "Oct 20");
```

`now` covers under a minute. A weekday is used for anything within the last 6
days that is not today. Otherwise the format is `toLocaleDateString(undefined, {
month: "short", day: "numeric" })`.

- [x] **Step 2: Project row.** The caret moves to the left and replaces the
  folder glyph. *New session* uses `icon("compose")` and keeps its
  `aria-label`. *New session* and ⋮ get `opacity: 0`, shown on `:hover`,
  `:focus-within`, `.active` and `[aria-expanded="true"]`. The heading's *Add
  folder* uses `icon("folderPlus")`.
- [x] **Step 3: Session rows.** Append `<span class="session-when">` with
  `formatRelativeTime(session.updatedAtMs)`. Truncate the title before the
  time, never the time.
- [x] **Step 4: Walk.** Tab through a project row: *New session* and ⋮ appear
  as they take focus. A collapsed project's caret points right. Times show on
  every row.
- [x] **Step 5: Style guide.** Update the §4 hover-reveal note to name the
  project row, and add a project-row specimen.
- [x] **Step 6:** Run the scoped checks, then fill in the Progress row.

### Task 7: Close-out

**Proposal:** §3 req. 17–18, §6, plus AGENTS.md "When a spec becomes Done".

- [x] **Step 1:** Walk every §6 acceptance line in the inspection app. Record
  each line as *Real* or *Simulated* (say what was stubbed) in a `## Result`
  section of this file.
- [x] **Step 2:** Add style guide §8's row: "Chrome with nothing to say — an
  empty *Workers* entry and a permanent *Check Updates* button cost space on
  every screen to report nothing."
- [x] **Step 3: `docs/USER_GUIDE.md`.** Cover where *Check for updates* moved,
  when *Workers* appears, and how to read a command step (exit mark,
  *approved*, the hook note). `TROUBLESHOOTING.md` gets a line only if a
  failure reads differently.
- [x] **Step 4: Records.** Set `Status: Done <date>` in the proposal, and fill
  in its §7 Implementation Notes. Update the README row and *What to build
  next*, remove the CHANGELOG Unreleased line, and update this header's
  `Started`/`Done` and the Progress table. Run `grep -rn
  "68_ui_chrome_and_command_activity" docs/specs/*.md docs/specs/*/*.md` for any
  `Depends on:` lines.
- [x] **Step 5:** Run the scoped checks and `npm run specs:check`. Report that
  the workspace clippy and nextest are left to CI.

## Progress

| Task | State | Notes |
|---|---|---|
| 1 · Inspection fixture | Done 2026-10-29 | `command_activity_session` helper in the `serves_the_ui_for_manual_inspection` fixture seeds one finished task titled "Command activity" with the 18 messages in order plus one recorded usage row (~5,840 tokens, 1 model call); printed as `command activity session = session_<hex>_<hex>_<hex>`. The cause line uses `workspace_engine::ApprovalCause::plain_words()` (exported). Clippy `-p desktop-shell` clean, fmt and typos clean. Before figure observed in the running app: 3 usage rows (under the first activity group, under the "cargo check" group, and under the "rm -rf" group ahead of the answer), where one is wanted. Note: the fixture's "Add retry" task records no usage; the row here comes from `record_task_usage_for_task_id`. |
| 2 · Usage once, duration | Done 2026-10-29 | `turnDurationMs` and `pausedTasksOf` (interim predicate, a named helper for Task 3 to replace) added; `renderMessages` draws usage only under `endsTheTurn`. `markMessageUsage` takes `durationMs`; the indicator exposes `startedAt`. Live rule: the four non-resume paths (verification stream, patch proposal, `sendChatPrompt`, ask-again) pass `Date.now() - indicator.startedAt`; the seven approval/question/offer resume paths pass `null`. Console asserts: 0 `turnDurationMs` at HEAD (ReferenceError), 4/4 pass after. Command activity session: 3 usage rows before, 1 after (under "The crate builds; the clean was declined.", no duration, task paused). The fixture's "Add retry" task records no usage, so it shows none; the refused-in-Ask session shows one row per turn with a duration. biome, `node --check`, typos clean. |
| 3 · Command steps and approvals | Done 2026-10-29 | `parseCommandResult` (proposal §5.1 pattern, greedy STDOUT, trailing single-line `` Hook ` `` split into `hookNote`), `commandReason` (exact `` Ran `<cmd>` — `` prefix), `approvalRequestFor(message, next, nextResult, endsTheTurn)` (both messages groupable, same task, result parses, action starts `` Ran `<parsed command>` ``, request contains it) and `unparsedRequestFor` (every condition but the parse; the command is found by trying each closing backtick). `renderCommandStep` draws the `.activity-command` disclosure row; the walk in `renderMessages` absorbs a request into the following step (3 messages). `.activity-approved` is the muted "approved" span after the exit mark, not a class on the step. Paused predicate: `pausedTasksOf(messages, endsTheTurn, tasks)` = task holds a `question`, an absorbed request, an unparsed (declined) request, or has status `waiting_for_approval` (covers a request still pending at reload, whose `endsTheTurn` message is the request). Live rule: `liveTurnDurationMs(payload, indicator)` returns null when the `done` payload carries `commandProposal`, `fetchProposal`, `planProposal` or `questionProposal`; used by all four live non-resume paths (verification stream, patch proposal, `sendChatPrompt`, ask-again). Console asserts: RED `parseCommandResult is not defined`; GREEN 11/11, plus synthetic predicate checks (absorbed, declined not absorbed but paused, waiting status paused, live null on a pending proposal). Walk (Command activity session): Activity (3 steps) — `ls target` `✓ 0`, no cwd, stdout only; first `cargo check` `approved` with the request under *Approval request*; second not approved. `rm -rf` request stays a bubble before the second Activity (4 steps), whose first step is the declined text as the plain `<pre>`. `sleep 999` reads `timed out`, shows `<root>/sub`, stderr `slow`, hook note last. Quote test reads `exit 1`, reason `Quote test.`, stdout keeps `x\n\nSTDERR:\nHalf`. `src/lib.rs` is the plain `<pre>`. One usage row, no duration; the refused-in-Ask session still shows `· 0s` durations. Style guide §7 "Command steps" and a specimen (approved + failed row) added. biome, `node --check`, typos clean. |
| 4 · Sidebar visibility | Done 2026-10-29 | `renderWorkersEntry` returns null with no undismissed workers, else a `.worker-dot` of the most urgent state (needs-you, running, paused, failed, finished), `Workers · N running`, and the needs-you badge; `dismissWorker` redraws the project list. Project menu gains *Workers board* (`workers-board` action). `resetUpdateButton` hides the footer; new `showRestartNotice()` (guarded by `appUpdateRestartPending` so a later check does not hide it) and `#update-restart-notice`; Settings → General `#check-updates-btn` (`.btn-quiet`, disabled while checking). `#repo-state` is `hidden`. Asserts 4/4 pass. Walk: empty project shows no entry, one running worker shows "Workers · 1 running"; the project menu lists Workers board; with a stubbed `tauriUpdater` returning null the footer stays hidden and Check for updates toasts "Damaian is up to date". Style guide §4 Sidebar paragraph and the specimen updated. biome, `node --check`, typos clean. |
| 5 · Icons, header, composer | Done 2026-10-29 | `icon(name)` and `contextMeterText` added; symbol search and terminal are `.icon-btn` (ids, labels, titles kept; icons mounted from JS), attach uses the paperclip; context chip is borderless with a 36x4 `.context-meter`, a `.thread-actions-divider`, label "38% of budget", and `; model window <n>` in the title. Composer shows `Mode Code` and `Effort Default` via `.trigger-key`. Removed `.topbar-button`, `.terminal-button`, `.symbol-*`, `.attach-icon` CSS (`.terminal-icon` kept for the tab bar). Asserts 4/4 pass. Header height: 45px before (30px controls, 14px padding, 1px border; the ⋮ menu button stays 30px), 45px after (measured), so no growth. Pressed terminal computes the accent tint. Tab order follows DOM order (not walked by keyboard). Style guide §3 table, footer-row rule and specimens added. |
| 6 · Project rows and times | Done 2026-10-29 | Caret leads the project row and replaces the folder glyph (collapsed points right); New session uses `icon("compose")`; Add folder uses `icon("folderPlus")`; New session and ⋮ are `opacity: 0` until hover, focus-within, active project or expanded menu (checked: 0 inactive, 1 on focus). `formatRelativeTime` and `.session-when` appended to session rows, never truncated. Asserts 5/5 pass; 7 session rows show times. Empty-state text no longer says "+" ("Add a working folder to begin"). Style guide hover-reveal note and a project-row specimen added. All three tasks landed in one commit. |
| 7 · Close-out | Done 2026-10-29 | Acceptance walk run in the inspection app (see Result); style guide §8 row; USER_GUIDE (updates, context meter, paperclip, `</>`, Workers entry, command steps); proposal Done with §7 notes; README row and What to build next; CHANGELOG Unreleased line removed; no `Depends on:` references to #68 exist. The whole-branch review's six fixes landed first. Scoped checks: `node --check`, `npm run lint:web`, `typos`. Workspace clippy and nextest are left to CI. |

## Result

Walked 2026-10-29 in the `serves_the_ui_for_manual_inspection` app on port 4899
(Browser pane, 800px wide). *Real* means the rendered DOM of a seeded or
reloaded session; *Simulated* names what was stubbed.

| §6 line | Result |
|---|---|
| One usage row per multi-step turn, under the answer, with a duration | One row: **Real** (Command activity: 18 messages, 1 row, was 3). The duration: **Simulated** for a multi-step turn, because that task is paused (it holds an approval request) and shows none by design. Durations are **Real** on the "warm up" session's single-call turns (`· 0s`), and the pause rule is asserted on synthetic messages. |
| Activity collapsed; opened, `$ cmd`, exit mark, reason; output has no empty STDERR and no root directory; timeout reads `timed out` | **Real.** Both Activities load collapsed; rows read `$ ls target ✓ 0`, `exit 1`, `timed out`; expanded output shows stdout only, the `sleep 999` step shows its `/sub` directory, and the hook note is last. "No output" is **Simulated**: a synthetic empty-stdout result rendered through `renderCommandStep`. |
| Any other result shape shows its whole text | **Real:** `src/lib.rs` and the declined text render as the plain `<pre>`. |
| Approved command: no bubble, step reads *approved*, request inside; pending shows the card; declined keeps its bubble | **Real** for approved and declined (0 request bubbles for the approved one, the `rm -rf` request still a bubble). A pending card: **Simulated** (predicate asserted with a `waiting_for_approval` task; no live pending request). |
| No *Workers* entry with no workers; menu opens the board; running worker shows dot and `1 running` | **Simulated:** `renderWorkersEntry` called with empty, running and dismissed rows, and `workersEnabled` set true to read the project menu (it lists *New worker* and *Workers board*). The fixture has no workers. |
| No footer when up to date; Settings *Check for updates* says up to date | **Simulated:** `window.__TAURI__.updater` stubbed (null, then an update, then mid-install, then restart notice). The footer stayed hidden when up to date, showed `Update 9.9.9`, ignored a check mid-install (0 calls), and showed the restart line. Settings button present and runs the check. |
| Brand shows no folder name; project controls on hover/focus; Tab reaches them | **Real.** Brand text is `Damaian`. With the project made inactive, *New session* and ⋮ computed opacity 0, and a real Tab key press onto them made it 1. |
| Session rows show relative times | **Real:** seven rows show `1m`–`3m`. |
| Header: *Context* meter, then `</>`, `>_`, ⋮ in one style with tooltips; pressed terminal tinted | **Real** for `</>`, `>_`, ⋮: all 28x28 `.icon-btn` with `aria-label` and `title`; the pressed terminal is tinted. Context: **Simulated** (`setContextManifest` with 3,800 of 10,000 tokens: label `38% of budget`, meter 38%, title with the model window, hidden again at null with `display: none`). Header height 44px. |
| Keyboard Tab walk through the header and a project row | **Real**, keys sent to the page: search, add folder, project select, *New session*, project ⋮, session, session ⋮, then symbol search, terminal, ⋮ (the hidden Context chip is skipped), then the log. The ⋮ shows a focus ring. |
| Composer reads *Mode Code*, *Effort Default*; attach is a paperclip | **Real.** |
| `npm run lint:web` clean; style-guide page renders the new specimens | Lint **Real**. The page: **not verified styled.** `file://` is blocked for the Playwright browser, and the Browser pane renders it as a `data:` snapshot in which the relative stylesheet link does not load. The new specimens are present in the DOM (command rows, exit marks, approved note, icon buttons including ⋮); the link target exists on disk. Open it from Finder to see it styled. |

Worker rows (review fix 3) were checked by building one with
`renderProjectWorker`: its ⋮ right edge matched a session row's (261px).

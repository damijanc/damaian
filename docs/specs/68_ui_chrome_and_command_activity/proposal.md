# Feature Spec: Quieter Chrome and Readable Command Activity

Status: Specified 2026-10-29. Not started.
Order: 68 of 68
Reference: [`../../UI_STYLE_GUIDE.md`](../../UI_STYLE_GUIDE.md) — this spec
changes its Worker sessions (§4), header and composer rules, and adds
specimens to [`../../ui-style-guide.html`](../../ui-style-guide.html).
Depends on: [#58](../58_header_repo_bar_and_menus/proposal.md) (header, repo
bar, the ⋮ menu factory), [#57](../57_worker_sessions/proposal.md) (the
sidebar Workers entry), [#67](../67_conversation_activity_and_plan_hierarchy.md)
(Activity grouping, which this extends) — all built. Edits no `chat.rs`.

## 1. Motivation

A UX review on 2026-10-29 found three ways the shell still makes a session hard
to follow and the window noisier than it needs to be. The review started from a
screenshot of a real session (2026-10-22) and from the current build, run
against the `serves_the_ui_for_manual_inspection` fixtures. The user agreed a
before/after mockup of each change before this spec was written. The mockup is
[`mockup.html`](mockup.html) in this folder; open it in a browser, no server
needed. Where this text and the mockup differ, this text wins. That makes this
the eighth exception of the #41–#44 and #58 kind: usability feedback, not a
roadmap graduation.

1. **The conversation still repeats itself.** #67 folds action/result pairs into
   one *Activity*. But the turn's token line is still drawn under **every**
   ungrouped assistant message of the turn, not only under the answer, so one
   turn shows `817,439 tokens · 11 model calls · 91% cached` several times. An
   approved command's request (the cause's plain words, then the command in
   backticks) stays a full bubble
   beside the Activity that holds the command it requested. Inside the
   Activity, a command's result is the raw
   `Command:/Working directory:/Exit code:/STDOUT:/STDERR:` text, including
   empty `STDERR:` sections and the repository root as the working directory.
2. **The sidebar shows chrome that has nothing to say.** Every project shows a
   *Workers* entry while it has no workers. A green *Check Updates* button sits
   in the footer permanently. The brand repeats the open folder name, which the
   project list, the header and the repo bar already show. Every project row
   always shows a chevron, a `+` and a ⋮. The `+` that adds a folder and the
   `+` that starts a session look the same.
3. **The top bar and composer use four visual languages and two unlabeled
   values.** The header mixes a bordered text button (*Context*), a typed `{}`
   glyph, a drawn box for the terminal and a borderless ⋮. In the composer,
   *Code* and *Default* don't say what they are. The window has three different
   `+` buttons.

This is presentation only. It changes no stored message, model context, approval
decision, command policy, API route or setting.

## 2. Current State

- `renderMessages` (`app.js`) draws a turn's end marks only on the turn's last
  assistant (`endsTheTurn`). The exception is `markMessageUsage`, which is called
  for **every** rendered message with a `taskId`. Its one-row-per-bubble guard
  stops two rows under one message, not one row under each of several.
- Live turns draw no intermediate messages. They stream a phase indicator, and
  on `done` the answer gets `markMessageUsage(assistantMessage, …)` once. So the
  repetition is a reload-path bug, and the reload path is what every later look
  at a session sees.
- A command request the user had to approve is stored as an assistant message
  built by `command_proposal_response` (`chat.rs`). Its first line is the
  deciding cause's plain words (spec 62), and it contains the command verbatim.
  After approval, the log holds an assistant `` Ran `<command>` — <reason> ``
  (`tool_call_summary`) followed by the `tool` result. #67 groups that pair; the
  request before it stays a bubble.
- A command's `tool` result is built by `sandbox_command_context` (`chat.rs`) in
  a fixed shape: `Command: …\nWorking directory: …\nExit code: <n | none (timed
  out) | none (cancelled by user)>\n\nSTDOUT:\n…\n\nSTDERR:\n…`. Either stream
  may end in `\n[truncated]`. When an `after_tool` hook denies, `hook_after_note`
  appends `\n\n` and a line starting `` Hook `<id>` `` after STDERR, in both the
  auto-run and the approved path. `renderActivityGroup` shows it all as one
  `<pre>`.
- A **declined** command is also followed by its `` Ran `X` — reason `` step.
  Its `tool` result is `` The user declined to run `X`. … ``, not the shape
  above (`chat.rs`, resume path).
- `limit.tokens` on a context manifest is the context *budget*: when its source
  is `Declared` it is the configured packing budget, not the provider's window,
  which is the separate optional `limit.window` (`context_manager.rs`).
- A successful install calls `resetUpdateButton("Restart Damaian to finish the
  update")`. No relaunch API is wired into the shell; restarting is the user's.
- Dismissing a worker from the board adds it to the client-side
  `dismissedWorkers` set. `/api/workers` still returns it.
- `renderWorkersEntry` is appended for every expanded project while the worker
  setting is on, whatever `workerRowsByProject` holds.
- `scheduleUpdateCheck` already checks for an update 1.2s after launch, and
  `resetUpdateButton` shows the footer for every outcome except "available".
- `#repo-state` under the brand always names the open folder (spec 58 §3.3).
- The header's buttons are `.topbar-button` (bordered), `.symbol-icon` (a `{}`
  text glyph), `.terminal-icon` (CSS-drawn box) and `.thread-menu-btn` (⋮).
  `renderContextButton` labels *Context* `"<total> / <limit>"` from
  `lastContextManifest.totalTokens` and `.limit.tokens`.
- Session objects carry `updatedAtMs`, and messages carry `createdAtMs`. Tasks
  on `/api/session` carry no timestamps.

## 3. Requirements

**Conversation**

1. The turn's usage row is drawn once per task, under the task's last assistant
   message, on reload as well as live.
2. The Activity stays collapsed by default, as #67 built it. Command rows appear
   only once it is opened, so seeing a command's output is two disclosures deep.
   This was the user's choice on 2026-10-29, over the mockup's always-visible
   rows: a long turn stays one line. A command step inside an Activity renders
   as one row: `$ <command>` in
   monospace, an exit mark (`✓ 0` in `--ok`, `exit N` in `--danger`, `timed
   out` or `cancelled` in `--warn`) and the model's reason in muted text. Its
   output stays behind a collapsed per-step disclosure.
3. In expanded output, the working directory appears only when it differs from
   the repository root, an empty stream is omitted, and an empty stdout and
   stderr together read `No output`. `[truncated]` markers are kept. The bytes
   shown for each stream are the stored bytes.
4. A tool result that does not match §5.1's shape exactly renders as it does
   today, as the whole stored text in a `<pre>`. Nothing is hidden by a parse
   that fails.
5. An approval request that is immediately followed by the grouped step that ran
   its command, and whose result parses as a command result (§5.1), is drawn
   inside that step as an *approved* note, not as its own bubble. Its full text
   stays reachable inside the step's disclosure. A request that was declined, is
   still pending, or is not followed by its step renders as today. A declined
   command's result never parses, so it can never read *approved*.
6. Each turn's usage row also shows how long the turn took, worked out from its
   messages' `createdAtMs` (§5.3). A turn that paused for the user (an approval
   request or a question in the task) shows no duration, because the span would
   include the user's wait. So does a task with fewer than two timestamped
   messages.

**Sidebar**

7. A project's *Workers* entry is drawn only when that project has at least one
   worker row that is not in `dismissedWorkers`. With such rows, the entry shows the most urgent state's dot (needs you
   › running › paused › failed › finished), the existing needs-you badge, and
   `N running` in muted text when any are running.
8. The project ⋮ menu gains *Workers board* beside *New worker*. It is hidden
   when the setting is off, so the board stays reachable with no rows.
9. The sidebar footer is shown only while an update is available, installing, or
   waiting for a restart. Settings → General gains *Check for updates*, which
   runs the existing manual check. The restart state is a line of text,
   *Restart Damaian to finish the update*, with no button: the mockup's
   *Restart* button would need a relaunch API, which is out of scope.
10. The brand shows the logo and *Damaian* only. `#repo-state` is removed from
    view; `setRepoState`'s header refresh is kept.
11. In a project row, the folder glyph gives way to the expand/collapse caret on
    the left, and *New session* uses a compose glyph. *New session* and ⋮ are
    hidden by `opacity` and shown on `:hover`, `:focus-within`, `.active` and
    `[aria-expanded="true"]` (style guide §4). The Projects heading's *Add
    folder* uses a folder-plus glyph.
12. Each session row shows its `updatedAtMs` as a muted relative time (`2m`,
    `3h`, a weekday within the week, otherwise `Oct 20`), refreshed when the
    list redraws. The row's accessible name and title are unchanged except for
    the time.

**Top bar and composer**

13. Header controls share one style: borderless, 28px square, a 16px stroke icon
    drawn in inline SVG, an `aria-label`, and a `title` tooltip. Symbol search
    uses a code glyph (`</>`). The terminal uses a prompt glyph (`>_`), and its
    pressed state keeps the accent tint.
14. *Context* sits left of the icon group, behind a 1px divider. It shows a
    36×4px meter (`--accent` on `--line`) of `totalTokens / limit.tokens`,
    followed by `N% of budget`, not the mockup's `N% context`: the limit is
    Damaian's packing budget, so a high figure must not read as "the model's
    window is full". The title keeps `"<total> of <limit> tokens"` and adds
    `"; model window <window>"` when `limit.window` is known. When
    `limit.tokens` is 0 it shows the token total and no meter. It opens the
    same panel and is hidden by the same rule.
15. The composer's mode and effort triggers get a muted key label: `Mode`
    *Code*, `Effort` *Default*. The value keeps the trigger's weight, per style
    guide §3 `.btn-trigger`. The model trigger is unchanged.
16. The composer's attach control uses a paperclip glyph, so the window no
    longer has three different-meaning `+` buttons.

**Records**

17. `UI_STYLE_GUIDE.md` and `ui-style-guide.html` are updated in the same
    change: §4 Worker sessions (sidebar entry), the header rule, the
    composer-trigger rule, a command-step specimen, and an icon-button
    specimen. §8 gains the anti-pattern this spec removes ("chrome with nothing
    to say").
18. `npm run lint:web` passes clean, and every requirement is walked in the
    running app against the inspection fixtures. The fixtures gain a session
    with an approved command and an empty-stderr result if they lack one.

## 4. Non-goals

- **Grouping crash-recovery cards.** This was agreed as proposal 10 and
  deferred: it touches more of the recovery rendering and ships separately.
- **Drawing intermediate steps during a live turn.** Live turns keep the phase
  indicator. This spec changes what a turn looks like once drawn.
- **Turn timestamps in the API.** The duration is derived client-side. Exposing
  `created_at_ms`/`completed_at_ms` on `/api/session` would be more exact, but
  that is a server change and out of scope.
- **A command palette, a dark theme, or merging session and symbol search.**
- **Changing any message text the engine stores**, including the approval
  request's wording and the command result's format.

## 5. Design

### 5.1 Command-result parsing

`parseCommandResult(text)` returns `null` unless `text` matches:

```
^Command: (.*)\nWorking directory: (.*)\nExit code: (-?\d+|none \(timed out\)|none \(cancelled by user\))\n\nSTDOUT:\n([\s\S]*)\n\nSTDERR:\n([\s\S]*)$
```

On a match it returns `{ command, workingDirectory, exit, stdout, stderr,
hookNote }`. STDOUT is the greedy group up to the **last** `\n\nSTDERR:\n`, so
stdout that itself contains that text cannot cut the stream short. If the
captured stderr ends with `\n\n` plus a single line starting `` Hook ` ``, that
line is removed from `stderr` and returned as `hookNote`. It is shown under the
output as a `--warn` note, never as stderr. On `null`, the step uses #67's
`<pre>`.

### 5.2 Command step and absorbed approval

`renderActivityGroup` keeps its grouping input. Per pair:

- When `parseCommandResult(result.content)` succeeds, the step is a
  `.activity-command` row: caret, `$ command` (`.activity-command-text`,
  truncates with ellipsis, full text in `title`), `.exit-mark[data-state]`, and
  the reason. The reason is the text after the first `` ` — `` in an action
  that starts `` Ran `<command>` — ``, otherwise nothing. The row is the trigger
  of a `createDisclosure` whose panel holds the streams and, when it differs
  from `repo()`, the working directory.
- Otherwise the step is #67's title and `<pre>`.

Absorbing the approval request is a widening of #67's grouping walk. An
assistant message **A** is absorbed when all of these hold:

- it is in the same task as the message after it;
- it is not `endsTheTurn` and is not otherwise excluded by `isGroupableAction`;
- the next message is a groupable action that starts `` Ran `X` ``;
- that action's `tool` result parses (§5.1), which a declined command's never
  does;
- **A** contains `X` verbatim.

The step then gets `.activity-approved` ("approved" in muted text after the
exit mark), and **A**'s text goes into the step's panel, above the output, under
a quiet *Approval request* label. If **A** opens a run, the run starts at **A**.
A message that fails any condition is drawn as today. A pending request has no
`Ran` step yet. A declined request has one, but its result is the "user
declined" text, so the parse condition keeps it a bubble.

### 5.3 Usage row once, with duration

`markMessageUsage` moves inside the `endsTheTurn` block in `renderMessages`.
`turnDurationMs(taskId, messages)` is the last minus the first `createdAtMs`
among the task's messages. It is `null` when fewer than two have one, or when
the task paused for the user: it holds a `question`, or an assistant message
absorbed as an approval request (§5.2) or left as a pending/declined request
bubble. When non-null, it is appended to the row as `· 2m 14s` (`formatElapsed`,
already used by the Workers board). The live `done` paths pass the indicator's
own elapsed time only for a turn that was not resumed from a pause. A resumed
turn's indicator restarts at resume, so it would disagree with reload.

### 5.4 Sidebar

- `renderWorkersEntry` returns `null` when no row is outside
  `dismissedWorkers`, and the caller skips it. `dismissWorker` redraws the
  project list as well as the board. Its dot uses `.worker-dot[data-state]` with the existing states;
  the label becomes `Workers` plus `· N running` when N > 0.
- The project menu's worker section becomes `[New worker, Workers board]`, both
  `hidden: !workersEnabled`. *Workers board* calls `openWorkersBoard`.
- `resetUpdateButton` hides `#update-app-footer` instead of showing *Check
  Updates*; the footer appears only from the "available", "installing" and
  "restart" branches of `checkForAppUpdate`/`installAppUpdate`. Restart gets a
  `showRestartNotice()` that shows the footer with text and no button, instead
  of `resetUpdateButton`. Settings →
  General gets a `.btn-quiet` *Check for updates* that calls
  `checkForAppUpdate(true)`, whose existing toasts report the outcome.
- `.brand p#repo-state` gets `hidden`. The element stays so `setRepoState`
  keeps working without a branch.

### 5.5 Icons

There is one inline-SVG icon set for the shell. It is a `const icons = { … }`
map of 24×24, `stroke="currentColor"`, 1.7-width path data, rendered by
`icon(name)`, which returns an `aria-hidden` `<svg class="icon">`. It replaces
`.symbol-icon`, `.terminal-icon`, `.attach-icon`, the folder glyph in project
rows and the `+` in *Add folder*/*New session*. Other CSS-drawn icons stay;
moving them is not required by this spec. `.icon-btn` is the shared header
button rule (28×28, transparent, `--muted`, hover `rgba(0,0,0,.05)`, focus ring
per §3).

## 6. Acceptance Criteria

- A reloaded session with a multi-step turn shows exactly one usage row for that
  turn, under its answer, with a duration.
- A finished turn's Activity is collapsed. Opened, a command step reads `$ cmd`,
  an exit mark and the reason. Expanding it shows
  the output, with no empty `STDERR` and no working directory when it is the
  root. A timed-out command reads `timed out`.
- A tool result in any other shape still shows its whole text.
- An approved command shows no separate request bubble. Its step reads
  *approved*, and the request's text is inside the step. A pending request still
  shows the approval card; a declined one still shows its bubble.
- A project with no workers shows no *Workers* entry, and its ⋮ menu still opens
  the board. A project with a running worker shows the accent dot and `1
  running`.
- No sidebar footer shows when Damaian is up to date. *Check for updates* in
  Settings reports "up to date".
- The brand shows no folder name. Project rows show their controls only on
  hover or focus, and Tab still reaches them.
- Session rows show relative times.
- The header shows *Context* with a meter, then `</>`, `>_` and ⋮ in one style,
  each with a tooltip. The pressed terminal is tinted.
- The composer reads *Mode Code* and *Effort Default*, and attach is a
  paperclip.
- `npm run lint:web` is clean, and the style-guide page renders the new
  specimens.

## 7. Implementation Notes

To be filled in as tasks land.

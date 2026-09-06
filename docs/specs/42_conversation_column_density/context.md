# Context: Conversation Column Density

Why this spec exists and what the code looks like today. The decision is in
[`proposal.md`](proposal.md); the execution order is in [`tasks.md`](tasks.md).

## 1. Motivation

**43% of the conversation column is chrome.** Measured 2026-09-04 in the
running app at 1280×800, with a two-message exchange and seven context files:

| Band | Height | What it is |
|---|---|---|
| Thread header | 51px | Status badge and two buttons, plus a static caption |
| Context strip | 99px | Seven file paths as full-size buttons, wrapped to three rows |
| Composer | 191px | A textarea with a 104px minimum, plus its controls |
| **Chrome total** | **341px** | |
| Chat log | 459px | The conversation itself |

At a laptop window height the user sees less than half the column devoted to
the thing they came for. Three of those costs are avoidable.

**The header captions a screen you are already looking at.** Its left half is
`<p class="eyebrow">Workspace Agent</p><h2>Conversation</h2>`
(`index.html:71-72`). Both strings are hardcoded and never change. Meanwhile
the working folder and session name — which do change, and which you need
often — are only visible in the sidebar.

**The context strip is the loudest control in the app used for reference
information.** `#chat-context` (`index.html:118`) renders every file the model
read as a full-size `<button>` showing its complete path. Seven files wrap to
three rows and 99px, docked permanently above the composer, and it shows only
the newest turn — each turn overwrites the last.

**Role labels cost 22px per message.** `.message-role` (`style.css:754`)
prints "ASSISTANT", "YOU" or "SYSTEM" above every bubble, plus the 6px grid
gap. The information is already carried by two stronger signals: user messages
are right-aligned on `--accent-soft`, assistant messages left-aligned on white.

**The composer reserves four rows whether or not you need them.** `#chat-prompt`
is `rows="4"` (`index.html:126`) over a base `textarea { min-height: 78px }`,
computing to a 104px minimum. Most prompts are one line.

## 2. Current state

| Element | Location | Problem |
|---|---|---|
| `.thread-header` left half | `index.html:69-73` | Static caption, never changes, occupies half a permanent 51px band |
| `#chat-context` | `index.html:118`, `style.css:2044` | Docked strip; 99px for seven files; overwritten every turn |
| `renderContextFiles` | `app.js:3435` | Renders full paths as full-size buttons into the global strip |
| `.message-role` | `app.js:2840-2842`, `style.css:754` | 22px per message restating what alignment and colour already say |
| `#chat-prompt` | `index.html:126`, `style.css:170` | `rows="4"` over `min-height: 78px` → 104px floor regardless of content |
| `.command-approval-disclosure` | `style.css` | The disclosure pattern exists but is named for one caller, so the context list cannot reuse it |

`renderContextFiles` has five call sites (`app.js:581, 4327, 4447, 4654,
4756`). The three that populate it all have the turn's `assistantMessage` in
scope, so folding the list into the turn needs no new plumbing. The two that
clear it become unnecessary once the strip is gone.

## 3. Known limitation: the disclosure does not survive a reload

`renderMessages` (`app.js:3082`) rebuilds a loaded session from stored messages
carrying `{role, content, id, taskId, sessionId}` and nothing else.
`contextFiles` exists only on the live turn's `done` payload — the server does
not persist which files a turn read.

So the folded disclosure is **live-turn only**. Reopen a session and the "Read
N files" rows are gone.

This is not a regression: today's strip is cleared on session load too
(`app.js:4447`), and it only ever showed one turn. But when this design was
chosen over the alternatives, part of the argument was that it "becomes
per-turn history instead of one global list". That was overstated. Making it
true requires persisting `context_files` per assistant message server-side,
which is a storage change and out of scope here — see
[`proposal.md`](proposal.md) §2.

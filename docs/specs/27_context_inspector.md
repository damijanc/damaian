# Feature Spec: Context Inspector

Status: Not started
Order: 27 of 27
Roadmap: `docs/ROADMAP/03_phase_3_code_understanding.md`, Phase 3, Work
Package 5 (Must). That directory is local-only and not committed, so the
reference is a name rather than a link; this spec is self-contained.
Related spec sections: `ai_coding_assistant_specification.md` section 7.1 (chat
interface), section 7.3 (path and secret policy), section 7.9 (context
assembly). Related implementation specs:
[`05_clickable_file_references.md`](05_clickable_file_references.md)
(navigation to referenced content),
[`11_agents_md_support.md`](11_agents_md_support.md),
[`20_working_modes.md`](20_working_modes.md) (session-scoped state pattern),
[`26_context_assembly.md`](26_context_assembly.md) (produces the manifest this
renders — this spec is unusable without it).

## 1. Motivation

"The user can see what context the agent uses" is one of the product's central
claims, and it is currently false. There is no view of assembled context, before
or after a call.

[Spec 26](26_context_assembly.md) produces the data — a manifest of what was
included, what was excluded, and why. This work package makes it visible and
gives the user control over the optional parts. Shipping the assembly pipeline
without the inspector ships the mechanism without the accountability: better
context selection that the user still cannot see.

It also matters for what comes next. Phase 3b delivers persistent memory through
this same view, and memory the user cannot inspect is precisely the
hidden-instruction failure the product direction rejects. An inspector built
only for files would have to be rebuilt for memory; built for categories, it
takes memory as one more category.

The control half is where the value compounds. A user who can see that 40% of
the window went to a large generated file can pin the two files that matter and
get a better answer for less money — a judgement they can make instantly and the
retrieval heuristics cannot make at all.

## 2. Current State

- **No context view exists**, pre-send or post-send.
- **`ContextPlan` is built and immediately consumed.**
  `ContextManager::build_context` (`crates/workspace-engine/src/context_manager.rs:66`)
  returns a `ContextPlan` (`context_manager.rs:31-38`) that the caller turns
  into model messages within the same call path. It is never persisted, never
  surfaced, and there is no point between assembly and sending where anything
  could intervene.
- **`ContextPlan` carries no exclusion or provenance data**:
  `{ repository_id, task_id, token_estimate, items, files }`. What was left out
  is unrecorded, so a pre-send view built on today's structure could show only
  what happened to fit.
- **`ContextItem` has no line range** (`context_manager.rs:22-29`), so content is
  whole-file. [Spec 26](26_context_assembly.md) §5.1 adds ranges and categories;
  this spec depends on that.
- **Per-session state has an established pattern.**
  `SessionStore::allow_browser_diagnostics_for_session` /
  `browser_diagnostics_allowed_for_session`
  (`crates/workspace-engine/src/session.rs:261-291`) append an event and replay
  the log. [Spec 20](20_working_modes.md) §5.4 follows it for session mode; pins
  and path restrictions follow it here.
- **Clickable file references exist** ([spec 05](05_clickable_file_references.md)),
  opening a file in the app or the configured editor. This is the "open
  referenced content" mechanism.
- **Nothing is pinnable.** `explicit_paths` is a `build_context` parameter
  (`context_manager.rs:73`) supplied per call, not durable session state.

## 3. Requirements

1. A pre-send and post-send view shows: instructions included; file paths and
   line ranges; search and symbol results; the conversation summary;
   approximate tokens by category; and what was excluded or truncated and why.
2. The user can pin a file or range; remove optional context; restrict the
   session to selected paths; clear pinned context; and open referenced
   content.
3. **Required safety instructions and the user's own messages cannot be
   removed.** The inspector controls optional context only.

## 4. Non-goals

- Changing what assembly selects. The inspector shows and constrains; the
  selection logic is [spec 26](26_context_assembly.md).
- Editing context content. A user may include or exclude an item, and pin a
  range — not rewrite what a file says on the way to the model.
- Showing raw provider payloads or message JSON. The view is categories, paths,
  ranges, and token counts.
- A permanent per-repository context configuration. Pins and restrictions are
  session-scoped; repository-wide rules are `AGENTS.md`
  ([spec 11](11_agents_md_support.md)) and `restricted_patterns`.
- Retroactively changing a sent turn. Post-send is a record.
- Memory inspection — Phase 3b, which adds a category to this view.
- Diffing manifests between turns.

## 5. Design

### 5.1 A pre-send gate

Requirement 1's pre-send view needs a point that does not currently exist:
`build_context` returns a plan and the caller sends it in the same path. Assembly
and sending must be separated.

```rust
/// Assembled and inspectable, not yet sent.
pub struct PreparedContext {
    pub manifest: ContextManifest,
    pub items: Vec<ContextItem>,
}
```

The turn becomes: assemble → `PreparedContext` → (optional inspection) → send.

Inspection is **opt-in and non-blocking by default**. A turn does not wait for
the user to approve its context; that would add a confirmation step to every
message and would be abandoned within a day. The default is that the view is
available — a control showing the token total, openable during or after the turn
— and the turn proceeds. A user who wants to inspect before every send can
enable a `pause_for_context_review` setting, off by default.

This is the design decision most likely to be got wrong in the other direction.
A pre-send view that blocks is technically closer to the requirement's wording
and worse as a product; the requirement is that the user *can* see before
sending, not that they must.

### 5.2 What the view shows

Rendered from `ContextManifest` ([spec 26](26_context_assembly.md) §5.5), which
already carries every field requirement 1 asks for:

```text
Context for this turn                    18,240 / 32,768 tokens  (declared limit)

  Required            412   user message, safety instructions        [locked]
  Instructions      1,180   AGENTS.md, crates/workspace-engine/AGENTS.md
  Pinned            2,940   src/upload.rs                            [unpin]
  Plan state          260   4 steps
  Repository map      880   3 roots
  Symbols           1,640   12 symbols  (4 heuristic)
  Search results    3,100   9 results
  File ranges       7,420   6 ranges in 4 files
  Findings            408   2 findings
  Summary               0   none

Excluded (11)
  .env                          restricted path
  src/generated/schema.rs       too large (1.2 MB)
  src/upload.rs:88-96           merged into src/upload.rs:80-140
  tests/legacy.rs               search results budget exhausted
```

Three things this rendering makes visible that nothing does today:

- **The excluded list**, which is the half users actually need. "Why didn't it
  read `upload.rs`?" is answerable.
- **The limit's source** — declared, observed, or default
  ([spec 26](26_context_assembly.md) §5.7) — so conservative context is
  attributable rather than mysterious.
- **Heuristic counts**, carrying [spec 25](25_symbol_and_relationship_index.md)'s
  provenance through to the surface. "12 symbols (4 heuristic)" tells the user
  how much of the structural information is a guess.

Every path is a clickable reference ([spec 05](05_clickable_file_references.md)),
including in the excluded list — a restricted file should still be openable by
the user, who has every right to read their own file.

### 5.3 Post-send

The post-send view is the same rendering of the manifest that was actually used.
Requirement 1 distinguishes pre-send from post-send, and the distinction is real:
if the user removed an item, or a `Required` item forced a re-plan, the sent
manifest differs from the first assembled one.

The manifest is recorded per turn (already the case, via
`AuditLog::record`), so the post-send view is available for any past turn in the
session, not only the most recent. A user debugging a bad answer three turns ago
can see what that turn was given.

Post-send is a record and is read-only.

### 5.4 Controls, and their persistence

| Control | Effect | Scope |
|---|---|---|
| Pin file or range | Enters the `Pinned` category on every subsequent turn | Session |
| Unpin / clear pins | Removes them | Session |
| Remove optional item | Excluded from this turn, reason `UserRemoved` | This turn |
| Restrict session to paths | Only these paths are eligible for context | Session |
| Open referenced content | Opens in app or configured editor | — |

Pins and restrictions persist as appended session events, following the
`browser_diagnostics_allowed_for_session` pattern (`session.rs:261-291`) and
[spec 20](20_working_modes.md) §5.4:

```json
{"seq":88,"eventType":"context_pin_added","sessionId":"session_...",
 "path":"src/upload.rs","range":{"start":40,"end":120}}
{"seq":91,"eventType":"context_restriction_set","sessionId":"session_...",
 "paths":["src/upload.rs","tests/upload.rs"]}
```

Replayed newest-wins, read by parsed `eventType` rather than substring matching,
per [spec 17](17_durable_task_state_and_crash_recovery.md) §5.2. Pins therefore
survive a restart, which matters because a user who pinned three files for a
long task should not lose that to a crash.

**A restriction narrows; it never widens.** `path_policy.rs` and
`restricted_patterns` still apply on top, so a user cannot restrict *to* a
restricted path and thereby include it. The restriction is an intersection with
what policy already permits — stated explicitly because "restrict the session to
selected paths" could be read as an override, and it is not.

A pinned file that later becomes restricted, or is deleted, is reported in the
excluded list with the reason rather than silently dropped from the pin list.

### 5.5 What cannot be removed

Requirement 3 is enforced by the type, not by the UI.
[Spec 26](26_context_assembly.md) §5.1 defines `ContextCategory::Required` as the
first variant, holding safety instructions and the user's own message. The
inspector renders `Required` items as locked, and the removal path rejects a
`Required` item — so a UI bug, or a future caller, cannot remove one.

This is worth enforcing below the UI because the failure is silent and severe: a
turn that sent no user message, or dropped a safety instruction, would look
normal and behave badly. The acceptance criterion asserts the rejection at the
API, not the absence of a button.

`Instructions` (the `AGENTS.md` hierarchy) is *not* `Required` and may be
removed for a turn. That is deliberate: repository instructions are the user's
own content, and a user debugging whether an `AGENTS.md` rule is causing a
behaviour needs to be able to take it out. Precedence stays
[spec 11](11_agents_md_support.md)'s; removal is a per-turn user action, not a
precedence change.

### 5.6 Surface

A context control in the conversation header showing the current turn's token
total, opening a panel with the §5.2 rendering. It shows the last assembled
manifest between turns, so it is useful outside an active turn.

Category rows expand to their items. An item row offers remove (if not
`Required`) and pin. The excluded section is collapsed by default and shows its
count, since it is long and mostly uninteresting until it is exactly what you
need.

### 5.7 Documentation

`docs/USER_GUIDE.md`: how to read the panel, what each category is, what pinning
and restricting do and how long they last, why some items are locked, and how to
enable pause-before-send. `docs/TROUBLESHOOTING.md`: using the excluded list to
diagnose why a file was not consulted, what each exclusion reason means, and how
to inspect a past turn's context.

## 6. Acceptance Criteria

- Before sending, the user can see every file path and line range that will be
  sent, the token count per category, and the effective limit with its source.
- The excluded list shows every omitted item with its reason, including
  restricted paths, oversized files, deduplicated ranges, and budget
  exhaustion.
- Symbol counts distinguish heuristic from language-server provenance.
- Every path in the view, including in the excluded list, is a clickable
  reference that opens the file.
- Removing an optional item measurably reduces what is sent, and the item
  appears in the manifest's excluded list as `UserRemoved`.
- A `Required` item cannot be removed: the removal path rejects it — asserted at
  the API, not by the absence of a UI control.
- An `Instructions` item can be removed for a turn without altering
  [spec 11](11_agents_md_support.md) precedence.
- Pinning a file or range includes it on subsequent turns, and pins survive a
  restart.
- Clearing pins removes them from subsequent turns.
- A session restriction narrows eligible context and cannot include a path that
  `path_policy.rs` or `restricted_patterns` excludes — asserted by test.
- A pinned file that becomes restricted or is deleted appears in the excluded
  list with the reason, rather than disappearing.
- Post-send, the view reflects what was actually sent, not what was first
  planned, and is read-only.
- A past turn's context is inspectable, not only the most recent.
- Inspection does not block a turn unless `pause_for_context_review` is enabled,
  which is off by default.
- No content from a restricted or secret-bearing file is shown in the panel —
  the view renders paths, ranges, and counts from the manifest, never content.
- The five quality-gate commands from `AGENTS.md` pass.

## 7. Implementation Notes

To be completed during implementation. Record:

- Whether separating assembly from sending (§5.1) required restructuring the
  turn path in `chat.rs`, and what else that touched.
- Whether users in practice pinned files or removed items more often — it
  indicates which control is carrying the value and where to invest next.
- Whether `pause_for_context_review` was used at all after the first day. If not,
  that is evidence the non-blocking default was right, and worth recording
  before someone proposes making it the default.

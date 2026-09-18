# Feature Spec: Session Search and Export

Status: Done. Search, export, and redaction ship per
[`tasks.md`](tasks.md); requirement 7's server-mode exclusion is live as a
forward-looking `origin` field (always `"user"` until spec 52 writes a
non-user origin), so the filter exists and excludes nothing yet.
Order: 53 of 53
Plan: `docs/PLAN/01_phase_1_trust_and_recovery.md`, Phase 1, Work
Package 9 (Should). That directory is local-only and not committed, so the
reference is a name rather than a link; this spec is self-contained.
Depends on: [#17](17_durable_task_state_and_crash_recovery/proposal.md) (the
session log it searches) — built. Everything else named below is a
cross-reference, not a prerequisite.
Related implementation specs:
[`16_session_checkpoints_and_rewind.md`](16_session_checkpoints_and_rewind.md)
(settled the redaction-versus-faithful-restore conflict this spec inherits),
[`17_durable_task_state_and_crash_recovery/proposal.md`](17_durable_task_state_and_crash_recovery/proposal.md)
(owns the session log format and the parse-first reader this reuses),
[`42_conversation_column_density/proposal.md`](42_conversation_column_density/proposal.md)
(owns the thread header a search entry point sits in),
[`52_mcp_server_mode.md`](52_mcp_server_mode.md) (creates sessions this must not
mix into the user's own).

## 1. Motivation

Damaian keeps every session and offers no way to find anything in one.

Listing, renaming and deleting already work end to end — `list_sessions`,
`rename_session`, `delete_session` in `SessionStore`, `GET /api/sessions` in the
shell, and a picker in `app.js`. What is missing is the question people actually
ask: *"where was that?"* The answer is in a session whose title was generated
from its first message, in a list that grows monotonically, and the only way to
find it is to open sessions one at a time until one looks familiar.

The second half is that nothing leaves. A session is a record of a piece of
work — what was decided, what was tried, what the checks said — and it is
trapped in an application-private JSONL file. A user who wants to paste the
reasoning into a pull request, keep a decision alongside the code, or send a
colleague what happened has no path that is not a screenshot.

Both are small features whose absence is felt daily, and both are cheap because
[spec 17](17_durable_task_state_and_crash_recovery/proposal.md) already built
the durable, replayable log they read from. This is a surface over existing
storage, not new storage.

## 2. Current State

- **Session management exists.** `SessionStore::list_sessions(repository_id)`,
  `read_session`, `rename_session`, `delete_session`, `read_messages`; HTTP
  routes in `crates/desktop-shell/src/lib.rs`; a session picker in `app.js`.
- **The log is append-only JSONL with a `seq`**, read by a parse-first reader
  that skips a torn line rather than fabricating a message from it (spec 17).
  Anything reading the log must go through that reader; a second parser is a
  second chance to invent a message that was never written.
- **Messages are stored verbatim.** `SessionStore::append_message` writes
  `content` unchanged — no `SecretScanner`. This is deliberate:
  [spec 16](16_session_checkpoints_and_rewind.md) settled the conflict between
  redaction and faithful restore in favour of restoring what was actually said,
  because a rewind that restores `[REDACTED]` has destroyed the conversation.
  **The consequence for this spec is the whole of §5.4: the log is not safe to
  copy out as-is.**
- **Search exists for code, not for conversation.** `vector_index.rs` and
  `embeddings.rs` serve [spec 02](02_semantic_search.md)'s codebase search, over
  repository files.
- **There is no export of anything.** No Markdown, no JSON, no copy of a
  transcript.

## 3. Requirements

1. A user can search across the sessions of the current repository by text, and
   reach the matching point in the matching session.
2. Results state which session, when, and enough surrounding text to recognise
   the match.
3. Search reads through [spec 17](17_durable_task_state_and_crash_recovery/proposal.md)'s
   reader and tolerates a truncated or torn log without failing the search.
4. A session exports to Markdown and to JSON.
5. **Exported and displayed content is redacted**, even though the stored
   content is not. An export states that it was redacted.
6. Searching and exporting are read-only: no session is modified, and no export
   leaves the machine on its own.
7. Sessions created by [MCP server mode](52_mcp_server_mode.md) are excluded
   from the user's search and session list by default.

## 4. Non-goals

- **Semantic or embedding-based search over conversations.** [Spec 02](02_semantic_search.md)'s
  index is for code. Conversation search is a text search, and a user looking
  for "that thing about the keychain" is looking for the word.
- **A second index.** §5.2 scans logs. If that proves too slow at real volumes,
  §7 records the measurement that would justify an index — and until then, an
  index is a second persisted artifact that can disagree with the log.
- **Cross-session context.** Search finds a session; it does not feed one
  session's content into another's context. That is memory
  ([spec 28](28_memory_model_and_storage.md) onward) with its own consent rules.
- **Sharing, uploading, or publishing.** Export writes a file the user chose.
  There is no link, no service and no account.
- **Editing a transcript.** The log is append-only and stays that way; a rewind
  ([spec 16](16_session_checkpoints_and_rewind.md)) is the only sanctioned way to
  change conversation position.
- **Searching audit records or command output.** Conversation and task events
  only.

## 5. Design

### 5.1 Scope, and why it defaults to one repository

Search defaults to the current repository, with an explicit control to widen to
all. The default is a privacy choice rather than a performance one: sessions
from an unrelated repository can contain that repository's file contents, and
surfacing them in this window — in a search the user ran for an unrelated
word — leaks across a boundary the rest of the product maintains.
`list_sessions` already takes `repository_id: Option<&str>`, so both scopes
exist; the default is what this spec fixes.

### 5.2 Search

A bounded scan, not an index:

1. Enumerate sessions in scope, newest first.
2. For each, read the log through spec 17's parse-first reader, matching
   `message_appended` content and task titles.
3. Stop at a result cap and report that the cap was reached, rather than
   silently truncating.

Matching is case-insensitive substring by default, with whole-word and
literal-phrase options. Ranking is deterministic: recency first, then match
count. No scoring function that would make two identical searches return
different orders as the corpus grows.

The cost is one pass over text the user has already generated, and the sessions
most likely to be wanted are read first. §7 records the measured cost at real
volumes; an index is the answer when a number says so, not before.

A torn line is skipped by the reader and counted, and a session whose log had
unreadable lines says so in its result — `unreadable_event_count` already exists
for exactly this purpose. A search that silently omitted a match because a line
was torn would be worse than one that admits it.

### 5.3 Reaching the match

A result carries the session id and the matching event's `seq`. Opening it
scrolls to that message. `seq` is the right anchor rather than an index into the
rendered list, because the rendered list changes with rewinds and recovery
markers while `seq` is stable — the same property that makes it spec 17's
ordering key.

### 5.4 Export redacts, because storage does not

Requirement 5, and the reason it is not optional.

The session log contains exactly what was said, unredacted, by design
([spec 16](16_session_checkpoints_and_rewind.md)). Every existing display path
redacts on the way out. An export is a display path that writes a file, and it
is the one that matters most, because the resulting file is the artifact people
paste into a pull request or send to someone.

So export runs `SecretScanner` over every message and every included event,
writes `[REDACTED]` where it fires, and states at the top of the document that
it was redacted and how many redactions were made. A count rather than a silent
substitution: a user pasting a transcript into a review should know that
something was removed, and how much.

Search shares this path. A result snippet is redacted before it is rendered, so
searching for a secret-shaped string finds the session without displaying the
secret — and a user searching for their own API key learns which session
contains it without the search results becoming a second copy of it.

### 5.5 The two formats

| Format | Contains | For |
|---|---|---|
| Markdown | Title, repository, time range, the conversation in order, task outcomes, plan steps and their evidence, token and cost totals ([spec 19](19_token_and_cost_accounting/proposal.md)) | Reading, pasting into a review |
| JSON | The same content as structured records, with `seq` preserved | Tooling |

Neither format is the log. Recovery markers, internal action markers and torn
lines are omitted; this is a record of the conversation, not a backup of the
store. A round trip is explicitly not supported — an export cannot be imported,
because an importable transcript would be a way to write a session log without
going through the append-only path, and spec 17's guarantees rest on that path
being the only one.

Where a plan exists, its steps and evidence are included, since "what was
actually verified" is the most useful part of a session to keep and the part
[spec 21](21_task_plan_progress_and_budget/proposal.md) works hardest to
establish.

### 5.6 Server-mode sessions

[Spec 52](52_mcp_server_mode.md) opens a session per server process so its reads
are attributable. Those are machine activity and would swamp a human's session
list, so the session record carries its origin and the default filter excludes
anything that is not a user session. A control shows them, because "what did
that other tool read" is a question worth being able to answer — which was the
reason spec 52 records them at all.

### 5.7 Documentation

`docs/USER_GUIDE.md`: how to search, what the scopes mean, what export contains,
and that exports are redacted and cannot be imported back.

## 6. Acceptance Criteria

- A phrase from an earlier session is found, and opening the result lands on the
  matching message.
- Search defaults to the current repository; widening to all is explicit.
- A session log with a torn line still returns its other matches, and the result
  reports the unreadable count.
- The result cap is reported when reached, never silently applied.
- Two identical searches over an unchanged corpus return identically ordered
  results.
- A session exports to Markdown and to JSON, and both carry the redaction
  notice and the redaction count.
- A seeded secret present in the stored log appears in neither export nor in any
  search snippet — asserted by test, since the log deliberately still contains
  it.
- Export writes only to the path the user chose and makes no network request.
- No search or export path modifies a session log — asserted by comparing
  `latest_event_seq` before and after.
- Server-mode sessions are absent from the default list and search, and
  reachable through an explicit control.
- Every quality-gate command from `AGENTS.md` passes.

## 7. Implementation Notes

To be completed during implementation. Record:

- Measured search time against a realistic corpus — how many sessions, how much
  text, how long. This is the number that decides whether §5.2's scan holds or
  an index is justified, and without it the decision would be a guess.
- Whether redaction on the export path produced false positives that made a
  transcript unusable, and what they were.

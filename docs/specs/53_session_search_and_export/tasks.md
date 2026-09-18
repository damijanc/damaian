# Session Search and Export Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Implements:** [`proposal.md`](proposal.md)
**Started:** 2026-09-18 — **Done:** 2026-09-18

**Goal:** Let a user find a past conversation by text and get it out of the app —
a bounded scan over the session logs already built by spec 17, redacted on the
way out because spec 16 deliberately stores content verbatim.

**Architecture:** Everything lands in `crates/workspace-engine/src/session.rs`
except the HTTP surface and the web UI. `Session` gains a forward-looking
`origin` field (always `"user"` until spec 52 lands), `SessionStore` gains
`search_sessions` and `export_session`, both reading through spec 17's
parse-first reader and taking a `&SecretScanner` so redaction is enforced at the
engine rather than trusted to the shell. Two new routes in
`crates/desktop-shell/src/lib.rs` expose them; the thread header gains a search
entry point and each session row an export button.

## Global Constraints

- **Read [`proposal.md`](proposal.md) §5 before starting.** §5.4 is the one that
  cannot be skimmed: storage is unredacted by design (spec 16), so export and
  search snippets *are* display paths and redaction is mandatory, with the count
  stated rather than silently applied.
- **Read through spec 17's parse-first reader.** No second JSONL parser. A torn
  line is skipped and counted (`unreadable_event_count`), never fabricated.
- **Never widen the trust boundary.** Search and export are engine methods
  dispatched in the shell; nothing here touches `command_policy.rs`,
  `command_allowlist`, or `PathPolicy`.
- **An export cannot be imported.** No round trip. The append-only log is the
  only write path to a session.
- **Repository config is untrusted input.** If search or export ever gains a
  cap that a repository could set, it is `RestrictOnly` — but this plan adds no
  config key; the result cap is a constant.
- **Clippy warnings are errors.** Fix rather than suppress.
- **Commit messages:** one subject line, no body, no `Co-Authored-By`.
- **Do not commit without asking.** Show the change and the gate result; the
  decision to commit is Damijan's.

## File Structure

| File | Responsibility |
|---|---|
| `crates/workspace-engine/src/session.rs` | `Session.origin`, `SessionStore::search_sessions`, `SessionStore::export_session`, the search/export types. Everything in this spec that is not a route or a DOM element. |
| `crates/workspace-engine/src/lib.rs` | Re-export the new types. |
| `crates/desktop-shell/src/lib.rs` | `GET /api/session-search`, `GET /api/session-export`, and their serialization helpers. |
| `crates/desktop-shell/static/app.js`, `index.html`, `style.css` | Search entry point and export button. |
| `docs/USER_GUIDE.md` | How to search, what the scopes mean, what export contains, that exports are redacted and cannot be imported. |
| `crates/workspace-engine/tests/session_search_export.rs` | **New.** Integration tests that write real session logs through `SessionStore` and assert on search/export. |

---

## Task 1: `Session.origin` (the spec-52 stub)

The forward-looking field that requirement 7 needs. It is always `"user"` today,
so the filter is live but excludes nothing until spec 52 writes a different
origin.

**Files:**
- Modify: `crates/workspace-engine/src/session.rs`, `crates/desktop-shell/src/lib.rs`

**Interfaces:**
- `Session` gains `pub origin: String`.
- `session_json` (both copies: `session.rs` and the shell's `session_json`) write
  `"origin":"…"`.
- `parse_session_event` reads `origin`, defaulting to `"user"` for logs written
  before this field existed (so an old log is not dropped).

- [x] **Step 1: Add the field and its serialization**

  In `session.rs`, add `pub origin: String` to `Session`, after `summary`.
  In `session_json`, add `,\"origin\":\"{}\"` with `escape_json(&session.origin)`.
  In `parse_session_event`, read it with
  `event.text("origin").unwrap_or_else(|| "user".to_string())`.

- [x] **Step 2: Thread the default through every `Session` construction site**

  `SessionStore::create_session` constructs a `Session`; give it
  `origin: "user".to_string()`. Search the tree for other `Session {` literals
  and set them all.

- [x] **Step 3: Mirror in the shell serializer**

  In `desktop-shell/src/lib.rs`'s `session_json` (near `sessions_json`), add the
  same `"origin"` field so the frontend can filter on it later.

- [x] **Step 4: Tests**

  A `Session` round-trips through `session_json`/`parse_session_event` with
  `origin` preserved, and an event without `origin` parses to `"user"`.

- [x] **Step 5: Scoped checks**

  `cargo nextest run -p workspace-engine --test foundation` and
  `cargo clippy -p workspace-engine --all-targets`.

- [ ] **Step 6: Show the change and ask before committing**

  Suggested subject line: `Record a session's origin so server-mode sessions can be filtered`

---

## Task 2: `SessionStore::search_sessions`

The bounded scan. Reads each in-scope session's log through spec 17's reader,
matches `message_appended` content and task titles, ranks deterministically
(recency first, then match count), and stops at a cap it reports rather than
silently applying.

**Files:**
- Modify: `crates/workspace-engine/src/session.rs`, `crates/workspace-engine/src/lib.rs`

**Interfaces:**
- `pub struct SessionSearchHit { pub session_id, pub session_title, pub seq, pub
  snippet, pub role, pub created_at_ms, pub match_count }` (all `Clone`).
- `pub struct SessionSearchResult { pub hits: Vec<SessionSearchHit>, pub capped:
  bool, pub unreadable_lines: usize }`.
- `SessionStore::search_sessions(&self, repository_id: Option<&str>, query: &str,
  options: SearchOptions, scanner: &SecretScanner) -> Result<SessionSearchResult>`
  where `SearchOptions { whole_word: bool, literal_phrase: bool, max_results:
  usize }`.

- [x] **Step 1: Write the failing tests**

  In `tests/session_search_export.rs`: seed two sessions with
  `create_session` + `append_message`; assert a phrase from one is found, the
  result carries `seq` and the session title, ordering is recency-then-count,
  the cap reports `capped`, a torn trailing line does not fail the search, and a
  seeded secret in the log appears as `[REDACTED_…]` in the snippet.

- [x] **Step 2: Implement**

  Enumerate sessions via `list_sessions`, filter to `origin == "user"`, then for
  each read the log through `parsed_events` and match. Redact snippets with the
  passed `scanner`.

- [x] **Step 3: Scoped checks**

  `cargo nextest run -p workspace-engine --test session_search_export` and clippy.

- [ ] **Step 4: Show the change and ask before committing**

  Suggested subject line: `Search sessions by text through the append-only log`

---

## Task 3: `SessionStore::export_session`

Markdown and JSON, both redacted, both carrying the redaction notice and count.

**Files:**
- Modify: `crates/workspace-engine/src/session.rs`

**Interfaces:**
- `pub enum ExportFormat { Markdown, Json }`
- `SessionStore::export_session(&self, session_id: &str, format: ExportFormat,
  scanner: &SecretScanner) -> Result<String>`

- [x] **Step 1: Write the failing tests**

  Seed a session with messages, tasks, a plan with evidence, and usage; export
  both formats; assert the redaction notice + count appear, a seeded secret is
  absent, `seq` is preserved in JSON, and task outcomes/plan steps/usage totals
  are present in Markdown.

- [x] **Step 2: Implement**

  Reuse `read_messages`, `read_tasks`, `read_task_statuses`,
  `read_task_usage`, and `read_session_plans` — no re-reading the log by hand.
  Build the Markdown/JSON, redacting every message and included event through
  the passed `scanner`, tallying the count.

- [x] **Step 3: Scoped checks**

  `cargo nextest run -p workspace-engine --test session_search_export` and clippy.

- [ ] **Step 4: Show the change and ask before committing**

  Suggested subject line: `Export a session to Markdown and JSON, redacted with a count`

---

## Task 4: HTTP routes

**Files:**
- Modify: `crates/desktop-shell/src/lib.rs`

**Interfaces:**
- `GET /api/session-search?repo=…&query=…&scope=…&whole_word=&literal=`
- `GET /api/session-export?session_id=…&format=markdown|json`

- [x] **Step 1: Add the routes**

  Search resolves `repository_id` from `repo` (or `None` for `scope=all`) and
  calls `search_sessions` with `engine.scanner`. Export calls `export_session`
  and returns the content with a `Content-Disposition: attachment` header so the
  browser saves it to the path the user chooses. Neither route writes a session
  log and neither makes an outbound network request.

- [x] **Step 2: Scoped checks**

  `cargo clippy -p desktop-shell --all-targets` and a manual curl against a
  `DAMAIAN_DATA_DIR=.damaian` instance.

- [ ] **Step 3: Show the change and ask before committing**

  Suggested subject line: `Serve session search and export over the shell`

---

## Task 5: Web UI

**Files:**
- Modify: `crates/desktop-shell/static/app.js`, `index.html`, `style.css`

- [x] **Step 1: Search entry point**

  A search box in the thread header. Submitting calls `/api/session-search`,
  renders hits (title, snippet, when), and opening a hit switches to the session
  and scrolls to the message anchored by `seq`.

- [x] **Step 2: Export button**

  One per session row in the project list; fetches the export and triggers a
  download through the browser's save dialog.

- [x] **Step 3: Scoped checks**

  `node --check crates/desktop-shell/static/app.js` and `npm run lint:web`.

- [ ] **Step 4: Show the change and ask before committing**

  Suggested subject line: `Add session search and export to the web UI`

---

## Task 6: Documentation

- [x] **Step 1: `docs/USER_GUIDE.md`**

  How to search, what the two scopes mean, what export contains, and that
  exports are redacted and cannot be imported back.

---

## Task 7: Spec bookkeeping

- [x] **Step 1: Update all four places**

  `proposal.md` `Status:` line, the `docs/specs/README.md` row, this file's
  Progress table, and this file's `**Done:**` header.

- [x] **Step 2: Full quality gate**

  All seven commands from `AGENTS.md`.

---

## Progress

| Task | What landed | Tests | Deviation |
|---|---|---|---|
| 1 | `Session.origin` forward-looking stub | 2 (origin round-trip, legacy default) | none |
| 2 | `SessionStore::search_sessions` | 8 (find, scope, torn line, cap, determinism, snippet redaction) | none |
| 3 | `SessionStore::export_session` | 2 (Markdown redact+count, JSON seq+redact) | none |
| 4 | HTTP routes | 0 (routes; covered by engine tests + clippy) | export returns `Content-Disposition: attachment` so the browser's save dialog is the chosen path |
| 5 | Web UI | 0 (UI; `node --check` + Biome clean) | messages now carry `seq` from `/api/session`, so a hit scrolls to its message |
| 6 | USER_GUIDE | 0 (prose) | none |
| 7 | Spec bookkeeping + gate | 663 total (10 new) | all seven gate commands pass; commits pending Damijan |

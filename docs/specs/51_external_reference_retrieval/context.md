# External Reference Retrieval — Context

This file records why the work exists and what the code looked like when the
spec became a folder (2026-10-28). [`proposal.md`](proposal.md) is the decision;
this is the starting point and the corrections planning found. Every line
reference was checked against the tree at commit `aaa3be8` on 2026-10-28; the
`config.rs` references were re-checked after Task 2, which shifts them.

## 1. Why now

Damaian has no way to read a page. Every task against a third-party API runs on
whatever the model remembers, unlabelled by version. The failure is a confident
call against an API that changed, which compiles only when the signature is
wrong.

Since the flat spec was written, two dependencies shipped: `#20` working modes
(the mode matrix that will gate the tool) and `#26` context assembly (the
category and budget fetched content will enter). Both are Done, so the only
prerequisite this spec names is built.

The reason this spec is Should-tier is §5.3 of the proposal: `fetch_url` is the
first tool that lets the model choose **what leaves the machine**. That is an
exfiltration channel before it is a reading feature, and the constraints in
§5.3 are the feature rather than a hardening pass over it.

## 2. Pre-implementation state

| What | Where today |
|---|---|
| Tool list offered to the model | `chat.rs:5019-5151`, filtered by `permits` (`mode.rs:139`) |
| Tool decode | `tool_action_from_call`, `chat.rs:10678` |
| Tool action type | `pub(crate) enum ToolAction`, `chat.rs:9856` |
| Mode × tool matrix | `mode_permits`, `mode.rs:236`; profile axis `profile_permits`, `mode.rs:157` |
| Action side-effect class | `action_effect`, `chat.rs:9977`; batching `chat.rs:10009` |
| Read-tool dispatch | `dispatch_read_only_action`, `chat.rs:8277` |
| Action markers | `tool_action_marker`, `chat.rs:10170`; `start_action`, `session.rs:1967` |
| Tool hook payload | `tool_event_payload`, `chat.rs:10079` |
| Pending turn (every pause kind) | `PendingChatTurn`, `chat.rs:8737`; nine struct literals |
| Resume entry points | `resume_after_command_decision`, `chat.rs:2916`; `refuse_unless_paused_for`, `chat.rs:2934` |
| Context categories | `ContextCategory`, `context_manager.rs:37`; `BUDGETED`, `:76` |
| Context budget | `ContextBudget.shares: [u16; 10]`, `context_manager.rs:118`; defaults `:127` |
| Context item | `ContextItem`, `context_manager.rs:576`; projection `IncludedRef`, `:473` |
| Assembly | `build_context`, `context_manager.rs:1270`; once per turn at `chat.rs:1250` |
| Prompt rendering | `ContextPlan::render_item`, `context_manager.rs:767`; sections `:810` |
| Inspector labels | `CONTEXT_CATEGORY_LABELS`, `app.js:2209`; `contextCategoryRow`, `app.js:2309` |
| Secret scanning | `SecretScanner::scan/redact`, `secret_scanner.rs:27`, `:42` |
| Audit redaction | `AuditLog::record`, `audit.rs:42` |
| Model transport (the only credentialed HTTP) | `CurlModelTransport`, `model.rs:530`; curl spawn `:637` |
| Cancellation-polled stream pump | `pump_stream`, `model.rs:814` |
| Child sweep registry | `ProcessKind`, `process_registry.rs:107`; `register`, `:225` |
| Config scopes | `ConfigScope`, `config.rs:67`; `RepositoryKeyClass`, `:80` |
| Config overlay boundary | `apply_overlay_scoped`, `config.rs:1134`; report `:174` |
| Config key partition | `ConfigKeyKind`, `config.rs:3127`; `classify_overlay_fields!`, `:3155` |
| Profile key split | `split_profile_keys`, `profile.rs:327` |
| `Allow Always` storage and writer | `command_allowlist_by_repository`, `config.rs:291`; fold `:2556`; writer `validation.rs:445` |
| Eval scenarios | TOML in `crates/eval-harness/scenarios`; loader `scenario.rs:475` |
| Eval assertions | `Asserts`, `scenario.rs:50-260` |

### 2.1 Corrections to the proposal's Current State

1. **The model-facing tool set is larger than §2 lists.** It is `run_command`,
   `propose_patch`, `propose_plan`, `complete_step`, `ask_user`, `read_file`,
   `list_directory`, `search_content`, `edit_file`, `search_codebase`,
   `read_git_status`, `read_git_diff`, `list_worktrees`, `read_git_log`,
   `turn_notes`, `propose_memory`, plus `inspect_web_page`/`run_web_scenario`
   **only when a browser-diagnostics runner is present** (`chat.rs:5113`), plus
   namespaced MCP tools. The proposal's point — nothing reaches the network for
   content — still holds.
2. **`ContextItem` is not `{kind, path, content, tokens, redaction_status}`.**
   It is `{category, kind, path, range, content, tokens, redaction_status,
   provenance, root_path}` (`context_manager.rs:576-595`), and `kind` is a typed
   `ContextKind` enum (`:250`), not a free string. A non-file source can use
   `path: None`, but there is **no field for an origin URL or fetch time**.
   Task 7 adds one; §5.4's "provenance label" cannot be met with today's shape.
3. **`ContextCategory` is closed and budgeted.** Ten categories, each with a
   share in a **fixed-length** `[u16; 10]` array (`context_manager.rs:118`), and
   the default shares sum to exactly 10,000 basis points. "Lowest-priority
   category" is therefore `ConversationSummary` today, and adding a category is
   an array-length change plus a config key, not a new variant alone.
4. **The context pipeline runs once per turn.** `build_context` is called before
   the model loop (`chat.rs:1250`); a tool result during the turn is appended to
   `messages` and never re-enters the plan. Fetched content cannot be both a
   same-turn tool result and a same-turn `ContextItem` without re-assembling the
   turn, which this spec deliberately does not do (D5).
5. **There is no per-host approval store.** The closest machinery is
   `command_allowlist_by_repository` plus `apply_repository_allowlist`
   (`config.rs:2556`) and the user-config writer in
   `ValidationOrchestrator::allow_command_always` (`validation.rs:445`). A new
   list and writer are needed; the pattern is reusable.
6. **`command_allowlist` is `UserOwned`, not `Forbidden`** (`config.rs:1377`).
   Spec 51's acceptance criterion says `web_domain_allowlist` must be rejected
   **as forbidden**, so the new key uses `RepositoryKeyClass::Forbidden` in both
   the overlay merge and `split_profile_keys`, not `UserOwned`.
7. **The network is not reached from one place.** `CurlModelTransport` is the
   only credentialed HTTP client, but MCP servers (`mcp.rs`), language servers
   (`lsp.rs`), hooks, the browser-diagnostics runner and `run_command curl` also
   leave the machine. The proposal's guarantee narrows to: **this tool never
   sends a credential and never reuses the model transport**.
8. **The model can already fetch through `run_command`.** `curl` classifies as
   network and needs approval (`command_policy.rs:424` onward); a user can also
   blocklist it. This spec does not remove that path. It gives the model a
   bounded, provenance-labelled, redirect-refusing alternative, and leaves the
   command path to the command policy.
9. **`SecretScanner` is inbound only.** It redacts audit fields (`audit.rs:42`)
   and command output, and exposes `scan`, `redact` and `contains_secrets`
   (`secret_scanner.rs:27-65`). Nothing scans a URL before a request, which is
   what §5.3 constraint 3 must add.
10. **Adding a tool touches eight exhaustive matches in `chat.rs`** (variant,
    offer list, decode, permission matrix in `mode.rs`, action effect, marker,
    label, hook payload, call key/rendering) and `ToolAction` is `pub(crate)`.
    This is why `chat.rs` is one task (Task 8) and the shell dialog another
    (Task 9).
11. **A fetch cannot be a `ReadOnly` action.** `ActionEffect::ReadOnly` calls run
    concurrently through `dispatch_read_only_action` (`chat.rs:8277`) and cannot
    pause. A fetch leaves the machine, so it is side-effecting and serialised.
    Its marker therefore also makes a crash mid-fetch an unknown outcome, which
    is the correct classification for a request that may have left.
12. **Every pause kind has its own slot and its own refusal.** `PendingChatTurn`
    (`chat.rs:8737`) has `web_diagnostic_call`, `mcp_call`, `plan_review`,
    `provider_fallback`, `round_budget`, `question`, `check_batch` and
    `context_review`; each resume function calls `refuse_unless_paused_for`.
    Fetch approval needs a new slot, a new `PausedFor` variant and a new resume
    function, or another pause's resume will mis-handle it.
13. **`pump_stream` is unbounded.** `model.rs:814` accumulates everything until
    the child closes. The fetch transport needs a bounded read that stops at the
    cap and kills curl; `--max-filesize` alone is not a bound against a server
    that sends no `Content-Length`.
14. **The eval harness has no stub HTTP server.** Scenarios are TOML with
    scripted turns and `Asserts` (`scenario.rs:50-260`), and the runner can
    approve a command or answer a question. Nothing serves a page for a
    deterministic fetch scenario; Task 10 adds a loopback stub page source and a
    `{stub_base}` substitution.
15. **The inspector needs a label.** `app.js:2209-2221` maps category names and
    `contextCategoryRow` (`app.js:2309`) renders each manifest category. A new
    category appears as its raw snake_case name until a label is added, and
    `conversation_summary` is skipped from the main list (`app.js:2424`) — the
    new category must not be skipped.
16. **The proposal's relative links all resolve**, but moving the file down one
    directory requires a `../` prefix on every one. Planning added it and
    verified each target exists.

## 3. Decisions

**D1 — Folder spec, `In progress`.** The proposal keeps its decision text; this
file and `tasks.md` carry execution state. Status is updated in four places
(proposal, README row, README current state, progress table/header).

**D2 — `fetch_url` is the requirement; `search_web` is not built.** No search
provider exists, and a tool the model can see is one it plans around. A later
spec or task may add `search_web`; nothing in this one depends on it.

**D3 — Transport is a separate, credential-free curl invocation.** Spawn
`curl --config -` with the request on stdin, in its own process group, registered
with `ProcessRegistry` for sweeping. No `--location`. Header dump to a temp file
under `<data_dir>/tmp`, removed on every exit. Connect timeout, stall limit and
max total time as `CurlModelTransport` uses. `KillOnDrop`-equivalent and a
cancellation poll that kills the child. No new dependency.

**D4 — URL parsing is strict and in-repo.** Accept `http` and `https` only, with
a non-empty DNS host or IP literal, an optional port, no userinfo, no whitespace
or control characters. The fragment is stripped from the request but is still
shown and scanned. A URL that fails parsing is a refused tool call, never a
request.

**D5 — Fetched content reaches context on the next assembly, not by rewriting
the turn loop.** The immediate result is a bounded, redacted tool result with a
fixed provenance frame. The fetch is also recorded as an external reference for
the session; the next turn's `build_context` includes it as the new
lowest-priority category. This is a deliberate reading of §5.4: adding a second
context assembly inside the turn loop would change every pause and every budget
accounting path for a feature whose content is already in the conversation as a
tool result. The acceptance criterion "appears in the context inspector" is met
on the next turn of the session, and Task 10 records that explicitly.

**D6 — New category `ExternalReference`, 11th and last.** `ContextCategory`
gains the variant after `ConversationSummary`; `BUDGETED` becomes 11 entries;
`ContextBudget.shares` becomes `[u16; 11]`; `context_budget.external_reference`
is added with a default 0.05 and `search_results` drops from 0.15 to 0.10 so the
default total stays 10,000. Spec 26 §5.2's table is updated in the same task.
The category is placed last so a crowded context drops a fetched page before any
repository content.

**D7 — `ContextItem.origin: Option<String>` and `IncludedRef.origin`.** The
string is the provenance label: the canonical URL plus the fetch time. `path`
stays `None`, so `files`, path policy and prompt rendering of file items are
unaffected; `render_item` prints the origin instead of a path when one is
present. Both fields are `#[serde(default)]` so old manifests load.

**D8 — Host approval lives in user scope.** `Config.web_domain_allowlist` is the
machine-wide list and `web_domain_allowlist.<repository_id>` is the per-checkout
`Allow Always` map. Both are classified `Capability` for the key partition and
`Forbidden` in `apply_overlay_scoped` and `split_profile_keys`; the repository
copy is reported through `RepositoryConfigReport.rejected_keys`, never honoured.
The map is folded into the effective list at load, like
`command_allowlist_by_repository`. The stored value is the host, lowercased,
with an explicit non-default port retained (`host:port`), so a loopback stub and
a default-port site are different approvals.

**D9 — The egress guard runs before approval.** A URL is refused outright when
`SecretScanner` finds a secret in it (the whole URL, not only the query), or when
the query/fragment — raw, percent-decoded, and base64-decoded values — contains a
line of at least 16 bytes from a file `read_file` returned during this task. A
query or fragment over 128 bytes is flagged in the dialog and not blocked.
Files reached only through `search_content`, `read_git_diff` or a command are
not part of the check; Task 4 records that limit and the threshold calibration.

**D10 — No redirect is followed, of any kind.** The proposal requires only that a
cross-host redirect is not followed. Not following a same-host redirect is
stricter, simpler to test and removes a whole class of decision; the result names
the `location` and the model may issue a new call, which then needs that host's
approval.

**D11 — Content rules are deterministic.** `text/*`, `application/json` and
`application/xhtml+xml` are converted; anything else returns its type and size
and no bytes. The body is capped (default 200 KiB), truncated with a stated
reason, scanned with `SecretScanner` before it is cached and again on the way to
the model (the cache therefore holds only redacted text). HTML is reduced to
text without scripts, styles or attributes; no rendering, no second model call.

**D12 — Per-turn cap and session-scoped cache.** `web_fetch_max_per_turn`
defaults to 8 and has a hard ceiling of 20, restrict-only like the other round
bounds. The body is cached under
`<data_dir>/external/<session-id>/<sha256(url)>.json` with URL, fetch time,
status and content hash. A repeat fetch in the same session is served from the
cache and says so; a different session never reads it, so a stale page can never
be presented as current.

## 4. Out-of-scope findings

- **`run_command curl` remains an unrestricted reader** once approved. This spec
  does not change the command policy; a follow-up might point the model at
  `fetch_url` in the system prompt, but that is a prompt change with its own
  measurement.
- **`ContextBudget` is not persisted** in session logs or patches, only in
  config, so widening `shares` has no migration cost. The `[u16; 10]` literals
  in `tests/context_assembly.rs` do need updating when Task 7 lands.
- **MCP fetch servers remain available** under spec 33. This spec declines to
  rely on one and does not disable one.
- **An image inside a fetched page is never fetched** (proposal §4 and spec 54).
  The converter drops sub-resources by construction; no extra guard is needed,
  but the USER_GUIDE must say so.
- **The audit log has no read API** (`audit.rs` exposes `record`, retention and
  escape only). Reconstructing "files read in this task" from it would mean a new
  parser; D9 uses the session's `action_started` markers instead and records the
  narrower scope.
- **`cargo deny check` failed at planning time for an unrelated advisory.** On
  2026-10-28 it reported `RUSTSEC-2026-0414` against `pulldown-cmark v0.13.4`
  (fixed in 0.13.5), reached through `workspace-engine`. No task here touches
  that dependency, so planning left it alone. It was fixed on `main` the same
  day as its own change (`ec9f7ea`), and this branch was rebased onto that
  commit, so the check now passes here.

## 5. What later tasks inherit

- The security properties in the proposal §5.3 are load-bearing. Do not move the
  egress guard after the approval dialog, do not add redirect-following, do not
  reuse `CurlModelTransport`, and do not let repository or profile config add a
  host.
- `chat.rs` is owned by this branch. Keep the tool wiring minimal and do not
  refactor the turn loop; D5 is the reason.
- Task 7 changes spec 26 §5.2's default table. AGENTS.md requires that edit in
  the same change as the behaviour.
- Task 10's injection scenarios are the evidence for requirement 4. A design
  argument is not evidence; record which fixtures ever succeeded.

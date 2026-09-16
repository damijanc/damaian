# Feature Spec: Image Input

Status: Not started
Order: 54 of 54
Plan: `docs/PLAN/03_phase_3_code_understanding.md`, Phase 3, Work
Package 9 (Should). That directory is local-only and not committed, so the
reference is a name rather than a link; this spec is self-contained.
Depends on: [#26](26_context_assembly.md) (the budget an image counts against)
— **not built**; Phase 1 WP3 (provider capability validation) —
**unspecified**. Everything else named below is a cross-reference, not a
prerequisite.
Related spec sections: `ai_coding_assistant_specification.md` section 7.1 (chat
interface), section 7.10 (secret detection).
Related implementation specs:
[`12_web_app_troubleshooting.md`](12_web_app_troubleshooting.md) (produces the
screenshot artifacts §1 is about; `In progress`, so §5.5 is additive and must be
coordinated rather than assumed),
[`26_context_assembly.md`](26_context_assembly.md) (the budget and category an
image enters through),
[`27_context_inspector.md`](27_context_inspector.md) (where a user sees what was
sent), [`44_composer_control_density/proposal.md`](44_composer_control_density/proposal.md)
(owns the composer surface the attach control lives on),
[`53_session_search_and_export.md`](53_session_search_and_export.md) (the export
path an image changes the guarantees of),
[`19_token_and_cost_accounting/proposal.md`](19_token_and_cost_accounting/proposal.md)
(the accounting an image breaks the `len / 4` assumption of).

## 1. Motivation

**Damaian takes screenshots it cannot look at.**

[Spec 12](12_web_app_troubleshooting.md)'s browser diagnostics capture
screenshots and record them as `WebDiagnosticArtifact { kind, path, mime_type,
width, height }` on a `WebDiagnosticReport`. The model that asked for the
diagnostic receives `report.text` and a list of file references. The pixels —
the thing a screenshot is for — reach nothing. The agent is told a picture was
taken and where it lives, and then reasons about a rendering problem from prose.

The user's side is the same wall from the other direction. `ModelMessage` is
`{ role: String, content: String, ... }`, nothing in `model.rs` or `chat.rs`
touches images, and the composer's attach control offers "Add file" and a
disabled "Add folder — coming soon". So the single most common way a person
reports a front-end bug — paste a screenshot of it — is not expressible. They
describe the misalignment instead, and the agent guesses.

This is the one remaining capability in this roadmap where Damaian has already
paid the cost of producing the data and gets none of the value.

It is Should-tier rather than Must-tier for one reason, and §5.3 is where it is
faced rather than avoided: **an image defeats `SecretScanner`.** A screenshot of
a terminal can contain an API key, and nothing in the redaction path can read a
pixel. The product's central promise about secrets meets a case it cannot keep,
and this spec's real work is deciding honestly what to say instead.

## 2. Current State

- **`ModelMessage.content` is a `String`.** `message_json` serialises
  `{"role": …, "content": …}` with a string body. The OpenAI-compatible content
  array form — a list of typed parts, some text, some image — is not
  constructed anywhere.
- **No image handling exists at all.** No base64 encoding, no `data:` URI
  construction, no MIME sniffing, no dimension handling, in either the engine or
  the shell.
- **Diagnostic artifacts are references.** `WebDiagnosticArtifact` records a
  path and dimensions; `extract_artifacts_from_text` pulls them out of the
  report text. The file sits under the data directory and is never read back for
  the model.
- **The composer attach menu is file-and-folder.** `attach-add-file-btn` adds a
  repository file to context; `attach-add-folder-btn` is `disabled` with
  `title="Coming soon"` (`crates/desktop-shell/static/index.html`). There is no
  paste handler and no drop target for an image.
- **Token estimation assumes text.** `len / 4` (`context_manager.rs`), which
  [spec 19](19_token_and_cost_accounting/proposal.md) measured as running
  2.5–6.3% high on real text. Against an image it is not a poor estimate; it is
  unrelated to the quantity being estimated.
- **Provider capability is declared, and in one case probed.**
  `ModelProviderConfig` carries `supports_native_tools` and
  `max_output_tokens`; spec 19 added a runtime probe for usage reporting.
  Nothing describes whether a provider accepts images, and the provider
  configured and measured in this repository does not.
- **Redaction is centralised and text-only.** `SecretScanner` runs over strings
  on the way into the audit log, command output, and every display path.

## 3. Requirements

1. A user can attach an image to a turn by paste, by drop, and through the
   existing attach control.
2. A screenshot captured by [spec 12](12_web_app_troubleshooting.md)'s
   diagnostics can be made visible to the model.
3. Image support is a provider capability that is detected or declared, never
   assumed. Where the active provider cannot accept images, the attempt fails
   with a clear message and the image is not silently dropped from the request.
4. An image costs what it costs: it is counted against the context budget by a
   figure derived from the provider's own rule for images, never by `len / 4`,
   and it is labelled estimated.
5. **Damaian does not claim to redact an image.** The secret guarantee is
   restated honestly for this one case, and the design converts it into visible,
   informed consent rather than a promise it cannot keep.
6. **No image the user has not seen is ever sent.** An agent-captured screenshot
   is shown before it goes, because the user did not choose its contents.
7. An exported or searched transcript makes the presence of an image explicit,
   so [spec 53](53_session_search_and_export.md)'s redaction notice is not read
   as covering something it cannot cover.
8. Every image sent is audited by hash, size, dimensions and origin — never by
   content.

## 4. Non-goals

- **Image generation, editing, or annotation.** Input only.
- **OCR.** §5.3 rejects it explicitly rather than by omission.
- **Reading images out of the repository as ordinary context.** An image enters
  because a user attached it or a diagnostic produced it, never because an
  indexer walked past a PNG.
- **Video, audio, or PDF.** PDFs are a recorded non-goal in the roadmap's
  deferred-work section and this spec does not reopen them.
- **Making images searchable.** [Spec 53](53_session_search_and_export.md)
  searches text; an image is found by the message around it.
- **A second attachment surface.** [Spec 44](44_composer_control_density/proposal.md)
  owns the composer's control row, and this extends the existing attach control
  rather than adding a control beside it.
- **Automatically attaching every diagnostic screenshot.** Requirement 6 makes
  that unsafe, and requirement 4 makes it expensive.

## 5. Design

### 5.1 The message shape

`ModelMessage.content: String` becomes a content list internally while keeping
the common case free:

```rust
pub enum ContentPart {
    Text(String),
    /// A local image, referenced rather than inlined until serialisation.
    Image {
        /// Path under the data directory. Never a repository path, and never
        /// a URL — see §5.4.
        path: PathBuf,
        mime_type: String,
        width: u32,
        height: u32,
        /// Content hash, for audit and for deduplicating a re-sent image.
        hash: String,
    },
}
```

A message whose parts are a single `Text` serialises to exactly the string body
it does today — byte-identical, so no provider sees a changed request shape
because the type changed. Only a message actually carrying an image serialises
to the content-array form. This matters beyond tidiness:
[spec 49](49_prompt_cache_accounting_and_reuse.md) depends on prefix stability,
and a request body that changed shape for every message would invalidate every
cached prefix in exchange for nothing.

The image is base64-encoded at serialisation and held as a path until then, so
an image sits in memory once per request rather than once per turn of history.

### 5.2 Provider capability

Requirement 3, following [spec 19](19_token_and_cost_accounting/proposal.md)'s
precedent rather than a name list: `ModelProviderConfig` gains
`supports_image_input: bool`, defaulting to **false**. A provider that accepts
images is configured to say so.

Defaulting false and requiring declaration — rather than probing — is deliberate
here, where spec 19 probed. A usage probe costs one extra request carrying the
same prompt. An image probe would upload an image to a provider that may reject
it, and the failure mode of guessing wrong is a rejected request carrying the
user's screenshot. Cheap to declare, expensive to discover.

Where the active provider does not support images, the attach control says so
before the user picks a file, and a turn cannot be sent with one attached.
Requirement 3's "not silently dropped" is the rule that matters: an image
quietly removed from a request produces an agent confidently answering a
question about a picture it never received, which is worse than a refusal.

### 5.3 The secret scanner cannot read an image

This is the section that decides whether the feature is safe, and its conclusion
is a limitation rather than a mechanism.

`SecretScanner` reads strings. A screenshot of a terminal showing
`export ANTHROPIC_API_KEY=sk-…`, an editor with a `.env` file open, or a browser
with a session token in a query string, is a byte array that no rule in
`secret_scanner.rs` can match. The moment images are accepted, the sentence "no
secret reaches the model, the logs, or a transcript unredacted" stops being true
in general.

Three responses, and this spec takes the third.

1. **Refuse images.** Keeps the guarantee, and is the status quo this spec
   exists to change.
2. **OCR the image, scan the text, redact or refuse.** Rejected on three
   independent grounds, any one sufficient. It needs a bundled OCR engine —
   `AGENTS.md` forbids a Node runtime dependency and a native model is a large
   binary for one feature. OCR is *lossy*: it misses rotated text, low contrast,
   and unusual fonts, so it would find most secrets and not all. And a guarantee
   that holds most of the time is worse than a stated limitation, because the
   user stops looking.
3. **Convert the guarantee into informed consent, and never send an unseen
   image.** Damaian states plainly, once, at the point of first use, that it
   cannot scan an image for secrets and that the user is responsible for what a
   picture contains. Every image is shown at full size before the turn is sent.
   Nothing changes about the text path, which keeps its guarantee intact and
   unqualified.

The asymmetry in requirement 6 follows from this and is the part most likely to
be dropped in implementation: a **user-attached** image was chosen by the person
who is accountable for it, but an **agent-captured** screenshot was framed by
Damaian — of a page the user may not have been looking at, possibly showing a
logged-in session, a token in a URL bar, or a staging environment's data. It
must be displayed and explicitly included by the user before any of its bytes
reach a provider. There is no "always include screenshots" setting, for the same
reason [spec 48](48_provider_limits_and_backpressure/proposal.md) refuses a remembered
model fallback: the risk is not repeatable, so consent cannot be either.

`docs/USER_GUIDE.md` and `docs/TROUBLESHOOTING.md` must both carry the
limitation in plain words. A user who believes redaction covers images will
paste one that a text message would have had cleaned.

### 5.4 What reaches the provider

| Rule | Value |
|---|---|
| Formats | PNG, JPEG, WebP, GIF (first frame). Anything else is refused by type, not attempted |
| Source | A local file the user chose, a pasted or dropped image, or a diagnostic artifact under the data directory |
| **Never** | An image fetched from a URL, including one named by a fetched page ([spec 51](51_external_reference_retrieval.md) §5.3 constraint 5 forbids sub-resource loading, and an image is a sub-resource) |
| Size | A byte cap and a dimension cap; a larger image is downscaled with the downscaling stated, or refused where it cannot be |
| Storage | Copied under the data directory keyed by content hash, so the transcript is stable if the user moves or deletes the original |
| Provenance | Recorded as user-attached or agent-captured, and shown as such in [spec 27](27_context_inspector.md) |

The "never from a URL" row is a boundary worth stating rather than assuming.
Spec 51 lets a model name a URL to fetch as text; if an image could arrive the
same way, a fetched page could put bytes of its choosing into the model's
context by naming them — with none of spec 51's text-conversion, size or
redaction handling in the way.

### 5.5 Diagnostic screenshots

Requirement 2. `WebDiagnosticReport` already carries artifacts with a path and
dimensions, so the report needs no new field: the change is that an artifact of
kind `screenshot` becomes *offerable* to the model rather than only referenced.

The flow is deliberately not automatic. A diagnostic returns its text and its
artifacts; the artifacts render in the transcript; the user includes one, having
seen it, and it joins the next request as an `Image` part with agent-captured
provenance. A model that wants to look at the screenshot it just requested asks
for it, and the asking is answered by a person.

[Spec 12](12_web_app_troubleshooting.md) is `In progress`, so this must be
coordinated with it in the same way [spec 22](22_findings_model_and_panel.md)
§5.4 coordinates its `entries` addition — additively, with `text` and
`artifacts` unchanged, and §7 recording whether that coordination happened or
whether this spec had to work around a closed one.

### 5.6 Cost

Requirement 4. Providers price images by a rule of their own — typically tiles
derived from dimensions — and `len / 4` over a base64 payload is not an
approximation of it but a different quantity entirely, wrong by an order of
magnitude in either direction depending on the image.

So `ModelProviderConfig` carries the provider's image-token rule alongside
`supports_image_input`, and the estimate is computed from width and height by
that rule. The figure is labelled `UsageSource::Estimated` like any other
estimate, and where the provider reports usage
([spec 19](19_token_and_cost_accounting/proposal.md)), the measured number
supersedes it as it does for text.

The user-visible consequence is stated in the composer rather than discovered on
a bill: attaching an image shows its estimated token cost before the turn is
sent. An image is frequently the most expensive thing in a request, and
[spec 21](21_task_plan_progress_and_budget/proposal.md)'s per-turn ceiling can
be reached by two screenshots.

### 5.7 Transcript, export and audit

An image is part of the conversation and is rendered in the transcript as a
thumbnail that opens full size.

[Spec 53](53_session_search_and_export.md)'s export is where requirement 7
bites. That spec's notice says the export was redacted and how many redactions
were made; an export containing a screenshot must additionally say that images
are included and were **not** scanned — otherwise the notice reads as a
guarantee covering the one thing it cannot cover. A Markdown export references
the image file and states this; a JSON export carries the hash and path, not the
bytes.

Audit records the hash, byte size, dimensions, MIME type and provenance. Never
the content, and never a derived description of it.

## 6. Acceptance Criteria

- An image is attached by paste, by drop and through the attach control, and
  reaches a provider that supports images.
- A message carrying only text serialises to a request body byte-identical to
  today's — asserted by test, because [spec 49](49_prompt_cache_accounting_and_reuse.md)'s
  prefix stability depends on it.
- With a provider that does not support images, the attach control states so and
  a turn carrying an image cannot be sent. No request is ever sent with an
  image silently removed — asserted against the constructed request.
- An agent-captured screenshot is not included in any request until the user has
  seen it and included it — asserted by test, including that no configuration
  value can make inclusion automatic.
- An image is never loaded from a URL, including one named by fetched content.
- An unsupported format is refused by type rather than attempted.
- An oversized image is downscaled with the downscaling stated, or refused.
- The estimated token cost of an image is computed from its dimensions by the
  provider's rule, is shown before sending, and is labelled estimated.
- An export containing an image states that images were not scanned for
  secrets, in addition to [spec 53](53_session_search_and_export.md)'s
  redaction notice.
- Audit records hash, size, dimensions and provenance, and no image content
  appears in any log.
- The image limitation appears in `docs/USER_GUIDE.md` and
  `docs/TROUBLESHOOTING.md` in plain words.
- Every quality-gate command from `AGENTS.md` passes.

## 7. Implementation Notes

To be completed during implementation. Record:

- Whether [spec 12](12_web_app_troubleshooting.md) was still open enough to
  coordinate §5.5 additively, or whether this worked around a closed spec.
- The measured token cost of a representative screenshot against the provider's
  reported usage, since §5.6's rule is a formula taken from a vendor and the
  only way to know it is right is to compare it with a measurement.
- Whether any provider configured in this repository supports image input at
  all. If none does, this spec ships a capability nobody can use yet, and that
  fact belongs here rather than being discovered later.

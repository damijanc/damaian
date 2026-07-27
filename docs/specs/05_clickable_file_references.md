# Feature Spec: Clickable In-Text File References

Status: Done
Order: 5 of 5
Related spec sections: `ai_coding_assistant_specification.md` §7.1 (Chat Interface — "File reference links that open the local file in the app or configured editor").

## 1. Motivation

The spec requires the chat interface to provide "File reference links that open the local file in the app or configured editor" (§7.1). This exists today, but only for the explicit **context-file chip list** shown above/around a message — not for file paths the model mentions inline in its prose.

- `renderContextFiles` (`crates/desktop-shell/static/app.js:1786-1805`) renders a clickable button per context file, which on click calls `/api/open-vscode-file` (server handler at `crates/desktop-shell/src/lib.rs:405`, resolved via `open_workspace_path_in_vscode`) to open it in VS Code.
- This mechanism only covers files the context manager actually pulled into the request (`ContextItem`s). It does **not** apply to file paths that appear as plain text inside the assistant's written answer — e.g. "the bug is in `src/auth/middleware.ts`" renders `src/auth/middleware.ts` as inert text (or as an inline-code span once [response formatting](01_response_formatting.md) lands), not a clickable link.

This is a small, self-contained gap best done after response formatting (#1) is in place, since it depends on having a real markdown/inline renderer to hook into.

## 2. Goals

- When rendering an assistant message, detect substrings that look like a repository-relative file path (optionally with a `:line` or `:line:col` suffix, matching common tooling conventions) and render them as clickable links.
- Clicking a detected file reference opens the file the same way the existing context-file chips do — reusing `/api/open-vscode-file` and `open_workspace_path_in_vscode` — with no new backend capability required.
- If the reference includes a line number (e.g. `src/auth/middleware.ts:42`), pass it through so the editor opens at that line, consistent with how `code --goto <path>:<line>` already works (the existing VS Code invocation at `lib.rs:1257` should be checked/extended to support a `:line` suffix if it doesn't already).
- Only link paths that actually resolve to a real file within the current repository — never render a link for a path-shaped string that doesn't exist on disk, to avoid dead/misleading links.

## 3. Non-Goals

- Fuzzy-matching or "did you mean" suggestions for near-miss paths.
- Making arbitrary prose words clickable (only strings that parse as a plausible relative file path, then are verified against the filesystem, qualify).
- Opening files in editors other than the currently configured one (VS Code today) — follow whatever editor configuration the app already supports; don't add new editor integrations in this spec.
- Linking file references inside code blocks' rendered content (comments mentioning a path inside a fenced code block should stay plain code text, not become clickable, to avoid visual noise in code samples).

## 4. Design

### 4.1 Detection

After markdown parsing ([response formatting](01_response_formatting.md) introduces a real parser), walk the rendered text nodes (excluding code-fence content, per Non-Goals) for tokens matching a conservative file-path pattern: contains at least one `/` or a recognized extension, no spaces, optionally followed by `:<digits>` or `:<digits>:<digits>`. Keep the pattern intentionally conservative — false negatives (a real path not linked) are far less harmful than false positives (random text incorrectly turned into a broken link).

### 4.2 Verification

Before rendering a match as a link, verify the path exists relative to the current repository root. This check should go through the same `FileAccessController`/`path_policy` boundary already used elsewhere (§7.3) — not a raw filesystem check — so restricted files (`.env`, credentials, etc., per §7.3's default-restricted list) are never turned into a clickable open-in-editor link even if they happen to exist on disk.

Recommend doing this verification server-side (Rust, in `workspace-engine` or `desktop-shell`) rather than in `app.js`, both to reuse the existing access-control code and to avoid shipping repository file-existence logic to the frontend.

### 4.3 Rendering

Render matched, verified paths as `<button class="file-reference">` elements (or `<a>` with a `javascript:`-free click handler, matching the existing `context-file` button pattern at `app.js:1790-1802`) wired to the same `/api/open-vscode-file` call, passing the detected line number through if present.

### 4.4 Line-number support in the editor-open path

Check whether `open_workspace_path_in_vscode` / the `code` command invocation (`lib.rs:1257`) already supports a line number; if not, extend it to accept an optional line (and column) and pass `--goto <path>:<line>[:<col>]` to the `code` CLI, which supports this natively.

## 5. Acceptance Criteria

- An assistant response containing "see `src/auth/middleware.ts:42`" (whether in a code span or plain text) renders that path as a clickable element distinct from surrounding text.
- Clicking it opens the file in VS Code at line 42.
- A response mentioning a path-shaped string that does not exist in the repository (e.g. a typo, or a path from an unrelated example) renders as plain text, not a broken link.
- A response mentioning a restricted file's path (e.g. `.env`) does not render a clickable link for it, consistent with §7.3's default-restricted access rules.
- A file path that appears only inside a fenced code block (e.g. as part of an example command) is not turned into a clickable link.

## 6. Open Questions / Decisions Needed

- Whether detection/verification should re-run on every render (cheap for message-length text) or be cached per message once computed, given messages are immutable after streaming completes.
- Whether to support absolute paths in addition to repository-relative ones, given the existing context-file chips and `FileAccessController` are scoped to selected repository roots (§7.3) — recommend repository-relative only, consistent with that existing scope.

## 7. Implementation Notes (as built)

Both open questions resolved as recommended: detection re-runs on every finalize render (no caching — it only runs once per message, on stream-complete or history load, and is cheap); only repository-relative paths are linked (the verifier passes `allow_outside_root: false`).

- **Detection (server-side, `render.rs`)**: added `render_markdown_to_html_with_file_links(markdown, verifier)` alongside the existing `render_markdown_to_html` (which now delegates to a shared core with a no-op verifier, so its behavior and all prior callers/tests are unchanged). During the pulldown-cmark event walk, `Event::Text` outside fenced code blocks and inline `Event::Code` spans are scanned for path-like tokens; fenced code block content is never scanned (per Non-Goals). A token qualifies via a conservative `looks_like_path` (no whitespace, and either contains `/` or ends in a short alphanumeric extension — deliberately rejecting bare words and extensionless dotfiles like `.env`), after peeling an optional `:line` / `:line:col` suffix. Surrounding punctuation (`()[]{}"'`\`.,;!?`) is split off as literal text so unlinked prose round-trips verbatim; when no link is found in a text run, the original text is emitted unchanged.
- **Verification (desktop-shell)**: the verifier closure runs through the repo's existing `path_policy.resolve_existing(..., allow_outside_root=false)` + `assert_not_restricted` + an `is_file` check — the same access-control boundary as context-file chips — so nonexistent paths, paths outside the repo, and restricted files (`.env`, credentials) are never linked. `/api/render-markdown` now takes an optional `repo`; without a valid one it falls back to the plain link-free render rather than erroring.
- **Rendering**: verified prose refs become `<button class="file-reference" data-path data-line data-col>`; verified inline-code refs become `<code class="file-reference" role="button" tabindex="0" ...>` (keeping monospace). Both escape the display text and use the verifier's canonical relative path for `data-path`.
- **Opening at a line**: `/api/open-vscode-file` now accepts optional `line`/`col`; `launch_vscode` gained line/col params. Since macOS `open -a` can't jump to a line, it tries `code --goto path:line[:col]` first and falls back to `open -a` (opening the file without the line) when the `code` CLI isn't on PATH. `open_in_vscode` (whole-repo) passes `None, None`.
- **Frontend (`app.js`)**: `finalizeChatMessage` passes the current repo to `/api/render-markdown` and calls a new `wireFileReferences` that attaches click (and Enter/Space for the `<code>` variant) handlers, POSTing `path` + `line` + `col` to `/api/open-vscode-file` — reusing the exact endpoint the context-file chips use. New `.file-reference` CSS styles both variants as accent-colored underlined links.

**Verification**: 7 new `render.rs` unit tests (prose link, `:line:col` → data attrs, unverified path not linked, path inside fenced block not linked, inline-code link, trailing-punctuation stripping, and default render never links); 1 new `desktop-shell` test driving `render_markdown_with_optional_file_links` against a real temp repo (real file linked with line; missing path and restricted `.env` left as plain text; no-repo → no links). Browser-verified end-to-end at the wiring level: the rendered `.file-reference` displays as an accent underlined link, and clicking it POSTs `repo=…&path=src/auth.rs&line=12` to `/api/open-vscode-file`. The actual VS Code launch and the full Tauri chat round-trip weren't browser-exercised (the desktop UI's API token comes from a Tauri-only bootstrap unavailable to a plain browser). `workspace-engine` lib tests 26 → 33; full `cargo test --workspace` green, zero regressions.

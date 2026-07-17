# Feature Spec: Clickable In-Text File References

Status: Not started
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

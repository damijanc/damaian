# Feature Spec: Response Formatting (Markdown + Syntax Highlighting)

Status: Done
Order: 1 of 5
Related spec sections: `ai_coding_assistant_specification.md` §7.1 (Chat Interface), acceptance criteria "Markdown rendering with syntax highlighting."

## 1. Motivation

The specification requires the chat interface to support "Markdown rendering with syntax highlighting" (§7.1). Today this is only half-implemented in the desktop client and entirely absent in the CLI:

- **Desktop UI** (`crates/desktop-shell/static/app.js`): a hand-rolled line-based renderer (`renderMarkdown`, `app.js:1659`) handles fenced code blocks, headings (`#`–`####`), bullet lists, and pipe tables, but:
  - `renderInlineMarkdown` (`app.js:1623`) only converts backtick inline code — no bold (`**`), italic (`*`/`_`), or link (`[text](url)`) support.
  - Fenced code blocks capture a `language-xxx` class but perform no actual tokenizing/highlighting — all code renders as flat monospace text.
  - No ordered lists, no nested lists, no blockquotes.
- **CLI** (`crates/damaian-cli/src/main.rs:227`): the `ask` command streams raw tokens straight to stdout via `print!("{token}")` with zero formatting — no color, no code fence detection, nothing.
- No markdown or syntax-highlighting library exists anywhere in the dependency tree (confirmed absent from `Cargo.lock` and from the static JS assets).

This is the highest-visibility, lowest-architectural-risk gap: it affects every single assistant response and requires no changes to the workspace engine's core logic (indexing, patching, tool-calling).

## 2. Goals

- Render assistant messages in the desktop UI with full CommonMark-subset support: bold, italic, inline code, links, ordered/unordered lists (including nesting), blockquotes, headings, and tables (already present).
- Apply real syntax highlighting to fenced code blocks, keyed off the language hint from the fence (e.g. ` ```rust `), covering at minimum the languages listed in the spec's target repository types (§4.3): JS/TS, Python, Go, PHP, Rust, Java/Kotlin, plus JSON/YAML/shell/Markdown for config and command output.
- Render the same formatting for **historical** messages on session reload (`renderMessages`, `app.js:1781`), not just for live streaming.
- Give the CLI's `ask` command readable terminal output: ANSI-colored headings/emphasis and syntax-highlighted (or at minimum visually delimited) code blocks, degrading gracefully to plain text when stdout is not a TTY or `NO_COLOR`/`--no-color` is set.
- Keep formatting incremental-render-safe: partial markdown mid-stream (e.g. an unterminated code fence) must not produce broken HTML or visual corruption; it should resolve cleanly once the fence closes.

## 3. Non-Goals

- Full CommonMark spec compliance (footnotes, HTML blocks, reference-style links are out of scope for v1).
- Rendering arbitrary embedded HTML from the model — inputs must still be escaped; only the constrained markdown subset above is interpreted.
- A rich WYSIWYG editor for user input; this spec covers rendering of assistant output only.
- Client-side plugins/extensibility for custom renderers.

## 4. Design

### 4.1 Desktop UI

Two implementation paths, in order of preference:

1. **Move rendering server-side (Rust) into `workspace-engine`.** Add `pulldown-cmark` (pure Rust, no unsafe, actively maintained) to parse markdown into an HTML string, and `syntect` for syntax highlighting of fenced code blocks (it ships Sublime-compatible syntax definitions covering all target languages and produces either inline-styled HTML or a token stream the frontend can map to CSS classes). The workspace engine already renders other structured text server-side (e.g. `patch_diff_text`), so this keeps the "dependency-free JS" philosophy of `desktop-shell/static` intact and centralizes formatting logic for reuse by both the desktop UI and CLI.
   - Expose a small helper, e.g. `render_markdown_to_html(markdown: &str) -> String`, from `workspace-engine`, called by `desktop-app`'s Tauri command layer before content reaches the frontend, or exposed as a new Tauri command the frontend invokes per message.
   - For **streaming** messages, re-render the full accumulated buffer on each token (cheap for message-sized text; avoids incremental-parser complexity) rather than trying to diff-patch the DOM.
2. **Fallback: extend `app.js` in place.** If keeping all rendering in JS is preferred (e.g. to avoid a Tauri round-trip per token), extend `renderInlineMarkdown`/`renderMarkdown` to add bold/italic/link/ordered-list/blockquote support, and vendor a small no-build syntax highlighter (e.g. a single-file `highlight.js` core build with only the target languages registered) as a static asset referenced from `index.html` — no npm/bundler required, consistent with the existing static-file approach.

Recommendation: path 1 (Rust-side via `pulldown-cmark` + `syntect`), because it is reusable by the CLI (§4.2) and keeps the trust boundary (escaping, sanitization) in the same memory-safe layer that already owns secret redaction.

Either path must:
- HTML-escape all literal text before/around interpreted markdown constructs (already partially done via `escapeHtml`, `app.js:1615`) to prevent XSS from model output.
- Add CSS rules for highlighted tokens (`style.css:640-770` region already hosts `.message-body` rules) — a small palette (keyword/string/comment/number/function) is sufficient; must work in both light and dark mode if the app supports theme switching.

### 4.2 CLI

Add ANSI formatting to `damaian-cli`'s `ask` command output:
- Reuse the same Rust-side markdown parser (`pulldown-cmark`) from `workspace-engine` so formatting logic is not duplicated.
- Walk the parsed markdown events and emit ANSI escape codes for emphasis/headings/inline code, and pass fenced-code-block content through `syntect`'s terminal-color highlighter (it has a built-in `syntect::easy::HighlightLines` + `as_24_bit_terminal_escaped` path made for exactly this).
- Detect non-TTY stdout (piped output) via `std::io::IsTerminal` and fall back to plain text; respect a `--no-color` flag and the `NO_COLOR` env var convention.
- Since `ask` currently streams token-by-token (`main.rs:227`), buffer and re-render per line or per completed block rather than per raw token, to avoid emitting broken ANSI sequences mid-escape-sequence.

### 4.3 Shared module placement

Add a new `render` module in `workspace-engine` (e.g. `crates/workspace-engine/src/render.rs`) owning both the HTML-for-desktop and ANSI-for-CLI code paths, so the two frontends stay in sync and future formatting fixes only need to happen once.

## 5. Acceptance Criteria

- A response containing `**bold**`, `_italic_`, `` `inline code` ``, and `[a link](https://example.com)` renders each construct distinctly in the desktop UI (not literal asterisks/underscores/brackets).
- A response with a ` ```python ` fenced block containing a function definition shows distinct colors for keywords, strings, and comments in the desktop UI.
- Reloading a past session (`renderMessages`) shows identically formatted output to what was shown live.
- A message that arrives with an unterminated code fence mid-stream does not visually break the message bubble; once the fence closes, the block renders correctly.
- Running `damaian ask ...` in an interactive terminal shows colored headings/emphasis and highlighted code blocks; piping the same command's output to a file or another process (`damaian ask ... | cat`) yields clean plain text with no stray escape codes.
- No raw HTML tags typed by the model (e.g. `<script>`) execute or render as live markup in the desktop UI — they appear as literal escaped text.

## 6. Open Questions / Decisions Needed

- Confirm whether `desktop-app`/`desktop-shell` should take a Tauri round-trip per render call, or whether pushing fully-rendered HTML alongside each streamed token (computed Rust-side) is preferred for latency. Recommendation: render server-side only on message completion and on the last N buffered tokens during streaming (e.g. throttle to ~10/sec), not on every token.
- Which theme/dark-mode strategy the syntax-highlight CSS palette should follow, if the desktop UI has (or plans) a dark mode toggle.

## 7. Implementation Notes (as built)

Landed as a hybrid of §4.1's two paths rather than picking one exclusively:

- `workspace-engine::render` (`crates/workspace-engine/src/render.rs`) holds `render_markdown_to_html` (pulldown-cmark + syntect, `ClassStyle::SpacedPrefixed { prefix: "hl-" }`, raw HTML/inline HTML folded into the escaped text path) and `render_markdown_to_ansi` (same parser, ANSI escapes for emphasis/headings/links, syntect terminal highlighting for code blocks). Both are exported from `workspace_engine`'s crate root.
- **CLI**: `damaian ask` no longer prints tokens live — it buffers (the callback is a no-op) and prints `result.response` once, through `render_markdown_to_ansi` when stdout is a TTY and neither `--no-color` nor `NO_COLOR` is set, otherwise prints the raw response unchanged. This is the simplification the spec's §4.2 anticipated ("buffer ... rather than per raw token"), taken further: buffer the whole answer rather than per-line.
- **Desktop UI**: kept the existing per-token SSE streaming and client-side `renderMarkdown`/`renderInlineMarkdown` in `app.js` for the *live* typing view (extended in place with bold/italic/link support — §4.1's "fallback" path), but added a new `POST /api/render-markdown` endpoint (`crates/desktop-shell/src/lib.rs`) that calls `render_markdown_to_html` server-side. A new `finalizeChatMessage` swaps a message's `innerHTML` for the real syntax-highlighted render once the message is final: after the SSE `done` event, and for every assistant message when a session's history loads (`renderMessages`). This avoids restructuring the SSE wire protocol while still giving true highlighting for the text people actually read/copy. Token color CSS lives under `.hl-*` classes in `style.css`, tuned against the existing dark `--code` background.
- Verified with: `workspace-engine`'s `render` unit tests, a real end-to-end TCP test against `/api/render-markdown` in `desktop-shell` (`render_markdown_endpoint_returns_syntax_highlighted_html`), and manual CLI smoke tests confirming colored TTY output, clean plain-text piped output, and `--no-color`/`NO_COLOR` suppression. The desktop UI's visual result was not verified in an actual browser/Tauri window — its bootstrap flow requires a real Tauri IPC call for the API token, which isn't reachable by pointing a browser at the raw HTTP server (see `crates/desktop-shell/static/app.js`'s `startBootstrap`).

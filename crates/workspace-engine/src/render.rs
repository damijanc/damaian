use pulldown_cmark::{CodeBlockKind, CowStr, Event, Options, Parser, Tag, TagEnd};
use std::path::Path;
use std::sync::OnceLock;
use syntect::easy::HighlightLines;
use syntect::highlighting::ThemeSet;
use syntect::html::{ClassStyle, ClassedHTMLGenerator};
use syntect::parsing::SyntaxSet;
use syntect::util::{LinesWithEndings, as_24_bit_terminal_escaped};

/// Resolves a path-like token to a repository-relative path if it is a real,
/// non-restricted file the assistant is allowed to reference; returns `None`
/// otherwise. Injected by the caller (desktop-shell) so the access-control
/// and filesystem checks stay behind the existing `path_policy` boundary
/// rather than being reimplemented here or shipped to the frontend.
pub type FileLinkVerifier<'a> = dyn Fn(&str) -> Option<String> + 'a;

fn no_file_links(_candidate: &str) -> Option<String> {
    None
}

const MARKDOWN_OPTIONS: Options = Options::ENABLE_TABLES
    .union(Options::ENABLE_STRIKETHROUGH)
    .union(Options::ENABLE_TASKLISTS);

const CLI_THEME: &str = "base16-ocean.dark";

fn syntax_set() -> &'static SyntaxSet {
    static SET: OnceLock<SyntaxSet> = OnceLock::new();
    SET.get_or_init(SyntaxSet::load_defaults_newlines)
}

fn theme_set() -> &'static ThemeSet {
    static SET: OnceLock<ThemeSet> = OnceLock::new();
    SET.get_or_init(ThemeSet::load_defaults)
}

/// Renders assistant markdown to HTML with syntax-highlighted code blocks,
/// suitable for direct use as `innerHTML` in the desktop chat log. Raw HTML
/// and inline HTML found in the input are escaped rather than passed
/// through, since the input is untrusted model output, not author-controlled
/// markup.
pub fn render_markdown_to_html(markdown: &str) -> String {
    render_markdown_html(markdown, &no_file_links)
}

/// Like [`render_markdown_to_html`] but turns file-path references that
/// `verifier` confirms into clickable `.file-reference` elements (both in
/// prose and in inline code spans; never inside fenced code blocks). Paths
/// may carry a `:line` or `:line:col` suffix, surfaced as `data-line` /
/// `data-col` attributes for the frontend to pass through when opening.
pub fn render_markdown_to_html_with_file_links(
    markdown: &str,
    verifier: &FileLinkVerifier<'_>,
) -> String {
    render_markdown_html(markdown, verifier)
}

fn render_markdown_html(markdown: &str, verifier: &FileLinkVerifier<'_>) -> String {
    let parser = Parser::new_ext(markdown, MARKDOWN_OPTIONS);
    let mut events: Vec<Event> = Vec::new();
    let mut in_code_block = false;
    let mut code_lang = String::new();
    let mut code_buffer = String::new();

    for event in parser {
        match event {
            Event::Start(Tag::CodeBlock(kind)) => {
                in_code_block = true;
                code_lang = fence_language(&kind);
                code_buffer.clear();
            }
            Event::End(TagEnd::CodeBlock) => {
                in_code_block = false;
                events.push(Event::Html(CowStr::from(highlight_code_block_html(
                    &code_lang,
                    &code_buffer,
                ))));
            }
            Event::Text(text) if in_code_block => code_buffer.push_str(&text),
            Event::Text(text) => events.extend(file_ref_events(&text, verifier)),
            Event::Code(code) => events.push(inline_code_event(&code, verifier)),
            // Raw HTML from the model must never reach `innerHTML` unescaped;
            // fold it back into the escaped text path pulldown-cmark already
            // uses for `Event::Text`.
            Event::Html(raw) | Event::InlineHtml(raw) => {
                events.push(Event::Text(CowStr::from(raw.into_string())));
            }
            other => events.push(other),
        }
    }

    let mut html_out = String::new();
    pulldown_cmark::html::push_html(&mut html_out, events.into_iter());
    html_out
}

/// A verified file reference found inside a run of prose. `prefix`/`suffix`
/// are the exact surrounding characters of the original word (leading/
/// trailing punctuation) so the untouched parts round-trip verbatim;
/// `display` is shown inside the link exactly as written.
struct FileRef {
    prefix: String,
    display: String,
    suffix: String,
    rel_path: String,
    line: Option<u32>,
    col: Option<u32>,
}

const LEADING_PUNCT: &[char] = &['(', '[', '{', '"', '\'', '`', '<'];
const TRAILING_PUNCT: &[char] = &[')', ']', '}', '"', '\'', '`', '>', '.', ',', ';', '!', '?'];

fn detect_file_ref(word: &str, verifier: &FileLinkVerifier<'_>) -> Option<FileRef> {
    let after_leading = word.trim_start_matches(LEADING_PUNCT);
    let prefix_len = word.len() - after_leading.len();
    let display = after_leading.trim_end_matches(TRAILING_PUNCT);
    if display.is_empty() {
        return None;
    }
    let (path_part, line, col) = peel_line_col(display);
    if !looks_like_path(path_part) {
        return None;
    }
    let rel_path = verifier(path_part)?;
    let suffix_start = prefix_len + display.len();
    Some(FileRef {
        prefix: word[..prefix_len].to_string(),
        display: display.to_string(),
        suffix: word[suffix_start..].to_string(),
        rel_path,
        line,
        col,
    })
}

/// Peels up to two trailing `:<digits>` groups off a path token, returning
/// the bare path plus the line and (optional) column. `foo.rs` → no line;
/// `foo.rs:12` → line 12; `foo.rs:12:3` → line 12, col 3.
fn peel_line_col(display: &str) -> (&str, Option<u32>, Option<u32>) {
    let mut path = display;
    let mut nums: Vec<u32> = Vec::new();
    while nums.len() < 2 {
        let Some((head, tail)) = path.rsplit_once(':') else {
            break;
        };
        if tail.is_empty() || !tail.bytes().all(|byte| byte.is_ascii_digit()) {
            break;
        }
        let Ok(value) = tail.parse::<u32>() else {
            break;
        };
        nums.push(value);
        path = head;
    }
    // `nums` is collected right-to-left: [line] or [col, line].
    match nums.len() {
        1 => (path, Some(nums[0]), None),
        2 => (path, Some(nums[1]), Some(nums[0])),
        _ => (path, None, None),
    }
}

/// Conservative check that a token could be a repository-relative file path:
/// no whitespace, and either it contains a `/` or ends in a short
/// alphanumeric extension. Deliberately rejects bare words (`hello`) and
/// extensionless dotfiles (`.env`) to avoid false-positive links.
fn looks_like_path(candidate: &str) -> bool {
    if candidate.is_empty() || candidate.contains(char::is_whitespace) {
        return false;
    }
    if candidate.contains('/') {
        return true;
    }
    Path::new(candidate)
        .extension()
        .map(|ext| {
            let ext = ext.to_string_lossy();
            !ext.is_empty() && ext.len() <= 10 && ext.chars().all(|c| c.is_ascii_alphanumeric())
        })
        .unwrap_or(false)
}

fn file_ref_events(text: &str, verifier: &FileLinkVerifier<'_>) -> Vec<Event<'static>> {
    let mut events: Vec<Event> = Vec::new();
    let mut pending = String::new();
    let mut linked_any = false;
    let mut last = 0;
    for (start, end) in word_ranges(text) {
        pending.push_str(&text[last..start]);
        let word = &text[start..end];
        match detect_file_ref(word, verifier) {
            Some(file_ref) => {
                pending.push_str(&file_ref.prefix);
                if !pending.is_empty() {
                    events.push(Event::Text(CowStr::from(std::mem::take(&mut pending))));
                }
                events.push(Event::Html(CowStr::from(file_ref_html(
                    &file_ref.rel_path,
                    file_ref.line,
                    file_ref.col,
                    &file_ref.display,
                    false,
                ))));
                pending.push_str(&file_ref.suffix);
                linked_any = true;
            }
            None => pending.push_str(word),
        }
        last = end;
    }
    pending.push_str(&text[last..]);
    if !linked_any {
        // No links found — behave exactly like the default text path so
        // unlinked prose is byte-for-byte unchanged.
        return vec![Event::Text(CowStr::from(text.to_string()))];
    }
    if !pending.is_empty() {
        events.push(Event::Text(CowStr::from(pending)));
    }
    events
}

fn inline_code_event(code: &str, verifier: &FileLinkVerifier<'_>) -> Event<'static> {
    let trimmed = code.trim();
    if let Some(file_ref) = detect_file_ref(trimmed, verifier) {
        if file_ref.prefix.is_empty() && file_ref.suffix.is_empty() {
            return Event::Html(CowStr::from(file_ref_html(
                &file_ref.rel_path,
                file_ref.line,
                file_ref.col,
                &file_ref.display,
                true,
            )));
        }
    }
    Event::Code(CowStr::from(code.to_string()))
}

fn file_ref_html(
    rel_path: &str,
    line: Option<u32>,
    col: Option<u32>,
    display: &str,
    inline_code: bool,
) -> String {
    let line_attr = line.map(|l| format!(" data-line=\"{l}\"")).unwrap_or_default();
    let col_attr = col.map(|c| format!(" data-col=\"{c}\"")).unwrap_or_default();
    if inline_code {
        format!(
            "<code class=\"file-reference\" role=\"button\" tabindex=\"0\" data-path=\"{}\"{}{}>{}</code>",
            escape_attr(rel_path),
            line_attr,
            col_attr,
            escape_html_text(display)
        )
    } else {
        format!(
            "<button type=\"button\" class=\"file-reference\" data-path=\"{}\"{}{}>{}</button>",
            escape_attr(rel_path),
            line_attr,
            col_attr,
            escape_html_text(display)
        )
    }
}

/// Byte ranges of maximal non-whitespace runs, so the whitespace between
/// them can be preserved verbatim when reconstructing the text.
fn word_ranges(text: &str) -> Vec<(usize, usize)> {
    let mut ranges = Vec::new();
    let mut start: Option<usize> = None;
    for (index, character) in text.char_indices() {
        if character.is_whitespace() {
            if let Some(begin) = start.take() {
                ranges.push((begin, index));
            }
        } else if start.is_none() {
            start = Some(index);
        }
    }
    if let Some(begin) = start {
        ranges.push((begin, text.len()));
    }
    ranges
}

/// Renders assistant markdown to an ANSI-colored terminal string. Callers
/// should only use this when stdout is a TTY and color hasn't been disabled;
/// otherwise prefer printing `markdown` unchanged so piped output stays
/// clean plain text.
pub fn render_markdown_to_ansi(markdown: &str) -> String {
    let parser = Parser::new_ext(markdown, MARKDOWN_OPTIONS);
    let mut out = String::new();
    let mut in_code_block = false;
    let mut code_lang = String::new();
    let mut code_buffer = String::new();
    let mut link_targets: Vec<String> = Vec::new();

    for event in parser {
        match event {
            Event::Start(Tag::CodeBlock(kind)) => {
                in_code_block = true;
                code_lang = fence_language(&kind);
                code_buffer.clear();
            }
            Event::End(TagEnd::CodeBlock) => {
                in_code_block = false;
                out.push_str(&highlight_code_block_ansi(&code_lang, &code_buffer));
            }
            Event::Text(text) => {
                if in_code_block {
                    code_buffer.push_str(&text);
                } else {
                    out.push_str(&text);
                }
            }
            Event::Code(text) => out.push_str(&format!("\x1b[36m{text}\x1b[0m")),
            Event::Start(Tag::Heading { level, .. }) => {
                out.push_str(&format!("\n\x1b[1;36m{} ", "#".repeat(level as usize)));
            }
            Event::End(TagEnd::Heading(_)) => out.push_str("\x1b[0m\n"),
            Event::Start(Tag::Strong) => out.push_str("\x1b[1m"),
            Event::End(TagEnd::Strong) => out.push_str("\x1b[22m"),
            Event::Start(Tag::Emphasis) => out.push_str("\x1b[3m"),
            Event::End(TagEnd::Emphasis) => out.push_str("\x1b[23m"),
            Event::Start(Tag::BlockQuote(_)) => out.push_str("\x1b[2m"),
            Event::End(TagEnd::BlockQuote(_)) => out.push_str("\x1b[22m\n"),
            Event::Start(Tag::Item) => out.push_str("  - "),
            Event::End(TagEnd::Item) => out.push('\n'),
            Event::Start(Tag::Link { dest_url, .. }) => {
                link_targets.push(dest_url.to_string());
                out.push_str("\x1b[4m");
            }
            Event::End(TagEnd::Link) => {
                out.push_str("\x1b[24m");
                if let Some(target) = link_targets.pop() {
                    out.push_str(&format!(" ({target})"));
                }
            }
            Event::End(TagEnd::Paragraph) => out.push_str("\n\n"),
            Event::SoftBreak | Event::HardBreak => out.push('\n'),
            Event::Rule => out.push_str("\n---\n"),
            _ => {}
        }
    }
    out.trim_end().to_string()
}

fn fence_language(kind: &CodeBlockKind) -> String {
    match kind {
        CodeBlockKind::Fenced(lang) => lang.to_string(),
        CodeBlockKind::Indented => String::new(),
    }
}

fn highlight_code_block_html(lang: &str, code: &str) -> String {
    let ss = syntax_set();
    let syntax = ss
        .find_syntax_by_token(lang.trim())
        .unwrap_or_else(|| ss.find_syntax_plain_text());
    let mut generator = ClassedHTMLGenerator::new_with_class_style(
        syntax,
        ss,
        ClassStyle::SpacedPrefixed { prefix: "hl-" },
    );
    for line in LinesWithEndings::from(code) {
        let _ = generator.parse_html_for_line_which_includes_newline(line);
    }
    let body = generator.finalize();
    format!(
        "<pre class=\"code-block\"><code class=\"language-{}\">{}</code></pre>",
        escape_attr(lang.trim()),
        body
    )
}

fn highlight_code_block_ansi(lang: &str, code: &str) -> String {
    let ss = syntax_set();
    let ts = theme_set();
    let syntax = ss
        .find_syntax_by_token(lang.trim())
        .unwrap_or_else(|| ss.find_syntax_plain_text());
    let Some(theme) = ts.themes.get(CLI_THEME).or_else(|| ts.themes.values().next()) else {
        return code.to_string();
    };
    let mut highlighter = HighlightLines::new(syntax, theme);
    let mut out = String::from("\n");
    for line in LinesWithEndings::from(code) {
        let Ok(ranges) = highlighter.highlight_line(line, ss) else {
            out.push_str(line);
            continue;
        };
        out.push_str(&as_24_bit_terminal_escaped(&ranges, false));
    }
    out.push_str("\x1b[0m\n");
    out
}

fn escape_attr(value: &str) -> String {
    value
        .replace('&', "&amp;")
        .replace('"', "&quot;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
}

fn escape_html_text(value: &str) -> String {
    value
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn renders_inline_emphasis_and_links() {
        let html = render_markdown_to_html("**bold** and _italic_ and [link](https://example.com)");
        assert!(html.contains("<strong>bold</strong>"));
        assert!(html.contains("<em>italic</em>"));
        assert!(html.contains("<a href=\"https://example.com\">link</a>"));
    }

    #[test]
    fn escapes_raw_html_instead_of_passing_it_through() {
        let html = render_markdown_to_html("<script>alert(1)</script>");
        assert!(!html.contains("<script>"));
        assert!(html.contains("&lt;script&gt;"));
    }

    #[test]
    fn highlights_fenced_code_blocks_with_language_classes() {
        let html = render_markdown_to_html("```rust\nfn main() {}\n```");
        assert!(html.contains("class=\"code-block\""));
        assert!(html.contains("hl-"));
        assert!(html.contains("fn"));
    }

    #[test]
    fn ansi_render_colors_headings_and_code() {
        let ansi = render_markdown_to_ansi("# Title\n\n```rust\nfn main() {}\n```");
        assert!(ansi.contains("\x1b["));
        assert!(ansi.contains("Title"));
        // Highlighted tokens are individually wrapped in escape codes, so
        // check for the pieces rather than one contiguous "fn main" run.
        assert!(ansi.contains("fn"));
        assert!(ansi.contains("main"));
    }

    #[test]
    fn ansi_render_leaves_plain_text_readable() {
        let ansi = render_markdown_to_ansi("plain paragraph with no formatting");
        assert!(ansi.contains("plain paragraph with no formatting"));
    }

    // Treats every path ending in `.rs` (with any `:line` stripped) as a
    // real, allowed file, so link-detection logic can be tested without a
    // filesystem.
    fn rs_verifier(candidate: &str) -> Option<String> {
        candidate.ends_with(".rs").then(|| candidate.to_string())
    }

    #[test]
    fn links_verified_file_reference_in_prose() {
        let html = render_markdown_to_html_with_file_links(
            "The bug is in src/auth.rs today.",
            &rs_verifier,
        );
        assert!(html.contains("class=\"file-reference\""));
        assert!(html.contains("data-path=\"src/auth.rs\""));
        assert!(html.contains(">src/auth.rs</button>"));
        // Surrounding prose is preserved.
        assert!(html.contains("The bug is in "));
        assert!(html.contains(" today."));
    }

    #[test]
    fn passes_line_and_column_suffix_as_data_attributes() {
        let html = render_markdown_to_html_with_file_links("see src/auth.rs:42:7 now", &rs_verifier);
        assert!(html.contains("data-path=\"src/auth.rs\""));
        assert!(html.contains("data-line=\"42\""));
        assert!(html.contains("data-col=\"7\""));
        assert!(html.contains(">src/auth.rs:42:7</button>"));
    }

    #[test]
    fn does_not_link_unverified_path() {
        let html =
            render_markdown_to_html_with_file_links("check src/missing.ts here", &rs_verifier);
        assert!(!html.contains("file-reference"));
        assert!(html.contains("src/missing.ts"));
    }

    #[test]
    fn does_not_link_paths_inside_fenced_code_blocks() {
        let html = render_markdown_to_html_with_file_links(
            "```\nedit src/auth.rs now\n```",
            &rs_verifier,
        );
        assert!(!html.contains("file-reference"));
        assert!(html.contains("src/auth.rs"));
    }

    #[test]
    fn links_file_reference_written_as_inline_code() {
        let html =
            render_markdown_to_html_with_file_links("open `src/auth.rs:10` please", &rs_verifier);
        assert!(html.contains("<code class=\"file-reference\""));
        assert!(html.contains("data-path=\"src/auth.rs\""));
        assert!(html.contains("data-line=\"10\""));
    }

    #[test]
    fn strips_trailing_punctuation_when_linking() {
        let html =
            render_markdown_to_html_with_file_links("look at (src/auth.rs).", &rs_verifier);
        assert!(html.contains(">src/auth.rs</button>"));
        // The parenthesis and period stay as literal text around the link.
        assert!(html.contains("("));
        assert!(html.contains(").") || html.contains(")."));
    }

    #[test]
    fn default_html_render_never_links_files() {
        let html = render_markdown_to_html("The bug is in src/auth.rs today.");
        assert!(!html.contains("file-reference"));
    }
}

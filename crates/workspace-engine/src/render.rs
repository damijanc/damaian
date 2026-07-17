use pulldown_cmark::{CodeBlockKind, CowStr, Event, Options, Parser, Tag, TagEnd};
use std::sync::OnceLock;
use syntect::easy::HighlightLines;
use syntect::highlighting::ThemeSet;
use syntect::html::{ClassStyle, ClassedHTMLGenerator};
use syntect::parsing::SyntaxSet;
use syntect::util::{LinesWithEndings, as_24_bit_terminal_escaped};

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
}

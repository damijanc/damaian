use std::path::Path;

use serde::{Deserialize, Serialize};

use SymbolKind::{
    Class, Constant, Enum, Function, Interface, Method, Module, Struct, Trait, Type, Variable,
};

/// What kind of declaration a symbol is. Spec 25 §5.1.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SymbolKind {
    Function,
    Method,
    Struct,
    Enum,
    Class,
    Interface,
    Trait,
    Type,
    Constant,
    Variable,
    Module,
    Unknown,
}

/// What source text alone says about one declaration. Task 2 turns it into a
/// `Symbol` by adding the path, root and provenance.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Declaration {
    pub name: String,
    pub kind: SymbolKind,
    /// 1-based.
    pub line: u32,
    /// 1-based, the first character of the name.
    pub column: Option<u32>,
    /// The enclosing type, for a Rust method inside `impl Type` or
    /// `impl Trait for Type`: the `Type`.
    pub container: Option<String>,
}

/// Finds declarations line by line, by keyword prefix, as `extract_symbols`
/// always has. It is a heuristic, not a parser (spec 25 §4). What it adds is a
/// scanner of comment and string delimiters, so a declaration inside either is
/// not matched, a brace depth, so JavaScript locals are not declarations, and
/// for Rust the `impl` or `trait` a method belongs to.
pub fn extract_declarations(content: &str, language: &str) -> Vec<Declaration> {
    let Some(syntax) = syntax_for(language) else {
        return Vec::new();
    };
    let mut lexical = Lexical::Code;
    let mut depth = 0usize;
    // Each open `impl` or `trait` body: the container's name and the brace
    // depth inside it.
    let mut containers: Vec<(String, usize)> = Vec::new();
    let mut pending: Option<PendingContainer> = None;
    let mut declarations = Vec::new();

    for (index, line) in content.lines().enumerate() {
        let chars: Vec<char> = line.chars().collect();
        let code: String = mask_line(&chars, &mut lexical, &syntax)
            .into_iter()
            .collect();
        let line_number = u32::try_from(index + 1).unwrap_or(u32::MAX);
        let container = containers
            .last()
            .filter(|(_, body)| *body == depth)
            .map(|(name, _)| name.clone());
        let mut header_start = 0;

        let matched = match language {
            "rust" => match rust_line(&code) {
                Some(RustLine::Impl(header)) => {
                    header_start = offset_in(&code, header);
                    pending = Some(PendingContainer {
                        name: None,
                        header: String::new(),
                    });
                    None
                }
                Some(RustLine::Declaration(kind, rest)) => Some((kind, rest)),
                None => None,
            },
            "javascript" | "typescript" => match_prefixes(&code, JS_PREFIXES)
                .filter(|(kind, _)| depth == 0 || !matches!(kind, Constant | Variable)),
            "python" => match_prefixes(&code, PYTHON_PREFIXES),
            "go" => go_line(&code),
            "java" | "kotlin" | "php" => match_prefixes(&code, CLASS_LANGUAGE_PREFIXES),
            _ => None,
        };

        if let Some((kind, rest)) = matched {
            let name = take_identifier(rest);
            if !name.is_empty() && name != "_" {
                let offset = offset_in(&code, rest);
                let kind = if kind == Function && container.is_some() {
                    Method
                } else {
                    kind
                };
                if language == "rust" && kind == Trait {
                    header_start = offset + name.len();
                    pending = Some(PendingContainer {
                        name: Some(name.clone()),
                        header: String::new(),
                    });
                }
                declarations.push(Declaration {
                    column: u32::try_from(code[..offset].chars().count() + 1).ok(),
                    name,
                    kind,
                    line: line_number,
                    container,
                });
            }
        }

        if !syntax.braces {
            continue;
        }
        for (position, character) in code.char_indices() {
            match character {
                '{' => {
                    depth += 1;
                    if position >= header_start
                        && let Some(open) = pending.take()
                        && let Some(name) = open.name.or_else(|| impl_target(&open.header))
                    {
                        containers.push((name, depth));
                    }
                }
                '}' => {
                    depth = depth.saturating_sub(1);
                    while containers.last().is_some_and(|(_, body)| *body > depth) {
                        containers.pop();
                    }
                }
                ';' if position >= header_start => pending = None,
                _ if position >= header_start => {
                    if let Some(open) = pending.as_mut() {
                        open.header.push(character);
                    }
                }
                _ => {}
            }
        }
        if let Some(open) = pending.as_mut() {
            open.header.push(' ');
        }
    }
    declarations
}

/// The prefixes `extract_symbols` has always matched, each with the kind it
/// now records. JavaScript `const` and `let` count only at brace depth 0.
const JS_PREFIXES: &[(&str, SymbolKind)] = &[
    ("export function ", Function),
    ("function ", Function),
    ("export class ", Class),
    ("class ", Class),
    ("export interface ", Interface),
    ("interface ", Interface),
    ("export const ", Constant),
    ("const ", Constant),
    ("let ", Variable),
];
const PYTHON_PREFIXES: &[(&str, SymbolKind)] = &[("def ", Function), ("class ", Class)];
const CLASS_LANGUAGE_PREFIXES: &[(&str, SymbolKind)] = &[
    ("class ", Class),
    ("function ", Function),
    ("fun ", Function),
];

fn match_prefixes<'a>(
    code: &'a str,
    prefixes: &[(&str, SymbolKind)],
) -> Option<(SymbolKind, &'a str)> {
    let trimmed = code.trim_start();
    prefixes
        .iter()
        .find_map(|(prefix, kind)| trimmed.strip_prefix(prefix).map(|rest| (*kind, rest)))
}

fn go_line(code: &str) -> Option<(SymbolKind, &str)> {
    let trimmed = code.trim_start();
    if let Some(rest) = trimmed.strip_prefix("func ") {
        // A method's receiver comes first, `func (u *T) Name`, so
        // `take_identifier` finds no name and the method is not recorded.
        return Some((Function, rest));
    }
    let rest = trimmed.strip_prefix("type ")?;
    let after_name = rest[take_identifier(rest).len()..].trim_start();
    let after_parameters = match after_name.strip_prefix('[') {
        Some(inner) => inner.split_once(']').map_or("", |(_, after)| after),
        None => after_name,
    }
    .trim_start();
    let kind = if after_parameters.starts_with("struct") {
        Struct
    } else if after_parameters.starts_with("interface") {
        Interface
    } else {
        Type
    };
    Some((kind, rest))
}

enum RustLine<'a> {
    Declaration(SymbolKind, &'a str),
    /// The text after the `impl` keyword.
    Impl(&'a str),
}

const RUST_KEYWORDS: &[(&str, SymbolKind)] = &[
    ("fn", Function),
    ("struct", Struct),
    ("enum", Enum),
    ("trait", Trait),
    ("type", Type),
    ("const", Constant),
    ("static", Variable),
    ("mod", Module),
];

fn rust_line(code: &str) -> Option<RustLine<'_>> {
    let mut rest = code.trim_start();
    if let Some(after) = strip_keyword(rest, "pub") {
        rest = after;
    } else if let Some(after) = rest.strip_prefix("pub(") {
        rest = after.split_once(')')?.1.trim_start();
    }
    // Qualifiers. An `extern` ABI string has already been masked to spaces.
    // `const` is a qualifier only before another function qualifier or `fn`.
    loop {
        if let Some(after) = ["async", "unsafe", "default", "extern"]
            .iter()
            .find_map(|qualifier| strip_keyword(rest, qualifier))
        {
            rest = after;
            continue;
        }
        if let Some(after) = strip_keyword(rest, "const")
            && ["fn", "async", "unsafe", "extern"]
                .iter()
                .any(|keyword| strip_keyword(after, keyword).is_some())
        {
            rest = after;
            continue;
        }
        break;
    }
    if let Some(after) = rest.strip_prefix("impl")
        && (after.starts_with('<') || after.starts_with(char::is_whitespace))
    {
        return Some(RustLine::Impl(after));
    }
    let (kind, after) = RUST_KEYWORDS
        .iter()
        .find_map(|(keyword, kind)| strip_keyword(rest, keyword).map(|after| (*kind, after)))?;
    let after = if kind == Variable {
        strip_keyword(after, "mut").unwrap_or(after)
    } else {
        after
    };
    Some(RustLine::Declaration(kind, after))
}

/// `value` with `keyword` and the whitespace after it removed, when `value`
/// starts with `keyword` as a whole word followed by whitespace.
fn strip_keyword<'a>(value: &'a str, keyword: &str) -> Option<&'a str> {
    let after = value.strip_prefix(keyword)?;
    after
        .starts_with(char::is_whitespace)
        .then(|| after.trim_start())
}

/// The implementing type in the text between `impl` and `{`: `Foo` in
/// `impl<T> fmt::Display for Foo<T> where T: Clone`.
fn impl_target(header: &str) -> Option<String> {
    let mut outer = String::new();
    let mut angle = 0usize;
    let mut previous = ' ';
    for character in header.chars() {
        match character {
            '<' => angle += 1,
            '>' if previous != '-' && angle > 0 => angle -= 1,
            _ if angle == 0 => outer.push(character),
            _ => {}
        }
        previous = character;
    }
    let tokens: Vec<&str> = outer
        .split_whitespace()
        .take_while(|token| *token != "where")
        .collect();
    let start = tokens
        .iter()
        .rposition(|token| *token == "for")
        .map_or(0, |position| position + 1);
    tokens[start..].iter().find_map(|token| {
        let token = token.trim_start_matches(['&', '!', '(', '*']);
        if token.starts_with('\'') || matches!(token, "" | "mut" | "dyn" | "const") {
            return None;
        }
        let segment = token.rsplit("::").next()?;
        let name: String = segment
            .chars()
            .take_while(|character| character.is_alphanumeric() || *character == '_')
            .collect();
        (!name.is_empty()).then_some(name)
    })
}

/// An `impl` or `trait` whose `{` has not been seen yet. A trait knows its
/// name; an `impl` collects its header until the brace.
struct PendingContainer {
    name: Option<String>,
    header: String,
}

/// The byte offset of `part` inside `whole`, which it must be a slice of.
fn offset_in(whole: &str, part: &str) -> usize {
    part.as_ptr() as usize - whole.as_ptr() as usize
}

#[derive(Clone, Copy)]
struct StringSyntax {
    delimiter: &'static str,
    multiline: bool,
    escapes: bool,
}

const fn string(delimiter: &'static str, multiline: bool, escapes: bool) -> StringSyntax {
    StringSyntax {
        delimiter,
        multiline,
        escapes,
    }
}

/// The comment and string delimiters of one language.
struct Syntax {
    line_comments: &'static [&'static str],
    block_comment: Option<(&'static str, &'static str)>,
    nested_block_comments: bool,
    /// Tried in order, so a triple quote comes before a single one.
    strings: &'static [StringSyntax],
    /// Rust raw strings (`r#"…"#`) and character literals, which have to be
    /// told apart from lifetimes.
    rust_literals: bool,
    /// Whether `{` and `}` delimit blocks, so the brace depth means something.
    braces: bool,
}

const C_COMMENTS: &[&str] = &["//"];
const C_BLOCK: Option<(&str, &str)> = Some(("/*", "*/"));
const RUST_STRINGS: &[StringSyntax] = &[string("\"", true, true)];
const JS_STRINGS: &[StringSyntax] = &[
    string("`", true, true),
    string("\"", false, true),
    string("'", false, true),
];
const PYTHON_STRINGS: &[StringSyntax] = &[
    string("\"\"\"", true, true),
    string("'''", true, true),
    string("\"", false, true),
    string("'", false, true),
];
const GO_STRINGS: &[StringSyntax] = &[
    string("`", true, false),
    string("\"", false, true),
    string("'", false, true),
];
/// Java text blocks and Kotlin raw strings. Kotlin's take no escapes. Java's
/// do, but an escaped quote run inside a text block is rare enough to ignore.
const JVM_STRINGS: &[StringSyntax] = &[
    string("\"\"\"", true, false),
    string("\"", false, true),
    string("'", false, true),
];
/// PHP strings of either quote may span lines.
const PHP_STRINGS: &[StringSyntax] = &[string("\"", true, true), string("'", true, true)];

fn syntax_for(language: &str) -> Option<Syntax> {
    let syntax = match language {
        "rust" => Syntax {
            line_comments: C_COMMENTS,
            block_comment: C_BLOCK,
            nested_block_comments: true,
            strings: RUST_STRINGS,
            rust_literals: true,
            braces: true,
        },
        "javascript" | "typescript" => Syntax {
            line_comments: C_COMMENTS,
            block_comment: C_BLOCK,
            nested_block_comments: false,
            strings: JS_STRINGS,
            rust_literals: false,
            braces: true,
        },
        "python" => Syntax {
            line_comments: &["#"],
            block_comment: None,
            nested_block_comments: false,
            strings: PYTHON_STRINGS,
            rust_literals: false,
            braces: false,
        },
        "go" => Syntax {
            line_comments: C_COMMENTS,
            block_comment: C_BLOCK,
            nested_block_comments: false,
            strings: GO_STRINGS,
            rust_literals: false,
            braces: true,
        },
        "java" | "kotlin" => Syntax {
            line_comments: C_COMMENTS,
            block_comment: C_BLOCK,
            nested_block_comments: language == "kotlin",
            strings: JVM_STRINGS,
            rust_literals: false,
            braces: true,
        },
        "php" => Syntax {
            line_comments: &["//", "#"],
            block_comment: C_BLOCK,
            nested_block_comments: false,
            strings: PHP_STRINGS,
            rust_literals: false,
            braces: true,
        },
        _ => return None,
    };
    Some(syntax)
}

/// Where a line starts: in code, or still inside a comment or string that an
/// earlier line opened.
enum Lexical {
    Code,
    BlockComment { depth: usize },
    String(StringSyntax),
    RawString { hashes: usize },
}

/// The line with every character inside a comment or string literal,
/// delimiters included, replaced by a space. Columns are preserved, so a
/// match in the result is at the same character position in the original.
fn mask_line(chars: &[char], lexical: &mut Lexical, syntax: &Syntax) -> Vec<char> {
    let mut code = Vec::with_capacity(chars.len());
    let mut index = 0;
    let blank = |code: &mut Vec<char>, count: usize| {
        code.extend(std::iter::repeat_n(' ', count));
    };
    'line: while index < chars.len() {
        match lexical {
            Lexical::Code => {
                if syntax
                    .line_comments
                    .iter()
                    .any(|marker| starts_with_at(chars, index, marker))
                {
                    blank(&mut code, chars.len() - index);
                    break;
                }
                if let Some((open, _)) = syntax.block_comment
                    && starts_with_at(chars, index, open)
                {
                    *lexical = Lexical::BlockComment { depth: 1 };
                    blank(&mut code, open.len());
                    index += open.len();
                    continue;
                }
                if syntax.rust_literals {
                    if let Some((hashes, length)) = rust_raw_string_open(chars, index) {
                        *lexical = Lexical::RawString { hashes };
                        blank(&mut code, length);
                        index += length;
                        continue;
                    }
                    if let Some(end) = rust_char_literal_end(chars, index) {
                        blank(&mut code, end + 1 - index);
                        index = end + 1;
                        continue;
                    }
                }
                for string in syntax.strings {
                    if starts_with_at(chars, index, string.delimiter) {
                        *lexical = Lexical::String(*string);
                        let length = string.delimiter.chars().count();
                        blank(&mut code, length);
                        index += length;
                        continue 'line;
                    }
                }
                code.push(chars[index]);
                index += 1;
            }
            Lexical::BlockComment { depth } => {
                let (open, close) = syntax.block_comment.unwrap_or(("/*", "*/"));
                if syntax.nested_block_comments && starts_with_at(chars, index, open) {
                    *depth += 1;
                    blank(&mut code, open.len());
                    index += open.len();
                } else if starts_with_at(chars, index, close) {
                    *depth -= 1;
                    if *depth == 0 {
                        *lexical = Lexical::Code;
                    }
                    blank(&mut code, close.len());
                    index += close.len();
                } else {
                    blank(&mut code, 1);
                    index += 1;
                }
            }
            Lexical::String(string) => {
                let length = string.delimiter.chars().count();
                if string.escapes && chars[index] == '\\' {
                    let skipped = 2.min(chars.len() - index);
                    blank(&mut code, skipped);
                    index += skipped;
                } else if starts_with_at(chars, index, string.delimiter) {
                    *lexical = Lexical::Code;
                    blank(&mut code, length);
                    index += length;
                } else {
                    blank(&mut code, 1);
                    index += 1;
                }
            }
            Lexical::RawString { hashes } => {
                let hashes = *hashes;
                if chars[index] == '"'
                    && chars.len() > index + hashes
                    && chars[index + 1..=index + hashes].iter().all(|c| *c == '#')
                {
                    *lexical = Lexical::Code;
                    blank(&mut code, hashes + 1);
                    index += hashes + 1;
                } else {
                    blank(&mut code, 1);
                    index += 1;
                }
            }
        }
    }
    if let Lexical::String(string) = lexical
        && !string.multiline
    {
        *lexical = Lexical::Code;
    }
    code
}

fn starts_with_at(chars: &[char], index: usize, marker: &str) -> bool {
    marker
        .chars()
        .enumerate()
        .all(|(offset, expected)| chars.get(index + offset) == Some(&expected))
}

/// A Rust raw string opening at `index` (`r"`, `r#"`, `br##"`, …): the number
/// of hashes and the opening's length in characters.
fn rust_raw_string_open(chars: &[char], index: usize) -> Option<(usize, usize)> {
    if index > 0 && (chars[index - 1].is_alphanumeric() || chars[index - 1] == '_') {
        return None;
    }
    let mut position = index;
    if chars.get(position) == Some(&'b') {
        position += 1;
    }
    if chars.get(position) != Some(&'r') {
        return None;
    }
    position += 1;
    let hashes = chars[position..].iter().take_while(|c| **c == '#').count();
    position += hashes;
    (chars.get(position) == Some(&'"')).then_some((hashes, position + 1 - index))
}

/// The index of the closing quote when a Rust character literal starts at
/// `index`. `'a` with no closing quote two characters on is a lifetime.
fn rust_char_literal_end(chars: &[char], index: usize) -> Option<usize> {
    if chars[index] != '\'' {
        return None;
    }
    if chars.get(index + 1) == Some(&'\\') {
        return (index + 3..chars.len()).find(|position| chars[*position] == '\'');
    }
    (chars.get(index + 2) == Some(&'\'')).then_some(index + 2)
}

pub fn detect_language(path: &str) -> &'static str {
    match Path::new(path)
        .extension()
        .and_then(|extension| extension.to_str())
        .unwrap_or_default()
        .to_ascii_lowercase()
        .as_str()
    {
        "js" | "jsx" | "mjs" | "cjs" => "javascript",
        "ts" | "tsx" => "typescript",
        "py" => "python",
        "go" => "go",
        "rs" => "rust",
        "java" => "java",
        "kt" => "kotlin",
        "php" => "php",
        "md" => "markdown",
        "json" => "json",
        "yml" | "yaml" => "yaml",
        "toml" => "toml",
        "html" => "html",
        "css" => "css",
        _ => "text",
    }
}

/// The names of `extract_declarations`, each once, in source order. Keyword
/// and semantic scoring read only names.
pub fn extract_symbols(content: &str, language: &str) -> Vec<String> {
    dedupe(
        extract_declarations(content, language)
            .into_iter()
            .map(|declaration| declaration.name)
            .collect(),
    )
}

pub fn extract_imports(content: &str, language: &str) -> Vec<String> {
    let mut imports = Vec::new();
    for line in content.lines() {
        let trimmed = line.trim();
        match language {
            "javascript" | "typescript" => {
                if let Some(value) = quoted_after(trimmed, "from ") {
                    imports.push(value);
                } else if trimmed.starts_with("import ") {
                    if let Some(value) = first_quoted(trimmed) {
                        imports.push(value);
                    }
                } else if let Some(start) = trimmed.find("require(")
                    && let Some(value) = first_quoted(&trimmed[start..])
                {
                    imports.push(value);
                }
            }
            "python" => {
                if let Some(rest) = trimmed.strip_prefix("from ") {
                    imports.push(take_identifier(rest));
                } else if let Some(rest) = trimmed.strip_prefix("import ") {
                    imports.push(take_identifier(rest));
                }
            }
            "go" => {
                if trimmed.starts_with('"')
                    && let Some(value) = first_quoted(trimmed)
                {
                    imports.push(value);
                }
            }
            "rust" => {
                if let Some(rest) = trimmed.strip_prefix("use ") {
                    imports.push(rest.trim_end_matches(';').to_string());
                }
            }
            "java" | "kotlin" => {
                if let Some(rest) = trimmed.strip_prefix("import ") {
                    imports.push(rest.trim_end_matches(';').to_string());
                }
            }
            _ => {}
        }
    }
    dedupe(imports)
}

fn take_identifier(value: &str) -> String {
    value
        .chars()
        .take_while(|character| {
            character.is_ascii_alphanumeric() || matches!(character, '_' | '$' | '.')
        })
        .collect()
}

fn first_quoted(value: &str) -> Option<String> {
    // Extract the first single- or double-quoted string. Find whichever quote
    // character appears first, then its matching close — the previous version
    // short-circuited on `"` and so missed single-quoted imports entirely.
    let (start, quote) = value
        .char_indices()
        .find(|(_, character)| matches!(character, '"' | '\''))?;
    let rest = &value[start + quote.len_utf8()..];
    let end = rest.find(quote)?;
    Some(rest[..end].to_string())
}

fn quoted_after(value: &str, marker: &str) -> Option<String> {
    let start = value.find(marker)?;
    first_quoted(&value[start + marker.len()..])
}

fn dedupe(values: Vec<String>) -> Vec<String> {
    let mut unique = Vec::new();
    for value in values {
        if !unique.contains(&value) {
            unique.push(value);
        }
    }
    unique
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn first_quoted_handles_both_quote_styles() {
        assert_eq!(first_quoted("import \"foo\"").as_deref(), Some("foo"));
        // Regression: single-quoted literals were previously dropped because
        // the double-quote lookup short-circuited the whole function.
        assert_eq!(first_quoted("import 'bar'").as_deref(), Some("bar"));
        assert_eq!(first_quoted("no quotes here"), None);
    }

    #[test]
    fn impl_target_is_the_implementing_type() {
        for (header, expected) in [
            (" Upload ", Some("Upload")),
            ("<T> Wrapper<T> ", Some("Wrapper")),
            (" fmt::Display for Upload ", Some("Upload")),
            (
                "<T> From<T> for UploadState where T: Into<String>, ",
                Some("UploadState"),
            ),
            ("<'a> Trait for &'a mut Inner<'a> ", Some("Inner")),
            (
                "<F> Run for Task<F> where F: for<'a> Fn(&'a str) -> u8 ",
                Some("Task"),
            ),
            (" !Send for Handle ", Some("Handle")),
            (" ", None),
        ] {
            assert_eq!(impl_target(header).as_deref(), expected, "{header:?}");
        }
    }

    #[test]
    fn a_lifetime_is_not_a_character_literal() {
        let mut lexical = Lexical::Code;
        let syntax = syntax_for("rust").unwrap();
        let line: Vec<char> = "fn f<'a>(x: &'a str) -> char { '{' }".chars().collect();
        let code: String = mask_line(&line, &mut lexical, &syntax)
            .into_iter()
            .collect();
        assert_eq!(code, "fn f<'a>(x: &'a str) -> char {     }");
        assert!(matches!(lexical, Lexical::Code));
    }

    #[test]
    fn extract_imports_reads_single_and_double_quoted_js_imports() {
        let source =
            "import a from \"./double\";\nimport b from './single';\nconst c = require('./req');\n";
        assert_eq!(
            extract_imports(source, "javascript"),
            vec![
                "./double".to_string(),
                "./single".to_string(),
                "./req".to_string()
            ]
        );
    }
}

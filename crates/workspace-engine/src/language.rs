use std::path::Path;

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

pub fn extract_symbols(content: &str, language: &str) -> Vec<String> {
    let mut symbols = Vec::new();
    for line in content.lines() {
        let trimmed = line.trim_start();
        match language {
            "javascript" | "typescript" => {
                collect_after_prefix(trimmed, "export function ", &mut symbols);
                collect_after_prefix(trimmed, "function ", &mut symbols);
                collect_after_prefix(trimmed, "export class ", &mut symbols);
                collect_after_prefix(trimmed, "class ", &mut symbols);
                collect_after_prefix(trimmed, "export interface ", &mut symbols);
                collect_after_prefix(trimmed, "interface ", &mut symbols);
                collect_variable(trimmed, "export const ", &mut symbols);
                collect_variable(trimmed, "const ", &mut symbols);
                collect_variable(trimmed, "let ", &mut symbols);
            }
            "python" => {
                collect_after_prefix(trimmed, "def ", &mut symbols);
                collect_after_prefix(trimmed, "class ", &mut symbols);
            }
            "go" => {
                collect_after_prefix(trimmed, "func ", &mut symbols);
                collect_after_prefix(trimmed, "type ", &mut symbols);
            }
            "rust" => {
                collect_after_prefix(trimmed.trim_start_matches("pub "), "fn ", &mut symbols);
                collect_after_prefix(trimmed.trim_start_matches("pub "), "struct ", &mut symbols);
                collect_after_prefix(trimmed.trim_start_matches("pub "), "enum ", &mut symbols);
            }
            "java" | "kotlin" | "php" => {
                collect_after_prefix(trimmed, "class ", &mut symbols);
                collect_after_prefix(trimmed, "function ", &mut symbols);
                collect_after_prefix(trimmed, "fun ", &mut symbols);
            }
            _ => {}
        }
    }
    dedupe(symbols)
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
                } else if let Some(start) = trimmed.find("require(") {
                    if let Some(value) = first_quoted(&trimmed[start..]) {
                        imports.push(value);
                    }
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
                if trimmed.starts_with('"') {
                    if let Some(value) = first_quoted(trimmed) {
                        imports.push(value);
                    }
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

fn collect_after_prefix(line: &str, prefix: &str, symbols: &mut Vec<String>) {
    if let Some(rest) = line.strip_prefix(prefix) {
        let symbol = take_identifier(rest);
        if !symbol.is_empty() {
            symbols.push(symbol);
        }
    }
}

fn collect_variable(line: &str, prefix: &str, symbols: &mut Vec<String>) {
    if let Some(rest) = line.strip_prefix(prefix) {
        let symbol = take_identifier(rest);
        if !symbol.is_empty() {
            symbols.push(symbol);
        }
    }
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
    for quote in ['"', '\''] {
        let start = value.find(quote)?;
        let end = value[start + 1..].find(quote)?;
        return Some(value[start + 1..start + 1 + end].to_string());
    }
    None
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

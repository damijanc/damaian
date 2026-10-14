//! Spec 25: the symbol and relationship index.
//!
//! Task 1's accuracy fixtures live in `tests/fixtures/symbols/`, one file per
//! language. Each has a table here that records the truth about every
//! declaration in it and how the heuristic extractor does on it, so the
//! remaining misses are pinned rather than hidden: a `Missed` row is a known
//! gap, and a row that moves from `Missed` to `Found` is an improvement the
//! table must be updated to claim.

use std::collections::BTreeSet;

use workspace_engine::language::extract_symbols;
use workspace_engine::{Declaration, SymbolKind, extract_declarations};

use SymbolKind::{
    Class, Constant, Enum, Function, Interface, Method, Module, Struct, Trait, Type, Variable,
};

/// One declaration as the tables describe it: name, kind, line, container.
/// The column is left out of the tables and tested on its own.
type Expected = (&'static str, SymbolKind, u32, Option<&'static str>);

/// How the heuristic extractor does on one declaration.
enum Row {
    /// In the source, and extracted exactly.
    Found(Expected),
    /// In the source, and not extracted at all.
    Missed(Expected),
    /// In the source and extracted by name, but with the wrong kind, line or
    /// container. `truth` is what the source says, `got` what is extracted.
    Inexact { truth: Expected, got: Expected },
    /// Not a declaration, but extracted as one.
    FalsePositive(Expected),
}

use Row::{FalsePositive, Found, Inexact, Missed};

struct Fixture {
    language: &'static str,
    file: &'static str,
    rows: Vec<Row>,
}

impl Fixture {
    fn content(&self) -> String {
        let path = format!(
            "{}/tests/fixtures/symbols/{}",
            env!("CARGO_MANIFEST_DIR"),
            self.file
        );
        std::fs::read_to_string(&path).unwrap_or_else(|error| panic!("{path}: {error}"))
    }

    /// What `extract_declarations` must return, in source order.
    fn extracted(&self) -> Vec<Expected> {
        let mut rows: Vec<Expected> = self
            .rows
            .iter()
            .filter_map(|row| match row {
                Found(expected) | FalsePositive(expected) => Some(*expected),
                Inexact { got, .. } => Some(*got),
                Missed(_) => None,
            })
            .collect();
        rows.sort_by_key(|(name, _, line, _)| (*line, *name));
        rows
    }

    /// The names of every real declaration in the fixture.
    fn true_names(&self) -> BTreeSet<&'static str> {
        self.rows
            .iter()
            .filter_map(|row| match row {
                Found((name, ..))
                | Missed((name, ..))
                | Inexact {
                    truth: (name, ..), ..
                } => Some(*name),
                FalsePositive(_) => None,
            })
            .collect()
    }
}

fn as_expected(declaration: &Declaration) -> (String, SymbolKind, u32, Option<String>) {
    (
        declaration.name.clone(),
        declaration.kind,
        declaration.line,
        declaration.container.clone(),
    )
}

fn assert_fixture(fixture: &Fixture) {
    let actual: Vec<_> = extract_declarations(&fixture.content(), fixture.language)
        .iter()
        .map(as_expected)
        .collect();
    let expected: Vec<_> = fixture
        .extracted()
        .into_iter()
        .map(|(name, kind, line, container)| {
            (name.to_string(), kind, line, container.map(str::to_string))
        })
        .collect();
    assert_eq!(actual, expected, "{} fixture", fixture.language);
}

fn rust() -> Fixture {
    Fixture {
        language: "rust",
        file: "rust.rs",
        rows: vec![
            Found(("Upload", Struct, 5, None)),
            Found(("UploadState", Enum, 9, None)),
            Found(("upload_retry", Function, 14, None)),
            Found(("new", Method, 19, Some("Upload"))),
            Found(("validate", Method, 23, Some("Upload"))),
            Found(("fmt", Method, 29, Some("Upload"))),
            Found(("from", Method, 38, Some("UploadState"))),
            Found(("crate_visible", Function, 43, None)),
            Found(("fetch_remote", Function, 45, None)),
            Found(("limit", Function, 47, None)),
            Found(("Store", Trait, 51, None)),
            Found(("put", Method, 52, Some("Store"))),
            Found(("put", Method, 56, Some("Upload"))),
            Found(("UploadMap", Type, 59, None)),
            Found(("MAX_RETRIES", Constant, 61, None)),
            Found(("GREETING", Variable, 63, None)),
            Found(("exported_callback", Function, 65, None)),
            Found(("helpers", Module, 67, None)),
            Found(("helper", Function, 68, None)),
            Found(("TEMPLATE", Constant, 78, None)),
            Found(("PLAIN", Constant, 82, None)),
            Found(("after_strings", Function, 86, None)),
            Found(("last_one", Function, 90, None)),
            // A macro body is matched like any other code.
            FalsePositive(("$name", Function, 94, None)),
        ],
    }
}

fn typescript() -> Fixture {
    Fixture {
        language: "typescript",
        file: "typescript.ts",
        rows: vec![
            Found(("UploadOptions", Interface, 4, None)),
            Found(("uploadRetry", Function, 8, None)),
            Found(("Uploader", Class, 14, None)),
            Missed(("upload", Method, 15, Some("Uploader"))),
            Found(("MAX_RETRIES", Constant, 21, None)),
            Found(("counter", Variable, 23, None)),
            Missed(("fetchRemote", Function, 25, None)),
            Missed(("UploadMap", Type, 27, None)),
            Missed(("UploadState", Enum, 29, None)),
            Found(("helper", Function, 34, None)),
            Found(("template", Constant, 44, None)),
            Found(("InternalShape", Interface, 48, None)),
        ],
    }
}

fn javascript() -> Fixture {
    Fixture {
        language: "javascript",
        file: "javascript.js",
        rows: vec![
            Found(("path", Constant, 2, None)),
            Found(("uploadRetry", Function, 4, None)),
            Found(("Uploader", Class, 9, None)),
            Missed(("constructor", Method, 10, Some("Uploader"))),
            Found(("createUploader", Function, 15, None)),
            Found(("DEFAULT_TARGET", Constant, 20, None)),
            Found(("pending", Variable, 22, None)),
            Found(("handler", Constant, 24, None)),
            Missed(("fetchRemote", Function, 29, None)),
            Found(("message", Constant, 35, None)),
        ],
    }
}

fn python() -> Fixture {
    Fixture {
        language: "python",
        file: "python.py",
        rows: vec![
            Found(("Uploader", Class, 11, None)),
            Inexact {
                truth: ("upload", Method, 17, Some("Uploader")),
                got: ("upload", Function, 17, None),
            },
            Found(("upload_retry", Function, 21, None)),
            Found(("nested_backoff", Function, 22, None)),
            Missed(("fetch_remote", Function, 28, None)),
            Missed(("TEMPLATE", Constant, 34, None)),
        ],
    }
}

fn go() -> Fixture {
    Fixture {
        language: "go",
        file: "go.go",
        rows: vec![
            Found(("Uploader", Struct, 8, None)),
            Found(("Store", Interface, 12, None)),
            Missed(("Put", Method, 13, Some("Store"))),
            Found(("UploadID", Type, 16, None)),
            Found(("UploadRetry", Function, 18, None)),
            Missed(("Upload", Method, 22, Some("Uploader"))),
            Missed(("template", Constant, 30, None)),
        ],
    }
}

fn java() -> Fixture {
    Fixture {
        language: "java",
        file: "java.java",
        rows: vec![
            Found(("Uploader", Class, 4, None)),
            Missed(("uploadRetry", Method, 7, Some("Uploader"))),
            Missed(("UploadService", Class, 12, None)),
            Missed(("describe", Method, 13, Some("UploadService"))),
        ],
    }
}

fn kotlin() -> Fixture {
    Fixture {
        language: "kotlin",
        file: "kotlin.kt",
        rows: vec![
            Found(("Uploader", Class, 4, None)),
            Inexact {
                truth: ("upload", Method, 5, Some("Uploader")),
                got: ("upload", Function, 5, None),
            },
            Missed(("UploadResult", Class, 10, None)),
            Found(("uploadRetry", Function, 12, None)),
            Missed(("helper", Function, 14, None)),
        ],
    }
}

fn php() -> Fixture {
    Fixture {
        language: "php",
        file: "php.php",
        rows: vec![
            Found(("Uploader", Class, 4, None)),
            Inexact {
                truth: ("upload", Method, 6, Some("Uploader")),
                got: ("upload", Function, 6, None),
            },
            Missed(("retry", Method, 11, Some("Uploader"))),
            Found(("upload_retry", Function, 17, None)),
        ],
    }
}

fn fixtures() -> Vec<Fixture> {
    vec![
        rust(),
        typescript(),
        javascript(),
        python(),
        go(),
        java(),
        kotlin(),
        php(),
    ]
}

#[test]
fn rust_declarations_match_the_fixture() {
    assert_fixture(&rust());
}

#[test]
fn typescript_declarations_match_the_fixture() {
    assert_fixture(&typescript());
}

#[test]
fn javascript_declarations_match_the_fixture() {
    assert_fixture(&javascript());
}

#[test]
fn python_declarations_match_the_fixture() {
    assert_fixture(&python());
}

#[test]
fn go_declarations_match_the_fixture() {
    assert_fixture(&go());
}

#[test]
fn java_declarations_match_the_fixture() {
    assert_fixture(&java());
}

#[test]
fn kotlin_declarations_match_the_fixture() {
    assert_fixture(&kotlin());
}

#[test]
fn php_declarations_match_the_fixture() {
    assert_fixture(&php());
}

/// Task 1 changes no caller: `extract_symbols` still returns names only, and
/// they are exactly the declarations' names, each once, in source order.
#[test]
fn extract_symbols_is_the_deduplicated_names_of_extract_declarations() {
    for fixture in fixtures() {
        let content = fixture.content();
        let mut names: Vec<String> = Vec::new();
        for declaration in extract_declarations(&content, fixture.language) {
            if !names.contains(&declaration.name) {
                names.push(declaration.name);
            }
        }
        assert!(!names.is_empty(), "{} fixture", fixture.language);
        assert_eq!(
            extract_symbols(&content, fixture.language),
            names,
            "{} fixture",
            fixture.language
        );
    }
}

/// The column counts characters, not bytes, and survives a comment in front
/// of the declaration that holds a multi-byte character.
#[test]
fn a_declaration_records_the_column_of_its_name() {
    let source = "    pub(crate) fn indented() {}\n/* é */ fn after_comment() {}\n";
    let columns: Vec<_> = extract_declarations(source, "rust")
        .into_iter()
        .map(|declaration| (declaration.name, declaration.column))
        .collect();
    assert_eq!(
        columns,
        vec![
            ("indented".to_string(), Some(19)),
            ("after_comment".to_string(), Some(12)),
        ]
    );
}

#[test]
fn a_language_without_declaration_rules_has_none() {
    assert!(extract_declarations("fn looks_like_rust() {}\n", "markdown").is_empty());
    assert!(extract_symbols("fn looks_like_rust() {}\n", "text").is_empty());
}

/// Prints name-level accuracy per language: how many true declarations
/// `extract_symbols` names (recall), and how many of its names are true
/// declarations (precision). This is proposal §7's heuristic accuracy record.
/// Run with `--no-capture` to read it.
#[test]
fn heuristic_accuracy_per_language() {
    println!("language    true  found  extra  precision  recall");
    for fixture in fixtures() {
        let truth = fixture.true_names();
        let names = extract_symbols(&fixture.content(), fixture.language);
        let found = names
            .iter()
            .filter(|name| truth.contains(name.as_str()))
            .count();
        let extra = names.len() - found;
        let precision = if names.is_empty() {
            0.0
        } else {
            found as f64 / names.len() as f64
        };
        let recall = found as f64 / truth.len() as f64;
        println!(
            "{:<11} {:>4}  {:>5}  {:>5}  {:>9.2}  {:>6.2}",
            fixture.language,
            truth.len(),
            found,
            extra,
            precision,
            recall
        );
        assert!(found <= truth.len());
    }
}

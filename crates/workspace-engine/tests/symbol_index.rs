//! Spec 25: the symbol and relationship index.
//!
//! Task 1's accuracy fixtures live in `tests/fixtures/symbols/`, one file per
//! language. Each has a table here that records the truth about every
//! declaration in it and how the heuristic extractor does on it, so the
//! remaining misses are pinned rather than hidden: a `Missed` row is a known
//! gap, and a row that moves from `Missed` to `Found` is an improvement the
//! table must be updated to claim.
//!
//! Task 2 pins `SymbolIndex::build` over real indexes of temp repositories:
//! lookups at exact lines, lines that survive redaction, restricted files
//! left out, roots, import resolution, `TestOf`, and determinism.

use std::collections::BTreeSet;
use std::fs;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use workspace_engine::language::extract_symbols;
use workspace_engine::{
    AuditLog, CommandPolicy, Config, Declaration, MatchQuality, PathPolicy, ProjectIndexer,
    Provenance, RelationKind, RelationTarget, RepositoryIndex, RepositoryMap, SecretScanner,
    SymbolIndex, SymbolKind, extract_declarations,
};

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

// ---- Task 2: `SymbolIndex::build` ----

static COUNTER: AtomicU64 = AtomicU64::new(1);

/// A temp repository, indexed for real with the watcher off.
struct Repo {
    root: PathBuf,
    data_dir: PathBuf,
    secret_patterns: Vec<String>,
}

impl Repo {
    fn new(name: &str, files: &[(&str, &str)]) -> Self {
        let unique = format!(
            "{}-{}-{}",
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .expect("clock should work")
                .as_nanos(),
            std::process::id(),
            COUNTER.fetch_add(1, Ordering::Relaxed)
        );
        let base = std::env::temp_dir().join(format!("damaian-symbols-{name}-{unique}"));
        let repo = Self {
            root: base.join("repo"),
            data_dir: base.join("data"),
            secret_patterns: Vec::new(),
        };
        for (path, content) in files {
            repo.write(path, content);
        }
        fs::create_dir_all(&repo.data_dir).unwrap();
        repo
    }

    fn write(&self, path: &str, content: &str) {
        let file = self.root.join(path);
        fs::create_dir_all(file.parent().unwrap()).unwrap();
        fs::write(file, content).unwrap();
    }

    fn config(&self) -> Config {
        Config {
            data_dir: self.data_dir.clone(),
            enable_index_watcher: false,
            shell: "/usr/bin/true".to_string(),
            secret_patterns: self.secret_patterns.clone(),
            ..Config::default()
        }
    }

    fn index(&self) -> RepositoryIndex {
        let config = self.config();
        let scanner = SecretScanner::new(config.secret_patterns.clone());
        let audit_log = AuditLog::new(&self.data_dir, true, scanner.clone());
        ProjectIndexer::new(config, scanner, audit_log)
            .index_repository(&self.root)
            .expect("the fixture should index")
    }

    fn build_from(&self, index: &RepositoryIndex, policy: &PathPolicy) -> SymbolIndex {
        let map = RepositoryMap::build(index, &CommandPolicy::new(self.config()));
        SymbolIndex::build(index, &map, policy)
    }

    fn build(&self) -> SymbolIndex {
        self.build_from(&self.index(), &PathPolicy::new(&self.config()))
    }
}

/// `(path, line)` of every symbol named `name`.
fn locations(index: &SymbolIndex, name: &str) -> Vec<(String, u32)> {
    index
        .symbols
        .iter()
        .filter(|symbol| symbol.name == name)
        .map(|symbol| (symbol.path.clone(), symbol.line))
        .collect()
}

/// Every edge of `kind` from `path`, as its target.
fn targets(index: &SymbolIndex, kind: RelationKind, path: &str) -> Vec<RelationTarget> {
    index
        .relations
        .iter()
        .filter(|relation| relation.kind == kind && relation.from.path == path)
        .map(|relation| relation.to.clone())
        .collect()
}

fn file(path: &str) -> RelationTarget {
    RelationTarget::File {
        path: path.to_string(),
    }
}

fn external(specifier: &str) -> RelationTarget {
    RelationTarget::External {
        specifier: specifier.to_string(),
    }
}

/// Task 1's fixtures, copied under `src/` with their own names.
fn fixture_repo(name: &str) -> Repo {
    let repo = Repo::new(name, &[]);
    for fixture in fixtures() {
        repo.write(&format!("src/{}", fixture.file), &fixture.content());
    }
    repo
}

#[test]
fn a_lookup_returns_the_definition_at_its_exact_path_and_line() {
    let index = fixture_repo("lookup").build();

    let exact: Vec<(String, u32, Provenance)> = index
        .find("upload_retry", 10)
        .into_iter()
        .filter(|found| found.quality == MatchQuality::Exact)
        .map(|found| {
            (
                found.symbol.path,
                found.symbol.line,
                found.symbol.provenance,
            )
        })
        .collect();
    assert_eq!(
        exact,
        [
            ("src/php.php".to_string(), 17, Provenance::Heuristic),
            ("src/python.py".to_string(), 21, Provenance::Heuristic),
            ("src/rust.rs".to_string(), 14, Provenance::Heuristic),
        ]
    );
    let method = index
        .symbols
        .iter()
        .find(|symbol| symbol.path == "src/rust.rs" && symbol.kind == SymbolKind::Method)
        .expect("the Rust fixture has methods");
    assert!(method.container.is_some(), "{method:?}");
}

#[test]
fn with_no_language_server_every_symbol_and_relation_is_heuristic() {
    let index = fixture_repo("heuristic").build();
    assert!(index.symbols.len() > 50, "{}", index.symbols.len());
    assert!(!index.relations.is_empty());
    assert!(
        index
            .symbols
            .iter()
            .all(|symbol| symbol.provenance == Provenance::Heuristic)
    );
    assert!(
        index
            .relations
            .iter()
            .all(|relation| relation.provenance == Provenance::Heuristic)
    );
}

#[test]
fn a_lookup_ranks_exact_then_prefix_then_fuzzy() {
    let index = Repo::new(
        "ranking",
        &[(
            "src/lib.rs",
            "fn upload_part() {}\nfn upload() {}\nfn unpaid_load() {}\nfn other() {}\n",
        )],
    )
    .build();

    let found: Vec<(String, MatchQuality)> = index
        .find("upload", 10)
        .into_iter()
        .map(|found| (found.symbol.name, found.quality))
        .collect();
    assert_eq!(
        found,
        [
            ("upload".to_string(), MatchQuality::Exact),
            ("upload_part".to_string(), MatchQuality::Prefix),
            ("unpaid_load".to_string(), MatchQuality::Fuzzy),
        ]
    );
    assert_eq!(index.find("UPLOAD", 10)[0].quality, MatchQuality::Prefix);
    assert_eq!(index.find("upload", 1).len(), 1);
    assert!(index.find("", 10).is_empty());
}

#[test]
fn provenance_orders_the_language_server_first() {
    assert!(Provenance::LanguageServer < Provenance::Heuristic);
    let mut sorted = vec![Provenance::Heuristic, Provenance::LanguageServer];
    sorted.sort();
    assert_eq!(sorted, [Provenance::LanguageServer, Provenance::Heuristic]);
}

#[test]
fn a_redacted_multi_line_secret_does_not_shift_the_lines_after_it() {
    let source = "\
/*
-----BEGIN PRIVATE KEY-----
MIIBVQIBADANBgkqhkiG9w0BAQEFAASCAT8wggE7AgEAAkEAq7BFUpkGp3+LQmlQ
Yx2eqzDV+xeG8kx/sQFV18S5JhzGeIGNA72wSQIDAQABAkBGXhI8TWHHyCHQ39cA
-----END PRIVATE KEY-----
*/
fn after_the_key() {}
";
    let repo = Repo::new("pem", &[("src/keys.rs", source)]);
    let index = repo.index();
    let record = index
        .files
        .iter()
        .find(|r| r.path == "src/keys.rs")
        .unwrap();
    assert!(
        !record.chunks[0].text.contains("MIIBVQ"),
        "the PEM block should be redacted from the indexed text"
    );

    assert_eq!(
        locations(&repo.build(), "after_the_key"),
        [("src/keys.rs".to_string(), 7)]
    );
}

#[test]
fn a_declaration_whose_name_or_container_is_a_secret_is_left_out() {
    let mut repo = Repo::new(
        "secret-name",
        &[(
            "src/lib.rs",
            "fn hunter2_rotate() {}\nstruct Hunter2Vault;\nimpl Hunter2Vault {\n    fn open(&self) {}\n}\nfn plain() {}\n",
        )],
    );
    repo.secret_patterns = vec!["hunter2".to_string(), "Hunter2".to_string()];
    let index = repo.build();

    let names: Vec<&str> = index.symbols.iter().map(|s| s.name.as_str()).collect();
    assert_eq!(names, ["plain"]);
}

#[test]
fn a_restricted_file_contributes_nothing() {
    let repo = Repo::new(
        "restricted",
        &[
            (".env.local", "const API_TOKEN = 1\n"),
            ("secrets/config.ts", "export const leakedName = 1;\n"),
            (
                "src/app.ts",
                "import { leakedName } from \"../secrets/config\";\n",
            ),
            ("secrets/config.test.ts", "export const leakedTest = 1;\n"),
        ],
    );
    let index = repo.index();
    assert!(
        index.files.iter().any(|r| r.path == "secrets/config.ts"),
        "the index itself does not filter restricted files"
    );

    let symbols = repo.build_from(&index, &PathPolicy::new(&repo.config()));
    let paths: BTreeSet<&str> = symbols
        .symbols
        .iter()
        .map(|symbol| symbol.path.as_str())
        .chain(symbols.relations.iter().map(|r| r.from.path.as_str()))
        .collect();
    assert_eq!(paths, BTreeSet::from(["src/app.ts"]));
    assert_eq!(
        targets(&symbols, RelationKind::Imports, "src/app.ts"),
        [external("../secrets/config")]
    );

    // The same files under a policy that restricts nothing are indexed as
    // symbols, so it is the policy that removed them.
    let open = repo.build_from(
        &index,
        &PathPolicy::new(&Config {
            restricted_patterns: Vec::new(),
            ..repo.config()
        }),
    );
    assert_eq!(
        locations(&open, "leakedName"),
        [("secrets/config.ts".to_string(), 1)]
    );
}

#[test]
fn each_symbol_records_its_own_root() {
    let repo = Repo::new(
        "roots",
        &[
            ("packages/api/package.json", "{}"),
            ("packages/api/index.ts", "export function serveApi() {}\n"),
            ("packages/web/package.json", "{}"),
            ("packages/web/index.ts", "export function renderWeb() {}\n"),
            ("tools/build.ts", "export function buildAll() {}\n"),
        ],
    );
    let index = repo.build();

    let root_of = |name: &str| {
        index
            .symbols
            .iter()
            .find(|symbol| symbol.name == name)
            .unwrap_or_else(|| panic!("{name} should be indexed"))
            .root_path
            .clone()
    };
    assert_eq!(root_of("serveApi"), "packages/api");
    assert_eq!(root_of("renderWeb"), "packages/web");
    assert_eq!(root_of("buildAll"), "");
}

#[test]
fn relative_imports_resolve_to_indexed_files_and_the_rest_stay_external() {
    let repo = Repo::new(
        "imports",
        &[
            (
                "src/app.ts",
                "import { x } from \"./upload\";\n\
                 import { x2 } from \"./upload.ts\";\n\
                 import { y } from \"../lib\";\n\
                 import React from \"react\";\n\
                 import { z } from \"./missing\";\n\
                 import { w } from \"../../outside\";\n",
            ),
            ("src/upload.ts", "export const x = 1;\n"),
            ("lib/index.ts", "export const y = 1;\n"),
            (
                "pkg/main.py",
                "from .upload import send\nfrom ..shared.util import helper\nfrom . import sibling\nimport os\nfrom .absent import nothing\n",
            ),
            ("pkg/upload.py", "def send():\n    pass\n"),
            ("pkg/__init__.py", ""),
            ("shared/util/__init__.py", "def helper():\n    pass\n"),
            ("src/lib.rs", "use crate::upload;\nuse serde::Serialize;\n"),
            ("src/upload.rs", "pub fn send() {}\n"),
        ],
    );
    let index = repo.build();

    assert_eq!(
        targets(&index, RelationKind::Imports, "src/app.ts"),
        [
            file("lib/index.ts"),
            file("src/upload.ts"),
            external("../../outside"),
            external("./missing"),
            external("react"),
        ]
    );
    assert_eq!(
        targets(&index, RelationKind::Imports, "pkg/main.py"),
        [
            file("pkg/__init__.py"),
            file("pkg/upload.py"),
            file("shared/util/__init__.py"),
            external(".absent"),
            external("os"),
        ]
    );
    // Rust module paths are not modelled, so even `crate::` stays external.
    assert_eq!(
        targets(&index, RelationKind::Imports, "src/lib.rs"),
        [external("crate::upload"), external("serde::Serialize")]
    );

    let indexed: BTreeSet<String> = repo.index().files.into_iter().map(|r| r.path).collect();
    for relation in &index.relations {
        if let RelationTarget::File { path } = &relation.to {
            assert!(
                indexed.contains(path),
                "{relation:?} names a path not in the index"
            );
        }
    }
}

#[test]
fn naming_conventions_pair_tests_with_their_subjects() {
    let repo = Repo::new(
        "test-of",
        &[
            ("Cargo.toml", "[package]\nname = \"x\"\n"),
            ("src/upload.rs", "pub fn send() {}\n"),
            ("tests/upload.rs", "fn sends() {}\n"),
            ("tests/orphan.rs", "fn nothing() {}\n"),
            ("web/foo.ts", "export const foo = 1;\n"),
            ("web/foo.test.ts", "test(\"foo\", () => {});\n"),
            ("web/bar.ts", "export const bar = 1;\n"),
            ("web/bar.spec.ts", "test(\"bar\", () => {});\n"),
            ("py/upload.py", "def send():\n    pass\n"),
            ("py/test_upload.py", "def test_send():\n    pass\n"),
            ("go/upload.go", "package upload\n"),
            ("go/upload_test.go", "package upload\n"),
        ],
    );
    let index = repo.build();

    let pairs: Vec<(String, RelationTarget, Provenance)> = index
        .relations
        .iter()
        .filter(|relation| relation.kind == RelationKind::TestOf)
        .map(|relation| {
            (
                relation.from.path.clone(),
                relation.to.clone(),
                relation.provenance,
            )
        })
        .collect();
    let heuristic = |from: &str, to: &str| (from.to_string(), file(to), Provenance::Heuristic);
    assert_eq!(
        pairs,
        [
            heuristic("go/upload_test.go", "go/upload.go"),
            heuristic("py/test_upload.py", "py/upload.py"),
            heuristic("tests/upload.rs", "src/upload.rs"),
            heuristic("web/bar.spec.ts", "web/bar.ts"),
            heuristic("web/foo.test.ts", "web/foo.ts"),
        ]
    );
}

#[test]
fn a_test_is_not_paired_with_a_subject_in_another_root() {
    let repo = Repo::new(
        "test-of-roots",
        &[
            ("Cargo.toml", "[workspace]\n"),
            // `src/upload.py` is the conventional subject of this test, but
            // `src` is a root of its own, so the pair is not made.
            ("tests/test_upload.py", "def test_send():\n    pass\n"),
            ("src/pyproject.toml", "[project]\nname = \"s\"\n"),
            ("src/upload.py", "def send():\n    pass\n"),
            ("crates/a/Cargo.toml", "[package]\nname = \"a\"\n"),
            ("crates/a/src/upload.rs", "pub fn send() {}\n"),
            ("crates/a/tests/upload.rs", "fn sends() {}\n"),
        ],
    );
    let index = repo.build();

    let pairs: Vec<(String, RelationTarget)> = index
        .relations
        .iter()
        .filter(|relation| relation.kind == RelationKind::TestOf)
        .map(|relation| (relation.from.path.clone(), relation.to.clone()))
        .collect();
    assert_eq!(
        pairs,
        [(
            "crates/a/tests/upload.rs".to_string(),
            file("crates/a/src/upload.rs")
        )]
    );
}

#[test]
fn a_build_is_deterministic_whatever_the_file_order() {
    let repo = fixture_repo("deterministic");
    repo.write("src/app.ts", "import { x } from \"./typescript\";\n");
    repo.write("tests/rust.rs", "fn checks() {}\n");
    let policy = PathPolicy::new(&repo.config());
    let index = repo.index();
    let mut reversed = index.clone();
    reversed.files.reverse();

    let first = serde_json::to_string(&repo.build_from(&index, &policy)).unwrap();
    let second = serde_json::to_string(&repo.build_from(&index, &policy)).unwrap();
    let from_reversed = serde_json::to_string(&repo.build_from(&reversed, &policy)).unwrap();
    assert_eq!(first, second);
    assert_eq!(first, from_reversed);
}

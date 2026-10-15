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
//!
//! Task 3 pins `SymbolIndexStore`: rebuilds and their audit, reuse, per-file
//! reuse across both tiers, and the engine's `find_symbols`.
//!
//! Task 5 pins `language_server.<language>` at every scope and the hub's
//! process lifetime, against a fake server that is this test binary
//! re-executed (context.md §2): registration before `initialize`, shutdown,
//! idle expiry, an unanswered `initialize`, one restart and no more, and the
//! sweep's start-time check over a `language_server` entry.

use std::collections::BTreeSet;
use std::fs;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use workspace_engine::language::extract_symbols;
use workspace_engine::{
    AuditLog, CommandPolicy, Config, ConfigOverlay, Declaration, LANGUAGE_SERVER_LANGUAGES,
    LanguageServerError, LanguageServerHub, LanguageServerLaunch, LanguageServerLimits,
    LanguageServerState, MatchQuality, PathPolicy, ProcessIdentity, ProcessKind, ProcessRegistry,
    ProjectIndexer, Provenance, RebuildReason, RegisteredProcess, RelationKind, RelationTarget,
    RepositoryIndex, RepositoryKeyClass, RepositoryMap, RepositoryTrustStore,
    SYMBOL_INDEX_SCHEMA_VERSION, SecretScanner, SweepScope, SymbolIndex, SymbolIndexStore,
    SymbolKind, SymbolLoad, WorkspaceEngine, extract_declarations, read_message, write_message,
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

// ---- Task 3: persistence, rebuild reporting, engine API ----

impl Repo {
    fn store(&self) -> SymbolIndexStore {
        let scanner = SecretScanner::new(self.secret_patterns.clone());
        SymbolIndexStore::new(&self.data_dir, AuditLog::new(&self.data_dir, true, scanner))
    }

    /// Indexes the repository again and loads its symbol index through the
    /// store, as the engine does.
    fn load(&self) -> (SymbolIndex, SymbolLoad) {
        let index = self.index();
        let map = RepositoryMap::build(&index, &CommandPolicy::new(self.config()));
        self.store()
            .load_or_build(&index, &map, &PathPolicy::new(&self.config()))
            .expect("the symbol index should load")
    }

    fn index_file(&self) -> PathBuf {
        SymbolIndexStore::path(&self.data_dir, &self.index().repository_id)
    }

    fn stored(&self) -> serde_json::Value {
        serde_json::from_slice(&fs::read(self.index_file()).unwrap()).unwrap()
    }

    fn store_json(&self, value: &serde_json::Value) {
        fs::write(self.index_file(), serde_json::to_vec_pretty(value).unwrap()).unwrap();
    }

    /// The `symbol_index_rebuilt` events in the audit log, oldest first.
    fn rebuild_events(&self) -> Vec<serde_json::Value> {
        fs::read_to_string(self.data_dir.join("audit/events.jsonl"))
            .unwrap_or_default()
            .lines()
            .map(|line| serde_json::from_str::<serde_json::Value>(line).unwrap())
            .filter(|event| event["eventType"] == "symbol_index_rebuilt")
            .collect()
    }
}

/// `(name, path, line, provenance)` of every symbol.
fn entries(index: &SymbolIndex) -> Vec<(String, String, u32, Provenance)> {
    index
        .symbols
        .iter()
        .map(|symbol| {
            (
                symbol.name.clone(),
                symbol.path.clone(),
                symbol.line,
                symbol.provenance,
            )
        })
        .collect()
}

fn two_file_repo(name: &str) -> Repo {
    Repo::new(
        name,
        &[
            ("src/alpha.rs", "fn alpha() {}\n"),
            ("src/beta.rs", "fn beta() {}\n"),
        ],
    )
}

#[test]
fn a_missing_index_is_built_and_an_unchanged_repository_reuses_it() {
    let repo = two_file_repo("store-reuse");
    let (built, outcome) = repo.load();
    assert_eq!(outcome, SymbolLoad::Built);
    let path = repo.index_file();
    assert!(
        path.ends_with(format!("symbol-index/{}.json", built.repository_id)),
        "{}",
        path.display()
    );
    let bytes = fs::read(&path).expect("the index is written");
    assert_eq!(
        serde_json::from_slice::<SymbolIndex>(&bytes).unwrap(),
        built
    );
    let stored = repo.stored();
    assert_eq!(stored["schemaVersion"], SYMBOL_INDEX_SCHEMA_VERSION);
    let hashes: Vec<(String, String)> = repo
        .index()
        .files
        .iter()
        .map(|file| (file.path.clone(), file.content_hash.clone()))
        .collect();
    let stored_hashes: Vec<(String, String)> = stored["files"]
        .as_array()
        .unwrap()
        .iter()
        .map(|file| {
            (
                file["path"].as_str().unwrap().to_string(),
                file["contentHash"].as_str().unwrap().to_string(),
            )
        })
        .collect();
    assert_eq!(stored_hashes, hashes, "one content hash per file");

    let (reused, outcome) = repo.load();
    assert_eq!(outcome, SymbolLoad::Reused);
    assert_eq!(reused, built);
    assert_eq!(fs::read(&path).unwrap(), bytes, "a reuse does not rewrite");
    assert!(repo.rebuild_events().is_empty(), "neither is a rebuild");
    let entries: Vec<_> = fs::read_dir(path.parent().unwrap()).unwrap().collect();
    assert_eq!(entries.len(), 1, "no temp file is left beside the index");
}

#[test]
fn a_garbage_index_file_is_rebuilt_as_corrupt_and_audited() {
    let repo = two_file_repo("store-corrupt");
    let path = repo.index_file();
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(&path, b"\x00\xffnot json at all").unwrap();

    let (index, outcome) = repo.load();
    assert_eq!(
        outcome,
        SymbolLoad::Rebuilt {
            reason: RebuildReason::Corrupt
        }
    );
    assert_eq!(
        locations(&index, "alpha"),
        [("src/alpha.rs".to_string(), 1)]
    );
    assert_eq!(
        serde_json::from_slice::<SymbolIndex>(&fs::read(&path).unwrap()).unwrap(),
        index
    );
    let events = repo.rebuild_events();
    assert_eq!(events.len(), 1, "{events:?}");
    assert_eq!(events[0]["reason"], "corrupt");
    assert_eq!(events[0]["repositoryId"], index.repository_id.as_str());
    assert_eq!(events[0]["fileCount"], "2");
    assert_eq!(events[0]["totalFiles"], "2");
    assert!(
        events[0]["durationMs"]
            .as_str()
            .is_some_and(|ms| ms.parse::<u64>().is_ok()),
        "{events:?}"
    );
    assert_eq!(repo.load().1, SymbolLoad::Reused);
    assert_eq!(repo.rebuild_events().len(), 1);
}

#[test]
fn an_index_from_another_schema_version_is_rebuilt_as_a_mismatch() {
    let repo = two_file_repo("store-schema");
    let (built, _) = repo.load();
    let mut stored = repo.stored();
    stored["schemaVersion"] = (SYMBOL_INDEX_SCHEMA_VERSION + 1).into();
    // Otherwise valid and current: only the version is wrong.
    repo.store_json(&stored);

    let (rebuilt, outcome) = repo.load();
    assert_eq!(
        outcome,
        SymbolLoad::Rebuilt {
            reason: RebuildReason::SchemaMismatch {
                found: SYMBOL_INDEX_SCHEMA_VERSION + 1
            }
        }
    );
    assert_eq!(rebuilt, built);
    let events = repo.rebuild_events();
    assert_eq!(events.len(), 1, "{events:?}");
    assert_eq!(events[0]["reason"], "schemaMismatch");
    assert_eq!(
        events[0]["foundSchemaVersion"],
        (SYMBOL_INDEX_SCHEMA_VERSION + 1).to_string()
    );
    assert_eq!(repo.stored()["schemaVersion"], SYMBOL_INDEX_SCHEMA_VERSION);
}

#[test]
fn an_index_of_another_repository_is_rebuilt_as_corrupt() {
    let repo = two_file_repo("store-foreign");
    repo.load();
    let mut stored = repo.stored();
    stored["repositoryId"] = "someone-else".into();
    repo.store_json(&stored);

    assert_eq!(
        repo.load().1,
        SymbolLoad::Rebuilt {
            reason: RebuildReason::Corrupt
        }
    );
}

#[test]
fn an_edited_file_is_taken_again_and_unchanged_files_keep_either_tier() {
    let repo = two_file_repo("store-incremental");
    repo.load();

    // Mark every stored symbol as a language server's, on a line the
    // heuristic would not give, and add a stored reference between the files.
    let mut stored = repo.stored();
    for symbol in stored["symbols"].as_array_mut().unwrap() {
        symbol["provenance"] = "language_server".into();
        symbol["line"] = 7.into();
    }
    stored["relations"] = serde_json::json!([{
        "kind": "references",
        "from": { "path": "src/alpha.rs", "name": "alpha", "line": 7 },
        "to": { "type": "symbol", "path": "src/beta.rs", "name": "beta", "line": 7 },
        "provenance": "language_server"
    }]);
    repo.store_json(&stored);

    let (kept, outcome) = repo.load();
    assert_eq!(outcome, SymbolLoad::Reused, "nothing changed");
    assert_eq!(
        entries(&kept),
        [
            (
                "alpha".into(),
                "src/alpha.rs".into(),
                7,
                Provenance::LanguageServer
            ),
            (
                "beta".into(),
                "src/beta.rs".into(),
                7,
                Provenance::LanguageServer
            ),
        ]
    );
    assert_eq!(kept.relations.len(), 1, "{:?}", kept.relations);

    repo.write("src/beta.rs", "fn beta() {}\nfn gamma() {}\n");
    repo.write("src/delta.rs", "fn delta() {}\n");
    let (updated, outcome) = repo.load();
    assert_eq!(
        outcome,
        SymbolLoad::Rebuilt {
            reason: RebuildReason::Stale
        }
    );
    assert_eq!(
        entries(&updated),
        [
            (
                "alpha".into(),
                "src/alpha.rs".into(),
                7,
                Provenance::LanguageServer
            ),
            (
                "beta".into(),
                "src/beta.rs".into(),
                1,
                Provenance::Heuristic
            ),
            (
                "gamma".into(),
                "src/beta.rs".into(),
                2,
                Provenance::Heuristic
            ),
            (
                "delta".into(),
                "src/delta.rs".into(),
                1,
                Provenance::Heuristic
            ),
        ],
        "the edited file's language-server entry is dropped, the other's kept"
    );
    assert!(
        updated.relations.is_empty(),
        "a reference into the edited file is dropped: {:?}",
        updated.relations
    );
    let events = repo.rebuild_events();
    assert_eq!(events.len(), 1, "{events:?}");
    assert_eq!(events[0]["reason"], "stale");
    assert_eq!(events[0]["fileCount"], "2", "beta and delta, not alpha");
    assert_eq!(events[0]["totalFiles"], "3");

    fs::remove_file(repo.root.join("src/alpha.rs")).unwrap();
    let (pruned, _) = repo.load();
    assert!(locations(&pruned, "alpha").is_empty());
    assert!(pruned.files.iter().all(|file| file.path != "src/alpha.rs"));
}

#[test]
fn the_engine_finds_symbols_from_the_stored_index() {
    let repo = two_file_repo("engine");
    let engine = WorkspaceEngine::new(repo.config());

    let (_, outcome) = engine.symbol_index(&repo.root).unwrap();
    assert_eq!(outcome, SymbolLoad::Built);
    let found = engine.find_symbols(&repo.root, "alpha", 10).unwrap();
    assert_eq!(found.len(), 1, "{found:?}");
    assert_eq!(found[0].quality, MatchQuality::Exact);
    assert_eq!(
        (
            found[0].symbol.path.as_str(),
            found[0].symbol.line,
            found[0].symbol.provenance
        ),
        ("src/alpha.rs", 1, Provenance::Heuristic)
    );
    assert_eq!(
        engine.symbol_index(&repo.root).unwrap().1,
        SymbolLoad::Reused
    );
}

/// Times `SymbolIndexStore::load_or_build` on this checkout, for proposal §7.
/// Ignored because it reads the user's own checkout. Run it with a release
/// build:
///
/// ```text
/// CARGO_TARGET_DIR="$PWD/target" cargo test --release -p workspace-engine \
///   --test symbol_index -- --ignored --nocapture measures_load_or_build_on_this_repository
/// ```
#[test]
#[ignore = "reads this checkout; run by hand for proposal §7"]
fn measures_load_or_build_on_this_repository() {
    let checkout = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..");
    let repo = Repo::new("measure", &[]);
    let config = Config {
        data_dir: repo.data_dir.clone(),
        enable_index_watcher: false,
        ..Config::default()
    };
    let scanner = SecretScanner::new(config.secret_patterns.clone());
    let audit_log = AuditLog::new(&repo.data_dir, true, scanner.clone());
    let index = ProjectIndexer::new(config.clone(), scanner, audit_log.clone())
        .index_repository(&checkout)
        .unwrap();
    let map = RepositoryMap::build(&index, &CommandPolicy::new(config.clone()));
    let policy = PathPolicy::new(&config);
    let store = SymbolIndexStore::new(&repo.data_dir, audit_log);

    let started = std::time::Instant::now();
    let (built, outcome) = store.load_or_build(&index, &map, &policy).unwrap();
    let built_time = started.elapsed();
    assert_eq!(outcome, SymbolLoad::Built);
    let started = std::time::Instant::now();
    let (_, outcome) = store.load_or_build(&index, &map, &policy).unwrap();
    let reused_time = started.elapsed();
    assert_eq!(outcome, SymbolLoad::Reused);
    let size = fs::metadata(SymbolIndexStore::path(&repo.data_dir, &index.repository_id))
        .unwrap()
        .len();
    println!(
        "files {} symbols {} relations {} | built {built_time:?} | reused {reused_time:?} | {size} bytes",
        built.files.len(),
        built.symbols.len(),
        built.relations.len()
    );
}

#[test]
fn a_reused_file_takes_its_root_from_the_current_map() {
    let repo = Repo::new(
        "store-root",
        &[("packages/web/index.ts", "export function web() {}\n")],
    );
    let (before, _) = repo.load();
    assert_eq!(before.symbols[0].root_path, "");

    // The file is unchanged; a new manifest moves it into its own root.
    repo.write("packages/web/package.json", "{}\n");
    let (after, _) = repo.load();
    let web = after
        .symbols
        .iter()
        .find(|symbol| symbol.name == "web")
        .expect("the symbol is kept");
    assert_eq!(web.root_path, "packages/web");
}

// ---- Task 5: configuration, the hub and process lifetime ----
//
// The fake server is this test binary, re-executed into
// `fake_language_server_helper` (context.md §2). libtest prints its own
// header on stdout before the helper runs, so the helper speaks LSP on fd 3,
// and the command line moves fd 3 onto the pipe the hub reads and sends
// stdout to /dev/null. `/bin/sh -c` does that, and only that: it `exec`s the
// test binary, so the PID the hub records is the helper's own.

const FAKE_MODE: &str = "DAMAIAN_FAKE_LSP_MODE";
const FAKE_LOG: &str = "DAMAIAN_FAKE_LSP_LOG";
const FAKE_REGISTRY: &str = "DAMAIAN_FAKE_LSP_REGISTRY";

/// A temp repository with a data directory, a fake server log, and the
/// config that names the fake server for Rust.
struct ServerFixture {
    repo: Repo,
    log: PathBuf,
}

impl ServerFixture {
    fn new(name: &str) -> Self {
        let repo = Repo::new(name, &[("src/lib.rs", "pub fn alpha() {}\n")]);
        let log = repo.data_dir.join("fake-server.log");
        Self { repo, log }
    }

    /// The `language_server.rust` command line for a fake server in `mode`.
    fn command(&self, mode: &str) -> String {
        let binary = std::env::current_exe().expect("test binary path");
        format!(
            "/bin/sh -c \"export {FAKE_MODE}={mode} {FAKE_LOG}='{}' {FAKE_REGISTRY}='{}'; \
             exec \\\"\\$0\\\" --ignored --exact fake_language_server_helper 3>&1 1>/dev/null\" '{}'",
            self.log.display(),
            self.repo.data_dir.join("processes").display(),
            binary.display(),
        )
    }

    fn launch(&self, mode: &str) -> LanguageServerLaunch {
        let mut config = self.repo.config();
        config
            .language_servers
            .insert("rust".to_string(), self.command(mode));
        LanguageServerLaunch::from_config(&config, &self.repo.root, "rust", "ses_task5")
            .expect("rust is configured")
    }

    fn registry_entries(&self) -> Vec<RegisteredProcess> {
        ProcessRegistry::open(&self.repo.data_dir)
            .unwrap()
            .entries()
            .unwrap()
            .into_iter()
            .map(|(path, entry)| entry.unwrap_or_else(|| panic!("unreadable entry {path:?}")))
            .collect()
    }

    fn log_lines(&self) -> Vec<String> {
        fs::read_to_string(&self.log)
            .unwrap_or_default()
            .lines()
            .map(str::to_string)
            .collect()
    }

    fn spawns(&self) -> usize {
        self.log_lines()
            .iter()
            .filter(|line| line.starts_with("start "))
            .count()
    }
}

fn limits(request_timeout: Duration, idle: Duration) -> LanguageServerLimits {
    LanguageServerLimits {
        request_timeout,
        idle,
        shutdown_grace: Duration::from_millis(500),
    }
}

/// Generous: a process launch on the development Mac can stall for seconds
/// (`AGENTS.md`), and a fake server's `initialize` is one launch away.
const START_TIMEOUT: Duration = Duration::from_secs(20);

fn document_symbol(
    hub: &LanguageServerHub,
    launch: &LanguageServerLaunch,
) -> Result<serde_json::Value, LanguageServerError> {
    hub.request(launch, |client, uris| {
        client.document_symbol(&uris.uri_for("src/lib.rs"))
    })
}

fn wait_until(what: &str, limit: Duration, done: impl Fn() -> bool) {
    let deadline = Instant::now() + limit;
    while !done() {
        assert!(Instant::now() < deadline, "timed out waiting for {what}");
        std::thread::sleep(Duration::from_millis(20));
    }
}

fn is_running(pid: u32) -> bool {
    ProcessIdentity::of(pid).is_some()
}

#[test]
fn with_no_language_server_configured_nothing_is_launched_or_recorded() {
    let fixture = ServerFixture::new("lsp-none");
    let engine = WorkspaceEngine::new(fixture.repo.config());
    for language in LANGUAGE_SERVER_LANGUAGES {
        assert_eq!(
            engine.language_server_launch(&fixture.repo.root, language, "ses_task5"),
            None,
            "{language}"
        );
    }
    // Configuring one language starts nothing for another.
    let mut config = fixture.repo.config();
    config
        .language_servers
        .insert("rust".to_string(), fixture.command("normal"));
    assert!(
        LanguageServerLaunch::from_config(&config, &fixture.repo.root, "python", "ses").is_none()
    );

    let hub = LanguageServerHub::with_limits(LanguageServerLimits::default());
    let repository_id = workspace_engine::repository_id_for_root(&fixture.repo.root);
    assert!(hub.status(&repository_id).is_empty());
    assert!(fixture.registry_entries().is_empty());
    assert_eq!(fixture.spawns(), 0);
}

#[test]
fn a_repository_cannot_name_a_language_server_and_the_users_stands() {
    let fixture = ServerFixture::new("lsp-repo-config");
    let user_config = fixture.repo.data_dir.join("config").join("user.conf");
    fs::create_dir_all(user_config.parent().unwrap()).unwrap();
    fs::write(
        &user_config,
        "language_server.rust=/usr/local/bin/rust-analyzer\n",
    )
    .unwrap();
    let repository_config = Config::repository_config_path(&fixture.repo.root);
    fs::create_dir_all(repository_config.parent().unwrap()).unwrap();
    fs::write(
        &repository_config,
        "language_server.rust=/bin/sh\nlanguage_server.python=./tools/pyls\n",
    )
    .unwrap();

    let (config, report) = Config::load_scoped(
        fixture.repo.config(),
        Some(&user_config),
        Some(&repository_config),
        None,
        Some(&fixture.repo.root),
    )
    .unwrap();

    assert_eq!(
        config.language_servers.get("rust").map(String::as_str),
        Some("/usr/local/bin/rust-analyzer")
    );
    assert_eq!(config.language_servers.get("python"), None);
    let refused: Vec<_> = report
        .rejected_keys
        .iter()
        .map(|rejected| (rejected.key.as_str(), rejected.class))
        .collect();
    assert!(
        refused.contains(&("language_server.rust", RepositoryKeyClass::Forbidden)),
        "{refused:?}"
    );
    assert!(
        refused.contains(&("language_server.python", RepositoryKeyClass::Forbidden)),
        "{refused:?}"
    );

    let audit = AuditLog::new(&fixture.repo.data_dir, true, SecretScanner::new(Vec::new()));
    RepositoryTrustStore::new(&fixture.repo.data_dir)
        .review(&report, &audit)
        .unwrap()
        .expect("the refusal is surfaced");
    let events =
        fs::read_to_string(fixture.repo.data_dir.join("audit").join("events.jsonl")).unwrap();
    assert!(
        events.contains("repository_config_key_rejected"),
        "{events}"
    );
    assert!(events.contains("language_server.rust"), "{events}");
    assert!(
        !events.contains("./tools/pyls"),
        "a refused value was logged"
    );
}

#[test]
fn an_unknown_language_is_a_parse_error() {
    let error = ConfigOverlay::parse("language_server.cobol=/usr/bin/cobol-ls\n").unwrap_err();
    assert!(
        error.to_string().contains("language_server.cobol"),
        "{error}"
    );
    assert!(ConfigOverlay::parse("language_server.rust=\n").is_err());
    let overlay = ConfigOverlay::parse("language_server.rust=rust-analyzer\n").unwrap();
    assert_eq!(
        overlay.to_policy_text(),
        "language_server.rust=rust-analyzer\n"
    );
}

#[test]
fn a_running_server_has_one_registry_entry_until_shutdown_all() {
    let fixture = ServerFixture::new("lsp-normal");
    let launch = fixture.launch("normal");
    let hub = LanguageServerHub::with_limits(limits(START_TIMEOUT, Duration::from_secs(600)));

    let answer = document_symbol(&hub, &launch).expect("the fake server answers");
    assert_eq!(answer, serde_json::json!([]));

    let entries = fixture.registry_entries();
    assert_eq!(entries.len(), 1, "{entries:?}");
    assert_eq!(entries[0].kind, "language_server");
    assert_eq!(entries[0].session_id, "ses_task5");
    let status = hub.status(&launch.repository_id);
    assert_eq!(status.len(), 1);
    assert_eq!(status[0].state, LanguageServerState::Ready);
    assert_eq!(status[0].language, "rust");
    let pid = status[0].pid.expect("a ready server has a pid");
    assert_eq!(entries[0].pid, pid);
    assert!(is_running(pid));
    // The entry exists by the time the server reads `initialize`. That alone
    // does not prove the order: the parent wins the race even when it writes
    // first. `lsp.rs`'s `a_server_is_registered_before_anything_is_written_to_it`
    // does.
    assert!(
        fixture
            .log_lines()
            .contains(&"initialize registered=true".to_string()),
        "{:?}",
        fixture.log_lines()
    );

    hub.shutdown_all();

    assert!(fixture.registry_entries().is_empty());
    assert!(!is_running(pid), "the server outlived shutdown_all");
    assert!(hub.status(&launch.repository_id).is_empty());
    let log = fixture.log_lines();
    assert!(
        log.contains(&"shutdown".to_string()) && log.contains(&"exit".to_string()),
        "the polite path was not taken: {log:?}"
    );
    assert_eq!(fixture.spawns(), 1);
}

#[test]
fn an_idle_server_is_stopped_and_its_entry_removed() {
    let fixture = ServerFixture::new("lsp-idle");
    let launch = fixture.launch("normal");
    let hub = LanguageServerHub::with_limits(limits(START_TIMEOUT, Duration::from_millis(300)));

    document_symbol(&hub, &launch).expect("the fake server answers");
    assert_eq!(fixture.registry_entries().len(), 1);
    let pid = fixture.registry_entries()[0].pid;

    // Nothing asks the hub anything from here: the reaper alone stops it.
    wait_until("the idle server to stop", Duration::from_secs(20), || {
        fixture.registry_entries().is_empty() && !is_running(pid)
    });
    assert!(hub.status(&launch.repository_id).is_empty());
}

#[test]
fn a_server_that_never_answers_initialize_is_unavailable_and_then_answers_at_once() {
    let fixture = ServerFixture::new("lsp-silent");
    let launch = fixture.launch("silent");
    let timeout = Duration::from_secs(2);
    let hub = LanguageServerHub::with_limits(limits(timeout, Duration::from_secs(600)));

    let started = Instant::now();
    let first = document_symbol(&hub, &launch);
    let waited = started.elapsed();
    let Err(LanguageServerError::Unavailable { reason }) = first else {
        panic!("expected unavailable, got {first:?}");
    };
    assert!(reason.contains("initialize"), "{reason}");
    assert!(waited >= timeout, "gave up before the timeout: {waited:?}");
    // The timeout, plus one process launch and a kill.
    assert!(waited < timeout + START_TIMEOUT, "{waited:?}");
    assert!(matches!(
        hub.status(&launch.repository_id)[0].state,
        LanguageServerState::Unavailable { .. }
    ));

    let started = Instant::now();
    let second = document_symbol(&hub, &launch);
    assert!(
        matches!(second, Err(LanguageServerError::Unavailable { .. })),
        "{second:?}"
    );
    assert!(
        started.elapsed() < Duration::from_millis(200),
        "an unavailable server must answer at once: {:?}",
        started.elapsed()
    );
    assert!(fixture.registry_entries().is_empty());
    assert_eq!(fixture.spawns(), 1, "an unavailable server is not retried");
}

#[test]
fn a_server_that_crashes_twice_is_restarted_once_and_then_left_down() {
    let fixture = ServerFixture::new("lsp-crash");
    let launch = fixture.launch("exit_after_1");
    let hub = LanguageServerHub::with_limits(limits(START_TIMEOUT, Duration::from_secs(600)));

    let results: Vec<_> = (0..6).map(|_| document_symbol(&hub, &launch)).collect();

    let answered = results.iter().filter(|result| result.is_ok()).count();
    assert_eq!(answered, 2, "one answer per process: {results:?}");
    assert_eq!(fixture.spawns(), 2, "{:?}", fixture.log_lines());
    let status = hub.status(&launch.repository_id);
    assert_eq!(
        status[0].state,
        LanguageServerState::Crashed { restarts: 1 },
        "{status:?}"
    );
    assert_eq!(status[0].pid, None);
    assert!(fixture.registry_entries().is_empty());
    for result in &results[results.len() - 2..] {
        assert!(
            matches!(result, Err(LanguageServerError::Unavailable { .. })),
            "{result:?}"
        );
    }

    // And no third process, however often it is asked.
    let started = Instant::now();
    assert!(document_symbol(&hub, &launch).is_err());
    assert!(started.elapsed() < Duration::from_millis(200));
    assert_eq!(fixture.spawns(), 2);
}

/// Spec 46's start-time check, over a `LanguageServer` entry (context.md §1):
/// the recorded PID is alive, but its start time is not the one recorded, so
/// the launch sweep refuses to kill it. The PID is this test process, which
/// is why the kill is injected: if the check failed, the sweep would kill the
/// test runner rather than report it.
#[test]
fn a_language_server_entry_whose_start_time_does_not_match_is_not_killed() {
    let fixture = ServerFixture::new("lsp-sweep");
    let config = fixture.repo.config();
    let (registry, audit) = ProcessRegistry::open_with_audit(&config).unwrap();
    let ours = ProcessIdentity::of(std::process::id()).unwrap();
    let entry = RegisteredProcess {
        pid: ours.pid,
        start_time_us: ours.start_time_us - 1,
        pgid: ours.pid,
        kind: ProcessKind::LanguageServer.as_str().to_string(),
        session_id: "ses_task5".to_string(),
        registered_at_ms: 0,
        // An owner that is provably gone, as after a crash.
        owner_pid: 99_999_999,
        owner_start_time_us: 1,
    };
    let processes = fixture.repo.data_dir.join("processes");
    fs::write(
        processes.join(entry.file_name()),
        serde_json::to_string(&entry).unwrap(),
    )
    .unwrap();

    let mut killed = Vec::new();
    let report = registry
        .sweep_with(
            &audit,
            SweepScope::CrashedOwners,
            &|pid| ProcessIdentity::of(pid),
            &mut |entry: &RegisteredProcess| killed.push(entry.pid),
        )
        .unwrap();

    assert!(killed.is_empty(), "a recycled PID was killed");
    assert_eq!(report.refused(), 1, "{report:?}");
    let events =
        fs::read_to_string(fixture.repo.data_dir.join("audit").join("events.jsonl")).unwrap();
    assert!(events.contains("orphan_process_kill_refused"), "{events}");
    assert!(events.contains("\"kind\":\"language_server\""), "{events}");
    assert!(
        fixture.registry_entries().is_empty(),
        "the refused entry is removed"
    );
}

/// The fake language server. Never run on its own: a test re-executes the
/// test binary into it through `ServerFixture::command`, and run directly it
/// returns at once. Its behaviour is `DAMAIAN_FAKE_LSP_MODE`:
///
/// - `normal`: answers `initialize`, `shutdown`, and every other request with
///   an empty result;
/// - `silent`: never answers `initialize`;
/// - `exit_after_<n>`: answers `n` requests after `initialize`, then exits;
/// - `exit_at_once`: exits before reading anything.
///
/// It appends what happened to `DAMAIAN_FAKE_LSP_LOG`, and on `initialize`
/// records whether its own registry entry already existed.
#[test]
#[ignore = "the fake language server; re-executed by the Task 5 tests"]
fn fake_language_server_helper() {
    use std::io::Write;
    use std::os::fd::FromRawFd;

    let Ok(mode) = std::env::var(FAKE_MODE) else {
        return;
    };
    let log_path = std::env::var(FAKE_LOG).expect("a log path");
    let registry = PathBuf::from(std::env::var(FAKE_REGISTRY).expect("a registry path"));
    let log = |line: &str| {
        let mut file = fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&log_path)
            .unwrap();
        writeln!(file, "{line}").unwrap();
    };
    let pid = std::process::id();
    log(&format!("start {pid}"));
    if mode == "exit_at_once" {
        std::process::exit(0);
    }
    let exit_after = mode
        .strip_prefix("exit_after_")
        .map(|count| count.parse::<usize>().unwrap());

    // SAFETY: fd 3 is the pipe the command line moved there, and nothing
    // else in this process owns it.
    let mut output = unsafe { fs::File::from_raw_fd(3) };
    let stdin = std::io::stdin();
    let mut input = stdin.lock();
    let mut answered = 0;
    let mut reply = |id: &serde_json::Value, result: serde_json::Value| {
        let message = serde_json::json!({ "jsonrpc": "2.0", "id": id, "result": result });
        write_message(&mut output, &message).unwrap();
    };
    loop {
        let Ok(Some(message)) = read_message(&mut input) else {
            std::process::exit(0);
        };
        let method = message["method"].as_str().unwrap_or_default().to_string();
        let id = message.get("id").cloned();
        match (method.as_str(), id) {
            ("initialize", Some(id)) => {
                let prefix = format!("{pid}-");
                let registered = fs::read_dir(&registry).is_ok_and(|entries| {
                    entries
                        .filter_map(|entry| entry.ok())
                        .any(|entry| entry.file_name().to_string_lossy().starts_with(&prefix))
                });
                log(&format!("initialize registered={registered}"));
                if mode != "silent" {
                    reply(&id, serde_json::json!({ "capabilities": {} }));
                }
            }
            ("shutdown", Some(id)) => {
                log("shutdown");
                reply(&id, serde_json::Value::Null);
            }
            ("exit", None) => {
                log("exit");
                std::process::exit(0);
            }
            (method, Some(id)) => {
                log(&format!("request {method}"));
                reply(&id, serde_json::json!([]));
                answered += 1;
                if exit_after.is_some_and(|limit| answered >= limit) {
                    std::process::exit(0);
                }
            }
            (_, None) => {}
        }
    }
}

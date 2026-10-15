//! Spec 25: the symbol and relationship index.
//!
//! [`SymbolIndex::build`] turns the heuristic declarations and imports the
//! index pass already extracted into [`Symbol`]s and [`Relation`]s, each
//! labelled with its [`Provenance`]. It is a pure function of the index, the
//! repository map and the path policy, so the same inputs always give the
//! same, sorted output.

use std::collections::BTreeSet;

use serde::{Deserialize, Serialize};

use crate::indexer::{FileRecord, RepositoryIndex};
use crate::language::SymbolKind;
use crate::path_policy::PathPolicy;
use crate::repository_map::RepositoryMap;

/// How a symbol or relationship was derived. Requirement 3.
///
/// Two values rather than a confidence score: what a reader needs to know is
/// whether an item may be presented as fact or must be hedged (proposal
/// §5.1). `Ord` puts `LanguageServer` first, which is how lookups and the
/// merge of the two tiers rank them.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Provenance {
    /// Reported by a language server. Authoritative.
    LanguageServer,
    /// Derived from source text by Damaian's own matching. A guess.
    Heuristic,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Symbol {
    pub name: String,
    pub kind: SymbolKind,
    /// Repository-relative path, plus the line the declaration starts on.
    pub path: String,
    /// 1-based.
    pub line: u32,
    /// 1-based.
    pub column: Option<u32>,
    /// Enclosing symbol, where known: the type a method belongs to.
    pub container: Option<String>,
    /// Nearest enclosing project root, from spec 24.
    pub root_path: String,
    pub provenance: Provenance,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RelationKind {
    /// File → file, or file → external package.
    Imports,
    /// Symbol → symbol.
    References,
    /// Type → trait or interface.
    Implements,
    /// Type → supertype.
    Extends,
    /// Test file or symbol → subject.
    TestOf,
}

/// One end of a relation. A file-level end, such as the importing file of an
/// `Imports` edge, has an empty `name` and `line` 0.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SymbolRef {
    pub path: String,
    pub name: String,
    pub line: u32,
}

impl SymbolRef {
    fn file(path: &str) -> Self {
        Self {
            path: path.to_string(),
            name: String::new(),
            line: 0,
        }
    }
}

/// An import may not resolve to a file in this repository.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "camelCase")]
pub enum RelationTarget {
    /// Resolved to a path inside the repository.
    File {
        path: String,
    },
    Symbol(SymbolRef),
    /// An external dependency, kept as the raw specifier.
    External {
        specifier: String,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Relation {
    pub kind: RelationKind,
    pub from: SymbolRef,
    pub to: RelationTarget,
    pub provenance: Provenance,
}

/// How well a symbol's name matched a lookup. Better matches sort first.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MatchQuality {
    /// The name is the query.
    Exact,
    /// The name starts with the query, ignoring case.
    Prefix,
    /// The query's characters appear in the name in order, ignoring case.
    Fuzzy,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SymbolMatch {
    pub symbol: Symbol,
    pub quality: MatchQuality,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SymbolIndex {
    pub repository_id: String,
    /// Sorted by path, line, column and name.
    pub symbols: Vec<Symbol>,
    /// Sorted, with no duplicates.
    pub relations: Vec<Relation>,
}

/// The extensions a relative JavaScript or TypeScript specifier may omit, in
/// the order they are tried (`context.md` §11).
const SCRIPT_EXTENSIONS: [&str; 6] = ["ts", "tsx", "js", "jsx", "mjs", "cjs"];

impl SymbolIndex {
    /// Every item is `Heuristic`: this is the tier that needs no language
    /// server. Files the policy restricts contribute nothing, as symbols,
    /// as import sources or targets, or as tests or subjects (`context.md`
    /// §7), because the index itself does not filter them.
    pub fn build(index: &RepositoryIndex, map: &RepositoryMap, policy: &PathPolicy) -> Self {
        let records: Vec<&FileRecord> = index
            .files
            .iter()
            .filter(|record| !policy.is_restricted(&record.path, false))
            .collect();
        let paths: BTreeSet<&str> = records.iter().map(|record| record.path.as_str()).collect();

        let mut symbols = Vec::new();
        let mut relations = Vec::new();
        for record in &records {
            let root_path = map.root_for_path(&record.path);
            symbols.extend(record.declarations.iter().map(|declaration| Symbol {
                name: declaration.name.clone(),
                kind: declaration.kind,
                path: record.path.clone(),
                line: declaration.line,
                column: declaration.column,
                container: declaration.container.clone(),
                root_path: root_path.to_string(),
                provenance: Provenance::Heuristic,
            }));
            for specifier in &record.imports {
                let to = resolve_import(&record.path, &record.language, specifier, &paths)
                    .map_or_else(
                        || RelationTarget::External {
                            specifier: specifier.clone(),
                        },
                        |path| RelationTarget::File { path },
                    );
                relations.push(Relation {
                    kind: RelationKind::Imports,
                    from: SymbolRef::file(&record.path),
                    to,
                    provenance: Provenance::Heuristic,
                });
            }
            if let Some(subject) = test_subject(record, map, &paths) {
                relations.push(Relation {
                    kind: RelationKind::TestOf,
                    from: SymbolRef::file(&record.path),
                    to: RelationTarget::File { path: subject },
                    provenance: Provenance::Heuristic,
                });
            }
        }

        symbols.sort_by(|left, right| {
            (
                &left.path,
                left.line,
                left.column,
                &left.name,
                &left.container,
            )
                .cmp(&(
                    &right.path,
                    right.line,
                    right.column,
                    &right.name,
                    &right.container,
                ))
        });
        relations.sort_by(|left, right| {
            (&left.from, left.kind, &left.to).cmp(&(&right.from, right.kind, &right.to))
        });
        relations.dedup();

        Self {
            repository_id: index.repository_id.clone(),
            symbols,
            relations,
        }
    }

    /// Exact matches, then prefix, then fuzzy, ordered by provenance first
    /// (proposal §5.11), then match quality, then path and line. An empty
    /// query matches nothing.
    pub fn find(&self, query: &str, limit: usize) -> Vec<SymbolMatch> {
        if query.is_empty() {
            return Vec::new();
        }
        let lowered = query.to_lowercase();
        let mut matches: Vec<SymbolMatch> = self
            .symbols
            .iter()
            .filter_map(|symbol| {
                match_quality(&symbol.name, query, &lowered).map(|quality| SymbolMatch {
                    symbol: symbol.clone(),
                    quality,
                })
            })
            .collect();
        matches.sort_by(|left, right| {
            (
                left.symbol.provenance,
                left.quality,
                &left.symbol.path,
                left.symbol.line,
                &left.symbol.name,
            )
                .cmp(&(
                    right.symbol.provenance,
                    right.quality,
                    &right.symbol.path,
                    right.symbol.line,
                    &right.symbol.name,
                ))
        });
        matches.truncate(limit);
        matches
    }
}

fn match_quality(name: &str, query: &str, lowered_query: &str) -> Option<MatchQuality> {
    if name == query {
        return Some(MatchQuality::Exact);
    }
    let lowered = name.to_lowercase();
    if lowered.starts_with(lowered_query) {
        return Some(MatchQuality::Prefix);
    }
    let mut remaining = lowered.chars();
    lowered_query
        .chars()
        .all(|wanted| remaining.any(|character| character == wanted))
        .then_some(MatchQuality::Fuzzy)
}

/// The indexed file a relative specifier names, or `None`, in which case the
/// import stays `External`. Only relative JavaScript, TypeScript and Python
/// specifiers resolve. Bare packages, `tsconfig` aliases and every Rust `use`
/// would need build configuration or a module model (`context.md` §11).
fn resolve_import(
    importer: &str,
    language: &str,
    specifier: &str,
    paths: &BTreeSet<&str>,
) -> Option<String> {
    let directory = parent(importer);
    let candidates = match language {
        "javascript" | "typescript" => {
            if !(specifier.starts_with("./") || specifier.starts_with("../")) {
                return None;
            }
            let base = normalize(&join(directory, specifier))?;
            let mut candidates = vec![base.clone()];
            candidates.extend(SCRIPT_EXTENSIONS.iter().map(|ext| format!("{base}.{ext}")));
            candidates.extend(
                SCRIPT_EXTENSIONS
                    .iter()
                    .map(|ext| join(&base, &format!("index.{ext}"))),
            );
            candidates
        }
        "python" => {
            let module = specifier.trim_start_matches('.');
            let dots = specifier.len() - module.len();
            if dots == 0 {
                return None;
            }
            // One dot is the importing file's own package; each further dot
            // is one package up.
            let mut package = directory.to_string();
            for _ in 1..dots {
                if package.is_empty() {
                    return None;
                }
                package = parent(&package).to_string();
            }
            if module.is_empty() {
                vec![join(&package, "__init__.py")]
            } else {
                let base = join(&package, &module.replace('.', "/"));
                vec![format!("{base}.py"), join(&base, "__init__.py")]
            }
        }
        _ => return None,
    };
    candidates
        .into_iter()
        .find(|candidate| paths.contains(candidate.as_str()))
}

/// The subject a test file tests, by naming convention, when that subject is
/// indexed and belongs to the same spec 24 root (`context.md` §11).
fn test_subject(
    record: &FileRecord,
    map: &RepositoryMap,
    paths: &BTreeSet<&str>,
) -> Option<String> {
    let root = map.root_for_path(&record.path);
    let directory = parent(&record.path);
    let file_name = record.path.rsplit('/').next().unwrap_or(&record.path);
    // The test directory of this root that holds the file, if any, and the
    // file's directory below it.
    let test_directory = map
        .roots
        .iter()
        .find(|candidate| candidate.path == root)
        .and_then(|owner| {
            owner.test_paths.iter().find_map(|test_path| {
                if directory == test_path {
                    Some(String::new())
                } else {
                    directory
                        .strip_prefix(test_path.as_str())
                        .and_then(|rest| rest.strip_prefix('/'))
                        .map(str::to_string)
                }
            })
        });

    let mut candidates = Vec::new();
    match record.language.as_str() {
        "rust" => {
            // `tests/<name>.rs` is an integration test of `src/<name>.rs`.
            if test_directory.as_deref() == Some("")
                && let Some(name) = file_name.strip_suffix(".rs")
            {
                candidates.push(join(root, &format!("src/{name}.rs")));
            }
        }
        "javascript" | "typescript" => {
            let subject_name = file_name.rsplit_once('.').and_then(|(stem, ext)| {
                [".test", ".spec"]
                    .iter()
                    .find_map(|marker| stem.strip_suffix(marker))
                    .map(|name| format!("{name}.{ext}"))
            });
            if let Some(subject_name) = subject_name {
                candidates.extend(subject_paths(
                    root,
                    directory,
                    test_directory.as_deref(),
                    &subject_name,
                ));
            }
        }
        "python" => {
            if let Some(name) = file_name
                .strip_prefix("test_")
                .filter(|name| name.ends_with(".py"))
            {
                candidates.extend(subject_paths(
                    root,
                    directory,
                    test_directory.as_deref(),
                    name,
                ));
            }
        }
        "go" => {
            if let Some(name) = file_name.strip_suffix("_test.go") {
                candidates.push(join(directory, &format!("{name}.go")));
            }
        }
        _ => {}
    }
    candidates.into_iter().find(|candidate| {
        candidate != &record.path
            && paths.contains(candidate.as_str())
            && map.root_for_path(candidate) == root
    })
}

/// Where the subject of a test named like `subject_name` may live: beside
/// the test, and, for a test inside one of the root's test directories, at
/// the same place below the root's `src` or below the root itself.
fn subject_paths(
    root: &str,
    directory: &str,
    below_test_directory: Option<&str>,
    subject_name: &str,
) -> Vec<String> {
    let mut paths = vec![join(directory, subject_name)];
    if let Some(below) = below_test_directory {
        let relative = join(below, subject_name);
        paths.push(join(root, &join("src", &relative)));
        paths.push(join(root, &relative));
    }
    paths
}

fn parent(path: &str) -> &str {
    path.rsplit_once('/').map_or("", |(directory, _)| directory)
}

fn join(directory: &str, path: &str) -> String {
    if directory.is_empty() {
        path.to_string()
    } else if path.is_empty() {
        directory.to_string()
    } else {
        format!("{directory}/{path}")
    }
}

/// Resolves `.` and `..` segments. `None` for a path that climbs above the
/// repository root, which cannot name an indexed file.
fn normalize(path: &str) -> Option<String> {
    let mut segments: Vec<&str> = Vec::new();
    for segment in path.split('/') {
        match segment {
            "" | "." => {}
            ".." => {
                segments.pop()?;
            }
            _ => segments.push(segment),
        }
    }
    Some(segments.join("/"))
}

//! Spec 25: the symbol and relationship index.
//!
//! [`SymbolIndex::build`] turns the heuristic declarations and imports the
//! index pass already extracted into [`Symbol`]s and [`Relation`]s, each
//! labelled with its [`Provenance`]. It is a pure function of the index, the
//! repository map and the path policy, so the same inputs always give the
//! same, sorted output.
//!
//! [`SymbolIndexStore`] persists it per repository, keyed by each file's
//! content hash, so an unchanged file keeps what is stored for it from either
//! tier, and only a changed or new file is taken from the index pass again.

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::io::ErrorKind;
use std::path::{Path, PathBuf};
use std::time::Instant;

use serde::{Deserialize, Serialize};

use crate::audit::AuditLog;
use crate::error::{ClientError, Result};
use crate::indexer::{FileRecord, RepositoryIndex};
use crate::language::SymbolKind;
use crate::path_policy::PathPolicy;
use crate::repository_map::{RebuildReason, RepositoryMap, write_replacing};

/// The stored index's layout. A stored index of another version is rebuilt,
/// never migrated: everything in it can be derived again.
pub const SYMBOL_INDEX_SCHEMA_VERSION: u32 = 1;

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

impl Provenance {
    /// The serde name, for surfaces that print rather than serialise.
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::LanguageServer => "language_server",
            Self::Heuristic => "heuristic",
        }
    }
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
    pub schema_version: u32,
    pub repository_id: String,
    /// Every file the symbols and relations were taken from, sorted by path:
    /// the indexed files the policy does not restrict.
    pub files: Vec<SymbolFile>,
    /// Sorted by path, line, column and name.
    pub symbols: Vec<Symbol>,
    /// Sorted, with no duplicates.
    pub relations: Vec<Relation>,
}

/// One source file and the hash of the content its symbols describe.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SymbolFile {
    pub path: String,
    pub content_hash: String,
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
        Self::assemble(index, map, policy, None).0
    }

    /// [`Self::build`], except that a file `stored` holds at the same content
    /// hash keeps its stored symbols, from either tier, and so do the stored
    /// language-server relations between such files. Every other file is
    /// taken from the index pass as heuristics, and heuristic relations are
    /// always derived again, because whether an import resolves depends on
    /// which other files exist. Root paths come from `map`, not from `stored`,
    /// since a new manifest moves a file into another root without changing
    /// it. Also returns how many files were taken from the index pass.
    fn assemble(
        index: &RepositoryIndex,
        map: &RepositoryMap,
        policy: &PathPolicy,
        stored: Option<&SymbolIndex>,
    ) -> (Self, usize) {
        let mut records: Vec<&FileRecord> = index
            .files
            .iter()
            .filter(|record| !policy.is_restricted(&record.path, false))
            .collect();
        records.sort_by(|left, right| left.path.cmp(&right.path));
        let paths: BTreeSet<&str> = records.iter().map(|record| record.path.as_str()).collect();

        let stored_hashes: BTreeMap<&str, &str> = stored
            .map(|stored| {
                stored
                    .files
                    .iter()
                    .map(|file| (file.path.as_str(), file.content_hash.as_str()))
                    .collect()
            })
            .unwrap_or_default();
        let mut stored_symbols: BTreeMap<&str, Vec<&Symbol>> = BTreeMap::new();
        for symbol in stored.iter().flat_map(|stored| &stored.symbols) {
            stored_symbols
                .entry(symbol.path.as_str())
                .or_default()
                .push(symbol);
        }
        let unchanged: BTreeSet<&str> = records
            .iter()
            .filter(|record| {
                stored_hashes.get(record.path.as_str()) == Some(&record.content_hash.as_str())
            })
            .map(|record| record.path.as_str())
            .collect();

        let mut symbols = Vec::new();
        let mut relations = Vec::new();
        let mut extracted = 0;
        for record in &records {
            let root_path = map.root_for_path(&record.path);
            if unchanged.contains(record.path.as_str()) {
                symbols.extend(
                    stored_symbols
                        .get(record.path.as_str())
                        .into_iter()
                        .flatten()
                        .map(|symbol| Symbol {
                            root_path: root_path.to_string(),
                            ..(*symbol).clone()
                        }),
                );
            } else {
                extracted += 1;
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
            }
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
        relations.extend(
            stored
                .iter()
                .flat_map(|stored| &stored.relations)
                .filter(|relation| {
                    relation.provenance != Provenance::Heuristic
                        && unchanged.contains(relation.from.path.as_str())
                        && match &relation.to {
                            RelationTarget::File { path } => unchanged.contains(path.as_str()),
                            RelationTarget::Symbol(target) => {
                                unchanged.contains(target.path.as_str())
                            }
                            RelationTarget::External { .. } => true,
                        }
                })
                .cloned(),
        );

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

        let files = records
            .iter()
            .map(|record| SymbolFile {
                path: record.path.clone(),
                content_hash: record.content_hash.clone(),
            })
            .collect();
        let index = Self {
            schema_version: SYMBOL_INDEX_SCHEMA_VERSION,
            repository_id: index.repository_id.clone(),
            files,
            symbols,
            relations,
        };
        (index, extracted)
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

/// Matches as a JSON array, for front ends that do not depend on serde.
pub fn symbol_matches_json(matches: &[SymbolMatch]) -> String {
    // Strings, numbers, options and unit enums, none of which can fail.
    serde_json::to_string(matches).expect("symbol matches always serialise")
}

/// How [`SymbolIndexStore::load_or_build`] got its index.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SymbolLoad {
    /// The stored index was still the index of these inputs.
    Reused,
    /// There was no stored index.
    Built,
    /// A stored index existed and could not be returned as stored. `Stale`
    /// means it was readable and some files were taken from it.
    Rebuilt { reason: RebuildReason },
}

/// The persisted symbol index, one JSON file per repository (proposal §5.10,
/// `context.md` §5), following [`crate::RepositoryMapStore`].
#[derive(Debug, Clone)]
pub struct SymbolIndexStore {
    data_dir: PathBuf,
    audit_log: AuditLog,
}

impl SymbolIndexStore {
    pub fn new(data_dir: impl AsRef<Path>, audit_log: AuditLog) -> Self {
        Self {
            data_dir: data_dir.as_ref().to_path_buf(),
            audit_log,
        }
    }

    pub fn path(data_dir: &Path, repository_id: &str) -> PathBuf {
        data_dir
            .join("symbol-index")
            .join(format!("{repository_id}.json"))
    }

    /// The index of `index`, reusing every stored file whose content hash
    /// still matches (see [`SymbolIndex::assemble`]). Anything that differs
    /// from what is stored is written before it is returned, and every
    /// rebuild is audited as `symbol_index_rebuilt` with its reason, the
    /// number of files taken from the index pass, and the time it took.
    pub fn load_or_build(
        &self,
        index: &RepositoryIndex,
        map: &RepositoryMap,
        policy: &PathPolicy,
    ) -> Result<(SymbolIndex, SymbolLoad)> {
        let started = Instant::now();
        let path = Self::path(&self.data_dir, &index.repository_id);
        let (stored, unusable) = match fs::read(&path) {
            Ok(bytes) => match stored_index(&bytes, &index.repository_id) {
                Ok(stored) => (Some(stored), None),
                Err(reason) => (None, Some(reason)),
            },
            Err(error) if error.kind() == ErrorKind::NotFound => (None, None),
            Err(error) => return Err(error.into()),
        };

        let (symbols, extracted) = SymbolIndex::assemble(index, map, policy, stored.as_ref());
        if stored.as_ref() == Some(&symbols) {
            return Ok((symbols, SymbolLoad::Reused));
        }
        let json = serde_json::to_vec_pretty(&symbols).map_err(|error| {
            ClientError::Io(format!("Failed to serialise the symbol index: {error}"))
        })?;
        write_replacing(&path, &json)?;
        let reason = match (stored, unusable) {
            (Some(_), _) => RebuildReason::Stale,
            (None, Some(reason)) => reason,
            (None, None) => return Ok((symbols, SymbolLoad::Built)),
        };
        let mut fields = vec![
            ("repositoryId", symbols.repository_id.clone()),
            ("reason", reason.as_str().to_string()),
            ("fileCount", extracted.to_string()),
            ("totalFiles", symbols.files.len().to_string()),
            ("durationMs", started.elapsed().as_millis().to_string()),
        ];
        if let RebuildReason::SchemaMismatch { found } = reason {
            fields.push(("foundSchemaVersion", found.to_string()));
        }
        self.audit_log.record("symbol_index_rebuilt", &fields)?;
        Ok((symbols, SymbolLoad::Rebuilt { reason }))
    }
}

/// Parses a stored index, or says why it cannot be used. The version is read
/// before the shape, so an index from another schema is reported as that
/// rather than as corrupt.
fn stored_index(
    bytes: &[u8],
    repository_id: &str,
) -> std::result::Result<SymbolIndex, RebuildReason> {
    let value: serde_json::Value =
        serde_json::from_slice(bytes).map_err(|_| RebuildReason::Corrupt)?;
    let found = value
        .get("schemaVersion")
        .and_then(serde_json::Value::as_u64)
        .and_then(|version| u32::try_from(version).ok())
        .ok_or(RebuildReason::Corrupt)?;
    if found != SYMBOL_INDEX_SCHEMA_VERSION {
        return Err(RebuildReason::SchemaMismatch { found });
    }
    let stored: SymbolIndex = serde_json::from_value(value).map_err(|_| RebuildReason::Corrupt)?;
    if stored.repository_id != repository_id {
        return Err(RebuildReason::Corrupt);
    }
    Ok(stored)
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

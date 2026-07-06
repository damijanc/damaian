use crate::audit::AuditLog;
use crate::config::{Config, DEFAULT_IGNORE_PATTERNS};
use crate::error::Result;
use crate::hash::{now_millis, sha256};
use crate::ignore::{IgnoreRule, is_ignored_by_rules, parse_ignore_patterns};
use crate::language::{detect_language, extract_imports, extract_symbols};
use crate::secret_scanner::SecretScanner;
use std::collections::HashSet;
use std::fs;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TextChunk {
    pub ordinal: usize,
    pub start: usize,
    pub end: usize,
    pub text: String,
    pub text_hash: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FileRecord {
    pub repository_id: String,
    pub path: String,
    pub language: String,
    pub size: u64,
    pub modified_time_ms: u128,
    pub content_hash: String,
    pub symbols: Vec<String>,
    pub imports: Vec<String>,
    pub terms: Vec<String>,
    pub chunks: Vec<TextChunk>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SkippedFile {
    pub path: String,
    pub reason: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SearchResult {
    pub path: String,
    pub language: String,
    pub symbols: Vec<String>,
    pub imports: Vec<String>,
    pub score: usize,
    pub snippet: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RepositoryIndex {
    pub repository_id: String,
    pub root_path: PathBuf,
    pub indexed_at_ms: u128,
    pub files: Vec<FileRecord>,
    pub skipped: Vec<SkippedFile>,
}

impl RepositoryIndex {
    pub fn keyword_search(&self, query: &str, limit: usize) -> Vec<SearchResult> {
        let query_terms = tokenize(query);
        let mut scored = self
            .files
            .iter()
            .filter_map(|record| {
                let score = score_record(record, &query_terms);
                (score > 0).then(|| SearchResult {
                    path: record.path.clone(),
                    language: record.language.clone(),
                    symbols: record.symbols.clone(),
                    imports: record.imports.clone(),
                    score,
                    snippet: record
                        .chunks
                        .first()
                        .map(|chunk| chunk.text.chars().take(500).collect())
                        .unwrap_or_default(),
                })
            })
            .collect::<Vec<_>>();
        scored.sort_by(|left, right| {
            right
                .score
                .cmp(&left.score)
                .then(left.path.cmp(&right.path))
        });
        scored.truncate(limit);
        scored
    }

    pub fn semantic_search(&self, query: &str, limit: usize) -> Vec<SearchResult> {
        let query_terms = tokenize(query);
        let mut scored = Vec::new();
        for record in &self.files {
            for chunk in &record.chunks {
                let chunk_terms = tokenize(&format!(
                    "{} {} {}",
                    record.path,
                    record.symbols.join(" "),
                    chunk.text
                ));
                let score = query_terms
                    .iter()
                    .filter(|term| chunk_terms.contains(*term))
                    .count();
                if score > 0 {
                    scored.push(SearchResult {
                        path: record.path.clone(),
                        language: record.language.clone(),
                        symbols: record.symbols.clone(),
                        imports: record.imports.clone(),
                        score,
                        snippet: chunk.text.chars().take(500).collect(),
                    });
                }
            }
        }
        scored.sort_by(|left, right| {
            right
                .score
                .cmp(&left.score)
                .then(left.path.cmp(&right.path))
        });
        scored.truncate(limit);
        scored
    }
}

#[derive(Debug, Clone)]
pub struct ProjectIndexer {
    config: Config,
    scanner: SecretScanner,
    audit_log: AuditLog,
}

impl ProjectIndexer {
    pub fn new(config: Config, scanner: SecretScanner, audit_log: AuditLog) -> Self {
        Self {
            config,
            scanner,
            audit_log,
        }
    }

    pub fn index_repository(&self, root_path: impl AsRef<Path>) -> Result<RepositoryIndex> {
        let root = fs::canonicalize(root_path)?;
        let repository_id = repository_id_for_root(&root);
        let mut files = Vec::new();
        let mut skipped = Vec::new();
        let default_patterns = if self.config.ignore_patterns.is_empty() {
            DEFAULT_IGNORE_PATTERNS
                .iter()
                .map(|pattern| pattern.to_string())
                .collect::<Vec<_>>()
        } else {
            self.config.ignore_patterns.clone()
        };
        let rules = parse_ignore_patterns(&default_patterns, "");
        self.walk(
            &root,
            &root,
            "",
            &rules,
            &repository_id,
            &mut files,
            &mut skipped,
        )?;

        let index = RepositoryIndex {
            repository_id,
            root_path: root.clone(),
            indexed_at_ms: now_millis(),
            files,
            skipped,
        };
        self.audit_log.record(
            "repository_indexed",
            &[
                ("actor", "system".to_string()),
                ("repositoryId", index.repository_id.clone()),
                ("resourcePath", root.to_string_lossy().to_string()),
                ("status", "complete".to_string()),
                ("fileCount", index.files.len().to_string()),
                ("skippedCount", index.skipped.len().to_string()),
            ],
        )?;
        Ok(index)
    }

    pub fn repository_id_for_path(&self, root_path: impl AsRef<Path>) -> Result<String> {
        let root = fs::canonicalize(root_path)?;
        Ok(repository_id_for_root(&root))
    }

    fn walk(
        &self,
        root: &Path,
        directory: &Path,
        relative_directory: &str,
        inherited_rules: &[IgnoreRule],
        repository_id: &str,
        files: &mut Vec<FileRecord>,
        skipped: &mut Vec<SkippedFile>,
    ) -> Result<()> {
        let mut rules = inherited_rules.to_vec();
        let gitignore_path = directory.join(".gitignore");
        if let Ok(content) = fs::read_to_string(gitignore_path) {
            let patterns = content
                .lines()
                .map(|line| line.to_string())
                .collect::<Vec<_>>();
            rules.extend(parse_ignore_patterns(&patterns, relative_directory));
        }

        let mut entries = fs::read_dir(directory)?.collect::<std::result::Result<Vec<_>, _>>()?;
        entries.sort_by_key(|entry| entry.file_name());

        for entry in entries {
            let file_name = entry.file_name().to_string_lossy().to_string();
            let relative_path = if relative_directory.is_empty() {
                file_name
            } else {
                format!(
                    "{relative_directory}/{}",
                    entry.file_name().to_string_lossy()
                )
            };
            let file_type = entry.file_type()?;
            let is_directory = file_type.is_dir();

            if is_ignored_by_rules(&rules, &relative_path, is_directory) {
                skipped.push(SkippedFile {
                    path: relative_path,
                    reason: "ignored".to_string(),
                });
                continue;
            }

            if file_type.is_symlink() {
                let resolved = fs::canonicalize(entry.path())?;
                if !resolved.starts_with(root) {
                    skipped.push(SkippedFile {
                        path: relative_path,
                        reason: "symlink_outside_root".to_string(),
                    });
                    continue;
                }
            }

            if is_directory {
                self.walk(
                    root,
                    &entry.path(),
                    &relative_path,
                    &rules,
                    repository_id,
                    files,
                    skipped,
                )?;
                continue;
            }

            if !file_type.is_file() {
                skipped.push(SkippedFile {
                    path: relative_path,
                    reason: "not_regular_file".to_string(),
                });
                continue;
            }

            self.add_file(repository_id, &entry.path(), &relative_path, files, skipped)?;
        }

        Ok(())
    }

    fn add_file(
        &self,
        repository_id: &str,
        absolute_path: &Path,
        relative_path: &str,
        files: &mut Vec<FileRecord>,
        skipped: &mut Vec<SkippedFile>,
    ) -> Result<()> {
        let metadata = fs::metadata(absolute_path)?;
        if metadata.len() > self.config.max_file_bytes {
            skipped.push(SkippedFile {
                path: relative_path.to_string(),
                reason: "too_large".to_string(),
            });
            return Ok(());
        }

        let bytes = fs::read(absolute_path)?;
        if bytes.iter().take(8000).any(|byte| *byte == 0) {
            skipped.push(SkippedFile {
                path: relative_path.to_string(),
                reason: "binary".to_string(),
            });
            return Ok(());
        }

        let content = String::from_utf8_lossy(&bytes).to_string();
        let secret_findings = self.scanner.scan(&content);
        if !secret_findings.is_empty() {
            skipped.push(SkippedFile {
                path: relative_path.to_string(),
                reason: "contains_secret".to_string(),
            });
            return Ok(());
        }

        let language = detect_language(relative_path).to_string();
        let symbols = extract_symbols(&content, &language);
        let imports = extract_imports(&content, &language);
        let modified_time_ms = metadata
            .modified()
            .ok()
            .and_then(|time| time.duration_since(std::time::UNIX_EPOCH).ok())
            .map(|duration| duration.as_millis())
            .unwrap_or_default();

        files.push(FileRecord {
            repository_id: repository_id.to_string(),
            path: relative_path.to_string(),
            language,
            size: metadata.len(),
            modified_time_ms,
            content_hash: sha256(&bytes),
            symbols,
            imports,
            terms: tokenize(&format!("{relative_path} {content}")),
            chunks: chunk_text(&content, 2400),
        });
        Ok(())
    }
}

fn repository_id_for_root(root: &Path) -> String {
    let digest = sha256(root.to_string_lossy().as_bytes());
    format!("repo_{}", &digest[..16])
}

fn tokenize(text: &str) -> Vec<String> {
    let mut terms = HashSet::new();
    let mut current = String::new();
    for character in text.chars().flat_map(|character| character.to_lowercase()) {
        if character.is_ascii_alphanumeric() || matches!(character, '_' | '.' | '$' | '/' | '-') {
            current.push(character);
        } else if !current.is_empty() {
            terms.insert(std::mem::take(&mut current));
        }
    }
    if !current.is_empty() {
        terms.insert(current);
    }
    let mut terms = terms.into_iter().collect::<Vec<_>>();
    terms.sort();
    terms
}

fn chunk_text(content: &str, max_chunk_chars: usize) -> Vec<TextChunk> {
    if content.is_empty() {
        return Vec::new();
    }
    let mut chunks = Vec::new();
    let mut start = 0;
    while start < content.len() {
        let mut end = (start + max_chunk_chars).min(content.len());
        while !content.is_char_boundary(end) {
            end -= 1;
        }
        let text = content[start..end].to_string();
        chunks.push(TextChunk {
            ordinal: chunks.len(),
            start,
            end,
            text_hash: sha256(text.as_bytes()),
            text,
        });
        start = end;
    }
    chunks
}

fn score_record(record: &FileRecord, query_terms: &[String]) -> usize {
    let searchable = format!(
        "{} {} {} {}",
        record.path,
        record.language,
        record.symbols.join(" "),
        record.imports.join(" ")
    )
    .to_ascii_lowercase();
    let mut score = 0;
    for term in query_terms {
        if record.path.to_ascii_lowercase().contains(term) {
            score += 5;
        }
        if record
            .symbols
            .iter()
            .any(|symbol| symbol.to_ascii_lowercase().contains(term))
        {
            score += 4;
        }
        if searchable.contains(term) || record.terms.contains(term) {
            score += 2;
        }
    }
    score
}

use crate::audit::AuditLog;
use crate::config::{Config, DEFAULT_IGNORE_PATTERNS};
use crate::error::Result;
use crate::file_access::LineRange;
use crate::hash::{now_millis, repository_id_for_root, sha256};
use crate::ignore::{is_ignored_by_rules, parse_ignore_patterns};
use crate::language::{
    Declaration, detect_language, extract_declarations, extract_imports, extract_symbols,
};
use crate::secret_scanner::SecretScanner;
use std::collections::HashSet;
use std::fs;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TextChunk {
    pub ordinal: usize,
    pub start: usize,
    pub end: usize,
    /// The lines the chunk spans, 1-based and inclusive, counted in the
    /// redacted text it was cut from. A multi-line secret collapsed by
    /// redaction shifts the lines after it, so a range can land a few lines
    /// early in such a file; context assembly reads it from the file as it is
    /// now, which already tolerates a file edited since indexing.
    pub lines: LineRange,
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
    /// Extracted from the original content, not the redacted text, so a
    /// multi-line secret collapsed by redaction cannot shift the lines after
    /// it. A declaration whose name or container the scanner flags is left
    /// out, so nothing unscanned is kept (spec 25 `context.md` §4.2).
    pub declarations: Vec<Declaration>,
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
    /// The lines of the chunk `snippet` was taken from, so a hit can be
    /// collected as that range rather than as the whole file. `None` only for
    /// a file with no content, which has no chunk.
    pub lines: Option<LineRange>,
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
        self.keyword_search_allowed(query, limit, &|_| true)
    }

    /// [`Self::keyword_search`] over files `allowed` admits.
    ///
    /// The filter runs before scoring and before the cap, so a hit outside the
    /// caller's selection cannot consume one of the returned slots.
    pub fn keyword_search_allowed(
        &self,
        query: &str,
        limit: usize,
        allowed: &dyn Fn(&str) -> bool,
    ) -> Vec<SearchResult> {
        let query_terms = search_terms(query);
        let demote_tests = !query_mentions_tests(&query_terms);
        let mut scored = self
            .files
            .iter()
            .filter(|record| allowed(&record.path))
            .filter_map(|record| {
                let score = demoted(
                    score_record(record, &query_terms),
                    demote_tests && is_test_path(&record.path),
                );
                (score > 0).then(|| {
                    let chunk = best_keyword_chunk(record, &query_terms);
                    SearchResult {
                        path: record.path.clone(),
                        language: record.language.clone(),
                        symbols: record.symbols.clone(),
                        imports: record.imports.clone(),
                        score,
                        snippet: chunk
                            .map(|chunk| chunk.text.chars().take(500).collect())
                            .unwrap_or_default(),
                        lines: chunk.map(|chunk| chunk.lines),
                    }
                })
            })
            .collect::<Vec<_>>();
        sort_results(&mut scored, demote_tests);
        scored.truncate(limit);
        scored
    }

    pub fn semantic_search(&self, query: &str, limit: usize) -> Vec<SearchResult> {
        self.semantic_search_allowed(query, limit, &|_| true)
    }

    /// [`Self::semantic_search`] over files `allowed` admits.
    ///
    /// As with [`Self::keyword_search_allowed`], the filter runs before the
    /// cap, so outside hits cannot crowd out admitted ones.
    pub fn semantic_search_allowed(
        &self,
        query: &str,
        limit: usize,
        allowed: &dyn Fn(&str) -> bool,
    ) -> Vec<SearchResult> {
        let query_terms = search_terms(query);
        let demote_tests = !query_mentions_tests(&query_terms);
        let mut scored = Vec::new();
        for record in self.files.iter().filter(|record| allowed(&record.path)) {
            let demote = demote_tests && is_test_path(&record.path);
            let file_terms = tokenize(&format!("{} {}", record.path, record.symbols.join(" ")));
            for chunk in &record.chunks {
                let text_terms = tokenize(&chunk.text);
                // A chunk is a hit only if its own text holds a term. The path
                // and symbols are the whole file's, so counting them alone made
                // every chunk of a matching file tie, and the hits were its
                // first chunks rather than the one that matched. They still
                // raise the score of a chunk that does match.
                if !query_terms.iter().any(|term| text_terms.contains(term)) {
                    continue;
                }
                let score = demoted(
                    query_terms
                        .iter()
                        .filter(|term| text_terms.contains(*term) || file_terms.contains(*term))
                        .count(),
                    demote,
                );
                scored.push(SearchResult {
                    path: record.path.clone(),
                    language: record.language.clone(),
                    symbols: record.symbols.clone(),
                    imports: record.imports.clone(),
                    score,
                    snippet: chunk.text.chars().take(500).collect(),
                    lines: Some(chunk.lines),
                });
            }
        }
        sort_results(&mut scored, demote_tests);
        scored.truncate(limit);
        scored
    }
}

/// Words that say nothing about which file is meant. Every large file holds
/// them, and test functions are named as sentences
/// (`a_command_the_stop_ended_is_named_…`), so as query terms they matched
/// symbols in nearly every test file and decided the ranking: a question about
/// user approval returned nine test files in its top ten. Search queries only;
/// [`tokenize`] itself is shared with memory recall and stays as it is.
const SEARCH_STOP_WORDS: &[&str] = &[
    "a", "about", "an", "and", "are", "as", "at", "be", "by", "can", "do", "does", "for", "from",
    "how", "i", "if", "in", "is", "it", "its", "me", "my", "of", "on", "or", "our", "so", "that",
    "the", "their", "there", "this", "to", "we", "what", "when", "where", "which", "who", "why",
    "with", "you", "your",
];

/// A query's terms for search: [`tokenize`] without [`SEARCH_STOP_WORDS`].
/// A query made only of stop-words keeps them, so it still searches for
/// something rather than for nothing.
pub(crate) fn search_terms(query: &str) -> Vec<String> {
    let terms = tokenize(query);
    let meaningful = terms
        .iter()
        .filter(|term| !SEARCH_STOP_WORDS.contains(&term.as_str()))
        .cloned()
        .collect::<Vec<_>>();
    if meaningful.is_empty() {
        terms
    } else {
        meaningful
    }
}

/// Whether the query asks about tests, in which case test files are not
/// demoted: "where is the approval test" should still find it first.
pub(crate) fn query_mentions_tests(query_terms: &[String]) -> bool {
    query_terms.iter().any(|term| {
        matches!(
            term.as_str(),
            "test" | "tests" | "testing" | "spec" | "specs"
        )
    })
}

/// Whether a repository path is a test file by the common conventions: a
/// `tests`, `test`, `__tests__` or `spec` directory, a Rust `tests.rs` or
/// `*_tests.rs`, and `*_test.*`, `test_*.*`, `*.test.*` or `*.spec.*` names.
pub(crate) fn is_test_path(path: &str) -> bool {
    let path = path.to_ascii_lowercase();
    let mut segments = path.split('/').collect::<Vec<_>>();
    let file = segments.pop().unwrap_or_default();
    if segments
        .iter()
        .any(|segment| matches!(*segment, "tests" | "test" | "__tests__" | "spec"))
    {
        return true;
    }
    let stem = file.split('.').next().unwrap_or_default();
    stem == "tests"
        || stem.ends_with("_tests")
        || stem.ends_with("_test")
        || stem.starts_with("test_")
        || file.contains(".test.")
        || file.contains(".spec.")
}

/// A test file's score is halved, rounding up so a hit never vanishes,
/// unless the query is about tests. Test files are large and cover every
/// behaviour, so on equal footing they crowd out the code they test.
pub(crate) fn demoted(score: usize, demote: bool) -> usize {
    if demote { score.div_ceil(2) } else { score }
}

/// Highest score first; on a tie a non-test file before a test file when
/// tests are demoted, then by path, so the order is total.
fn sort_results(results: &mut [SearchResult], demote_tests: bool) {
    results.sort_by(|left, right| {
        right
            .score
            .cmp(&left.score)
            .then_with(|| {
                if demote_tests {
                    is_test_path(&left.path).cmp(&is_test_path(&right.path))
                } else {
                    std::cmp::Ordering::Equal
                }
            })
            .then(left.path.cmp(&right.path))
    });
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

    /// The configuration this indexer was built with, so callers that own an
    /// indexer rather than a `Config` (such as `IndexCache`) can read it.
    pub fn config(&self) -> &Config {
        &self.config
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
        // The traversal itself lives in `tree_walk` so the navigation tools of
        // spec 47 share it rather than reimplementing the ignore and
        // symlink-escape rules. This visitor is the index-building half that
        // stayed behind.
        crate::tree_walk::walk(&root, &root, "", &rules, &mut |event| match event {
            crate::tree_walk::WalkEvent::Skipped(skip) => {
                skipped.push(SkippedFile {
                    path: skip.path.clone(),
                    reason: skip.reason.clone(),
                });
                Ok(())
            }
            crate::tree_walk::WalkEvent::File(file) => self.add_file(
                &repository_id,
                &file.absolute_path,
                &file.relative_path,
                &mut files,
                &mut skipped,
            ),
        })?;

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

    /// Re-indexes a single file, for incremental updates driven by a
    /// filesystem watcher. Returns `Ok(None)` if the path is no longer a
    /// regular file (deleted, replaced by a directory) or is ignored, in
    /// which case the caller should drop any existing record for it.
    ///
    /// Unlike `index_repository`'s full walk, this only checks the default
    /// and configured ignore patterns — not per-directory `.gitignore` files
    /// — to stay cheap on the hot path; the periodic full rescan in
    /// `IndexCache` corrects any drift this simplification causes.
    pub fn index_single_file(
        &self,
        repository_id: &str,
        root: &Path,
        relative_path: &str,
    ) -> Result<Option<FileRecord>> {
        let absolute_path = root.join(relative_path);
        if !absolute_path.is_file() {
            return Ok(None);
        }
        let default_patterns = if self.config.ignore_patterns.is_empty() {
            DEFAULT_IGNORE_PATTERNS
                .iter()
                .map(|pattern| pattern.to_string())
                .collect::<Vec<_>>()
        } else {
            self.config.ignore_patterns.clone()
        };
        let rules = parse_ignore_patterns(&default_patterns, "");
        if is_ignored_by_rules(&rules, relative_path, false) {
            return Ok(None);
        }

        let mut files = Vec::new();
        let mut skipped = Vec::new();
        self.add_file(
            repository_id,
            &absolute_path,
            relative_path,
            &mut files,
            &mut skipped,
        )?;
        Ok(files.into_iter().next())
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
        let redacted = self.scanner.redact(&content);
        let indexed_content = redacted.text;

        let language = detect_language(relative_path).to_string();
        let symbols = extract_symbols(&indexed_content, &language);
        let declarations = extract_declarations(&content, &language)
            .into_iter()
            .filter(|declaration| {
                !self.scanner.contains_secrets(&declaration.name)
                    && !declaration
                        .container
                        .as_deref()
                        .is_some_and(|container| self.scanner.contains_secrets(container))
            })
            .collect();
        let imports = extract_imports(&indexed_content, &language);
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
            declarations,
            imports,
            terms: tokenize(&format!("{relative_path} {indexed_content}")),
            chunks: chunk_text(&indexed_content, 2400),
        });
        Ok(())
    }
}

/// The index's term splitter, shared with memory recall (spec 30 C1).
pub(crate) fn tokenize(text: &str) -> Vec<String> {
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
    let mut start_line = 1;
    while start < content.len() {
        let mut end = (start + max_chunk_chars).min(content.len());
        while !content.is_char_boundary(end) {
            end -= 1;
        }
        let text = content[start..end].to_string();
        let newlines = text.matches('\n').count();
        // A chunk ending on its newline does not reach into the next line.
        let end_line = if text.ends_with('\n') {
            start_line + newlines - 1
        } else {
            start_line + newlines
        };
        chunks.push(TextChunk {
            ordinal: chunks.len(),
            start,
            end,
            lines: LineRange {
                start: start_line,
                end: end_line,
            },
            text_hash: sha256(text.as_bytes()),
            text,
        });
        start_line += newlines;
        start = end;
    }
    chunks
}

/// The chunk a keyword hit points at: the one holding the most query terms,
/// the earliest on a tie. A file scored only on its path or symbols has no
/// chunk that holds a term, and gets its first chunk, as the snippet always did.
fn best_keyword_chunk<'a>(record: &'a FileRecord, query_terms: &[String]) -> Option<&'a TextChunk> {
    let mut best: Option<(usize, &TextChunk)> = None;
    for chunk in &record.chunks {
        let chunk_terms = tokenize(&chunk.text);
        let matched = query_terms
            .iter()
            .filter(|term| chunk_terms.contains(*term))
            .count();
        if matched > 0 && best.is_none_or(|(most, _)| matched > most) {
            best = Some((matched, chunk));
        }
    }
    best.map(|(_, chunk)| chunk)
        .or_else(|| record.chunks.first())
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

#[cfg(test)]
mod tests {
    use super::*;

    fn record(path: &str, symbols: &[&str], text: &str) -> FileRecord {
        FileRecord {
            repository_id: "repo_test".to_string(),
            path: path.to_string(),
            language: "rust".to_string(),
            size: text.len() as u64,
            modified_time_ms: 0,
            content_hash: String::new(),
            symbols: symbols.iter().map(|symbol| symbol.to_string()).collect(),
            declarations: Vec::new(),
            imports: Vec::new(),
            terms: tokenize(&format!("{path} {text}")),
            chunks: chunk_text(text, 2400),
        }
    }

    fn index(files: Vec<FileRecord>) -> RepositoryIndex {
        RepositoryIndex {
            repository_id: "repo_test".to_string(),
            root_path: PathBuf::from("/repo"),
            indexed_at_ms: 0,
            files,
            skipped: Vec::new(),
        }
    }

    #[test]
    fn chunks_carry_the_lines_they_span() {
        let text = (1..=300)
            .map(|line| format!("line number {line:04} of the fixture\n"))
            .collect::<String>();
        let chunks = chunk_text(&text, 2400);
        assert!(chunks.len() > 1);
        assert_eq!(chunks[0].lines.start, 1);
        for pair in chunks.windows(2) {
            // Consecutive chunks share or adjoin a line, so adjacent hits merge.
            assert!(pair[1].lines.start <= pair[0].lines.end + 1, "{pair:?}");
        }
        assert_eq!(chunks.last().unwrap().lines.end, 300);
        for chunk in &chunks {
            let first = chunk.text.lines().next().unwrap();
            let line = text.lines().nth(chunk.lines.start - 1).unwrap();
            assert!(
                line.ends_with(first),
                "{first:?} is not on line {}",
                chunk.lines.start
            );
        }
    }

    #[test]
    fn stop_words_are_not_search_terms_unless_they_are_all_there_is() {
        assert_eq!(
            search_terms("what part of the code is responsible for user approval"),
            ["approval", "code", "part", "responsible", "user"]
        );
        assert_eq!(search_terms("what is the"), ["is", "the", "what"]);
    }

    #[test]
    fn test_paths_are_recognised_by_common_conventions() {
        for path in [
            "crates/workspace-engine/tests/foundation.rs",
            "crates/desktop-shell/src/tests.rs",
            "crates/desktop-shell/src/responsive_tests.rs",
            "crates/desktop-shell/src/tests/worker_turns.rs",
            "pkg/upload_test.go",
            "test_upload.py",
            "web/__tests__/upload.js",
            "web/upload.test.ts",
            "web/upload.spec.ts",
            "spec/upload_spec.rb",
        ] {
            assert!(is_test_path(path), "{path}");
        }
        for path in [
            "crates/workspace-engine/src/command_policy.rs",
            "src/contest.rs",
            "src/attestation.rs",
            "docs/specs/26_context_assembly/proposal.md",
            "src/testing_utils_inner/latest.rs",
        ] {
            assert!(!is_test_path(path), "{path}");
        }
    }

    /// The defect this demotion fixes: test functions named as sentences
    /// matched every stop-word, so a question about approval returned the
    /// test files ahead of the code they test.
    #[test]
    fn source_outranks_sentence_named_tests_unless_the_query_is_about_tests() {
        let files = vec![
            record(
                "src/tests.rs",
                &[
                    "the_approval_is_what_the_user_gave_for_the_command",
                    "an_approval_for_the_user_is_part_of_the_code",
                ],
                "fn the_approval_is_what_the_user_gave_for_the_command() { approval(); }\n",
            ),
            record(
                "src/approval.rs",
                &["ApprovalDecision", "record_approval"],
                "pub fn record_approval(user: &User) -> ApprovalDecision { todo!() }\n",
            ),
        ];
        let index = index(files);

        let question = index.keyword_search("what part of the code is the user approval", 8);
        assert_eq!(question[0].path, "src/approval.rs", "{question:?}");
        let question = index.semantic_search("what part of the code is the user approval", 8);
        assert_eq!(question[0].path, "src/approval.rs", "{question:?}");

        let about_tests = index.keyword_search("approval tests", 8);
        assert_eq!(about_tests[0].path, "src/tests.rs", "{about_tests:?}");
    }
}

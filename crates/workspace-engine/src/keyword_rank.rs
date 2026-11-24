//! Two-stage keyword ranking (spec 02 §8, "Two-stage ranking").
//!
//! The ranking this replaced counted, per file, how many query terms occurred
//! anywhere in it. A 4,000-line file holds nearly every word, so it matched
//! nearly every term and size decided the order. Two signals separate a file
//! that is *about* the query from one that merely mentions its words:
//!
//! 1. **Rarity.** Each term is weighted by how few files contain it (BM25's
//!    inverse document frequency), so a match on `protected` counts for more
//!    than a match on `user`.
//! 2. **Proximity.** The terms of a relevant file occur near each other. Stage
//!    two slides a window of [`WINDOW_LINES`] lines over each candidate one
//!    matching line at a time, so no fixed boundary can split two terms that
//!    sit close together, as cutting at chunk boundaries would.
//!
//! Stage one scores every allowed file on rarity alone, in one pass over each
//! file's sorted term list, and keeps the best [`CANDIDATES`]. Stage two reads
//! only those candidates' text, and finds the query terms in it by substring
//! search rather than by tokenizing it. Nothing is precomputed across files,
//! so the index the watcher updates file by file never has a second structure
//! to fall out of step with.

use crate::file_access::LineRange;
use crate::indexer::{
    FileRecord, RepositoryIndex, SearchResult, is_test_path, query_mentions_tests, search_terms,
};
use std::collections::HashMap;

/// Files stage one hands to stage two. Enough that a relevant file ranked low
/// on rarity alone still gets its proximity pass, few enough that stage two's
/// cost does not grow with the repository.
const CANDIDATES: usize = 50;

/// The proximity window, in lines. About one index chunk of code, and a
/// typical function.
const WINDOW_LINES: usize = 60;

/// Lines of context added on each side of the matched lines in the range a hit
/// reports, so the model sees the code around a match, not only the match.
const RANGE_PADDING: usize = 8;

/// How far above a window's first match the range may reach back to the
/// declaration that encloses it.
const DECLARATION_REACH: usize = 40;

/// Stage two reads at most this much candidate text in total, in stage one's
/// order. A candidate past it keeps its stage-one score, so a handful of very
/// large files (generated bindings run to megabytes) cannot make one search
/// slow.
const STAGE_TWO_BYTES: usize = 8 * 1024 * 1024;

/// At most this many query terms are weighed: stage one records which terms a
/// file holds as bits of a `u64`. A query is a sentence, not a document.
const MAX_TERMS: usize = 64;

/// BM25's inverse document frequency: rare terms weigh more, and a term in
/// every file still weighs a little rather than nothing.
fn idf(document_frequency: usize, total: usize) -> f64 {
    let df = document_frequency as f64;
    (((total as f64 - df) + 0.5) / (df + 0.5) + 1.0).ln()
}

/// One query term with its rarity weight.
struct Term {
    text: String,
    weight: f64,
}

/// The query's terms and its adjacent pairs. Weights are set by
/// [`Candidates::new`], which counts document frequency in the same pass that
/// scores the files.
struct Query {
    terms: Vec<Term>,
    /// Pairs of term indices adjacent in the query, for the phrase bonus:
    /// "deep water" as written outranks "deep" and "water" 30 lines apart.
    pairs: Vec<(usize, usize)>,
}

impl Query {
    fn new(query: &str) -> Query {
        let mut texts = search_terms(query);
        texts.truncate(MAX_TERMS);
        let by_text = texts
            .iter()
            .enumerate()
            .map(|(index, text)| (text.clone(), index))
            .collect::<HashMap<_, _>>();
        let ordered = tokens(query)
            .filter_map(|token| by_text.get(&token.to_lowercase()).copied())
            .collect::<Vec<_>>();
        let mut pairs = Vec::new();
        for pair in ordered.windows(2) {
            if pair[0] != pair[1] && !pairs.contains(&(pair[0], pair[1])) {
                pairs.push((pair[0], pair[1]));
            }
        }
        Query {
            terms: texts
                .into_iter()
                .map(|text| Term { text, weight: 0.0 })
                .collect(),
            pairs,
        }
    }

    fn weight_of(&self, present: u64) -> f64 {
        self.terms
            .iter()
            .enumerate()
            .filter(|(index, _)| present & (1 << index) != 0)
            .map(|(_, term)| term.weight)
            .sum()
    }
}

/// The tokens of `text` in order, with their original case, split where
/// [`crate::indexer::tokenize`] splits.
fn tokens(text: &str) -> impl Iterator<Item = &str> {
    text.split(|character: char| {
        !(character.is_ascii_alphanumeric() || matches!(character, '_' | '.' | '$' | '/' | '-'))
    })
    .filter(|token| !token.is_empty())
}

/// An identifier's parts, lowercased: split at `_ . / - $` and at camelCase
/// boundaries, so `record_approval` and `ApprovalDecision` both hold
/// `approval`. Without this a query word never matched the identifiers code
/// is written in, only the prose around them.
fn parts(token: &str) -> Vec<String> {
    let mut parts = Vec::new();
    for piece in token.split(['_', '.', '/', '-', '$']) {
        let characters = piece.chars().collect::<Vec<_>>();
        let mut start = 0;
        for index in 1..characters.len() {
            let (before, here) = (characters[index - 1], characters[index]);
            let next_is_lower = characters
                .get(index + 1)
                .is_some_and(|next| next.is_ascii_lowercase());
            // `recordApproval` splits before `A`; `HTTPServer` before `Se`.
            if (before.is_ascii_lowercase() && here.is_ascii_uppercase())
                || (before.is_ascii_uppercase() && here.is_ascii_uppercase() && next_is_lower)
            {
                parts.push(characters[start..index].iter().collect::<String>());
                start = index;
            }
        }
        parts.push(characters[start..].iter().collect::<String>());
    }
    parts
        .into_iter()
        .filter(|part| !part.is_empty())
        .map(|part| part.to_ascii_lowercase())
        .collect()
}

/// What a token matches: itself, lowercased, and its parts when it has more
/// than one.
fn token_terms(token: &str) -> Vec<String> {
    let whole = token.to_lowercase();
    let mut terms = parts(token);
    if terms.len() <= 1 {
        return vec![whole];
    }
    terms.push(whole);
    terms
}

/// A file's searchable terms for the index: every token and every part of
/// every identifier, sorted and unique, so stage one can binary-search it.
pub(crate) fn index_terms(text: &str) -> Vec<String> {
    let mut terms = tokens(text).flat_map(token_terms).collect::<Vec<_>>();
    terms.sort();
    terms.dedup();
    terms
}

/// Whether `term` is among `sorted` name parts ([`index_terms`] of a path or
/// of symbol names): equal to one, or, from five characters, the start of one
/// ("approval" names `approvals`). As a bare substring "is" matched
/// `CommandRisk`. Binary searches, because a query visits every file.
fn names(sorted: &[String], term: &str) -> bool {
    let at = sorted.partition_point(|part| part.as_str() < term);
    sorted
        .get(at)
        .is_some_and(|part| part == term || (term.len() >= 5 && part.starts_with(term)))
}

/// How strongly a file's name and symbols point at `term`: a path that names
/// it is strong evidence of what the file is for, a symbol less so.
fn naming_weight(file: &FileRecord, term: &Term) -> f64 {
    if names(&file.name_terms, &term.text) {
        term.weight * 1.5
    } else if names(&file.symbol_terms, &term.text) {
        term.weight * 0.5
    } else {
        0.0
    }
}

fn naming(file: &FileRecord, query: &Query) -> f64 {
    query
        .terms
        .iter()
        .map(|term| naming_weight(file, term))
        .sum()
}

/// Where the query's terms occur in one file's text, found by substring
/// search over a lowercased copy and checked against word boundaries in the
/// original: `approval` is found in `record_approval` and `ApprovalDecision`
/// but not inside `disapproval`, whose `a` follows a letter. Tokenizing the
/// text instead allocated per word, hundreds of milliseconds per query on
/// generated bindings several megabytes long.
struct Hits {
    /// (line, term), sorted by line.
    terms: Vec<(usize, usize)>,
    /// (line, pair), sorted by line: an adjacent query pair found adjacent.
    phrases: Vec<(usize, usize)>,
    /// Byte offset of each line's start.
    line_starts: Vec<usize>,
}

impl Hits {
    fn find(text: &str, query: &Query) -> Hits {
        let bytes = text.as_bytes();
        let lower = text.to_ascii_lowercase();
        let mut line_starts = vec![0];
        line_starts.extend(
            bytes
                .iter()
                .enumerate()
                .filter(|(_, byte)| **byte == b'\n')
                .map(|(index, _)| index + 1)
                .filter(|start| *start < bytes.len()),
        );
        let line_of = |offset: usize| line_starts.partition_point(|start| *start <= offset);

        // Sorted start offsets of each term's occurrences, for phrase lookup.
        let mut starts: Vec<Vec<(usize, usize)>> = Vec::with_capacity(query.terms.len());
        let mut terms = Vec::new();
        for (index, term) in query.terms.iter().enumerate() {
            let length = term.text.len();
            let found = lower
                .match_indices(term.text.as_str())
                .map(|(offset, _)| (offset, offset + length))
                .filter(|&(start, end)| starts_word(bytes, start) && ends_word(bytes, end, length))
                .collect::<Vec<_>>();
            terms.extend(found.iter().map(|&(start, _)| (line_of(start), index)));
            starts.push(found);
        }
        terms.sort_unstable();

        let mut phrases = Vec::new();
        for (pair, &(first, second)) in query.pairs.iter().enumerate() {
            for &(start, end) in &starts[first] {
                // `deep water`, `deep_water`, `deep-water` and `deepWater`.
                let gap = bytes[end..]
                    .iter()
                    .take(3)
                    .take_while(|byte| matches!(byte, b' ' | b'_' | b'-' | b'\t'))
                    .count();
                let next = end + gap;
                if starts[second]
                    .binary_search_by(|&(offset, _)| offset.cmp(&next))
                    .is_ok()
                {
                    phrases.push((line_of(start), pair));
                }
            }
        }
        phrases.sort_unstable();
        Hits {
            terms,
            phrases,
            line_starts,
        }
    }

    fn total_lines(&self) -> usize {
        self.line_starts.len()
    }

    /// The text of `range`, at most 500 characters.
    fn snippet(&self, text: &str, range: LineRange) -> String {
        let start = self.line_starts[range.start - 1];
        let end = self
            .line_starts
            .get(range.end)
            .copied()
            .unwrap_or(text.len());
        text[start..end].trim_end().chars().take(500).collect()
    }
}

/// A match may start at the text's start, after a non-alphanumeric byte (`_`,
/// `.`, space), or at a camelCase boundary: `approval` starts a word in
/// `recordApproval` and `server` in `HTTPServer`.
fn starts_word(bytes: &[u8], start: usize) -> bool {
    let Some(&before) = start.checked_sub(1).and_then(|index| bytes.get(index)) else {
        return true;
    };
    let here = bytes[start];
    !before.is_ascii_alphanumeric()
        || ((before.is_ascii_lowercase() || before.is_ascii_digit()) && here.is_ascii_uppercase())
        || (before.is_ascii_uppercase()
            && here.is_ascii_uppercase()
            && bytes.get(start + 1).is_some_and(u8::is_ascii_lowercase))
}

/// A match may end at the text's end, before a non-alphanumeric byte, or at a
/// camelCase boundary; a term of five characters or more may also end inside
/// a word, as a prefix ("approval" in `approvals`), as [`names`] allows.
fn ends_word(bytes: &[u8], end: usize, length: usize) -> bool {
    let Some(&after) = bytes.get(end) else {
        return true;
    };
    let last = bytes[end - 1];
    length >= 5
        || !after.is_ascii_alphanumeric()
        || (last.is_ascii_lowercase() && after.is_ascii_uppercase())
        || (after.is_ascii_uppercase() && bytes.get(end + 1).is_some_and(u8::is_ascii_lowercase))
}

/// A scored window: its weight and the first and last matching line in it.
#[derive(Clone, Copy)]
struct Window {
    score: f64,
    first: usize,
    last: usize,
}

/// Stage two's result for one file.
struct Proximity {
    /// The best window, plus half the best window that does not overlap it, so
    /// a file relevant in two places outranks one relevant in one.
    score: f64,
    best: Option<Window>,
}

/// Every window of [`WINDOW_LINES`] lines that starts on a matching line,
/// scored by the distinct terms in it, each counted once at its weight, plus
/// the phrase bonus for adjacent query pairs found adjacent in the text.
/// `test_from` is the first line of an inline test module when tests are
/// demoted: a window starting there is halved, as a test file is.
fn proximity(hits: &Hits, query: &Query, test_from: Option<usize>) -> Proximity {
    let occurrences = &hits.terms;
    if occurrences.is_empty() {
        return Proximity {
            score: 0.0,
            best: None,
        };
    }
    let mut counts = vec![0usize; query.terms.len()];
    let mut windows = Vec::with_capacity(occurrences.len());
    let mut end = 0;
    for start in 0..occurrences.len() {
        let start_line = occurrences[start].0;
        while end < occurrences.len() && occurrences[end].0 < start_line + WINDOW_LINES {
            counts[occurrences[end].1] += 1;
            end += 1;
        }
        let last = occurrences[end - 1].0;
        let mut score = counts
            .iter()
            .zip(&query.terms)
            .filter(|(count, _)| **count > 0)
            .map(|(_, term)| term.weight)
            .sum::<f64>();
        let from = hits.phrases.partition_point(|&(line, _)| line < start_line);
        let mut seen = 0u64;
        for &(line, pair) in &hits.phrases[from..] {
            if line > last {
                break;
            }
            if seen & (1 << (pair % 64)) == 0 {
                seen |= 1 << (pair % 64);
                let (first, second) = query.pairs[pair];
                score += (query.terms[first].weight + query.terms[second].weight) * 0.5;
            }
        }
        if test_from.is_some_and(|from| start_line >= from) {
            score *= 0.5;
        }
        windows.push(Window {
            score,
            first: start_line,
            last,
        });
        counts[occurrences[start].1] -= 1;
    }

    let best = windows
        .iter()
        .copied()
        .max_by(|left, right| {
            left.score
                .total_cmp(&right.score)
                // The earliest of equal windows, so the result is stable.
                .then(right.first.cmp(&left.first))
        })
        .expect("a file with hits has a window");
    let second = windows
        .iter()
        .filter(|window| window.last < best.first || window.first > best.last)
        .map(|window| window.score)
        .fold(0.0, f64::max);
    Proximity {
        score: best.score + second * 0.5,
        best: Some(best),
    }
}

/// The line of a Rust inline test module's `#[cfg(test)]`, which by
/// convention closes the file: everything after it is tests.
fn inline_tests_start(file: &FileRecord, text: &str, hits: &Hits) -> Option<usize> {
    if file.language != "rust" {
        return None;
    }
    text.match_indices("#[cfg(test)]")
        .map(|(offset, _)| offset)
        .find(|offset| *offset == 0 || text.as_bytes()[offset - 1] == b'\n')
        .map(|offset| hits.line_starts.partition_point(|start| *start <= offset))
}

/// The range a hit reports: the window's matches, padded, reaching back to an
/// enclosing declaration when one starts shortly above.
fn hit_range(file: &FileRecord, window: Window, total_lines: usize) -> LineRange {
    let mut start = window.first.saturating_sub(RANGE_PADDING).max(1);
    if let Some(declaration) = file
        .declarations
        .iter()
        .map(|declaration| declaration.line as usize)
        .filter(|line| *line <= window.first && window.first - line <= DECLARATION_REACH)
        .max()
    {
        start = start.min(declaration);
    }
    let end = (window.last + RANGE_PADDING).min(total_lines.max(window.last));
    LineRange { start, end }
}

/// The file's indexed text: its chunks are contiguous slices of it.
fn indexed_text(file: &FileRecord) -> String {
    file.chunks
        .iter()
        .map(|chunk| chunk.text.as_str())
        .collect()
}

/// Stage one over the files `allowed` admits: the weighted query, whether
/// test files are demoted, and the best candidates by rarity, highest first.
struct Candidates<'a> {
    query: Query,
    demote_tests: bool,
    files: Vec<(f64, &'a FileRecord)>,
}

impl<'a> Candidates<'a> {
    fn new(
        index: &'a RepositoryIndex,
        query: &str,
        keep: usize,
        allowed: &dyn Fn(&str) -> bool,
    ) -> Candidates<'a> {
        let mut query = Query::new(query);
        let demote_tests = !query_mentions_tests(
            &query
                .terms
                .iter()
                .map(|term| term.text.clone())
                .collect::<Vec<_>>(),
        );
        // One pass: which terms each file's text holds, as bits. Document
        // frequency and the files' scores both come from these.
        let files = index
            .files
            .iter()
            .filter(|file| allowed(&file.path))
            .map(|file| {
                let present = query
                    .terms
                    .iter()
                    .enumerate()
                    .filter(|(_, term)| file.terms.binary_search(&term.text).is_ok())
                    .fold(0u64, |bits, (index, _)| bits | (1 << index));
                (present, file)
            })
            .collect::<Vec<_>>();
        for (index, term) in query.terms.iter_mut().enumerate() {
            let df = files
                .iter()
                .filter(|(present, _)| present & (1 << index) != 0)
                .count();
            term.weight = idf(df, files.len());
        }
        let mut candidates = Candidates {
            query,
            demote_tests,
            files: Vec::new(),
        };
        let mut scored = files
            .into_iter()
            .map(|(present, file)| {
                let score = candidates.query.weight_of(present) + naming(file, &candidates.query);
                (candidates.demote(file, score), file)
            })
            .filter(|(score, _)| *score > 0.0)
            .collect::<Vec<_>>();
        scored.sort_by(|left, right| {
            right
                .0
                .total_cmp(&left.0)
                .then(left.1.path.cmp(&right.1.path))
        });
        scored.truncate(keep);
        candidates.files = scored;
        candidates
    }

    /// A test file's score is halved unless the query is about tests.
    fn demote(&self, file: &FileRecord, score: f64) -> f64 {
        if self.demote_tests && is_test_path(&file.path) {
            score * 0.5
        } else {
            score
        }
    }

    /// The candidates in stage one's order, with their stage-one score and
    /// whether their text still fits [`STAGE_TWO_BYTES`].
    fn within_budget(&self) -> Vec<(f64, &'a FileRecord, bool)> {
        let mut read = 0usize;
        self.files
            .iter()
            .map(|&(score, file)| {
                let size: usize = file.chunks.iter().map(|chunk| chunk.text.len()).sum();
                let fits = read + size <= STAGE_TWO_BYTES;
                if fits {
                    read += size;
                }
                (score, file, fits)
            })
            .collect()
    }
}

/// Highest score first; on a tie a non-test file first when tests are
/// demoted, then by path, so the order is total.
fn sort_and_cap(results: &mut Vec<SearchResult>, demote_tests: bool, limit: usize) {
    results.sort_by(|left, right| {
        right.score.cmp(&left.score).then_with(|| {
            if demote_tests {
                is_test_path(&left.path).cmp(&is_test_path(&right.path))
            } else {
                std::cmp::Ordering::Equal
            }
            .then(left.path.cmp(&right.path))
        })
    });
    results.truncate(limit);
}

/// Hundredths, so the integer score keeps the ranking's order.
fn as_score(score: f64) -> usize {
    (score * 100.0).round() as usize
}

fn result(
    file: &FileRecord,
    score: f64,
    snippet: String,
    lines: Option<LineRange>,
) -> SearchResult {
    SearchResult {
        path: file.path.clone(),
        language: file.language.clone(),
        symbols: file.symbols.clone(),
        imports: file.imports.clone(),
        score: as_score(score),
        snippet,
        lines,
    }
}

/// The term-overlap stand-in for semantic search, used when embeddings are
/// off: each chunk of stage one's candidates scored by the rarity of the query
/// terms its own lines hold, identifier parts included. A chunk whose text
/// holds no term is not a hit, however well its file's name matches. It used
/// to tokenize every chunk of the repository on every query, about 9 s on a
/// 41,000-file corpus (spec 02 §8).
pub(crate) fn rank_chunks(
    index: &RepositoryIndex,
    query: &str,
    limit: usize,
    allowed: &dyn Fn(&str) -> bool,
) -> Vec<SearchResult> {
    let candidates = Candidates::new(index, query, CANDIDATES.max(limit), allowed);
    let query = &candidates.query;
    let mut results = Vec::new();
    for (_, file, fits) in candidates.within_budget() {
        if !fits {
            continue;
        }
        let hits = Hits::find(&indexed_text(file), query);
        let named = naming(file, query) * 0.5;
        for chunk in &file.chunks {
            let from = hits
                .terms
                .partition_point(|&(line, _)| line < chunk.lines.start);
            let present = hits.terms[from..]
                .iter()
                .take_while(|&&(line, _)| line <= chunk.lines.end)
                .fold(0u64, |bits, &(_, term)| bits | (1 << term));
            if present == 0 {
                continue;
            }
            let score = candidates.demote(file, query.weight_of(present) + named);
            results.push(result(
                file,
                score,
                chunk.text.chars().take(500).collect(),
                Some(chunk.lines),
            ));
        }
    }
    sort_and_cap(&mut results, candidates.demote_tests, limit);
    results
}

/// [`RepositoryIndex::keyword_search_allowed`]'s ranking.
pub(crate) fn rank(
    index: &RepositoryIndex,
    query: &str,
    limit: usize,
    allowed: &dyn Fn(&str) -> bool,
) -> Vec<SearchResult> {
    let candidates = Candidates::new(index, query, CANDIDATES.max(limit), allowed);
    let query = &candidates.query;
    let mut results = candidates
        .within_budget()
        .into_iter()
        .map(|(rarity, file, fits)| {
            if !fits {
                let first = file.chunks.first();
                return result(
                    file,
                    rarity,
                    first
                        .map(|chunk| chunk.text.chars().take(500).collect())
                        .unwrap_or_default(),
                    first.map(|chunk| chunk.lines),
                );
            }
            let text = indexed_text(file);
            let hits = Hits::find(&text, query);
            let test_from = candidates
                .demote_tests
                .then(|| inline_tests_start(file, &text, &hits))
                .flatten();
            let found = proximity(&hits, query, test_from);
            let range = found
                .best
                .map(|window| hit_range(file, window, hits.total_lines()));
            let snippet = range
                .map(|range| hits.snippet(&text, range))
                .unwrap_or_else(|| text.chars().take(500).collect());
            // The path and symbols still count: a file named for the query is
            // evidence about what it is for, beyond any one window.
            let score = candidates.demote(file, found.score + naming(file, query));
            result(
                file,
                score,
                snippet,
                range.or_else(|| file.chunks.first().map(|chunk| chunk.lines)),
            )
        })
        .filter(|result| result.score > 0)
        .collect::<Vec<_>>();
    sort_and_cap(&mut results, candidates.demote_tests, limit);
    results
}

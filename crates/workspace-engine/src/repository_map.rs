//! The repository map (spec 24): where a repository's project roots are,
//! and what each one holds.
//!
//! Detection reads the index rather than walking again, so it honours
//! exactly the ignore, size and symlink rules the index does, and it is a
//! pure function of the index's paths (`context.md` §3).

use crate::command_policy::{CommandPolicy, CommandRisk, PROJECT_MANIFESTS};
use crate::config::ConfigOverlay;
use crate::context_manager::{AGENT_INSTRUCTIONS_FILE, agent_instruction_paths};
use crate::hash::{now_millis, sha256};
use crate::indexer::{RepositoryIndex, SkippedFile};
use crate::path_policy::PathPolicy;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};

/// Bumped whenever the map's shape or derivation changes, so a persisted
/// map from an older build is rebuilt rather than trusted (§5.8).
pub const REPOSITORY_MAP_SCHEMA_VERSION: u32 = 1;

/// Deepest directory, in path segments, that can be a root. `""` is depth 0
/// and `packages/api` is depth 2. A manifest deeper than this is recorded in
/// `excluded`, not silently dropped (§5.2).
pub const MAX_ROOT_DEPTH: usize = 6;

/// Directories that are never roots, whatever `ignore_patterns` says (§5.2).
/// Independent of the ignore list on purpose: a user may index
/// `node_modules`, but its packages are still not the user's projects.
pub const VENDOR_DIRECTORIES: [&str; 7] = [
    "node_modules",
    "vendor",
    "target",
    "dist",
    "build",
    ".venv",
    "venv",
];

/// Root-relative paths that are a root's entry points when they exist. Path
/// based, because proposal §4 rules out parsing manifests. `src/bin/*.rs`
/// and `cmd/*/main.go` are matched by [`is_entry_point`].
const ENTRY_POINTS: [&str; 13] = [
    "src/main.rs",
    "src/lib.rs",
    "main.go",
    "index.js",
    "index.ts",
    "src/index.js",
    "src/index.ts",
    "src/main.js",
    "src/main.ts",
    "main.py",
    "__main__.py",
    "app.py",
    "manage.py",
];

/// Names of a root's immediate child directories that hold its tests.
const TEST_DIRECTORIES: [&str; 5] = ["tests", "test", "__tests__", "spec", "e2e"];

/// Why a directory is a root. The UI turns this into a sentence the user can
/// disagree with (requirement 7).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum RootEvidence {
    /// The repository root, which is a root even with no manifest.
    RepositoryRoot,
    /// This manifest, repository-relative, exists.
    Manifest { path: String },
    /// Named in `project_roots_added`, with no manifest of its own.
    UserOverride,
}

/// The user's correction to detection (requirement 7, §5.7).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum RootOverride {
    Added,
    /// Kept in the map so the UI can show and undo it, but not a root for
    /// anything: it owns no files and offers no commands.
    Removed,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum ExclusionReason {
    Vendor,
    BelowDepthCeiling,
    /// A `project_roots_added` or `project_roots_removed` entry that was not
    /// applied (`context.md` §6).
    InvalidOverride,
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ExcludedPath {
    pub path: String,
    pub reason: ExclusionReason,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DetectedRoot {
    /// Repository-relative. `""` is the repository root.
    pub path: String,
    pub detected_by: RootEvidence,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RootDetection {
    /// Sorted by path, so `""` is first.
    pub roots: Vec<DetectedRoot>,
    /// Sorted by path, then reason.
    pub excluded: Vec<ExcludedPath>,
}

/// Finds the project roots among an index's files.
///
/// A directory is a root when it holds one of [`PROJECT_MANIFESTS`]. Nested
/// roots are all kept: a Cargo workspace and its members are each right for
/// their own commands (§5.2). The output does not depend on input order:
/// both collections are sorted, and a directory's evidence is its first
/// manifest in `PROJECT_MANIFESTS` order (§5.3).
pub fn detect_roots<'a>(
    files: impl IntoIterator<Item = &'a str>,
    skipped: &[SkippedFile],
) -> RootDetection {
    // directory -> (rank in PROJECT_MANIFESTS, manifest path)
    let mut found: BTreeMap<String, (usize, String)> = BTreeMap::new();
    let mut excluded = BTreeSet::new();

    for path in files {
        let (directory, file_name) = path.rsplit_once('/').unwrap_or(("", path));
        let Some(rank) = PROJECT_MANIFESTS
            .iter()
            .position(|manifest| *manifest == file_name)
        else {
            continue;
        };
        if let Some(vendor) = vendor_prefix(directory) {
            excluded.insert(ExcludedPath {
                path: vendor,
                reason: ExclusionReason::Vendor,
            });
            continue;
        }
        if depth(directory) > MAX_ROOT_DEPTH {
            excluded.insert(ExcludedPath {
                path: directory.to_string(),
                reason: ExclusionReason::BelowDepthCeiling,
            });
            continue;
        }
        let candidate = (rank, path.to_string());
        found
            .entry(directory.to_string())
            .and_modify(|current| {
                if candidate < *current {
                    *current = candidate.clone();
                }
            })
            .or_insert(candidate);
    }

    // Only vendor directories are listed. Other ignored entries were never
    // candidates, and listing them would grow with the repository
    // (context.md §11).
    for skip in skipped {
        let last = skip.path.rsplit('/').next().unwrap_or(&skip.path);
        if skip.reason == "ignored" && VENDOR_DIRECTORIES.contains(&last) {
            excluded.insert(ExcludedPath {
                path: skip.path.clone(),
                reason: ExclusionReason::Vendor,
            });
        }
    }

    let mut roots: Vec<DetectedRoot> = found
        .into_iter()
        .map(|(path, (_, manifest))| DetectedRoot {
            path,
            detected_by: RootEvidence::Manifest { path: manifest },
        })
        .collect();
    if roots.first().is_none_or(|first| !first.path.is_empty()) {
        roots.insert(
            0,
            DetectedRoot {
                path: String::new(),
                detected_by: RootEvidence::RepositoryRoot,
            },
        );
    }

    RootDetection {
        roots,
        excluded: excluded.into_iter().collect(),
    }
}

fn depth(directory: &str) -> usize {
    if directory.is_empty() {
        0
    } else {
        directory.split('/').count()
    }
}

/// The path up to and including the first vendor segment, if any.
fn vendor_prefix(directory: &str) -> Option<String> {
    if directory.is_empty() {
        return None;
    }
    let segments: Vec<&str> = directory.split('/').collect();
    let index = segments
        .iter()
        .position(|segment| VENDOR_DIRECTORIES.contains(segment))?;
    Some(segments[..=index].join("/"))
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RootCommand {
    pub name: String,
    pub command: String,
    pub risk: CommandRisk,
    /// Repository-relative directory the command runs in. `""` is the
    /// repository root (requirement 5).
    pub working_directory: String,
}

/// One project root and what it holds. Every path is repository-relative,
/// like `path` itself, so a reader never has to know which root a path was
/// relative to.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProjectRoot {
    /// `""` is the repository root.
    pub path: String,
    /// Why this is a root (requirement 7).
    pub detected_by: RootEvidence,
    /// Distinct languages of the files this root owns, except `text`.
    pub languages: Vec<String>,
    /// The root's own [`PROJECT_MANIFESTS`] files.
    pub manifests: Vec<String>,
    pub entry_points: Vec<String>,
    /// Immediate child directories named like test directories.
    pub test_paths: Vec<String>,
    /// Excluded vendor and build-output directories this root owns.
    pub generated_paths: Vec<String>,
    /// Immediate child directories that are not roots (`context.md` §12).
    pub major_directories: Vec<String>,
    /// Spec 11's `AGENTS.md` files for a path inside this root, broadest
    /// first, kept when indexed (`context.md` §14). Not sorted: the order is
    /// the precedence.
    pub instruction_files: Vec<String>,
    /// Sorted by name.
    pub commands: Vec<RootCommand>,
    pub user_override: Option<RootOverride>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RepositoryMap {
    pub repository_id: String,
    pub schema_version: u32,
    pub generated_at_ms: u128,
    /// Hash of the inputs the map was derived from, for staleness detection
    /// (`context.md` §3).
    pub fingerprint: String,
    /// Sorted by path, so `""` is first.
    pub roots: Vec<ProjectRoot>,
    pub excluded: Vec<ExcludedPath>,
}

impl RepositoryMap {
    /// Derives the map from an index, with no second walk. Only the commands
    /// read the disk, through [`CommandPolicy::detect_root_commands`]. Every
    /// collection is sorted here rather than trusted to the index's order,
    /// because the watcher appends patched records at the end (§5.3).
    ///
    /// The user's overrides come from the policy's config, the same config
    /// that classifies the commands, so the map and the approvals never
    /// disagree about which repository config is in effect.
    pub fn build(index: &RepositoryIndex, policy: &CommandPolicy) -> Self {
        let config = policy.config();
        let mut detection = detect_roots(
            index.files.iter().map(|file| file.path.as_str()),
            &index.skipped,
        );
        let indexed: BTreeSet<&str> = index.files.iter().map(|file| file.path.as_str()).collect();
        let overrides = apply_overrides(
            &mut detection,
            &config.project_roots_added,
            &config.project_roots_removed,
            &indexed,
            &PathPolicy::new(config),
        );
        // A removed root is not a root for ownership, so its files, its
        // directory and its vendor output go to the nearest root that is
        // not removed. `active[i]` is the position in `roots` of
        // `root_paths[i]`.
        let (active, root_paths): (Vec<usize>, Vec<&str>) = detection
            .roots
            .iter()
            .enumerate()
            .filter(|(_, root)| overrides.get(&root.path) != Some(&RootOverride::Removed))
            .map(|(position, root)| (position, root.path.as_str()))
            .unzip();

        let mut roots: Vec<ProjectRoot> = detection
            .roots
            .iter()
            .map(|root| {
                let user_override = overrides.get(&root.path).copied();
                let removed = user_override == Some(RootOverride::Removed);
                ProjectRoot {
                    path: root.path.clone(),
                    detected_by: root.detected_by.clone(),
                    languages: Vec::new(),
                    manifests: Vec::new(),
                    entry_points: Vec::new(),
                    test_paths: Vec::new(),
                    generated_paths: Vec::new(),
                    major_directories: Vec::new(),
                    instruction_files: if removed {
                        Vec::new()
                    } else {
                        instruction_files(&root.path, &indexed)
                    },
                    commands: if removed {
                        Vec::new()
                    } else {
                        policy
                            .detect_root_commands(&index.root_path, &root.path)
                            .unwrap_or_default()
                    },
                    user_override,
                }
            })
            .collect();

        for file in &index.files {
            let owner = nearest_root(&root_paths, &file.path);
            let root = &mut roots[active[owner]];
            let relative = relative_to(root_paths[owner], &file.path);
            if file.language != "text" && !root.languages.contains(&file.language) {
                root.languages.push(file.language.clone());
            }
            if PROJECT_MANIFESTS.contains(&relative) {
                root.manifests.push(file.path.clone());
            }
            if is_entry_point(relative) {
                root.entry_points.push(file.path.clone());
            }

            // A child directory belongs to every enclosing root, not only the
            // nearest: `crates` is a major directory of the workspace even
            // when every file beneath it belongs to a member crate.
            for (position, root_path) in root_paths.iter().enumerate() {
                if !owns(root_path, &file.path) {
                    continue;
                }
                let Some((child, _)) = relative_to(root_path, &file.path).split_once('/') else {
                    continue;
                };
                let child_path = join(root_path, child);
                if root_paths.contains(&child_path.as_str()) {
                    continue;
                }
                let root = &mut roots[active[position]];
                if TEST_DIRECTORIES.contains(&child) {
                    root.test_paths.push(child_path.clone());
                }
                root.major_directories.push(child_path);
            }
        }

        for excluded in &detection.excluded {
            if excluded.reason == ExclusionReason::Vendor {
                let owner = nearest_root(&root_paths, &excluded.path);
                roots[active[owner]]
                    .generated_paths
                    .push(excluded.path.clone());
            }
        }

        for root in &mut roots {
            root.languages.sort();
            sort_and_dedup(&mut root.manifests);
            sort_and_dedup(&mut root.entry_points);
            sort_and_dedup(&mut root.test_paths);
            sort_and_dedup(&mut root.generated_paths);
            sort_and_dedup(&mut root.major_directories);
        }

        RepositoryMap {
            repository_id: index.repository_id.clone(),
            schema_version: REPOSITORY_MAP_SCHEMA_VERSION,
            generated_at_ms: now_millis(),
            fingerprint: fingerprint(
                index,
                &detection.excluded,
                &config.project_roots_added,
                &config.project_roots_removed,
            ),
            roots,
            excluded: detection.excluded,
        }
    }

    /// The root that owns `path`: the longest root that is a path-segment
    /// prefix of it, so `packages/a` does not own `packages/ab/x`. Falls back
    /// to the repository root.
    /// A removed root owns nothing.
    pub fn root_for_path(&self, path: &str) -> &str {
        self.roots
            .iter()
            .filter(|root| root.user_override != Some(RootOverride::Removed))
            .filter(|root| owns(&root.path, path))
            .max_by_key(|root| root.path.len())
            .map_or("", |root| root.path.as_str())
    }
}

/// Hashes the map's inputs, not its output (`context.md` §3): the sorted
/// indexed paths, the content of every manifest and instruction file, the
/// excluded entries, the user's override lists, and the schema version.
/// Other files' content and every modification time are left out, so an
/// edit to source code does not make the map stale.
fn fingerprint(
    index: &RepositoryIndex,
    excluded: &[ExcludedPath],
    added: &[String],
    removed: &[String],
) -> String {
    let mut files: Vec<(&str, &str)> = index
        .files
        .iter()
        .map(|file| (file.path.as_str(), file.content_hash.as_str()))
        .collect();
    files.sort_unstable();

    // Length-prefixed fields, so no path can be confused with a separator.
    let mut input = format!("schema {REPOSITORY_MAP_SCHEMA_VERSION}\n");
    for (path, content_hash) in files {
        let file_name = path.rsplit('/').next().unwrap_or(path);
        let hashed = PROJECT_MANIFESTS.contains(&file_name) || file_name == AGENT_INSTRUCTIONS_FILE;
        let content_hash = if hashed { content_hash } else { "" };
        input.push_str(&format!(
            "file {}:{path} {}:{content_hash}\n",
            path.len(),
            content_hash.len()
        ));
    }
    for entry in excluded {
        input.push_str(&format!(
            "excluded {}:{} {:?}\n",
            entry.path.len(),
            entry.path,
            entry.reason
        ));
    }
    for (list, entries) in [("added", added), ("removed", removed)] {
        for entry in entries {
            input.push_str(&format!("{list} {}:{entry}\n", entry.len()));
        }
    }
    sha256(input)
}

/// Applies the user's overrides to `detection` and returns the override of
/// each root it touched. An entry that cannot apply is recorded in
/// `detection.excluded` as `invalidOverride` and changes nothing else
/// (`context.md` §6).
fn apply_overrides(
    detection: &mut RootDetection,
    added: &[String],
    removed: &[String],
    indexed: &BTreeSet<&str>,
    paths: &PathPolicy,
) -> BTreeMap<String, RootOverride> {
    let mut overrides = BTreeMap::new();
    let mut invalid = Vec::new();
    let valid = |entry: &str| valid_override(entry, indexed, paths);

    for entry in added {
        if !valid(entry) || removed.contains(entry) {
            invalid.push(entry.clone());
            continue;
        }
        if !detection.roots.iter().any(|root| root.path == *entry) {
            detection.roots.push(DetectedRoot {
                path: entry.clone(),
                detected_by: RootEvidence::UserOverride,
            });
        }
        overrides.insert(entry.clone(), RootOverride::Added);
    }
    for entry in removed {
        // The repository root is never removed: the map is never empty
        // (§5.2). An entry that names no root would do nothing, so the user
        // is shown it rather than left to think it worked.
        let names_root = detection.roots.iter().any(|root| root.path == *entry);
        if entry.is_empty() || !valid(entry) || added.contains(entry) || !names_root {
            invalid.push(entry.clone());
            continue;
        }
        overrides.insert(entry.clone(), RootOverride::Removed);
    }

    detection.roots.sort_by(|a, b| a.path.cmp(&b.path));
    detection
        .excluded
        .extend(invalid.into_iter().map(|path| ExcludedPath {
            path,
            reason: ExclusionReason::InvalidOverride,
        }));
    detection.excluded.sort();
    detection.excluded.dedup();
    overrides
}

/// Whether `entry` can be a root: a normalised repository-relative path, not
/// under a vendor directory, not restricted, and a directory holding at
/// least one indexed file.
fn valid_override(entry: &str, indexed: &BTreeSet<&str>, paths: &PathPolicy) -> bool {
    if entry.is_empty() {
        return false;
    }
    let segments: Vec<&str> = entry.split('/').collect();
    // Rejects absolute paths (a leading empty segment), `.`, `..`, `//` and
    // a trailing `/`.
    if segments
        .iter()
        .any(|segment| matches!(*segment, "" | "." | ".."))
    {
        return false;
    }
    if segments
        .iter()
        .any(|segment| VENDOR_DIRECTORIES.contains(segment))
    {
        return false;
    }
    // A restricted ancestor restricts everything beneath it.
    if (1..=segments.len()).any(|end| paths.is_restricted(&segments[..end].join("/"), true)) {
        return false;
    }
    indexed
        .iter()
        .any(|path| path.len() > entry.len() && owns(entry, path))
}

/// What `damaian repo-root` and the UI do to repository config.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RootOverrideEdit {
    Add,
    Remove,
    /// Forget the path in both lists, so detection decides again.
    Clear,
}

/// Edits the override lists in `overlay`. A path is in at most one list
/// afterwards, and an emptied list is unset so its line disappears.
pub fn edit_root_overrides(overlay: &mut ConfigOverlay, edit: RootOverrideEdit, path: &str) {
    let mut added = overlay.project_roots_added.take().unwrap_or_default();
    let mut removed = overlay.project_roots_removed.take().unwrap_or_default();
    added.retain(|entry| entry != path);
    removed.retain(|entry| entry != path);
    match edit {
        RootOverrideEdit::Add => added.push(path.to_string()),
        RootOverrideEdit::Remove => removed.push(path.to_string()),
        RootOverrideEdit::Clear => {}
    }
    overlay.project_roots_added = (!added.is_empty()).then_some(added);
    overlay.project_roots_removed = (!removed.is_empty()).then_some(removed);
}

/// Spec 11's ancestor walk for a path inside `root`, kept when indexed.
fn instruction_files(root: &str, indexed: &BTreeSet<&str>) -> Vec<String> {
    agent_instruction_paths(&[join(root, "_")])
        .into_iter()
        .filter(|path| indexed.contains(path.as_str()))
        .collect()
}

fn is_entry_point(relative: &str) -> bool {
    if ENTRY_POINTS.contains(&relative) {
        return true;
    }
    if let Some(binary) = relative.strip_prefix("src/bin/") {
        return !binary.contains('/') && binary.len() > ".rs".len() && binary.ends_with(".rs");
    }
    if let Some(rest) = relative.strip_prefix("cmd/") {
        return rest
            .strip_suffix("/main.go")
            .is_some_and(|command| !command.is_empty() && !command.contains('/'));
    }
    false
}

/// Whether the root `root` contains `path`, by whole path segments.
fn owns(root: &str, path: &str) -> bool {
    root.is_empty()
        || path
            .strip_prefix(root)
            .is_some_and(|rest| rest.is_empty() || rest.starts_with('/'))
}

/// Index into `roots` of the root that owns `path`. `roots` always holds
/// `""`, which owns everything.
fn nearest_root(roots: &[&str], path: &str) -> usize {
    roots
        .iter()
        .enumerate()
        .filter(|(_, root)| owns(root, path))
        .max_by_key(|(_, root)| root.len())
        .map_or(0, |(position, _)| position)
}

/// `path` relative to `root`, which owns it.
fn relative_to<'a>(root: &str, path: &'a str) -> &'a str {
    if root.is_empty() {
        path
    } else {
        path.strip_prefix(root)
            .and_then(|rest| rest.strip_prefix('/'))
            .unwrap_or("")
    }
}

fn join(root: &str, child: &str) -> String {
    if root.is_empty() {
        child.to_string()
    } else {
        format!("{root}/{child}")
    }
}

fn sort_and_dedup(values: &mut Vec<String>) {
    values.sort();
    values.dedup();
}

//! The repository map (spec 24): where a repository's project roots are,
//! and what each one holds.
//!
//! Detection reads the index rather than walking again, so it honours
//! exactly the ignore, size and symlink rules the index does, and it is a
//! pure function of the index's paths (`context.md` §3).

use crate::audit::AuditLog;
use crate::command_policy::{CommandPolicy, CommandRisk, PROJECT_MANIFESTS};
use crate::config::{Config, ConfigOverlay};
use crate::context_manager::{AGENT_INSTRUCTIONS_FILE, agent_instruction_paths};
use crate::error::{ClientError, Result};
use crate::hash::{create_id, now_millis, sha256};
use crate::indexer::{RepositoryIndex, SkippedFile};
use crate::path_policy::PathPolicy;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::io::ErrorKind;
use std::path::{Path, PathBuf};

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
        let indexed: BTreeSet<&str> = index.files.iter().map(|file| file.path.as_str()).collect();
        let (detection, overrides) = detect_with_overrides(index, &indexed, config);
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
            fingerprint: fingerprint(index, &detection.excluded, config),
            roots,
            excluded: detection.excluded,
        }
    }

    /// The map as the JSON the map file holds, for callers with no serde of
    /// their own (the CLI).
    pub fn to_json(&self) -> String {
        // Every field is a string, a number, a vector or a unit-tagged enum,
        // none of which can fail to serialise.
        serde_json::to_string_pretty(self).expect("a repository map always serialises")
    }

    /// The map as model context, at most `max_tokens` by the `len / 4`
    /// estimate `build_context` uses. A map that does not fit degrades in
    /// proposal §5.4's order and ends with a line saying what it dropped:
    ///
    /// 1. each root's entry points and test paths, the root with the most
    ///    of them (by length) first;
    /// 2. major directories;
    /// 3. generated paths, reduced to a count;
    /// 4. whole roots, deepest first and then from the end of path order,
    ///    so what stays is a prefix of each depth. The repository root goes
    ///    only when nothing else fits, and then the text is empty.
    ///
    /// Languages and commands are never dropped from a root that is shown.
    /// Removed roots are never rendered or counted.
    pub fn render_for_model(&self, max_tokens: usize) -> RenderedMap {
        let roots: Vec<&ProjectRoot> = self
            .roots
            .iter()
            .filter(|root| root.user_override != Some(RootOverride::Removed))
            .collect();
        let mut rendering = Rendering {
            shown: vec![true; roots.len()],
            details: vec![true; roots.len()],
            directories: true,
            generated: true,
            dropped: Vec::new(),
            roots,
        };
        let fits = |rendering: &Rendering| {
            let text = rendering.text();
            (estimate_tokens(&text) <= max_tokens).then_some(text)
        };
        if let Some(text) = fits(&rendering) {
            return rendering.finish(text);
        }

        let mut by_size: Vec<usize> = (0..rendering.roots.len())
            .filter(|&position| detail_length(rendering.roots[position]) > 0)
            .collect();
        by_size.sort_by(|&a, &b| {
            let (a, b) = (rendering.roots[a], rendering.roots[b]);
            detail_length(b)
                .cmp(&detail_length(a))
                .then_with(|| a.path.cmp(&b.path))
        });
        for position in by_size {
            rendering.details[position] = false;
            rendering
                .dropped
                .push(Degradation::EntryPointsAndTestPaths {
                    root: rendering.roots[position].path.clone(),
                });
            if let Some(text) = fits(&rendering) {
                return rendering.finish(text);
            }
        }

        if rendering
            .roots
            .iter()
            .any(|root| !root.major_directories.is_empty())
        {
            rendering.directories = false;
            rendering.dropped.push(Degradation::MajorDirectories);
            if let Some(text) = fits(&rendering) {
                return rendering.finish(text);
            }
        }

        if rendering
            .roots
            .iter()
            .any(|root| !root.generated_paths.is_empty())
        {
            rendering.generated = false;
            rendering.dropped.push(Degradation::GeneratedPathsCounted);
            if let Some(text) = fits(&rendering) {
                return rendering.finish(text);
            }
        }

        let mut by_depth: Vec<usize> = (0..rendering.roots.len())
            .filter(|&position| !rendering.roots[position].path.is_empty())
            .collect();
        by_depth.sort_by(|&a, &b| {
            let (a, b) = (rendering.roots[a], rendering.roots[b]);
            depth(&b.path)
                .cmp(&depth(&a.path))
                .then_with(|| b.path.cmp(&a.path))
        });
        rendering.dropped.push(Degradation::Roots {
            omitted: Vec::new(),
        });
        for position in by_depth {
            rendering.omit(position);
            if let Some(text) = fits(&rendering) {
                return rendering.finish(text);
            }
        }

        // Not even the repository root fits. The ceiling is the bound, so
        // nothing is rendered rather than something over it.
        for position in 0..rendering.roots.len() {
            if rendering.shown[position] {
                rendering.omit(position);
            }
        }
        rendering.finish(String::new())
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

/// [`RepositoryMap::render_for_model`]'s output.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RenderedMap {
    /// Empty when not even the repository root fits.
    pub text: String,
    /// `text.len().div_ceil(4)`, the estimate `build_context` uses.
    pub tokens: usize,
    pub roots_shown: usize,
    /// Roots that are not removed.
    pub roots_total: usize,
    /// In the order applied.
    pub dropped: Vec<Degradation>,
}

/// One step of proposal §5.4's degradation, in the order they are applied.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Degradation {
    /// This root's entry points and test paths were left out.
    EntryPointsAndTestPaths { root: String },
    /// Every root's major directories were left out.
    MajorDirectories,
    /// Every root's generated paths were reduced to a count.
    GeneratedPathsCounted,
    /// These roots were left out, in the order they were dropped.
    Roots { omitted: Vec<String> },
}

/// What [`RepositoryMap::render_for_model`] keeps at one step.
struct Rendering<'a> {
    roots: Vec<&'a ProjectRoot>,
    shown: Vec<bool>,
    /// Entry points and test paths, per root.
    details: Vec<bool>,
    directories: bool,
    /// Listed rather than counted.
    generated: bool,
    dropped: Vec<Degradation>,
}

impl Rendering<'_> {
    fn omit(&mut self, position: usize) {
        self.shown[position] = false;
        if let Some(Degradation::Roots { omitted }) = self.dropped.last_mut() {
            omitted.push(self.roots[position].path.clone());
        }
    }

    fn roots_shown(&self) -> usize {
        self.shown.iter().filter(|shown| **shown).count()
    }

    fn finish(self, text: String) -> RenderedMap {
        RenderedMap {
            tokens: estimate_tokens(&text),
            text,
            roots_shown: self.roots_shown(),
            roots_total: self.roots.len(),
            dropped: self.dropped,
        }
    }

    fn text(&self) -> String {
        let mut output = String::from(
            "Project roots. Paths are repository-relative and `.` is the repository root. \
             A root's commands belong to its directory.\n",
        );
        for (position, root) in self.roots.iter().enumerate() {
            if !self.shown[position] {
                continue;
            }
            let name = if root.path.is_empty() {
                "."
            } else {
                &root.path
            };
            output.push_str("- ");
            output.push_str(name);
            // A root's manifests sit in its own directory, so the file name
            // says enough.
            let manifests: Vec<&str> = root
                .manifests
                .iter()
                .map(|manifest| manifest.rsplit('/').next().unwrap_or(manifest))
                .collect();
            let about: Vec<String> = [manifests.join(", "), root.languages.join(", ")]
                .into_iter()
                .filter(|part| !part.is_empty())
                .collect();
            if !about.is_empty() {
                output.push_str(&format!(" ({})", about.join("; ")));
            }
            output.push('\n');

            push_list(
                &mut output,
                "commands",
                root.commands
                    .iter()
                    .map(|command| format!("`{}`", command.command)),
            );
            // Only the root's own: an enclosing root lists the files above
            // it, and spec 11 loads them for any path in context.
            push_list(
                &mut output,
                "instructions",
                root.instruction_files
                    .iter()
                    .filter(|file| {
                        let directory =
                            file.rsplit_once('/').map_or("", |(directory, _)| directory);
                        directory == root.path
                    })
                    .cloned(),
            );
            if self.details[position] {
                push_list(
                    &mut output,
                    "entry points",
                    root.entry_points.iter().cloned(),
                );
                push_list(&mut output, "tests", root.test_paths.iter().cloned());
            }
            if self.directories {
                // Test directories are also major directories. While the
                // test line is shown, they are not repeated here.
                push_list(
                    &mut output,
                    "directories",
                    root.major_directories
                        .iter()
                        .filter(|directory| {
                            !self.details[position] || !root.test_paths.contains(directory)
                        })
                        .cloned(),
                );
            }
            match root.generated_paths.len() {
                0 => {}
                count if !self.generated => {
                    let noun = if count == 1 { "path" } else { "paths" };
                    output.push_str(&format!("  generated: {count} {noun}\n"));
                }
                _ => push_list(
                    &mut output,
                    "generated",
                    root.generated_paths.iter().cloned(),
                ),
            }
        }
        if !self.dropped.is_empty() {
            output.push_str(&self.abridgement());
        }
        output
    }

    /// The line that says what was dropped, so an abridged map is never
    /// mistaken for a whole one (§5.4).
    fn abridgement(&self) -> String {
        let mut parts = vec![format!(
            "{} of {} roots shown",
            self.roots_shown(),
            self.roots.len()
        )];
        let without_details = self
            .dropped
            .iter()
            .filter(|step| matches!(step, Degradation::EntryPointsAndTestPaths { .. }))
            .count();
        let with_details = self
            .roots
            .iter()
            .filter(|root| detail_length(root) > 0)
            .count();
        if without_details == with_details && without_details > 0 {
            parts.push("entry points and test paths omitted".to_string());
        } else if without_details > 0 {
            parts.push(format!(
                "entry points and test paths omitted for {without_details} of {with_details} roots"
            ));
        }
        if !self.directories {
            parts.push("major directories omitted".to_string());
        }
        if !self.generated {
            parts.push("generated paths counted".to_string());
        }
        format!("Abridged to fit: {}.\n", parts.join("; "))
    }
}

fn push_list(output: &mut String, label: &str, items: impl Iterator<Item = String>) {
    let items: Vec<String> = items.collect();
    if !items.is_empty() {
        output.push_str(&format!("  {label}: {}\n", items.join(", ")));
    }
}

/// The length of what step 1 drops from a root, which decides which root is
/// largest.
fn detail_length(root: &ProjectRoot) -> usize {
    root.entry_points
        .iter()
        .chain(&root.test_paths)
        .map(String::len)
        .sum()
}

/// `build_context`'s estimate, so the map's ceiling and the context budget
/// count the same way.
fn estimate_tokens(text: &str) -> usize {
    text.len().div_ceil(4)
}

/// How [`RepositoryMapStore::load_or_build`] got its map.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MapLoad {
    /// The stored map matched the inputs and was returned as stored.
    Reused,
    /// There was no stored map.
    Built,
    /// A stored map existed and could not be used (§5.8).
    Rebuilt { reason: RebuildReason },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RebuildReason {
    /// Not a map this build can read, or another repository's.
    Corrupt,
    SchemaMismatch {
        found: u32,
    },
    /// Readable, but derived from inputs that have since changed.
    Stale,
}

impl RebuildReason {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Corrupt => "corrupt",
            Self::SchemaMismatch { .. } => "schemaMismatch",
            Self::Stale => "stale",
        }
    }
}

/// The persisted map, one JSON file per repository, reused while its input
/// fingerprint matches and rebuilt otherwise (`context.md` §3).
#[derive(Debug, Clone)]
pub struct RepositoryMapStore {
    data_dir: PathBuf,
    audit_log: AuditLog,
}

impl RepositoryMapStore {
    pub fn new(data_dir: impl AsRef<Path>, audit_log: AuditLog) -> Self {
        Self {
            data_dir: data_dir.as_ref().to_path_buf(),
            audit_log,
        }
    }

    pub fn path(data_dir: &Path, repository_id: &str) -> PathBuf {
        data_dir
            .join("repository-map")
            .join(format!("{repository_id}.json"))
    }

    /// The stored map of `index` when it is still the map `policy` would
    /// build, and otherwise a fresh one, written before it is returned.
    /// Reuse is decided on the fingerprint alone, which needs no command
    /// detection, so a reuse reads one file and the index.
    ///
    /// Nothing about approval rests on a stored map: every proposal is
    /// classified again when it is made.
    pub fn load_or_build(
        &self,
        index: &RepositoryIndex,
        policy: &CommandPolicy,
    ) -> Result<(RepositoryMap, MapLoad)> {
        let path = Self::path(&self.data_dir, &index.repository_id);
        let reason = match fs::read(&path) {
            Ok(bytes) => match stored_map(&bytes, &index.repository_id) {
                Ok(map) if map.fingerprint == input_fingerprint(index, policy) => {
                    return Ok((map, MapLoad::Reused));
                }
                Ok(_) => Some(RebuildReason::Stale),
                Err(reason) => Some(reason),
            },
            Err(error) if error.kind() == ErrorKind::NotFound => None,
            Err(error) => return Err(error.into()),
        };

        let map = RepositoryMap::build(index, policy);
        let json = serde_json::to_vec_pretty(&map).map_err(|error| {
            ClientError::Io(format!("Failed to serialise the repository map: {error}"))
        })?;
        write_replacing(&path, &json)?;
        let Some(reason) = reason else {
            return Ok((map, MapLoad::Built));
        };
        let mut fields = vec![
            ("repositoryId", map.repository_id.clone()),
            ("reason", reason.as_str().to_string()),
        ];
        if let RebuildReason::SchemaMismatch { found } = reason {
            fields.push(("foundSchemaVersion", found.to_string()));
        }
        self.audit_log.record("repository_map_rebuilt", &fields)?;
        Ok((map, MapLoad::Rebuilt { reason }))
    }
}

/// Parses a stored map, or says why it cannot be used. The version is read
/// before the shape, so a map from another schema is reported as that rather
/// than as corrupt.
fn stored_map(
    bytes: &[u8],
    repository_id: &str,
) -> std::result::Result<RepositoryMap, RebuildReason> {
    let value: serde_json::Value =
        serde_json::from_slice(bytes).map_err(|_| RebuildReason::Corrupt)?;
    let found = value
        .get("schemaVersion")
        .and_then(serde_json::Value::as_u64)
        .and_then(|version| u32::try_from(version).ok())
        .ok_or(RebuildReason::Corrupt)?;
    if found != REPOSITORY_MAP_SCHEMA_VERSION {
        return Err(RebuildReason::SchemaMismatch { found });
    }
    let map: RepositoryMap = serde_json::from_value(value).map_err(|_| RebuildReason::Corrupt)?;
    if map.repository_id != repository_id {
        return Err(RebuildReason::Corrupt);
    }
    Ok(map)
}

/// Writes `path` through a temp file in the same directory and a rename, so
/// a crash leaves the old file or the new one, never half of either. The
/// temp name is unique, so two processes rebuilding at once do not write
/// into each other's file.
fn write_replacing(path: &Path, bytes: &[u8]) -> Result<()> {
    let directory = path
        .parent()
        .ok_or_else(|| ClientError::Io(format!("{} has no parent", path.display())))?;
    fs::create_dir_all(directory)?;
    let file_name = path.file_name().unwrap_or_default().to_string_lossy();
    let temp_path = directory.join(format!(".{file_name}.{}.tmp", create_id("map")));
    let written = fs::write(&temp_path, bytes).and_then(|()| fs::rename(&temp_path, path));
    if written.is_err() {
        let _ = fs::remove_file(&temp_path);
    }
    Ok(written?)
}

/// Root detection with the user's overrides applied, the part of
/// [`RepositoryMap::build`] that reads neither the disk nor the commands.
fn detect_with_overrides(
    index: &RepositoryIndex,
    indexed: &BTreeSet<&str>,
    config: &Config,
) -> (RootDetection, BTreeMap<String, RootOverride>) {
    let mut detection = detect_roots(
        index.files.iter().map(|file| file.path.as_str()),
        &index.skipped,
    );
    let overrides = apply_overrides(
        &mut detection,
        &config.project_roots_added,
        &config.project_roots_removed,
        indexed,
        &PathPolicy::new(config),
    );
    (detection, overrides)
}

/// The fingerprint [`RepositoryMap::build`] would give a map of `index`
/// under `policy`, without building one: no command is detected and no
/// manifest is read.
fn input_fingerprint(index: &RepositoryIndex, policy: &CommandPolicy) -> String {
    let indexed: BTreeSet<&str> = index.files.iter().map(|file| file.path.as_str()).collect();
    let (detection, _) = detect_with_overrides(index, &indexed, policy.config());
    fingerprint(index, &detection.excluded, policy.config())
}

/// Hashes the map's inputs, not its output (`context.md` §3): the sorted
/// indexed paths, the content of every manifest and instruction file, the
/// excluded entries, the schema version, and the config the map reads: the
/// user's override lists, and `command_allowlist` and `command_blocklist`,
/// which decide a command's risk. Without those two a stored map would keep
/// a risk from before a grant. `restricted_patterns` is not hashed itself:
/// its only effect on the map is whether an override applies, and a
/// rejected override is an `excluded` entry, which is. Other files' content
/// and every modification time are left out, so an edit to source code does
/// not make the map stale.
fn fingerprint(index: &RepositoryIndex, excluded: &[ExcludedPath], config: &Config) -> String {
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
    for (list, entries) in [
        ("added", &config.project_roots_added),
        ("removed", &config.project_roots_removed),
        ("allowlist", &config.command_allowlist),
        ("blocklist", &config.command_blocklist),
    ] {
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

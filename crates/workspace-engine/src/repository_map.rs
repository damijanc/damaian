//! The repository map (spec 24): where a repository's project roots are,
//! and what each one holds.
//!
//! Detection reads the index rather than walking again, so it honours
//! exactly the ignore, size and symlink rules the index does, and it is a
//! pure function of the index's paths (`context.md` §3).

use crate::command_policy::PROJECT_MANIFESTS;
use crate::indexer::SkippedFile;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};

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

/// Why a directory is a root. The UI turns this into a sentence the user can
/// disagree with (requirement 7).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum RootEvidence {
    /// The repository root, which is a root even with no manifest.
    RepositoryRoot,
    /// This manifest, repository-relative, exists.
    Manifest { path: String },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum ExclusionReason {
    Vendor,
    BelowDepthCeiling,
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

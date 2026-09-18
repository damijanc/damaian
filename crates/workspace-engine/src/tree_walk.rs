//! The one directory traversal in this crate.
//!
//! Lifted out of `ProjectIndexer::walk` (spec 47 §5.4) so the navigation tools
//! cannot become a second place the ignore rules and the symlink-escape check
//! live. A second walker would be a second place for them to drift, and the
//! escape check is a security property rather than a convenience: a symlink
//! that canonicalizes outside the root would otherwise let a walk reach
//! anything the user can read.

use crate::error::Result;
use crate::ignore::{IgnoreRule, is_ignored_by_rules, parse_ignore_patterns};
use std::fs;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WalkFile {
    pub relative_path: String,
    pub absolute_path: PathBuf,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WalkSkip {
    pub path: String,
    /// Why it was skipped, in the vocabulary the index already reports:
    /// `ignored`, `symlink_outside_root`, `not_regular_file`.
    pub reason: String,
}

/// What the walk found. A visitor sees every file and every skip, so a caller
/// that wants to report what it passed over can, and one that does not can
/// ignore the variant. Directories are recursed into rather than reported —
/// no caller needs them yet, and an unused variant is a guess about the future.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WalkEvent {
    File(WalkFile),
    Skipped(WalkSkip),
}

/// Walks `directory` under `root`, accumulating per-directory `.gitignore`
/// rules on the way down and handing every outcome to `visitor`.
///
/// The order of the three checks — ignore, then symlink, then directory — is
/// load-bearing and matches what the index has always done: an ignored symlink
/// is reported as `ignored` rather than resolved, so a walk never canonicalizes
/// a path the ignore rules already excluded.
///
/// Entries are visited in sorted order, which is what makes a listing stable
/// across runs rather than dependent on directory iteration order.
pub fn walk(
    root: &Path,
    directory: &Path,
    relative_directory: &str,
    inherited_rules: &[IgnoreRule],
    visitor: &mut dyn FnMut(&WalkEvent) -> Result<()>,
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
            visitor(&WalkEvent::Skipped(WalkSkip {
                path: relative_path,
                reason: "ignored".to_string(),
            }))?;
            continue;
        }

        if file_type.is_symlink() {
            let resolved = fs::canonicalize(entry.path())?;
            if !resolved.starts_with(root) {
                visitor(&WalkEvent::Skipped(WalkSkip {
                    path: relative_path,
                    reason: "symlink_outside_root".to_string(),
                }))?;
                continue;
            }
        }

        if is_directory {
            walk(root, &entry.path(), &relative_path, &rules, visitor)?;
            continue;
        }

        if !file_type.is_file() {
            visitor(&WalkEvent::Skipped(WalkSkip {
                path: relative_path,
                reason: "not_regular_file".to_string(),
            }))?;
            continue;
        }

        visitor(&WalkEvent::File(WalkFile {
            relative_path,
            absolute_path: entry.path(),
        }))?;
    }
    Ok(())
}

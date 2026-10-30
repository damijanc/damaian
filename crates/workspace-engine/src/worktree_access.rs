//! Which git worktrees a read tool may reach, per
//! `docs/specs/60_worktree_aware_read_tools/proposal.md` §5.1.
//!
//! A worktree is reachable only when git itself lists it for this repository,
//! compared by canonical path. Directories under `.claude/worktrees/` or
//! `.damaian/worktrees/` are repository content, and a clone can ship a symlink
//! there; registration lives in `.git/worktrees/`, which is never cloned. So
//! nothing here trusts a path for where it is, only for what git says it is.
//!
//! The reachable set is computed per call, not cached, so a worktree created or
//! removed mid-turn is seen correctly.

use crate::error::{ClientError, Result};
use crate::git_service::GitService;
use crate::path_policy::PathPolicy;
use std::ffi::OsStr;
use std::fmt;
use std::fs;
use std::os::unix::ffi::OsStrExt;
use std::path::{Path, PathBuf};

/// One record of `git worktree list --porcelain -z`.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct WorktreeEntry {
    /// The path as git listed it, not yet canonicalized.
    pub path: PathBuf,
    pub head: Option<String>,
    /// The branch name with `refs/heads/` stripped; `None` when detached or bare.
    pub branch: Option<String>,
    pub bare: bool,
    pub detached: bool,
    pub locked: bool,
    pub prunable: bool,
}

/// Why a listed worktree is not reachable. Named in the refusal, so the model
/// can correct itself and a user can tell the cases apart.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WorktreeRefusal {
    /// The session's own checkout, addressed by omitting `worktree`.
    OwnCheckout,
    /// A bare repository entry has no working tree to read.
    Bare,
    /// Git would prune it: its administrative files or its directory are gone.
    Prunable,
    /// Its listed path does not resolve to a directory.
    Missing,
    /// Neither inside the repository root nor inside an `allowed_roots` entry.
    OutsideAllowedRoots,
}

impl fmt::Display for WorktreeRefusal {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::OwnCheckout => "is this session's own checkout; omit `worktree` to read it",
            Self::Bare => "is a bare repository with no working tree",
            Self::Prunable => "is prunable: git no longer considers it a valid worktree",
            Self::Missing => "does not exist on disk",
            Self::OutsideAllowedRoots => {
                "is outside the repository root and no `allowed_roots` entry contains it"
            }
        })
    }
}

/// A listed worktree with its reachability decided.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ClassifiedWorktree {
    pub entry: WorktreeEntry,
    /// `None` when the listed path does not canonicalize.
    pub canonical: Option<PathBuf>,
    /// Repository-relative when inside the repository root, else absolute.
    /// What `list_worktrees` shows and what a `worktree` argument may repeat.
    pub display: String,
    pub refusal: Option<WorktreeRefusal>,
}

/// Parses `git worktree list --porcelain -z`.
///
/// `-z` terminates every field with NUL and ends a record with an empty field,
/// so a path or lock reason containing a newline cannot forge another record.
/// Unknown fields are ignored, as git's documentation asks of porcelain readers.
pub fn parse_worktree_list(raw: &[u8]) -> Vec<WorktreeEntry> {
    let mut entries = Vec::new();
    let mut current: Option<WorktreeEntry> = None;
    for field in raw.split(|byte| *byte == 0) {
        if field.is_empty() {
            entries.extend(current.take());
            continue;
        }
        let (key, value) = match field.iter().position(|byte| *byte == b' ') {
            Some(space) => (&field[..space], Some(&field[space + 1..])),
            None => (field, None),
        };
        if key == b"worktree" {
            entries.extend(current.take());
            current = Some(WorktreeEntry {
                path: PathBuf::from(OsStr::from_bytes(value.unwrap_or_default())),
                ..WorktreeEntry::default()
            });
            continue;
        }
        let Some(entry) = current.as_mut() else {
            continue;
        };
        let text = || String::from_utf8_lossy(value.unwrap_or_default()).to_string();
        match key {
            b"HEAD" => entry.head = Some(text()),
            b"branch" => {
                let branch = text();
                entry.branch = Some(
                    branch
                        .strip_prefix("refs/heads/")
                        .map(str::to_string)
                        .unwrap_or(branch),
                );
            }
            b"bare" => entry.bare = true,
            b"detached" => entry.detached = true,
            b"locked" => entry.locked = true,
            b"prunable" => entry.prunable = true,
            _ => {}
        }
    }
    entries.extend(current);
    entries
}

/// Decides, for each listed worktree, whether a read tool may reach it from
/// `repository_root`, the session's own checkout.
///
/// Checked in this order, first match wins: bare, prunable, missing, own
/// checkout, outside. A worktree is inside when its canonical path is inside
/// the canonical repository root or a canonical `allowed_roots` entry, and
/// `PathPolicy::canonical_root` must accept it either way.
pub fn classify_worktrees(
    policy: &PathPolicy,
    repository_root: &Path,
    entries: Vec<WorktreeEntry>,
) -> Result<Vec<ClassifiedWorktree>> {
    let root = fs::canonicalize(repository_root)?;
    Ok(entries
        .into_iter()
        .map(|entry| {
            let canonical = fs::canonicalize(&entry.path)
                .ok()
                .filter(|path| path.is_dir());
            let refusal = if entry.bare {
                Some(WorktreeRefusal::Bare)
            } else if entry.prunable {
                Some(WorktreeRefusal::Prunable)
            } else {
                match canonical.as_deref() {
                    None => Some(WorktreeRefusal::Missing),
                    Some(path) if path == root => Some(WorktreeRefusal::OwnCheckout),
                    Some(path)
                        if !(path.starts_with(&root) || policy.is_inside_allowed_root(path))
                            || policy.canonical_root(path).is_err() =>
                    {
                        Some(WorktreeRefusal::OutsideAllowedRoots)
                    }
                    Some(_) => None,
                }
            };
            let shown = canonical.as_deref().unwrap_or(&entry.path);
            let display = match shown.strip_prefix(&root) {
                Ok(relative) if !relative.as_os_str().is_empty() => {
                    relative.to_string_lossy().to_string()
                }
                _ => shown.to_string_lossy().to_string(),
            };
            ClassifiedWorktree {
                entry,
                canonical,
                display,
                refusal,
            }
        })
        .collect())
}

/// The registered worktrees an edit from `repository_root` must not reach, as
/// (canonical path, display) pairs: every one but the session's own checkout
/// and any that contains it. A worker's session is itself a linked worktree
/// inside the main checkout, which must not make its own files foreign
/// (spec 76).
pub fn foreign_worktrees(
    git: &GitService,
    policy: &PathPolicy,
    repository_root: &Path,
) -> Result<Vec<(PathBuf, String)>> {
    let root = fs::canonicalize(repository_root)?;
    let entries = git.worktrees(repository_root)?;
    Ok(classify_worktrees(policy, repository_root, entries)?
        .into_iter()
        .filter_map(|worktree| {
            let canonical = worktree.canonical?;
            (canonical != root && !root.starts_with(&canonical))
                .then_some((canonical, worktree.display))
        })
        .collect())
}

/// Resolves a `worktree` argument to a reachable worktree, or refuses it.
///
/// The argument is canonicalized, absolute or relative to `repository_root`,
/// and must equal a reachable worktree's canonical path exactly. Every refusal
/// lists the reachable worktrees so the model can correct itself in the same
/// round, and names the reason when the argument is a registered worktree that
/// is not reachable.
pub fn resolve_worktree(
    git: &GitService,
    policy: &PathPolicy,
    repository_root: &Path,
    requested: &str,
) -> Result<ClassifiedWorktree> {
    let entries = git.worktrees(repository_root)?;
    let classified = classify_worktrees(policy, repository_root, entries)?;
    let candidate = Path::new(requested);
    let candidate = if candidate.is_absolute() {
        candidate.to_path_buf()
    } else {
        repository_root.join(candidate)
    };
    let reason = match fs::canonicalize(&candidate) {
        Err(_) => "does not exist".to_string(),
        Ok(canonical) => {
            match classified
                .iter()
                .find(|worktree| worktree.canonical.as_deref() == Some(canonical.as_path()))
            {
                Some(worktree) => match worktree.refusal {
                    None => return Ok(worktree.clone()),
                    Some(refusal) => refusal.to_string(),
                },
                None => "is not a registered worktree of this repository".to_string(),
            }
        }
    };
    let reachable = classified
        .iter()
        .filter(|worktree| worktree.refusal.is_none())
        .map(|worktree| worktree.display.as_str())
        .collect::<Vec<_>>();
    let listing = if reachable.is_empty() {
        "there are no reachable worktrees".to_string()
    } else {
        format!("reachable worktrees: {}", reachable.join(", "))
    };
    Err(ClientError::AccessDenied(format!(
        "Worktree `{requested}` {reason}; {listing}"
    )))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn record(fields: &[&[u8]]) -> Vec<u8> {
        let mut raw = Vec::new();
        for field in fields {
            raw.extend_from_slice(field);
            raw.push(0);
        }
        raw.push(0);
        raw
    }

    #[test]
    fn worktree_access_parses_main_and_linked_entries() {
        let mut raw = record(&[b"worktree /repo", b"HEAD aaaa", b"branch refs/heads/main"]);
        raw.extend(record(&[
            b"worktree /repo/.claude/worktrees/a",
            b"HEAD bbbb",
            b"branch refs/heads/wt-a",
        ]));
        let entries = parse_worktree_list(&raw);
        assert_eq!(entries.len(), 2);
        assert_eq!(entries[0].path, PathBuf::from("/repo"));
        assert_eq!(entries[0].branch.as_deref(), Some("main"));
        assert_eq!(entries[1].head.as_deref(), Some("bbbb"));
        assert_eq!(entries[1].branch.as_deref(), Some("wt-a"));
    }

    #[test]
    fn worktree_access_parses_detached_locked_and_prunable() {
        let raw = record(&[
            b"worktree /repo/x",
            b"HEAD cccc",
            b"detached",
            b"locked on a removable drive",
            b"prunable gitdir file points to non-existent location",
        ]);
        let entry = &parse_worktree_list(&raw)[0];
        assert!(entry.detached && entry.locked && entry.prunable);
        assert_eq!(entry.branch, None);
    }

    #[test]
    fn worktree_access_parses_a_bare_entry() {
        let raw = record(&[b"worktree /repo.git", b"bare"]);
        let entry = &parse_worktree_list(&raw)[0];
        assert!(entry.bare);
        assert_eq!(entry.head, None);
    }

    #[test]
    fn worktree_access_keeps_a_newline_inside_a_path() {
        // Without `-z` this path would read as two lines, the second a forged
        // `worktree` field.
        let raw = record(&[
            b"worktree /repo/odd\nworktree /etc",
            b"HEAD dddd",
            b"detached",
        ]);
        let entries = parse_worktree_list(&raw);
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].path, PathBuf::from("/repo/odd\nworktree /etc"));
    }

    #[test]
    fn worktree_access_ignores_unknown_fields_and_empty_input() {
        assert!(parse_worktree_list(b"").is_empty());
        let raw = record(&[b"worktree /repo", b"future-field value", b"HEAD eeee"]);
        let entries = parse_worktree_list(&raw);
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].head.as_deref(), Some("eeee"));
    }
}

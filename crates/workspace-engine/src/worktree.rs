//! Worker worktrees (spec 57 §5.1): `<repo>/.damaian/worktrees/<slug>` on a
//! new `damaian/<slug>` branch, and their removal.
//!
//! `.damaian/` is repository-controlled (#34): a cloned repository can ship a
//! symlink or a tracked file there. So every check runs before the first
//! mutation, and each path component is examined with `symlink_metadata`;
//! `canonicalize` alone would follow the very link the check exists to catch.
//! Every git command goes through `checkpoint::run_user_git` and is recorded
//! exactly as it ran.

use crate::checkpoint::{run_user_git, user_git_command_line};
use crate::error::{ClientError, Result};
use std::collections::BTreeSet;
use std::fs;
use std::io::{ErrorKind, Write};
use std::path::{Path, PathBuf};

const SLUG_MAX_LEN: usize = 40;
const WORKTREES_DIR: [&str; 2] = [".damaian", "worktrees"];
const EXCLUDE_LINE: &str = ".damaian/worktrees/";
const BRANCH_PREFIX: &str = "damaian/";

/// Lowercase ASCII words of the prompt's first non-blank line, joined by `-`,
/// at most 40 characters; `worker` when nothing is left. Non-ASCII characters
/// are dropped rather than treated as separators.
pub fn worker_slug(prompt: &str) -> String {
    let line = prompt
        .lines()
        .find(|line| !line.trim().is_empty())
        .unwrap_or_default();
    let mut words: Vec<String> = vec![String::new()];
    for c in line.chars() {
        if c.is_ascii_alphanumeric() {
            words.last_mut().unwrap().push(c.to_ascii_lowercase());
        } else if c.is_ascii() && !words.last().unwrap().is_empty() {
            words.push(String::new());
        }
    }
    let mut slug = String::new();
    for word in words.iter().filter(|word| !word.is_empty()) {
        if slug.is_empty() {
            slug = word.chars().take(SLUG_MAX_LEN).collect();
        } else if slug.len() + 1 + word.len() <= SLUG_MAX_LEN {
            slug.push('-');
            slug.push_str(word);
        } else {
            break;
        }
    }
    if slug.is_empty() {
        "worker".to_string()
    } else {
        slug
    }
}

/// The source tree's uncommitted files, for the New worker dialog's warning.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SourceChanges {
    /// Tracked changes and untracked, non-ignored files; `mentioned` first.
    pub paths: Vec<String>,
    /// The paths the prompt names, in the order it first names them.
    pub mentioned: Vec<String>,
}

pub fn source_changes(repo: &Path, prompt: &str) -> Result<SourceChanges> {
    let mut git = Git::new(repo);
    let all = git.status_paths()?;
    let mut mentioned: Vec<(usize, String)> = all
        .iter()
        .filter_map(|path| first_mention(prompt, path).map(|at| (at, path.clone())))
        .collect();
    mentioned.sort();
    let mentioned: Vec<String> = mentioned.into_iter().map(|(_, path)| path).collect();
    let paths = mentioned
        .iter()
        .cloned()
        .chain(all.into_iter().filter(|path| !mentioned.contains(path)))
        .collect();
    Ok(SourceChanges { paths, mentioned })
}

/// Where `path` first appears in `prompt` as a whole path, so that `a.txt` is
/// not found inside `data.txt`. A trailing `.` that ends a sentence is allowed.
fn first_mention(prompt: &str, path: &str) -> Option<usize> {
    let is_path_char = |c: char| c.is_ascii_alphanumeric() || matches!(c, '_' | '-' | '/');
    prompt.match_indices(path).map(|(at, _)| at).find(|&at| {
        let before = prompt[..at].chars().next_back();
        let mut after = prompt[at + path.len()..].chars();
        let starts_clean = before.is_none_or(|c| !is_path_char(c) && c != '.');
        let ends_clean = match after.next() {
            None => true,
            Some('.') => after.next().is_none_or(|c| !c.is_ascii_alphanumeric()),
            Some(c) => !is_path_char(c),
        };
        starts_clean && ends_clean
    })
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CreatedWorktree {
    pub slug: String,
    pub path: PathBuf,
    pub branch: String,
    pub base_commit: String,
    /// Every git command run, in order, exactly as executed.
    pub git_commands: Vec<String>,
}

/// Creates a worker's worktree, following §5.1's steps 1–6. Every refusal
/// happens before the first change to the repository.
pub fn create_worker_worktree(
    repo: &Path,
    prompt: &str,
    include_uncommitted: bool,
) -> Result<CreatedWorktree> {
    // 1. The source must be a repository's top level.
    let repo = canonical_repository(repo)?;
    let mut git = Git::new(&repo);
    require_top_level(&mut git)?;

    // 2. The base. `git stash create` writes a commit object for tracked
    // changes and touches neither the working tree nor the index.
    let stash = if include_uncommitted {
        git.text(&["stash", "create"])?
    } else {
        String::new()
    };
    let base_commit = if stash.is_empty() {
        git.text(&["rev-parse", "--verify", "HEAD^{commit}"])?
    } else {
        stash
    };

    // Naming: the first suffix free as both a branch and a path.
    let branches: BTreeSet<String> = git
        .text(&[
            "for-each-ref",
            "--format=%(refname)",
            &format!("refs/heads/{BRANCH_PREFIX}"),
        ])?
        .lines()
        .map(str::to_string)
        .collect();
    let base_slug = worker_slug(prompt);
    let worktrees = WORKTREES_DIR
        .iter()
        .fold(repo.clone(), |path, part| path.join(part));
    let slug = (1..)
        .map(|n| match n {
            1 => base_slug.clone(),
            n => format!("{base_slug}-{n}"),
        })
        .find(|slug| {
            !branches.contains(&format!("refs/heads/{BRANCH_PREFIX}{slug}"))
                && fs::symlink_metadata(worktrees.join(slug)).is_err()
        })
        .expect("an unbounded suffix search always finds a free name");
    let branch = format!("{BRANCH_PREFIX}{slug}");

    // 3. The location, which the repository controls.
    let path = checked_worktree_location(&repo, &slug)?;
    let relative = format!("{EXCLUDE_LINE}{slug}");
    if !git.run(&["ls-files", "-z", "--", &relative])?.is_empty() {
        return Err(ClientError::AccessDenied(format!(
            "the repository tracks files under {relative}; refusing to create a worktree there"
        )));
    }
    ensure_inside(&repo, &path)?;

    // 4. Keep worktrees out of the source's status, locally and never in
    // `.gitignore`.
    let exclude = git.text(&["rev-parse", "--git-path", "info/exclude"])?;
    ensure_exclude_line(&repo.join(exclude))?;

    // 5.
    let path_text = path.to_string_lossy().to_string();
    git.run(&["worktree", "add", "-b", &branch, &path_text, &base_commit])?;

    // 6. `git stash create` does not capture untracked files.
    if include_uncommitted {
        let untracked = git.run(&["ls-files", "--others", "--exclude-standard", "-z"])?;
        for relative in nul_separated(&untracked) {
            copy_untracked(&repo, &path, &relative)?;
        }
    }

    Ok(CreatedWorktree {
        slug,
        path,
        branch,
        base_commit,
        git_commands: git.commands,
    })
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RemoveOutcome {
    Removed,
    /// Nothing was removed. The caller confirms these paths with the user,
    /// then calls again with `force_after_confirmation`.
    Dirty {
        paths: Vec<String>,
    },
    /// The worktree was already removed outside Damaian; git's stale record
    /// of it was pruned (spec 87).
    AlreadyGone,
}

/// `git worktree remove`, refusing anything but a worker worktree of `repo`.
/// `--force` is passed only for a dirty worktree, and only once the user has
/// confirmed its paths (#36). The branch is never deleted.
pub fn remove_worker_worktree(
    repo: &Path,
    worktree: &Path,
    force_after_confirmation: bool,
) -> Result<RemoveOutcome> {
    let repo = canonical_repository(repo)?;
    require_top_level(&mut Git::new(&repo))?;
    if worker_worktree_is_gone(&repo, worktree) {
        // Removed outside Damaian. Refusing would leave a removal that can
        // never succeed; pruning drops only git's record of a worktree that
        // is no longer there (spec 87).
        Git::new(&repo).run(&["worktree", "prune"])?;
        return Ok(RemoveOutcome::AlreadyGone);
    }
    let path = registered_worker_worktree(&repo, worktree)?;

    let paths = Git::new(&path).status_paths()?;
    if !paths.is_empty() && !force_after_confirmation {
        return Ok(RemoveOutcome::Dirty { paths });
    }
    let path_text = path.to_string_lossy().to_string();
    let mut args = vec!["worktree", "remove"];
    if !paths.is_empty() {
        args.push("--force");
    }
    args.push(&path_text);
    Git::new(&repo).run(&args)?;
    Ok(RemoveOutcome::Removed)
}

/// Records each command, then runs it through the one git runner.
struct Git {
    root: PathBuf,
    commands: Vec<String>,
}

impl Git {
    fn new(root: &Path) -> Self {
        Self {
            root: root.to_path_buf(),
            commands: Vec::new(),
        }
    }

    fn run(&mut self, args: &[&str]) -> Result<Vec<u8>> {
        self.commands.push(user_git_command_line(&self.root, args));
        run_user_git(&self.root, args)
    }

    fn text(&mut self, args: &[&str]) -> Result<String> {
        let output = self.run(args)?;
        Ok(String::from_utf8_lossy(&output).trim().to_string())
    }

    /// Tracked changes and untracked, non-ignored files, in git's order. A
    /// rename lists its new path only.
    fn status_paths(&mut self) -> Result<Vec<String>> {
        let output = self.run(&["status", "--porcelain=v1", "--untracked-files=all", "-z"])?;
        let mut entries = nul_separated(&output).into_iter();
        let mut paths = Vec::new();
        while let Some(entry) = entries.next() {
            let (code, path) = entry.split_at(entry.len().min(3));
            if code.starts_with('R') || code.starts_with('C') {
                // `-z` puts a rename's original path in the next entry.
                entries.next();
            }
            paths.push(path.to_string());
        }
        Ok(paths)
    }
}

fn nul_separated(output: &[u8]) -> Vec<String> {
    output
        .split(|byte| *byte == 0)
        .filter(|entry| !entry.is_empty())
        .map(|entry| String::from_utf8_lossy(entry).to_string())
        .collect()
}

fn not_a_repository(repo: &Path, error: ClientError) -> ClientError {
    ClientError::InvalidInput(format!(
        "{} is not a git repository: {error}",
        repo.display()
    ))
}

fn canonical_repository(repo: &Path) -> Result<PathBuf> {
    fs::canonicalize(repo).map_err(|error| {
        ClientError::InvalidInput(format!("{} cannot be resolved: {error}", repo.display()))
    })
}

/// The repository must be its own top level: from a subdirectory,
/// `.damaian/worktrees` would land somewhere git does not expect.
fn require_top_level(git: &mut Git) -> Result<()> {
    let top_level = git
        .text(&["rev-parse", "--show-toplevel"])
        .map_err(|error| not_a_repository(&git.root, error))?;
    if fs::canonicalize(&top_level).ok().as_deref() != Some(git.root.as_path()) {
        return Err(ClientError::InvalidInput(format!(
            "{} is inside the repository at {top_level}, not its top level",
            git.root.display()
        )));
    }
    Ok(())
}

/// §5.1 step 3's component checks, with `symlink_metadata` at each level:
/// `.damaian` and `worktrees` may be absent or real directories, never
/// symlinks or files, and `<slug>` must not exist at all.
fn checked_worktree_location(repo: &Path, slug: &str) -> Result<PathBuf> {
    let mut path = repo.to_path_buf();
    let components = WORKTREES_DIR.iter().copied().chain([slug]);
    let last = WORKTREES_DIR.len();
    for (index, component) in components.enumerate() {
        path.push(component);
        let metadata = match fs::symlink_metadata(&path) {
            Ok(metadata) => metadata,
            // Nothing exists below a missing component, so nothing there can
            // be a link. The loop still builds the full path.
            Err(error) if error.kind() == ErrorKind::NotFound => continue,
            Err(error) => return Err(error.into()),
        };
        let refusal = if metadata.file_type().is_symlink() {
            "is a symlink"
        } else if index == last {
            "already exists"
        } else if !metadata.is_dir() {
            "is not a directory"
        } else {
            continue;
        };
        return Err(ClientError::AccessDenied(format!(
            "{} {refusal}; refusing to create a worker worktree there",
            path.display()
        )));
    }
    Ok(path)
}

/// Resolves the deepest existing ancestor of `path` and requires it inside
/// `repo`. Behind the component checks this is defence in depth.
fn ensure_inside(repo: &Path, path: &Path) -> Result<()> {
    let existing = path
        .ancestors()
        .find(|ancestor| fs::symlink_metadata(ancestor).is_ok())
        .unwrap_or(path);
    let resolved = fs::canonicalize(existing)?;
    if !resolved.starts_with(repo) {
        return Err(ClientError::AccessDenied(format!(
            "{} resolves to {}, outside the repository",
            path.display(),
            resolved.display()
        )));
    }
    Ok(())
}

fn ensure_exclude_line(exclude: &Path) -> Result<()> {
    let current = match fs::read_to_string(exclude) {
        Ok(current) => current,
        Err(error) if error.kind() == ErrorKind::NotFound => String::new(),
        Err(error) => return Err(error.into()),
    };
    if current.lines().any(|line| line.trim() == EXCLUDE_LINE) {
        return Ok(());
    }
    if let Some(parent) = exclude.parent() {
        fs::create_dir_all(parent)?;
    }
    let mut file = fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(exclude)?;
    let separator = if current.is_empty() || current.ends_with('\n') {
        ""
    } else {
        "\n"
    };
    writeln!(file, "{separator}{EXCLUDE_LINE}")?;
    Ok(())
}

/// Copies one untracked file into the same relative path in the worktree. A
/// symlink is recreated, not followed. The worktree's directories come from
/// the repository's own content, so none on the way may be a symlink either.
fn copy_untracked(repo: &Path, worktree: &Path, relative: &str) -> Result<()> {
    let source = repo.join(relative);
    let target = worktree.join(relative);
    let mut directory = worktree.to_path_buf();
    if let Some(parent) = Path::new(relative).parent() {
        for component in parent.components() {
            directory.push(component);
            match fs::symlink_metadata(&directory) {
                Ok(metadata) if metadata.file_type().is_symlink() || !metadata.is_dir() => {
                    return Err(ClientError::AccessDenied(format!(
                        "cannot copy untracked {relative}: {} in the worktree is not a directory",
                        directory.display()
                    )));
                }
                Ok(_) => {}
                Err(error) if error.kind() == ErrorKind::NotFound => {
                    fs::create_dir(&directory)?;
                }
                Err(error) => return Err(error.into()),
            }
        }
    }
    if fs::symlink_metadata(&source)?.file_type().is_symlink() {
        std::os::unix::fs::symlink(fs::read_link(&source)?, &target)?;
    } else {
        fs::copy(&source, &target)?;
    }
    Ok(())
}

/// The canonical path of `worktree` if it is a direct child of `repo`'s
/// `.damaian/worktrees`, reached through no symlink, and registered with git.
/// True when `worktree` would sit directly under the source's
/// `.damaian/worktrees/` but nothing is there any more. Any other missing path
/// stays a refusal in [`registered_worker_worktree`].
fn worker_worktree_is_gone(repo: &Path, worktree: &Path) -> bool {
    if fs::symlink_metadata(worktree).is_ok() {
        return false;
    }
    let expected = WORKTREES_DIR
        .iter()
        .fold(repo.to_path_buf(), |path, component| path.join(component));
    worktree.parent().is_some_and(|parent| {
        fs::canonicalize(parent).map_or(parent == expected, |parent| parent == expected)
    })
}

fn registered_worker_worktree(repo: &Path, worktree: &Path) -> Result<PathBuf> {
    let refuse = |reason: &str| {
        ClientError::AccessDenied(format!(
            "{} {reason}; only a worker worktree of {} can be removed",
            worktree.display(),
            repo.display()
        ))
    };
    let name = worktree
        .file_name()
        .and_then(|name| name.to_str())
        .ok_or_else(|| refuse("has no name"))?;
    let mut path = repo.to_path_buf();
    for component in WORKTREES_DIR.iter().copied().chain([name]) {
        path.push(component);
        let metadata = fs::symlink_metadata(&path).map_err(|_| refuse("does not exist"))?;
        if metadata.file_type().is_symlink() || !metadata.is_dir() {
            return Err(refuse("is not reached through real directories"));
        }
    }
    let given = fs::canonicalize(worktree).map_err(|_| refuse("does not exist"))?;
    if given != path {
        return Err(refuse("is not under .damaian/worktrees"));
    }
    let listed = Git::new(repo).run(&["worktree", "list", "--porcelain", "-z"])?;
    let registered = nul_separated(&listed)
        .iter()
        .filter_map(|line| line.strip_prefix("worktree "))
        .any(|listed| fs::canonicalize(listed).ok().as_deref() == Some(path.as_path()));
    if !registered {
        return Err(refuse("is not a registered worktree"));
    }
    Ok(path)
}

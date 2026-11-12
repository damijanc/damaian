//! The task-start tree a failure-time baseline runs in (spec 109 D1, D2).
//!
//! A baseline answers "did this check fail before the task changed anything?".
//! Since spec 109 it is taken only after a check fails, when the task's changes
//! are already in the user's checkout, so it cannot run there. It runs in a
//! temporary tree instead:
//!
//! 1. a shared clone of the repository at `HEAD` (alternates, no checkout of
//!    the user's files, no entry in the user's `.git`);
//! 2. every path the user's checkout has changed or untracked, copied as it is
//!    now, except the paths this task chain changed;
//! 3. every path this task chain changed, put back to what it held before the
//!    chain first changed it.
//!
//! Step 3 uses what the engine already recorded when the change happened, so
//! nothing is captured at task start: an applied patch's rollback snapshot
//! (its pre-apply bytes, which must match the base hash the patch was reviewed
//! against) and a turn checkpoint's snapshot (the pre-change bytes of a file a
//! patch or an approved command changed, which must match the hash recorded
//! with it). The earliest record per path is the task-start content.
//!
//! Anything that cannot be reconstructed exactly — a changed path with no
//! record, a record whose bytes do not match its hash, a command whose effects
//! the checkpoint did not cover, a repository that is not a Git checkout with a
//! commit, submodules — makes the tree [`TreeBuild::Unavailable`]. The caller
//! records that as `CheckBaseline::Unavailable`, which classifies as unknown
//! provenance rather than guessing.
//!
//! The user's checkout is only read: `git` runs with `--no-optional-locks`,
//! and every write goes to the temporary directory, which [`PreTaskTree`]
//! removes when it is dropped, on every path out — success, error and cancel.

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::{Component, Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, SystemTime};

use crate::checkpoint::{CheckpointStore, ObjectStore};
use crate::error::Result;
use crate::hash::{create_id, repository_id_for_root, sha256};
use crate::patch_engine::read_pre_apply_content;
use crate::session::SessionStore;
use crate::verification::VerificationEvent;

/// The prefix of every temporary tree's directory name, so a tree a crashed
/// process left behind can be recognised and removed later.
const TREE_PREFIX: &str = "damaian-baseline-";

/// A leftover tree older than this is removed by the next build. Younger ones
/// may belong to a baseline another Damaian process is running right now.
const STALE_TREE_AGE: Duration = Duration::from_secs(24 * 60 * 60);

/// A temporary task-start tree. Removed from disk when dropped.
#[derive(Debug)]
pub struct PreTaskTree {
    root: PathBuf,
}

impl PreTaskTree {
    /// The tree's root, the counterpart of the repository root.
    pub fn root(&self) -> &Path {
        &self.root
    }
}

impl Drop for PreTaskTree {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.root);
    }
}

/// What building a task-start tree came to.
#[derive(Debug)]
pub enum TreeBuild {
    Ready(PreTaskTree),
    /// The task-start state cannot be reconstructed exactly. The reason is for
    /// the audit log; it names paths but never content.
    Unavailable(String),
}

/// Where the task-start content of the chain's changed paths is recorded.
pub struct PreTaskSources<'a> {
    pub data_dir: &'a Path,
    pub session_store: &'a SessionStore,
    pub checkpoints: &'a CheckpointStore,
}

/// Build the task-start tree for the task chain `task_id` belongs to.
///
/// `Err` is an internal failure (the session log or the data directory could
/// not be read); every fact about the repository or the recorded state that
/// prevents an exact tree is `Ok(TreeBuild::Unavailable)`.
pub fn build_pre_task_tree(
    repository_root: &Path,
    sources: &PreTaskSources<'_>,
    session_id: &str,
    task_id: &str,
) -> Result<TreeBuild> {
    let contents = match task_start_contents(repository_root, sources, session_id, task_id)? {
        Ok(contents) => contents,
        Err(reason) => return Ok(TreeBuild::Unavailable(reason)),
    };
    let parent = tree_parent(sources.data_dir, repository_root);
    if fs::create_dir_all(&parent).is_err() {
        return Ok(TreeBuild::Unavailable(
            "the temporary tree's directory could not be created".to_string(),
        ));
    }
    remove_stale_trees(&parent);
    let tree = PreTaskTree {
        root: parent.join(format!("{TREE_PREFIX}{}", create_id("tree"))),
    };
    match populate(repository_root, sources.data_dir, &tree, &contents) {
        Ok(()) => Ok(TreeBuild::Ready(tree)),
        // `tree` is dropped here, so a half-built tree is removed too.
        Err(reason) => Ok(TreeBuild::Unavailable(reason)),
    }
}

/// Each path the chain changed, with the bytes it held before the chain first
/// changed it, or `None` when it did not exist.
type TaskStartContents = BTreeMap<String, Option<Vec<u8>>>;

/// The content every path the chain changed held before the chain first
/// changed it: `Some(bytes)`, or `None` for a path that did not exist.
fn task_start_contents(
    repository_root: &Path,
    sources: &PreTaskSources<'_>,
    session_id: &str,
    task_id: &str,
) -> Result<std::result::Result<TaskStartContents, String>> {
    let chain = sources
        .session_store
        .task_continuation_chain(session_id, task_id)?;

    // One record per source and path, the earliest of each kind.
    let mut from_patches: BTreeMap<String, (u128, Option<Vec<u8>>, bool)> = BTreeMap::new();
    for decision in sources
        .session_store
        .read_all_patch_file_decisions(session_id)?
    {
        if !decision.applied || !chain.contains(&decision.task_id) {
            continue;
        }
        for path in &decision.paths {
            let Some(snapshot) =
                read_pre_apply_content(sources.data_dir, &decision.patch_id, path)?
            else {
                return Ok(Err(format!(
                    "no pre-apply snapshot of {path} for patch {}",
                    decision.patch_id
                )));
            };
            let verified = snapshot.verified();
            let bytes = snapshot.content.map(String::into_bytes);
            match from_patches.get(path) {
                Some((at, _, _)) if *at <= snapshot.captured_at_ms => {}
                _ => {
                    from_patches.insert(path.clone(), (snapshot.captured_at_ms, bytes, verified));
                }
            }
        }
    }

    let mut command_paths = BTreeSet::new();
    for event in sources
        .session_store
        .read_verification_events(session_id, &[])?
    {
        // An approved command's `Due` names no patch and no checks (spec 85).
        // The answer-time completion due also names no patch, but it lists the
        // floor checks it must run and the task's changed paths, which are not
        // a command's effect.
        if let VerificationEvent::Due {
            task_id: due_task,
            patch_id,
            paths,
            checks,
        } = event
            && patch_id.is_empty()
            && checks.is_empty()
            && chain.contains(&due_task)
        {
            command_paths.extend(paths);
        }
    }

    let repository_id = repository_id_for_root(repository_root);
    // Oldest first, so the first entry found for a path is its earliest.
    let mut manifests = sources
        .checkpoints
        .list_checkpoints(&repository_id)?
        .into_iter()
        .filter(|manifest| {
            manifest
                .task_id
                .as_ref()
                .is_some_and(|task| chain.contains(task))
        })
        .collect::<Vec<_>>();
    manifests.reverse();
    if !command_paths.is_empty()
        && manifests
            .iter()
            .any(|manifest| !manifest.command_effects_covered)
    {
        return Ok(Err(
            "a checkpoint of this task did not cover what its commands changed".to_string(),
        ));
    }

    let touched = from_patches
        .keys()
        .cloned()
        .chain(command_paths.iter().cloned())
        .collect::<BTreeSet<_>>();
    let mut store = None;
    let mut contents = BTreeMap::new();
    for path in touched {
        let checkpointed = manifests.iter().find_map(|manifest| {
            manifest
                .files
                .iter()
                .find(|file| file.path == path)
                .map(|file| (manifest.created_at_ms, file))
        });
        let patched = from_patches.get(&path);
        // A checkpoint is taken when its turn starts, before any of that
        // turn's changes, so on a tie it is the older record.
        let use_checkpoint = match (checkpointed, patched) {
            (Some((at, _)), Some((patched_at, _, _))) => at <= *patched_at,
            (Some(_), None) => true,
            (None, _) => false,
        };
        if use_checkpoint {
            let Some((_, file)) = checkpointed else {
                unreachable!("use_checkpoint is only true with a checkpoint entry")
            };
            if !file.existed {
                contents.insert(path, None);
                continue;
            }
            let (Some(oid), Some(hash)) = (file.oid.as_deref(), file.hash.as_deref()) else {
                return Ok(Err(format!("the checkpoint of {path} holds no content")));
            };
            let store = match &store {
                Some(store) => store,
                None => store.insert(ObjectStore::open(sources.data_dir, &repository_id)?),
            };
            let Ok(bytes) = store.read_blob(oid) else {
                return Ok(Err(format!("the checkpoint of {path} could not be read")));
            };
            if sha256(&bytes) != hash {
                return Ok(Err(format!(
                    "the checkpoint of {path} does not match its recorded hash"
                )));
            }
            contents.insert(path, Some(bytes));
            continue;
        }
        if command_paths.contains(&path) && patched.is_none() {
            return Ok(Err(format!(
                "no record of {path} before a command of this task changed it"
            )));
        }
        let Some((_, bytes, verified)) = patched else {
            return Ok(Err(format!(
                "no record of {path} before this task changed it"
            )));
        };
        if command_paths.contains(&path) {
            // A command changed this path too, and no checkpoint holds it from
            // before the first patch, so the command may have come first.
            return Ok(Err(format!(
                "no checkpoint of {path} from before a command of this task changed it"
            )));
        }
        if !verified {
            return Ok(Err(format!(
                "the pre-apply snapshot of {path} does not match the hash its patch was reviewed against"
            )));
        }
        contents.insert(path, bytes.clone());
    }
    Ok(Ok(contents))
}

/// Clone, check out `HEAD`, and lay the user's other uncommitted state and the
/// task-start content over it.
fn populate(
    repository_root: &Path,
    data_dir: &Path,
    tree: &PreTaskTree,
    contents: &BTreeMap<String, Option<Vec<u8>>>,
) -> std::result::Result<(), String> {
    let common_dir = user_git(repository_root, &["rev-parse", "--git-common-dir"])
        .map_err(|_| "the repository is not a Git checkout".to_string())?;
    let common_dir = PathBuf::from(common_dir.trim());
    let common_dir = if common_dir.is_absolute() {
        common_dir
    } else {
        repository_root.join(common_dir)
    };
    // A repository with no commit yet has nothing to check out: every file is
    // untracked, so the overlay below is the whole tree.
    let head = user_git(repository_root, &["rev-parse", "--verify", "HEAD^{commit}"])
        .ok()
        .map(|head| head.trim().to_string());
    match head {
        Some(head) => {
            let spec = format!("{head}:.gitmodules");
            if user_git(repository_root, &["cat-file", "-e", &spec]).is_ok() {
                return Err(
                    "the repository has submodules, which are not reconstructed".to_string()
                );
            }
            // A shared clone borrows the user's objects through alternates and
            // writes nothing into the user's repository; `git worktree add`
            // would register the tree there, and a crash would leave that entry
            // behind. Hooks are disabled so cloning and checking out run nothing
            // of the user's.
            tree_git(
                None,
                &[
                    "clone",
                    "--shared",
                    "--no-checkout",
                    "--quiet",
                    &common_dir.to_string_lossy(),
                    &tree.root.to_string_lossy(),
                ],
            )?;
            tree_git(
                Some(&tree.root),
                &[
                    "-c",
                    "advice.detachedHead=false",
                    "checkout",
                    "--quiet",
                    "--detach",
                    &head,
                ],
            )?;
        }
        None => tree_git(None, &["init", "--quiet", &tree.root.to_string_lossy()])?,
    }

    let data_dir = fs::canonicalize(data_dir).unwrap_or_else(|_| data_dir.to_path_buf());
    let root = fs::canonicalize(repository_root).unwrap_or_else(|_| repository_root.to_path_buf());
    let own_data = data_dir
        .strip_prefix(&root)
        .ok()
        .map(|prefix| prefix.to_string_lossy().replace('\\', "/"));
    let status = user_git_bytes(
        repository_root,
        &["status", "--porcelain=v1", "--untracked-files=all", "-z"],
    )
    .map_err(|error| format!("git status failed: {error}"))?;
    for path in status_paths(&status) {
        if contents.contains_key(&path) {
            continue;
        }
        if own_data.as_deref().is_some_and(|prefix| {
            prefix.is_empty() || path == prefix || path.starts_with(&format!("{prefix}/"))
        }) {
            continue;
        }
        copy_current(repository_root, &tree.root, &path)?;
    }
    for (path, content) in contents {
        let target = tree_path(&tree.root, path)?;
        match content {
            Some(bytes) => {
                if let Some(parent) = target.parent() {
                    fs::create_dir_all(parent).map_err(|error| error.to_string())?;
                }
                // `fs::write` keeps an existing file's mode, which the checkout
                // set from `HEAD`.
                fs::write(&target, bytes).map_err(|error| error.to_string())?;
            }
            None => remove_path(&target)?,
        }
    }
    Ok(())
}

/// Copy one changed or untracked path from the user's checkout as it is now.
fn copy_current(
    repository_root: &Path,
    tree_root: &Path,
    path: &str,
) -> std::result::Result<(), String> {
    let source = tree_path(repository_root, path)?;
    let target = tree_path(tree_root, path)?;
    let metadata = match fs::symlink_metadata(&source) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return remove_path(&target),
        Err(error) => return Err(error.to_string()),
    };
    if metadata.is_dir() {
        // A nested repository shows up as a directory; its contents are not
        // this repository's to reconstruct.
        return Ok(());
    }
    remove_path(&target)?;
    if let Some(parent) = target.parent() {
        fs::create_dir_all(parent).map_err(|error| error.to_string())?;
    }
    if metadata.file_type().is_symlink() {
        let link = fs::read_link(&source).map_err(|error| error.to_string())?;
        std::os::unix::fs::symlink(link, &target).map_err(|error| error.to_string())?;
        return Ok(());
    }
    fs::copy(&source, &target).map_err(|error| error.to_string())?;
    Ok(())
}

fn remove_path(target: &Path) -> std::result::Result<(), String> {
    match fs::symlink_metadata(target) {
        Ok(metadata) if metadata.is_dir() => {
            fs::remove_dir_all(target).map_err(|error| error.to_string())
        }
        Ok(_) => fs::remove_file(target).map_err(|error| error.to_string()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error.to_string()),
    }
}

/// `root` joined with a repository-relative path that came from Git or the
/// session log. Anything that could leave `root` is refused.
fn tree_path(root: &Path, path: &str) -> std::result::Result<PathBuf, String> {
    let relative = Path::new(path);
    if path.is_empty()
        || relative
            .components()
            .any(|component| !matches!(component, Component::Normal(_)))
    {
        return Err(format!("unexpected path {path}"));
    }
    Ok(root.join(relative))
}

/// The paths `git status --porcelain=v1 -z` names, both sides of a rename.
fn status_paths(output: &[u8]) -> Vec<String> {
    let mut paths = Vec::new();
    let mut fields = output
        .split(|byte| *byte == 0)
        .filter(|field| !field.is_empty());
    while let Some(field) = fields.next() {
        let text = String::from_utf8_lossy(field);
        let Some((status, path)) = text.split_at_checked(3) else {
            continue;
        };
        if (status.starts_with('R') || status.starts_with('C'))
            && let Some(original) = fields.next()
        {
            paths.push(String::from_utf8_lossy(original).to_string());
        }
        paths.push(path.to_string());
    }
    paths.sort();
    paths.dedup();
    paths
}

/// Read-only `git` in the user's checkout: no optional locks, so `status`
/// does not refresh the user's index.
fn user_git(root: &Path, args: &[&str]) -> std::result::Result<String, String> {
    user_git_bytes(root, args).map(|bytes| String::from_utf8_lossy(&bytes).to_string())
}

fn user_git_bytes(root: &Path, args: &[&str]) -> std::result::Result<Vec<u8>, String> {
    let output = Command::new("git")
        .arg("--no-optional-locks")
        .arg("-C")
        .arg(root)
        .args(args)
        .stdin(Stdio::null())
        .output()
        .map_err(|error| error.to_string())?;
    if !output.status.success() {
        return Err(String::from_utf8_lossy(&output.stderr).trim().to_string());
    }
    Ok(output.stdout)
}

/// `git` for the temporary tree, with the user's hooks switched off.
fn tree_git(cwd: Option<&Path>, args: &[&str]) -> std::result::Result<(), String> {
    let mut command = Command::new("git");
    command.args(["-c", "core.hooksPath=/dev/null"]);
    if let Some(cwd) = cwd {
        command.arg("-C").arg(cwd);
    }
    let output = command
        .args(args)
        .stdin(Stdio::null())
        .output()
        .map_err(|error| error.to_string())?;
    if !output.status.success() {
        return Err(format!(
            "git {} failed: {}",
            args.iter()
                .find(|arg| !arg.starts_with('-') && !arg.contains('='))
                .copied()
                .unwrap_or_default(),
            String::from_utf8_lossy(&output.stderr).trim()
        ));
    }
    Ok(())
}

/// Where temporary trees are made: under Damaian's data directory, unless that
/// directory is inside the repository (a development setup), where a nested
/// tree would sit inside the user's checkout and a build tool looking upwards
/// for its workspace would find the user's. Then the system temporary
/// directory.
pub fn tree_parent(data_dir: &Path, repository_root: &Path) -> PathBuf {
    let data = fs::canonicalize(data_dir).unwrap_or_else(|_| data_dir.to_path_buf());
    let root = fs::canonicalize(repository_root).unwrap_or_else(|_| repository_root.to_path_buf());
    if data.starts_with(&root) {
        std::env::temp_dir().join("damaian-baselines")
    } else {
        data_dir.join("baselines").join("trees")
    }
}

/// Remove trees a crashed process left behind. Only old ones: a young tree may
/// be a baseline another Damaian process is running.
fn remove_stale_trees(parent: &Path) {
    let Ok(entries) = fs::read_dir(parent) else {
        return;
    };
    let now = SystemTime::now();
    for entry in entries.flatten() {
        if !entry.file_name().to_string_lossy().starts_with(TREE_PREFIX) {
            continue;
        }
        let old = entry
            .metadata()
            .and_then(|metadata| metadata.modified())
            .ok()
            .and_then(|modified| now.duration_since(modified).ok())
            .is_some_and(|age| age > STALE_TREE_AGE);
        if old {
            let _ = fs::remove_dir_all(entry.path());
        }
    }
}

/// The temporary trees on disk now for this data directory and repository.
/// For tests that prove a tree is always removed.
pub fn temporary_trees(data_dir: &Path, repository_root: &Path) -> Vec<PathBuf> {
    let Ok(entries) = fs::read_dir(tree_parent(data_dir, repository_root)) else {
        return Vec::new();
    };
    entries
        .flatten()
        .filter(|entry| entry.file_name().to_string_lossy().starts_with(TREE_PREFIX))
        .map(|entry| entry.path())
        .collect()
}

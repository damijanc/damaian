use std::path::{Path, PathBuf};
use std::process::Command;

use serde::Deserialize;
use workspace_engine::{ClientError, Result};

use crate::guard;

/// One materialized fixture: a temporary git repository plus the temporary data
/// directory the run against it should use.
pub struct Materialized {
    pub root: PathBuf,
    pub version: String,
    pub data_dir: PathBuf,
}

/// The committed `fixture.toml` beside a fixture's tree. Beyond `version` and
/// `description` (which every fixture carries), it may describe linked
/// worktrees and symlinks to plant after the baseline commit, since a fixture
/// cannot ship those as committed content — git will not track a nested `.git`
/// and a symlink would be dereferenced by the tree copy.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct FixtureSpec {
    version: String,
    #[serde(default)]
    #[allow(dead_code)]
    description: String,
    #[serde(default)]
    worktrees: Vec<WorktreeSpec>,
    #[serde(default)]
    symlinks: Vec<SymlinkSpec>,
}

/// One linked worktree to create with `git worktree add -b`, then mutate:
/// `dirty` files are written without a commit, `commits` files are written and
/// committed, in order, so a worktree can be both dirty and ahead.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct WorktreeSpec {
    /// Repository-relative, e.g. `.claude/worktrees/wt-a`.
    path: String,
    branch: String,
    #[serde(default)]
    dirty: Vec<FileChange>,
    #[serde(default)]
    commits: Vec<CommitSpec>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct FileChange {
    path: String,
    content: String,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct CommitSpec {
    file: String,
    content: String,
    message: String,
}

/// A symlink to plant under the repository root. `target = "@data"` names the
/// run's data directory, which is guaranteed to exist and to sit outside the
/// repository — the "outside" a planted symlink must point at.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct SymlinkSpec {
    path: String,
    target: String,
}

/// Where the committed fixture trees live. Resolved from `CARGO_MANIFEST_DIR`
/// so it works whether the harness is run as a binary or from a test.
pub fn fixtures_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("fixtures")
}

/// Copies `fixtures/<name>/` to a fresh temporary directory and turns it into a
/// git repository with one commit.
///
/// Fixtures cannot ship with a nested `.git` — git will not track one — so the
/// repository is built at run time. A fixed identity and a single commit mean
/// every run starts from an identical state (proposal §5.2).
pub fn materialize(name: &str) -> Result<Materialized> {
    let source = fixtures_dir().join(name);
    if !source.is_dir() {
        return Err(ClientError::InvalidInput(format!(
            "unknown fixture {name}: {} is not a directory",
            source.display()
        )));
    }
    let spec = read_spec(&source)?;

    let base = guard::eval_data_dir()?;
    let root = base.join("repo");
    let data_dir = base.join("data");
    std::fs::create_dir_all(&data_dir).map_err(io("create data dir"))?;
    copy_tree(&source, &root)?;
    // fixture.toml is harness metadata, not part of the repository under test.
    let _ = std::fs::remove_file(root.join("fixture.toml"));

    run_git(&root, &["init", "--quiet"])?;
    run_git(&root, &["config", "user.email", "eval@damaian.invalid"])?;
    run_git(&root, &["config", "user.name", "Damaian Eval"])?;
    // A worktree commit below must not try to GPG-sign, whatever the user's
    // global git config says.
    run_git(&root, &["config", "commit.gpgsign", "false"])?;
    run_git(&root, &["add", "."])?;
    run_git(&root, &["commit", "--quiet", "-m", "Fixture baseline"])?;

    for worktree in &spec.worktrees {
        let path = root.join(&worktree.path);
        let path_str = path.to_string_lossy().into_owned();
        run_git(
            &root,
            &[
                "worktree",
                "add",
                "-b",
                worktree.branch.as_str(),
                path_str.as_str(),
            ],
        )?;
        for change in &worktree.dirty {
            write_file(&path.join(&change.path), &change.content)?;
        }
        for commit in &worktree.commits {
            write_file(&path.join(&commit.file), &commit.content)?;
            run_git(&path, &["add", commit.file.as_str()])?;
            run_git(&path, &["commit", "--quiet", "-m", commit.message.as_str()])?;
        }
    }

    for symlink in &spec.symlinks {
        let target = match symlink.target.as_str() {
            "@data" => data_dir.clone(),
            other => PathBuf::from(other),
        };
        let link = root.join(&symlink.path);
        if let Some(parent) = link.parent() {
            std::fs::create_dir_all(parent).map_err(io("create symlink parent"))?;
        }
        std::os::unix::fs::symlink(&target, &link).map_err(io("create symlink"))?;
    }

    Ok(Materialized {
        root,
        version: spec.version,
        data_dir,
    })
}

fn read_spec(source: &Path) -> Result<FixtureSpec> {
    let path = source.join("fixture.toml");
    let text = std::fs::read_to_string(&path).map_err(io("read fixture.toml"))?;
    toml::from_str(&text)
        .map_err(|error| ClientError::InvalidInput(format!("{}: {error}", path.display())))
}

fn write_file(path: &Path, content: &str) -> Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(io("create file parent"))?;
    }
    std::fs::write(path, content).map_err(io("write file"))
}

fn copy_tree(source: &Path, destination: &Path) -> Result<()> {
    std::fs::create_dir_all(destination).map_err(io("create fixture copy"))?;
    for entry in std::fs::read_dir(source).map_err(io("read fixture dir"))? {
        let entry = entry.map_err(io("read fixture entry"))?;
        let target = destination.join(entry.file_name());
        if entry.path().is_dir() {
            copy_tree(&entry.path(), &target)?;
        } else {
            std::fs::copy(entry.path(), &target).map_err(io("copy fixture file"))?;
        }
    }
    Ok(())
}

fn run_git(repo: &Path, args: &[&str]) -> Result<()> {
    let output = Command::new("git")
        .arg("-C")
        .arg(repo)
        .args(args)
        .output()
        .map_err(io("run git"))?;
    if !output.status.success() {
        return Err(ClientError::Git(format!(
            "git {args:?} failed: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        )));
    }
    Ok(())
}

fn io(action: &'static str) -> impl Fn(std::io::Error) -> ClientError {
    move |error| ClientError::Io(format!("{action}: {error}"))
}

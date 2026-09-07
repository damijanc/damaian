use std::path::{Path, PathBuf};
use std::process::Command;

use workspace_engine::{ClientError, Result};

use crate::guard;

/// One materialized fixture: a temporary git repository plus the temporary data
/// directory the run against it should use.
pub struct Materialized {
    pub root: PathBuf,
    pub version: String,
    pub data_dir: PathBuf,
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
    let version = read_version(&source)?;

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
    run_git(&root, &["add", "."])?;
    run_git(&root, &["commit", "--quiet", "-m", "Fixture baseline"])?;

    Ok(Materialized {
        root,
        version,
        data_dir,
    })
}

fn read_version(source: &Path) -> Result<String> {
    let path = source.join("fixture.toml");
    let text = std::fs::read_to_string(&path).map_err(io("read fixture.toml"))?;
    let value: toml::Value = toml::from_str(&text)
        .map_err(|error| ClientError::InvalidInput(format!("{}: {error}", path.display())))?;
    value
        .get("version")
        .and_then(|version| version.as_str())
        .map(str::to_string)
        .ok_or_else(|| {
            ClientError::InvalidInput(format!("{} needs a string `version`", path.display()))
        })
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

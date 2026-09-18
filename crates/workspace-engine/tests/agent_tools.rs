//! Spec 47, requirements 1–4: the agent working floor.
//!
//! Every acceptance criterion in `docs/specs/47_agent_working_capability/proposal.md`
//! §6's first-slice group lives here.

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};
use workspace_engine::tree_walk::{self, WalkEvent};

static COUNTER: AtomicU64 = AtomicU64::new(1);

fn temp_dir(name: &str) -> PathBuf {
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("clock should work")
        .as_nanos();
    let dir = std::env::temp_dir().join(format!(
        "damaian-agent-tools-{name}-{now}-{}",
        COUNTER.fetch_add(1, Ordering::Relaxed)
    ));
    fs::create_dir_all(&dir).expect("temp dir should be created");
    dir
}

fn write_fixture(root: &Path, relative_path: &str, content: &str) {
    let path = root.join(relative_path);
    fs::create_dir_all(path.parent().expect("fixture should have parent")).unwrap();
    fs::write(path, content).unwrap();
}

/// The symlink-escape check is the reason the walk is shared rather than
/// reimplemented: a symlink that canonicalizes outside the root would otherwise
/// let a navigation tool read anything the user can.
#[test]
fn the_walk_rejects_a_symlink_that_escapes_the_root() {
    let repo = temp_dir("walk-symlink");
    write_fixture(&repo, "src/lib.rs", "pub fn a() {}\n");
    let outside = temp_dir("walk-outside");
    write_fixture(&outside, "secret.txt", "s3cret\n");
    std::os::unix::fs::symlink(&outside, repo.join("src/escape")).unwrap();

    let mut seen = Vec::new();
    let mut skipped = Vec::new();
    tree_walk::walk(&repo, &repo, "", &[], &mut |event| {
        match event {
            WalkEvent::File(file) => seen.push(file.relative_path.clone()),
            WalkEvent::Skipped(skip) => skipped.push((skip.path.clone(), skip.reason.clone())),
        }
        Ok(())
    })
    .unwrap();

    assert!(seen.contains(&"src/lib.rs".to_string()), "got {seen:?}");
    assert!(
        !seen.iter().any(|path| path.contains("escape")),
        "a symlink resolving outside the root must not be walked: {seen:?}"
    );
    assert_eq!(
        skipped,
        vec![("src/escape".to_string(), "symlink_outside_root".to_string())]
    );
}

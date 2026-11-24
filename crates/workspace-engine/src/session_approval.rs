//! **Approve for this session** (spec 109 D5): the session a turn runs for,
//! so the command policy can add that session's approvals as session rules
//! (spec 111 D2), and the before/after check of the repository around a run
//! one of them covers.
//!
//! The session is thread-scoped, as the turn's cancel token is
//! ([`crate::cancel::CancelToken::scope`]): the policy is shared by every
//! runner a turn reaches, and none of their signatures carries the session.
//! A classification outside any scope (the CLI, a settings route) sees no
//! session approvals, which only ever means it asks.

use crate::hash::sha256_hex;
use std::cell::RefCell;
use std::collections::BTreeMap;
use std::fs;
use std::path::Path;
use std::process::Command;

thread_local! {
    static SESSION: RefCell<Option<String>> = const { RefCell::new(None) };
    /// Revocations made in the scope, so the turn that made them can say so.
    static NOTICES: RefCell<Vec<String>> = const { RefCell::new(Vec::new()) };
}

/// Runs `work` as the turn of `session_id` on this thread, restoring the
/// previous session afterwards, on unwind too.
pub fn with_session<T>(session_id: &str, work: impl FnOnce() -> T) -> T {
    struct Restore(Option<String>);
    impl Drop for Restore {
        fn drop(&mut self) {
            let previous = self.0.take();
            SESSION.with(|current| *current.borrow_mut() = previous);
        }
    }
    let previous = SESSION.with(|current| current.borrow_mut().replace(session_id.to_string()));
    let _restore = Restore(previous);
    work()
}

/// The session whose turn runs on this thread, if one set it.
pub fn current_session() -> Option<String> {
    SESSION.with(|current| current.borrow().clone())
}

/// Records a revocation the running turn should report.
pub(crate) fn push_notice(notice: String) {
    NOTICES.with(|notices| notices.borrow_mut().push(notice));
}

/// Takes the revocations recorded on this thread since the last call.
pub fn take_notices() -> Vec<String> {
    NOTICES.with(|notices| std::mem::take(&mut *notices.borrow_mut()))
}

/// What the repository looks like to Git: `HEAD`, and each changed or
/// untracked path with its status and content hash. Git-ignored files (build
/// output such as `target/`) are not part of it, so a check that only writes
/// build output leaves it unchanged.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RepositoryState {
    head: String,
    paths: BTreeMap<String, String>,
}

impl RepositoryState {
    /// The state of the Git checkout at `root`; `None` when it is not one or
    /// Git cannot read it, in which case nothing can be approved for the
    /// session there. Paths under `exclude` (Damaian's own data directory,
    /// when it lives inside the repository) are left out: Damaian writes its
    /// audit and session log there while the command runs.
    pub fn capture(root: &Path, exclude: Option<&Path>) -> Option<Self> {
        let excluded = exclude
            .and_then(|dir| dir.strip_prefix(root).ok())
            .map(|relative| relative.to_string_lossy().trim_end_matches('/').to_string())
            .filter(|relative| !relative.is_empty());
        let head = git(root, &["rev-parse", "--verify", "-q", "HEAD"])
            .map(|out| String::from_utf8_lossy(&out).trim().to_string())
            .unwrap_or_default();
        let status = git(
            root,
            &["status", "--porcelain=v1", "-z", "--untracked-files=all"],
        )?;
        let mut paths = BTreeMap::new();
        let mut entries = status.split(|byte| *byte == 0).filter(|e| !e.is_empty());
        while let Some(entry) = entries.next() {
            if entry.len() < 4 {
                continue;
            }
            let code = String::from_utf8_lossy(&entry[..2]).to_string();
            let path = String::from_utf8_lossy(&entry[3..]).to_string();
            // A rename or copy is followed by its source path.
            if code.starts_with('R') || code.starts_with('C') {
                entries.next();
            }
            if let Some(excluded) = &excluded
                && (path == *excluded || path.starts_with(&format!("{excluded}/")))
            {
                continue;
            }
            let content = fs::read(root.join(&path))
                .map(|bytes| sha256_hex(&bytes))
                .unwrap_or_else(|_| "missing".to_string());
            paths.insert(path, format!("{code} {content}"));
        }
        Some(Self { head, paths })
    }

    /// The paths that differ between `self` (before) and `after`, sorted;
    /// `HEAD` names a moved branch tip.
    pub fn changed_since(&self, after: &Self) -> Vec<String> {
        let mut changed = Vec::new();
        if self.head != after.head {
            changed.push("HEAD".to_string());
        }
        for (path, state) in &self.paths {
            if after.paths.get(path) != Some(state) {
                changed.push(path.clone());
            }
        }
        for path in after.paths.keys() {
            if !self.paths.contains_key(path) {
                changed.push(path.clone());
            }
        }
        changed.sort();
        changed.dedup();
        changed
    }
}

/// `git -C root <args>` without taking Git's optional locks, so capturing
/// the state never rewrites the user's index.
fn git(root: &Path, args: &[&str]) -> Option<Vec<u8>> {
    let output = Command::new("git")
        .arg("--no-optional-locks")
        .arg("-C")
        .arg(root)
        .args(args)
        .output()
        .ok()?;
    output.status.success().then_some(output.stdout)
}

/// Whether `root` is the top of a Git checkout's work tree, or inside one.
pub fn is_git_checkout(root: &Path) -> bool {
    git(root, &["rev-parse", "--is-inside-work-tree"])
        .is_some_and(|out| String::from_utf8_lossy(&out).trim() == "true")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_session_is_current_only_inside_its_scope_and_nests() {
        assert_eq!(current_session(), None);
        with_session("a", || {
            assert_eq!(current_session().as_deref(), Some("a"));
            with_session("b", || assert_eq!(current_session().as_deref(), Some("b")));
            assert_eq!(current_session().as_deref(), Some("a"));
        });
        assert_eq!(current_session(), None);
        let unwound = std::panic::catch_unwind(|| with_session("c", || panic!("turn failed")));
        assert!(unwound.is_err());
        assert_eq!(current_session(), None);
    }

    #[test]
    fn the_state_ignores_ignored_output_and_sees_every_other_change() {
        let dir = std::env::temp_dir().join(format!(
            "damaian-session-approval-{}",
            crate::hash::create_id("t")
        ));
        fs::create_dir_all(&dir).unwrap();
        let root = dir.as_path();
        let run = |args: &[&str]| {
            assert!(
                Command::new("git")
                    .arg("-C")
                    .arg(root)
                    .args(args)
                    .output()
                    .unwrap()
                    .status
                    .success()
            );
        };
        run(&["init", "-q"]);
        fs::write(root.join(".gitignore"), "target/\n").unwrap();
        fs::write(root.join("a.txt"), "one").unwrap();
        run(&["add", "."]);
        run(&[
            "-c",
            "user.email=t@example.com",
            "-c",
            "user.name=t",
            "commit",
            "-qm",
            "init",
        ]);
        assert!(is_git_checkout(root));
        let before = RepositoryState::capture(root, None).unwrap();
        fs::create_dir_all(root.join("target")).unwrap();
        fs::write(root.join("target/out"), "built").unwrap();
        let after_build = RepositoryState::capture(root, None).unwrap();
        assert!(before.changed_since(&after_build).is_empty());

        fs::write(root.join("a.txt"), "two").unwrap();
        fs::write(root.join("new.txt"), "x").unwrap();
        let after_edit = RepositoryState::capture(root, None).unwrap();
        assert_eq!(
            before.changed_since(&after_edit),
            vec!["a.txt".to_string(), "new.txt".to_string()]
        );
        // A second edit of an already-changed file is a change too.
        fs::write(root.join("a.txt"), "three").unwrap();
        let again = RepositoryState::capture(root, None).unwrap();
        assert_eq!(after_edit.changed_since(&again), vec!["a.txt".to_string()]);
        assert!(!is_git_checkout(&root.join("target").join("nowhere")));
        // Damaian's own data directory inside the repository is not a change.
        let data = root.join(".damaian");
        let before = RepositoryState::capture(root, Some(&data)).unwrap();
        fs::create_dir_all(data.join("audit")).unwrap();
        fs::write(data.join("audit/events.jsonl"), "{}").unwrap();
        let after = RepositoryState::capture(root, Some(&data)).unwrap();
        assert!(before.changed_since(&after).is_empty());
        let _ = fs::remove_dir_all(&dir);
    }
}

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use workspace_engine::{ClientError, Result};

static COUNTER: AtomicU64 = AtomicU64::new(1);

/// Refuses any data directory that would put harness state in the user's real
/// Damaian data. Proposal §5.2: a run must never read or write it.
///
/// The check is on the path, before anything is created, because the damage is
/// done by the first write.
pub fn assert_safe_data_dir(dir: &Path) -> Result<()> {
    let Some(home) = std::env::var_os("HOME") else {
        return Err(ClientError::InvalidInput(
            "HOME is unset, so the harness cannot tell a temporary data directory from the real \
             one; refusing to run"
                .to_string(),
        ));
    };
    let forbidden = PathBuf::from(home).join("Library/Application Support");
    // Compared lexically rather than canonicalized: `dir` need not exist yet,
    // and `canonicalize` on a missing path is an error rather than an answer.
    if dir.starts_with(&forbidden) {
        return Err(ClientError::AccessDenied(format!(
            "refusing to run against {}: the harness must not touch the real data directory under \
             ~/Library/Application Support",
            dir.display()
        )));
    }
    Ok(())
}

/// A fresh, checked temporary data directory for one run.
///
/// The name carries the process id because nextest runs each test in its own
/// process, where `COUNTER` restarts at 1, and macOS's clock only has
/// microsecond resolution. And it is created with `create_dir`, not
/// `create_dir_all`, so a name that is somehow already taken is skipped rather
/// than silently shared with another run.
pub fn eval_data_dir() -> Result<PathBuf> {
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|error| ClientError::Io(format!("system clock is before the epoch: {error}")))?
        .as_nanos();
    let pid = std::process::id();
    let base = std::env::temp_dir();
    for _ in 0..100 {
        let dir = base.join(format!(
            "damaian-eval-{now}-{pid}-{}",
            COUNTER.fetch_add(1, Ordering::Relaxed)
        ));
        assert_safe_data_dir(&dir)?;
        match std::fs::create_dir(&dir) {
            Ok(()) => return Ok(dir),
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(error) => {
                return Err(ClientError::Io(format!(
                    "could not create {}: {error}",
                    dir.display()
                )));
            }
        }
    }
    Err(ClientError::Io(format!(
        "could not find an unused data directory name under {}",
        base.display()
    )))
}

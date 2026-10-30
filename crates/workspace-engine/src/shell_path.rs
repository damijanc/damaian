//! The `PATH` the user's interactive login shell ends up with, for commands
//! that run in a login shell that is not interactive (spec 69).
//!
//! `<shell> -lc` reads `~/.zprofile` but not `~/.zshrc`, which is where nvm,
//! pyenv and most version managers put their `PATH` setup, and a GUI launch
//! inherits launchd's minimal `PATH` rather than the Terminal's. So the
//! interactive shell is asked, once per shell per process, what its `PATH` is,
//! and only that value crosses: no alias, function or other variable does, so
//! the command policy's reading of a command's text still describes what runs.

use crate::process_registry::{ProcessKind, ProcessRegistry};
use std::collections::HashMap;
use std::io::Read;
use std::os::unix::process::CommandExt;
use std::path::Path;
use std::process::{Child, Command, Stdio};
use std::sync::mpsc::{self, RecvTimeoutError};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};

/// How long the interactive shell may take before it is killed (spec 69
/// requirement 4).
pub const RESOLVE_TIMEOUT: Duration = Duration::from_secs(5);

/// Set in the interactive shell's environment so a user can skip slow setup
/// in their rc files while Damaian asks for `PATH`.
pub const RESOLVING_VARIABLE: &str = "DAMAIAN_RESOLVING_SHELL_PATH";

/// Carries the resolved `PATH` to a command, so the `export` prefix can
/// restore it after the login profile has rewritten `PATH` (§5.2).
pub const PATH_VARIABLE: &str = "DAMAIAN_SHELL_PATH";

// Unlikely enough in rc output that the text between them is the `PATH`.
const START_MARKER: &str = "__DAMAIAN_SHELL_PATH_START_c41f__";
const END_MARKER: &str = "__DAMAIAN_SHELL_PATH_END_c41f__";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ResolutionStatus {
    Resolved,
    TimedOut,
    Failed,
    /// Not a POSIX-family shell: its `PATH` syntax differs (fish's is a
    /// list), so it is not asked and commands run as before.
    Unsupported,
}

impl ResolutionStatus {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Resolved => "resolved",
            Self::TimedOut => "timed_out",
            Self::Failed => "failed",
            Self::Unsupported => "unsupported",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Resolution {
    pub status: ResolutionStatus,
    /// `Some` exactly when `status` is `Resolved`.
    pub path: Option<String>,
    /// Entries kept after dropping empty and relative ones.
    pub entries: usize,
    pub duration_ms: u128,
}

/// The `PATH` between the markers in `output`, with empty and relative
/// entries dropped (requirement 6), or `None` when the markers are missing or
/// nothing usable is left.
pub fn extract_path(output: &str) -> Option<String> {
    let start = output.find(START_MARKER)? + START_MARKER.len();
    let length = output[start..].find(END_MARKER)?;
    let kept: Vec<&str> = output[start..start + length]
        .split(':')
        .filter(|entry| entry.starts_with('/') && !entry.contains(['\n', '\0']))
        .collect();
    (!kept.is_empty()).then(|| kept.join(":"))
}

/// Whether `shell` is one whose `PATH` this module reads and restores: it
/// prints `$PATH` colon-separated and understands `export NAME="$VALUE";`.
pub fn is_posix_family(shell: &str) -> bool {
    let name = Path::new(shell)
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or_default();
    matches!(name, "sh" | "bash" | "zsh" | "ksh" | "dash" | "mksh")
}

/// The command line handed to `<shell> -lc` when a `PATH` was resolved: the
/// command behind an `export` on the same line, so error line numbers do not
/// move. Only the spawn argument carries it.
pub fn command_with_path(command: &str) -> String {
    format!("export PATH=\"${PATH_VARIABLE}\"; {command}")
}

/// Asks `shell` for its interactive login `PATH`, bounded by `timeout`.
///
/// Runs in `$HOME`, never a repository, so an rc hook that reacts to the
/// working directory sees the home directory. `data_dir` is where the
/// spec 46 registry lives; the shell is registered for its lifetime.
pub fn resolve(shell: &str, timeout: Duration, data_dir: &Path) -> Resolution {
    let started = Instant::now();
    let finish = |status, path: Option<String>| Resolution {
        status,
        entries: path.as_deref().map_or(0, |path| path.split(':').count()),
        path,
        duration_ms: started.elapsed().as_millis(),
    };
    if !is_posix_family(shell) {
        return finish(ResolutionStatus::Unsupported, None);
    }
    let home = std::env::var_os("HOME").unwrap_or_else(|| "/".into());
    let script = format!("printf '%s%s%s' '{START_MARKER}' \"$PATH\" '{END_MARKER}'");
    let Ok(mut child) = Command::new(shell)
        .arg("-ilc")
        .arg(script)
        .current_dir(&home)
        .env(RESOLVING_VARIABLE, "1")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .process_group(0)
        .spawn()
    else {
        return finish(ResolutionStatus::Failed, None);
    };
    let _registration = ProcessRegistry::open(data_dir)
        .and_then(|registry| registry.register(ProcessKind::Command, "", child.id()));

    // Read on a thread and stop at the end marker: a background job started
    // by an rc file can hold stdout open long after the shell exits, so
    // waiting for end of file could wait forever.
    let mut stdout = child.stdout.take().expect("stdout was piped");
    let (sender, receiver) = mpsc::channel::<Vec<u8>>();
    std::thread::spawn(move || {
        let mut buffer = [0u8; 4096];
        while let Ok(read) = stdout.read(&mut buffer) {
            if read == 0 || sender.send(buffer[..read].to_vec()).is_err() {
                break;
            }
        }
    });

    let deadline = started + timeout;
    let mut output = Vec::new();
    loop {
        if let Some(path) = extract_path(&String::from_utf8_lossy(&output)) {
            settle(&mut child, deadline);
            return finish(ResolutionStatus::Resolved, Some(path));
        }
        let remaining = deadline.saturating_duration_since(Instant::now());
        match receiver.recv_timeout(remaining) {
            Ok(chunk) => output.extend(chunk),
            Err(RecvTimeoutError::Timeout) => {
                kill_group(&mut child);
                return finish(ResolutionStatus::TimedOut, None);
            }
            Err(RecvTimeoutError::Disconnected) => {
                settle(&mut child, deadline);
                let path = extract_path(&String::from_utf8_lossy(&output));
                let status = if path.is_some() {
                    ResolutionStatus::Resolved
                } else {
                    ResolutionStatus::Failed
                };
                return finish(status, path);
            }
        }
    }
}

/// Gives the shell until `deadline` (at most half a second) to exit on its
/// own, then kills its group, and always reaps it.
fn settle(child: &mut Child, deadline: Instant) {
    let until = deadline.min(Instant::now() + Duration::from_millis(500));
    while Instant::now() < until {
        if let Ok(Some(_)) = child.try_wait() {
            return;
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    kill_group(child);
}

fn kill_group(child: &mut Child) {
    // SAFETY: `kill` with a negative pid signals the process group the child
    // leads (`process_group(0)` above); it touches no memory.
    unsafe { libc::kill(-(child.id() as libc::pid_t), libc::SIGKILL) };
    let _ = child.wait();
}

/// The resolution for `shell`, computed once per process (requirement 3),
/// failures included, so a shell that times out costs the timeout once rather
/// than on every command. Concurrent first callers wait for one resolution.
/// `on_resolved` runs only for the call that resolved, so the outcome is
/// recorded once.
pub fn cached(
    shell: &str,
    timeout: Duration,
    data_dir: &Path,
    on_resolved: impl FnOnce(&Resolution),
) -> Resolution {
    static CACHE: OnceLock<Mutex<HashMap<String, Arc<OnceLock<Resolution>>>>> = OnceLock::new();
    let cell = CACHE
        .get_or_init(Default::default)
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .entry(shell.to_string())
        .or_default()
        .clone();
    let mut resolved_here = false;
    let resolution = cell
        .get_or_init(|| {
            resolved_here = true;
            resolve(shell, timeout, data_dir)
        })
        .clone();
    if resolved_here {
        on_resolved(&resolution);
    }
    resolution
}

#[cfg(test)]
mod tests {
    use super::*;

    fn wrapped(path: &str) -> String {
        format!("{START_MARKER}{path}{END_MARKER}")
    }

    #[test]
    fn the_path_is_the_text_between_the_markers() {
        let output = format!(
            "Welcome back!\nnvm loaded\n{}trailing rc noise\n",
            wrapped("/Users/u/.nvm/bin:/opt/homebrew/bin:/usr/bin")
        );
        assert_eq!(
            extract_path(&output).as_deref(),
            Some("/Users/u/.nvm/bin:/opt/homebrew/bin:/usr/bin")
        );
    }

    #[test]
    fn empty_and_relative_entries_are_dropped() {
        assert_eq!(
            extract_path(&wrapped("::.:bin:/usr/bin:node_modules/.bin:/bin:")).as_deref(),
            Some("/usr/bin:/bin")
        );
    }

    #[test]
    fn missing_markers_or_no_usable_entry_is_no_path() {
        assert_eq!(extract_path("/usr/bin:/bin"), None);
        assert_eq!(extract_path(&format!("{START_MARKER}/usr/bin")), None);
        assert_eq!(extract_path(&wrapped(".:bin")), None);
        assert_eq!(extract_path(&wrapped("")), None);
    }

    #[test]
    fn the_export_prefix_keeps_the_command_on_its_line() {
        assert_eq!(
            command_with_path("npm test"),
            "export PATH=\"$DAMAIAN_SHELL_PATH\"; npm test"
        );
    }

    #[test]
    fn only_posix_family_shells_are_asked() {
        for shell in [
            "/bin/zsh",
            "/bin/bash",
            "/bin/sh",
            "/usr/local/bin/dash",
            "ksh",
        ] {
            assert!(is_posix_family(shell), "{shell}");
        }
        for shell in ["/opt/homebrew/bin/fish", "/usr/local/bin/nu", ""] {
            assert!(!is_posix_family(shell), "{shell}");
            let resolution = resolve(shell, RESOLVE_TIMEOUT, &std::env::temp_dir());
            assert_eq!(resolution.status, ResolutionStatus::Unsupported, "{shell}");
            assert_eq!(resolution.path, None);
        }
    }
}

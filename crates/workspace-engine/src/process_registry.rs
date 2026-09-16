//! A registry of the child processes this instance spawned, and the launch-time
//! sweep that kills the ones a crashed instance left behind, per
//! `docs/specs/46_process_registry_and_orphan_sweep/proposal.md`.
//!
//! The whole module turns on one question: does a recorded PID still name the
//! process that was recorded? A PID is reused, so killing by number alone would
//! eventually kill a stranger — a worse bug than the leak this exists to fix.
//! `ProcessIdentity` is the evidence, and nothing here kills without it.

use crate::audit::AuditLog;
use crate::error::{ClientError, Result};
use crate::hash::now_millis;
use serde::{Deserialize, Serialize};
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};

/// A PID together with the evidence that it is still the same process.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ProcessIdentity {
    pub pid: u32,
    pub start_time_us: u64,
}

impl ProcessIdentity {
    /// Reads a live process's start time, to the microsecond.
    ///
    /// `None` covers a dead PID, a zombie, and a process owned by another user.
    /// Callers must never distinguish them: all three mean "not provably a live
    /// process of ours", and the only safe response to that is to leave it
    /// alone. That the API collapses them is why it was chosen over parsing
    /// `ps`, which reports whole seconds and would need a policy for each case.
    pub fn of(pid: u32) -> Option<Self> {
        let mut info: libc::proc_bsdinfo = unsafe { std::mem::zeroed() };
        let size = std::mem::size_of::<libc::proc_bsdinfo>() as libc::c_int;
        // SAFETY: `info` is a zeroed, correctly aligned `proc_bsdinfo` and
        // `size` is exactly its length, so `proc_pidinfo` cannot write past it.
        // It reports the byte count it wrote, and a short write means the
        // kernel had nothing to report for this pid.
        let written = unsafe {
            libc::proc_pidinfo(
                pid as libc::c_int,
                libc::PROC_PIDTBSDINFO,
                0,
                std::ptr::from_mut(&mut info).cast::<libc::c_void>(),
                size,
            )
        };
        if written != size {
            return None;
        }
        Some(Self {
            pid,
            start_time_us: info.pbi_start_tvsec * 1_000_000 + info.pbi_start_tvusec,
        })
    }
}

/// What a registry entry describes, so an audited sweep says what it killed
/// rather than only which number.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProcessKind {
    McpServer,
    ModelCall,
    Terminal,
    Command,
}

impl ProcessKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::McpServer => "mcp_server",
            Self::ModelCall => "model_call",
            Self::Terminal => "terminal",
            Self::Command => "command",
        }
    }
}

/// One live child, as recorded on disk.
///
/// The fields are flat and `camelCase` so the file reads the same way as an
/// audit event, and so `pid`/`startTimeUs` can be compared against a filename
/// without unwrapping a nested object.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RegisteredProcess {
    pub pid: u32,
    pub start_time_us: u64,
    /// The process group to signal. Equal to `pid` whenever the spawn site put
    /// the child in its own group, which is the only case where the group may
    /// be killed — see `proposal.md` §5.5.
    pub pgid: u32,
    pub kind: String,
    pub session_id: String,
    pub registered_at_ms: u64,
    pub owner_pid: u32,
    pub owner_start_time_us: u64,
}

impl RegisteredProcess {
    pub fn identity(&self) -> ProcessIdentity {
        ProcessIdentity {
            pid: self.pid,
            start_time_us: self.start_time_us,
        }
    }

    pub fn owner(&self) -> ProcessIdentity {
        ProcessIdentity {
            pid: self.owner_pid,
            start_time_us: self.owner_start_time_us,
        }
    }

    pub fn file_name(&self) -> String {
        format!("{}-{}.json", self.pid, self.start_time_us)
    }
}

/// The set of entry files under `<data_dir>/processes`, owned by this instance.
#[derive(Debug, Clone)]
pub struct ProcessRegistry {
    dir: PathBuf,
    owner: ProcessIdentity,
}

impl ProcessRegistry {
    pub fn open(data_dir: impl AsRef<Path>) -> Result<Self> {
        let dir = data_dir.as_ref().join("processes");
        fs::create_dir_all(&dir)?;
        let owner = ProcessIdentity::of(std::process::id()).ok_or_else(|| {
            ClientError::Io("cannot read this process's own start time".to_string())
        })?;
        Ok(Self { dir, owner })
    }

    /// Records `pid` and returns a handle that removes the record when dropped.
    ///
    /// The file reaches disk before this returns, so requirement 3 holds by
    /// construction: there is no window in which a child is running and
    /// unrecorded, and nothing has to be written at exit for the record to
    /// survive a `SIGKILL`.
    pub fn register(
        &self,
        kind: ProcessKind,
        session_id: &str,
        pid: u32,
    ) -> Result<RegistrationHandle> {
        let identity = ProcessIdentity::of(pid).ok_or_else(|| {
            ClientError::Io(format!("process {pid} has no readable identity to record"))
        })?;
        // Read rather than assumed. Every spawn site puts the child in its own
        // group, so this should equal `pid`; where it does not, the sweep falls
        // back to killing the bare pid rather than a group we did not create.
        // SAFETY: `getpgid` takes a pid by value and touches no memory of ours.
        let pgid = unsafe { libc::getpgid(pid as libc::pid_t) };
        let entry = RegisteredProcess {
            pid: identity.pid,
            start_time_us: identity.start_time_us,
            pgid: if pgid > 0 { pgid as u32 } else { pid },
            kind: kind.as_str().to_string(),
            session_id: session_id.to_string(),
            registered_at_ms: now_millis() as u64,
            owner_pid: self.owner.pid,
            owner_start_time_us: self.owner.start_time_us,
        };
        let path = self.dir.join(entry.file_name());
        let json =
            serde_json::to_string(&entry).map_err(|error| ClientError::Io(error.to_string()))?;
        let mut file = fs::OpenOptions::new()
            .create_new(true)
            .write(true)
            .open(&path)?;
        file.write_all(json.as_bytes())?;
        file.sync_all()?;
        Ok(RegistrationHandle { path: Some(path) })
    }

    /// Every entry file, paired with its parsed content. `None` means the file
    /// did not parse — a write interrupted between `create_new` and
    /// `write_all`, which the sweep audits rather than ignores.
    pub fn entries(&self) -> Result<Vec<(PathBuf, Option<RegisteredProcess>)>> {
        let mut entries = Vec::new();
        for entry in fs::read_dir(&self.dir)? {
            let path = entry?.path();
            if path.extension().and_then(|extension| extension.to_str()) != Some("json") {
                continue;
            }
            let parsed = fs::read_to_string(&path)
                .ok()
                .and_then(|text| serde_json::from_str::<RegisteredProcess>(&text).ok());
            entries.push((path, parsed));
        }
        Ok(entries)
    }

    pub fn owner(&self) -> ProcessIdentity {
        self.owner
    }
}

/// Which entries a sweep considers. The per-entry identity check is identical
/// in both; only the owner rule differs, because at shutdown the owner is this
/// very process and `CrashedOwners` would therefore skip every entry.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SweepScope {
    /// Launch: entries whose owning instance is provably gone.
    CrashedOwners,
    /// Shutdown: this instance's own entries, because it is the one going away.
    OwnProcess,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SweepDecision {
    Unreadable,
    OwnerAlive,
    AlreadyExited,
    Refused {
        recorded_start_time_us: u64,
        actual_start_time_us: u64,
    },
    Killed {
        pid: u32,
        pgid: u32,
    },
}

#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct SweepReport {
    pub decisions: Vec<SweepDecision>,
}

impl SweepReport {
    pub fn killed(&self) -> usize {
        self.decisions
            .iter()
            .filter(|decision| matches!(decision, SweepDecision::Killed { .. }))
            .count()
    }

    pub fn refused(&self) -> usize {
        self.decisions
            .iter()
            .filter(|decision| matches!(decision, SweepDecision::Refused { .. }))
            .count()
    }
}

impl ProcessRegistry {
    #[cfg(test)]
    fn entries_dir_for_test(&self) -> PathBuf {
        self.dir.clone()
    }

    /// The decision table from `proposal.md` §5.4, with the identity lookup and
    /// the kill injected so every row is testable without a real process.
    pub fn sweep_with(
        &self,
        audit: &AuditLog,
        scope: SweepScope,
        identity_of: &dyn Fn(u32) -> Option<ProcessIdentity>,
        kill: &mut dyn FnMut(&RegisteredProcess),
    ) -> Result<SweepReport> {
        let mut report = SweepReport::default();
        for (path, parsed) in self.entries()? {
            let Some(entry) = parsed else {
                audit.record(
                    "process_registry_entry_unreadable",
                    &[
                        ("actor", "system".to_string()),
                        ("path", path.display().to_string()),
                    ],
                )?;
                let _ = fs::remove_file(&path);
                report.decisions.push(SweepDecision::Unreadable);
                continue;
            };

            let owner_alive = identity_of(entry.owner_pid) == Some(entry.owner());
            let in_scope = match scope {
                SweepScope::CrashedOwners => !owner_alive,
                SweepScope::OwnProcess => entry.owner() == self.owner,
            };
            if !in_scope {
                report.decisions.push(SweepDecision::OwnerAlive);
                continue;
            }

            let Some(actual) = identity_of(entry.pid) else {
                audit.record(
                    "orphan_process_already_exited",
                    &[
                        ("actor", "system".to_string()),
                        ("pid", entry.pid.to_string()),
                        ("kind", entry.kind.clone()),
                        ("sessionId", entry.session_id.clone()),
                    ],
                )?;
                let _ = fs::remove_file(&path);
                report.decisions.push(SweepDecision::AlreadyExited);
                continue;
            };

            if actual.start_time_us != entry.start_time_us {
                // The number is the same and the process is not. Requirement 2.
                audit.record(
                    "orphan_process_kill_refused",
                    &[
                        ("actor", "system".to_string()),
                        ("pid", entry.pid.to_string()),
                        ("kind", entry.kind.clone()),
                        ("sessionId", entry.session_id.clone()),
                        ("recordedStartTimeUs", entry.start_time_us.to_string()),
                        ("actualStartTimeUs", actual.start_time_us.to_string()),
                    ],
                )?;
                let _ = fs::remove_file(&path);
                report.decisions.push(SweepDecision::Refused {
                    recorded_start_time_us: entry.start_time_us,
                    actual_start_time_us: actual.start_time_us,
                });
                continue;
            }

            kill(&entry);
            audit.record(
                "orphan_process_killed",
                &[
                    ("actor", "system".to_string()),
                    ("pid", entry.pid.to_string()),
                    ("pgid", entry.pgid.to_string()),
                    ("kind", entry.kind.clone()),
                    ("sessionId", entry.session_id.clone()),
                    ("scope", format!("{scope:?}")),
                ],
            )?;
            // At shutdown the entry is deliberately left: see `proposal.md`
            // §5.6. The next launch finds no identity for it and removes it.
            if scope == SweepScope::CrashedOwners {
                let _ = fs::remove_file(&path);
            }
            report.decisions.push(SweepDecision::Killed {
                pid: entry.pid,
                pgid: entry.pgid,
            });
        }
        Ok(report)
    }
}

/// How long a swept process is given to act on `SIGTERM` before `SIGKILL`.
///
/// A swept entry is often a shell command, and a `git` or `npm` part way
/// through writing the user's repository is worth this much delay. The
/// escalation means "not still running after recovery" holds either way.
const TERMINATE_GRACE: std::time::Duration = std::time::Duration::from_millis(200);

/// `SIGTERM`, then `SIGKILL` if it is still there.
///
/// The group is signalled only when the leader is the process whose identity
/// the caller just matched. A group whose leader is gone may already belong to
/// someone else, so widening past the evidence is exactly the bug `proposal.md`
/// §1 forbids.
fn terminate(entry: &RegisteredProcess) {
    let target = if entry.pgid == entry.pid {
        -(entry.pgid as libc::pid_t)
    } else {
        entry.pid as libc::pid_t
    };
    // SAFETY: `kill` takes two integers by value and touches no memory. A
    // negative target is a process group, which is why the branch above gates
    // it on the leader's identity having matched.
    unsafe { libc::kill(target, libc::SIGTERM) };

    let deadline = std::time::Instant::now() + TERMINATE_GRACE;
    while std::time::Instant::now() < deadline {
        if ProcessIdentity::of(entry.pid).is_none() {
            return;
        }
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
    // SAFETY: as above.
    unsafe { libc::kill(target, libc::SIGKILL) };
}

impl ProcessRegistry {
    /// The launch-time sweep: entries whose owning instance is gone.
    pub fn sweep(&self, audit: &AuditLog) -> Result<SweepReport> {
        // Closures rather than `&ProcessIdentity::of` / `&mut terminate`: a
        // function *item* has no place to borrow from, so it will not coerce
        // to `&dyn Fn` / `&mut dyn FnMut` directly.
        self.sweep_with(
            audit,
            SweepScope::CrashedOwners,
            &|pid| ProcessIdentity::of(pid),
            &mut |entry: &RegisteredProcess| terminate(entry),
        )
    }

    /// The shutdown sweep: this instance's own entries.
    pub fn sweep_own(&self, audit: &AuditLog) -> Result<SweepReport> {
        self.sweep_with(
            audit,
            SweepScope::OwnProcess,
            &|pid| ProcessIdentity::of(pid),
            &mut |entry: &RegisteredProcess| terminate(entry),
        )
    }
}

/// Removes its entry when dropped, so requirement 4 holds on every path that
/// unwinds or returns normally without a caller remembering anything.
#[derive(Debug)]
pub struct RegistrationHandle {
    path: Option<PathBuf>,
}

impl RegistrationHandle {
    /// A handle that records nothing, for the mock adapters and the test
    /// fixtures that have no data directory. Never use it on a path that
    /// really spawns a process — an unregistered child cannot be swept.
    pub fn none() -> Self {
        Self { path: None }
    }
}

impl Drop for RegistrationHandle {
    fn drop(&mut self) {
        if let Some(path) = &self.path {
            // Already gone is the normal case when a sweep got there first.
            let _ = fs::remove_file(path);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    /// A scratch data directory, so tests never touch the user's real
    /// `~/Library/Application Support/DamaianClient`.
    fn scratch(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "damaian-registry-{name}-{}-{:?}",
            std::process::id(),
            std::thread::current().id()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("scratch data dir");
        dir
    }

    #[test]
    fn registering_writes_an_entry_before_it_returns() {
        let data_dir = scratch("register");
        let registry = ProcessRegistry::open(&data_dir).unwrap();
        // Our own pid: a live process we can read, with no spawning involved.
        let handle = registry
            .register(ProcessKind::Command, "ses_1", std::process::id())
            .unwrap();

        let entries = registry.entries().unwrap();
        assert_eq!(entries.len(), 1);
        let entry = entries[0].1.clone().expect("a parsable entry");
        assert_eq!(entry.pid, std::process::id());
        assert_eq!(entry.kind, ProcessKind::Command.as_str());
        assert_eq!(entry.session_id, "ses_1");
        assert_eq!(
            entry.owner(),
            ProcessIdentity::of(std::process::id()).unwrap(),
            "the owner is this instance, so a concurrent instance can tell it is alive"
        );
        drop(handle);
    }

    #[test]
    fn the_file_name_carries_both_halves_of_the_identity() {
        let data_dir = scratch("filename");
        let registry = ProcessRegistry::open(&data_dir).unwrap();
        let handle = registry
            .register(ProcessKind::ModelCall, "ses_1", std::process::id())
            .unwrap();

        let identity = ProcessIdentity::of(std::process::id()).unwrap();
        let expected = format!("{}-{}.json", identity.pid, identity.start_time_us);
        assert!(
            data_dir.join("processes").join(&expected).exists(),
            "a recycled pid must not be able to collide with the entry it recycled"
        );
        drop(handle);
    }

    #[test]
    fn dropping_the_handle_removes_the_entry() {
        let data_dir = scratch("clean-exit");
        let registry = ProcessRegistry::open(&data_dir).unwrap();
        let handle = registry
            .register(ProcessKind::Terminal, "ses_1", std::process::id())
            .unwrap();
        assert_eq!(registry.entries().unwrap().len(), 1);

        drop(handle);

        assert!(
            registry.entries().unwrap().is_empty(),
            "requirement 4: a clean exit leaves no registry entry"
        );
    }

    #[test]
    fn a_half_written_entry_is_reported_rather_than_parsed() {
        let data_dir = scratch("torn");
        let registry = ProcessRegistry::open(&data_dir).unwrap();
        std::fs::write(
            data_dir.join("processes").join("123-456.json"),
            "{\"pid\":1",
        )
        .unwrap();

        let entries = registry.entries().unwrap();
        assert_eq!(entries.len(), 1);
        assert!(
            entries[0].1.is_none(),
            "a torn write is evidence of the crash, not something to guess at"
        );
    }

    #[test]
    fn registering_a_pid_with_no_identity_fails_rather_than_recording_a_number() {
        let data_dir = scratch("no-identity");
        let registry = ProcessRegistry::open(&data_dir).unwrap();
        assert!(
            registry
                .register(ProcessKind::Command, "ses_1", 4_000_000)
                .is_err()
        );
        assert!(registry.entries().unwrap().is_empty());
    }

    use crate::audit::AuditLog;
    use crate::secret_scanner::SecretScanner;
    use std::cell::RefCell;

    fn audit_for(data_dir: &std::path::Path) -> AuditLog {
        AuditLog::new(data_dir, true, SecretScanner::new(Vec::new()))
    }

    /// Writes an entry by hand so a test can describe a process that does not
    /// exist, which is the whole point of most of these cases.
    fn plant(registry: &ProcessRegistry, entry: &RegisteredProcess) {
        std::fs::write(
            registry.entries_dir_for_test().join(entry.file_name()),
            serde_json::to_string(entry).unwrap(),
        )
        .unwrap();
    }

    fn entry_owned_by(owner: ProcessIdentity, pid: u32, start_time_us: u64) -> RegisteredProcess {
        RegisteredProcess {
            pid,
            start_time_us,
            pgid: pid,
            kind: ProcessKind::Command.as_str().to_string(),
            session_id: "ses_1".to_string(),
            registered_at_ms: 0,
            owner_pid: owner.pid,
            owner_start_time_us: owner.start_time_us,
        }
    }

    /// An owner that is definitely gone: a pid that cannot name a live process,
    /// with a start time **no injected lookup below returns**. That matters —
    /// if it collided, `owner_alive` would be true by accident, the entry would
    /// be skipped, and the test would pass without reaching what it asserts.
    fn dead_owner() -> ProcessIdentity {
        ProcessIdentity {
            pid: 4_000_001,
            start_time_us: 1,
        }
    }

    fn sweep_recording(
        registry: &ProcessRegistry,
        audit: &AuditLog,
        scope: SweepScope,
        identity_of: &dyn Fn(u32) -> Option<ProcessIdentity>,
    ) -> (SweepReport, Vec<u32>) {
        let killed = RefCell::new(Vec::new());
        let report = registry
            .sweep_with(audit, scope, identity_of, &mut |entry| {
                killed.borrow_mut().push(entry.pid);
            })
            .unwrap();
        (report, killed.into_inner())
    }

    #[test]
    fn a_start_time_mismatch_refuses_to_kill() {
        let data_dir = scratch("refuse");
        let registry = ProcessRegistry::open(&data_dir).unwrap();
        let audit = audit_for(&data_dir);
        plant(&registry, &entry_owned_by(dead_owner(), 5000, 111));

        // The pid is alive, but it is a different process than the one recorded.
        let (report, killed) =
            sweep_recording(&registry, &audit, SweepScope::CrashedOwners, &|pid| {
                Some(ProcessIdentity {
                    pid,
                    start_time_us: 999,
                })
            });

        assert!(killed.is_empty(), "a recycled pid must never be killed");
        assert_eq!(
            report.decisions,
            vec![SweepDecision::Refused {
                recorded_start_time_us: 111,
                actual_start_time_us: 999,
            }]
        );
        let log = std::fs::read_to_string(data_dir.join("audit").join("events.jsonl")).unwrap();
        assert!(log.contains("orphan_process_kill_refused"));
        assert!(log.contains("\"recordedStartTimeUs\":\"111\""));
        assert!(log.contains("\"actualStartTimeUs\":\"999\""));
    }

    #[test]
    fn a_matching_start_time_is_killed() {
        let data_dir = scratch("kill");
        let registry = ProcessRegistry::open(&data_dir).unwrap();
        let audit = audit_for(&data_dir);
        plant(&registry, &entry_owned_by(dead_owner(), 5000, 111));

        let (report, killed) =
            sweep_recording(&registry, &audit, SweepScope::CrashedOwners, &|pid| {
                Some(ProcessIdentity {
                    pid,
                    start_time_us: 111,
                })
            });

        assert_eq!(killed, vec![5000]);
        assert_eq!(
            report.decisions,
            vec![SweepDecision::Killed {
                pid: 5000,
                pgid: 5000
            }]
        );
        assert!(registry.entries().unwrap().is_empty(), "the entry is spent");
    }

    #[test]
    fn a_process_that_already_exited_is_not_killed() {
        let data_dir = scratch("exited");
        let registry = ProcessRegistry::open(&data_dir).unwrap();
        let audit = audit_for(&data_dir);
        plant(&registry, &entry_owned_by(dead_owner(), 5000, 111));

        let (report, killed) =
            sweep_recording(&registry, &audit, SweepScope::CrashedOwners, &|_| None);

        assert!(killed.is_empty());
        assert_eq!(report.decisions, vec![SweepDecision::AlreadyExited]);
        assert!(registry.entries().unwrap().is_empty());
    }

    #[test]
    fn an_entry_owned_by_a_live_instance_is_left_alone() {
        let data_dir = scratch("live-owner");
        let registry = ProcessRegistry::open(&data_dir).unwrap();
        let audit = audit_for(&data_dir);
        let owner = registry.owner();
        // Owned by *us*, and we are alive: this is a second Damaian instance's
        // process from the sweeping instance's point of view.
        plant(&registry, &entry_owned_by(owner, 5000, 111));

        // The owner must read back as genuinely alive, so the lookup returns
        // its real identity rather than the synthetic one used for children.
        let (report, killed) =
            sweep_recording(&registry, &audit, SweepScope::CrashedOwners, &|pid| {
                if pid == owner.pid {
                    Some(owner)
                } else {
                    Some(ProcessIdentity {
                        pid,
                        start_time_us: 111,
                    })
                }
            });

        assert!(
            killed.is_empty(),
            "requirement 6: a running instance's processes are not ours to kill"
        );
        assert_eq!(report.decisions, vec![SweepDecision::OwnerAlive]);
        assert_eq!(
            registry.entries().unwrap().len(),
            1,
            "and its entry stays, because that instance still needs it"
        );
    }

    #[test]
    fn shutdown_scope_sweeps_exactly_the_entries_the_launch_scope_skips() {
        let data_dir = scratch("own-scope");
        let registry = ProcessRegistry::open(&data_dir).unwrap();
        let audit = audit_for(&data_dir);
        let owner = registry.owner();
        plant(&registry, &entry_owned_by(owner, 5000, 111));
        plant(&registry, &entry_owned_by(dead_owner(), 6000, 222));

        let (_, killed) = sweep_recording(&registry, &audit, SweepScope::OwnProcess, &|pid| {
            Some(ProcessIdentity {
                pid,
                start_time_us: if pid == 5000 { 111 } else { 222 },
            })
        });

        assert_eq!(
            killed,
            vec![5000],
            "at shutdown the owner is alive by definition: it is us"
        );
    }

    #[test]
    fn an_unreadable_entry_is_audited_and_discarded() {
        let data_dir = scratch("unreadable");
        let registry = ProcessRegistry::open(&data_dir).unwrap();
        let audit = audit_for(&data_dir);
        std::fs::write(
            registry.entries_dir_for_test().join("123-456.json"),
            "{\"pid\":1",
        )
        .unwrap();

        let (report, killed) =
            sweep_recording(&registry, &audit, SweepScope::CrashedOwners, &|_| None);

        assert!(killed.is_empty());
        assert_eq!(report.decisions, vec![SweepDecision::Unreadable]);
        let log = std::fs::read_to_string(data_dir.join("audit").join("events.jsonl")).unwrap();
        assert!(log.contains("process_registry_entry_unreadable"));
    }

    /// The kill path against a real child: `SIGTERM` first, `SIGKILL` if it
    /// refuses, and the group reached rather than just the leader.
    ///
    /// `#[ignore]`d per `AGENTS.md` because it spawns and kills real processes.
    /// Run it by hand:
    ///
    /// ```sh
    /// cargo test -p workspace-engine --lib -- --ignored --exact \
    ///   process_registry::tests::terminating_reaches_the_whole_group
    /// ```
    #[test]
    #[ignore]
    fn terminating_reaches_the_whole_group() {
        use std::os::unix::process::CommandExt;
        use std::process::{Command, Stdio};
        use std::time::{Duration, Instant};

        let data_dir = scratch("terminate");
        let registry = ProcessRegistry::open(&data_dir).unwrap();
        let audit = audit_for(&data_dir);

        // A shell with a background job, so the group holds three processes.
        // A bare `kill(pid)` would leave both sleeps behind and this test
        // would fail — which is the point of reaching the group.
        let mut child = Command::new("/bin/sh")
            .arg("-lc")
            .arg("sleep 120 & sleep 120")
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .process_group(0)
            .spawn()
            .expect("child should spawn");
        let pid = child.id();
        let handle = registry
            .register(ProcessKind::Command, "ses_1", pid)
            .unwrap();
        // The entry must outlive this instance, exactly as a crash would leave
        // it, so the handle must not unlink on the way out.
        std::mem::forget(handle);

        // The group's members, captured before the sweep. Asserting only that
        // the leader died would pass just as well against `kill(pid)`, which
        // is the bug this test exists to catch.
        let settle = Instant::now() + Duration::from_secs(5);
        let members = loop {
            let members = group_members(pid);
            if members.len() >= 3 || Instant::now() > settle {
                break members;
            }
            std::thread::sleep(Duration::from_millis(20));
        };
        assert!(
            members.len() >= 3,
            "expected the shell and both sleeps in group {pid}, got {members:?}"
        );

        let report = registry.sweep_own(&audit).unwrap();
        assert_eq!(report.killed(), 1);

        let deadline = Instant::now() + Duration::from_secs(10);
        loop {
            let alive: Vec<u32> = members
                .iter()
                .copied()
                .filter(|member| ProcessIdentity::of(*member).is_some())
                .collect();
            if alive.is_empty() {
                break;
            }
            assert!(
                Instant::now() < deadline,
                "these group members survived the sweep: {alive:?}"
            );
            std::thread::sleep(Duration::from_millis(20));
        }
        let _ = child.wait();
    }

    /// Every pid in one process group. Test-only: the sweep itself never
    /// enumerates a group, it only signals one.
    ///
    /// `ps -g` is not this — on macOS it selects by session leader, not by
    /// process-group membership — so the pgid column is read and filtered here.
    fn group_members(pgid: u32) -> Vec<u32> {
        let output = std::process::Command::new("/bin/ps")
            .args(["-ax", "-o", "pid=,pgid="])
            .output()
            .expect("ps should run");
        String::from_utf8_lossy(&output.stdout)
            .lines()
            .filter_map(|line| {
                let mut fields = line.split_whitespace();
                let pid = fields.next()?.parse::<u32>().ok()?;
                let group = fields.next()?.parse::<u32>().ok()?;
                (group == pgid).then_some(pid)
            })
            .collect()
    }

    #[test]
    fn our_own_identity_is_readable_and_stable() {
        let first = ProcessIdentity::of(std::process::id()).expect("we are alive");
        let second = ProcessIdentity::of(std::process::id()).expect("still alive");
        assert_eq!(
            first, second,
            "a start time that changes between reads cannot be evidence of anything"
        );
        assert_eq!(first.pid, std::process::id());
        assert!(first.start_time_us > 0);
    }

    #[test]
    fn a_pid_that_cannot_exist_has_no_identity() {
        // Far above the macOS pid ceiling, so it can never name a live process.
        assert_eq!(ProcessIdentity::of(4_000_000), None);
    }

    #[test]
    fn a_process_owned_by_another_user_has_no_identity() {
        // SAFETY: `geteuid` takes no arguments, touches no memory and cannot fail.
        if unsafe { libc::geteuid() } == 0 {
            // As root every process is readable, so the premise does not hold
            // and the assertion below would be testing nothing.
            return;
        }
        // pid 1 is `launchd`: alive, but root-owned. "Alive" is not enough —
        // the sweep needs "alive *and* ours", and this is the difference.
        assert_eq!(ProcessIdentity::of(1), None);
    }
}

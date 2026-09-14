# Process Registry and Orphan Sweep Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Implements:** [`proposal.md`](proposal.md)
**Started:** not yet

**Goal:** Record every child process Damaian spawns in a file written at spawn
time, and at the next launch kill the ones left behind by a crashed instance —
killing only where the recorded PID is still alive *and* its start time still
matches, so a recycled PID never costs a stranger their process.

**Architecture:** A new `workspace-engine/src/process_registry.rs` owns three
things: `ProcessIdentity` (a PID plus its start time, read through
`proc_pidinfo`), one JSON file per live child under `<data_dir>/processes/`, and
a sweep that walks those files applying the decision table in
[`proposal.md`](proposal.md) §5.4. The four spawn sites — MCP stdio, `curl`, the
PTY, and the shell command runner — each put their child in its own process
group and register it. A self-pipe signal handler gives the same identity-gated
kill a second caller, so Ctrl-C cleans up too.

**Tech Stack:** Rust 2024 (workspace edition). One new direct dependency:
`libc`, already in the tree transitively at 0.2.186 under MIT/Apache-2.0, both
on `deny.toml`'s allow-list. `serde`/`serde_json` for the entry files, already
dependencies of `workspace-engine`.

## Global Constraints

Every task's requirements implicitly include this section.

- **Read [`proposal.md`](proposal.md) §5 before starting.** Every design
  decision below has a "why" there, and several of them are counter-intuitive
  (why a file per process rather than a log; why the owner is recorded at all;
  why the signal handler does almost nothing). A task that looks like
  unnecessary indirection usually is not.
- **Never kill by name.** `AGENTS.md` states this and §1 of the proposal is
  built on it. Every kill in this work goes through a `ProcessIdentity`
  comparison. There is no `pkill`, no `killall`, and no matching on `pbi_comm`
  or `pbi_name`.
- **`None` from `ProcessIdentity::of` always means "do not kill".** It covers a
  dead PID, a zombie, and a process owned by another user, and the code must
  never branch on which. Adding a distinction here reintroduces exactly the
  class of bug §1 exists to prevent.
- **Never widen a kill past the evidence.** `kill(-pgid)` is permitted *only*
  when the recorded `pgid` equals the recorded `pid` and that PID's identity
  matched. A group whose leader is gone may already belong to someone else.
- **Registry entries are written at spawn, never at exit** (requirement 3). The
  file must exist before the spawning function returns its child to a caller.
  Nothing about recovery may depend on a write that happens at shutdown.
- **The signal handler does one `write` and nothing else.** `proc_pidinfo`,
  allocation, formatting, and file I/O are not async-signal-safe. Anything that
  is not a single `write` to the self-pipe belongs on the watchdog thread.
- **`unsafe` carries a `// SAFETY:` comment** saying why the call is sound —
  what the pointer points at and why the length is right. There are four
  `unsafe` blocks in this whole plan and no more.
- **Anything that spawns or kills a real process is `#[ignore]`d** with the
  exact manual command in its doc comment, per `AGENTS.md`. Reading *our own*
  identity is not spawning; those tests run normally.
- **Clippy warnings are errors.** Fix rather than suppress; an `#[allow(...)]`
  needs a comment saying why.
- **Every quality-gate command from `AGENTS.md` must pass** at the end of every
  task. That is seven commands and the list in `AGENTS.md` is authoritative —
  read it, do not rely on a remembered list. `typos` and
  `node --check crates/desktop-shell/static/app.js` are the two that get missed.
- **Commit messages:** one subject line, no body, no `Co-Authored-By`. Rationale
  belongs in this plan and the proposal. Never cite commit SHAs in
  documentation.
- **Do not commit without asking.** Show the change and the gate result; the
  decision to commit is Damijan's.

## File Structure

| File | Responsibility |
|---|---|
| `crates/workspace-engine/src/process_registry.rs` | **New.** `ProcessIdentity`, `ProcessKind`, `RegisteredProcess`, `ProcessRegistry`, `RegistrationHandle`, the sweep, the terminator, the shutdown handler. Everything in this spec that is not a call site. |
| `crates/workspace-engine/src/lib.rs` | Add `pub mod process_registry;` and the re-exports. |
| `crates/workspace-engine/Cargo.toml` | Add `libc`. |
| `crates/workspace-engine/src/mcp.rs` | `StdioTransport::spawn` takes a registry, own process group, holds a handle. |
| `crates/workspace-engine/src/model.rs` | `CurlModelTransport` takes a registry; `KillOnDrop` holds a handle. |
| `crates/workspace-engine/src/command_runner.rs` | `output()` → `spawn()` + `wait_with_output()`, own group, register, kill-on-drop guard. |
| `crates/desktop-shell/src/terminal.rs` | `open` takes a data directory; the session holds a handle. |
| `crates/desktop-shell/src/lib.rs` | Sweep at startup; pass the registry to the `CurlModelTransport` and `McpClient` call sites. |
| `crates/desktop-shell/src/main.rs`, `crates/damaian-cli/src/main.rs` | Install the shutdown handler. |
| `crates/workspace-engine/tests/process_registry.rs` | **New.** The `#[ignore]`d real-process tests and their re-exec helpers. |

The sweep's decision logic and the four call sites are deliberately separate
files: the decision table is the part that must not regress and is unit-tested
with no process at all, and the call sites are mechanical.

---

## Task 1: `ProcessIdentity`

The load-bearing primitive. Everything else is bookkeeping around it.

**Files:**
- Create: `crates/workspace-engine/src/process_registry.rs`
- Modify: `crates/workspace-engine/Cargo.toml`, `crates/workspace-engine/src/lib.rs`

**Interfaces:**
- Consumes: nothing.
- Produces: `ProcessIdentity { pid: u32, start_time_us: u64 }`, `Copy + Clone +
  Debug + PartialEq + Eq`, and `ProcessIdentity::of(pid: u32) ->
  Option<ProcessIdentity>`.

- [ ] **Step 1: Add the dependency**

In `crates/workspace-engine/Cargo.toml`, under `[dependencies]`, keeping the
list alphabetical:

```toml
libc = "0.2.186"
```

- [ ] **Step 2: Write the failing tests**

Create `crates/workspace-engine/src/process_registry.rs` containing only this
test module for now:

```rust
#[cfg(test)]
mod tests {
    use super::*;

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
```

- [ ] **Step 3: Run the tests to verify they fail**

```bash
cargo test -p workspace-engine --lib process_registry
```

Expected: FAIL to compile, `cannot find type ProcessIdentity in this scope`.

- [ ] **Step 4: Write the implementation**

Put this above the test module in the same file:

```rust
//! A registry of the child processes this instance spawned, and the launch-time
//! sweep that kills the ones a crashed instance left behind, per
//! `docs/specs/46_process_registry_and_orphan_sweep/proposal.md`.
//!
//! The whole module turns on one question: does a recorded PID still name the
//! process that was recorded? A PID is reused, so killing by number alone would
//! eventually kill a stranger — a worse bug than the leak this exists to fix.
//! `ProcessIdentity` is the evidence, and nothing here kills without it.

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
```

In `crates/workspace-engine/src/lib.rs`, add `pub mod process_registry;` in
alphabetical order (between `plan` and `recovery`) and the re-export beside the
others:

```rust
pub use process_registry::ProcessIdentity;
```

- [ ] **Step 5: Run the tests to verify they pass**

```bash
cargo test -p workspace-engine --lib process_registry
```

Expected: 3 passed.

- [ ] **Step 6: Run the full quality gate**

All seven commands from `AGENTS.md`. `cargo deny check` matters here — it is
the one that confirms `libc`'s license is on the allow-list.

- [ ] **Step 7: Show the change and ask before committing**

Suggested subject line: `Read a process start time so a recycled PID is detectable`

---

## Task 2: The entry file

One file per live child, written before the spawning function returns.

**Files:**
- Modify: `crates/workspace-engine/src/process_registry.rs`,
  `crates/workspace-engine/src/lib.rs`

**Interfaces:**
- Consumes: `ProcessIdentity`, `ProcessIdentity::of` from Task 1.
- Produces:
  - `ProcessKind` — `McpServer | ModelCall | Terminal | Command`, with
    `as_str(self) -> &'static str` and `parse(&str) -> Option<Self>`.
  - `RegisteredProcess` — the on-disk record, with `identity(&self) ->
    ProcessIdentity`, `owner(&self) -> ProcessIdentity`, `file_name(&self) ->
    String`.
  - `ProcessRegistry::open(data_dir: impl AsRef<Path>) -> Result<ProcessRegistry>`
  - `ProcessRegistry::register(&self, kind: ProcessKind, session_id: &str, pid:
    u32) -> Result<RegistrationHandle>`
  - `ProcessRegistry::entries(&self) -> Result<Vec<(PathBuf,
    Option<RegisteredProcess>)>>` — `None` for a file that does not parse.
  - `RegistrationHandle`, whose `Drop` unlinks, plus `RegistrationHandle::none()`.

- [ ] **Step 1: Write the failing tests**

Add to the `tests` module in `process_registry.rs`:

```rust
    use std::path::PathBuf;

    /// A scratch data directory under the target dir, so tests never touch the
    /// user's real `~/Library/Application Support/DamaianClient`.
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
        std::fs::write(data_dir.join("processes").join("123-456.json"), "{\"pid\":1")
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
        assert!(registry
            .register(ProcessKind::Command, "ses_1", 4_000_000)
            .is_err());
        assert!(registry.entries().unwrap().is_empty());
    }
```

- [ ] **Step 2: Run the tests to verify they fail**

```bash
cargo test -p workspace-engine --lib process_registry
```

Expected: FAIL to compile, `cannot find type ProcessRegistry in this scope`.

- [ ] **Step 3: Write the implementation**

Add these imports at the top of `process_registry.rs`:

```rust
use crate::error::{ClientError, Result};
use crate::hash::now_millis;
use serde::{Deserialize, Serialize};
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
```

Then, above the test module:

```rust
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
        let json = serde_json::to_string(&entry)
            .map_err(|error| ClientError::Io(error.to_string()))?;
        let mut file = fs::OpenOptions::new()
            .create_new(true)
            .write(true)
            .open(&path)?;
        file.write_all(json.as_bytes())?;
        file.sync_all()?;
        Ok(RegistrationHandle { path: Some(path) })
    }

    /// Every entry file, paired with its parsed content. `None` means the file
    /// did not parse — a write interrupted between `create_new` and `write_all`,
    /// which the sweep audits rather than ignores.
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
```

Add to `lib.rs`:

```rust
pub use process_registry::{
    ProcessIdentity, ProcessKind, ProcessRegistry, RegisteredProcess, RegistrationHandle,
};
```

(replacing the single-item re-export from Task 1).

- [ ] **Step 4: Run the tests to verify they pass**

```bash
cargo test -p workspace-engine --lib process_registry
```

Expected: 8 passed.

- [ ] **Step 5: Run the full quality gate**

All seven commands from `AGENTS.md`.

- [ ] **Step 6: Show the change and ask before committing**

Suggested subject line: `Record every spawned child in a file written before it runs`

---

## Task 3: The sweep decision table

The part that must not regress, and the part that needs no real process to
exercise. Killing is injected, so every row of §5.4's table is an ordinary unit
test.

**Files:**
- Modify: `crates/workspace-engine/src/process_registry.rs`,
  `crates/workspace-engine/src/lib.rs`

**Interfaces:**
- Consumes: everything from Tasks 1 and 2.
- Produces:
  - `SweepScope` — `CrashedOwners | OwnProcess`.
  - `SweepDecision` — `Unreadable | OwnerAlive | AlreadyExited | Refused {
    recorded_start_time_us: u64, actual_start_time_us: u64 } | Killed { pid:
    u32, pgid: u32 }`.
  - `SweepReport { decisions: Vec<SweepDecision> }` with `killed(&self) ->
    usize` and `refused(&self) -> usize`.
  - `ProcessRegistry::sweep_with(&self, audit: &AuditLog, scope: SweepScope,
    identity_of: &dyn Fn(u32) -> Option<ProcessIdentity>, kill: &mut dyn
    FnMut(&RegisteredProcess)) -> Result<SweepReport>`

- [ ] **Step 1: Write the failing tests**

Add to the `tests` module. These are the five rows of §5.4 plus the
concurrent-instance case:

```rust
    use crate::audit::AuditLog;
    use crate::secret_scanner::SecretScanner;
    use std::cell::RefCell;

    fn audit_for(data_dir: &std::path::Path) -> AuditLog {
        AuditLog::new(data_dir, true, SecretScanner::new(Vec::new()))
    }

    /// Writes an entry by hand so a test can describe a process that does not
    /// exist, which is the whole point of most of these cases.
    fn plant(registry: &ProcessRegistry, entry: &RegisteredProcess) {
        let dir = registry.entries_dir_for_test();
        std::fs::write(
            dir.join(entry.file_name()),
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

    /// An owner that is definitely gone: a pid that cannot name a live process.
    fn dead_owner() -> ProcessIdentity {
        ProcessIdentity { pid: 4_000_001, start_time_us: 111 }
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
        let (report, killed) = sweep_recording(&registry, &audit, SweepScope::CrashedOwners, &|pid| {
            Some(ProcessIdentity { pid, start_time_us: 999 })
        });

        assert!(killed.is_empty(), "a recycled pid must never be killed");
        assert_eq!(
            report.decisions,
            vec![SweepDecision::Refused {
                recorded_start_time_us: 111,
                actual_start_time_us: 999,
            }]
        );
        let log =
            std::fs::read_to_string(data_dir.join("audit").join("events.jsonl")).unwrap();
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

        let (report, killed) = sweep_recording(&registry, &audit, SweepScope::CrashedOwners, &|pid| {
            Some(ProcessIdentity { pid, start_time_us: 111 })
        });

        assert_eq!(killed, vec![5000]);
        assert_eq!(
            report.decisions,
            vec![SweepDecision::Killed { pid: 5000, pgid: 5000 }]
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
        // Owned by *us*, and we are alive: this is a second Damaian instance's
        // process from the sweeping instance's point of view.
        plant(&registry, &entry_owned_by(registry.owner(), 5000, 111));

        let (report, killed) = sweep_recording(&registry, &audit, SweepScope::CrashedOwners, &|pid| {
            Some(ProcessIdentity { pid, start_time_us: 111 })
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
        plant(&registry, &entry_owned_by(registry.owner(), 5000, 111));
        plant(&registry, &entry_owned_by(dead_owner(), 6000, 222));

        let (_, killed) = sweep_recording(&registry, &audit, SweepScope::OwnProcess, &|pid| {
            Some(ProcessIdentity { pid, start_time_us: if pid == 5000 { 111 } else { 222 } })
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
        let log =
            std::fs::read_to_string(data_dir.join("audit").join("events.jsonl")).unwrap();
        assert!(log.contains("process_registry_entry_unreadable"));
    }
```

- [ ] **Step 2: Run the tests to verify they fail**

```bash
cargo test -p workspace-engine --lib process_registry
```

Expected: FAIL to compile, `cannot find type SweepScope in this scope`.

- [ ] **Step 3: Write the implementation**

Add `use crate::audit::AuditLog;` to the imports, then:

```rust
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
```

Add `SweepDecision`, `SweepReport`, `SweepScope` to the `lib.rs` re-export list.

- [ ] **Step 4: Run the tests to verify they pass**

```bash
cargo test -p workspace-engine --lib process_registry
```

Expected: 14 passed.

- [ ] **Step 5: Run the full quality gate**

All seven commands from `AGENTS.md`.

- [ ] **Step 6: Show the change and ask before committing**

Suggested subject line: `Refuse to kill a recorded PID whose start time no longer matches`

---

## Task 4: The real kill

**Files:**
- Modify: `crates/workspace-engine/src/process_registry.rs`

**Interfaces:**
- Consumes: `SweepScope`, `sweep_with`, `RegisteredProcess`.
- Produces: `ProcessRegistry::sweep(&self, audit: &AuditLog) ->
  Result<SweepReport>` and `ProcessRegistry::sweep_own(&self, audit: &AuditLog)
  -> Result<SweepReport>`.

- [ ] **Step 1: Write the failing test**

This one spawns a real process, so it is `#[ignore]`d per `AGENTS.md`. Add it to
the `tests` module:

```rust
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

        // A shell whose child outlives a bare `kill(pid)`, so this test fails
        // if the group is not reached.
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

        let report = registry.sweep_own(&audit).unwrap();
        assert_eq!(report.killed(), 1);

        let deadline = Instant::now() + Duration::from_secs(10);
        while ProcessIdentity::of(pid).is_some() {
            assert!(Instant::now() < deadline, "the leader survived the sweep");
            std::thread::sleep(Duration::from_millis(20));
        }
        let _ = child.wait();
    }
```

- [ ] **Step 2: Run the test to verify it fails**

```bash
cargo test -p workspace-engine --lib -- --ignored --exact \
  process_registry::tests::terminating_reaches_the_whole_group
```

Expected: FAIL to compile, `no method named sweep_own`.

- [ ] **Step 3: Write the implementation**

```rust
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
```

- [ ] **Step 4: Run the test to verify it passes**

```bash
cargo test -p workspace-engine --lib -- --ignored --exact \
  process_registry::tests::terminating_reaches_the_whole_group
```

Expected: PASS. Then confirm no stragglers with `pgrep -fl "sleep 120"` — it
should print nothing.

- [ ] **Step 5: Run the full quality gate**

All seven commands from `AGENTS.md`. The new test is `#[ignore]`d, so
`cargo test --workspace --locked` must not run it.

- [ ] **Step 6: Show the change and ask before committing**

Suggested subject line: `Kill a swept process group with SIGTERM before SIGKILL`

---

## Task 5: Register MCP stdio servers

**Files:**
- Modify: `crates/workspace-engine/src/mcp.rs` (`StdioTransport` at `:281`,
  `spawn` at `:323`, `Drop` at `:395`, `McpClient::connect` at `:144`)
- Modify call sites: `crates/desktop-shell/src/lib.rs:1532`,
  `crates/desktop-shell/src/lib.rs:1815`,
  `crates/workspace-engine/src/mcp.rs:647`,
  `crates/workspace-engine/tests/foundation.rs:4568`

**Interfaces:**
- Consumes: `ProcessRegistry`, `ProcessKind::McpServer`, `RegistrationHandle`.
- Produces: `McpClient::connect(config: &McpServerConfig, auth_token:
  Option<String>, registry: &ProcessRegistry, session_id: &str) ->
  Result<McpClient>`.

- [ ] **Step 1: Write the failing test**

Add to `crates/workspace-engine/tests/foundation.rs`, next to the existing MCP
tests:

```rust
#[test]
fn an_mcp_stdio_server_is_registered_while_it_runs_and_not_after() {
    let data_dir = tempdir_for("mcp-registry");
    let registry = workspace_engine::ProcessRegistry::open(&data_dir).unwrap();
    let config = stdio_echo_server_config();

    let client = McpClient::connect(&config, None, &registry, "ses_1").expect("connect");
    let while_running = registry.entries().unwrap();
    assert_eq!(
        while_running.len(),
        1,
        "a server that is running must be recorded, or a crash leaks it"
    );
    assert_eq!(
        while_running[0].1.as_ref().unwrap().kind,
        workspace_engine::ProcessKind::McpServer.as_str()
    );

    drop(client);
    assert!(
        registry.entries().unwrap().is_empty(),
        "requirement 4: a clean shutdown leaves no entry"
    );
}
```

Reuse whatever helper the existing MCP test at `foundation.rs:4568` uses to
build a stdio server config and a temp directory; do not invent a second one.

- [ ] **Step 2: Run the test to verify it fails**

```bash
cargo test -p workspace-engine --test foundation an_mcp_stdio_server_is_registered
```

Expected: FAIL to compile — `connect` takes 2 arguments.

- [ ] **Step 3: Change `StdioTransport`**

In `mcp.rs`, add `use crate::process_registry::{ProcessKind, ProcessRegistry,
RegistrationHandle};` and `use std::os::unix::process::CommandExt;`, then give
the struct a handle field:

```rust
struct StdioTransport {
    child: Child,
    stdin: ChildStdin,
    lines: Receiver<String>,
    /// Removes this server's registry entry when the transport is dropped.
    /// Declared last so it is dropped after the child has been killed.
    _registration: RegistrationHandle,
}
```

In `spawn`, add the process-group call to the existing builder chain, just after
`.stderr(Stdio::null())`:

```rust
            // Its own group, so the sweep can reach a server that exec'd a
            // worker — `npx` leaving a `node` behind is the common case.
            .process_group(0);
```

and immediately after the existing `let mut child = command.spawn()...?;`:

```rust
        // Before anything else touches the child: a server that is running and
        // unrecorded is exactly what requirement 3 forbids.
        let registration = registry.register(ProcessKind::McpServer, session_id, child.id())?;
```

Change the signature to
`fn spawn(config: &McpServerConfig, registry: &ProcessRegistry, session_id: &str) -> Result<Self>`
and add `_registration: registration` to the returned struct literal.

- [ ] **Step 4: Thread it through `McpClient::connect`**

```rust
    pub fn connect(
        config: &McpServerConfig,
        auth_token: Option<String>,
        registry: &ProcessRegistry,
        session_id: &str,
    ) -> Result<Self> {
        let transport: Box<dyn RpcTransport> = match config.transport {
            McpTransport::Stdio => Box::new(StdioTransport::spawn(config, registry, session_id)?),
            McpTransport::Http => Box::new(HttpTransport::new(config, auth_token)?),
        };
```

- [ ] **Step 5: Update the four call sites**

Each already has an engine or config in scope, so the registry is one line:

- `crates/workspace-engine/src/mcp.rs:647` — thread `registry` and `session_id`
  in from that function's own parameters; add them to its signature and to its
  callers if it has any.
- `crates/desktop-shell/src/lib.rs:1532` and `:1815` — build with
  `ProcessRegistry::open(&engine.config.data_dir)?` and pass the session id that
  handler already has.
- `crates/workspace-engine/tests/foundation.rs:4568` — build a registry over the
  test's temp directory.

- [ ] **Step 6: Run the tests to verify they pass**

```bash
cargo test -p workspace-engine --test foundation
```

Expected: PASS, including the new test.

- [ ] **Step 7: Run the full quality gate**

All seven commands from `AGENTS.md`.

- [ ] **Step 8: Show the change and ask before committing**

Suggested subject line: `Register MCP stdio servers so a crash cannot leak them`

---

## Task 6: Register `curl` model calls

**Files:**
- Modify: `crates/workspace-engine/src/model.rs` (`CurlModelTransport` at `:467`,
  `send_stream` at `:503`, `KillOnDrop` at `:618`)
- Modify call sites: `crates/damaian-cli/src/main.rs:316,365`,
  `crates/eval-harness/src/runner.rs:135`,
  `crates/desktop-shell/src/lib.rs:653,1185,1237,1319,1366`, and the two tests
  at `crates/workspace-engine/src/model.rs:1513,1535`.

**Interfaces:**
- Consumes: `ProcessRegistry`, `ProcessKind::ModelCall`, `RegistrationHandle`.
- Produces: `CurlModelTransport::new(base_url: impl Into<String>, api_key: impl
  Into<String>, registry: ProcessRegistry) -> CurlModelTransport`.

- [ ] **Step 1: Write the failing test**

Add to the existing `#[cfg(test)] mod tests` in `model.rs`:

```rust
    #[test]
    fn a_model_call_transport_carries_a_registry_so_curl_can_be_swept() {
        let data_dir = std::env::temp_dir().join(format!(
            "damaian-model-registry-{}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&data_dir);
        let registry = crate::process_registry::ProcessRegistry::open(&data_dir).unwrap();
        let transport =
            CurlModelTransport::new("https://api.example.test/", "sk_test", registry);
        // No network: the point is that the transport holds the registry it
        // needs, so a `curl` it spawns is recordable.
        assert_eq!(transport.base_url, "https://api.example.test");
        assert!(transport.registry.entries().unwrap().is_empty());
    }
```

- [ ] **Step 2: Run the test to verify it fails**

```bash
cargo test -p workspace-engine --lib a_model_call_transport_carries_a_registry
```

Expected: FAIL to compile — `new` takes 2 arguments.

- [ ] **Step 3: Write the implementation**

```rust
pub struct CurlModelTransport {
    pub base_url: String,
    pub api_key: String,
    /// So a `curl` streaming a paid-for completion is swept if this process is
    /// killed before `KillOnDrop` can run.
    pub(crate) registry: ProcessRegistry,
}

impl CurlModelTransport {
    pub fn new(
        base_url: impl Into<String>,
        api_key: impl Into<String>,
        registry: ProcessRegistry,
    ) -> Self {
        Self {
            base_url: base_url.into().trim_end_matches('/').to_string(),
            api_key: api_key.into(),
            registry,
        }
    }
```

In `send_stream`, add `.process_group(0)` to the builder chain before `.spawn()?`
and register before wrapping in `KillOnDrop`:

```rust
        let child = Command::new("curl")
            .args(Self::curl_args())
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .process_group(0)
            .spawn()?;
        let registration =
            self.registry
                .register(ProcessKind::ModelCall, "", child.id())?;
        let mut child = KillOnDrop(child, registration);
```

and extend the guard so the entry goes when the child does:

```rust
/// Kills the child if it is still running when this is dropped, so a panic on
/// the calling thread cannot leave `curl` streaming a paid-for completion into
/// nothing for the rest of `max-time`. The second field removes the registry
/// entry on the same path; a `SIGKILL` that skips this leaves the entry for the
/// next launch's sweep, which is the point of recording it.
struct KillOnDrop(Child, RegistrationHandle);
```

`KillOnDrop::child` and the `Drop` body still use `self.0`, unchanged.

- [ ] **Step 4: Update the call sites**

Every real call site already has `engine.config.data_dir` or `config.data_dir`
in scope:

```rust
let transport = CurlModelTransport::new(
    &engine.config.model_base_url,
    api_key,
    ProcessRegistry::open(&engine.config.data_dir)?,
);
```

For the two tests inside `model.rs`, use a scratch directory the same way the
new test above does.

- [ ] **Step 5: Run the tests to verify they pass**

```bash
cargo test -p workspace-engine --lib model
```

Expected: PASS, including `curl_transport_does_not_put_api_key_in_argv` and the
other existing transport tests.

- [ ] **Step 6: Run the full quality gate**

All seven commands from `AGENTS.md`.

- [ ] **Step 7: Show the change and ask before committing**

Suggested subject line: `Register curl model calls so a killed turn stops billing`

---

## Task 7: Register shell commands

The source the original spec 17 analysis wrongly dismissed. See
[`proposal.md`](proposal.md) §2 for the measurement.

**Files:**
- Modify: `crates/workspace-engine/src/command_runner.rs:89-93`

**Interfaces:**
- Consumes: `ProcessRegistry`, `ProcessKind::Command`, `RegistrationHandle`.
- Produces: no signature change. `CommandRunner::new` keeps its four parameters
  and builds the registry from `config.data_dir` internally.

- [ ] **Step 1: Write the failing test**

Add to the `#[cfg(test)] mod tests` in `command_runner.rs`:

```rust
    /// A command that runs to completion must leave nothing behind, and the
    /// child must be in its own group so the sweep can reach a pipeline.
    ///
    /// `#[ignore]`d per `AGENTS.md` because it runs a real shell command. Run
    /// it by hand:
    ///
    /// ```sh
    /// cargo test -p workspace-engine --lib -- --ignored --exact \
    ///   command_runner::tests::a_completed_command_leaves_no_registry_entry
    /// ```
    #[test]
    #[ignore]
    fn a_completed_command_leaves_no_registry_entry() {
        let (runner, data_dir) = runner_with_scratch_data_dir();
        let registry = crate::process_registry::ProcessRegistry::open(&data_dir).unwrap();

        let execution = runner
            .run("echo hello", &data_dir, true, Some("local_user"), None)
            .expect("the command should run");

        assert_eq!(execution.exit_code, Some(0));
        assert!(execution.stdout.contains("hello"));
        assert!(
            registry.entries().unwrap().is_empty(),
            "requirement 4: a command that exited leaves no entry"
        );
    }
```

Match `run`'s real parameter list — read it at `command_runner.rs:50` rather
than trusting the call above — and write `runner_with_scratch_data_dir` to build
a `Config` whose `data_dir` is a fresh temp directory, following whatever
fixture pattern the surrounding tests already use.

- [ ] **Step 2: Run the test to verify it fails**

```bash
cargo test -p workspace-engine --lib -- --ignored --exact \
  command_runner::tests::a_completed_command_leaves_no_registry_entry
```

Expected: FAIL — the registry directory does not exist, or the assertion on
`entries()` cannot compile because nothing registers.

- [ ] **Step 3: Replace `output()` with `spawn()`**

At `command_runner.rs:89-93`, replace:

```rust
        let output = Command::new(&self.config.shell)
            .arg("-lc")
            .arg(command)
            .current_dir(cwd.as_ref())
            .output()?;
```

with:

```rust
        // `spawn` rather than `output` so the child's pid is knowable. `output`
        // hides it, which is the only reason spec 17 concluded a command could
        // not be cleaned up — it orphans under `SIGKILL` like anything else.
        // Its own group so a pipeline's members are reachable too.
        let child = Command::new(&self.config.shell)
            .arg("-lc")
            .arg(command)
            .current_dir(cwd.as_ref())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .process_group(0)
            .spawn()?;
        let mut guard = CommandGuard {
            pid: child.id(),
            armed: true,
            _registration: ProcessRegistry::open(&self.config.data_dir)?.register(
                ProcessKind::Command,
                task_id.unwrap_or_default(),
                child.id(),
            )?,
        };
        let output = child.wait_with_output()?;
        // Reaped, so the pid is free and its group may already be someone
        // else's. Disarm the kill — but let the guard drop normally, because
        // its handle is what removes the registry entry.
        guard.armed = false;
        drop(guard);
```

Add the imports `use std::os::unix::process::CommandExt;`, `use
std::process::Stdio;` and `use crate::process_registry::{ProcessKind,
ProcessRegistry, RegistrationHandle};`, and define the guard beside
`truncate_output`:

```rust
/// Kills the command's process group if this frame unwinds — a panic between
/// the spawn and the reap would otherwise leave the command running with
/// nothing waiting on it. A `SIGKILL` skips this, which is what the registry
/// entry is for.
///
/// `armed` is cleared once the child is reaped. After that its pid is free and
/// its group may belong to someone else, so killing would be exactly the
/// widening `proposal.md` §5.5 forbids. The handle still drops either way, so
/// the registry entry goes on both paths.
struct CommandGuard {
    pid: u32,
    armed: bool,
    _registration: RegistrationHandle,
}

impl Drop for CommandGuard {
    fn drop(&mut self) {
        if !self.armed {
            return;
        }
        // SAFETY: `kill` takes two integers by value and touches no memory.
        // The group id is this child's own pid, because it was spawned with
        // `process_group(0)`, and the child is known not to have been reaped.
        unsafe { libc::kill(-(self.pid as libc::pid_t), libc::SIGKILL) };
    }
}
```

- [ ] **Step 4: Run the tests to verify they pass**

```bash
cargo test -p workspace-engine --lib command_runner
cargo test -p workspace-engine --lib -- --ignored --exact \
  command_runner::tests::a_completed_command_leaves_no_registry_entry
```

Expected: PASS for both, and every pre-existing `command_runner` test still
passes — output truncation, the docker diagnostic, and the redaction tests all
read `output.stdout`/`output.stderr`, which `wait_with_output` fills exactly as
`output` did.

- [ ] **Step 5: Run the eval harness**

`AGENTS.md` requires this after touching tool-dispatch code:

```bash
cargo run -p eval-harness -- run --tier deterministic
```

Expected: the approval-denial and restricted-path scenarios still pass.

- [ ] **Step 6: Run the full quality gate**

All seven commands from `AGENTS.md`.

- [ ] **Step 7: Show the change and ask before committing**

Suggested subject line: `Spawn shell commands in their own group and register them`

---

## Task 8: Register PTY sessions

**Files:**
- Modify: `crates/desktop-shell/src/terminal.rs` (`PtySession` at `:23`, `open`
  at `:49`, `close` at `:168`)
- Modify call sites: `crates/desktop-shell/src/lib.rs:4711`,
  `crates/desktop-app/src/main.rs:155`

**Interfaces:**
- Consumes: `ProcessRegistry`, `ProcessKind::Terminal`, `RegistrationHandle`.
- Produces: `terminal::open(cwd: &Path, cols: u16, rows: u16, data_dir: &Path)
  -> Result<String, String>`.

- [ ] **Step 1: Write the failing test**

Add to the `#[cfg(test)] mod tests` in `crates/desktop-shell/src/lib.rs`, beside
the existing pty test at `:4711`:

```rust
    /// `#[ignore]`d per `AGENTS.md` because it spawns a real login shell. Run
    /// it by hand:
    ///
    /// ```sh
    /// cargo test -p desktop-shell --lib -- --ignored --exact \
    ///   tests::a_pty_session_is_registered_while_it_runs_and_not_after
    /// ```
    #[test]
    #[ignore]
    fn a_pty_session_is_registered_while_it_runs_and_not_after() {
        let cwd = std::env::temp_dir();
        let data_dir = std::env::temp_dir().join(format!(
            "damaian-pty-registry-{}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&data_dir);
        let registry =
            workspace_engine::ProcessRegistry::open(&data_dir).expect("registry");

        let id = super::terminal::open(&cwd, 80, 24, &data_dir).expect("open pty session");
        let while_running = registry.entries().unwrap();
        assert_eq!(while_running.len(), 1);
        assert_eq!(
            while_running[0].1.as_ref().unwrap().kind,
            workspace_engine::ProcessKind::Terminal.as_str()
        );

        super::terminal::close(&id);
        assert!(
            registry.entries().unwrap().is_empty(),
            "closing a terminal is a clean exit"
        );
    }
```

- [ ] **Step 2: Run the test to verify it fails**

```bash
cargo test -p desktop-shell --lib -- --ignored --exact \
  tests::a_pty_session_is_registered_while_it_runs_and_not_after
```

Expected: FAIL to compile — `open` takes 3 arguments.

- [ ] **Step 3: Write the implementation**

Give `PtySession` a handle field, declared after `child` so it drops last:

```rust
struct PtySession {
    master: Box<dyn MasterPty + Send>,
    writer: Box<dyn Write + Send>,
    child: Box<dyn portable_pty::Child + Send + Sync>,
    /// Output channel, taken by the streaming endpoint on first connect.
    receiver: Option<Receiver<Vec<u8>>>,
    /// Removes this session's registry entry when the session is dropped.
    _registration: RegistrationHandle,
}
```

In `open`, take `data_dir: &Path` and register right after `spawn_command`. The
pty child is already a session leader — `portable_pty` calls `setsid` so the
shell can own the terminal — so its process group already equals its pid and no
`process_group` call is needed or wanted here:

```rust
    let child = pair
        .slave
        .spawn_command(cmd)
        .map_err(|error| format!("failed to start shell: {error}"))?;
    let registration = {
        let pid = child
            .process_id()
            .ok_or_else(|| "the pty shell reported no pid".to_string())?;
        let registry = ProcessRegistry::open(data_dir).map_err(|error| error.to_string())?;
        registry
            .register(ProcessKind::Terminal, "", pid)
            .map_err(|error| error.to_string())?
    };
```

Add `_registration: registration` to the `PtySession` literal, and
`use workspace_engine::{ProcessKind, ProcessRegistry, RegistrationHandle};` to
the imports.

`close` needs no change: it removes the session from the map, and dropping the
`PtySession` drops the handle.

- [ ] **Step 4: Update the two call sites**

`crates/desktop-shell/src/lib.rs:4711` and `crates/desktop-app/src/main.rs:155`
both need a data directory. In the shell it is `engine.config.data_dir`; in the
Tauri app, resolve it the same way that file already resolves config.

- [ ] **Step 5: Run the tests to verify they pass**

```bash
cargo test -p desktop-shell --lib
cargo test -p desktop-shell --lib -- --ignored --exact \
  tests::a_pty_session_is_registered_while_it_runs_and_not_after
```

Expected: PASS.

- [ ] **Step 6: Run the full quality gate**

All seven commands from `AGENTS.md`.

- [ ] **Step 7: Show the change and ask before committing**

Suggested subject line: `Register PTY sessions so a crash does not leak a shell`

---

## Task 9: The shutdown handler

**Files:**
- Modify: `crates/workspace-engine/src/process_registry.rs`
- Create: `crates/workspace-engine/tests/process_registry.rs`

**Interfaces:**
- Consumes: `ProcessRegistry::sweep_own`, `AuditLog`.
- Produces: `ProcessRegistry::install_shutdown_handler(self, audit: AuditLog)
  -> Result<()>`.

- [ ] **Step 1: Write the failing test**

Create `crates/workspace-engine/tests/process_registry.rs`. The child is this
same test binary re-executed into a helper, following the pattern at
`crates/workspace-engine/tests/crash_recovery.rs:994`:

```rust
//! The real-process half of spec 46. Every test here spawns or kills something,
//! so every test here is `#[ignore]`d, per `AGENTS.md`.

use std::path::PathBuf;
use std::time::{Duration, Instant};

fn scratch(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("damaian-sweep-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

/// A silent child is the case that matters: one that writes would die of
/// `SIGPIPE` when the parent's pipe read-ends close, and the test would pass
/// without the handler doing anything at all.
///
/// `#[ignore]`d per `AGENTS.md` because it spawns and signals real processes.
/// Run it by hand:
///
/// ```sh
/// cargo test -p workspace-engine --test process_registry -- --ignored --exact \
///   sigint_cleans_up_a_silent_child_and_still_exits_signalled
/// ```
#[test]
#[ignore]
fn sigint_cleans_up_a_silent_child_and_still_exits_signalled() {
    use std::os::unix::process::ExitStatusExt;
    use std::process::Command;
    use workspace_engine::ProcessIdentity;

    let data_dir = scratch("sigint");
    let mut owner = Command::new(std::env::current_exe().unwrap())
        .args([
            "--ignored",
            "--exact",
            "sigint_helper_registers_a_silent_child_and_waits",
        ])
        .env("DAMAIAN_SWEEP_DIR", &data_dir)
        .spawn()
        .expect("owner should spawn");

    // Wait for the entry to reach disk rather than sleeping a guessed interval.
    let processes = data_dir.join("processes");
    let deadline = Instant::now() + Duration::from_secs(30);
    let child_pid = loop {
        if let Some(pid) = std::fs::read_dir(&processes)
            .ok()
            .and_then(|mut dir| dir.next())
            .and_then(|entry| entry.ok())
            .and_then(|entry| {
                entry
                    .file_name()
                    .to_str()
                    .and_then(|name| name.split('-').next())
                    .and_then(|pid| pid.parse::<u32>().ok())
            })
        {
            break pid;
        }
        assert!(Instant::now() < deadline, "the helper never registered");
        std::thread::sleep(Duration::from_millis(20));
    };

    // By pid, through the handle we own — never by name. `AGENTS.md`.
    // SAFETY: `kill` takes two integers by value and touches no memory.
    unsafe { libc::kill(owner.id() as libc::pid_t, libc::SIGINT) };
    let status = owner.wait().expect("owner should be reaped");

    assert_eq!(
        status.signal(),
        Some(libc::SIGINT),
        "the handler must re-raise so the exit status still says it was signalled"
    );

    let deadline = Instant::now() + Duration::from_secs(10);
    while ProcessIdentity::of(child_pid).is_some() {
        assert!(
            Instant::now() < deadline,
            "the silent child survived SIGINT, so the handler did nothing"
        );
        std::thread::sleep(Duration::from_millis(20));
    }
}

/// Re-executed by the test above. Not a test.
#[test]
#[ignore]
fn sigint_helper_registers_a_silent_child_and_waits() {
    use std::os::unix::process::CommandExt;
    use std::process::{Command, Stdio};
    use workspace_engine::{AuditLog, ProcessKind, ProcessRegistry, SecretScanner};

    let Ok(data_dir) = std::env::var("DAMAIAN_SWEEP_DIR") else {
        return; // Run directly rather than re-executed: nothing to do.
    };
    let registry = ProcessRegistry::open(&data_dir).unwrap();
    let audit = AuditLog::new(&data_dir, true, SecretScanner::new(Vec::new()));
    registry
        .clone()
        .install_shutdown_handler(audit)
        .expect("handler should install");

    let child = Command::new("/bin/sh")
        .arg("-lc")
        .arg("sleep 300")
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .process_group(0)
        .spawn()
        .unwrap();
    let handle = registry
        .register(ProcessKind::Command, "ses_1", child.id())
        .unwrap();
    // The entry must survive this process, exactly as a crash would leave it.
    std::mem::forget(handle);

    std::thread::sleep(Duration::from_secs(60));
}
```

Add `libc` to `crates/workspace-engine/Cargo.toml` under `[dev-dependencies]`
if it is not already reachable from tests.

- [ ] **Step 2: Run the test to verify it fails**

```bash
cargo test -p workspace-engine --test process_registry -- --ignored --exact \
  sigint_cleans_up_a_silent_child_and_still_exits_signalled
```

Expected: FAIL to compile — `no method named install_shutdown_handler`.

- [ ] **Step 3: Write the implementation**

Add to `process_registry.rs`:

```rust
use std::sync::atomic::{AtomicI32, Ordering};

/// The write end of the self-pipe. The signal handler may touch this and
/// nothing else.
static WAKE_WRITE: AtomicI32 = AtomicI32::new(-1);

/// The only code that runs in signal context.
///
/// `write` is on POSIX's async-signal-safe list. The identity-checked kill is
/// not — it needs `proc_pidinfo`, allocation and file I/O — so it happens on
/// the watchdog thread, which is ordinary code. Killing from here would mean
/// killing by number, which is the one thing `proposal.md` §1 forbids.
extern "C" fn on_signal(_signal: libc::c_int) {
    let fd = WAKE_WRITE.load(Ordering::Relaxed);
    if fd >= 0 {
        let byte = 1_u8;
        // SAFETY: a one-byte write to a pipe we created and never close. The
        // return value is deliberately ignored: there is no recovery available
        // in signal context, and a full pipe means a wake-up is already queued.
        unsafe {
            libc::write(fd, std::ptr::from_ref(&byte).cast::<libc::c_void>(), 1);
        }
    }
}

impl ProcessRegistry {
    /// Kills this instance's registered processes when the process is signalled.
    ///
    /// Call it from `main`, never from library code: a Tauri host and the test
    /// harness must keep their own signal dispositions.
    ///
    /// Registry entries are deliberately not removed here. A killed process is
    /// gone, so the next launch's sweep finds no identity for it and removes it
    /// as `orphan_process_already_exited`. Leaving the entry is also the honest
    /// record if this thread is itself killed part way through.
    pub fn install_shutdown_handler(self, audit: AuditLog) -> Result<()> {
        let mut fds = [0 as libc::c_int; 2];
        // SAFETY: `pipe` writes exactly two file descriptors into the array.
        if unsafe { libc::pipe(fds.as_mut_ptr()) } != 0 {
            return Err(ClientError::Io(
                "could not create the shutdown pipe".to_string(),
            ));
        }
        WAKE_WRITE.store(fds[1], Ordering::Relaxed);
        for signal in [libc::SIGINT, libc::SIGTERM, libc::SIGHUP] {
            // SAFETY: installing a handler that only calls `write`.
            unsafe { libc::signal(signal, on_signal as *const () as libc::sighandler_t) };
        }

        let read_fd = fds[0];
        std::thread::spawn(move || {
            let mut byte = [0_u8; 1];
            // SAFETY: a one-byte read into a local buffer from a pipe we own.
            let read = unsafe {
                libc::read(read_fd, byte.as_mut_ptr().cast::<libc::c_void>(), 1)
            };
            if read != 1 {
                return;
            }
            // Ordinary code from here, so the full identity check is available.
            let _ = self.sweep_own(&audit);
            // Restore the default disposition and re-raise, so the process
            // reports the correct `WIFSIGNALED` status to whatever ran it —
            // and so a second signal forces an exit if this ever wedges.
            // SAFETY: both calls take integers by value.
            unsafe {
                libc::signal(libc::SIGINT, libc::SIG_DFL);
                libc::raise(libc::SIGINT);
            }
        });
        Ok(())
    }
}
```

- [ ] **Step 4: Run the test to verify it passes**

```bash
cargo test -p workspace-engine --test process_registry -- --ignored --exact \
  sigint_cleans_up_a_silent_child_and_still_exits_signalled
```

Expected: PASS. Then confirm with `pgrep -fl "sleep 300"` that nothing is left.

- [ ] **Step 5: Run the full quality gate**

All seven commands from `AGENTS.md`. Both tests in the new file are
`#[ignore]`d, so `cargo test --workspace --locked` must not run either.

- [ ] **Step 6: Show the change and ask before committing**

Suggested subject line: `Kill registered processes on Ctrl-C through a self-pipe handler`

---

## Task 10: Sweep at launch, install the handler

The wiring that makes the previous nine tasks do anything.

**Files:**
- Modify: `crates/desktop-shell/src/lib.rs` (`run_server_with_ready` at `:70`)
- Modify: `crates/desktop-shell/src/main.rs`, `crates/damaian-cli/src/main.rs`

**Interfaces:**
- Consumes: `ProcessRegistry::open`, `sweep`, `install_shutdown_handler`.
- Produces: nothing new.

- [ ] **Step 1: Write the failing test**

Add to the `#[cfg(test)] mod tests` in `crates/desktop-shell/src/lib.rs`:

```rust
    #[test]
    fn startup_sweeps_an_entry_left_by_a_crashed_instance() {
        let data_dir = std::env::temp_dir().join(format!(
            "damaian-startup-sweep-{}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&data_dir);
        std::fs::create_dir_all(data_dir.join("processes")).unwrap();
        // An entry naming a pid that cannot exist, owned by an instance that
        // cannot exist: the sweep must read it, decide, and remove it.
        std::fs::write(
            data_dir.join("processes").join("4000002-111.json"),
            "{\"pid\":4000002,\"startTimeUs\":111,\"pgid\":4000002,\"kind\":\"command\",\
             \"sessionId\":\"ses_1\",\"registeredAtMs\":0,\"ownerPid\":4000001,\
             \"ownerStartTimeUs\":222}",
        )
        .unwrap();

        super::sweep_orphaned_processes(&data_dir).expect("the sweep should run");

        assert!(
            std::fs::read_dir(data_dir.join("processes"))
                .unwrap()
                .next()
                .is_none(),
            "a spent entry is removed so it is not re-decided at every launch"
        );
    }
```

- [ ] **Step 2: Run the test to verify it fails**

```bash
cargo test -p desktop-shell --lib startup_sweeps_an_entry
```

Expected: FAIL to compile — `cannot find function sweep_orphaned_processes`.

- [ ] **Step 3: Write the implementation**

In `crates/desktop-shell/src/lib.rs`, beside `verify_data_dir_schema_at`:

```rust
/// Kills what a crashed instance left running, per
/// `docs/specs/46_process_registry_and_orphan_sweep/proposal.md` §5.7.
///
/// Eagerly at startup rather than inside `recovery::sweep_once`, which is
/// memoized on the first HTTP request: an orphan must not outlive the crash
/// just because nobody opened the UI. An entry whose owner is still alive is
/// skipped, so running this from both front ends is a no-op the second time.
pub fn sweep_orphaned_processes(data_dir: &Path) -> Result<(), String> {
    let config = Config::load_for_repository(None).map_err(|error| error.to_string())?;
    let scanner = SecretScanner::new(config.secret_patterns.clone());
    let audit = AuditLog::with_retention(
        data_dir,
        config.audit_enabled,
        config.audit_retention_days,
        scanner,
    );
    let registry = ProcessRegistry::open(data_dir).map_err(|error| error.to_string())?;
    let report = registry.sweep(&audit).map_err(|error| error.to_string())?;
    if report.killed() > 0 || report.refused() > 0 {
        println!(
            "Orphan sweep: killed {}, refused {} on a start-time mismatch",
            report.killed(),
            report.refused()
        );
    }
    Ok(())
}
```

In `run_server_with_ready`, immediately after `verify_data_dir_schema()?;`:

```rust
    let config = Config::load_for_repository(None).map_err(|error| error.to_string())?;
    // Before the port is bound, so an orphan never overlaps a new session.
    sweep_orphaned_processes(&config.data_dir)?;
```

In both `crates/desktop-shell/src/main.rs` and
`crates/damaian-cli/src/main.rs`, at the top of `main`, install the handler:

```rust
    let config = Config::load_for_repository(None)?;
    let scanner = SecretScanner::new(config.secret_patterns.clone());
    let audit = AuditLog::with_retention(
        &config.data_dir,
        config.audit_enabled,
        config.audit_retention_days,
        scanner,
    );
    ProcessRegistry::open(&config.data_dir)?.install_shutdown_handler(audit)?;
```

and in the CLI only, run the launch sweep too — the CLI spawns `curl` and MCP
servers and may be the only front end a user runs.

- [ ] **Step 4: Run the test to verify it passes**

```bash
cargo test -p desktop-shell --lib startup_sweeps_an_entry
```

Expected: PASS.

- [ ] **Step 5: Verify it by hand against the real app**

Per `AGENTS.md`, use a port and data directory of your own — never take 4765 and
never `pkill` by name:

```bash
DAMAIAN_DATA_DIR=.damaian cargo run -p desktop-shell -- --port 4899
```

Start it, open a terminal in the UI, note the pid from
`.damaian/processes/`, `kill -9` the shell's own pid (the one you started, by
PID), restart it, and confirm the pty shell is gone and the audit log has an
`orphan_process_killed` event.

- [ ] **Step 6: Run the eval harness and the full quality gate**

```bash
cargo run -p eval-harness -- run --tier deterministic
```

then all seven commands from `AGENTS.md`.

- [ ] **Step 7: Show the change and ask before committing**

Suggested subject line: `Sweep orphaned processes at launch and on shutdown`

---

## Task 11: Documentation

**Files:**
- Modify: `docs/specs/46_process_registry_and_orphan_sweep/proposal.md`,
  `docs/specs/46_process_registry_and_orphan_sweep/tasks.md`,
  `docs/specs/README.md`, `docs/TROUBLESHOOTING.md`

- [ ] **Step 1: Fill in §7 of the proposal**

Record what the implementation found that the design did not predict — the
convention in this repo is that §7 is written from experience, not restated from
§5. If nothing surprised you, say that in one line rather than padding it.

- [ ] **Step 2: Set the status**

`Status: Done` in `proposal.md`, and `**Started:** not yet` in this file becomes
the date the work began.

- [ ] **Step 3: Update the specs README**

Row 46 still describes three sources and a session-scoped registry. Rewrite it
for what shipped: four sources, an owner-scoped registry, and the launch sweep
plus the signal handler. Mark it Done.

- [ ] **Step 4: Add the diagnostic to TROUBLESHOOTING.md**

Readers need to know `<data_dir>/processes/` exists, what a file in it means, and
that `orphan_process_killed`, `orphan_process_kill_refused`,
`orphan_process_already_exited` and `process_registry_entry_unreadable` are the
audit events to grep for. Follow the structure of the sections already there.

- [ ] **Step 5: Run the full quality gate**

All seven commands from `AGENTS.md`. `typos` is the one that catches
documentation changes.

- [ ] **Step 6: Show the change and ask before committing**

Suggested subject line: `Document the process registry and the orphan sweep`

---

## Self-Review

Checked against [`proposal.md`](proposal.md) after writing:

| Proposal | Task |
|---|---|
| §3 requirement 1 (cleaned up, never by name) | 3, 4 |
| §3 requirement 2 (alive *and* start time matches) | 1, 3 |
| §3 requirement 3 (survives `SIGKILL`, written at spawn) | 2, 5–8 |
| §3 requirement 4 (clean exit leaves no entry) | 2, 5–8 |
| §3 requirement 5 (audited, refusals included) | 3 |
| §3 requirement 6 (never touches a live instance) | 3 |
| §5.1 start time | 1 |
| §5.2 one file per process | 2 |
| §5.3 owner identity | 2, 3 |
| §5.4 decision table | 3, 4 |
| §5.5 process groups, leader-gated | 4, 5, 6, 7 |
| §5.6 shutdown handler | 9 |
| §5.7 where the sweep runs | 10 |
| §5.8 wiring | 5, 6, 7, 8 |
| §5.9 testing | every task |
| §6 acceptance criteria | 3 (refusal, live owner), 4 and 9 (nothing left running), 2 (clean exit), 10 (by hand) |

Two things a reader should know about the plan rather than the design:

- **Task 7's guard disarms rather than being forgotten.** After
  `wait_with_output` reaps, the child's pid is free and its group may already be
  someone else's, so the kill must not run — but the registry entry must still
  be removed. A `mem::forget` would suppress both and fail that task's own
  test; clearing `armed` and dropping normally suppresses only the kill.
- **Task 6 registers `curl` with an empty session id.** `CurlModelTransport` has
  no session in scope and threading one through eleven call sites for an audit
  field is not worth it. If a later spec needs per-session attribution for model
  calls, that is the change to make then.

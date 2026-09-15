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

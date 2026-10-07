//! An engine's process group. Each engine is started as the leader of a group of its own
//! (`process_group(0)`), so every process it forks (vLLM's `EngineCore`, multiprocessing's
//! resource tracker) is in that group, and ending the engine signals the group: `SIGTERM`, a
//! bounded wait, then `SIGKILL`. The group id is the pid of the child inferd itself created,
//! never found by a name or a pattern.
//!
//! What covers each way an engine can end:
//! - the supervisor stops or evicts it, or a start never becomes ready: [`end`] (the whole group,
//!   before the engine is reported gone, so the next engine finds the memory free);
//! - the engine's leader exits or crashes by itself: [`sweep`] kills what is left of its group;
//! - inferd exits normally (returns, or unwinds): `ProcessHost`'s drop kills every group still
//!   running;
//! - inferd is killed (`SIGKILL`, the OOM killer): nothing of inferd runs. The kernel kills the
//!   direct child (`PR_SET_PDEATHSIG`, [`die_with_parent`]), which reaches no grandchild; those
//!   are left to the cgroup of the unit inferd runs in (`KillMode=control-group` in
//!   `dist/inferd.service`). Outside a unit (tests, harnesses) a grandchild of a killed inferd
//!   lives on.

use rustix::process::{Pid, Signal, kill_process_group, test_kill_process_group};
use std::time::Duration;
use tokio::process::Command;
use tokio::time::Instant;

/// How often a group that is going is looked at.
const LOOK: Duration = Duration::from_millis(25);

/// The group a child leads: its pid (the child was started with `process_group(0)`).
pub fn group_of(child: &tokio::process::Child) -> Option<Pid> {
    child
        .id()
        .and_then(|id| i32::try_from(id).ok())
        .and_then(Pid::from_raw)
}

/// Whether anything is still in the group (a process that has exited and not been waited for
/// still counts).
pub fn alive(group: Pid) -> bool {
    test_kill_process_group(group).is_ok()
}

/// Sends `SIGTERM` to the group.
pub fn terminate(group: Pid) {
    let _ = kill_process_group(group, Signal::TERM);
}

/// The leader is gone (and waited for): whatever is left of the group is killed at once.
pub fn sweep(group: Pid) {
    if alive(group) {
        let _ = kill_process_group(group, Signal::KILL);
    }
}

/// After the leader has been waited for: the rest of the group, which has had its `SIGTERM`, is
/// given `grace` to go, then killed.
pub async fn end(group: Pid, grace: Duration) {
    let deadline = Instant::now() + grace;
    while alive(group) && Instant::now() < deadline {
        tokio::time::sleep(LOOK).await;
    }
    sweep(group);
}

/// Makes the kernel send the child `SIGKILL` when the process that started it dies. It covers the
/// child only, not its children; the group does that where inferd lives to run [`end`].
#[allow(unsafe_code)]
pub fn die_with_parent(command: &mut Command) {
    // SAFETY: the closure makes one system call (`prctl`) between fork and exec, which is
    // async-signal-safe, and allocates nothing.
    unsafe {
        command.pre_exec(|| {
            rustix::process::set_parent_process_death_signal(Some(Signal::KILL))
                .map_err(std::io::Error::from)
        });
    }
}

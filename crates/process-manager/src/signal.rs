//! Unix process-group signalling helpers.
//!
//! The child is spawned with `process_group(0)`, which makes the child pid the
//! process-group id. Sending a signal to `-pid` therefore reaches the child and
//! all of its descendants that stayed in the group.

#[cfg(unix)]
fn send_group_signal(pid: u32, signal: libc::c_int) {
    if pid == 0 || pid > i32::MAX as u32 {
        return;
    }
    // SAFETY: `kill(2)` is safe to call with a negative pid; the negative pid
    // selects the process group. Errors (for example `ESRCH` when the process
    // already exited) are intentionally ignored.
    unsafe {
        let _ = libc::kill(-(pid as libc::pid_t), signal);
    }
}

/// Send `SIGINT` to the process group led by `pid`.
#[cfg(unix)]
pub(crate) fn interrupt_group(pid: u32) {
    send_group_signal(pid, libc::SIGINT);
}

/// Send `SIGTERM` to the process group led by `pid`.
#[cfg(unix)]
pub(crate) fn terminate_group(pid: u32) {
    send_group_signal(pid, libc::SIGTERM);
}

/// Send `SIGKILL` to the process group led by `pid`.
#[cfg(unix)]
pub(crate) fn kill_group(pid: u32) {
    send_group_signal(pid, libc::SIGKILL);
}

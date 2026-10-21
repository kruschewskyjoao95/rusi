#[cfg(unix)]
mod sys {
    unsafe extern "C" {
        pub fn kill(pid: i32, sig: i32) -> i32;
    }
}

/// Sends `SIGKILL` to the process group associated with `pid` on Unix systems.
#[cfg(unix)]
pub(crate) fn kill_process_group(pid: u32) {
    if pid > 1 {
        // Send SIGKILL (signal 9) to the process group (-pid)
        unsafe {
            let _ = sys::kill(-(pid as i32), 9);
        }
    }
}

#[cfg(not(unix))]
pub(crate) fn kill_process_group(_pid: u32) {}

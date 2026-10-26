//! Graceful stop on Windows logoff / reboot / session end.
//!
//! Sleep/hibernate is handled via power notifications. Reboot/shutdown for a
//! `windows_subsystem = "windows"` agent does **not** reliably deliver
//! `CTRL_SHUTDOWN_EVENT` (GUI apps) — use `WM_QUERYENDSESSION` /
//! `WM_ENDSESSION` on the power message window (see `power.rs`) plus this
//! flag. Without a graceful stop, FFmpeg is killed mid-encode and leaves
//! `*.partial.mp4` behind.

use std::sync::atomic::{AtomicBool, Ordering};
use std::thread;
use std::time::Duration;

use windows::core::BOOL;
use windows::Win32::Foundation::{FALSE, TRUE};
use windows::Win32::System::Console::{
    CTRL_CLOSE_EVENT, CTRL_LOGOFF_EVENT, CTRL_SHUTDOWN_EVENT, SetConsoleCtrlHandler,
};

static REQUESTED: AtomicBool = AtomicBool::new(false);
static DRAINED: AtomicBool = AtomicBool::new(false);
static INSTALLED: AtomicBool = AtomicBool::new(false);

/// Install process-wide console control handler (idempotent).
/// Useful for debug/console builds; reboot for GUI agents relies on WM_ENDSESSION.
pub fn install() {
    if INSTALLED.swap(true, Ordering::SeqCst) {
        return;
    }
    unsafe {
        let ok = SetConsoleCtrlHandler(Some(ctrl_handler), true);
        if ok.is_err() {
            crate::debug_log::write(format!("shutdown handler install failed: {ok:?}"));
        } else {
            crate::debug_log::write("shutdown handler installed (logoff/reboot console path)");
        }
    }
}

/// Signal the agent to stop recording and flush (from WM_QUERYENDSESSION / ctrl).
pub fn request() {
    DRAINED.store(false, Ordering::Release);
    REQUESTED.store(true, Ordering::Release);
}

pub fn requested() -> bool {
    REQUESTED.load(Ordering::Acquire)
}

/// Call when the agent has finished stopping capture and flushing uploads.
pub fn mark_drained() {
    DRAINED.store(true, Ordering::Release);
}

pub fn drained() -> bool {
    DRAINED.load(Ordering::Acquire)
}

/// Block the calling thread briefly so finalize can finish (ENDSESSION / ctrl).
pub fn wait_for_drain(max: Duration) {
    let deadline = std::time::Instant::now() + max;
    while std::time::Instant::now() < deadline {
        if drained() {
            break;
        }
        thread::sleep(Duration::from_millis(100));
    }
}

unsafe extern "system" fn ctrl_handler(ctrl_type: u32) -> BOOL {
    match ctrl_type {
        CTRL_CLOSE_EVENT | CTRL_LOGOFF_EVENT | CTRL_SHUTDOWN_EVENT => {
            crate::debug_log::write(format!("shutdown signal ctrl={ctrl_type}"));
            request();
            // Block the system briefly so the agent loop can finalize FFmpeg.
            wait_for_drain(Duration::from_secs(20));
            TRUE
        }
        _ => FALSE,
    }
}

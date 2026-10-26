//! Cross-process limit on concurrent recording sessions + agent instance checks.

use std::time::Duration;

use windows::Win32::Foundation::{
    CloseHandle, ERROR_ALREADY_EXISTS, GetLastError, HANDLE, WAIT_ABANDONED, WAIT_OBJECT_0,
    WAIT_TIMEOUT,
};
use windows::Win32::System::Diagnostics::ToolHelp::{
    CreateToolhelp32Snapshot, PROCESSENTRY32W, Process32FirstW, Process32NextW, TH32CS_SNAPPROCESS,
};
use windows::Win32::System::Threading::{CreateMutexW, ReleaseMutex, WaitForSingleObject};
use windows::core::{HSTRING, PCWSTR};

use crate::i18n::Message;
use crate::msg;

const SLOT_PREFIX: &str = "Local\\OfficePC-RecorderSession-";
pub const AGENT_INSTANCE_MUTEX: &str = "Local\\OfficePC-RecorderAgent-Instance";
const MAX_SLOTS: u32 = 8;
const AGENT_IMAGE: &str = "recorder-agent.exe";

/// Held for the lifetime of one recording. Dropping releases the slot.
pub struct SessionGuard {
    handle: HANDLE,
}

impl Drop for SessionGuard {
    fn drop(&mut self) {
        unsafe {
            let _ = ReleaseMutex(self.handle);
            let _ = CloseHandle(self.handle);
        }
    }
}

// SAFETY: exclusive ownership of the mutex handle for this process.
unsafe impl Send for SessionGuard {}

fn wait_acquired(code: windows::Win32::Foundation::WAIT_EVENT) -> bool {
    code == WAIT_OBJECT_0 || code == WAIT_ABANDONED
}

/// Try to reserve one of `max_sessions` global recording slots (default 1).
pub fn try_acquire(max_sessions: u32) -> Result<SessionGuard, Message> {
    let max = max_sessions.clamp(1, MAX_SLOTS);
    for index in 0..max {
        let name = HSTRING::from(format!("{SLOT_PREFIX}{index}"));
        let handle =
            unsafe { CreateMutexW(None, false, PCWSTR(name.as_ptr())) }.map_err(|error| {
                msg!(
                    "Could not create recording lock: {error}",
                    "Не удалось создать блокировку записи: {error}"
                )
            })?;
        let wait = unsafe { WaitForSingleObject(handle, 0) };
        if wait_acquired(wait) {
            return Ok(SessionGuard { handle });
        }
        if wait != WAIT_TIMEOUT {
            crate::debug_log::write(format!(
                "session WaitForSingleObject unexpected code={}",
                wait.0
            ));
        }
        let _ = unsafe { CloseHandle(handle) };
    }
    Err(msg!(
        "Recording limit reached (max {max}). Stop the other session first.",
        "Достигнут лимит записей (макс. {max}). Сначала остановите другую сессию."
    ))
}

/// Claim the single-instance lock for the agent (held until process exit).
pub fn try_claim_agent_instance() -> Result<SessionGuard, Message> {
    let name = HSTRING::from(AGENT_INSTANCE_MUTEX);
    let handle = unsafe { CreateMutexW(None, false, PCWSTR(name.as_ptr())) }.map_err(|error| {
        msg!(
            "Could not create agent lock: {error}",
            "Не удалось создать блокировку агента: {error}"
        )
    })?;
    let wait = unsafe { WaitForSingleObject(handle, 0) };
    if wait_acquired(wait) {
        Ok(SessionGuard { handle })
    } else {
        let _ = unsafe { CloseHandle(handle) };
        Err(msg!(
            "Recorder agent is already running.",
            "Агент записи уже запущен."
        ))
    }
}

/// True only when a live `recorder-agent.exe` process exists (not a stale mutex).
pub fn agent_already_running() -> bool {
    agent_process_running()
}

/// Enumerate processes and look for recorder-agent.exe (case-insensitive).
pub fn agent_process_running() -> bool {
    unsafe {
        let snap = match CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0) {
            Ok(h) => h,
            Err(_) => return false,
        };
        let mut entry = PROCESSENTRY32W {
            dwSize: std::mem::size_of::<PROCESSENTRY32W>() as u32,
            ..Default::default()
        };
        let mut found = false;
        if Process32FirstW(snap, &mut entry).is_ok() {
            loop {
                let name = String::from_utf16_lossy(
                    entry
                        .szExeFile
                        .iter()
                        .take_while(|&&c| c != 0)
                        .copied()
                        .collect::<Vec<_>>()
                        .as_slice(),
                );
                if name.eq_ignore_ascii_case(AGENT_IMAGE) {
                    found = true;
                    break;
                }
                if Process32NextW(snap, &mut entry).is_err() {
                    break;
                }
            }
        }
        let _ = CloseHandle(snap);
        found
    }
}

/// Force-stop every recorder-agent.exe via taskkill (used after graceful timeout).
pub fn force_kill_agent_processes() {
    use std::os::windows::process::CommandExt;
    let _ = std::process::Command::new("taskkill")
        .args(["/IM", AGENT_IMAGE, "/F", "/T"])
        .creation_flags(0x08000000) // CREATE_NO_WINDOW
        .output();
    // Brief settle so mutex / handles release.
    std::thread::sleep(Duration::from_millis(400));
}

//! Cross-process limit on concurrent recording sessions.

use windows::Win32::Foundation::{
    CloseHandle, ERROR_ALREADY_EXISTS, GetLastError, HANDLE, WAIT_ABANDONED, WAIT_OBJECT_0,
    WAIT_TIMEOUT,
};
use windows::Win32::System::Threading::{CreateMutexW, ReleaseMutex, WaitForSingleObject};
use windows::core::{HSTRING, PCWSTR};

use crate::i18n::Message;
use crate::msg;

const SLOT_PREFIX: &str = "Local\\OfficePC-RecorderSession-";
pub const AGENT_INSTANCE_MUTEX: &str = "Local\\OfficePC-RecorderAgent-Instance";
const MAX_SLOTS: u32 = 8;

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

/// Claim the single-instance lock for the tray agent (held until process exit).
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

/// Soft check used by the console before spawning another agent process.
pub fn agent_already_running() -> bool {
    let name = HSTRING::from(AGENT_INSTANCE_MUTEX);
    let Ok(handle) = (unsafe { CreateMutexW(None, false, PCWSTR(name.as_ptr())) }) else {
        return false;
    };
    let already = unsafe { GetLastError() == ERROR_ALREADY_EXISTS };
    if !already {
        let _ = unsafe { CloseHandle(handle) };
        return false;
    }
    let wait = unsafe { WaitForSingleObject(handle, 0) };
    if wait_acquired(wait) {
        // Abandoned / free — not a live agent.
        let _ = unsafe { ReleaseMutex(handle) };
        let _ = unsafe { CloseHandle(handle) };
        return false;
    }
    let _ = unsafe { CloseHandle(handle) };
    true
}

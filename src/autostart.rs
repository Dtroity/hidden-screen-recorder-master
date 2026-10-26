//! Visible logon autostart via HKCU Run (avoids schtasks OEM/encoding issues).

use std::path::PathBuf;

use windows::Win32::Foundation::{ERROR_FILE_NOT_FOUND, ERROR_SUCCESS, WIN32_ERROR};
use windows::Win32::System::Registry::{
    HKEY_CURRENT_USER, KEY_READ, KEY_SET_VALUE, REG_OPTION_NON_VOLATILE, REG_SZ, RegCloseKey,
    RegCreateKeyExW, RegDeleteValueW, RegOpenKeyExW, RegQueryValueExW, RegSetValueExW,
};
use windows::core::{HSTRING, PCWSTR};

use crate::i18n::Message;
use crate::msg;

pub const RUN_VALUE_NAME: &str = "OfficePC-RecorderAgent";
const RUN_SUBKEY: &str = r"Software\Microsoft\Windows\CurrentVersion\Run";

fn agent_exe_path() -> Result<PathBuf, Message> {
    let current = std::env::current_exe().map_err(|error| {
        msg!(
            "Could not resolve current executable: {error}",
            "Не удалось определить путь к программе: {error}"
        )
    })?;
    let dir = current.parent().ok_or_else(|| {
        msg!(
            "Executable directory is unavailable",
            "Каталог программы недоступен"
        )
    })?;
    let agent = dir.join("recorder-agent.exe");
    if agent.is_file() {
        Ok(agent)
    } else if current
        .file_stem()
        .and_then(|s| s.to_str())
        .is_some_and(|s| s.eq_ignore_ascii_case("recorder-agent"))
    {
        Ok(current)
    } else {
        Err(msg!(
            "recorder-agent.exe not found next to the console. Put both exe files in the same folder.",
            "Рядом с консолью нет recorder-agent.exe. Положите оба exe в одну папку."
        ))
    }
}

fn quoted_command(path: &std::path::Path) -> String {
    format!("\"{}\"", path.display())
}

fn win_err(error: WIN32_ERROR) -> windows::core::Result<()> {
    error.ok()
}

/// Register the agent in the current user's Run key (Task Manager → Startup apps)
/// and a watchdog task that restarts the agent after crashes (not after Stop-Agent.bat).
pub fn enable() -> Result<(), Message> {
    let exe = agent_exe_path()?;
    let command = quoted_command(&exe);
    set_run_value(&command).map_err(|error| {
        msg!(
            "Could not enable autostart: {error}",
            "Не удалось включить автозапуск: {error}"
        )
    })?;
    // Clean up the Task Scheduler entry from older builds (ignore failures).
    let _ = std::process::Command::new("schtasks")
        .args(["/Delete", "/TN", "OfficePC-RecorderAgent", "/F"])
        .output();
    let _ = enable_watchdog_task();
    // Allow watchdog / Run key to start the agent again.
    if let Some(flag) = admin_stopped_flag_path() {
        let _ = std::fs::remove_file(flag);
    }
    Ok(())
}

pub fn disable() -> Result<(), Message> {
    delete_run_value().map_err(|error| {
        msg!(
            "Could not disable autostart: {error}",
            "Не удалось отключить автозапуск: {error}"
        )
    })?;
    let _ = std::process::Command::new("schtasks")
        .args(["/Delete", "/TN", "OfficePC-RecorderAgent", "/F"])
        .output();
    let _ = std::process::Command::new("schtasks")
        .args(["/Delete", "/TN", "OfficePC-RecorderAgentWatchdog", "/F"])
        .output();
    Ok(())
}

fn admin_stopped_flag_path() -> Option<PathBuf> {
    agent_exe_path()
        .ok()
        .and_then(|p| p.parent().map(|d| d.join("agent.admin-stopped")))
}

fn watchdog_bat_path() -> Option<PathBuf> {
    let exe = agent_exe_path().ok()?;
    let dir = exe.parent()?;
    // Prefer Watch-Agent.bat next to package root (parent of bin\) or next to exe.
    let candidates = [
        dir.join("Watch-Agent.bat"),
        dir.parent().map(|p| p.join("Watch-Agent.bat")).unwrap_or_default(),
    ];
    candidates.into_iter().find(|p| p.is_file())
}

fn enable_watchdog_task() -> Result<(), Message> {
    let Some(watch) = watchdog_bat_path() else {
        return Ok(());
    };
    let tr = format!("\"{}\"", watch.display());
    let output = std::process::Command::new("schtasks")
        .args([
            "/Create",
            "/TN",
            "OfficePC-RecorderAgentWatchdog",
            "/SC",
            "MINUTE",
            "/MO",
            "2",
            "/TR",
            &tr,
            "/F",
        ])
        .output()
        .map_err(|error| {
            msg!(
                "Could not create watchdog task: {error}",
                "Не удалось создать задачу watchdog: {error}"
            )
        })?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(msg!(
            "Could not create watchdog task: {stderr}",
            "Не удалось создать задачу watchdog: {stderr}"
        ));
    }
    Ok(())
}

pub fn is_enabled() -> bool {
    matches!(read_run_value(), Ok(Some(value)) if !value.trim().is_empty())
}

pub fn launch_agent() -> Result<(), Message> {
    if crate::session::agent_already_running() {
        return Err(msg!(
            "Recorder agent is already running.",
            "Агент записи уже запущен."
        ));
    }
    let exe = agent_exe_path()?;
    std::process::Command::new(&exe).spawn().map_err(|error| {
        msg!(
            "Could not start recorder agent {}: {error}",
            "Не удалось запустить агент {}: {error}",
            exe.display()
        )
    })?;
    Ok(())
}

fn set_run_value(command: &str) -> windows::core::Result<()> {
    unsafe {
        let mut key = Default::default();
        let subkey = HSTRING::from(RUN_SUBKEY);
        let name = HSTRING::from(RUN_VALUE_NAME);
        win_err(RegCreateKeyExW(
            HKEY_CURRENT_USER,
            PCWSTR(subkey.as_ptr()),
            None,
            None,
            REG_OPTION_NON_VOLATILE,
            KEY_SET_VALUE,
            None,
            &mut key,
            None,
        ))?;
        let wide: Vec<u16> = command.encode_utf16().chain(std::iter::once(0)).collect();
        let bytes = std::slice::from_raw_parts(
            wide.as_ptr() as *const u8,
            wide.len() * std::mem::size_of::<u16>(),
        );
        let result = RegSetValueExW(key, PCWSTR(name.as_ptr()), None, REG_SZ, Some(bytes));
        let _ = RegCloseKey(key);
        win_err(result)
    }
}

fn delete_run_value() -> windows::core::Result<()> {
    unsafe {
        let mut key = Default::default();
        let subkey = HSTRING::from(RUN_SUBKEY);
        let name = HSTRING::from(RUN_VALUE_NAME);
        let open = RegOpenKeyExW(
            HKEY_CURRENT_USER,
            PCWSTR(subkey.as_ptr()),
            None,
            KEY_SET_VALUE,
            &mut key,
        );
        if open != ERROR_SUCCESS {
            return Ok(());
        }
        let result = RegDeleteValueW(key, PCWSTR(name.as_ptr()));
        let _ = RegCloseKey(key);
        if result == ERROR_SUCCESS || result == ERROR_FILE_NOT_FOUND {
            Ok(())
        } else {
            win_err(result)
        }
    }
}

fn read_run_value() -> windows::core::Result<Option<String>> {
    unsafe {
        let mut key = Default::default();
        let subkey = HSTRING::from(RUN_SUBKEY);
        let name = HSTRING::from(RUN_VALUE_NAME);
        win_err(RegOpenKeyExW(
            HKEY_CURRENT_USER,
            PCWSTR(subkey.as_ptr()),
            None,
            KEY_READ,
            &mut key,
        ))?;
        let mut data_type = Default::default();
        let mut size = 0u32;
        let query = RegQueryValueExW(
            key,
            PCWSTR(name.as_ptr()),
            None,
            Some(&mut data_type),
            None,
            Some(&mut size),
        );
        if query == ERROR_FILE_NOT_FOUND {
            let _ = RegCloseKey(key);
            return Ok(None);
        }
        win_err(query)?;
        let mut buffer = vec![0u8; size as usize];
        win_err(RegQueryValueExW(
            key,
            PCWSTR(name.as_ptr()),
            None,
            Some(&mut data_type),
            Some(buffer.as_mut_ptr()),
            Some(&mut size),
        ))?;
        let _ = RegCloseKey(key);
        if size < 2 {
            return Ok(Some(String::new()));
        }
        let u16_len = (size as usize) / 2;
        let words = std::slice::from_raw_parts(buffer.as_ptr() as *const u16, u16_len);
        let trimmed = if words.last() == Some(&0) {
            &words[..words.len() - 1]
        } else {
            words
        };
        Ok(Some(String::from_utf16_lossy(trimmed)))
    }
}

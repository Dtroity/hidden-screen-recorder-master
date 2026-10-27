//! Visible logon autostart via HKCU Run (avoids schtasks OEM/encoding issues).

use std::os::windows::ffi::OsStrExt;
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
    let _ = run_schtasks(&["/Delete", "/TN", "OfficePC-RecorderAgent", "/F"]);
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
    let _ = run_schtasks(&["/Delete", "/TN", "OfficePC-RecorderAgent", "/F"]);
    let _ = run_schtasks(&["/Delete", "/TN", "OfficePC-RecorderAgentWatchdog", "/F"]);
    Ok(())
}

fn admin_stopped_flag_path() -> Option<PathBuf> {
    agent_exe_path()
        .ok()
        .and_then(|p| p.parent().map(|d| d.join("agent.admin-stopped")))
}

fn package_root() -> Option<PathBuf> {
    let exe = agent_exe_path().ok()?;
    let dir = exe.parent()?;
    if dir
        .file_name()
        .and_then(|n| n.to_str())
        .is_some_and(|n| n.eq_ignore_ascii_case("bin"))
    {
        dir.parent().map(|p| p.to_path_buf())
    } else {
        Some(dir.to_path_buf())
    }
}

/// Silent watchdog action for Task Scheduler (no cmd.exe flash).
/// Uses 8.3 short paths — `schtasks /TR` splits on spaces in user profile paths.
fn watchdog_task_command() -> Option<String> {
    let root = package_root()?;
    let vbs = root.join("Watch-Agent.vbs");
    let ps1 = root.join("Watch-Agent.ps1");
    if vbs.is_file() {
        let short = short_path(&vbs).unwrap_or_else(|| vbs.display().to_string());
        Some(format!("wscript.exe //B //Nologo {short}"))
    } else if ps1.is_file() {
        let short = short_path(&ps1).unwrap_or_else(|| ps1.display().to_string());
        Some(format!(
            "powershell.exe -NoProfile -WindowStyle Hidden -ExecutionPolicy Bypass -File {short}"
        ))
    } else {
        None
    }
}

fn short_path(path: &std::path::Path) -> Option<String> {
    use windows::core::PCWSTR;
    use windows::Win32::Storage::FileSystem::GetShortPathNameW;
    let wide: Vec<u16> = path
        .as_os_str()
        .encode_wide()
        .chain(std::iter::once(0))
        .collect();
    let mut buf = vec![0u16; 512];
    let len = unsafe { GetShortPathNameW(PCWSTR(wide.as_ptr()), Some(&mut buf)) };
    if len == 0 || (len as usize) >= buf.len() {
        return None;
    }
    Some(String::from_utf16_lossy(&buf[..len as usize]))
}

fn run_schtasks(args: &[&str]) -> std::io::Result<std::process::Output> {
    use std::os::windows::process::CommandExt;
    const CREATE_NO_WINDOW: u32 = 0x0800_0000;
    std::process::Command::new("schtasks")
        .args(args)
        .creation_flags(CREATE_NO_WINDOW)
        .output()
}

fn enable_watchdog_task() -> Result<(), Message> {
    let Some(tr) = watchdog_task_command() else {
        return Ok(());
    };
    let output = run_schtasks(&[
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

fn stop_flag_path() -> Option<PathBuf> {
    agent_exe_path()
        .ok()
        .and_then(|p| p.parent().map(|d| d.join("agent.stop")))
}

/// Clear Stop-Agent / admin-stop flags so the agent may run again.
pub fn clear_agent_stop_flags() {
    if let Some(path) = admin_stopped_flag_path() {
        let _ = std::fs::remove_file(path);
    }
    if let Some(path) = stop_flag_path() {
        let _ = std::fs::remove_file(path);
    }
}

pub fn admin_stopped() -> bool {
    admin_stopped_flag_path().is_some_and(|p| p.is_file())
}

pub fn launch_agent() -> Result<(), Message> {
    // Always clear admin-stop before spawn — console "Start" is a legitimate start.
    clear_agent_stop_flags();

    if crate::session::agent_process_running() {
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
    // Give the process a moment to claim the mutex / fail on leftover flags.
    std::thread::sleep(std::time::Duration::from_millis(500));
    if !crate::session::agent_process_running() {
        return Err(msg!(
            "Agent exited immediately. Check agent.admin-stopped next to recorder-agent.exe, or diagnostic logs.",
            "Агент сразу завершился. Проверьте agent.admin-stopped рядом с recorder-agent.exe или журнал диагностики."
        ));
    }
    let _ = enable_watchdog_task();
    Ok(())
}

/// Graceful stop (flag) then force-kill — same semantics as Stop-Agent.bat.
pub fn stop_agent() -> Result<(), Message> {
    let exe = agent_exe_path()?;
    let dir = exe.parent().ok_or_else(|| {
        msg!(
            "Executable directory is unavailable",
            "Каталог программы недоступен"
        )
    })?;
    let _ = std::fs::write(dir.join("agent.admin-stopped"), b"1\n");
    let _ = std::fs::write(dir.join("agent.stop"), b"1\n");

    if !crate::session::agent_process_running() {
        let _ = std::fs::remove_file(dir.join("agent.stop"));
        return Ok(());
    }

    // Allow FFmpeg finalize + archive HTTP upload (was ~3s — clips never reached the server).
    for _ in 0..180 {
        if !crate::session::agent_process_running() {
            let _ = std::fs::remove_file(dir.join("agent.stop"));
            return Ok(());
        }
        std::thread::sleep(std::time::Duration::from_secs(1));
    }

    crate::session::force_kill_agent_processes();
    let _ = std::fs::remove_file(dir.join("agent.stop"));
    if crate::session::agent_process_running() {
        return Err(msg!(
            "Could not stop recorder-agent.exe. Try Task Manager.",
            "Не удалось остановить recorder-agent.exe. Закройте в Диспетчере задач."
        ));
    }
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

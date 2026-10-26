//! Workstation identity used to tag recordings and upload folders.

use std::ffi::OsString;
use std::os::windows::ffi::OsStringExt;

use windows::Win32::Foundation::MAX_PATH;
use windows::Win32::System::WindowsProgramming::GetComputerNameW;
use windows::core::PWSTR;

/// Hostname from Windows (`COMPUTERNAME` / `GetComputerNameW`).
pub fn hostname() -> String {
    if let Ok(name) = std::env::var("COMPUTERNAME") {
        let trimmed = name.trim();
        if !trimmed.is_empty() {
            return sanitize_label(trimmed);
        }
    }

    let mut buffer = vec![0u16; MAX_PATH as usize];
    let mut size = buffer.len() as u32;
    let ok = unsafe { GetComputerNameW(Some(PWSTR(buffer.as_mut_ptr())), &mut size) };
    if ok.is_ok() && size > 0 {
        let name = OsString::from_wide(&buffer[..size as usize]);
        let text = name.to_string_lossy();
        let trimmed = text.trim();
        if !trimmed.is_empty() {
            return sanitize_label(trimmed);
        }
    }
    "pc".to_owned()
}

/// Keep labels filesystem- and URL-friendly.
pub fn sanitize_label(value: &str) -> String {
    let cleaned: String = value
        .chars()
        .map(|ch| {
            if ch.is_ascii_alphanumeric() || matches!(ch, '-' | '_' | '.') {
                ch
            } else {
                '_'
            }
        })
        .collect();
    let trimmed = cleaned.trim_matches(['_', '.']);
    if trimmed.is_empty() {
        "pc".to_owned()
    } else {
        trimmed.chars().take(64).collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sanitizes_unsafe_characters() {
        assert_eq!(sanitize_label("DESK-01"), "DESK-01");
        assert_eq!(sanitize_label("PC Name/A"), "PC_Name_A");
        assert_eq!(sanitize_label("   "), "pc");
    }
}

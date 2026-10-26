//! Copy finished recordings to a network share or synced cloud folder.

use std::fs;
use std::path::{Path, PathBuf};

use crate::config::AppConfig;
use crate::debug_log;
use crate::i18n::Message;
use crate::msg;

#[derive(Debug, Clone)]
pub struct UploadResult {
    pub destination: PathBuf,
    pub deleted_local: bool,
}

/// Destination folder: `{upload_dir}/{pc_id}/`.
pub fn destination_dir(config: &AppConfig) -> Option<PathBuf> {
    if !config.upload_enabled {
        return None;
    }
    let root = config.upload_dir.as_ref()?;
    if root.as_os_str().is_empty() {
        return None;
    }
    Some(root.join(config.effective_pc_id()))
}

pub fn upload_recording(
    config: &AppConfig,
    local_path: &Path,
) -> Result<Option<UploadResult>, Message> {
    let Some(dest_dir) = destination_dir(config) else {
        return Ok(None);
    };
    if !local_path.is_file() {
        return Err(msg!(
            "Recording file is missing: {}",
            "Файл записи отсутствует: {}",
            local_path.display()
        ));
    }
    // Already written straight into the cloud/share folder.
    if local_path.starts_with(&dest_dir) {
        debug_log::write(format!(
            "skip upload; already in cloud folder: {}",
            local_path.display()
        ));
        return Ok(None);
    }
    fs::create_dir_all(&dest_dir).map_err(|error| {
        msg!(
            "Could not create upload folder {}: {error}",
            "Не удалось создать папку выгрузки {}: {error}",
            dest_dir.display()
        )
    })?;

    let file_name = local_path
        .file_name()
        .ok_or_else(|| msg!("Invalid recording path", "Некорректный путь к записи"))?;
    let mut destination = dest_dir.join(file_name);
    destination = unique_path(destination);

    debug_log::write(format!(
        "uploading {} -> {} (pc={})",
        local_path.display(),
        destination.display(),
        config.effective_pc_id()
    ));
    fs::copy(local_path, &destination).map_err(|error| {
        msg!(
            "Could not copy to {}: {error}",
            "Не удалось скопировать в {}: {error}",
            destination.display()
        )
    })?;

    let mut deleted_local = false;
    if config.upload_delete_local {
        match fs::remove_file(local_path) {
            Ok(()) => deleted_local = true,
            Err(error) => {
                debug_log::write(format!("upload succeeded but local delete failed: {error}"));
            }
        }
    }

    Ok(Some(UploadResult {
        destination,
        deleted_local,
    }))
}

fn unique_path(path: PathBuf) -> PathBuf {
    if !path.exists() {
        return path;
    }
    let stem = path
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("recording");
    let ext = path
        .extension()
        .and_then(|s| s.to_str())
        .map(|e| format!(".{e}"))
        .unwrap_or_default();
    let parent = path.parent().unwrap_or_else(|| Path::new("."));
    for index in 1..10_000 {
        let candidate = parent.join(format!("{stem}_{index}{ext}"));
        if !candidate.exists() {
            return candidate;
        }
    }
    path
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn copies_into_pc_subdirectory() {
        let dir = tempdir().unwrap();
        let local = dir.path().join("clip.mp4");
        fs::write(&local, b"video").unwrap();
        let upload_root = dir.path().join("share");
        let config = AppConfig {
            upload_enabled: true,
            upload_dir: Some(upload_root.clone()),
            pc_id: Some("OFFICE-01".into()),
            upload_delete_local: false,
            ..Default::default()
        };
        let result = upload_recording(&config, &local).unwrap().unwrap();
        assert!(
            result
                .destination
                .starts_with(upload_root.join("OFFICE-01"))
        );
        assert!(result.destination.is_file());
        assert!(local.is_file());
        assert!(!result.deleted_local);
    }
}

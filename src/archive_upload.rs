//! Persistent queue for HTTP uploads to the archive server.

use std::{
    fs::{self, File, OpenOptions},
    io::{Read, Write},
    path::{Path, PathBuf},
    sync::Mutex,
};

use directories::ProjectDirs;
use serde::{Deserialize, Serialize};

use crate::config::AppConfig;
use crate::debug_log;
use crate::i18n::Message;
use crate::msg;

/// In-process lock (agent + console in one process). Cross-process: file lock below.
static QUEUE_MUTEX: Mutex<()> = Mutex::new(());

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
struct UploadQueue {
    items: Vec<QueueItem>,
    /// Idempotency keys of successfully uploaded files (survives restart; capped).
    #[serde(default)]
    completed_keys: Vec<String>,
    #[serde(default)]
    pub last_error: Option<String>,
    #[serde(default)]
    pub last_ok_at: Option<String>,
}

const COMPLETED_KEYS_CAP: usize = 2_000;

#[derive(Debug, Clone, Serialize, Deserialize)]
struct QueueItem {
    path: PathBuf,
    attempts: u32,
}

fn queue_path() -> Option<PathBuf> {
    ProjectDirs::from("dev", "", "hidden-screen-recorder")
        .map(|d| d.data_local_dir().join("archive_upload_queue.json"))
}

fn lock_path() -> Option<PathBuf> {
    queue_path().map(|p| p.with_extension("lock"))
}

struct QueueFileLock {
    _file: File,
}

fn acquire_queue_lock() -> Option<QueueFileLock> {
    let path = lock_path()?;
    if let Some(parent) = path.parent() {
        let _ = fs::create_dir_all(parent);
    }
    // Exclusive create: if another process holds the lock file open for write on Windows,
    // sharing may still allow open — we additionally use an in-process mutex.
    let file = OpenOptions::new()
        .create(true)
        .write(true)
        .truncate(true)
        .open(&path)
        .ok()?;
    Some(QueueFileLock { _file: file })
}

fn load_queue() -> UploadQueue {
    let Some(path) = queue_path() else {
        return UploadQueue::default();
    };
    fs::read_to_string(path)
        .ok()
        .and_then(|t| serde_json::from_str(&t).ok())
        .unwrap_or_default()
}

fn save_queue(queue: &UploadQueue) -> Result<(), String> {
    let path = queue_path().ok_or_else(|| "queue path unavailable".to_owned())?;
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    }
    let mut tmp = tempfile::NamedTempFile::new_in(path.parent().unwrap_or_else(|| Path::new(".")))
        .map_err(|e| e.to_string())?;
    tmp.write_all(serde_json::to_string_pretty(queue).map_err(|e| e.to_string())?.as_bytes())
        .map_err(|e| e.to_string())?;
    tmp.persist(&path).map_err(|e| e.to_string())?;
    Ok(())
}

/// Enqueue a finished local recording and try to flush the queue.
pub fn enqueue_and_flush(config: &AppConfig, local_path: &Path) -> Result<(), Message> {
    if !config.archive_upload_enabled {
        return Ok(());
    }
    ensure_archive_settings(config)?;
    if !local_path.is_file() {
        return Err(msg!(
            "Recording file is missing: {}",
            "Файл записи отсутствует: {}",
            local_path.display()
        ));
    }

    let _mem = QUEUE_MUTEX.lock().unwrap_or_else(|e| e.into_inner());
    let _disk = acquire_queue_lock();
    enqueue_path_locked(config, local_path);
    flush_queue_locked(config)
}

/// Retry pending uploads (call periodically from the agent).
pub fn flush_queue(config: &AppConfig) -> Result<(), Message> {
    if !config.archive_upload_enabled {
        return Ok(());
    }
    let _mem = QUEUE_MUTEX.lock().unwrap_or_else(|e| e.into_inner());
    let _disk = acquire_queue_lock();
    flush_queue_locked(config)
}

/// On agent start / wake: re-queue local finished MP4s that never reached the server,
/// then flush. Safe with idempotency keys (no archive duplicates).
pub fn recover_pending_and_flush(config: &AppConfig) -> Result<(), Message> {
    if !config.archive_upload_enabled {
        return Ok(());
    }
    ensure_archive_settings(config)?;
    let _mem = QUEUE_MUTEX.lock().unwrap_or_else(|e| e.into_inner());
    let _disk = acquire_queue_lock();
    let added = scan_and_enqueue_local_locked(config);
    if added > 0 {
        debug_log::write(format!(
            "archive queue: recovered {added} local recording(s) for upload"
        ));
    }
    flush_queue_locked(config)
}

fn ensure_archive_settings(config: &AppConfig) -> Result<(), Message> {
    if config
        .archive_url
        .as_ref()
        .map(|s| s.trim())
        .unwrap_or("")
        .is_empty()
    {
        return Err(msg!(
            "Archive server URL is not set",
            "Не задан адрес сервера архива"
        ));
    }
    if config
        .archive_token
        .as_ref()
        .map(|s| s.trim())
        .unwrap_or("")
        .is_empty()
    {
        return Err(msg!(
            "Archive upload token is not set",
            "Не задан токен отправки в архив"
        ));
    }
    Ok(())
}

fn enqueue_path_locked(config: &AppConfig, local_path: &Path) {
    let pc = config.effective_pc_id();
    let key = idempotency_key(&pc, local_path);
    let mut queue = load_queue();
    if queue.completed_keys.iter().any(|k| k == &key) {
        return;
    }
    if queue.items.iter().any(|i| i.path == local_path) {
        return;
    }
    queue.items.push(QueueItem {
        path: local_path.to_path_buf(),
        attempts: 0,
    });
    let _ = save_queue(&queue);
}

fn scan_and_enqueue_local_locked(config: &AppConfig) -> usize {
    let dir = config.effective_output_dir();
    let Ok(entries) = fs::read_dir(&dir) else {
        return 0;
    };
    let pc = config.effective_pc_id();
    let mut queue = load_queue();
    let mut added = 0usize;
    for entry in entries.flatten() {
        let path = entry.path();
        let Some(name) = path.file_name().and_then(|n| n.to_str()) else {
            continue;
        };
        let lower = name.to_ascii_lowercase();
        if !lower.ends_with(".mp4") || lower.contains(".partial.") {
            continue;
        }
        if !crate::recorder::mp4_is_playable(&path) {
            continue;
        }
        let key = idempotency_key(&pc, &path);
        if queue.completed_keys.iter().any(|k| k == &key) {
            continue;
        }
        if queue.items.iter().any(|i| i.path == path) {
            continue;
        }
        queue.items.push(QueueItem {
            path,
            attempts: 0,
        });
        added += 1;
    }
    if added > 0 {
        let _ = save_queue(&queue);
    }
    added
}

fn remember_completed(queue: &mut UploadQueue, key: String) {
    if queue.completed_keys.iter().any(|k| k == &key) {
        return;
    }
    queue.completed_keys.push(key);
    if queue.completed_keys.len() > COMPLETED_KEYS_CAP {
        let drop_n = queue.completed_keys.len() - COMPLETED_KEYS_CAP;
        queue.completed_keys.drain(0..drop_n);
    }
}

fn flush_queue_locked(config: &AppConfig) -> Result<(), Message> {
    let url = config
        .archive_url
        .as_ref()
        .map(|s| s.trim().trim_end_matches('/'))
        .filter(|s| !s.is_empty());
    let token = config
        .archive_token
        .as_ref()
        .map(|s| s.trim())
        .filter(|s| !s.is_empty());
    let (Some(base), Some(token)) = (url, token) else {
        return Ok(());
    };

    let mut queue = load_queue();
    if queue.items.is_empty() {
        return Ok(());
    }

    let pending: Vec<QueueItem> = queue.items.drain(..).collect();
    let mut remaining = Vec::new();
    let mut newly_done: Vec<String> = Vec::new();
    let mut last_error = None;
    let mut last_ok = None;

    for mut item in pending {
        if !item.path.is_file() {
            debug_log::write(format!(
                "archive queue: skip missing {}",
                item.path.display()
            ));
            continue;
        }
        let key = idempotency_key(&config.effective_pc_id(), &item.path);
        match post_file(config, base, token, &item.path) {
            Ok(()) => {
                debug_log::write(format!("archive upload ok: {}", item.path.display()));
                last_ok = Some(chrono::Utc::now().to_rfc3339());
                newly_done.push(key);
                if config.archive_delete_local {
                    if let Err(error) = fs::remove_file(&item.path) {
                        debug_log::write(format!(
                            "archive upload ok but local delete failed: {error}"
                        ));
                    }
                }
            }
            Err(error) => {
                item.attempts = item.attempts.saturating_add(1);
                // Never log the bearer token — error text is HTTP status / body only.
                debug_log::write(format!(
                    "archive upload failed (attempt {}): {} — {error}",
                    item.attempts,
                    item.path.display()
                ));
                last_error = Some(error);
                remaining.push(item);
            }
        }
    }

    queue.items = remaining;
    for key in newly_done {
        remember_completed(&mut queue, key);
    }
    if last_error.is_some() {
        queue.last_error = last_error.clone();
    }
    if last_ok.is_some() {
        queue.last_ok_at = last_ok;
        if queue.items.is_empty() {
            queue.last_error = None;
        }
    }
    let _ = save_queue(&queue);

    if let Some(error) = last_error {
        Err(msg!(
            "Archive upload error: {error}",
            "Ошибка отправки в архив: {error}"
        ))
    } else {
        Ok(())
    }
}

pub fn status_summary() -> String {
    let q = load_queue();
    let pending = q.items.len();
    match (pending, q.last_error, q.last_ok_at) {
        (0, None, Some(ok)) => format!("archive: ok {ok}"),
        (0, None, None) => "archive: idle".into(),
        (n, Some(err), _) => format!("archive: {n} pending; last error: {err}"),
        (n, None, _) => format!("archive: {n} pending"),
    }
}

/// Lightweight connectivity check: server reachable + agent token accepted.
/// Does not upload a real recording. Never logs the token.
pub fn test_connection(config: &AppConfig) -> Result<String, String> {
    let base = config
        .archive_url
        .as_ref()
        .map(|s| s.trim().trim_end_matches('/'))
        .filter(|s| !s.is_empty())
        .ok_or_else(|| "Server URL is empty".to_owned())?;
    let token = config
        .archive_token
        .as_ref()
        .map(|s| s.trim())
        .filter(|s| !s.is_empty())
        .ok_or_else(|| "Upload token is empty".to_owned())?;

    let home = ureq::get(base).timeout(std::time::Duration::from_secs(8)).call();
    match home {
        Ok(resp) if (200..400).contains(&resp.status()) => {}
        Ok(resp) => {
            return Err(format!(
                "Server responded HTTP {} at {base}",
                resp.status()
            ));
        }
        Err(e) => {
            // Do not include URL query/secrets; base is user-configured host.
            return Err(format!("Cannot reach server: {e}"));
        }
    }

    // Auth probe: ureq treats HTTP 4xx as Err(Status). After a valid token the server
    // returns 400 "file field required" — that means auth succeeded.
    let boundary = format!("----hsrprobe{}", uuid::Uuid::new_v4().simple());
    let body = format!(
        "--{boundary}\r\nContent-Disposition: form-data; name=\"probe\"\r\n\r\nx\r\n--{boundary}--\r\n"
    );
    let endpoint = format!("{base}/api/upload");
    let resp = ureq::post(&endpoint)
        .timeout(std::time::Duration::from_secs(8))
        .set("Authorization", &format!("Bearer {token}"))
        .set("X-PC-ID", "connection-test")
        .set(
            "Content-Type",
            &format!("multipart/form-data; boundary={boundary}"),
        )
        .send_string(&body);

    let status = match &resp {
        Ok(r) => r.status(),
        Err(ureq::Error::Status(code, _)) => *code,
        Err(e) => {
            return Err(format!("Upload probe failed: {e}"));
        }
    };

    match status {
        200..=299 | 400 => Ok(
            "Connected: server reachable, upload token accepted.".into(),
        ),
        401 | 403 => Err(
            "Server reachable, but upload token was rejected (use agent token from show-agent-token / AGENT-TOKEN.txt, not the admin password)."
                .into(),
        ),
        other => Err(format!("Unexpected HTTP {other} from /api/upload")),
    }
}

fn idempotency_key(pc: &str, path: &Path) -> String {
    let meta = fs::metadata(path).ok();
    let size = meta.as_ref().map(|m| m.len()).unwrap_or(0);
    let mtime = meta
        .and_then(|m| m.modified().ok())
        .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
        .map(|d| d.as_secs())
        .unwrap_or(0);
    // Stable across retries of the same finished file; changes if the file is rewritten.
    format!("{pc}|{}|{size}|{mtime}", path.to_string_lossy())
}

fn post_file(config: &AppConfig, base: &str, token: &str, path: &Path) -> Result<(), String> {
    let endpoint = format!("{base}/api/upload");
    let file_name = path
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("recording.mp4");
    let pc = config.effective_pc_id();
    let recorded = file_mtime_rfc3339(path).unwrap_or_else(|| chrono::Utc::now().to_rfc3339());
    let idem = idempotency_key(&pc, path);

    // Build multipart on disk so large recordings are not held entirely in RAM.
    let boundary = format!("----hsr{}", uuid::Uuid::new_v4().simple());
    let mut tmp = tempfile::NamedTempFile::new().map_err(|e| e.to_string())?;
    write!(
        tmp,
        "--{boundary}\r\nContent-Disposition: form-data; name=\"file\"; filename=\"{file_name}\"\r\nContent-Type: video/mp4\r\n\r\n"
    )
    .map_err(|e| e.to_string())?;
    {
        let mut input = File::open(path).map_err(|e| e.to_string())?;
        let mut buf = [0u8; 64 * 1024];
        loop {
            let n = input.read(&mut buf).map_err(|e| e.to_string())?;
            if n == 0 {
                break;
            }
            tmp.write_all(&buf[..n]).map_err(|e| e.to_string())?;
        }
    }
    write!(tmp, "\r\n--{boundary}--\r\n").map_err(|e| e.to_string())?;
    tmp.flush().map_err(|e| e.to_string())?;
    let len = tmp.as_file().metadata().map_err(|e| e.to_string())?.len();
    let body = File::open(tmp.path()).map_err(|e| e.to_string())?;

    let resp = ureq::post(&endpoint)
        .set("Authorization", &format!("Bearer {token}"))
        .set("X-PC-ID", &pc)
        .set("X-Display-Name", &pc)
        .set("X-Recorded-At", &recorded)
        .set("X-Idempotency-Key", &idem)
        .set(
            "Content-Type",
            &format!("multipart/form-data; boundary={boundary}"),
        )
        .set("Content-Length", &len.to_string())
        .send(body)
        .map_err(|e| e.to_string())?;

    let status = resp.status();
    if !(200..300).contains(&status) {
        let text = resp.into_string().unwrap_or_default();
        let safe = text.chars().take(200).collect::<String>();
        return Err(format!("HTTP {status}: {safe}"));
    }
    Ok(())
}

fn file_mtime_rfc3339(path: &Path) -> Option<String> {
    let meta = fs::metadata(path).ok()?;
    let modified = meta.modified().ok()?;
    let dt: chrono::DateTime<chrono::Utc> = modified.into();
    Some(dt.to_rfc3339())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn idempotency_key_stable_for_same_meta() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("a.mp4");
        fs::write(&path, b"abc").unwrap();
        let a = idempotency_key("PC1", &path);
        let b = idempotency_key("PC1", &path);
        assert_eq!(a, b);
        assert!(a.contains("PC1"));
        assert!(a.contains("3"));
    }
}

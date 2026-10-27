//! Persistent queue for HTTP uploads to the archive server.

use std::{
    fs::{self, File, OpenOptions},
    io::{Read, Seek, SeekFrom, Write},
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

/// Prefer `OfficePC\archive_upload_queue.json` (portable); fall back to LocalAppData.
fn queue_path() -> Option<PathBuf> {
    if let Some(root) = AppConfig::portable_root() {
        return Some(root.join("archive_upload_queue.json"));
    }
    ProjectDirs::from("dev", "", "hidden-screen-recorder")
        .map(|d| d.data_local_dir().join("archive_upload_queue.json"))
}

fn lock_path() -> Option<PathBuf> {
    queue_path().map(|p| p.with_extension("lock"))
}

fn log_path() -> Option<PathBuf> {
    queue_path().map(|p| p.with_file_name("archive_upload.log"))
}

struct QueueFileLock {
    _file: File,
}

fn acquire_queue_lock() -> Option<QueueFileLock> {
    let path = lock_path()?;
    if let Some(parent) = path.parent() {
        let _ = fs::create_dir_all(parent);
    }
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
    if !config.archive_upload_active() {
        always_log(&format!(
            "skip enqueue (archive URL/token missing or upload off): {}",
            local_path.display()
        ));
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

    always_log(&format!("enqueue {}", local_path.display()));
    let _mem = QUEUE_MUTEX.lock().unwrap_or_else(|e| e.into_inner());
    let _disk = acquire_queue_lock();
    enqueue_path_locked(config, local_path);
    flush_queue_locked(config)
}

/// Enqueue only (flush happens on the agent timer / background worker).
pub fn enqueue(config: &AppConfig, local_path: &Path) -> Result<(), Message> {
    if !config.archive_upload_active() {
        always_log(&format!(
            "skip enqueue (archive URL/token missing or upload off): {}",
            local_path.display()
        ));
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
    always_log(&format!("enqueue {}", local_path.display()));
    let _mem = QUEUE_MUTEX.lock().unwrap_or_else(|e| e.into_inner());
    let _disk = acquire_queue_lock();
    enqueue_path_locked(config, local_path);
    Ok(())
}

/// Retry pending uploads (call periodically from the agent).
pub fn flush_queue(config: &AppConfig) -> Result<(), Message> {
    if !config.archive_upload_active() {
        return Ok(());
    }
    let _mem = QUEUE_MUTEX.lock().unwrap_or_else(|e| e.into_inner());
    let _disk = acquire_queue_lock();
    flush_queue_locked(config)
}

/// On agent start / wake: recover orphan partials, re-queue finished MP4s, flush.
pub fn recover_pending_and_flush(config: &AppConfig) -> Result<(), Message> {
    let recovered_partials =
        crate::recorder::recover_orphan_partials(&config.effective_output_dir());

    if !config.archive_upload_active() {
        always_log(
            "archive upload inactive (set archive_url + archive_token in OfficePC\\settings.json)",
        );
        return Ok(());
    }
    ensure_archive_settings(config)?;
    let _mem = QUEUE_MUTEX.lock().unwrap_or_else(|e| e.into_inner());
    let _disk = acquire_queue_lock();
    for path in &recovered_partials {
        enqueue_path_locked(config, path);
    }
    let added = scan_and_enqueue_local_locked(config);
    if added > 0 || !recovered_partials.is_empty() {
        always_log(&format!(
            "recovered {} file(s), {} from orphan partials",
            added + recovered_partials.len(),
            recovered_partials.len()
        ));
    }
    flush_queue_locked(config)
}

/// Run from agent startup: probe server and write a clear line into archive_upload.log.
pub fn startup_probe(config: &AppConfig) {
    if !config.archive_upload_active() {
        always_log("startup: archive upload OFF — fill archive_url + archive_token, then restart agent");
        return;
    }
    match test_connection(config) {
        Ok(msg) => always_log(&format!("startup probe OK: {msg}")),
        Err(err) => always_log(&format!("startup probe FAIL: {err}")),
    }
    if let Some(path) = log_path() {
        always_log(&format!("log file: {}", path.display()));
    }
}

fn always_log(line: &str) {
    debug_log::write(format!("archive: {line}"));
    let Some(path) = log_path() else {
        return;
    };
    if let Some(parent) = path.parent() {
        let _ = fs::create_dir_all(parent);
    }
    if let Ok(mut f) = OpenOptions::new().create(true).append(true).open(path) {
        let _ = writeln!(
            f,
            "{} {line}",
            chrono::Local::now().format("%Y-%m-%d %H:%M:%S")
        );
    }
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
    let token = config
        .archive_token
        .as_ref()
        .map(|s| s.trim())
        .unwrap_or("");
    if token.is_empty() {
        return Err(msg!(
            "Archive upload token is not set",
            "Не задан токен отправки в архив"
        ));
    }
    if token_looks_like_placeholder(token) {
        return Err(msg!(
            "Archive token is still a placeholder. On the VPS run: archive-server --config /etc/hsr/server.toml show-agent-token — then paste that token (not the admin password).",
            "В settings.json всё ещё подставка вместо токена. На VPS: archive-server --config /etc/hsr/server.toml show-agent-token — вставьте этот токен (не пароль администратора)."
        ));
    }
    Ok(())
}

fn token_looks_like_placeholder(token: &str) -> bool {
    let t = token.trim();
    let lower = t.to_ascii_lowercase();
    t.is_empty()
        || lower.contains("paste_")
        || lower.contains("token_here")
        || lower.contains("changeme")
        || lower == "password"
        || lower == "admin"
        || lower == "secret"
}

fn enqueue_path_locked(config: &AppConfig, local_path: &Path) {
    let pc = config.effective_pc_id();
    let key = idempotency_key(&pc, local_path);
    let mut queue = load_queue();
    if queue.completed_keys.iter().any(|k| k == &key) {
        always_log(&format!(
            "skip already-uploaded {}",
            local_path.display()
        ));
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
        always_log("flush skipped: archive URL or token empty");
        return Ok(());
    };

    let mut queue = load_queue();
    if queue.items.is_empty() {
        return Ok(());
    }
    always_log(&format!("flush start: {} item(s) -> {base}", queue.items.len()));

    let pending: Vec<QueueItem> = queue.items.drain(..).collect();
    let mut remaining = Vec::new();
    let mut newly_done: Vec<String> = Vec::new();
    let mut last_error = None;
    let mut last_ok = None;
    let mut stop_auth = false;

    for mut item in pending {
        if stop_auth {
            remaining.push(item);
            continue;
        }
        if !item.path.is_file() {
            always_log(&format!("skip missing {}", item.path.display()));
            continue;
        }
        let key = idempotency_key(&config.effective_pc_id(), &item.path);
        match post_file(config, base, token, &item.path) {
            Ok(()) => {
                always_log(&format!("upload ok: {}", item.path.display()));
                last_ok = Some(chrono::Utc::now().to_rfc3339());
                newly_done.push(key);
                if config.archive_delete_local {
                    if let Err(error) = fs::remove_file(&item.path) {
                        always_log(&format!(
                            "upload ok but local delete failed: {error}"
                        ));
                    }
                }
            }
            Err(error) => {
                item.attempts = item.attempts.saturating_add(1);
                always_log(&format!(
                    "queue fail attempt {} {}: {error}",
                    item.attempts,
                    item.path.display()
                ));
                let lower = error.to_ascii_lowercase();
                let auth_fail = lower.contains("401")
                    || lower.contains("bad token")
                    || lower.contains("unauthorized");
                last_error = Some(error);
                remaining.push(item);
                // Wrong token will never succeed — don't hammer the server this cycle.
                if auth_fail {
                    always_log(
                        "stopping flush: HTTP 401 bad token — fix archive_token in settings.json (VPS: show-agent-token)",
                    );
                    stop_auth = true;
                }
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
    if token_looks_like_placeholder(token) {
        return Err(
            "Token looks like a placeholder / admin password. On VPS run show-agent-token and paste the agent token into settings.json."
                .into(),
        );
    }

    let home = ureq::get(base)
        .timeout(std::time::Duration::from_secs(8))
        .call();
    match home {
        Ok(resp) if (200..400).contains(&resp.status()) => {}
        Ok(resp) => {
            return Err(format!(
                "Server responded HTTP {} at {base}",
                resp.status()
            ));
        }
        Err(e) => {
            return Err(format!("Cannot reach server: {e}"));
        }
    }

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
    format!("{pc}|{}|{size}|{mtime}", path.to_string_lossy())
}

fn post_file(config: &AppConfig, base: &str, token: &str, path: &Path) -> Result<(), String> {
    let mut last = String::new();
    for attempt in 1..=4 {
        match post_file_once(config, base, token, path) {
            Ok(()) => return Ok(()),
            Err(error) => {
                last = error;
                always_log(&format!(
                    "upload attempt {attempt}/4 failed for {}: {last}",
                    path.display()
                ));
                std::thread::sleep(std::time::Duration::from_millis(500 * attempt as u64));
            }
        }
    }
    Err(last)
}

fn post_file_once(config: &AppConfig, base: &str, token: &str, path: &Path) -> Result<(), String> {
    let endpoint = format!("{base}/api/upload");
    let file_name = path
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("recording.mp4");
    let pc = config.effective_pc_id();
    let recorded = file_mtime_rfc3339(path).unwrap_or_else(|| chrono::Utc::now().to_rfc3339());
    let idem = idempotency_key(&pc, path);

    let boundary = format!("----hsr{}", uuid::Uuid::new_v4().simple());
    let mut tmp = tempfile::NamedTempFile::new().map_err(|e| e.to_string())?;
    write!(
        tmp,
        "--{boundary}\r\nContent-Disposition: form-data; name=\"file\"; filename=\"{file_name}\"\r\nContent-Type: video/mp4\r\n\r\n"
    )
    .map_err(|e| e.to_string())?;
    {
        let mut input = File::open(path).map_err(|e| e.to_string())?;
        let mut buf = [0u8; 256 * 1024];
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
    tmp.as_file_mut()
        .seek(SeekFrom::Start(0))
        .map_err(|e| e.to_string())?;
    let len = tmp.as_file().metadata().map_err(|e| e.to_string())?.len();

    // Prefer send from the same handle after seek(0) — avoids Windows share issues
    // with a second File::open on a still-held NamedTempFile.
    let resp = ureq::post(&endpoint)
        .timeout(std::time::Duration::from_secs(900))
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
        .send(tmp.as_file());

    match resp {
        Ok(r) => {
            let status = r.status();
            if (200..300).contains(&status) {
                Ok(())
            } else {
                let text = r.into_string().unwrap_or_default();
                let safe = text.chars().take(200).collect::<String>();
                Err(format!("HTTP {status}: {safe}"))
            }
        }
        Err(ureq::Error::Status(code, r)) => {
            let text = r.into_string().unwrap_or_default();
            let safe = text.chars().take(200).collect::<String>();
            Err(format!("HTTP {code}: {safe}"))
        }
        Err(e) => Err(e.to_string()),
    }
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

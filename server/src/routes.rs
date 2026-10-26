use std::{
    io::SeekFrom,
    path::{Component, Path, PathBuf},
    sync::Arc,
};

use axum::{
    body::Body,
    extract::{FromRequest, Multipart, Path as AxumPath, Query, Request, State},
    http::{
        HeaderMap, StatusCode,
        header::{AUTHORIZATION, CONTENT_LENGTH, CONTENT_RANGE, CONTENT_TYPE, RANGE},
    },
    response::{Html, Response},
    Json,
};
use chrono::{DateTime, Utc};
use serde::Deserialize;
use tokio::io::{AsyncReadExt, AsyncSeekExt};
use tokio_util::io::ReaderStream;

use crate::{
    auth::{admin_login, agent_authorized, Sessions},
    config::ServerConfig,
    db::Db,
};

#[derive(Clone)]
pub struct AppState {
    pub cfg: Arc<ServerConfig>,
    pub db: Db,
    pub sessions: Arc<Sessions>,
}

pub async fn index() -> Html<&'static str> {
    Html(include_str!("../static/index.html"))
}

#[derive(Deserialize)]
pub struct LoginBody {
    password: String,
}

pub async fn login(
    State(state): State<AppState>,
    Json(body): Json<LoginBody>,
) -> Result<Json<serde_json::Value>, StatusCode> {
    match admin_login(&state.cfg, &state.sessions, &body.password) {
        Some(token) => Ok(Json(serde_json::json!({ "token": token }))),
        None => Err(StatusCode::UNAUTHORIZED),
    }
}

fn bearer(headers: &HeaderMap) -> Option<String> {
    headers
        .get(AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.strip_prefix("Bearer ").map(|s| s.to_string()))
}

fn require_admin(state: &AppState, headers: &HeaderMap) -> Result<(), StatusCode> {
    let token = bearer(headers).ok_or(StatusCode::UNAUTHORIZED)?;
    if state.sessions.valid(&token) {
        Ok(())
    } else {
        Err(StatusCode::UNAUTHORIZED)
    }
}

pub async fn list_computers(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> Result<Json<serde_json::Value>, StatusCode> {
    require_admin(&state, &headers)?;
    let rows = state.db.list_computers().map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    Ok(Json(serde_json::json!(rows)))
}

#[derive(Deserialize)]
pub struct RecQuery {
    pc_id: Option<String>,
    from: Option<String>,
    to: Option<String>,
    q: Option<String>,
}

pub async fn list_recordings(
    State(state): State<AppState>,
    headers: HeaderMap,
    Query(q): Query<RecQuery>,
) -> Result<Json<serde_json::Value>, StatusCode> {
    require_admin(&state, &headers)?;
    let rows = state
        .db
        .list_recordings(
            q.pc_id.as_deref(),
            q.from.as_deref(),
            q.to.as_deref(),
            q.q.as_deref(),
        )
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    Ok(Json(serde_json::json!(rows)))
}

#[derive(Deserialize)]
pub struct RenameBody {
    display_name: String,
}

pub async fn rename_computer(
    State(state): State<AppState>,
    headers: HeaderMap,
    AxumPath(pc_id): AxumPath<String>,
    Json(body): Json<RenameBody>,
) -> Result<StatusCode, StatusCode> {
    require_admin(&state, &headers)?;
    let name = sanitize_label(&body.display_name);
    if name.is_empty() {
        return Err(StatusCode::BAD_REQUEST);
    }
    state
        .db
        .rename_computer(&pc_id, &name)
        .map_err(|_| StatusCode::NOT_FOUND)?;
    Ok(StatusCode::NO_CONTENT)
}

#[derive(Deserialize)]
pub struct IdsBody {
    ids: Vec<i64>,
}

/// DELETE /api/recordings/{id} — one recording (file + DB).
pub async fn delete_recording(
    State(state): State<AppState>,
    headers: HeaderMap,
    AxumPath(id): AxumPath<i64>,
) -> Result<Json<serde_json::Value>, StatusCode> {
    require_admin(&state, &headers)?;
    let (deleted, deleted_ids, missing) = delete_recording_files(&state, &[id]).await?;
    Ok(Json(serde_json::json!({
        "deleted": deleted,
        "deleted_ids": deleted_ids,
        "missing_ids": missing
    })))
}

/// POST /api/recordings/delete — batch delete by ids.
pub async fn delete_recordings_batch(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(body): Json<IdsBody>,
) -> Result<Json<serde_json::Value>, StatusCode> {
    require_admin(&state, &headers)?;
    if body.ids.is_empty() {
        return Err(StatusCode::BAD_REQUEST);
    }
    if body.ids.len() > 500 {
        return Err(StatusCode::BAD_REQUEST);
    }
    let (deleted, deleted_ids, missing) = delete_recording_files(&state, &body.ids).await?;
    Ok(Json(serde_json::json!({
        "deleted": deleted,
        "deleted_ids": deleted_ids,
        "missing_ids": missing
    })))
}

async fn delete_recording_files(
    state: &AppState,
    ids: &[i64],
) -> Result<(usize, Vec<i64>, Vec<i64>), StatusCode> {
    // Preserve caller order; skip duplicates so each id is attempted once.
    let mut seen = std::collections::HashSet::new();
    let mut unique_ids = Vec::new();
    for id in ids {
        if seen.insert(*id) {
            unique_ids.push(*id);
        }
    }

    let rows = state
        .db
        .recordings_by_ids(&unique_ids)
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    let found: std::collections::HashSet<i64> = rows.iter().map(|r| r.id).collect();
    let missing: Vec<i64> = unique_ids
        .iter()
        .copied()
        .filter(|id| !found.contains(id))
        .collect();

    let mut deleted_ids = Vec::new();
    for row in rows {
        if let Some(abs) = safe_join(&state.cfg.archive_dir, &row.relative_path) {
            let _ = tokio::fs::remove_file(&abs).await;
            // Best-effort: remove empty parent day/month dirs only (never recursive).
            if let Some(parent) = abs.parent() {
                let _ = tokio::fs::remove_dir(parent).await;
            }
        }
        if state
            .db
            .delete_recording_row(row.id)
            .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?
            .is_some()
        {
            deleted_ids.push(row.id);
        }
    }
    Ok((deleted_ids.len(), deleted_ids, missing))
}

/// DELETE /api/computers/{pc_id} — wipe all recordings for a workplace + DB rows.
pub async fn delete_computer_archive(
    State(state): State<AppState>,
    headers: HeaderMap,
    AxumPath(pc_id): AxumPath<String>,
) -> Result<Json<serde_json::Value>, StatusCode> {
    require_admin(&state, &headers)?;
    if pc_id.trim().is_empty() || pc_id.contains("..") || pc_id.contains('/') || pc_id.contains('\\') {
        return Err(StatusCode::BAD_REQUEST);
    }
    let rows = state
        .db
        .list_recording_paths_for_pc(&pc_id)
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    let count = rows.len();
    for row in &rows {
        if let Some(abs) = safe_join(&state.cfg.archive_dir, &row.relative_path) {
            let _ = tokio::fs::remove_file(&abs).await;
        }
    }
    // Remove PC folder under archive if present.
    let pc_dir = state.cfg.archive_dir.join(&pc_id);
    if pc_dir.is_dir() && pc_dir.starts_with(&state.cfg.archive_dir) {
        let _ = tokio::fs::remove_dir_all(&pc_dir).await;
    }
    state
        .db
        .delete_computer_and_recordings(&pc_id)
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    Ok(Json(serde_json::json!({ "deleted_recordings": count, "pc_id": pc_id })))
}

/// GET /api/storage — disk usage for the archive volume.
pub async fn storage_info(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> Result<Json<serde_json::Value>, StatusCode> {
    require_admin(&state, &headers)?;
    let path = state.cfg.archive_dir.clone();
    let (total, available) = tokio::task::spawn_blocking(move || -> Result<(u64, u64), String> {
        let total = fs4::total_space(&path).map_err(|e| e.to_string())?;
        let available = fs4::available_space(&path).map_err(|e| e.to_string())?;
        Ok((total, available))
    })
    .await
    .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?
    .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    let used = total.saturating_sub(available);
    Ok(Json(serde_json::json!({
        "total_bytes": total,
        "used_bytes": used,
        "free_bytes": available,
    })))
}

#[derive(Deserialize)]
pub struct MediaQuery {
    access_token: Option<String>,
}

pub async fn media(
    State(state): State<AppState>,
    headers: HeaderMap,
    AxumPath(id): AxumPath<i64>,
    Query(q): Query<MediaQuery>,
) -> Result<Response, StatusCode> {
    let token = bearer(&headers).or(q.access_token);
    let token = token.ok_or(StatusCode::UNAUTHORIZED)?;
    if !state.sessions.valid(&token) {
        return Err(StatusCode::UNAUTHORIZED);
    }
    let row = state
        .db
        .recording_by_id(id)
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?
        .ok_or(StatusCode::NOT_FOUND)?;
    let abs = safe_join(&state.cfg.archive_dir, &row.relative_path)
        .ok_or(StatusCode::BAD_REQUEST)?;
    if !abs.is_file() {
        return Err(StatusCode::NOT_FOUND);
    }
    serve_mp4(abs, headers.get(RANGE).and_then(|v| v.to_str().ok())).await
}

async fn serve_mp4(path: PathBuf, range_header: Option<&str>) -> Result<Response, StatusCode> {
    let mut file = tokio::fs::File::open(&path)
        .await
        .map_err(|_| StatusCode::NOT_FOUND)?;
    let meta = file.metadata().await.map_err(|_| StatusCode::NOT_FOUND)?;
    let len = meta.len();

    if let Some(range) = range_header.and_then(parse_bytes_range) {
        let (start, end) = range;
        if start >= len || end < start {
            return Response::builder()
                .status(StatusCode::RANGE_NOT_SATISFIABLE)
                .header(CONTENT_RANGE, format!("bytes */{len}"))
                .body(Body::empty())
                .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR);
        }
        let end = end.min(len - 1);
        let take = end - start + 1;
        file.seek(SeekFrom::Start(start))
            .await
            .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
        let limited = file.take(take);
        let body = Body::from_stream(ReaderStream::new(limited));
        return Response::builder()
            .status(StatusCode::PARTIAL_CONTENT)
            // codecs=hvc1 helps browsers that gate HEVC on Content-Type (file stays HEVC).
            .header(CONTENT_TYPE, "video/mp4; codecs=\"hvc1\"")
            .header(CONTENT_LENGTH, take)
            .header(CONTENT_RANGE, format!("bytes {start}-{end}/{len}"))
            .header("Accept-Ranges", "bytes")
            .body(body)
            .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR);
    }

    let body = Body::from_stream(ReaderStream::new(file));
    Response::builder()
        .status(StatusCode::OK)
        .header(CONTENT_TYPE, "video/mp4; codecs=\"hvc1\"")
        .header(CONTENT_LENGTH, len)
        .header("Accept-Ranges", "bytes")
        .body(body)
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)
}

fn parse_bytes_range(h: &str) -> Option<(u64, u64)> {
    let h = h.strip_prefix("bytes=")?;
    let mut parts = h.splitn(2, '-');
    let start: u64 = parts.next()?.parse().ok()?;
    let end = parts.next().unwrap_or("");
    let end = if end.is_empty() {
        u64::MAX
    } else {
        end.parse().ok()?
    };
    Some((start, end))
}

pub async fn upload(
    State(state): State<AppState>,
    req: Request,
) -> Result<Json<serde_json::Value>, (StatusCode, String)> {
    // Authorize before parsing multipart so missing/invalid tokens never look like 400 body errors
    // and large unauthenticated uploads are rejected early.
    let headers = req.headers().clone();
    let token = bearer(&headers).ok_or((StatusCode::UNAUTHORIZED, "missing token".into()))?;
    if !agent_authorized(&state.cfg, &token) {
        return Err((StatusCode::UNAUTHORIZED, "bad token".into()));
    }
    let pc_id = headers
        .get("x-pc-id")
        .and_then(|v| v.to_str().ok())
        .map(sanitize_label)
        .filter(|s| !s.is_empty())
        .ok_or((StatusCode::BAD_REQUEST, "X-PC-ID required".into()))?;
    let display = headers
        .get("x-display-name")
        .and_then(|v| v.to_str().ok())
        .map(sanitize_label)
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| pc_id.clone());
    let recorded_at = headers
        .get("x-recorded-at")
        .and_then(|v| v.to_str().ok())
        .and_then(|s| DateTime::parse_from_rfc3339(s).ok())
        .map(|d| d.with_timezone(&Utc))
        .unwrap_or_else(Utc::now);
    let idempotency_key = headers
        .get("x-idempotency-key")
        .and_then(|v| v.to_str().ok())
        .map(|s| s.trim().chars().take(200).collect::<String>())
        .filter(|s| !s.is_empty());

    if let Some(key) = idempotency_key.as_deref()
        && let Ok(Some((id, path, size))) = state.db.find_by_idempotency(key)
    {
        return Ok(Json(serde_json::json!({
            "id": id,
            "path": path,
            "size": size,
            "duplicate": true
        })));
    }

    let mut multipart = Multipart::from_request(req, &state)
        .await
        .map_err(|e| (StatusCode::BAD_REQUEST, e.to_string()))?;

    let mut file_name = String::new();
    let mut tmp_path = None;
    let mut size = 0u64;

    while let Some(field) = multipart
        .next_field()
        .await
        .map_err(|e| (StatusCode::BAD_REQUEST, e.to_string()))?
    {
        if field.name() != Some("file") {
            continue;
        }
        let original = field
            .file_name()
            .map(|s| s.to_string())
            .unwrap_or_else(|| "recording.mp4".into());
        file_name = sanitize_filename(&original);
        if !file_name.to_lowercase().ends_with(".mp4") {
            file_name.push_str(".mp4");
        }
        let day = recorded_at.format("%Y/%m/%d").to_string();
        let rel_dir = format!("{pc_id}/{day}");
        let dest_dir = safe_join(&state.cfg.archive_dir, &rel_dir)
            .ok_or((StatusCode::BAD_REQUEST, "bad path".into()))?;
        tokio::fs::create_dir_all(&dest_dir)
            .await
            .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

        let unique = unique_name(&dest_dir, &file_name)
            .await
            .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e))?;
        file_name = unique
            .file_name()
            .unwrap()
            .to_string_lossy()
            .into_owned();
        let partial = unique.with_extension("mp4.partial");
        let mut out = tokio::fs::File::create(&partial)
            .await
            .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
        let mut field = field;
        loop {
            let chunk = field
                .chunk()
                .await
                .map_err(|e| (StatusCode::BAD_REQUEST, e.to_string()))?;
            let Some(chunk) = chunk else { break };
            size += chunk.len() as u64;
            tokio::io::AsyncWriteExt::write_all(&mut out, &chunk)
                .await
                .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
        }
        tokio::io::AsyncWriteExt::shutdown(&mut out)
            .await
            .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
        tokio::fs::rename(&partial, &unique)
            .await
            .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
        tmp_path = Some(unique);
        break;
    }

    let abs = tmp_path.ok_or((StatusCode::BAD_REQUEST, "file field required".into()))?;
    let rel = abs
        .strip_prefix(&state.cfg.archive_dir)
        .map_err(|_| (StatusCode::INTERNAL_SERVER_ERROR, "path".into()))?
        .to_string_lossy()
        .replace('\\', "/");

    state
        .db
        .upsert_computer(&pc_id, &display)
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e))?;
    let id = match state.db.insert_recording(
        &pc_id,
        &file_name,
        &rel,
        recorded_at,
        size as i64,
        None,
        idempotency_key.as_deref(),
    ) {
        Ok(id) => id,
        Err(e) if e.contains("UNIQUE") && idempotency_key.is_some() => {
            // Race: another request with the same key finished first.
            if let Ok(Some((id, path, size))) = state
                .db
                .find_by_idempotency(idempotency_key.as_deref().unwrap())
            {
                let _ = tokio::fs::remove_file(&abs).await;
                return Ok(Json(serde_json::json!({
                    "id": id,
                    "path": path,
                    "size": size,
                    "duplicate": true
                })));
            }
            return Err((StatusCode::INTERNAL_SERVER_ERROR, e));
        }
        Err(e) => return Err((StatusCode::INTERNAL_SERVER_ERROR, e)),
    };

    Ok(Json(serde_json::json!({ "id": id, "path": rel, "size": size })))
}

fn sanitize_label(s: &str) -> String {
    s.chars()
        .map(|c| match c {
            'a'..='z' | 'A'..='Z' | '0'..='9' | '-' | '_' | '.' | ' ' => c,
            _ => '_',
        })
        .collect::<String>()
        .trim()
        .chars()
        .take(64)
        .collect()
}

fn sanitize_filename(s: &str) -> String {
    let name = Path::new(s)
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("recording.mp4");
    name.chars()
        .map(|c| match c {
            'a'..='z' | 'A'..='Z' | '0'..='9' | '-' | '_' | '.' => c,
            _ => '_',
        })
        .take(180)
        .collect()
}

fn safe_join(root: &Path, relative: &str) -> Option<PathBuf> {
    let rel = Path::new(relative);
    if rel.is_absolute() {
        return None;
    }
    let mut out = root.to_path_buf();
    for comp in rel.components() {
        match comp {
            Component::Normal(c) => out.push(c),
            Component::CurDir => {}
            _ => return None,
        }
    }
    // Logical containment (works before the path exists on disk).
    if out.starts_with(root) {
        return Some(out);
    }
    None
}

async fn unique_name(dir: &Path, file_name: &str) -> Result<PathBuf, String> {
    let candidate = dir.join(file_name);
    if !candidate.exists() {
        return Ok(candidate);
    }
    let stem = Path::new(file_name)
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("recording");
    for i in 1..10_000 {
        let c = dir.join(format!("{stem}_{i}.mp4"));
        if !c.exists() {
            return Ok(c);
        }
    }
    Err("could not allocate unique name".into())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn safe_join_rejects_traversal() {
        let root = Path::new(r"C:\archive");
        assert!(safe_join(root, r"..\secret").is_none());
        assert!(safe_join(root, r"PC1/../../../Windows").is_none());
        assert!(safe_join(root, r"C:\Windows\file.mp4").is_none());
        let ok = safe_join(root, "PC1/2026/01/01/clip.mp4").unwrap();
        assert!(ok.ends_with(Path::new(r"PC1\2026\01\01\clip.mp4")) || ok.ends_with("PC1/2026/01/01/clip.mp4"));
    }

    #[test]
    fn parse_range_ok_and_suffix() {
        assert_eq!(parse_bytes_range("bytes=0-9"), Some((0, 9)));
        assert_eq!(parse_bytes_range("bytes=5-"), Some((5, u64::MAX)));
        assert!(parse_bytes_range("bytes=abc").is_none());
    }
}

use std::path::{Path, PathBuf};

use chrono::{DateTime, Utc};
use rusqlite::{Connection, OptionalExtension, params};
use serde::Serialize;

#[derive(Clone)]
pub struct Db {
    path: PathBuf,
}

#[derive(Debug, Serialize)]
pub struct ComputerRow {
    pub pc_id: String,
    pub display_name: String,
    pub recording_count: i64,
    pub last_recorded_at: Option<String>,
    pub total_bytes: i64,
}

#[derive(Debug, Serialize)]
pub struct RecordingRow {
    pub id: i64,
    pub pc_id: String,
    pub display_name: String,
    pub file_name: String,
    pub relative_path: String,
    pub recorded_at: String,
    pub received_at: String,
    pub size_bytes: i64,
    pub duration_secs: Option<f64>,
    pub ready: bool,
}

impl Db {
    pub fn open(archive_dir: &Path) -> Result<Self, String> {
        std::fs::create_dir_all(archive_dir).map_err(|e| e.to_string())?;
        let path = archive_dir.join("index.sqlite");
        let db = Self { path };
        db.with(|conn| {
            conn.execute_batch(
                r#"
                PRAGMA journal_mode=WAL;
                CREATE TABLE IF NOT EXISTS computers (
                    pc_id TEXT PRIMARY KEY,
                    display_name TEXT NOT NULL
                );
                CREATE TABLE IF NOT EXISTS recordings (
                    id INTEGER PRIMARY KEY AUTOINCREMENT,
                    pc_id TEXT NOT NULL,
                    file_name TEXT NOT NULL,
                    relative_path TEXT NOT NULL UNIQUE,
                    recorded_at TEXT NOT NULL,
                    received_at TEXT NOT NULL,
                    size_bytes INTEGER NOT NULL,
                    duration_secs REAL,
                    ready INTEGER NOT NULL DEFAULT 1,
                    FOREIGN KEY(pc_id) REFERENCES computers(pc_id)
                );
                CREATE INDEX IF NOT EXISTS idx_rec_pc_time ON recordings(pc_id, recorded_at DESC);
                "#,
            )
            .map_err(|e| e.to_string())?;
            // Idempotent agent retries (column may already exist).
            let _ = conn.execute(
                "ALTER TABLE recordings ADD COLUMN idempotency_key TEXT",
                [],
            );
            conn.execute_batch(
                "CREATE UNIQUE INDEX IF NOT EXISTS idx_rec_idempotency
                 ON recordings(idempotency_key) WHERE idempotency_key IS NOT NULL;",
            )
            .map_err(|e| e.to_string())?;
            Ok(())
        })?;
        Ok(db)
    }

    pub fn find_by_idempotency(
        &self,
        key: &str,
    ) -> Result<Option<(i64, String, i64)>, String> {
        self.with(|conn| {
            conn.query_row(
                "SELECT id, relative_path, size_bytes FROM recordings WHERE idempotency_key=?1",
                params![key],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
            )
            .optional()
            .map_err(|e| e.to_string())
        })
    }

    fn with<T>(&self, f: impl FnOnce(&Connection) -> Result<T, String>) -> Result<T, String> {
        let conn = Connection::open(&self.path).map_err(|e| e.to_string())?;
        f(&conn)
    }

    pub fn upsert_computer(&self, pc_id: &str, display_name: &str) -> Result<(), String> {
        self.with(|conn| {
            conn.execute(
                r#"INSERT INTO computers(pc_id, display_name) VALUES (?1, ?2)
                   ON CONFLICT(pc_id) DO UPDATE SET
                     display_name=CASE
                       WHEN computers.display_name = computers.pc_id THEN excluded.display_name
                       ELSE computers.display_name
                     END"#,
                params![pc_id, display_name],
            )
            .map_err(|e| e.to_string())?;
            Ok(())
        })
    }

    pub fn rename_computer(&self, pc_id: &str, display_name: &str) -> Result<(), String> {
        self.with(|conn| {
            let n = conn
                .execute(
                    "UPDATE computers SET display_name=?1 WHERE pc_id=?2",
                    params![display_name, pc_id],
                )
                .map_err(|e| e.to_string())?;
            if n == 0 {
                return Err("computer not found".into());
            }
            Ok(())
        })
    }

    pub fn insert_recording(
        &self,
        pc_id: &str,
        file_name: &str,
        relative_path: &str,
        recorded_at: DateTime<Utc>,
        size_bytes: i64,
        duration_secs: Option<f64>,
        idempotency_key: Option<&str>,
    ) -> Result<i64, String> {
        let received = Utc::now().to_rfc3339();
        self.with(|conn| {
            conn.execute(
                r#"INSERT INTO recordings(
                    pc_id, file_name, relative_path, recorded_at, received_at,
                    size_bytes, duration_secs, ready, idempotency_key
                ) VALUES (?1,?2,?3,?4,?5,?6,?7,1,?8)"#,
                params![
                    pc_id,
                    file_name,
                    relative_path,
                    recorded_at.to_rfc3339(),
                    received,
                    size_bytes,
                    duration_secs,
                    idempotency_key
                ],
            )
            .map_err(|e| e.to_string())?;
            Ok(conn.last_insert_rowid())
        })
    }

    pub fn list_computers(&self) -> Result<Vec<ComputerRow>, String> {
        self.with(|conn| {
            let mut stmt = conn
                .prepare(
                    r#"SELECT c.pc_id, c.display_name,
                              COUNT(r.id),
                              MAX(r.recorded_at),
                              COALESCE(SUM(r.size_bytes),0)
                       FROM computers c
                       LEFT JOIN recordings r ON r.pc_id = c.pc_id AND r.ready = 1
                       GROUP BY c.pc_id
                       ORDER BY (MAX(r.recorded_at) IS NULL), MAX(r.recorded_at) DESC, c.display_name"#,
                )
                .map_err(|e| e.to_string())?;
            let rows = stmt
                .query_map([], |row| {
                    Ok(ComputerRow {
                        pc_id: row.get(0)?,
                        display_name: row.get(1)?,
                        recording_count: row.get(2)?,
                        last_recorded_at: row.get(3)?,
                        total_bytes: row.get(4)?,
                    })
                })
                .map_err(|e| e.to_string())?
                .collect::<Result<Vec<_>, _>>()
                .map_err(|e| e.to_string())?;
            Ok(rows)
        })
    }

    pub fn list_recordings(
        &self,
        pc_id: Option<&str>,
        from: Option<&str>,
        to: Option<&str>,
        q: Option<&str>,
    ) -> Result<Vec<RecordingRow>, String> {
        self.with(|conn| {
            let mut sql = String::from(
                r#"SELECT r.id, r.pc_id, c.display_name, r.file_name, r.relative_path,
                          r.recorded_at, r.received_at, r.size_bytes, r.duration_secs, r.ready
                   FROM recordings r
                   JOIN computers c ON c.pc_id = r.pc_id
                   WHERE r.ready = 1"#,
            );
            let mut args: Vec<Box<dyn rusqlite::types::ToSql>> = Vec::new();
            if let Some(pc) = pc_id {
                sql.push_str(" AND r.pc_id = ?");
                args.push(Box::new(pc.to_string()));
            }
            if let Some(from) = from {
                sql.push_str(" AND r.recorded_at >= ?");
                args.push(Box::new(from.to_string()));
            }
            if let Some(to) = to {
                sql.push_str(" AND r.recorded_at <= ?");
                args.push(Box::new(to.to_string()));
            }
            if let Some(q) = q.filter(|s| !s.trim().is_empty()) {
                sql.push_str(" AND (c.display_name LIKE ? OR r.file_name LIKE ? OR r.pc_id LIKE ?)");
                let like = format!("%{q}%");
                args.push(Box::new(like.clone()));
                args.push(Box::new(like.clone()));
                args.push(Box::new(like));
            }
            sql.push_str(" ORDER BY r.recorded_at DESC LIMIT 500");
            let mut stmt = conn.prepare(&sql).map_err(|e| e.to_string())?;
            let params_ref: Vec<&dyn rusqlite::types::ToSql> =
                args.iter().map(|a| a.as_ref()).collect();
            let rows = stmt
                .query_map(params_ref.as_slice(), |row| {
                    Ok(RecordingRow {
                        id: row.get(0)?,
                        pc_id: row.get(1)?,
                        display_name: row.get(2)?,
                        file_name: row.get(3)?,
                        relative_path: row.get(4)?,
                        recorded_at: row.get(5)?,
                        received_at: row.get(6)?,
                        size_bytes: row.get(7)?,
                        duration_secs: row.get(8)?,
                        ready: row.get::<_, i64>(9)? != 0,
                    })
                })
                .map_err(|e| e.to_string())?
                .collect::<Result<Vec<_>, _>>()
                .map_err(|e| e.to_string())?;
            Ok(rows)
        })
    }

    pub fn recording_by_id(&self, id: i64) -> Result<Option<RecordingRow>, String> {
        self.with(|conn| {
            conn.query_row(
                r#"SELECT r.id, r.pc_id, c.display_name, r.file_name, r.relative_path,
                          r.recorded_at, r.received_at, r.size_bytes, r.duration_secs, r.ready
                   FROM recordings r JOIN computers c ON c.pc_id = r.pc_id
                   WHERE r.id = ?1"#,
                params![id],
                |row| {
                    Ok(RecordingRow {
                        id: row.get(0)?,
                        pc_id: row.get(1)?,
                        display_name: row.get(2)?,
                        file_name: row.get(3)?,
                        relative_path: row.get(4)?,
                        recorded_at: row.get(5)?,
                        received_at: row.get(6)?,
                        size_bytes: row.get(7)?,
                        duration_secs: row.get(8)?,
                        ready: row.get::<_, i64>(9)? != 0,
                    })
                },
            )
            .optional()
            .map_err(|e| e.to_string())
        })
    }

    /// Insert missing files found under archive_dir into the index.
    pub fn rescan(&self, archive_dir: &Path) -> Result<u32, String> {
        let mut added = 0u32;
        let walker = walk_mp4(archive_dir)?;
        for (rel, abs) in walker {
            let exists = self.with(|conn| {
                conn.query_row(
                    "SELECT 1 FROM recordings WHERE relative_path=?1",
                    params![rel],
                    |_| Ok(1),
                )
                .optional()
                .map_err(|e| e.to_string())
            })?;
            if exists.is_some() {
                continue;
            }
            let parts: Vec<&str> = rel.split('/').collect();
            // archive/<pc>/<yyyy>/<mm>/<dd>/<file>
            if parts.len() < 5 {
                continue;
            }
            let pc_id = parts[0];
            let file_name = parts[parts.len() - 1];
            let meta = std::fs::metadata(&abs).map_err(|e| e.to_string())?;
            let recorded = guess_recorded_at(&rel, file_name);
            self.upsert_computer(pc_id, pc_id)?;
            self.insert_recording(
                pc_id,
                file_name,
                &rel,
                recorded,
                meta.len() as i64,
                None,
                None,
            )?;
            added += 1;
        }
        Ok(added)
    }
}

fn guess_recorded_at(rel: &str, file_name: &str) -> DateTime<Utc> {
    let _ = file_name;
    let parts: Vec<&str> = rel.split('/').collect();
    if parts.len() >= 4 {
        if let (Ok(y), Ok(m), Ok(d)) = (
            parts[1].parse::<i32>(),
            parts[2].parse::<u32>(),
            parts[3].parse::<u32>(),
        ) {
            if let Some(date) = chrono::NaiveDate::from_ymd_opt(y, m, d) {
                return date.and_hms_opt(0, 0, 0).unwrap().and_utc();
            }
        }
    }
    Utc::now()
}

fn walk_mp4(root: &Path) -> Result<Vec<(String, PathBuf)>, String> {
    let mut out = Vec::new();
    fn rec(dir: &Path, root: &Path, out: &mut Vec<(String, PathBuf)>) -> Result<(), String> {
        for entry in std::fs::read_dir(dir).map_err(|e| e.to_string())? {
            let entry = entry.map_err(|e| e.to_string())?;
            let path = entry.path();
            if path.is_dir() {
                rec(&path, root, out)?;
            } else if path
                .extension()
                .and_then(|e| e.to_str())
                .is_some_and(|e| e.eq_ignore_ascii_case("mp4"))
            {
                let rel = path
                    .strip_prefix(root)
                    .map_err(|e| e.to_string())?
                    .to_string_lossy()
                    .replace('\\', "/");
                out.push((rel, path));
            }
        }
        Ok(())
    }
    rec(root, root, &mut out)?;
    Ok(out)
}

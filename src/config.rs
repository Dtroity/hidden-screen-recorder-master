use std::{
    fs,
    io::Write,
    path::{Path, PathBuf},
    sync::OnceLock,
};

use directories::ProjectDirs;
use serde::{Deserialize, Serialize};

use crate::hotkeys::Shortcuts;
use crate::identity;
use crate::{
    i18n::{Language, Message},
    msg,
};

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum NoiseSuppressionMethod {
    Speex,
    #[default]
    Rnnoise,
}

/// Daily recording window for the visible agent (local time, minutes from midnight).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(default)]
pub struct ScheduleConfig {
    pub enabled: bool,
    pub start_minutes: u32,
    pub end_minutes: u32,
}

impl Default for ScheduleConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            start_minutes: 9 * 60,
            end_minutes: 18 * 60,
        }
    }
}

/// Video encode backend. Missing field in old settings → Media Foundation.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EncodeBackendKind {
    MediaFoundation,
    #[default]
    Ffmpeg,
}

impl EncodeBackendKind {
    pub fn label(self, language: crate::i18n::Language) -> &'static str {
        match self {
            Self::MediaFoundation => language.text("Media Foundation", "Media Foundation"),
            Self::Ffmpeg => language.text("FFmpeg (HEVC)", "FFmpeg (HEVC)"),
        }
    }
}

/// Optional quality presets for office archiving. `Custom` keeps manual FPS/bitrate/resolution.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum QualityProfile {
    Custom,
    Economy,
    #[default]
    Balanced,
    Readable,
}

impl QualityProfile {
    pub fn label(self, language: crate::i18n::Language) -> &'static str {
        match self {
            Self::Custom => language.text("Custom", "Свой"),
            Self::Economy => language.text("Economy (archive)", "Экономичный (архив)"),
            Self::Balanced => language.text("Balanced", "Сбалансированный"),
            Self::Readable => language.text("Readable", "Читаемость"),
        }
    }

    /// FPS / Max1080p / bitrate_kbps for HEVC office profiles.
    pub fn apply(self) -> Option<(u32, ResolutionMode, u32)> {
        match self {
            Self::Custom => None,
            // Bitrate targets assume FFmpeg libx265 (HEVC), not H.264.
            Self::Economy => Some((1, ResolutionMode::Max1080p, 450)),
            Self::Balanced => Some((2, ResolutionMode::Max1080p, 800)),
            Self::Readable => Some((5, ResolutionMode::Max1080p, 1600)),
        }
    }
}

/// Encode resolution relative to the capture monitor (smaller = smaller files).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ResolutionMode {
    /// Full monitor pixels.
    Native,
    /// 75% of each side (~56% of pixels).
    Scale75,
    /// 50% of each side (¼ of pixels) — good office default on large screens.
    Scale50,
    /// Fit inside 1920×1080.
    #[default]
    Max1080p,
    /// Fit inside 1280×720 — smallest useful office mode.
    Max720p,
}

impl ResolutionMode {
    /// Even width/height for H.264.
    pub fn encode_size(self, src_w: u32, src_h: u32) -> (u32, u32) {
        let (mut w, mut h) = match self {
            Self::Native => (src_w, src_h),
            Self::Scale75 => (src_w * 3 / 4, src_h * 3 / 4),
            Self::Scale50 => (src_w / 2, src_h / 2),
            Self::Max1080p => fit_inside(src_w, src_h, 1920, 1080),
            Self::Max720p => fit_inside(src_w, src_h, 1280, 720),
        };
        w = w.max(2) & !1;
        h = h.max(2) & !1;
        (w, h)
    }

    pub fn label(self, language: crate::i18n::Language) -> &'static str {
        match self {
            Self::Native => language.text("Full resolution", "Полное разрешение"),
            Self::Scale75 => language.text("75% scale", "Масштаб 75%"),
            Self::Scale50 => language.text("50% scale", "Масштаб 50%"),
            Self::Max1080p => language.text("Max 1080p", "Не выше 1080p"),
            Self::Max720p => language.text("Max 720p", "Не выше 720p"),
        }
    }
}

fn fit_inside(src_w: u32, src_h: u32, max_w: u32, max_h: u32) -> (u32, u32) {
    if src_w <= max_w && src_h <= max_h {
        return (src_w, src_h);
    }
    let scale = (max_w as f64 / src_w as f64).min(max_h as f64 / src_h as f64);
    (
        ((src_w as f64) * scale).round() as u32,
        ((src_h as f64) * scale).round() as u32,
    )
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct AppConfig {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub language: Option<Language>,
    pub monitor_index: usize,
    pub fps: u32,
    pub bitrate_mbps: u32,
    /// Optional kbps override (FFmpeg profiles / sub‑Mbps). When set, takes precedence over `bitrate_mbps`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub bitrate_kbps: Option<u32>,
    pub resolution_mode: ResolutionMode,
    #[serde(default)]
    pub encode_backend: EncodeBackendKind,
    #[serde(default)]
    pub quality_profile: QualityProfile,
    /// Optional path to ffmpeg.exe; otherwise next to the app binary, then PATH.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ffmpeg_path: Option<PathBuf>,
    pub show_cursor: bool,
    pub system_audio: bool,
    pub microphone: bool,
    pub microphone_device: Option<String>,
    pub system_volume: f32,
    pub microphone_volume: f32,
    pub microphone_threshold_db: Option<f32>,
    pub microphone_noise_suppression: bool,
    pub microphone_noise_method: NoiseSuppressionMethod,
    pub microphone_suppression_db: i32,
    #[serde(default)]
    pub microphone_filter_defaults_version: u8,
    pub audio_buffer_ms: u32,
    pub audio_device_timestamps: bool,
    pub duration_minutes: u32,
    pub output_dir: PathBuf,
    pub filename_template: String,
    pub logging_enabled: bool,
    pub hide_window: bool,
    pub shortcuts: Shortcuts,
    /// Optional inventory label; empty means use Windows hostname.
    pub pc_id: Option<String>,
    pub upload_enabled: bool,
    pub upload_dir: Option<PathBuf>,
    pub upload_delete_local: bool,
    /// Write MP4 directly into `{upload_dir}/{pc_id}/` (e.g. Yandex Disk sync folder).
    pub record_directly_to_cloud: bool,
    /// HTTP archive server (optional; independent from folder upload).
    #[serde(default)]
    pub archive_upload_enabled: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub archive_url: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub archive_token: Option<String>,
    #[serde(default)]
    pub archive_delete_local: bool,
    /// Stable device id (UUID); created once if missing.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub device_id: Option<String>,
    pub schedule: ScheduleConfig,
    /// Agent records automatically when awake and inside the schedule (if enabled).
    pub agent_auto_record: bool,
    pub stop_on_sleep: bool,
    pub record_on_wake: bool,
    pub agent_autostart: bool,
    /// Max concurrent recordings across console + agent processes (default 1).
    pub max_recording_sessions: u32,
    /// Bumped when office size defaults are applied once to existing installs.
    #[serde(default)]
    pub size_defaults_version: u8,
}

impl Default for AppConfig {
    fn default() -> Self {
        let output_dir = directories::UserDirs::new()
            .and_then(|dirs| {
                dirs.video_dir()
                    .map(|path| path.join("hidden-screen-recorder"))
            })
            .unwrap_or_else(|| PathBuf::from("recordings"));

        Self {
            language: None,
            monitor_index: 1,
            // FFmpeg HEVC Balanced: ~1080p / 2 fps / 800 kb/s (readable office text).
            fps: 2,
            bitrate_mbps: 1,
            bitrate_kbps: Some(800),
            resolution_mode: ResolutionMode::Max1080p,
            encode_backend: EncodeBackendKind::Ffmpeg,
            quality_profile: QualityProfile::Balanced,
            ffmpeg_path: None,
            show_cursor: true,
            system_audio: false,
            microphone: false,
            microphone_device: None,
            system_volume: 1.0,
            microphone_volume: 1.0,
            microphone_threshold_db: None,
            microphone_noise_suppression: true,
            microphone_noise_method: NoiseSuppressionMethod::Rnnoise,
            microphone_suppression_db: -30,
            microphone_filter_defaults_version: 1,
            audio_buffer_ms: 250,
            audio_device_timestamps: true,
            duration_minutes: 60,
            output_dir,
            filename_template: crate::filename::DEFAULT_TEMPLATE.to_owned(),
            logging_enabled: false,
            hide_window: true,
            shortcuts: Shortcuts::default(),
            pc_id: None,
            upload_enabled: false,
            upload_dir: None,
            upload_delete_local: false,
            record_directly_to_cloud: false,
            archive_upload_enabled: false,
            archive_url: None,
            archive_token: None,
            archive_delete_local: false,
            device_id: None,
            schedule: ScheduleConfig::default(),
            agent_auto_record: true,
            stop_on_sleep: true,
            record_on_wake: true,
            agent_autostart: false,
            max_recording_sessions: 1,
            size_defaults_version: 4,
        }
    }
}

impl AppConfig {
    pub fn language(&self) -> Language {
        self.language.unwrap_or_else(Language::system)
    }

    /// PC label used in filenames and upload subfolders.
    pub fn effective_pc_id(&self) -> String {
        let custom = self.pc_id.as_deref().unwrap_or("").trim();
        if custom.is_empty() {
            identity::hostname()
        } else {
            identity::sanitize_label(custom)
        }
    }

    /// Folder where new recordings are written.
    pub fn effective_output_dir(&self) -> PathBuf {
        if self.record_directly_to_cloud
            && let Some(root) = self.upload_dir.as_ref()
            && !root.as_os_str().is_empty()
        {
            return root.join(self.effective_pc_id());
        }
        self.output_dir.clone()
    }

    /// Effective video bitrate in bits per second.
    pub fn video_bitrate_bps(&self) -> u32 {
        if let Some(kbps) = self.bitrate_kbps {
            kbps.saturating_mul(1_000).max(1_000)
        } else {
            self.bitrate_mbps.saturating_mul(1_000_000).max(1_000)
        }
    }

    /// Apply a starter quality profile to FPS / resolution / bitrate fields.
    pub fn apply_quality_profile(&mut self, profile: QualityProfile) {
        self.quality_profile = profile;
        if let Some((fps, resolution, kbps)) = profile.apply() {
            self.fps = fps;
            self.resolution_mode = resolution;
            self.bitrate_kbps = Some(kbps);
            self.bitrate_mbps = (kbps.div_ceil(1000)).max(1);
        }
    }

    /// True when at least one audio source is enabled.
    pub fn audio_enabled(&self) -> bool {
        self.system_audio || self.microphone
    }

    /// True when archive HTTP upload should run (toggle on, or URL+token both set).
    pub fn archive_upload_active(&self) -> bool {
        let url_ok = self
            .archive_url
            .as_ref()
            .map(|s| !s.trim().is_empty())
            .unwrap_or(false);
        let token_ok = self
            .archive_token
            .as_ref()
            .map(|s| !s.trim().is_empty())
            .unwrap_or(false);
        (self.archive_upload_enabled || (url_ok && token_ok)) && url_ok && token_ok
    }

    /// True when binaries live in an OfficePC-style `bin\` folder (portable pack).
    pub fn is_portable_layout() -> bool {
        std::env::current_exe()
            .ok()
            .and_then(|exe| {
                exe.parent().map(|dir| {
                    dir.file_name()
                        .and_then(|n| n.to_str())
                        .is_some_and(|n| n.eq_ignore_ascii_case("bin"))
                })
            })
            .unwrap_or(false)
    }

    /// Package root for OfficePC (`…\OfficePC` when exe is `…\OfficePC\bin\*.exe`).
    pub fn portable_root() -> Option<PathBuf> {
        let exe = std::env::current_exe().ok()?;
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

    /// Candidate settings locations for a portable OfficePC pack:
    /// Under `bin\` → **only** `OfficePC/settings.json` (ignore AppData — avoids split brain).
    /// Otherwise → exe dir, then AppData / legacy.
    fn candidate_paths() -> Vec<PathBuf> {
        let mut paths = Vec::new();
        if let Some(root) = Self::portable_root() {
            paths.push(root.join("settings.json"));
            if Self::is_portable_layout() {
                return paths;
            }
        }
        if let Ok(exe) = std::env::current_exe()
            && let Some(dir) = exe.parent()
        {
            let p = dir.join("settings.json");
            if !paths.iter().any(|x| x == &p) {
                paths.push(p);
            }
        }
        if let Some(dirs) = ProjectDirs::from("dev", "", "hidden-screen-recorder") {
            paths.push(dirs.config_dir().join("settings.json"));
        }
        if let Some(dirs) = ProjectDirs::from("dev", "ScreenRecord", "ScreenRecord") {
            paths.push(dirs.config_dir().join("settings.json"));
        }
        paths
    }

    fn path() -> Option<PathBuf> {
        // Prefer an already-used path (load/save stick to the same file).
        if let Some(cached) = SETTINGS_PATH.get() {
            return Some(cached.clone());
        }
        let candidates = Self::candidate_paths();
        for path in &candidates {
            if path.is_file() {
                return Some(path.clone());
            }
        }
        candidates.into_iter().next()
    }

    fn remember_path(path: &Path) {
        let _ = SETTINGS_PATH.set(path.to_path_buf());
    }

    pub fn load() -> Result<(Self, Option<Message>), Message> {
        let candidates = Self::candidate_paths();
        if candidates.is_empty() {
            return Err(msg!(
                "The settings directory is unavailable",
                "Каталог настроек недоступен"
            ));
        }
        for path in &candidates {
            if path.is_file() {
                Self::remember_path(path);
                return Self::load_from(path);
            }
        }
        // Fresh install: prefer portable OfficePC/settings.json when running from bin\.
        let preferred = candidates[0].clone();
        Self::remember_path(&preferred);
        Self::load_from(&preferred)
    }

    fn load_from(path: &Path) -> Result<(Self, Option<Message>), Message> {
        let json = match fs::read_to_string(path) {
            Ok(json) => json,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                return Ok((Self::default(), None));
            }
            Err(error) => {
                return Err(msg!(
                    "Could not read {}: {error}",
                    "Не удалось прочитать {}: {error}",
                    path.display()
                ));
            }
        };
        let mut config: Self = serde_json::from_str(&json).map_err(|error| {
            let backup = path.with_extension(format!(
                "corrupt-{}.json",
                chrono::Local::now().format("%Y%m%d%H%M%S%f")
            ));
            match fs::copy(path, &backup) {
                Ok(_) => msg!(
                    "Invalid settings: {error}. Backup: {}",
                    "Повреждены настройки: {error}. Копия: {}",
                    backup.display()
                ),
                Err(copy_error) => msg!(
                    "Invalid settings: {error}. Backup failed: {copy_error}",
                    "Повреждены настройки: {error}. Не удалось сохранить копию: {copy_error}"
                ),
            }
        })?;
        // Apply the enabled RNNoise default once to existing installations.
        // Subsequent changes made by the user are saved and respected.
        if config.microphone_filter_defaults_version == 0 {
            config.microphone_noise_suppression = true;
            config.microphone_noise_method = NoiseSuppressionMethod::Rnnoise;
            config.microphone_filter_defaults_version = 1;
        }
        // One-time shrink for previous office defaults (3–12 Mbps / 15–30 FPS).
        let mut size_migrated = false;
        if config.size_defaults_version < 1 {
            if (3..=12).contains(&config.bitrate_mbps) {
                config.bitrate_mbps = 2;
                size_migrated = true;
            }
            if (15..=30).contains(&config.fps) {
                config.fps = 10;
                size_migrated = true;
            }
            if matches!(
                config.resolution_mode,
                ResolutionMode::Native | ResolutionMode::Scale75 | ResolutionMode::Max1080p
            ) {
                config.resolution_mode = ResolutionMode::Max720p;
                size_migrated = true;
            }
            config.size_defaults_version = 1;
        }
        // v2: FFmpeg HEVC + Balanced 1080p + audio off (office archive product defaults).
        let mut hevc_defaults_migrated = false;
        if config.size_defaults_version < 2 {
            config.encode_backend = EncodeBackendKind::Ffmpeg;
            config.apply_quality_profile(QualityProfile::Balanced);
            config.system_audio = false;
            config.microphone = false;
            config.size_defaults_version = 2;
            hevc_defaults_migrated = true;
        }
        // v3: enable logging; avoid endless single-file sessions.
        let mut segment_migrated = false;
        if config.size_defaults_version < 3 {
            if config.duration_minutes == 0 {
                config.duration_minutes = 60;
                segment_migrated = true;
            }
            config.logging_enabled = true;
            config.size_defaults_version = 3;
        }
        // v4: office segments 60 min (was briefly 15; also fixes short clips from sleep false-positives).
        if config.size_defaults_version < 4 {
            if config.duration_minutes > 0 && config.duration_minutes < 30 {
                config.duration_minutes = 60;
                segment_migrated = true;
            }
            if config.duration_minutes == 0 {
                config.duration_minutes = 60;
                segment_migrated = true;
            }
            config.size_defaults_version = 4;
        }
        let mut assigned_device_id = false;
        if config
            .device_id
            .as_ref()
            .map(|s| s.trim().is_empty())
            .unwrap_or(true)
        {
            config.device_id = Some(uuid::Uuid::new_v4().to_string());
            assigned_device_id = true;
        }
        let before = serde_json::to_string(&config).map_err(|error| error.to_string())?;
        // Preserve documented advanced values and expose the same ranges in UI.
        config.fps = config.fps.clamp(1, 240);
        config.bitrate_mbps = config.bitrate_mbps.clamp(1, 100);
        if let Some(kbps) = config.bitrate_kbps.as_mut() {
            *kbps = (*kbps).clamp(50, 50_000);
        }
        config.system_volume = config.system_volume.clamp(0.0, 1.5);
        config.microphone_volume = config.microphone_volume.clamp(0.0, 1.5);
        config.microphone_suppression_db = config.microphone_suppression_db.clamp(-60, 0);
        config.microphone_threshold_db = config
            .microphone_threshold_db
            .filter(|value| value.is_finite())
            .map(|value| value.clamp(-60.0, -10.0));
        config.audio_buffer_ms = config.audio_buffer_ms.clamp(100, 1000);
        config.duration_minutes = config.duration_minutes.min(24 * 60);
        if !config.shortcuts.is_valid() {
            config.shortcuts = Shortcuts::default();
        }
        if config.output_dir.as_os_str().is_empty() {
            config.output_dir = Self::default().output_dir;
        }
        config.schedule.start_minutes = config.schedule.start_minutes.min(24 * 60 - 1);
        config.schedule.end_minutes = config.schedule.end_minutes.min(24 * 60 - 1);
        config.max_recording_sessions = config.max_recording_sessions.clamp(1, 8);
        if let Some(pc_id) = config.pc_id.as_mut() {
            *pc_id = identity::sanitize_label(pc_id);
            if pc_id.is_empty() {
                config.pc_id = None;
            }
        }
        if let Some(url) = config.archive_url.as_mut() {
            *url = url.trim().to_owned();
            if url.is_empty() {
                config.archive_url = None;
            }
        }
        let changed =
            before != serde_json::to_string(&config).map_err(|error| error.to_string())?;
        if size_migrated || assigned_device_id || hevc_defaults_migrated || segment_migrated {
            let _ = config.save_to(path);
        }
        Ok((
            config,
            if size_migrated {
                Some(msg!(
                    "Recording quality was lowered to save disk space (720p / 10 FPS / 2 Mbps). You can raise it in settings.",
                    "Качество записи снижено для экономии места (720p / 10 FPS / 2 Мбит/с). Можно поднять в настройках."
                ))
            } else if hevc_defaults_migrated {
                Some(msg!(
                    "Defaults updated: FFmpeg HEVC, Balanced 1080p, audio off. Change in settings if needed.",
                    "Обновлены значения по умолчанию: FFmpeg HEVC, Сбалансированный 1080p, звук выкл. При необходимости измените в настройках."
                ))
            } else if segment_migrated {
                Some(msg!(
                    "Recordings now split every 60 minutes (unless sleep/shutdown stops sooner).",
                    "Запись нарезается каждые 60 минут (раньше — при сне или выключении)."
                ))
            } else {
                changed.then(|| {
                    msg!(
                        "Invalid settings were corrected; check your preferences and shortcuts.",
                        "Некорректные настройки исправлены; проверьте значения и сочетания клавиш."
                    )
                })
            },
        ))
    }

    pub fn save(&self) -> Result<(), Message> {
        let path = Self::path().ok_or(msg!(
            "The settings directory is unavailable",
            "Каталог настроек недоступен"
        ))?;
        Self::remember_path(&path);
        self.save_to(&path).map_err(|error| {
            msg!(
                "Could not save {}: {error}",
                "Не удалось сохранить {}: {error}",
                path.display()
            )
        })
    }

    fn save_to(&self, path: &Path) -> Result<(), Box<dyn std::error::Error>> {
        let parent = path.parent().ok_or("Missing settings directory")?;
        fs::create_dir_all(parent)?;
        let mut temporary = tempfile::NamedTempFile::new_in(parent)?;
        temporary.write_all(serde_json::to_string_pretty(self)?.as_bytes())?;
        temporary.as_file().sync_all()?;
        temporary.persist(path)?;
        Ok(())
    }
}

static SETTINGS_PATH: OnceLock<PathBuf> = OnceLock::new();

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rnnoise_is_enabled_by_default_and_migrates_existing_settings_once() {
        let defaults = AppConfig::default();
        assert!(defaults.microphone_noise_suppression);
        assert_eq!(
            defaults.microphone_noise_method,
            NoiseSuppressionMethod::Rnnoise
        );
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("settings.json");
        // Pin size_defaults_version so product migrations do not rewrite this fixture.
        fs::write(&path, r#"{"microphone_noise_suppression":false,"microphone_noise_method":"speex","microphone_volume":0.8,"size_defaults_version":4}"#).unwrap();
        let (mut config, warning) = AppConfig::load_from(&path).unwrap();
        assert!(warning.is_none());
        assert!(config.microphone_noise_suppression);
        assert_eq!(
            config.microphone_noise_method,
            NoiseSuppressionMethod::Rnnoise
        );
        assert_eq!(config.microphone_volume, 0.8);
        config.microphone_noise_suppression = false;
        config.microphone_noise_method = NoiseSuppressionMethod::Speex;
        config.save_to(&path).unwrap();
        let (loaded, warning) = AppConfig::load_from(&path).unwrap();
        assert!(warning.is_none());
        assert!(
            !loaded.microphone_noise_suppression,
            "an explicit choice after migration must persist"
        );
        assert_eq!(
            loaded.microphone_noise_method,
            NoiseSuppressionMethod::Speex
        );
    }

    #[test]
    fn atomic_save_replaces_existing_valid_settings() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("settings.json");
        fs::write(&path, "previous contents").unwrap();
        let config = AppConfig {
            fps: 144,
            duration_minutes: 1000,
            ..Default::default()
        };
        config.save_to(&path).unwrap();
        let (loaded, warning) = AppConfig::load_from(&path).unwrap();
        assert_eq!(loaded.fps, 144);
        assert_eq!(loaded.duration_minutes, 1000);
        assert!(warning.is_none());
        assert_eq!(fs::read_dir(directory.path()).unwrap().count(), 1);
    }

    #[test]
    fn corrupt_settings_are_reported_and_backed_up() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("settings.json");
        fs::write(&path, "{broken").unwrap();
        assert!(
            AppConfig::load_from(&path)
                .unwrap_err()
                .render(Language::Ru)
                .contains("Повреждены настройки")
        );
        let files: Vec<_> = fs::read_dir(directory.path())
            .unwrap()
            .map(|entry| entry.unwrap().path())
            .collect();
        assert_eq!(files.len(), 2);
        assert!(
            files
                .iter()
                .all(|file| fs::read_to_string(file).unwrap() == "{broken")
        );
    }

    #[test]
    fn invalid_values_are_corrected_with_a_warning() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("settings.json");
        fs::write(&path, r#"{"fps":0,"bitrate_mbps":300,"microphone_threshold_db":-100,"duration_minutes":99999,"shortcuts":{"stop":{"key":120,"ctrl":true,"alt":false,"shift":false}},"size_defaults_version":4}"#).unwrap();
        let (loaded, warning) = AppConfig::load_from(&path).unwrap();
        assert_eq!(loaded.fps, 1);
        assert_eq!(loaded.bitrate_mbps, 100);
        assert_eq!(loaded.microphone_threshold_db, Some(-60.0));
        assert_eq!(loaded.duration_minutes, 1440);
        assert!(loaded.shortcuts.is_valid());
        assert!(warning.is_some());
    }

    #[test]
    fn failed_save_preserves_original_file() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("settings.json");
        fs::write(&path, "original").unwrap();
        let guard = fs::OpenOptions::new().read(true).open(&path).unwrap();
        assert!(AppConfig::default().save_to(&path).is_err());
        assert_eq!(fs::read_to_string(&path).unwrap(), "original");
        drop(guard);
    }

    #[test]
    fn missing_fields_use_product_defaults() {
        let config: AppConfig = serde_json::from_str(r#"{"fps":10,"bitrate_mbps":2}"#).unwrap();
        assert_eq!(config.encode_backend, EncodeBackendKind::Ffmpeg);
        assert_eq!(config.quality_profile, QualityProfile::Balanced);
        assert_eq!(config.bitrate_kbps, None);
        assert_eq!(config.video_bitrate_bps(), 2_000_000);
        assert!(!config.audio_enabled());
    }

    #[test]
    fn new_defaults_are_ffmpeg_hevc_balanced_1080p_silent() {
        let config = AppConfig::default();
        assert_eq!(config.encode_backend, EncodeBackendKind::Ffmpeg);
        assert_eq!(config.quality_profile, QualityProfile::Balanced);
        assert_eq!(config.resolution_mode, ResolutionMode::Max1080p);
        assert_eq!(config.fps, 2);
        assert_eq!(config.bitrate_kbps, Some(800));
        assert!(!config.system_audio);
        assert!(!config.microphone);
        assert!(!config.audio_enabled());
    }

    #[test]
    fn bitrate_kbps_overrides_mbps() {
        let mut config = AppConfig {
            bitrate_mbps: 2,
            bitrate_kbps: Some(300),
            ..Default::default()
        };
        assert_eq!(config.video_bitrate_bps(), 300_000);
        config.apply_quality_profile(QualityProfile::Balanced);
        assert_eq!(config.fps, 2);
        assert_eq!(config.resolution_mode, ResolutionMode::Max1080p);
        assert_eq!(config.bitrate_kbps, Some(800));
        config.apply_quality_profile(QualityProfile::Economy);
        assert_eq!(config.fps, 1);
        assert_eq!(config.bitrate_kbps, Some(450));
    }

    #[test]
    fn old_settings_keep_hidden_mode_and_disable_logging() {
        let config: AppConfig = serde_json::from_str(r#"{"fps":60,"microphone":true}"#).unwrap();
        assert_eq!(config.fps, 60);
        assert!(config.microphone);
        assert_eq!(config.microphone_threshold_db, None);
        assert!(config.microphone_noise_suppression);
        assert_eq!(
            config.microphone_noise_method,
            NoiseSuppressionMethod::Rnnoise
        );
        assert_eq!(config.microphone_suppression_db, -30);
        assert!(config.hide_window);
        assert!(!config.logging_enabled);
        assert_eq!(config.shortcuts, Shortcuts::default());
        assert_eq!(config.language, None);
        assert_eq!(config.language(), Language::system());
    }

    #[test]
    fn new_preferences_survive_serialization() {
        let config = AppConfig {
            logging_enabled: true,
            hide_window: false,
            audio_buffer_ms: 500,
            audio_device_timestamps: false,
            microphone_threshold_db: Some(-38.0),
            microphone_noise_suppression: true,
            microphone_noise_method: NoiseSuppressionMethod::Rnnoise,
            microphone_suppression_db: -42,
            filename_template: "Game_{d}_{t}".to_owned(),
            shortcuts: Shortcuts {
                start: crate::hotkeys::Shortcut {
                    key: 0x52,
                    ctrl: true,
                    alt: false,
                    shift: true,
                },
                ..Default::default()
            },
            ..Default::default()
        };
        let json = serde_json::to_string(&config).unwrap();
        let restored: AppConfig = serde_json::from_str(&json).unwrap();
        assert!(restored.logging_enabled);
        assert!(!restored.hide_window);
        assert_eq!(restored.audio_buffer_ms, 500);
        assert!(!restored.audio_device_timestamps);
        assert_eq!(restored.microphone_threshold_db, Some(-38.0));
        assert!(restored.microphone_noise_suppression);
        assert_eq!(
            restored.microphone_noise_method,
            NoiseSuppressionMethod::Rnnoise
        );
        assert_eq!(restored.microphone_suppression_db, -42);
        assert_eq!(restored.filename_template, "Game_{d}_{t}");
        assert_eq!(restored.shortcuts, config.shortcuts);
    }

    #[test]
    fn explicit_language_survives_save_and_overrides_system_default() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("settings.json");
        for language in [Language::En, Language::Ru] {
            let config = AppConfig {
                language: Some(language),
                ..Default::default()
            };
            config.save_to(&path).unwrap();
            let (restored, warning) = AppConfig::load_from(&path).unwrap();
            assert_eq!(restored.language, Some(language));
            assert_eq!(restored.language(), language);
            assert!(warning.is_none());
        }
    }
}

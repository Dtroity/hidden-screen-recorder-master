use crate::{i18n::Message, msg};
use std::{
    fs::{self, File, OpenOptions},
    io::{Read, Seek, SeekFrom},
    path::Path,
    path::PathBuf,
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
        mpsc::{self, Receiver, Sender},
    },
    thread,
    time::{Duration, Instant},
};

use chrono::Local;
use windows::Win32::{
    Foundation::ERROR_SUCCESS,
    Graphics::Gdi::{
        BI_RGB, BITMAPINFO, BITMAPINFOHEADER, BitBlt, CAPTUREBLT, CreateCompatibleDC,
        CreateDIBSection, DIB_RGB_COLORS, DeleteDC, DeleteObject, GetDC, GetMonitorInfoW, HBITMAP,
        HDC, HGDIOBJ, HMONITOR, MONITORINFO, ROP_CODE, ReleaseDC, SRCCOPY, SelectObject,
    },
    System::Registry::{
        HKEY_CURRENT_USER, KEY_SET_VALUE, REG_OPTION_NON_VOLATILE, REG_SZ, RegCloseKey,
        RegCreateKeyExW, RegSetValueExW,
    },
    UI::WindowsAndMessaging::{
        CURSOR_SHOWING, CURSORINFO, DI_NORMAL, DrawIconEx, GetCursorInfo, GetIconInfo, HICON,
        ICONINFO,
    },
};
use windows::core::{HSTRING, PCWSTR};
use windows_capture::{
    capture::{Context, GraphicsCaptureApiHandler},
    frame::Frame,
    graphics_capture_api::{GraphicsCaptureApi, InternalCaptureControl},
    monitor::Monitor,
    settings::{
        ColorFormat, CursorCaptureSettings, DirtyRegionSettings, DrawBorderSettings,
        GraphicsCaptureItemType, MinimumUpdateIntervalSettings, SecondaryWindowSettings, Settings,
    },
};

use crate::{
    audio::AudioPipeline,
    config::AppConfig,
    debug_log,
    encode::{self, EncodeParams, SharedEncoder},
};

#[derive(Debug, Clone)]
pub enum RecorderEvent {
    Started(PathBuf),
    AudioWarning(Message),
    Finished(PathBuf),
    Failed(Message),
}

#[derive(Default)]
pub struct RecorderManager {
    stop: Option<Arc<AtomicBool>>,
    worker: Option<thread::JoinHandle<()>>,
    relay: Option<thread::JoinHandle<()>>,
    session: Option<crate::session::SessionGuard>,
}

impl RecorderManager {
    pub fn start(
        &mut self,
        config: AppConfig,
        stop: Arc<AtomicBool>,
        wake: Arc<dyn Fn() + Send + Sync>,
    ) -> Receiver<RecorderEvent> {
        self.shutdown();
        debug_log::write("RecorderManager::start called");
        let (worker_sender, worker_receiver) = mpsc::channel();
        let (event_sender, event_receiver) = mpsc::channel();
        self.stop = Some(stop.clone());

        let failure_sender = event_sender.clone();
        let failure_wake = wake.clone();
        match crate::session::try_acquire(config.max_recording_sessions) {
            Ok(guard) => self.session = Some(guard),
            Err(error) => {
                debug_log::write(format!("recording session rejected: {error}"));
                let _ = failure_sender.send(RecorderEvent::Failed(error));
                failure_wake();
                return event_receiver;
            }
        }
        match thread::Builder::new()
            .name("recorder-events".into())
            .spawn(move || {
                while let Ok(event) = worker_receiver.recv() {
                    if event_sender.send(event).is_err() {
                        break;
                    }
                    wake();
                }
            }) {
            Ok(relay) => self.relay = Some(relay),
            Err(error) => {
                self.session = None;
                let _ = failure_sender.send(RecorderEvent::Failed(msg!(
                    "Could not start the recording event handler: {error}",
                    "Не удалось запустить обработчик записи: {error}"
                )));
                failure_wake();
                return event_receiver;
            }
        }
        let panic_sender = worker_sender.clone();
        match thread::Builder::new()
            .name("screen-recorder".into())
            .spawn(move || {
                if std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                    run(config, stop, worker_sender)
                }))
                .is_err()
                {
                    let _ = panic_sender.send(RecorderEvent::Failed(msg!(
                        "The recording thread crashed.",
                        "Поток записи завершился аварийно."
                    )));
                }
            }) {
            Ok(worker) => self.worker = Some(worker),
            Err(error) => {
                self.session = None;
                let _ = failure_sender.send(RecorderEvent::Failed(msg!(
                    "Could not start recording: {error}",
                    "Не удалось запустить запись: {error}"
                )));
                failure_wake();
            }
        }

        event_receiver
    }

    pub fn stop(&self) {
        debug_log::write("RecorderManager::stop called");
        if let Some(stop) = &self.stop {
            stop.store(true, Ordering::Release);
        }
    }

    pub fn clear(&mut self) {
        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
        }
        if let Some(relay) = self.relay.take() {
            let _ = relay.join();
        }
        self.stop = None;
        // Release the cross-process session slot after the worker has stopped.
        self.session = None;
    }
}

impl RecorderManager {
    pub fn shutdown(&mut self) {
        self.stop();
        self.clear();
    }
}
impl Drop for RecorderManager {
    fn drop(&mut self) {
        self.shutdown();
    }
}

// create_new arbitrates between instances; the empty file reserves the name.
// Close its handle before WinRT opens the file with exclusive sharing.
fn reserve_output(directory: &Path, stem: &str) -> std::io::Result<PathBuf> {
    for suffix in 0..u64::MAX {
        let name = if suffix == 0 {
            format!("{stem}.mp4")
        } else {
            format!("{stem}_{suffix}.mp4")
        };
        let path = directory.join(name);
        match OpenOptions::new()
            .read(true)
            .write(true)
            .create_new(true)
            .open(&path)
        {
            Ok(_) => return Ok(path),
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(error) => return Err(error),
        }
    }
    Err(std::io::Error::other(
        "Could not reserve a recording filename",
    ))
}

/// True when the MP4 has a `moov` atom (required for normal playback).
/// Incomplete stops (sleep/crash) often leave a stream-only file that DirectShow
/// reports as File Source (Async.) with an unknown subtype.
pub(crate) fn mp4_is_playable(path: &Path) -> bool {
    let Ok(mut file) = File::open(path) else {
        return false;
    };
    let Ok(meta) = file.metadata() else {
        return false;
    };
    let len = meta.len();
    if len < 16 {
        return false;
    }
    let mut offset = 0u64;
    while offset + 8 <= len {
        if file.seek(SeekFrom::Start(offset)).is_err() {
            return false;
        }
        let mut header = [0u8; 8];
        if file.read_exact(&mut header).is_err() {
            return false;
        }
        let size32 = u32::from_be_bytes([header[0], header[1], header[2], header[3]]);
        let kind = &header[4..8];
        if kind == b"moov" {
            return true;
        }
        let box_size = match size32 {
            0 => return false,
            1 => {
                let mut ext = [0u8; 8];
                if file.read_exact(&mut ext).is_err() {
                    return false;
                }
                u64::from_be_bytes(ext)
            }
            n => u64::from(n),
        };
        if box_size < 8 || offset.saturating_add(box_size) > len {
            return false;
        }
        offset = offset.saturating_add(box_size);
    }
    false
}

fn discard_unplayable_recording(path: &Path) {
    if path.exists() && !mp4_is_playable(path) {
        debug_log::write(format!(
            "removing incomplete/unplayable recording: {}",
            path.display()
        ));
        if let Err(error) = fs::remove_file(path) {
            debug_log::write(format!("could not remove incomplete recording: {error}"));
        }
    }
}

/// Drop the empty name-reservation file (and FFmpeg temp) after a failed start.
fn release_reserved_output(path: &Path) {
    discard_unplayable_recording(path);
    // Matches encode/ffmpeg.rs partial_path naming (and legacy .mp4.partial).
    let stem = path
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("recording");
    let parent = path.parent().unwrap_or_else(|| Path::new("."));
    for name in [
        format!("{stem}.partial.mp4"),
        format!("{stem}.mp4.partial"),
        format!("{stem}.partial"),
    ] {
        let partial = parent.join(name);
        if partial.is_file() {
            let _ = fs::remove_file(&partial);
        }
    }
}

/// After reboot/crash: promote playable `*.partial.mp4` to final `.mp4`, delete the rest.
/// Returns paths of recovered final files (ready to upload).
///
/// Skips files that are still locked (active FFmpeg encode) — rename fails on Windows.
pub fn recover_orphan_partials(output_dir: &Path) -> Vec<PathBuf> {
    let Ok(entries) = fs::read_dir(output_dir) else {
        return Vec::new();
    };
    let mut recovered = Vec::new();
    for entry in entries.flatten() {
        let path = entry.path();
        let Some(name) = path.file_name().and_then(|n| n.to_str()) else {
            continue;
        };
        let lower = name.to_ascii_lowercase();
        let final_name = if let Some(prefix) = lower.strip_suffix(".partial.mp4") {
            format!("{}.mp4", &name[..prefix.len()])
        } else if let Some(prefix) = lower.strip_suffix(".mp4.partial") {
            format!("{}.mp4", &name[..prefix.len()])
        } else {
            continue;
        };
        // Still growing / locked by encoder → leave alone.
        if file_recently_modified(&path, std::time::Duration::from_secs(8)) {
            debug_log::write(format!(
                "orphan partial skip (still writing): {}",
                path.display()
            ));
            continue;
        }
        let final_path = output_dir.join(&final_name);
        if mp4_is_playable(&path) {
            if final_path.exists() {
                let _ = fs::remove_file(&path);
                debug_log::write(format!(
                    "orphan partial discarded (final already exists): {}",
                    path.display()
                ));
                continue;
            }
            match fs::rename(&path, &final_path) {
                Ok(()) => {
                    debug_log::write(format!(
                        "orphan partial recovered -> {}",
                        final_path.display()
                    ));
                    recovered.push(final_path);
                }
                Err(error) => {
                    debug_log::write(format!(
                        "orphan partial rename failed {}: {error}",
                        path.display()
                    ));
                }
            }
        } else {
            let _ = fs::remove_file(&path);
            debug_log::write(format!(
                "orphan partial discarded (no moov): {}",
                path.display()
            ));
        }
    }
    recovered
}

fn file_recently_modified(path: &Path, max_age: std::time::Duration) -> bool {
    let Ok(meta) = fs::metadata(path) else {
        return false;
    };
    let Ok(modified) = meta.modified() else {
        return false;
    };
    modified.elapsed().map(|age| age < max_age).unwrap_or(false)
}

fn emit_finished_or_reject(events: &Sender<RecorderEvent>, output: PathBuf) {
    if mp4_is_playable(&output) {
        debug_log::write(format!(
            "recording finished successfully: {}",
            output.display()
        ));
        let _ = events.send(RecorderEvent::Finished(output));
        return;
    }
    debug_log::write(format!(
        "recording rejected: missing moov atom in {}",
        output.display()
    ));
    discard_unplayable_recording(&output);
    let _ = events.send(RecorderEvent::Failed(msg!(
        "Recording was incomplete (interrupted before finalize). The broken file was discarded. Prefer VLC or Movies & TV for playback.",
        "Запись не завершена (прервана до финализации). Битый файл удалён. Для воспроизведения лучше VLC или «Кино и ТВ»."
    )));
}

struct CaptureFlags {
    config: AppConfig,
    output: PathBuf,
    /// Capture surface size (monitor pixels).
    width: u32,
    height: u32,
    /// Encoder size (may be scaled down).
    encode_width: u32,
    encode_height: u32,
    stop: Arc<AtomicBool>,
    stop_on_sleep: bool,
    events: Sender<RecorderEvent>,
    audio: Option<AudioPipeline>,
    finalization_error: Arc<Mutex<Option<String>>>,
}

struct Capture {
    encoder: SharedEncoder,
    stop: Arc<AtomicBool>,
    stop_on_sleep: bool,
    frame_interval: Duration,
    last_frame: Option<Instant>,
    audio: Option<AudioPipeline>,
    finalization_error: Arc<Mutex<Option<String>>>,
}

impl GraphicsCaptureApiHandler for Capture {
    type Flags = CaptureFlags;
    type Error = Box<dyn std::error::Error + Send + Sync>;

    fn new(ctx: Context<Self::Flags>) -> Result<Self, Self::Error> {
        debug_log::write(format!(
            "capture handler initializing encoder: encode={}x{} capture={}x{} fps={} output={}",
            ctx.flags.encode_width,
            ctx.flags.encode_height,
            ctx.flags.width,
            ctx.flags.height,
            ctx.flags.config.fps,
            ctx.flags.output.display()
        ));
        let params = EncodeParams::from_config(
            &ctx.flags.config,
            ctx.flags.encode_width,
            ctx.flags.encode_height,
            false,
            ctx.flags.output.clone(),
        );
        let encoder = encode::open_encoder(&ctx.flags.config, params)
            .map_err(|error| error.render(ctx.flags.config.language()).to_owned())?;

        let mut capture = Self {
            encoder: Arc::new(Mutex::new(Some(encoder))),
            stop: ctx.flags.stop,
            stop_on_sleep: ctx.flags.stop_on_sleep,
            frame_interval: Duration::from_secs_f64(1.0 / ctx.flags.config.fps.max(1) as f64),
            last_frame: None,
            audio: ctx.flags.audio,
            finalization_error: ctx.flags.finalization_error,
        };
        if let Some(audio) = &mut capture.audio {
            audio.begin(capture.encoder.clone())?;
        }
        let _ = ctx
            .flags
            .events
            .send(RecorderEvent::Started(ctx.flags.output));
        debug_log::write("encoder created; Started event sent");
        Ok(capture)
    }

    fn on_frame_arrived(
        &mut self,
        frame: &mut Frame,
        capture_control: InternalCaptureControl,
    ) -> Result<(), Self::Error> {
        if should_stop(&self.stop, self.stop_on_sleep) {
            debug_log::write("frame handler observed stop/suspend");
            capture_control.stop();
            return Ok(());
        }

        let now = Instant::now();
        if self
            .last_frame
            .is_some_and(|last| now.duration_since(last) < self.frame_interval)
        {
            return Ok(());
        }
        self.encoder
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .as_mut()
            .expect("encoder is available")
            .push_wgc_frame(frame)?;
        self.last_frame = Some(now);
        Ok(())
    }
}

impl Drop for Capture {
    fn drop(&mut self) {
        debug_log::write("capture handler dropped; finalizing encoder");
        if let Some(audio) = &mut self.audio
            && let Err(error) = audio.stop_and_join()
        {
            *self
                .finalization_error
                .lock()
                .unwrap_or_else(|e| e.into_inner()) = Some(error);
        }
        if let Some(encoder) = self
            .encoder
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .take()
        {
            let dropped = encoder.dropped_frames();
            if dropped > 0 {
                debug_log::write(format!("encode dropped_frames={dropped}"));
            }
            if let Err(error) = encoder.finish() {
                debug_log::write(format!("encoder finalization failed: {error}"));
                *self.finalization_error.lock().unwrap() = Some(error);
            } else {
                debug_log::write("encoder finalization completed");
            }
        } else {
            debug_log::write("encoder finalization completed");
        }
    }
}

struct GdiCapture {
    screen_dc: HDC,
    memory_dc: HDC,
    bitmap: HBITMAP,
    previous_object: HGDIOBJ,
    pixels: *mut u8,
    left: i32,
    top: i32,
    width: u32,
    height: u32,
}

impl GdiCapture {
    fn new(monitor: Monitor, width: u32, height: u32) -> Result<Self, String> {
        let mut monitor_info = MONITORINFO {
            cbSize: std::mem::size_of::<MONITORINFO>() as u32,
            ..MONITORINFO::default()
        };
        let hmonitor = HMONITOR(monitor.as_raw_hmonitor());
        if !unsafe { GetMonitorInfoW(hmonitor, &mut monitor_info) }.as_bool() {
            return Err(format!(
                "GetMonitorInfoW: {}",
                std::io::Error::last_os_error()
            ));
        }

        let screen_dc = unsafe { GetDC(None) };
        if screen_dc.0.is_null() {
            return Err(format!("GetDC: {}", std::io::Error::last_os_error()));
        }
        let memory_dc = unsafe { CreateCompatibleDC(Some(screen_dc)) };
        if memory_dc.0.is_null() {
            unsafe { ReleaseDC(None, screen_dc) };
            return Err(format!(
                "CreateCompatibleDC: {}",
                std::io::Error::last_os_error()
            ));
        }

        let bitmap_info = BITMAPINFO {
            bmiHeader: BITMAPINFOHEADER {
                biSize: std::mem::size_of::<BITMAPINFOHEADER>() as u32,
                biWidth: width as i32,
                // Positive height produces bottom-up BGRA (MF send_frame_buffer /
                // FFmpeg with -vf vflip).
                biHeight: height as i32,
                biPlanes: 1,
                biBitCount: 32,
                biCompression: BI_RGB.0,
                biSizeImage: width.saturating_mul(height).saturating_mul(4),
                ..BITMAPINFOHEADER::default()
            },
            ..BITMAPINFO::default()
        };
        let mut pixels = std::ptr::null_mut();
        let bitmap = match unsafe {
            CreateDIBSection(
                Some(screen_dc),
                &bitmap_info,
                DIB_RGB_COLORS,
                &mut pixels,
                None,
                0,
            )
        } {
            Ok(bitmap) => bitmap,
            Err(error) => {
                unsafe {
                    let _ = DeleteDC(memory_dc);
                    ReleaseDC(None, screen_dc);
                }
                return Err(format!("CreateDIBSection: {error}"));
            }
        };
        let previous_object = unsafe { SelectObject(memory_dc, HGDIOBJ(bitmap.0)) };

        Ok(Self {
            screen_dc,
            memory_dc,
            bitmap,
            previous_object,
            pixels: pixels.cast(),
            left: monitor_info.rcMonitor.left,
            top: monitor_info.rcMonitor.top,
            width,
            height,
        })
    }

    fn grab(&mut self, include_cursor: bool) -> Result<&[u8], String> {
        unsafe {
            if let Err(error) = BitBlt(
                self.memory_dc,
                0,
                0,
                self.width as i32,
                self.height as i32,
                Some(self.screen_dc),
                self.left,
                self.top,
                ROP_CODE(SRCCOPY.0 | CAPTUREBLT.0),
            ) {
                // After sleep/lock, the screen DC is often invalid (0x80070005).
                // Refresh GetDC once and retry before failing the whole recording.
                debug_log::write(format!("BitBlt failed ({error}); refreshing screen DC"));
                ReleaseDC(None, self.screen_dc);
                self.screen_dc = GetDC(None);
                if self.screen_dc.0.is_null() {
                    return Err(format!("BitBlt: {error}"));
                }
                BitBlt(
                    self.memory_dc,
                    0,
                    0,
                    self.width as i32,
                    self.height as i32,
                    Some(self.screen_dc),
                    self.left,
                    self.top,
                    ROP_CODE(SRCCOPY.0 | CAPTUREBLT.0),
                )
                .map_err(|retry| format!("BitBlt: {retry}"))?;
            }
        }

        if include_cursor {
            self.draw_cursor();
        }

        let length = self.width as usize * self.height as usize * 4;
        Ok(unsafe { std::slice::from_raw_parts(self.pixels, length) })
    }

    fn draw_cursor(&self) {
        let mut cursor = CURSORINFO {
            cbSize: std::mem::size_of::<CURSORINFO>() as u32,
            ..CURSORINFO::default()
        };
        if unsafe { GetCursorInfo(&mut cursor) }.is_err()
            || cursor.flags != CURSOR_SHOWING
            || cursor.hCursor.0.is_null()
        {
            return;
        }

        let icon_handle = HICON(cursor.hCursor.0);
        let mut icon = ICONINFO::default();
        let (hotspot_x, hotspot_y) = if unsafe { GetIconInfo(icon_handle, &mut icon) }.is_ok() {
            let hotspot = (icon.xHotspot as i32, icon.yHotspot as i32);
            unsafe {
                if !icon.hbmMask.0.is_null() {
                    let _ = DeleteObject(HGDIOBJ(icon.hbmMask.0));
                }
                if !icon.hbmColor.0.is_null() {
                    let _ = DeleteObject(HGDIOBJ(icon.hbmColor.0));
                }
            }
            hotspot
        } else {
            (0, 0)
        };

        let x = cursor.ptScreenPos.x - self.left - hotspot_x;
        let y = cursor.ptScreenPos.y - self.top - hotspot_y;
        let _ = unsafe { DrawIconEx(self.memory_dc, x, y, icon_handle, 0, 0, 0, None, DI_NORMAL) };
    }
}

impl Drop for GdiCapture {
    fn drop(&mut self) {
        unsafe {
            SelectObject(self.memory_dc, self.previous_object);
            let _ = DeleteObject(HGDIOBJ(self.bitmap.0));
            let _ = DeleteDC(self.memory_dc);
            ReleaseDC(None, self.screen_dc);
        }
    }
}

fn downsample_bgra(src: &[u8], src_w: u32, src_h: u32, dst_w: u32, dst_h: u32) -> Vec<u8> {
    if src_w == dst_w && src_h == dst_h {
        return src.to_vec();
    }
    let mut out = vec![0u8; (dst_w as usize) * (dst_h as usize) * 4];
    for y in 0..dst_h {
        let sy = (u64::from(y) * u64::from(src_h) / u64::from(dst_h)) as u32;
        for x in 0..dst_w {
            let sx = (u64::from(x) * u64::from(src_w) / u64::from(dst_w)) as u32;
            let si = ((sy * src_w + sx) * 4) as usize;
            let di = ((y * dst_w + x) * 4) as usize;
            if si + 4 <= src.len() && di + 4 <= out.len() {
                out[di..di + 4].copy_from_slice(&src[si..si + 4]);
            }
        }
    }
    out
}

fn should_stop(stop: &AtomicBool, stop_on_sleep: bool) -> bool {
    if stop.load(Ordering::Acquire) {
        return true;
    }
    if stop_on_sleep && crate::power::is_system_suspended() {
        debug_log::write("recorder observed system suspend; stopping");
        stop.store(true, Ordering::Release);
        return true;
    }
    false
}

fn run(config: AppConfig, stop: Arc<AtomicBool>, events: Sender<RecorderEvent>) {
    if config.stop_on_sleep {
        crate::power::ensure_global_watcher();
    }
    let output_dir = config.effective_output_dir();
    debug_log::write(format!(
        "recorder thread entered; monitor={}, output_dir={}, stop_on_sleep={}",
        config.monitor_index,
        output_dir.display(),
        config.stop_on_sleep
    ));
    if let Err(error) = fs::create_dir_all(&output_dir) {
        let _ = events.send(RecorderEvent::Failed(msg!(
            "Could not create the recording folder: {error}",
            "Не удалось создать папку записи: {error}"
        )));
        return;
    }

    let monitor = match Monitor::from_index(config.monitor_index) {
        Ok(monitor) => monitor,
        Err(error) => {
            let _ = events.send(RecorderEvent::Failed(msg!(
                "Monitor unavailable: {error}",
                "Монитор недоступен: {error}"
            )));
            return;
        }
    };
    let (width, height) = match (monitor.width(), monitor.height()) {
        (Ok(width), Ok(height)) => (width, height),
        (Err(error), _) | (_, Err(error)) => {
            let _ = events.send(RecorderEvent::Failed(error.to_string().into()));
            return;
        }
    };
    let (encode_width, encode_height) = config.resolution_mode.encode_size(width, height);
    let needs_scale = encode_width != width || encode_height != height;
    debug_log::write(format!(
        "capture {width}x{height} -> encode {encode_width}x{encode_height} ({:?})",
        config.resolution_mode
    ));
    let output = match reserve_output(
        &output_dir,
        &crate::filename::render_with_pc(
            &config.filename_template,
            Local::now().naive_local(),
            config.monitor_index,
            &config.effective_pc_id(),
        ),
    ) {
        Ok(reserved) => reserved,
        Err(error) => {
            let _ = events.send(RecorderEvent::Failed(msg!(
                "Could not create the recording file: {error}",
                "Не удалось создать файл записи: {error}"
            )));
            return;
        }
    };
    let capture_probe: Result<GraphicsCaptureItemType, windows::core::Error> = monitor.try_into();
    let audio = match AudioPipeline::start(&config, stop.clone(), events.clone()) {
        Ok(audio) => audio,
        Err(error) => {
            release_reserved_output(&output);
            let _ = events.send(RecorderEvent::Failed(error));
            return;
        }
    };
    let finalization_error = Arc::new(Mutex::new(None));
    let flags = CaptureFlags {
        config: config.clone(),
        output: output.clone(),
        width,
        height,
        encode_width,
        encode_height,
        stop: stop.clone(),
        stop_on_sleep: config.stop_on_sleep,
        events: events.clone(),
        audio,
        finalization_error: finalization_error.clone(),
    };

    // Scaling requires a CPU downsample path; WGC encoder only crops, so use GDI.
    if needs_scale {
        debug_log::write("using GDI path for resolution scaling");
        run_gdi(monitor, flags);
        return;
    }

    match capture_probe {
        Ok(_) => {}
        Err(error) => {
            debug_log::write(format!(
                "Windows Graphics Capture unavailable: code={:?}, error={error}; using GDI fallback; stop={}",
                error.code(),
                stop.load(Ordering::Acquire)
            ));
            let _ = events.send(RecorderEvent::AudioWarning(msg!(
                "Windows Graphics Capture is unavailable; using GDI compatibility mode.",
                "Windows Graphics Capture недоступен; используется совместимый режим GDI."
            )));
            run_gdi(monitor, flags);
            return;
        }
    }

    // Yellow WGC border: only Win11+ can disable it. If the API is missing, GDI has no border.
    let border_toggle_ok =
        GraphicsCaptureApi::is_border_settings_supported().unwrap_or(false);
    if !border_toggle_ok {
        debug_log::write(
            "WGC cannot hide yellow border on this OS; using GDI capture (no border)",
        );
        run_gdi(monitor, flags);
        return;
    }
    ensure_wgc_borderless_consent();

    let cursor_settings = if config.show_cursor {
        CursorCaptureSettings::Default
    } else if GraphicsCaptureApi::is_cursor_settings_supported().unwrap_or(false) {
        CursorCaptureSettings::WithoutCursor
    } else {
        debug_log::write("cursor capture toggle unsupported; using Default");
        CursorCaptureSettings::Default
    };

    let settings = Settings::new(
        monitor,
        cursor_settings,
        DrawBorderSettings::WithoutBorder,
        SecondaryWindowSettings::Default,
        MinimumUpdateIntervalSettings::Default,
        DirtyRegionSettings::Default,
        ColorFormat::Bgra8,
        flags,
    );

    let control = match Capture::start_free_threaded(settings) {
        Ok(control) => control,
        Err(error) => {
            debug_log::write(format!("capture startup failed: {error}"));
            stop.store(true, Ordering::Release);
            release_reserved_output(&output);
            let _ = events.send(RecorderEvent::Failed(msg!(
                "Recording failed: {error}",
                "Ошибка записи: {error}"
            )));
            return;
        }
    };
    debug_log::write("capture control created; entering stop loop");

    let deadline = (config.duration_minutes > 0)
        .then(|| Instant::now() + Duration::from_secs(u64::from(config.duration_minutes) * 60));
    let capture_result = loop {
        if control.is_finished() {
            debug_log::write("capture control finished on its own");
            break control.wait();
        }
        if should_stop(&stop, config.stop_on_sleep)
            || deadline.is_some_and(|deadline| Instant::now() >= deadline)
        {
            debug_log::write(
                "stop loop observed stop/suspend/deadline; calling CaptureControl::stop",
            );
            stop.store(true, Ordering::Release);
            break control.stop();
        }
        thread::sleep(Duration::from_millis(20));
    };

    stop.store(true, Ordering::Release);
    if let Err(error) = capture_result {
        debug_log::write(format!("CaptureControl stop/wait failed: {error}"));
        release_reserved_output(&output);
        let _ = events.send(RecorderEvent::Failed(msg!(
            "Could not stop recording: {error}",
            "Ошибка остановки записи: {error}"
        )));
        return;
    }

    let error = finalization_error.lock().unwrap().take();
    if let Some(error) = error {
        debug_log::write(format!("recording failed during finalization: {error}"));
        release_reserved_output(&output);
        let _ = events.send(RecorderEvent::Failed(msg!(
            "Could not finalize MP4: {error}",
            "Не удалось завершить MP4: {error}"
        )));
    } else {
        emit_finished_or_reject(&events, output);
    }
}

/// Allow unpackaged desktop apps to hide the yellow WGC screenshot border (Win11).
/// Writes HKCU ConsentStore — no elevation required; ignored on older Windows.
fn ensure_wgc_borderless_consent() {
    const ROOT: &str =
        r"Software\Microsoft\Windows\CurrentVersion\CapabilityAccessManager\ConsentStore";
    for sub in [
        "graphicsCaptureWithoutBorder",
        r"graphicsCaptureWithoutBorder\NonPackaged",
        "graphicsCaptureProgrammatic",
        r"graphicsCaptureProgrammatic\NonPackaged",
    ] {
        let path = format!("{ROOT}\\{sub}");
        if let Err(error) = set_hkcu_reg_sz(&path, "Value", "Allow") {
            debug_log::write(format!("borderless consent {path}: {error}"));
        }
    }
}

fn set_hkcu_reg_sz(subkey: &str, name: &str, value: &str) -> windows::core::Result<()> {
    unsafe {
        let mut key = Default::default();
        let sub = HSTRING::from(subkey);
        let value_name = HSTRING::from(name);
        let status = RegCreateKeyExW(
            HKEY_CURRENT_USER,
            PCWSTR(sub.as_ptr()),
            None,
            None,
            REG_OPTION_NON_VOLATILE,
            KEY_SET_VALUE,
            None,
            &mut key,
            None,
        );
        if status != ERROR_SUCCESS {
            return Err(windows::core::Error::from(status));
        }
        let wide: Vec<u16> = value.encode_utf16().chain(std::iter::once(0)).collect();
        let bytes = std::slice::from_raw_parts(
            wide.as_ptr() as *const u8,
            wide.len() * std::mem::size_of::<u16>(),
        );
        let set = RegSetValueExW(key, PCWSTR(value_name.as_ptr()), None, REG_SZ, Some(bytes));
        let _ = RegCloseKey(key);
        if set != ERROR_SUCCESS {
            return Err(windows::core::Error::from(set));
        }
        Ok(())
    }
}

fn run_gdi(monitor: Monitor, flags: CaptureFlags) {
    let CaptureFlags {
        config,
        output,
        width,
        height,
        encode_width,
        encode_height,
        stop,
        events,
        mut audio,
        ..
    } = flags;
    debug_log::write(format!(
        "initializing GDI capture: {width}x{height} -> encode {encode_width}x{encode_height}"
    ));
    let mut capture = match GdiCapture::new(monitor, width, height) {
        Ok(capture) => capture,
        Err(error) => {
            stop.store(true, Ordering::Release);
            debug_log::write(format!("GDI initialization failed: {error}"));
            release_reserved_output(&output);
            let _ = events.send(RecorderEvent::Failed(msg!(
                "Could not start compatible capture: {error}",
                "Не удалось запустить совместимый захват: {error}"
            )));
            return;
        }
    };

    let params =
        EncodeParams::from_config(&config, encode_width, encode_height, true, output.clone());
    let encoder = match encode::open_encoder(&config, params) {
        Ok(encoder) => encoder,
        Err(error) => {
            stop.store(true, Ordering::Release);
            debug_log::write(format!("GDI encoder initialization failed: {error:?}"));
            release_reserved_output(&output);
            let _ = events.send(RecorderEvent::Failed(error));
            return;
        }
    };

    let encoder = Arc::new(Mutex::new(Some(encoder)));
    if let Some(audio) = &mut audio
        && let Err(error) = audio.begin(encoder.clone())
    {
        let _ = audio.stop_and_join();
        if let Some(encoder) = encoder.lock().unwrap_or_else(|e| e.into_inner()).take() {
            let _ = encoder.finish();
        }
        release_reserved_output(&output);
        let _ = events.send(RecorderEvent::Failed(msg!(
            "Could not start audio encoding: {error}",
            "Не удалось запустить обработку звука: {error}"
        )));
        return;
    }
    let _ = events.send(RecorderEvent::Started(output.clone()));
    debug_log::write("GDI encoder and independent audio writer started");
    let started_at = Instant::now();
    let frame_interval = Duration::from_secs_f64(1.0 / config.fps.max(1) as f64);
    let deadline = (config.duration_minutes > 0)
        .then(|| started_at + Duration::from_secs(u64::from(config.duration_minutes) * 60));
    let mut next_frame = started_at;

    let capture_result: Result<(), String> = loop {
        if should_stop(&stop, config.stop_on_sleep)
            || deadline.is_some_and(|deadline| Instant::now() >= deadline)
        {
            debug_log::write(format!(
                "GDI loop observed stop/suspend/deadline; stop={}",
                stop.load(Ordering::Acquire)
            ));
            break Ok(());
        }

        let now = Instant::now();
        if now < next_frame {
            thread::sleep((next_frame - now).min(Duration::from_millis(5)));
            continue;
        }

        let grabbed = match capture.grab(config.show_cursor) {
            Ok(buffer) => buffer,
            Err(error) => {
                // Access-denied after sleep: stop cleanly so the MP4 can finalize
                // instead of leaving a broken file and a scary UI error.
                let access_denied = error.contains("0x80070005")
                    || error.to_ascii_lowercase().contains("отказано")
                    || error.to_ascii_lowercase().contains("access is denied")
                    || error.to_ascii_lowercase().contains("access denied");
                debug_log::write(format!("GDI frame grab failed: {error}"));
                if access_denied {
                    debug_log::write(
                        "GDI access denied after sleep/lock; ending recording gracefully",
                    );
                    break Ok(());
                }
                break Err(error);
            }
        };
        let scaled;
        let frame_pixels: &[u8] = if encode_width == width && encode_height == height {
            grabbed
        } else {
            scaled = downsample_bgra(grabbed, width, height, encode_width, encode_height);
            &scaled
        };
        let timestamp = i64::try_from(started_at.elapsed().as_nanos() / 100).unwrap_or(i64::MAX);
        if let Err(error) = encoder
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .as_mut()
            .expect("encoder is available")
            .push_bgra(frame_pixels, timestamp)
        {
            let error = format!("frame encoding failed: {error}");
            debug_log::write(&error);
            break Err(error);
        }

        next_frame += frame_interval;
        if next_frame < Instant::now() {
            next_frame = Instant::now() + frame_interval;
        }
    };

    stop.store(true, Ordering::Release);
    let audio_result = if let Some(audio) = &mut audio {
        audio.stop_and_join()
    } else {
        Ok(())
    };
    drop(audio);
    debug_log::write("finalizing GDI encoder");
    let taken = encoder
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .take()
        .expect("encoder is available");
    let dropped = taken.dropped_frames();
    if dropped > 0 {
        debug_log::write(format!("GDI encode dropped_frames={dropped}"));
    }
    let finish_result = taken.finish();

    let result = match capture_result {
        Err(error) => Err(error),
        Ok(()) => audio_result.and(finish_result),
    };
    if let Err(error) = result {
        debug_log::write(format!("GDI recording failed: {error}"));
        release_reserved_output(&output);
        let _ = events.send(RecorderEvent::Failed(msg!(
            "Recording failed: {error}",
            "Ошибка записи: {error}"
        )));
    } else {
        emit_finished_or_reject(&events, output);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn mp4_boxes(mut bytes: &[u8]) -> Vec<(&[u8], &[u8])> {
        let mut boxes = Vec::new();
        while bytes.len() >= 8 {
            let length = u32::from_be_bytes(bytes[..4].try_into().unwrap());
            let (length, header) = match length {
                0 => (bytes.len(), 8),
                1 if bytes.len() >= 16 => (
                    u64::from_be_bytes(bytes[8..16].try_into().unwrap()) as usize,
                    16,
                ),
                length => (length as usize, 8),
            };
            assert!(
                length >= header && length <= bytes.len(),
                "invalid MP4 box length"
            );
            boxes.push((&bytes[4..8], &bytes[header..length]));
            bytes = &bytes[length..];
        }
        boxes
    }

    fn track_durations(bytes: &[u8]) -> (f64, f64) {
        let movie = mp4_boxes(bytes)
            .into_iter()
            .find(|(kind, _)| *kind == b"moov")
            .expect("finalized MP4 movie atom")
            .1;
        let (mut video, mut audio) = (None, None);
        for (_, track) in mp4_boxes(movie)
            .into_iter()
            .filter(|(kind, _)| *kind == b"trak")
        {
            let media = mp4_boxes(track)
                .into_iter()
                .find(|(kind, _)| *kind == b"mdia")
                .unwrap()
                .1;
            let children = mp4_boxes(media);
            let header = children
                .iter()
                .find(|(kind, _)| *kind == b"mdhd")
                .unwrap()
                .1;
            let handler = children
                .iter()
                .find(|(kind, _)| *kind == b"hdlr")
                .unwrap()
                .1;
            let (scale, duration) = if header[0] == 1 {
                (
                    u32::from_be_bytes(header[20..24].try_into().unwrap()),
                    u64::from_be_bytes(header[24..32].try_into().unwrap()),
                )
            } else {
                (
                    u32::from_be_bytes(header[12..16].try_into().unwrap()),
                    u64::from(u32::from_be_bytes(header[16..20].try_into().unwrap())),
                )
            };
            let seconds = duration as f64 / f64::from(scale);
            match &handler[8..12] {
                b"vide" => video = Some(seconds),
                b"soun" => audio = Some(seconds),
                _ => {}
            }
        }
        (video.expect("video track"), audio.expect("audio track"))
    }

    #[test]
    fn output_reservation_never_truncates_existing_recordings() {
        let directory = tempfile::tempdir().unwrap();
        let existing = directory.path().join("same.mp4");
        fs::write(&existing, b"previous recording").unwrap();
        let first = reserve_output(directory.path(), "same").unwrap();
        let second = reserve_output(directory.path(), "same").unwrap();
        assert_ne!(first, second);
        assert_eq!(fs::read(existing).unwrap(), b"previous recording");
        assert!(first.exists());
    }

    #[test]
    fn concurrent_instances_reserve_distinct_outputs() {
        let directory = tempfile::tempdir().unwrap();
        let workers: Vec<_> = (0..8)
            .map(|_| {
                let path = directory.path().to_path_buf();
                thread::spawn(move || reserve_output(&path, "same").unwrap())
            })
            .collect();
        let reservations: Vec<_> = workers
            .into_iter()
            .map(|worker| worker.join().unwrap())
            .collect();
        let paths: std::collections::HashSet<_> = reservations.iter().collect();
        assert_eq!(paths.len(), 8);
    }

    #[test]
    fn shutdown_waits_for_worker_cleanup() {
        let stop = Arc::new(AtomicBool::new(false));
        let cleanup_complete = Arc::new(AtomicBool::new(false));
        let worker_stop = stop.clone();
        let worker_complete = cleanup_complete.clone();
        let worker = thread::spawn(move || {
            while !worker_stop.load(Ordering::Acquire) {
                thread::yield_now();
            }
            worker_complete.store(true, Ordering::Release);
        });
        let mut manager = RecorderManager {
            stop: Some(stop),
            worker: Some(worker),
            relay: None,
            session: None,
        };
        manager.shutdown();
        assert!(cleanup_complete.load(Ordering::Acquire));
        assert!(manager.worker.is_none());
    }

    #[test]
    #[ignore = "captures the live desktop for the recorder integration test"]
    fn live_capture_starts_and_stops_from_atomic_signal() {
        live_capture(true, false);
    }

    #[test]
    #[ignore = "captures the live desktop with audio disabled (no AAC track)"]
    fn live_capture_without_audio_has_valid_tracks() {
        live_capture(false, false);
    }

    #[test]
    #[ignore = "captures the live desktop and microphone with Speex enabled"]
    fn live_capture_with_microphone_noise_suppression() {
        live_capture(true, true);
    }

    fn live_capture(system_audio: bool, microphone: bool) {
        let output_dir = std::env::current_dir()
            .expect("working directory")
            .join("target")
            .join("capture-self-test");
        let monitor = Monitor::enumerate()
            .expect("monitor enumeration")
            .first()
            .copied()
            .expect("at least one monitor");
        let monitor_index = monitor.index().expect("monitor index");
        let config = AppConfig {
            monitor_index,
            system_audio,
            microphone,
            microphone_noise_suppression: microphone,
            output_dir,
            ..AppConfig::default()
        };
        let stop = Arc::new(AtomicBool::new(false));
        let mut manager = RecorderManager::default();
        let events = manager.start(config, stop, Arc::new(|| {}));

        let started = loop {
            match events
                .recv_timeout(Duration::from_secs(20))
                .expect("recorder start event")
            {
                RecorderEvent::Started(path) => break path,
                RecorderEvent::AudioWarning(message) => {
                    assert!(!microphone, "microphone capture warning: {message}");
                    assert_ne!(
                        message.render(crate::i18n::Language::En),
                        "Audio dropouts detected"
                    );
                }
                RecorderEvent::Failed(error) => panic!("recorder failed to start: {error}"),
                RecorderEvent::Finished(path) => {
                    panic!("recorder finished before stop: {}", path.display())
                }
            }
        };

        thread::sleep(Duration::from_secs(2));
        manager.stop();

        loop {
            match events
                .recv_timeout(Duration::from_secs(20))
                .expect("recorder finish event")
            {
                RecorderEvent::Finished(path) => {
                    assert_eq!(path, started);
                    assert!(path.exists(), "output file was not created");
                    assert!(fs::metadata(&path).expect("output metadata").len() > 0);
                    let (video, audio) = track_durations(&fs::read(&path).expect("recorded MP4"));
                    println!(
                        "MP4 duration: video={video:.3}s, audio={audio:.3}s, delta={:.3}s",
                        (audio - video).abs()
                    );
                    assert!(video > 1.5, "video track too short");
                    if system_audio || microphone {
                        assert!(audio > 1.5, "audio track missing or too short");
                        assert!((audio - video).abs() < 0.2, "audio/video duration mismatch");
                    } else {
                        assert!(
                            audio <= 0.01,
                            "unexpected audio track when both sources are off"
                        );
                    }
                    if std::env::var_os("HSR_KEEP_TEST_VIDEO").is_none() {
                        let _ = fs::remove_file(path);
                    } else {
                        println!("Test recording retained: {}", path.display());
                    }
                    break;
                }
                RecorderEvent::AudioWarning(message) => {
                    assert!(!microphone, "microphone capture warning: {message}");
                    assert_ne!(
                        message.render(crate::i18n::Language::En),
                        "Audio dropouts detected"
                    );
                }
                RecorderEvent::Started(_) => {}
                RecorderEvent::Failed(error) => panic!("recorder failed to stop: {error}"),
            }
        }
    }
}

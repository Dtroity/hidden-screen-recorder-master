//! FFmpeg encode backend: raw BGRA frames on stdin, optional f32le audio via named pipe.
//!
//! Playback-oriented defaults: H.264 Main / yuv420p / no B-frames / CFR timestamps,
//! write to a `.partial` file then atomically replace the reserved output path so
//! `+faststart` can finalize a clean MP4 (avoids “first frame only / scrub works” files).

use std::{
    fs,
    io::{BufRead, BufReader, Write},
    path::{Path, PathBuf},
    process::{Child, ChildStdin, Command, Stdio},
    sync::{
        Arc,
        atomic::{AtomicBool, AtomicU64, Ordering},
        mpsc::{SyncSender, sync_channel},
    },
    thread::{self, JoinHandle},
    time::Duration,
};

#[cfg(windows)]
use std::os::windows::process::CommandExt;

/// Hide the FFmpeg console window (ffmpeg.exe is a console subsystem binary).
#[cfg(windows)]
const CREATE_NO_WINDOW: u32 = 0x0800_0000;

use windows::Win32::{
    Foundation::{CloseHandle, ERROR_PIPE_CONNECTED, HANDLE, INVALID_HANDLE_VALUE},
    Storage::FileSystem::{PIPE_ACCESS_OUTBOUND, WriteFile},
    System::Pipes::{
        ConnectNamedPipe, CreateNamedPipeW, NAMED_PIPE_MODE, PIPE_READMODE_BYTE,
        PIPE_REJECT_REMOTE_CLIENTS, PIPE_TYPE_BYTE, PIPE_WAIT,
    },
};
use windows::core::PCWSTR;
use windows_capture::frame::Frame;

use crate::audio::{CHANNELS, SAMPLE_RATE};
use crate::debug_log;

use super::{EncodeParams, EncodeSink};

const VIDEO_QUEUE_SLOTS: usize = 2;

pub struct FfmpegEncoder {
    child: Child,
    video_tx: Option<SyncSender<Vec<u8>>>,
    video_writer: Option<JoinHandle<Result<(), String>>>,
    audio_tx: Option<SyncSender<Vec<u8>>>,
    audio_writer: Option<JoinHandle<Result<(), String>>>,
    stderr_thread: Option<JoinHandle<()>>,
    frame_bytes: usize,
    dropped_frames: Arc<AtomicU64>,
    failed: Arc<AtomicBool>,
    /// Temporary encode target (`*.mp4.partial`).
    partial: PathBuf,
    /// Final reserved path (`*.mp4`).
    output: PathBuf,
    frames_written: Arc<AtomicU64>,
}

pub fn resolve_ffmpeg_path(configured: Option<&Path>) -> Result<PathBuf, String> {
    if let Some(path) = configured {
        if path.is_file() {
            return Ok(path.to_path_buf());
        }
        return Err(format!(
            "Configured FFmpeg path not found: {}",
            path.display()
        ));
    }
    if let Ok(exe) = std::env::current_exe()
        && let Some(dir) = exe.parent()
    {
        let candidate = dir.join("ffmpeg.exe");
        if candidate.is_file() {
            return Ok(candidate);
        }
        let nested = dir.join("ffmpeg").join("ffmpeg.exe");
        if nested.is_file() {
            return Ok(nested);
        }
    }
    // Only accept PATH fallback if ffmpeg actually runs — avoids empty reserved
    // .mp4 files when the backend is selected but the binary is missing.
    if ffmpeg_on_path_works() {
        return Ok(PathBuf::from("ffmpeg"));
    }
    Err(
        "FFmpeg not found. Place a GPL build with libx265 next to the app \
         (ffmpeg.exe or ffmpeg\\ffmpeg.exe), or set ffmpeg_path in settings."
            .into(),
    )
}

fn ffmpeg_on_path_works() -> bool {
    let mut cmd = Command::new("ffmpeg");
    cmd.args(["-hide_banner", "-version"])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    #[cfg(windows)]
    cmd.creation_flags(CREATE_NO_WINDOW);
    cmd.status().map(|s| s.success()).unwrap_or(false)
}

fn partial_path(output: &Path) -> PathBuf {
    // FFmpeg 9.x refuses ".mp4.partial" (unknown muxer). Keep a real ".mp4" suffix.
    let stem = output
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("recording");
    output.with_file_name(format!("{stem}.partial.mp4"))
}

impl FfmpegEncoder {
    pub fn open(params: &EncodeParams) -> Result<Self, String> {
        let ffmpeg = resolve_ffmpeg_path(params.ffmpeg_path.as_deref())?;
        let frame_bytes = params.frame_bytes();
        if frame_bytes == 0 {
            return Err("Invalid encode frame size".into());
        }

        let bitrate_k = (params.bitrate_bps / 1_000).max(50);
        let (maxrate_k, bufsize_k) = match bitrate_k {
            k if k <= 350 => (k, k.saturating_mul(2)),
            k if k <= 900 => {
                let max = ((k as u64 * 123) / 100).min(800) as u32;
                (max, max.saturating_mul(2))
            }
            k => {
                let max = ((k as u64 * 125) / 100) as u32;
                (max, max.saturating_mul(2))
            }
        };

        let audio_pipe_name = if params.audio_enabled {
            Some(format!(
                r"\\.\pipe\hsr-audio-{}-{}",
                std::process::id(),
                std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .map(|d| d.as_millis())
                    .unwrap_or(0)
            ))
        } else {
            None
        };

        let audio_server = if let Some(name) = &audio_pipe_name {
            Some(create_audio_pipe(name)?)
        } else {
            None
        };

        let partial = partial_path(&params.output);
        if partial.exists() {
            let _ = fs::remove_file(&partial);
        }

        let vf = if params.bottom_up {
            "vflip,format=yuv420p"
        } else {
            "format=yuv420p"
        };
        let gop = (params.fps.saturating_mul(2)).max(1);

        // Input: -framerate sets the rawvideo timebase so each stdin frame advances CFR.
        // Output: HEVC (libx265) + CFR; hvc1 tag for Windows/macOS players.
        let mut args: Vec<String> = vec![
            "-y".into(),
            "-hide_banner".into(),
            "-loglevel".into(),
            "error".into(),
            "-f".into(),
            "rawvideo".into(),
            "-pix_fmt".into(),
            "bgra".into(),
            "-video_size".into(),
            format!("{}x{}", params.width, params.height),
            "-framerate".into(),
            params.fps.to_string(),
            "-i".into(),
            "pipe:0".into(),
        ];
        if let Some(name) = &audio_pipe_name {
            args.extend([
                "-f".into(),
                "f32le".into(),
                "-ar".into(),
                SAMPLE_RATE.to_string(),
                "-ac".into(),
                CHANNELS.to_string(),
                "-i".into(),
                name.clone(),
            ]);
        }
        let x265_params = format!(
            "log-level=error:keyint={gop}:min-keyint={gop}:scenecut=0:repeat-headers=1"
        );
        args.extend([
            "-vf".into(),
            vf.into(),
            "-c:v".into(),
            "libx265".into(),
            "-tag:v".into(),
            "hvc1".into(),
            "-pix_fmt".into(),
            "yuv420p".into(),
            "-preset".into(),
            "veryfast".into(),
            "-b:v".into(),
            format!("{bitrate_k}k"),
            "-maxrate".into(),
            format!("{maxrate_k}k"),
            "-bufsize".into(),
            format!("{bufsize_k}k"),
            "-x265-params".into(),
            x265_params,
            "-r".into(),
            params.fps.to_string(),
            "-fps_mode".into(),
            "cfr".into(),
        ]);
        if params.audio_enabled {
            args.extend([
                "-c:a".into(),
                "aac".into(),
                "-b:a".into(),
                "96k".into(),
                "-ar".into(),
                SAMPLE_RATE.to_string(),
                "-ac".into(),
                CHANNELS.to_string(),
            ]);
        } else {
            args.push("-an".into());
        }
        args.extend([
            "-movflags".into(),
            "+faststart".into(),
            "-write_tmcd".into(),
            "0".into(),
            // Explicit muxer: required when the temp name is not a plain ".mp4".
            "-f".into(),
            "mp4".into(),
            partial.to_string_lossy().into_owned(),
        ]);

        debug_log::write(format!(
            "FFmpeg cmdline: {} {}",
            ffmpeg.display(),
            args.join(" ")
        ));

        let mut cmd = Command::new(&ffmpeg);
        cmd.args(&args)
            .stdin(Stdio::piped())
            .stdout(Stdio::null())
            .stderr(Stdio::piped());
        #[cfg(windows)]
        cmd.creation_flags(CREATE_NO_WINDOW);
        let mut child = cmd.spawn().map_err(|error| {
            format!(
                "Failed to start FFmpeg ({}): {error}. Place ffmpeg.exe next to the app or set ffmpeg_path in settings.",
                ffmpeg.display()
            )
        })?;

        let stderr_thread = child.stderr.take().map(|stderr| {
            thread::spawn(move || {
                let reader = BufReader::new(stderr);
                for line in reader.lines().map_while(Result::ok) {
                    if !line.trim().is_empty() {
                        debug_log::write(format!("ffmpeg: {line}"));
                    }
                }
            })
        });

        let stdin = child
            .stdin
            .take()
            .ok_or_else(|| "FFmpeg stdin unavailable".to_owned())?;

        let failed = Arc::new(AtomicBool::new(false));
        let dropped_frames = Arc::new(AtomicU64::new(0));
        let frames_written = Arc::new(AtomicU64::new(0));

        let (video_tx, video_rx) = sync_channel::<Vec<u8>>(VIDEO_QUEUE_SLOTS);
        let failed_v = failed.clone();
        let frames_v = frames_written.clone();
        let video_writer = thread::Builder::new()
            .name("ffmpeg-video-writer".into())
            .spawn(move || video_write_loop(stdin, video_rx, failed_v, frames_v))
            .map_err(|error| error.to_string())?;

        let (audio_tx, audio_writer) = if let Some(server) = audio_server {
            let (tx, rx) = sync_channel::<Vec<u8>>(VIDEO_QUEUE_SLOTS);
            let failed_a = failed.clone();
            let handle = thread::Builder::new()
                .name("ffmpeg-audio-writer".into())
                .spawn(move || audio_write_loop(server, rx, failed_a))
                .map_err(|error| error.to_string())?;
            thread::sleep(Duration::from_millis(80));
            (Some(tx), Some(handle))
        } else {
            (None, None)
        };

        Ok(Self {
            child,
            video_tx: Some(video_tx),
            video_writer: Some(video_writer),
            audio_tx,
            audio_writer,
            stderr_thread,
            frame_bytes,
            dropped_frames,
            failed,
            partial,
            output: params.output.clone(),
            frames_written,
        })
    }

    fn ensure_alive(&self) -> Result<(), String> {
        if self.failed.load(Ordering::Acquire) {
            return Err("FFmpeg encoder failed (see log)".into());
        }
        Ok(())
    }
}

impl EncodeSink for FfmpegEncoder {
    fn push_bgra(&mut self, pixels: &[u8], _pts_100ns: i64) -> Result<(), String> {
        self.ensure_alive()?;
        if pixels.len() != self.frame_bytes {
            return Err(format!(
                "Frame size mismatch: got {} bytes, expected {}",
                pixels.len(),
                self.frame_bytes
            ));
        }
        let Some(tx) = &self.video_tx else {
            return Err("FFmpeg video channel closed".into());
        };
        match tx.try_send(pixels.to_vec()) {
            Ok(()) => Ok(()),
            Err(std::sync::mpsc::TrySendError::Full(_)) => {
                // Drop newest under backpressure. CFR timeline follows frames actually written
                // (shorter file under overload) — avoids duplicate PTS / frozen playback.
                self.dropped_frames.fetch_add(1, Ordering::Relaxed);
                debug_log::write(format!(
                    "FFmpeg video queue full; dropped frame (total={})",
                    self.dropped_frames.load(Ordering::Relaxed)
                ));
                Ok(())
            }
            Err(std::sync::mpsc::TrySendError::Disconnected(_)) => {
                self.failed.store(true, Ordering::Release);
                Err("FFmpeg video writer disconnected".into())
            }
        }
    }

    fn push_wgc_frame(&mut self, frame: &mut Frame) -> Result<(), String> {
        let mut scratch = Vec::new();
        let buffer = frame
            .buffer()
            .map_err(|error| format!("WGC frame buffer: {error}"))?;
        let pixels = buffer.as_nopadding_buffer(&mut scratch);
        self.push_bgra(pixels, 0)
    }

    fn push_audio(&mut self, pcm_f32le: &[u8]) -> Result<(), String> {
        self.ensure_alive()?;
        let Some(tx) = &self.audio_tx else {
            return Ok(());
        };
        match tx.try_send(pcm_f32le.to_vec()) {
            Ok(()) => Ok(()),
            Err(std::sync::mpsc::TrySendError::Full(_)) => {
                debug_log::write("FFmpeg audio queue full; dropped audio chunk");
                Ok(())
            }
            Err(std::sync::mpsc::TrySendError::Disconnected(_)) => {
                self.failed.store(true, Ordering::Release);
                Err("FFmpeg audio writer disconnected".into())
            }
        }
    }

    fn finish(mut self: Box<Self>) -> Result<(), String> {
        let dropped = self.dropped_frames.load(Ordering::Relaxed);
        if dropped > 0 {
            debug_log::write(format!("FFmpeg session dropped_frames={dropped}"));
        }
        self.video_tx.take();
        self.audio_tx.take();
        if let Some(handle) = self.video_writer.take() {
            match handle.join() {
                Ok(Ok(())) => {}
                Ok(Err(error)) => {
                    let _ = self.child.kill();
                    let _ = self.child.wait();
                    let _ = fs::remove_file(&self.partial);
                    return Err(error);
                }
                Err(_) => {
                    let _ = self.child.kill();
                    let _ = self.child.wait();
                    let _ = fs::remove_file(&self.partial);
                    return Err("FFmpeg video writer crashed".into());
                }
            }
        }
        if let Some(handle) = self.audio_writer.take() {
            match handle.join() {
                Ok(Ok(())) => {}
                Ok(Err(error)) => {
                    let _ = self.child.kill();
                    let _ = self.child.wait();
                    let _ = fs::remove_file(&self.partial);
                    return Err(error);
                }
                Err(_) => {
                    let _ = self.child.kill();
                    let _ = self.child.wait();
                    let _ = fs::remove_file(&self.partial);
                    return Err("FFmpeg audio writer crashed".into());
                }
            }
        }
        let status = self
            .child
            .wait()
            .map_err(|error| format!("Waiting for FFmpeg failed: {error}"))?;
        if let Some(handle) = self.stderr_thread.take() {
            let _ = handle.join();
        }
        let frames = self.frames_written.load(Ordering::Relaxed);
        debug_log::write(format!(
            "FFmpeg finished status={status} frames_written={frames} partial={}",
            self.partial.display()
        ));
        if !status.success() {
            let _ = fs::remove_file(&self.partial);
            return Err(format!(
                "FFmpeg exited with {status}; output may be incomplete ({})",
                self.partial.display()
            ));
        }
        if frames == 0 {
            let _ = fs::remove_file(&self.partial);
            return Err("FFmpeg finished but no video frames were written".into());
        }
        if !self.partial.is_file() {
            return Err("FFmpeg finished but partial output file is missing".into());
        }
        let meta = fs::metadata(&self.partial).map_err(|e| e.to_string())?;
        if meta.len() < 32 {
            let _ = fs::remove_file(&self.partial);
            return Err("FFmpeg output file is too small to be a valid MP4".into());
        }
        // Replace the empty reserved path with the finalized MP4 (moov rewritten by faststart).
        if self.output.exists() {
            fs::remove_file(&self.output).map_err(|error| {
                format!(
                    "Could not replace reserved output {}: {error}",
                    self.output.display()
                )
            })?;
        }
        fs::rename(&self.partial, &self.output).map_err(|error| {
            format!(
                "Could not move {} -> {}: {error}",
                self.partial.display(),
                self.output.display()
            )
        })?;
        Ok(())
    }

    fn dropped_frames(&self) -> u64 {
        self.dropped_frames.load(Ordering::Relaxed)
    }
}

fn video_write_loop(
    mut stdin: ChildStdin,
    rx: std::sync::mpsc::Receiver<Vec<u8>>,
    failed: Arc<AtomicBool>,
    frames_written: Arc<AtomicU64>,
) -> Result<(), String> {
    while let Ok(frame) = rx.recv() {
        if let Err(error) = stdin.write_all(&frame) {
            failed.store(true, Ordering::Release);
            return Err(format!("FFmpeg stdin write failed: {error}"));
        }
        frames_written.fetch_add(1, Ordering::Relaxed);
    }
    if let Err(error) = stdin.flush() {
        failed.store(true, Ordering::Release);
        return Err(format!("FFmpeg stdin flush failed: {error}"));
    }
    // Closing stdin signals EOF so FFmpeg can finalize moov / faststart.
    drop(stdin);
    Ok(())
}

struct AudioPipe {
    handle: HANDLE,
}

unsafe impl Send for AudioPipe {}

impl Drop for AudioPipe {
    fn drop(&mut self) {
        if !self.handle.is_invalid() {
            let _ = unsafe { CloseHandle(self.handle) };
            self.handle = INVALID_HANDLE_VALUE;
        }
    }
}

fn create_audio_pipe(name: &str) -> Result<AudioPipe, String> {
    let wide: Vec<u16> = name.encode_utf16().chain(std::iter::once(0)).collect();
    let handle = unsafe {
        CreateNamedPipeW(
            PCWSTR(wide.as_ptr()),
            PIPE_ACCESS_OUTBOUND,
            NAMED_PIPE_MODE(
                PIPE_TYPE_BYTE.0
                    | PIPE_READMODE_BYTE.0
                    | PIPE_WAIT.0
                    | PIPE_REJECT_REMOTE_CLIENTS.0,
            ),
            1,
            256 * 1024,
            256 * 1024,
            0,
            None,
        )
    };
    if handle.is_invalid() {
        return Err(format!(
            "CreateNamedPipeW({name}) failed: {}",
            std::io::Error::last_os_error()
        ));
    }
    Ok(AudioPipe { handle })
}

fn audio_write_loop(
    pipe: AudioPipe,
    rx: std::sync::mpsc::Receiver<Vec<u8>>,
    failed: Arc<AtomicBool>,
) -> Result<(), String> {
    match unsafe { ConnectNamedPipe(pipe.handle, None) } {
        Ok(()) => {}
        Err(error) if error.code() == ERROR_PIPE_CONNECTED.to_hresult() => {}
        Err(error) => {
            failed.store(true, Ordering::Release);
            return Err(format!("ConnectNamedPipe: {error}"));
        }
    }

    while let Ok(chunk) = rx.recv() {
        let mut written = 0u32;
        let result = unsafe {
            WriteFile(
                pipe.handle,
                Some(chunk.as_slice()),
                Some(&mut written),
                None,
            )
        };
        if result.is_err() || written as usize != chunk.len() {
            failed.store(true, Ordering::Release);
            return Err(format!(
                "FFmpeg audio pipe write failed (written={written}/{})",
                chunk.len()
            ));
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resolve_ffmpeg_without_binary_is_err_or_path() {
        // CI may or may not have ffmpeg on PATH; both outcomes are valid.
        match resolve_ffmpeg_path(None) {
            Ok(path) => assert!(
                path.ends_with("ffmpeg") || path.ends_with("ffmpeg.exe"),
                "{}",
                path.display()
            ),
            Err(msg) => assert!(msg.contains("FFmpeg not found"), "{msg}"),
        }
    }

    #[test]
    fn partial_path_keeps_mp4_extension() {
        let p = partial_path(Path::new(r"C:\rec\clip.mp4"));
        assert_eq!(p.file_name().and_then(|s| s.to_str()), Some("clip.partial.mp4"));
        assert_eq!(p.extension().and_then(|s| s.to_str()), Some("mp4"));
    }
}

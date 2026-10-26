//! Pluggable video encode backends (Media Foundation default, FFmpeg optional).

mod ffmpeg;
mod media_foundation;

use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use windows_capture::frame::Frame;

use crate::config::{AppConfig, EncodeBackendKind};
use crate::debug_log;
use crate::{i18n::Message, msg};

pub use ffmpeg::{FfmpegEncoder, resolve_ffmpeg_path};
pub use media_foundation::MediaFoundationEncoder;

/// Shared encoder handle used by capture + audio writer threads.
pub type SharedEncoder = Arc<Mutex<Option<Box<dyn EncodeSink>>>>;

/// Parameters passed when opening an encode session.
#[derive(Debug, Clone)]
pub struct EncodeParams {
    pub width: u32,
    pub height: u32,
    pub fps: u32,
    pub bitrate_bps: u32,
    pub audio_enabled: bool,
    /// GDI BitBlt buffers are bottom-up; WGC staging buffers are top-down.
    pub bottom_up: bool,
    pub output: PathBuf,
    pub ffmpeg_path: Option<PathBuf>,
}

impl EncodeParams {
    pub fn from_config(
        config: &AppConfig,
        width: u32,
        height: u32,
        bottom_up: bool,
        output: PathBuf,
    ) -> Self {
        Self {
            width,
            height,
            fps: config.fps.max(1),
            bitrate_bps: config.video_bitrate_bps(),
            audio_enabled: config.audio_enabled(),
            bottom_up,
            output,
            ffmpeg_path: config.ffmpeg_path.clone(),
        }
    }

    pub fn frame_bytes(&self) -> usize {
        (self.width as usize)
            .saturating_mul(self.height as usize)
            .saturating_mul(4)
    }
}

/// Minimal encode sink used by WGC/GDI capture paths.
pub trait EncodeSink: Send {
    fn push_bgra(&mut self, pixels: &[u8], pts_100ns: i64) -> Result<(), String>;
    fn push_wgc_frame(&mut self, frame: &mut Frame) -> Result<(), String>;
    fn push_audio(&mut self, pcm_f32le: &[u8]) -> Result<(), String>;
    fn finish(self: Box<Self>) -> Result<(), String>;
    fn dropped_frames(&self) -> u64 {
        0
    }
}

/// Open the configured backend. Default remains Media Foundation.
pub fn open_encoder(
    config: &AppConfig,
    params: EncodeParams,
) -> Result<Box<dyn EncodeSink>, Message> {
    debug_log::write(format!(
        "opening encode backend={:?} {}x{} fps={} bitrate_bps={} audio={} bottom_up={} out={}",
        config.encode_backend,
        params.width,
        params.height,
        params.fps,
        params.bitrate_bps,
        params.audio_enabled,
        params.bottom_up,
        params.output.display()
    ));
    match config.encode_backend {
        EncodeBackendKind::MediaFoundation => MediaFoundationEncoder::open(&params)
            .map(|encoder| Box::new(encoder) as Box<dyn EncodeSink>)
            .map_err(|error| {
                msg!(
                    "Could not start the encoder: {error}",
                    "Не удалось запустить кодировщик: {error}"
                )
            }),
        EncodeBackendKind::Ffmpeg => FfmpegEncoder::open(&params)
            .map(|encoder| Box::new(encoder) as Box<dyn EncodeSink>)
            .map_err(|error| {
                msg!(
                    "Could not start FFmpeg encoder: {error}",
                    "Не удалось запустить кодировщик FFmpeg: {error}"
                )
            }),
    }
}

//! Media Foundation backend via `windows-capture` (unchanged default behaviour).

use windows_capture::{
    encoder::{
        AudioSettingsBuilder, ContainerSettingsBuilder, VideoEncoder, VideoSettingsBuilder,
        VideoSettingsSubType,
    },
    frame::Frame,
};

use crate::audio::{CHANNELS, SAMPLE_RATE};
use crate::debug_log;

use super::{EncodeParams, EncodeSink};

pub struct MediaFoundationEncoder {
    inner: VideoEncoder,
    audio_enabled: bool,
}

impl MediaFoundationEncoder {
    pub fn open(params: &EncodeParams) -> Result<Self, String> {
        // Rate control: only SetBitrate is exposed by windows-capture 2.0.1.
        // HardwareAccelerationEnabled is hard-coded true inside the crate (no public toggle).
        debug_log::write(format!(
            "MF encoder: {}x{} fps={} bitrate_bps={} audio={}",
            params.width, params.height, params.fps, params.bitrate_bps, params.audio_enabled
        ));
        let encoder = VideoEncoder::new(
            VideoSettingsBuilder::new(params.width, params.height)
                .sub_type(VideoSettingsSubType::H264)
                .bitrate(params.bitrate_bps)
                .frame_rate(params.fps),
            AudioSettingsBuilder::new()
                .disabled(!params.audio_enabled)
                .sample_rate(SAMPLE_RATE)
                .channel_count(CHANNELS)
                .bit_per_sample(16)
                .bitrate(96_000),
            ContainerSettingsBuilder::new(),
            &params.output,
        )
        .map_err(|error| error.to_string())?;
        Ok(Self {
            inner: encoder,
            audio_enabled: params.audio_enabled,
        })
    }
}

impl EncodeSink for MediaFoundationEncoder {
    fn push_bgra(&mut self, pixels: &[u8], pts_100ns: i64) -> Result<(), String> {
        self.inner
            .send_frame_buffer(pixels, pts_100ns)
            .map_err(|error| error.to_string())
    }

    fn push_wgc_frame(&mut self, frame: &mut Frame) -> Result<(), String> {
        self.inner
            .send_frame(frame)
            .map_err(|error| error.to_string())
    }

    fn push_audio(&mut self, pcm_f32le: &[u8]) -> Result<(), String> {
        if !self.audio_enabled {
            return Ok(());
        }
        self.inner
            .send_audio_buffer(pcm_f32le, 0)
            .map_err(|error| error.to_string())
    }

    fn finish(self: Box<Self>) -> Result<(), String> {
        self.inner.finish().map_err(|error| error.to_string())
    }
}

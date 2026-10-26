//! Shared library for the admin console and the visible recorder agent.

pub mod agent;
pub mod app;
pub mod archive_upload;
pub mod audio;
pub mod autostart;
pub mod config;
pub mod debug_log;
pub mod encode;
pub mod filename;
pub mod hotkeys;
pub mod i18n;
pub mod identity;
pub mod power;
pub mod recorder;
pub mod schedule;
pub mod session;
pub mod shutdown;
pub mod speex;
pub mod upload;
pub mod window;

pub use app::{HiddenScreenRecorderApp, MIN_WINDOW_SIZE, WINDOW_SIZE, WINDOW_TITLE};

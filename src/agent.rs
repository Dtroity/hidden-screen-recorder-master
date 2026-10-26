//! Headless office agent: records on schedule / wake, stops on sleep, uploads results.
//! No tray icon (users stop it via Stop-Agent.bat).

use std::path::PathBuf;
use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
    mpsc,
};
use std::thread;
use std::time::{Duration, Instant};

use windows::Win32::UI::WindowsAndMessaging::{
    DispatchMessageW, MSG, PM_REMOVE, PeekMessageW, TranslateMessage, WM_QUIT,
};

use crate::config::AppConfig;
use crate::debug_log;
use crate::power::{PowerState, SleepJumpDetector};
use crate::recorder::{RecorderEvent, RecorderManager};
use crate::schedule;
use crate::upload;
use crate::identity;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum AgentPhase {
    Idle,
    Recording,
    Stopping,
}

pub fn run() -> Result<(), String> {
    // Stop-Agent.bat sets this so Run/autostart/watchdog cannot revive the agent.
    if admin_stopped() {
        return Err(
            "Agent was stopped by admin (agent.admin-stopped). Use Start-Agent.bat to run again."
                .into(),
        );
    }

    let (mut config, warning) = AppConfig::load().unwrap_or_else(|error| {
        eprintln!("{}", error.render(crate::i18n::Language::system()));
        (AppConfig::default(), Some(error))
    });
    debug_log::set_enabled(config.logging_enabled);
    debug_log::reset();
    debug_log::write(format!(
        "recorder-agent started (no tray); pc={}",
        config.effective_pc_id()
    ));
    if let Some(warning) = warning {
        debug_log::write(warning.to_string());
    }

    if config.pc_id.as_deref().unwrap_or("").trim().is_empty() {
        config.pc_id = Some(identity::hostname());
        let _ = config.save();
    }

    let _agent_instance =
        crate::session::try_claim_agent_instance().map_err(|error| error.to_string())?;

    clear_stop_request();

    let power = PowerState::new();
    let _power_thread = crate::power::spawn_watcher(power.clone());

    // Retry anything left in the persistent queue / local folder (server was offline, sleep, etc.).
    if let Err(error) = crate::archive_upload::recover_pending_and_flush(&config) {
        debug_log::write(error.to_string());
    }

    let mut recorder = RecorderManager::default();
    let mut recorder_events = None::<mpsc::Receiver<RecorderEvent>>;
    let mut stop_signal = Arc::new(AtomicBool::new(false));
    let mut phase = AgentPhase::Idle;
    let mut manual_override: Option<bool> = None;
    let mut blocked_until_wake_clear = false;
    let mut was_suspended = power.is_suspended();
    let mut wake_cooldown_until: Option<Instant> = None;
    let mut sleep_jumps = SleepJumpDetector::new();
    let mut last_schedule_check = Instant::now()
        .checked_sub(Duration::from_secs(60))
        .unwrap_or_else(Instant::now);
    let mut last_config_reload = Instant::now();
    let mut last_archive_flush = Instant::now();
    let mut last_archive_recover = Instant::now();
    let mut failure_cooldown_until: Option<Instant> = None;
    let mut shutdown_requested = false;

    loop {
        pump_windows_messages();

        if stop_request_pending() {
            if !shutdown_requested {
                debug_log::write("agent: stop requested via Stop-Agent.bat");
                shutdown_requested = true;
            }
            if matches!(phase, AgentPhase::Recording) {
                request_stop(&mut recorder, &mut phase, &stop_signal);
            }
            if matches!(phase, AgentPhase::Idle) {
                break;
            }
        }

        if sleep_jumps.poll_wake() {
            debug_log::write("power: sleep/wake gap detected via clock jump");
            if config.stop_on_sleep && matches!(phase, AgentPhase::Recording) {
                request_stop(&mut recorder, &mut phase, &stop_signal);
            }
            if !config.record_on_wake {
                blocked_until_wake_clear = true;
            }
            wake_cooldown_until = Some(Instant::now() + Duration::from_secs(5));
            // After sleep: push any finished files that waited while the network was down.
            if let Err(error) = crate::archive_upload::recover_pending_and_flush(&config) {
                debug_log::write(error.to_string());
            }
            last_archive_flush = Instant::now();
            last_archive_recover = Instant::now();
        }

        let suspended = power.is_suspended();
        if suspended && !was_suspended {
            if config.stop_on_sleep && matches!(phase, AgentPhase::Recording) {
                request_stop(&mut recorder, &mut phase, &stop_signal);
            }
            if !config.record_on_wake {
                blocked_until_wake_clear = true;
            }
        }
        if power.take_resume_edge() || (!suspended && was_suspended) {
            if config.record_on_wake {
                blocked_until_wake_clear = false;
            }
            wake_cooldown_until = Some(Instant::now() + Duration::from_secs(5));
            debug_log::write("power: resume edge; applying 5s recording cooldown");
            if let Err(error) = crate::archive_upload::recover_pending_and_flush(&config) {
                debug_log::write(error.to_string());
            }
            last_archive_flush = Instant::now();
            last_archive_recover = Instant::now();
        }
        if !schedule::recording_allowed(&config.schedule) {
            blocked_until_wake_clear = false;
        }
        was_suspended = suspended;
        let in_wake_cooldown = wake_cooldown_until.is_some_and(|until| Instant::now() < until);
        if !in_wake_cooldown {
            wake_cooldown_until = None;
        }

        if last_config_reload.elapsed() > Duration::from_secs(5) {
            if let Ok((loaded, _)) = AppConfig::load() {
                config = loaded;
            }
            last_config_reload = Instant::now();
        }
        if last_archive_flush.elapsed() > Duration::from_secs(30) {
            if let Err(error) = crate::archive_upload::flush_queue(&config) {
                debug_log::write(error.to_string());
            }
            last_archive_flush = Instant::now();
        }
        // Periodic folder scan: catch files finished while the agent was stopping/offline.
        if last_archive_recover.elapsed() > Duration::from_secs(120) {
            if let Err(error) = crate::archive_upload::recover_pending_and_flush(&config) {
                debug_log::write(error.to_string());
            }
            last_archive_recover = Instant::now();
            last_archive_flush = Instant::now();
        }

        if let Some(receiver) = &recorder_events {
            let events: Vec<_> = receiver.try_iter().collect();
            for event in events {
                handle_recorder_event(
                    event,
                    &config,
                    &mut recorder,
                    &mut recorder_events,
                    &mut phase,
                    &mut failure_cooldown_until,
                );
            }
        }

        if shutdown_requested {
            if matches!(phase, AgentPhase::Idle) {
                break;
            }
            thread::sleep(Duration::from_millis(200));
            continue;
        }

        if last_schedule_check.elapsed() >= Duration::from_secs(2) {
            last_schedule_check = Instant::now();
            let in_failure_cooldown = failure_cooldown_until.is_some_and(|until| Instant::now() < until);
            if !in_failure_cooldown {
                failure_cooldown_until = None;
            }
            let want = desired_recording(
                &config,
                &power,
                manual_override,
                blocked_until_wake_clear,
                in_wake_cooldown,
            ) && !in_failure_cooldown;
            match (want, phase) {
                (true, AgentPhase::Idle) => {
                    start_recording(
                        &config,
                        &mut recorder,
                        &mut recorder_events,
                        &mut stop_signal,
                        &mut phase,
                    );
                }
                (false, AgentPhase::Recording) => {
                    request_stop(&mut recorder, &mut phase, &stop_signal);
                }
                _ => {}
            }
            if manual_override == Some(false) && !schedule::recording_allowed(&config.schedule) {
                manual_override = None;
            }
            if manual_override == Some(true) && schedule::recording_allowed(&config.schedule) {
                manual_override = None;
            }
        }

        thread::sleep(Duration::from_millis(200));
    }

    if matches!(phase, AgentPhase::Recording | AgentPhase::Stopping) {
        stop_signal.store(true, Ordering::Release);
        recorder.stop();
        thread::sleep(Duration::from_millis(800));
    }
    clear_stop_request();
    debug_log::write("recorder-agent stopped");
    Ok(())
}

fn stop_flag_path() -> Option<PathBuf> {
    std::env::current_exe()
        .ok()
        .and_then(|p| p.parent().map(|d| d.join("agent.stop")))
}

fn admin_stopped_flag_path() -> Option<PathBuf> {
    std::env::current_exe()
        .ok()
        .and_then(|p| p.parent().map(|d| d.join("agent.admin-stopped")))
}

fn admin_stopped() -> bool {
    admin_stopped_flag_path().is_some_and(|p| p.is_file())
}

fn stop_request_pending() -> bool {
    stop_flag_path().is_some_and(|p| p.is_file())
}

fn clear_stop_request() {
    if let Some(path) = stop_flag_path() {
        let _ = std::fs::remove_file(path);
    }
}

fn desired_recording(
    config: &AppConfig,
    power: &PowerState,
    manual_override: Option<bool>,
    blocked_until_wake_clear: bool,
    in_wake_cooldown: bool,
) -> bool {
    if power.is_suspended() || in_wake_cooldown {
        return false;
    }
    if let Some(manual) = manual_override {
        return manual;
    }
    if !config.agent_auto_record {
        return false;
    }
    if blocked_until_wake_clear {
        return false;
    }
    schedule::recording_allowed(&config.schedule)
}

fn start_recording(
    config: &AppConfig,
    recorder: &mut RecorderManager,
    recorder_events: &mut Option<mpsc::Receiver<RecorderEvent>>,
    stop_signal: &mut Arc<AtomicBool>,
    phase: &mut AgentPhase,
) {
    if !matches!(phase, AgentPhase::Idle) {
        return;
    }
    debug_log::write("agent: starting recording");
    *stop_signal = Arc::new(AtomicBool::new(false));
    let wake = Arc::new(|| {});
    *recorder_events = Some(recorder.start(config.clone(), stop_signal.clone(), wake));
    *phase = AgentPhase::Recording;
}

fn request_stop(
    recorder: &mut RecorderManager,
    phase: &mut AgentPhase,
    stop_signal: &Arc<AtomicBool>,
) {
    if matches!(phase, AgentPhase::Recording) {
        debug_log::write("agent: stopping recording");
        stop_signal.store(true, Ordering::Release);
        recorder.stop();
        *phase = AgentPhase::Stopping;
    }
}

fn handle_recorder_event(
    event: RecorderEvent,
    config: &AppConfig,
    recorder: &mut RecorderManager,
    recorder_events: &mut Option<mpsc::Receiver<RecorderEvent>>,
    phase: &mut AgentPhase,
    failure_cooldown_until: &mut Option<Instant>,
) {
    match event {
        RecorderEvent::Started(path) => {
            debug_log::write(format!("agent: recording started -> {}", path.display()));
            *failure_cooldown_until = None;
            *phase = AgentPhase::Recording;
        }
        RecorderEvent::AudioWarning(message) => {
            debug_log::write(message.to_string());
        }
        RecorderEvent::Finished(path) => {
            debug_log::write(format!("agent: recording finished -> {}", path.display()));
            *failure_cooldown_until = None;
            match upload::upload_recording(config, &path) {
                Ok(Some(result)) => debug_log::write(format!(
                    "agent: uploaded to {} (deleted_local={})",
                    result.destination.display(),
                    result.deleted_local
                )),
                Ok(None) => {}
                Err(error) => debug_log::write(error.to_string()),
            }
            match crate::archive_upload::enqueue_and_flush(config, &path) {
                Ok(()) => {}
                Err(error) => debug_log::write(error.to_string()),
            }
            recorder.clear();
            *recorder_events = None;
            *phase = AgentPhase::Idle;
        }
        RecorderEvent::Failed(message) => {
            debug_log::write(format!("agent: recording failed: {}", message));
            *failure_cooldown_until = Some(Instant::now() + Duration::from_secs(60));
            debug_log::write("agent: waiting 60s before next auto-record attempt");
            recorder.clear();
            *recorder_events = None;
            *phase = AgentPhase::Idle;
        }
    }
}

fn pump_windows_messages() {
    unsafe {
        let mut msg = MSG::default();
        while PeekMessageW(&mut msg, None, 0, 0, PM_REMOVE).as_bool() {
            if msg.message == WM_QUIT {
                break;
            }
            let _ = TranslateMessage(&msg);
            DispatchMessageW(&msg);
        }
    }
}

//! Suspend / resume detection shared by the agent and the recorder thread.
//!
//! Primary path: `PowerRegisterSuspendResumeNotification` (callback — works even
//! when no UI message pump runs on the recorder thread).
//! Fallback: window `RegisterSuspendResumeNotification` + Instant clock-jump.

use std::ffi::c_void;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, OnceLock};
use std::thread;
use std::time::{Duration, Instant, SystemTime};

use windows::Win32::Foundation::{HANDLE, HWND, LPARAM, LRESULT, WPARAM};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::System::Power::{
    DEVICE_NOTIFY_SUBSCRIBE_PARAMETERS, HPOWERNOTIFY, PowerRegisterSuspendResumeNotification,
    RegisterSuspendResumeNotification, UnregisterSuspendResumeNotification,
};
use windows::Win32::System::Shutdown::{ShutdownBlockReasonCreate, ShutdownBlockReasonDestroy};
use windows::Win32::UI::WindowsAndMessaging::{
    CS_HREDRAW, CS_VREDRAW, CreateWindowExW, DEVICE_NOTIFY_CALLBACK, DEVICE_NOTIFY_WINDOW_HANDLE,
    DefWindowProcW, DestroyWindow, DispatchMessageW, GWLP_USERDATA, GetMessageW, GetWindowLongPtrW,
    MSG, PostQuitMessage, RegisterClassW, SetWindowLongPtrW, TranslateMessage, WM_DESTROY,
    WM_ENDSESSION, WM_POWERBROADCAST, WM_QUERYENDSESSION, WNDCLASSW, WS_EX_NOACTIVATE,
    WS_EX_TOOLWINDOW, WS_POPUP,
};
use windows::core::w;

const PBT_APMSUSPEND: u32 = 0x0004;
const PBT_APMRESUMESUSPEND: u32 = 0x0007;
const PBT_APMRESUMEAUTOMATIC: u32 = 0x0012;
const PBT_APMRESUMECRITICAL: u32 = 0x0006;

/// Process-wide suspend flag — checked inside capture loops.
static GLOBAL_SUSPENDED: AtomicBool = AtomicBool::new(false);
static GLOBAL_RESUME_EDGE: AtomicBool = AtomicBool::new(false);
static WATCHER_STARTED: OnceLock<()> = OnceLock::new();

fn mark_suspended_global() {
    GLOBAL_SUSPENDED.store(true, Ordering::Release);
    crate::debug_log::write("power: suspend");
}

fn mark_resumed_global() {
    if GLOBAL_SUSPENDED.swap(false, Ordering::AcqRel) {
        GLOBAL_RESUME_EDGE.store(true, Ordering::Release);
        crate::debug_log::write("power: resume");
    }
}

pub fn is_system_suspended() -> bool {
    GLOBAL_SUSPENDED.load(Ordering::Acquire)
}

pub fn take_global_resume_edge() -> bool {
    GLOBAL_RESUME_EDGE.swap(false, Ordering::AcqRel)
}

/// Ensure the process-wide power watcher is running (idempotent).
pub fn ensure_global_watcher() {
    WATCHER_STARTED.get_or_init(|| {
        let _ = thread::Builder::new().name("power-watch".into()).spawn(|| {
            if let Err(error) = run_callback_watcher() {
                crate::debug_log::write(format!(
                    "power callback watcher failed ({error}); trying window watcher"
                ));
                let state = PowerState::from_global();
                if let Err(error) = run_message_loop(state) {
                    crate::debug_log::write(format!("power window watcher exited: {error}"));
                }
            }
        });
    });
}

unsafe extern "system" fn suspend_resume_callback(
    _context: *const c_void,
    type_: u32,
    _setting: *const c_void,
) -> u32 {
    if type_ == PBT_APMSUSPEND {
        mark_suspended_global();
    } else if type_ == PBT_APMRESUMESUSPEND
        || type_ == PBT_APMRESUMEAUTOMATIC
        || type_ == PBT_APMRESUMECRITICAL
    {
        mark_resumed_global();
    }
    0
}

fn run_callback_watcher() -> Result<(), String> {
    unsafe {
        let mut params = DEVICE_NOTIFY_SUBSCRIBE_PARAMETERS {
            Callback: Some(suspend_resume_callback),
            Context: std::ptr::null_mut(),
        };
        let mut registration: *mut c_void = std::ptr::null_mut();
        let err = PowerRegisterSuspendResumeNotification(
            DEVICE_NOTIFY_CALLBACK,
            HANDLE(std::ptr::addr_of_mut!(params).cast()),
            &mut registration,
        );
        if err.is_err() {
            return Err(format!("PowerRegisterSuspendResumeNotification: {err:?}"));
        }
        // `params` must stay at this address for the lifetime of the registration.
        let _registration = registration;
        crate::debug_log::write("power: callback notification registered");
        loop {
            thread::sleep(Duration::from_secs(3600));
            std::sync::atomic::compiler_fence(Ordering::SeqCst);
            let _keep = std::ptr::addr_of!(params);
        }
    }
}

#[derive(Debug, Clone)]
pub struct PowerState {
    suspended: Arc<AtomicBool>,
    resumed_edge: Arc<AtomicBool>,
}

impl PowerState {
    pub fn new() -> Self {
        ensure_global_watcher();
        Self::from_global()
    }

    fn from_global() -> Self {
        Self {
            suspended: Arc::new(AtomicBool::new(false)),
            resumed_edge: Arc::new(AtomicBool::new(false)),
        }
    }

    pub fn is_suspended(&self) -> bool {
        is_system_suspended() || self.suspended.load(Ordering::Acquire)
    }

    pub fn take_resume_edge(&self) -> bool {
        take_global_resume_edge() || self.resumed_edge.swap(false, Ordering::AcqRel)
    }

    fn mark_suspended(&self) {
        mark_suspended_global();
        self.suspended.store(true, Ordering::Release);
    }

    fn mark_resumed(&self) {
        mark_resumed_global();
        if self.suspended.swap(false, Ordering::AcqRel) {
            self.resumed_edge.store(true, Ordering::Release);
        }
    }
}

impl Default for PowerState {
    fn default() -> Self {
        Self::new()
    }
}

pub fn spawn_watcher(state: PowerState) -> thread::JoinHandle<()> {
    ensure_global_watcher();
    // Keep a lightweight window watcher as a second channel.
    thread::Builder::new()
        .name("power-watch-wnd".into())
        .spawn(move || {
            if let Err(error) = run_message_loop(state) {
                crate::debug_log::write(format!("power window watcher exited: {error}"));
            }
        })
        .expect("spawn power window watcher")
}

fn run_message_loop(state: PowerState) -> Result<(), String> {
    unsafe {
        let class_name = w!("OfficePcRecorderPowerWatch");
        let module = GetModuleHandleW(None).map_err(|e| e.to_string())?;
        let wnd_class = WNDCLASSW {
            style: CS_HREDRAW | CS_VREDRAW,
            lpfnWndProc: Some(power_wnd_proc),
            hInstance: module.into(),
            lpszClassName: class_name,
            ..Default::default()
        };
        let _ = RegisterClassW(&wnd_class);

        // Real top-level HWND (1×1 popup, no taskbar) so WM_QUERYENDSESSION /
        // WM_ENDSESSION are delivered on reboot/logoff — a 0×0 style-0 window is unreliable.
        let hwnd = CreateWindowExW(
            WS_EX_TOOLWINDOW | WS_EX_NOACTIVATE,
            class_name,
            w!("Office PC Recorder Power"),
            WS_POPUP,
            0,
            0,
            1,
            1,
            None,
            None,
            Some(module.into()),
            None,
        )
        .map_err(|e| e.to_string())?;

        let boxed = Box::new(state);
        let raw = Box::into_raw(boxed);
        SetWindowLongPtrW(hwnd, GWLP_USERDATA, raw as isize);
        crate::debug_log::write(format!(
            "power: end-session window ready hwnd={:?}",
            hwnd.0
        ));

        let notify = RegisterSuspendResumeNotification(HANDLE(hwnd.0), DEVICE_NOTIFY_WINDOW_HANDLE);
        let notify_handle = match notify {
            Ok(handle) => Some(handle),
            Err(error) => {
                crate::debug_log::write(format!(
                    "power: window RegisterSuspendResumeNotification failed: {error}"
                ));
                None
            }
        };

        let mut message = MSG::default();
        while GetMessageW(&mut message, None, 0, 0).into() {
            let _ = TranslateMessage(&message);
            DispatchMessageW(&message);
        }

        if let Some(handle) = notify_handle {
            let _ = UnregisterSuspendResumeNotification(HPOWERNOTIFY(handle.0));
        }

        let ptr = GetWindowLongPtrW(hwnd, GWLP_USERDATA) as *mut PowerState;
        if !ptr.is_null() {
            drop(Box::from_raw(ptr));
        }
        let _ = DestroyWindow(hwnd);
    }
    Ok(())
}

unsafe extern "system" fn power_wnd_proc(
    hwnd: HWND,
    msg: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    match msg {
        WM_POWERBROADCAST => {
            let event = wparam.0 as u32;
            let ptr = unsafe { GetWindowLongPtrW(hwnd, GWLP_USERDATA) } as *mut PowerState;
            if !ptr.is_null() {
                let state = unsafe { &*ptr };
                if event == PBT_APMSUSPEND {
                    state.mark_suspended();
                } else if event == PBT_APMRESUMESUSPEND
                    || event == PBT_APMRESUMEAUTOMATIC
                    || event == PBT_APMRESUMECRITICAL
                {
                    state.mark_resumed();
                }
            }
            LRESULT(1)
        }
        // GUI agent path for reboot/logoff (SetConsoleCtrlHandler is unreliable here).
        WM_QUERYENDSESSION => {
            crate::debug_log::write("power: WM_QUERYENDSESSION — requesting graceful stop");
            crate::shutdown::request();
            let reason = w!("Office Recorder: finishing recording before shutdown");
            let _ = unsafe { ShutdownBlockReasonCreate(hwnd, reason) };
            LRESULT(1) // TRUE — allow session end; we block briefly via ShutdownBlockReason
        }
        WM_ENDSESSION => {
            let ending = wparam.0 != 0;
            crate::debug_log::write(format!("power: WM_ENDSESSION ending={ending}"));
            if ending {
                crate::shutdown::request();
                // Give the agent time to stop FFmpeg and write moov (sleep path already works).
                crate::shutdown::wait_for_drain(Duration::from_secs(25));
            }
            let _ = unsafe { ShutdownBlockReasonDestroy(hwnd) };
            LRESULT(0)
        }
        WM_DESTROY => {
            unsafe { PostQuitMessage(0) };
            LRESULT(0)
        }
        _ => unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) },
    }
}

/// Detects sleep/wake via Instant/wall-clock gaps when notifications are missed.
pub struct SleepJumpDetector {
    last_instant: Instant,
    last_wall: SystemTime,
}

impl SleepJumpDetector {
    pub fn new() -> Self {
        Self {
            last_instant: Instant::now(),
            last_wall: SystemTime::now(),
        }
    }

    pub fn poll_wake(&mut self) -> bool {
        let now_i = Instant::now();
        let now_w = SystemTime::now();
        let instant_gap = now_i.saturating_duration_since(self.last_instant);
        let wall_gap = now_w
            .duration_since(self.last_wall)
            .unwrap_or(Duration::ZERO);
        self.last_instant = now_i;
        self.last_wall = now_w;
        if instant_gap > Duration::from_secs(3) || wall_gap > Duration::from_secs(3) {
            mark_resumed_global();
            true
        } else {
            false
        }
    }
}

impl Default for SleepJumpDetector {
    fn default() -> Self {
        Self::new()
    }
}

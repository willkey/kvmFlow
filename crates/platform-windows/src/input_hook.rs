#[cfg(windows)]
use kvmflow_domain::{InputEvent, KeyAction, MouseButton};
use kvmflow_domain::{KeyUsage, ScreenEdge};
use kvmflow_platform_api::PlatformBridge;
#[cfg(windows)]
use kvmflow_platform_api::{PlatformCommand, platform_bridge};

#[cfg(windows)]
const PLATFORM_CHANNEL_CAPACITY: usize = 2048;

const VK_BACK: u16 = 0x08;
const VK_TAB: u16 = 0x09;
const VK_RETURN: u16 = 0x0d;
const VK_SHIFT: u16 = 0x10;
const VK_CONTROL: u16 = 0x11;
const VK_MENU: u16 = 0x12;
const VK_CAPITAL: u16 = 0x14;
const VK_ESCAPE: u16 = 0x1b;
const VK_SPACE: u16 = 0x20;
const VK_PRIOR: u16 = 0x21;
const VK_NEXT: u16 = 0x22;
const VK_END: u16 = 0x23;
const VK_HOME: u16 = 0x24;
const VK_LEFT: u16 = 0x25;
const VK_UP: u16 = 0x26;
const VK_RIGHT: u16 = 0x27;
const VK_DOWN: u16 = 0x28;
const VK_DELETE: u16 = 0x2e;
const VK_0: u16 = 0x30;
const VK_9: u16 = 0x39;
const VK_A: u16 = 0x41;
const VK_Z: u16 = 0x5a;
const VK_LWIN: u16 = 0x5b;
const VK_RWIN: u16 = 0x5c;
const VK_NUMPAD0: u16 = 0x60;
const VK_NUMPAD1: u16 = 0x61;
const VK_NUMPAD9: u16 = 0x69;
const VK_MULTIPLY: u16 = 0x6a;
const VK_ADD: u16 = 0x6b;
const VK_SUBTRACT: u16 = 0x6d;
const VK_DECIMAL: u16 = 0x6e;
const VK_DIVIDE: u16 = 0x6f;
const VK_F1: u16 = 0x70;
const VK_F12: u16 = 0x7b;
const VK_NUMLOCK: u16 = 0x90;
const VK_LSHIFT: u16 = 0xa0;
const VK_RSHIFT: u16 = 0xa1;
const VK_LCONTROL: u16 = 0xa2;
const VK_RCONTROL: u16 = 0xa3;
const VK_LMENU: u16 = 0xa4;
const VK_RMENU: u16 = 0xa5;

#[cfg(any(windows, test))]
#[derive(Default)]
struct RawPointerState {
    last_absolute: Option<(i32, i32)>,
}

#[cfg(any(windows, test))]
impl RawPointerState {
    fn movement(&mut self, absolute: bool, x: i32, y: i32) -> Option<(i32, i32)> {
        let movement = if absolute {
            self.last_absolute
                .replace((x, y))
                .map(|(previous_x, previous_y)| (x - previous_x, y - previous_y))
        } else {
            self.last_absolute = None;
            Some((x, y))
        };
        movement.filter(|(dx, dy)| *dx != 0 || *dy != 0)
    }

    fn reset(&mut self) {
        self.last_absolute = None;
    }
}

#[cfg(any(windows, test))]
const EDGE_TRIGGER_BAND_PX: i32 = 2;
#[cfg(any(windows, test))]
const EDGE_REARM_DISTANCE_PX: i32 = 12;
#[cfg(any(windows, test))]
const LOCAL_RESTORE_INSET_PX: i32 = 16;

#[cfg(any(windows, test))]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct ScreenBounds {
    left: i32,
    top: i32,
    right: i32,
    bottom: i32,
}

#[cfg(any(windows, test))]
impl ScreenBounds {
    fn normalized_position(self, edge: ScreenEdge, x: i32, y: i32) -> f32 {
        match edge {
            ScreenEdge::Left | ScreenEdge::Right => {
                let span = (self.bottom - self.top).max(1);
                ((y - self.top) as f32 / span as f32).clamp(0.0, 1.0)
            }
            ScreenEdge::Top | ScreenEdge::Bottom => {
                let span = (self.right - self.left).max(1);
                ((x - self.left) as f32 / span as f32).clamp(0.0, 1.0)
            }
        }
    }

    fn contains_edge_position(self, edge: ScreenEdge, x: i32, y: i32) -> bool {
        match edge {
            ScreenEdge::Left | ScreenEdge::Right => (self.top..=self.bottom).contains(&y),
            ScreenEdge::Top | ScreenEdge::Bottom => (self.left..=self.right).contains(&x),
        }
    }

    fn reached(self, edge: ScreenEdge, x: i32, y: i32) -> bool {
        match edge {
            ScreenEdge::Right => {
                (self.right - (EDGE_TRIGGER_BAND_PX - 1)..=self.right).contains(&x)
            }
            ScreenEdge::Left => (self.left..=self.left + (EDGE_TRIGGER_BAND_PX - 1)).contains(&x),
            ScreenEdge::Top => (self.top..=self.top + (EDGE_TRIGGER_BAND_PX - 1)).contains(&y),
            ScreenEdge::Bottom => {
                (self.bottom - (EDGE_TRIGGER_BAND_PX - 1)..=self.bottom).contains(&y)
            }
        }
    }

    fn moved_away(self, edge: ScreenEdge, x: i32, y: i32) -> bool {
        match edge {
            ScreenEdge::Right => x <= self.right - EDGE_REARM_DISTANCE_PX,
            ScreenEdge::Left => x >= self.left + EDGE_REARM_DISTANCE_PX,
            ScreenEdge::Top => y >= self.top + EDGE_REARM_DISTANCE_PX,
            ScreenEdge::Bottom => y <= self.bottom - EDGE_REARM_DISTANCE_PX,
        }
    }

    fn restore_point(self, edge: ScreenEdge, position: f32) -> (i32, i32) {
        let position = position.clamp(0.0, 1.0);
        match edge {
            ScreenEdge::Left | ScreenEdge::Right => {
                let inset = LOCAL_RESTORE_INSET_PX.min((self.right - self.left).max(1));
                let x = if edge == ScreenEdge::Right {
                    self.right - inset
                } else {
                    self.left + inset
                };
                let y =
                    self.top + ((self.bottom - self.top).max(1) as f32 * position).round() as i32;
                (x, y.clamp(self.top, self.bottom))
            }
            ScreenEdge::Top | ScreenEdge::Bottom => {
                let inset = LOCAL_RESTORE_INSET_PX.min((self.bottom - self.top).max(1));
                let x =
                    self.left + ((self.right - self.left).max(1) as f32 * position).round() as i32;
                let y = if edge == ScreenEdge::Bottom {
                    self.bottom - inset
                } else {
                    self.top + inset
                };
                (x.clamp(self.left, self.right), y)
            }
        }
    }
}

#[cfg(any(windows, test))]
#[derive(Debug, Clone, Copy, PartialEq)]
struct EdgeTrigger {
    edge: ScreenEdge,
    position: f32,
}

#[cfg(any(windows, test))]
#[derive(Debug)]
struct EdgeDetector {
    edge: ScreenEdge,
    enabled: bool,
    latched: bool,
}

#[cfg(any(windows, test))]
impl EdgeDetector {
    fn new(edge: ScreenEdge) -> Self {
        Self {
            edge,
            enabled: false,
            latched: false,
        }
    }

    fn set_enabled(&mut self, enabled: bool) {
        self.enabled = enabled;
        if !enabled {
            self.latched = false;
        }
    }

    fn set_edge(&mut self, edge: ScreenEdge) {
        self.edge = edge;
        self.latched = false;
    }

    fn observe(
        &mut self,
        bounds: ScreenBounds,
        x: i32,
        y: i32,
        button_held: bool,
    ) -> Option<EdgeTrigger> {
        if !self.enabled || !bounds.contains_edge_position(self.edge, x, y) {
            return None;
        }
        if self.latched {
            if bounds.moved_away(self.edge, x, y) {
                self.latched = false;
            }
            return None;
        }
        if button_held || !bounds.reached(self.edge, x, y) {
            return None;
        }
        self.latched = true;
        Some(EdgeTrigger {
            edge: self.edge,
            position: bounds.normalized_position(self.edge, x, y),
        })
    }
}

#[cfg(windows)]
#[derive(Debug, Clone, Copy)]
struct ActiveEdgeTransition {
    edge: ScreenEdge,
    return_position: f32,
}

pub fn virtual_key_to_remote_usage(virtual_key: u16) -> Option<KeyUsage> {
    virtual_key_to_remote_usage_with_extended(virtual_key, false)
}

fn virtual_key_to_remote_usage_with_extended(virtual_key: u16, extended: bool) -> Option<KeyUsage> {
    let usage = match virtual_key {
        VK_A..=VK_Z => 0x04 + virtual_key - VK_A,
        0x31..=VK_9 => 0x1e + virtual_key - 0x31,
        VK_0 => 0x27,
        VK_RETURN if extended => KeyUsage::KEYPAD_ENTER.0,
        VK_RETURN => KeyUsage::ENTER.0,
        VK_ESCAPE => 0x29,
        VK_BACK => 0x2a,
        VK_TAB => 0x2b,
        VK_SPACE => 0x2c,
        VK_CAPITAL => KeyUsage::CAPS_LOCK.0,
        0xbd => 0x2d,
        0xbb => 0x2e,
        0xdb => 0x2f,
        0xdd => 0x30,
        0xdc => 0x31,
        0xba => 0x33,
        0xde => 0x34,
        0xc0 => 0x35,
        0xbc => 0x36,
        0xbe => 0x37,
        0xbf => 0x38,
        VK_F1..=VK_F12 => 0x3a + virtual_key - VK_F1,
        VK_NUMLOCK => KeyUsage::NUM_LOCK.0,
        VK_DIVIDE => 0x54,
        VK_MULTIPLY => 0x55,
        VK_SUBTRACT => 0x56,
        VK_ADD => 0x57,
        VK_NUMPAD1..=VK_NUMPAD9 => 0x59 + virtual_key - VK_NUMPAD1,
        VK_NUMPAD0 => 0x62,
        VK_DECIMAL => 0x63,
        VK_HOME => 0x4a,
        VK_PRIOR => 0x4b,
        VK_END => 0x4d,
        VK_NEXT => 0x4e,
        VK_RIGHT => 0x4f,
        VK_LEFT => 0x50,
        VK_DOWN => 0x51,
        VK_UP => 0x52,
        VK_DELETE => 0x4c,
        VK_CONTROL | VK_LCONTROL => KeyUsage::LEFT_GUI.0,
        VK_RCONTROL => KeyUsage::RIGHT_GUI.0,
        VK_SHIFT | VK_LSHIFT => KeyUsage::LEFT_SHIFT.0,
        VK_RSHIFT => KeyUsage::RIGHT_SHIFT.0,
        VK_MENU | VK_LMENU => KeyUsage::LEFT_ALT.0,
        VK_RMENU => KeyUsage::RIGHT_ALT.0,
        VK_LWIN => KeyUsage::LEFT_CONTROL.0,
        VK_RWIN => KeyUsage::RIGHT_CONTROL.0,
        _ => return None,
    };
    Some(KeyUsage(usage))
}

#[cfg(windows)]
pub fn spawn_windows_platform_adapter(
    edge: ScreenEdge,
) -> Result<PlatformBridge, WindowsInputAdapterError> {
    let (mut driver, bridge) = platform_bridge(PLATFORM_CHANNEL_CAPACITY);
    let control = Win32HookControl::start(driver.events.clone(), edge)?;
    tokio::spawn(async move {
        let mut control = control;
        while let Some(command) = driver.commands.recv().await {
            match command {
                PlatformCommand::SetInputSuppressed(suppressed) => {
                    control.set_suppressed(suppressed);
                }
                PlatformCommand::SetEdgeSwitchEnabled(enabled) => {
                    control.set_edge_switch_enabled(enabled);
                }
                PlatformCommand::SetEdgeSwitchEdge(edge) => {
                    control.set_edge_switch_edge(edge);
                }
                PlatformCommand::BeginEdgeTransition { edge, position } => {
                    control.begin_edge_transition(edge, position);
                }
                PlatformCommand::RestoreLocalPointer { position } => {
                    control.restore_local_pointer(position);
                }
                PlatformCommand::Shutdown => {
                    control.set_suppressed(false);
                    return;
                }
                PlatformCommand::Inject(_)
                | PlatformCommand::ReleaseAll
                | PlatformCommand::PrepareRemotePointer { .. }
                | PlatformCommand::RefreshPermission
                | PlatformCommand::OpenAccessibilitySettings => {}
            }
        }
        control.set_suppressed(false);
    });
    Ok(bridge)
}

#[cfg(not(windows))]
pub fn spawn_windows_platform_adapter(
    _edge: ScreenEdge,
) -> Result<PlatformBridge, WindowsInputAdapterError> {
    Err(WindowsInputAdapterError::Unavailable)
}

#[cfg(windows)]
mod win32 {
    use std::{
        collections::HashSet,
        ptr,
        sync::{
            Arc, Mutex, OnceLock,
            atomic::{AtomicBool, Ordering},
            mpsc,
        },
        time::{Duration, Instant},
    };

    use kvmflow_platform_api::PlatformEvent;
    use tokio::sync::mpsc::Sender;
    use windows_sys::Win32::{
        Foundation::{HWND, LPARAM, LRESULT, WPARAM},
        System::{LibraryLoader::GetModuleHandleW, Threading::GetCurrentThreadId},
        UI::{
            Input::{
                GetRawInputData, MOUSE_MOVE_ABSOLUTE, MOUSE_VIRTUAL_DESKTOP, RAWINPUT,
                RAWINPUTDEVICE, RAWINPUTHEADER, RID_INPUT, RIDEV_INPUTSINK, RIDEV_REMOVE,
                RIM_TYPEKEYBOARD, RIM_TYPEMOUSE, RegisterRawInputDevices,
            },
            WindowsAndMessaging::{
                CallNextHookEx, CreateWindowExW, DefWindowProcW, DestroyWindow, DispatchMessageW,
                GetMessageW, GetSystemMetrics, HWND_MESSAGE, KBDLLHOOKSTRUCT, MSG, MSLLHOOKSTRUCT,
                PostThreadMessageW, RI_KEY_BREAK, RI_KEY_E0, RegisterClassW, SM_CXSCREEN,
                SM_CXVIRTUALSCREEN, SM_CYSCREEN, SM_CYVIRTUALSCREEN, SetCursorPos,
                SetWindowsHookExW, TranslateMessage, UnhookWindowsHookEx, UnregisterClassW,
                WH_KEYBOARD_LL, WH_MOUSE_LL, WM_INPUT, WM_KEYDOWN, WM_KEYUP, WM_LBUTTONDOWN,
                WM_LBUTTONUP, WM_MBUTTONDOWN, WM_MBUTTONUP, WM_MOUSEHWHEEL, WM_MOUSEMOVE,
                WM_MOUSEWHEEL, WM_QUIT, WM_RBUTTONDOWN, WM_RBUTTONUP, WM_SYSKEYDOWN, WM_SYSKEYUP,
                WM_XBUTTONDOWN, WM_XBUTTONUP, WNDCLASSW,
            },
        },
    };

    use super::{
        ActiveEdgeTransition, EdgeDetector, InputEvent, KeyAction, MouseButton, RawPointerState,
        ScreenBounds, ScreenEdge, VK_CONTROL, VK_F12, VK_LCONTROL, VK_LMENU, VK_LSHIFT, VK_MENU,
        VK_RCONTROL, VK_RMENU, VK_RSHIFT, VK_SHIFT, WindowsInputAdapterError,
        virtual_key_to_remote_usage_with_extended,
    };

    const HID_USAGE_PAGE_GENERIC: u16 = 0x01;
    const HID_USAGE_GENERIC_MOUSE: u16 = 0x02;
    const HID_USAGE_GENERIC_KEYBOARD: u16 = 0x06;
    const EMERGENCY_EDGE_COOLDOWN: Duration = Duration::from_secs(3);

    pub(super) struct Win32HookControl {
        thread_id: u32,
        completion: mpsc::Receiver<()>,
        join: Option<std::thread::JoinHandle<()>>,
        suppressed: Arc<AtomicBool>,
    }

    impl Win32HookControl {
        pub(super) fn start(
            events: Sender<PlatformEvent>,
            edge: ScreenEdge,
        ) -> Result<Self, WindowsInputAdapterError> {
            let (startup_tx, startup_rx) = mpsc::channel();
            let (completion_tx, completion_rx) = mpsc::channel();
            let suppressed = Arc::new(AtomicBool::new(false));
            let thread_suppressed = suppressed.clone();
            let join = std::thread::Builder::new()
                .name("kvmflow-win32-input".into())
                .spawn(move || {
                    run_hook_thread(events, thread_suppressed, edge, startup_tx);
                    let _ = completion_tx.send(());
                })
                .map_err(|_| WindowsInputAdapterError::StartFailed)?;
            let thread_id = startup_rx
                .recv()
                .map_err(|_| WindowsInputAdapterError::StartFailed)??;
            Ok(Self {
                thread_id,
                completion: completion_rx,
                join: Some(join),
                suppressed,
            })
        }

        pub(super) fn set_suppressed(&mut self, suppressed: bool) {
            let was_suppressed = self.suppressed.swap(suppressed, Ordering::SeqCst);
            let mut restore_point = None;
            if let Ok(mut state) = hook_state().lock()
                && let Some(state) = state.as_mut()
            {
                if suppressed && !was_suppressed {
                    state.stats = CaptureSessionStats::default();
                    state.remote_pressed_keys.clear();
                    state.raw_keyboard_observed = false;
                    state.remote_started_at = Some(Instant::now());
                } else if !suppressed && state.remote_active {
                    state.stats.log_and_reset("control_ended");
                }
                if suppressed {
                    state.remote_active = true;
                    state.transition_pending = false;
                } else {
                    if state.remote_active {
                        restore_point = state.active_transition.map(|transition| {
                            state
                                .screen_bounds
                                .restore_point(transition.edge, transition.return_position)
                        });
                    }
                    state.remote_active = false;
                    state.transition_pending = false;
                    state.active_transition = None;
                }
                state.raw_pointer.reset();
                state.raw_input_observed = false;
                if !suppressed {
                    state.pressed_keys.clear();
                    state.remote_pressed_keys.clear();
                    state.remote_started_at = None;
                }
            }
            if let Some((x, y)) = restore_point {
                unsafe {
                    SetCursorPos(x, y);
                }
            }
        }

        pub(super) fn set_edge_switch_enabled(&mut self, enabled: bool) {
            if let Ok(mut state) = hook_state().lock()
                && let Some(state) = state.as_mut()
            {
                state.screen_bounds = primary_screen_bounds();
                state.edge_detector.set_enabled(enabled);
                if !enabled && !state.remote_active {
                    state.transition_pending = false;
                    state.active_transition = None;
                }
            }
        }

        pub(super) fn set_edge_switch_edge(&mut self, edge: ScreenEdge) {
            if let Ok(mut state) = hook_state().lock()
                && let Some(state) = state.as_mut()
            {
                state.edge_detector.set_edge(edge);
            }
        }

        pub(super) fn begin_edge_transition(&mut self, edge: ScreenEdge, position: f32) {
            if let Ok(mut state) = hook_state().lock()
                && let Some(state) = state.as_mut()
            {
                state.transition_pending = true;
                state.active_transition = Some(ActiveEdgeTransition {
                    edge,
                    return_position: position.clamp(0.0, 1.0),
                });
            }
        }

        pub(super) fn restore_local_pointer(&mut self, position: f32) {
            if let Ok(mut state) = hook_state().lock()
                && let Some(state) = state.as_mut()
                && let Some(transition) = state.active_transition.as_mut()
            {
                transition.return_position = position.clamp(0.0, 1.0);
            }
        }
    }

    impl Drop for Win32HookControl {
        fn drop(&mut self) {
            self.set_suppressed(false);
            unsafe {
                PostThreadMessageW(self.thread_id, WM_QUIT, 0, 0);
            }
            if self.completion.recv_timeout(Duration::from_secs(2)).is_ok()
                && let Some(join) = self.join.take()
            {
                let _ = join.join();
            }
        }
    }

    struct HookState {
        events: Sender<PlatformEvent>,
        suppressed: Arc<AtomicBool>,
        pressed_keys: HashSet<u16>,
        remote_pressed_keys: HashSet<kvmflow_domain::KeyUsage>,
        raw_pointer: RawPointerState,
        raw_input_observed: bool,
        raw_keyboard_observed: bool,
        screen_bounds: ScreenBounds,
        edge_detector: EdgeDetector,
        transition_pending: bool,
        active_transition: Option<ActiveEdgeTransition>,
        remote_active: bool,
        held_mouse_buttons: HashSet<MouseButton>,
        edge_switch_cooldown_until: Option<Instant>,
        stats: CaptureSessionStats,
        remote_started_at: Option<Instant>,
    }

    #[derive(Debug, Default)]
    struct CaptureSessionStats {
        key_down_events: u64,
        key_up_events: u64,
        modifier_down_events: u64,
        modifier_up_events: u64,
        repeated_key_down_events: u64,
        ignored_key_up_events: u64,
        unsupported_key_events: u64,
        mouse_button_events: u64,
        back_button_events: u64,
        forward_button_events: u64,
        mouse_move_queue_drops: u64,
        reliable_queue_failures: u64,
        first_key_latency_ms: Option<u64>,
    }

    impl CaptureSessionStats {
        fn record_key(&mut self, usage: kvmflow_domain::KeyUsage, down: bool) {
            if down {
                self.key_down_events = self.key_down_events.saturating_add(1);
                if is_modifier_usage(usage) {
                    self.modifier_down_events = self.modifier_down_events.saturating_add(1);
                }
            } else {
                self.key_up_events = self.key_up_events.saturating_add(1);
                if is_modifier_usage(usage) {
                    self.modifier_up_events = self.modifier_up_events.saturating_add(1);
                }
            }
        }

        fn record_mouse_event(&mut self, event: &InputEvent) {
            let InputEvent::MouseButton { button, .. } = event else {
                return;
            };
            self.mouse_button_events = self.mouse_button_events.saturating_add(1);
            match button {
                MouseButton::Back => {
                    self.back_button_events = self.back_button_events.saturating_add(1);
                }
                MouseButton::Forward => {
                    self.forward_button_events = self.forward_button_events.saturating_add(1);
                }
                MouseButton::Left | MouseButton::Right | MouseButton::Middle => {}
            }
        }

        fn log_and_reset(&mut self, reason: &'static str) {
            if self.key_down_events == 0
                && self.key_up_events == 0
                && self.unsupported_key_events == 0
                && self.mouse_button_events == 0
                && self.mouse_move_queue_drops == 0
                && self.reliable_queue_failures == 0
            {
                return;
            }
            tracing::info!(
                component = "input",
                stage = "windows_capture",
                reason,
                key_down_events = self.key_down_events,
                key_up_events = self.key_up_events,
                modifier_down_events = self.modifier_down_events,
                modifier_up_events = self.modifier_up_events,
                repeated_key_down_events = self.repeated_key_down_events,
                ignored_key_up_events = self.ignored_key_up_events,
                unsupported_key_events = self.unsupported_key_events,
                mouse_button_events = self.mouse_button_events,
                back_button_events = self.back_button_events,
                forward_button_events = self.forward_button_events,
                mouse_move_queue_drops = self.mouse_move_queue_drops,
                reliable_queue_failures = self.reliable_queue_failures,
                first_key_latency_ms = self.first_key_latency_ms,
                "Windows input capture session summary"
            );
            *self = Self::default();
        }
    }

    fn hook_state() -> &'static Mutex<Option<HookState>> {
        static STATE: OnceLock<Mutex<Option<HookState>>> = OnceLock::new();
        STATE.get_or_init(|| Mutex::new(None))
    }

    fn run_hook_thread(
        events: Sender<PlatformEvent>,
        suppressed: Arc<AtomicBool>,
        edge: ScreenEdge,
        startup: mpsc::Sender<Result<u32, WindowsInputAdapterError>>,
    ) {
        let mut message: MSG = unsafe { std::mem::zeroed() };
        let thread_id = unsafe { GetCurrentThreadId() };
        if let Ok(mut state) = hook_state().lock() {
            *state = Some(HookState {
                events,
                suppressed,
                pressed_keys: HashSet::new(),
                remote_pressed_keys: HashSet::new(),
                raw_pointer: RawPointerState::default(),
                raw_input_observed: false,
                raw_keyboard_observed: false,
                screen_bounds: primary_screen_bounds(),
                edge_detector: EdgeDetector::new(edge),
                transition_pending: false,
                active_transition: None,
                remote_active: false,
                held_mouse_buttons: HashSet::new(),
                edge_switch_cooldown_until: None,
                stats: CaptureSessionStats::default(),
                remote_started_at: None,
            });
        } else {
            let _ = startup.send(Err(WindowsInputAdapterError::StartFailed));
            return;
        }
        let raw_input_window = match RawInputWindow::create() {
            Ok(window) => window,
            Err(error) => {
                clear_hook_state();
                let _ = startup.send(Err(error));
                return;
            }
        };
        let keyboard =
            unsafe { SetWindowsHookExW(WH_KEYBOARD_LL, Some(keyboard_hook), ptr::null_mut(), 0) };
        if keyboard.is_null() {
            clear_hook_state();
            let _ = startup.send(Err(WindowsInputAdapterError::InstallFailed));
            return;
        }
        let mouse = unsafe { SetWindowsHookExW(WH_MOUSE_LL, Some(mouse_hook), ptr::null_mut(), 0) };
        if mouse.is_null() {
            unsafe {
                UnhookWindowsHookEx(keyboard);
            }
            clear_hook_state();
            let _ = startup.send(Err(WindowsInputAdapterError::InstallFailed));
            return;
        }
        let _ = startup.send(Ok(thread_id));
        loop {
            let result = unsafe { GetMessageW(&mut message, ptr::null_mut(), 0, 0) };
            if result <= 0 {
                break;
            }
            unsafe {
                TranslateMessage(&message);
                DispatchMessageW(&message);
            }
        }
        unsafe {
            UnhookWindowsHookEx(keyboard);
            UnhookWindowsHookEx(mouse);
        }
        drop(raw_input_window);
        clear_hook_state();
    }

    struct RawInputWindow {
        hwnd: HWND,
        instance: *mut core::ffi::c_void,
    }

    impl RawInputWindow {
        fn create() -> Result<Self, WindowsInputAdapterError> {
            let class_name = raw_input_window_class();
            let instance = unsafe { GetModuleHandleW(ptr::null()) };
            if instance.is_null() {
                return Err(WindowsInputAdapterError::RawInputWindowFailed);
            }
            let window_class = WNDCLASSW {
                lpfnWndProc: Some(raw_input_window_proc),
                hInstance: instance,
                lpszClassName: class_name.as_ptr(),
                ..unsafe { std::mem::zeroed() }
            };
            if unsafe { RegisterClassW(&window_class) } == 0 {
                return Err(WindowsInputAdapterError::RawInputWindowFailed);
            }
            let hwnd = unsafe {
                CreateWindowExW(
                    0,
                    class_name.as_ptr(),
                    ptr::null(),
                    0,
                    0,
                    0,
                    0,
                    0,
                    HWND_MESSAGE,
                    ptr::null_mut(),
                    instance,
                    ptr::null(),
                )
            };
            if hwnd.is_null() {
                unsafe {
                    UnregisterClassW(class_name.as_ptr(), instance);
                }
                return Err(WindowsInputAdapterError::RawInputWindowFailed);
            }
            let devices = raw_input_devices(RIDEV_INPUTSINK, hwnd);
            if unsafe {
                RegisterRawInputDevices(
                    devices.as_ptr(),
                    devices.len() as u32,
                    std::mem::size_of::<RAWINPUTDEVICE>() as u32,
                )
            } == 0
            {
                unsafe {
                    DestroyWindow(hwnd);
                    UnregisterClassW(class_name.as_ptr(), instance);
                }
                return Err(WindowsInputAdapterError::RawInputRegistrationFailed);
            }
            Ok(Self { hwnd, instance })
        }
    }

    impl Drop for RawInputWindow {
        fn drop(&mut self) {
            let devices = raw_input_devices(RIDEV_REMOVE, ptr::null_mut());
            unsafe {
                RegisterRawInputDevices(
                    devices.as_ptr(),
                    devices.len() as u32,
                    std::mem::size_of::<RAWINPUTDEVICE>() as u32,
                );
                DestroyWindow(self.hwnd);
                UnregisterClassW(raw_input_window_class().as_ptr(), self.instance);
            }
        }
    }

    fn raw_input_window_class() -> &'static [u16] {
        static CLASS_NAME: OnceLock<Vec<u16>> = OnceLock::new();
        CLASS_NAME.get_or_init(|| "kvmFlowRawInputWindow\0".encode_utf16().collect())
    }

    fn raw_input_devices(flags: u32, hwnd: HWND) -> [RAWINPUTDEVICE; 2] {
        [
            RAWINPUTDEVICE {
                usUsagePage: HID_USAGE_PAGE_GENERIC,
                usUsage: HID_USAGE_GENERIC_MOUSE,
                dwFlags: flags,
                hwndTarget: hwnd,
            },
            RAWINPUTDEVICE {
                usUsagePage: HID_USAGE_PAGE_GENERIC,
                usUsage: HID_USAGE_GENERIC_KEYBOARD,
                dwFlags: flags,
                hwndTarget: hwnd,
            },
        ]
    }

    unsafe extern "system" fn raw_input_window_proc(
        hwnd: HWND,
        message: u32,
        wparam: WPARAM,
        lparam: LPARAM,
    ) -> LRESULT {
        if message == WM_INPUT {
            process_raw_input(lparam);
            unsafe {
                DefWindowProcW(hwnd, message, wparam, lparam);
            }
            return 0;
        }
        unsafe { DefWindowProcW(hwnd, message, wparam, lparam) }
    }

    fn process_raw_input(lparam: LPARAM) {
        if lparam == 0 {
            return;
        }
        let mut input = RAWINPUT::default();
        let mut input_size = std::mem::size_of::<RAWINPUT>() as u32;
        let result = unsafe {
            GetRawInputData(
                lparam as _,
                RID_INPUT,
                &mut input as *mut _ as *mut core::ffi::c_void,
                &mut input_size,
                std::mem::size_of::<RAWINPUTHEADER>() as u32,
            )
        };
        if result == u32::MAX || result == 0 {
            return;
        }
        match input.header.dwType {
            RIM_TYPEMOUSE => {
                let mouse = unsafe { input.data.mouse };
                process_raw_mouse(mouse.usFlags, mouse.lLastX, mouse.lLastY);
            }
            RIM_TYPEKEYBOARD => {
                let keyboard = unsafe { input.data.keyboard };
                process_raw_keyboard(keyboard.VKey, keyboard.MakeCode, keyboard.Flags);
            }
            _ => {}
        }
    }

    fn process_raw_keyboard(virtual_key: u16, make_code: u16, flags: u16) {
        if virtual_key == u8::MAX.into() {
            return;
        }
        let extended = u32::from(flags) & RI_KEY_E0 != 0;
        let virtual_key = normalize_raw_virtual_key(virtual_key, make_code, extended);
        let down = u32::from(flags) & RI_KEY_BREAK == 0;
        let Ok(mut guard) = hook_state().lock() else {
            return;
        };
        let Some(state) = guard.as_mut() else {
            return;
        };
        if !state.suppressed.load(Ordering::SeqCst) {
            return;
        }
        if !state.raw_keyboard_observed {
            state.raw_keyboard_observed = true;
            state.stats.first_key_latency_ms = state
                .remote_started_at
                .map(|started_at| started_at.elapsed().as_millis() as u64);
            tracing::info!(component = "input", "Windows raw keyboard capture active");
        }
        let Some(usage) = virtual_key_to_remote_usage_with_extended(virtual_key, extended) else {
            state.stats.unsupported_key_events =
                state.stats.unsupported_key_events.saturating_add(1);
            return;
        };

        if down {
            let repeated = !state.remote_pressed_keys.insert(usage);
            if repeated {
                state.stats.repeated_key_down_events =
                    state.stats.repeated_key_down_events.saturating_add(1);
                if is_modifier_usage(usage) || usage == kvmflow_domain::KeyUsage::CAPS_LOCK {
                    return;
                }
            }
        } else if !state.remote_pressed_keys.remove(&usage) {
            state.stats.ignored_key_up_events = state.stats.ignored_key_up_events.saturating_add(1);
            return;
        }

        state.stats.record_key(usage, down);
        send_input(
            state,
            InputEvent::Key {
                usage,
                action: if down { KeyAction::Down } else { KeyAction::Up },
            },
            true,
        );
    }

    fn normalize_raw_virtual_key(virtual_key: u16, make_code: u16, extended: bool) -> u16 {
        match virtual_key {
            VK_SHIFT if make_code == 0x36 => VK_RSHIFT,
            VK_SHIFT => VK_LSHIFT,
            VK_CONTROL if extended => VK_RCONTROL,
            VK_CONTROL => VK_LCONTROL,
            VK_MENU if extended => VK_RMENU,
            VK_MENU => VK_LMENU,
            _ => virtual_key,
        }
    }

    fn process_raw_mouse(flags: u16, x: i32, y: i32) {
        let absolute = flags & MOUSE_MOVE_ABSOLUTE != 0;
        let (x, y) = if absolute {
            raw_absolute_position(flags, x, y)
        } else {
            (x, y)
        };
        let Ok(mut guard) = hook_state().lock() else {
            return;
        };
        let Some(state) = guard.as_mut() else {
            return;
        };
        if !state.suppressed.load(Ordering::SeqCst) {
            state.raw_pointer.reset();
            state.raw_input_observed = false;
            return;
        }
        if let Some((dx, dy)) = state.raw_pointer.movement(absolute, x, y) {
            if !state.raw_input_observed {
                tracing::info!(
                    component = "input",
                    "Windows raw mouse movement capture active"
                );
                state.raw_input_observed = true;
            }
            send_input(state, InputEvent::MouseMove { dx, dy }, false);
        }
    }

    fn raw_absolute_position(flags: u16, x: i32, y: i32) -> (i32, i32) {
        let virtual_desktop = flags & MOUSE_VIRTUAL_DESKTOP != 0;
        let (width_metric, height_metric) = if virtual_desktop {
            (SM_CXVIRTUALSCREEN, SM_CYVIRTUALSCREEN)
        } else {
            (SM_CXSCREEN, SM_CYSCREEN)
        };
        let width = unsafe { GetSystemMetrics(width_metric) }.max(1) as i64;
        let height = unsafe { GetSystemMetrics(height_metric) }.max(1) as i64;
        (
            ((x as i64 * width) / u16::MAX as i64) as i32,
            ((y as i64 * height) / u16::MAX as i64) as i32,
        )
    }

    fn primary_screen_bounds() -> ScreenBounds {
        ScreenBounds {
            left: 0,
            top: 0,
            right: unsafe { GetSystemMetrics(SM_CXSCREEN) }.max(1) - 1,
            bottom: unsafe { GetSystemMetrics(SM_CYSCREEN) }.max(1) - 1,
        }
    }

    fn clear_hook_state() {
        if let Ok(mut state) = hook_state().lock() {
            *state = None;
        }
    }

    unsafe extern "system" fn keyboard_hook(code: i32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
        if code >= 0 && lparam != 0 {
            let data = unsafe { *(lparam as *const KBDLLHOOKSTRUCT) };
            let virtual_key = data.vkCode as u16;
            let down = matches!(wparam as u32, WM_KEYDOWN | WM_SYSKEYDOWN);
            let up = matches!(wparam as u32, WM_KEYUP | WM_SYSKEYUP);
            if (down || up) && process_keyboard_hook(virtual_key, down) {
                return 1;
            }
        }
        unsafe { CallNextHookEx(ptr::null_mut(), code, wparam, lparam) }
    }

    fn process_keyboard_hook(virtual_key: u16, down: bool) -> bool {
        let Ok(mut guard) = hook_state().lock() else {
            return false;
        };
        let Some(state) = guard.as_mut() else {
            return false;
        };
        if down {
            state.pressed_keys.insert(virtual_key);
        } else {
            state.pressed_keys.remove(&virtual_key);
        }
        let suppressed = state.suppressed.load(Ordering::SeqCst);
        if (suppressed || state.transition_pending) && emergency_hotkey(&state.pressed_keys) {
            state.suppressed.store(false, Ordering::SeqCst);
            state.stats.log_and_reset("emergency_hotkey");
            state.pressed_keys.clear();
            state.transition_pending = false;
            state.edge_switch_cooldown_until = Some(Instant::now() + EMERGENCY_EDGE_COOLDOWN);
            let _ = state.events.try_send(PlatformEvent::EmergencyReturn);
            return true;
        }
        suppressed
    }

    unsafe extern "system" fn mouse_hook(code: i32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
        if code >= 0 && lparam != 0 {
            let data = unsafe { *(lparam as *const MSLLHOOKSTRUCT) };
            if process_mouse(wparam as u32, &data) {
                return 1;
            }
        }
        unsafe { CallNextHookEx(ptr::null_mut(), code, wparam, lparam) }
    }

    fn process_mouse(message: u32, data: &MSLLHOOKSTRUCT) -> bool {
        let Ok(mut guard) = hook_state().lock() else {
            return false;
        };
        let Some(state) = guard.as_mut() else {
            return false;
        };
        if let Some((button, down)) = mouse_button_change(message, data.mouseData) {
            if down {
                state.held_mouse_buttons.insert(button);
            } else {
                state.held_mouse_buttons.remove(&button);
            }
        }
        let suppressed = state.suppressed.load(Ordering::SeqCst);
        if !suppressed {
            if message != WM_MOUSEMOVE {
                return false;
            }
            if state.transition_pending {
                return true;
            }
            if state
                .edge_switch_cooldown_until
                .is_some_and(|until| until > Instant::now())
            {
                return false;
            }
            state.edge_switch_cooldown_until = None;
            if let Some(trigger) = state.edge_detector.observe(
                state.screen_bounds,
                data.pt.x,
                data.pt.y,
                !state.held_mouse_buttons.is_empty(),
            ) {
                if state
                    .events
                    .try_send(PlatformEvent::LocalEdgeReached {
                        edge: trigger.edge,
                        position: trigger.position,
                    })
                    .is_ok()
                {
                    state.transition_pending = true;
                    state.active_transition = Some(ActiveEdgeTransition {
                        edge: trigger.edge,
                        return_position: trigger.position,
                    });
                    return true;
                }
                tracing::warn!(
                    component = "input",
                    "Windows edge transition event queue is full"
                );
            }
            return false;
        }
        let event = match message {
            WM_MOUSEMOVE => None,
            WM_LBUTTONDOWN | WM_LBUTTONUP => Some(InputEvent::MouseButton {
                button: MouseButton::Left,
                action: button_action(message == WM_LBUTTONDOWN),
            }),
            WM_RBUTTONDOWN | WM_RBUTTONUP => Some(InputEvent::MouseButton {
                button: MouseButton::Right,
                action: button_action(message == WM_RBUTTONDOWN),
            }),
            WM_MBUTTONDOWN | WM_MBUTTONUP => Some(InputEvent::MouseButton {
                button: MouseButton::Middle,
                action: button_action(message == WM_MBUTTONDOWN),
            }),
            WM_XBUTTONDOWN | WM_XBUTTONUP => Some(InputEvent::MouseButton {
                button: if high_word(data.mouseData) == 1 {
                    MouseButton::Back
                } else {
                    MouseButton::Forward
                },
                action: button_action(message == WM_XBUTTONDOWN),
            }),
            WM_MOUSEWHEEL => Some(InputEvent::Wheel {
                delta_x: 0,
                delta_y: high_word(data.mouseData) as i16,
            }),
            WM_MOUSEHWHEEL => Some(InputEvent::Wheel {
                delta_x: high_word(data.mouseData) as i16,
                delta_y: 0,
            }),
            _ => None,
        };
        if let Some(event) = event {
            let reliable = !matches!(event, InputEvent::MouseMove { .. });
            state.stats.record_mouse_event(&event);
            send_input(state, event, reliable);
        }
        true
    }

    fn mouse_button_change(message: u32, mouse_data: u32) -> Option<(MouseButton, bool)> {
        let change = match message {
            WM_LBUTTONDOWN => (MouseButton::Left, true),
            WM_LBUTTONUP => (MouseButton::Left, false),
            WM_RBUTTONDOWN => (MouseButton::Right, true),
            WM_RBUTTONUP => (MouseButton::Right, false),
            WM_MBUTTONDOWN => (MouseButton::Middle, true),
            WM_MBUTTONUP => (MouseButton::Middle, false),
            WM_XBUTTONDOWN => (
                if high_word(mouse_data) == 1 {
                    MouseButton::Back
                } else {
                    MouseButton::Forward
                },
                true,
            ),
            WM_XBUTTONUP => (
                if high_word(mouse_data) == 1 {
                    MouseButton::Back
                } else {
                    MouseButton::Forward
                },
                false,
            ),
            _ => return None,
        };
        Some(change)
    }

    fn send_input(state: &mut HookState, event: InputEvent, reliable: bool) {
        let movement = matches!(event, InputEvent::MouseMove { .. });
        if state
            .events
            .try_send(PlatformEvent::InputCaptured(event))
            .is_err()
        {
            if reliable {
                state.stats.reliable_queue_failures =
                    state.stats.reliable_queue_failures.saturating_add(1);
                tracing::error!(
                    component = "input",
                    "reliable Windows input could not enter the platform queue"
                );
                state.stats.log_and_reset("reliable_queue_full");
                state.suppressed.store(false, Ordering::SeqCst);
                let _ = state.events.try_send(PlatformEvent::EmergencyReturn);
            } else if movement {
                state.stats.mouse_move_queue_drops =
                    state.stats.mouse_move_queue_drops.saturating_add(1);
            }
        }
    }

    fn button_action(down: bool) -> KeyAction {
        if down { KeyAction::Down } else { KeyAction::Up }
    }

    fn high_word(value: u32) -> u16 {
        ((value >> 16) & 0xffff) as u16
    }

    fn emergency_hotkey(pressed: &HashSet<u16>) -> bool {
        let control = pressed.contains(&VK_CONTROL)
            || pressed.contains(&VK_LCONTROL)
            || pressed.contains(&VK_RCONTROL);
        let alt = pressed.contains(&VK_MENU)
            || pressed.contains(&VK_LMENU)
            || pressed.contains(&VK_RMENU);
        let shift = pressed.contains(&VK_SHIFT)
            || pressed.contains(&VK_LSHIFT)
            || pressed.contains(&VK_RSHIFT);
        control && alt && shift && pressed.contains(&VK_F12)
    }

    fn is_modifier_usage(usage: kvmflow_domain::KeyUsage) -> bool {
        matches!(
            usage,
            kvmflow_domain::KeyUsage::LEFT_CONTROL
                | kvmflow_domain::KeyUsage::RIGHT_CONTROL
                | kvmflow_domain::KeyUsage::LEFT_SHIFT
                | kvmflow_domain::KeyUsage::RIGHT_SHIFT
                | kvmflow_domain::KeyUsage::LEFT_ALT
                | kvmflow_domain::KeyUsage::RIGHT_ALT
                | kvmflow_domain::KeyUsage::LEFT_GUI
                | kvmflow_domain::KeyUsage::RIGHT_GUI
        )
    }
}

#[cfg(windows)]
use win32::Win32HookControl;

#[derive(Debug, thiserror::Error)]
pub enum WindowsInputAdapterError {
    #[error("Windows input hooks are unavailable on this platform")]
    Unavailable,
    #[error("failed to start the Windows input hook thread")]
    StartFailed,
    #[error("failed to install Windows low-level input hooks")]
    InstallFailed,
    #[error("failed to create the Windows raw-input message window")]
    RawInputWindowFailed,
    #[error("failed to register the Windows raw-input mouse device")]
    RawInputRegistrationFailed,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn maps_windows_modifiers_to_macos_shortcut_conventions() {
        assert_eq!(
            virtual_key_to_remote_usage(VK_LCONTROL),
            Some(KeyUsage::LEFT_GUI)
        );
        assert_eq!(
            virtual_key_to_remote_usage(VK_LWIN),
            Some(KeyUsage::LEFT_CONTROL)
        );
        assert_eq!(
            virtual_key_to_remote_usage(VK_LMENU),
            Some(KeyUsage::LEFT_ALT)
        );
        assert_eq!(
            virtual_key_to_remote_usage(VK_LSHIFT),
            Some(KeyUsage::LEFT_SHIFT)
        );
    }

    #[test]
    fn maps_letters_navigation_and_f12() {
        assert_eq!(virtual_key_to_remote_usage(VK_A), Some(KeyUsage::A));
        assert_eq!(virtual_key_to_remote_usage(VK_Z), Some(KeyUsage::Z));
        assert_eq!(
            virtual_key_to_remote_usage(VK_LEFT),
            Some(KeyUsage::LEFT_ARROW)
        );
        assert_eq!(virtual_key_to_remote_usage(VK_F12), Some(KeyUsage::F12));
    }

    #[test]
    fn maps_number_row_to_usb_hid_order() {
        for (virtual_key, usage) in (0x31..=VK_9).zip(0x1e..=0x26) {
            assert_eq!(
                virtual_key_to_remote_usage(virtual_key),
                Some(KeyUsage(usage))
            );
        }
        assert_eq!(virtual_key_to_remote_usage(VK_0), Some(KeyUsage(0x27)));
    }

    #[test]
    fn maps_caps_lock_forward_delete_and_right_modifiers() {
        assert_eq!(
            virtual_key_to_remote_usage(VK_CAPITAL),
            Some(KeyUsage(0x39))
        );
        assert_eq!(virtual_key_to_remote_usage(VK_DELETE), Some(KeyUsage(0x4c)));
        assert_eq!(
            virtual_key_to_remote_usage(VK_RCONTROL),
            Some(KeyUsage::RIGHT_GUI)
        );
        assert_eq!(
            virtual_key_to_remote_usage(VK_RSHIFT),
            Some(KeyUsage::RIGHT_SHIFT)
        );
        assert_eq!(
            virtual_key_to_remote_usage(VK_RMENU),
            Some(KeyUsage::RIGHT_ALT)
        );
        assert_eq!(
            virtual_key_to_remote_usage(VK_RWIN),
            Some(KeyUsage::RIGHT_CONTROL)
        );
    }

    #[test]
    fn maps_numeric_keypad_and_distinguishes_extended_enter() {
        assert_eq!(
            virtual_key_to_remote_usage(VK_NUMLOCK),
            Some(KeyUsage::NUM_LOCK)
        );
        assert_eq!(
            virtual_key_to_remote_usage(VK_NUMPAD0),
            Some(KeyUsage(0x62))
        );
        assert_eq!(
            virtual_key_to_remote_usage(VK_NUMPAD9),
            Some(KeyUsage(0x61))
        );
        assert_eq!(virtual_key_to_remote_usage(VK_DIVIDE), Some(KeyUsage(0x54)));
        assert_eq!(
            virtual_key_to_remote_usage(VK_MULTIPLY),
            Some(KeyUsage(0x55))
        );
        assert_eq!(
            virtual_key_to_remote_usage(VK_SUBTRACT),
            Some(KeyUsage(0x56))
        );
        assert_eq!(virtual_key_to_remote_usage(VK_ADD), Some(KeyUsage(0x57)));
        assert_eq!(
            virtual_key_to_remote_usage(VK_DECIMAL),
            Some(KeyUsage(0x63))
        );
        assert_eq!(
            virtual_key_to_remote_usage_with_extended(VK_RETURN, false),
            Some(KeyUsage::ENTER)
        );
        assert_eq!(
            virtual_key_to_remote_usage_with_extended(VK_RETURN, true),
            Some(KeyUsage::KEYPAD_ENTER)
        );
    }

    #[test]
    fn relative_raw_pointer_events_preserve_each_delta() {
        let mut state = RawPointerState::default();

        assert_eq!(state.movement(false, 8, -3), Some((8, -3)));
        assert_eq!(state.movement(false, 8, -3), Some((8, -3)));
        assert_eq!(state.movement(false, 0, 0), None);
    }

    #[test]
    fn absolute_raw_pointer_events_seed_then_report_deltas() {
        let mut state = RawPointerState::default();

        assert_eq!(state.movement(true, 100, 200), None);
        assert_eq!(state.movement(true, 108, 197), Some((8, -3)));
        state.reset();
        assert_eq!(state.movement(true, 300, 400), None);
    }

    #[test]
    fn right_edge_triggers_once_and_rearms_after_moving_inward() {
        let bounds = ScreenBounds {
            left: 0,
            top: 0,
            right: 1919,
            bottom: 1000,
        };
        let mut detector = EdgeDetector::new(ScreenEdge::Right);
        detector.set_enabled(true);

        assert_eq!(
            detector.observe(bounds, 1919, 500, false),
            Some(EdgeTrigger {
                edge: ScreenEdge::Right,
                position: 0.5,
            })
        );
        assert_eq!(detector.observe(bounds, 1919, 500, false), None);
        assert_eq!(detector.observe(bounds, 1907, 500, false), None);
        assert!(detector.observe(bounds, 1918, 250, false).is_some());
    }

    #[test]
    fn left_edge_preserves_vertical_position_and_ignores_held_drag() {
        let bounds = ScreenBounds {
            left: 0,
            top: 100,
            right: 1279,
            bottom: 1100,
        };
        let mut detector = EdgeDetector::new(ScreenEdge::Left);
        detector.set_enabled(true);

        assert_eq!(detector.observe(bounds, 0, 350, true), None);
        assert_eq!(
            detector.observe(bounds, 1, 350, false),
            Some(EdgeTrigger {
                edge: ScreenEdge::Left,
                position: 0.25,
            })
        );
    }

    #[test]
    fn bottom_edge_uses_horizontal_position_and_rearms_upward() {
        let bounds = ScreenBounds {
            left: 0,
            top: 0,
            right: 1000,
            bottom: 799,
        };
        let mut detector = EdgeDetector::new(ScreenEdge::Bottom);
        detector.set_enabled(true);

        assert_eq!(
            detector.observe(bounds, 500, 799, false),
            Some(EdgeTrigger {
                edge: ScreenEdge::Bottom,
                position: 0.5,
            })
        );
        assert_eq!(detector.observe(bounds, 500, 799, false), None);
        assert_eq!(detector.observe(bounds, 500, 787, false), None);
        assert!(detector.observe(bounds, 250, 798, false).is_some());
    }

    #[test]
    fn coordinates_on_an_adjacent_windows_display_do_not_trigger() {
        let bounds = ScreenBounds {
            left: 0,
            top: 0,
            right: 1919,
            bottom: 1079,
        };
        let mut right = EdgeDetector::new(ScreenEdge::Right);
        let mut bottom = EdgeDetector::new(ScreenEdge::Bottom);
        right.set_enabled(true);
        bottom.set_enabled(true);

        assert_eq!(right.observe(bounds, 1920, 500, false), None);
        assert_eq!(bottom.observe(bounds, 500, 1080, false), None);
    }

    #[test]
    fn changing_edge_rearms_the_detector_for_the_new_screen_position() {
        let bounds = ScreenBounds {
            left: 0,
            top: 0,
            right: 1000,
            bottom: 800,
        };
        let mut detector = EdgeDetector::new(ScreenEdge::Right);
        detector.set_enabled(true);
        assert!(detector.observe(bounds, 1000, 400, false).is_some());

        detector.set_edge(ScreenEdge::Bottom);

        assert_eq!(
            detector.observe(bounds, 250, 800, false),
            Some(EdgeTrigger {
                edge: ScreenEdge::Bottom,
                position: 0.25,
            })
        );
    }

    #[test]
    fn restoring_local_pointer_uses_the_original_edge_and_height() {
        let bounds = ScreenBounds {
            left: 0,
            top: 0,
            right: 1919,
            bottom: 1000,
        };

        assert_eq!(bounds.restore_point(ScreenEdge::Right, 0.25), (1903, 250));
        assert_eq!(bounds.restore_point(ScreenEdge::Left, 0.75), (16, 750));
        assert_eq!(bounds.restore_point(ScreenEdge::Bottom, 0.25), (480, 984));
        assert_eq!(bounds.restore_point(ScreenEdge::Top, 0.75), (1439, 16));
    }
}

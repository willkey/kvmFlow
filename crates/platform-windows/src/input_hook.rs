use kvmflow_domain::{DisplayTarget, KeyUsage, ScreenEdge};
#[cfg(windows)]
use kvmflow_domain::{InputEvent, KeyAction, MouseButton};
use kvmflow_platform_api::PlatformBridge;
#[cfg(windows)]
use kvmflow_platform_api::{ClipboardAdapter, PlatformCommand, PlatformEvent, platform_bridge};

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
const WINDOWS_SIDE_MODIFIER_VIRTUAL_KEYS: [u16; 8] = [
    VK_LCONTROL,
    VK_RCONTROL,
    VK_LSHIFT,
    VK_RSHIFT,
    VK_LMENU,
    VK_RMENU,
    VK_LWIN,
    VK_RWIN,
];

#[cfg(any(windows, test))]
fn rect_covers_monitor(window: (i32, i32, i32, i32), monitor: (i32, i32, i32, i32)) -> bool {
    const TOLERANCE_PX: i32 = 2;
    (i64::from(window.0) - i64::from(monitor.0)).abs() <= i64::from(TOLERANCE_PX)
        && (i64::from(window.1) - i64::from(monitor.1)).abs() <= i64::from(TOLERANCE_PX)
        && (i64::from(window.2) - i64::from(monitor.2)).abs() <= i64::from(TOLERANCE_PX)
        && (i64::from(window.3) - i64::from(monitor.3)).abs() <= i64::from(TOLERANCE_PX)
}

#[cfg(any(windows, test))]
fn fullscreen_window_style(style: u32, extended_style: u32, class_name: &str) -> bool {
    const WINDOWED_STYLE: u32 = 0x00c0_0000 | 0x0004_0000 | 0x4000_0000;
    const TOOL_WINDOW: u32 = 0x0000_0080;
    style & WINDOWED_STYLE == 0
        && extended_style & TOOL_WINDOW == 0
        && !matches!(
            class_name,
            "Shell_TrayWnd"
                | "Shell_SecondaryTrayWnd"
                | "XamlExplorerHostIslandWindow"
                | "Progman"
                | "WorkerW"
        )
}

#[cfg(any(windows, test))]
fn update_remote_pressed_keys(
    pressed: &mut std::collections::HashSet<KeyUsage>,
    usage: KeyUsage,
    down: bool,
) -> RemoteKeyTransition {
    if down {
        if pressed.insert(usage) {
            RemoteKeyTransition::Forward
        } else if is_modifier_usage(usage) || usage == KeyUsage::CAPS_LOCK {
            RemoteKeyTransition::IgnoreRepeat
        } else {
            RemoteKeyTransition::ForwardRepeat
        }
    } else if pressed.remove(&usage) {
        RemoteKeyTransition::Forward
    } else {
        RemoteKeyTransition::IgnoreRelease
    }
}

#[cfg(any(windows, test))]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum RemoteKeyTransition {
    Forward,
    ForwardRepeat,
    IgnoreRepeat,
    IgnoreRelease,
}

#[cfg(any(windows, test))]
fn is_modifier_usage(usage: KeyUsage) -> bool {
    matches!(
        usage,
        KeyUsage::LEFT_CONTROL
            | KeyUsage::RIGHT_CONTROL
            | KeyUsage::LEFT_SHIFT
            | KeyUsage::RIGHT_SHIFT
            | KeyUsage::LEFT_ALT
            | KeyUsage::RIGHT_ALT
            | KeyUsage::LEFT_GUI
            | KeyUsage::RIGHT_GUI
    )
}

#[cfg(any(windows, test))]
fn update_locally_visible_modifier(
    visible: &mut std::collections::HashSet<u16>,
    virtual_key: u16,
    intercepted: bool,
    down: bool,
) {
    if intercepted || !is_windows_modifier_virtual_key(virtual_key) {
        return;
    }
    if down {
        visible.insert(virtual_key);
    } else {
        visible.remove(&virtual_key);
    }
}

#[cfg(any(windows, test))]
fn observe_local_modifier_event(
    visible: &mut std::collections::HashSet<u16>,
    pressed: &mut std::collections::HashSet<u16>,
    virtual_key: u16,
    down: bool,
) {
    if !is_windows_modifier_virtual_key(virtual_key) {
        return;
    }
    update_locally_visible_modifier(visible, virtual_key, false, down);
    if down {
        pressed.insert(virtual_key);
    } else {
        pressed.remove(&virtual_key);
    }
}

#[cfg(any(windows, test))]
fn seed_locally_visible_modifiers(
    visible: &mut std::collections::HashSet<u16>,
    pressed: &mut std::collections::HashSet<u16>,
    down_virtual_keys: impl IntoIterator<Item = u16>,
) -> usize {
    let previous_count = visible.len();
    for virtual_key in down_virtual_keys {
        observe_local_modifier_event(visible, pressed, virtual_key, true);
    }
    visible.len().saturating_sub(previous_count)
}

#[cfg(any(windows, test))]
fn local_modifier_repair_plan(
    visible: &std::collections::HashSet<u16>,
    system_down: impl IntoIterator<Item = u16>,
) -> Vec<u16> {
    let mut candidates = visible.clone();
    candidates.extend(system_down);
    let mut release = candidates.into_iter().collect::<Vec<_>>();
    release.sort_unstable();
    release
}

#[cfg(any(windows, test))]
fn modifier_release_is_extended(virtual_key: u16) -> bool {
    matches!(virtual_key, VK_RCONTROL | VK_RMENU | VK_LWIN | VK_RWIN)
}

#[cfg(any(windows, test))]
fn modifier_virtual_key_name(virtual_key: u16) -> Option<&'static str> {
    match virtual_key {
        VK_LCONTROL => Some("left_control"),
        VK_RCONTROL => Some("right_control"),
        VK_LSHIFT => Some("left_shift"),
        VK_RSHIFT => Some("right_shift"),
        VK_LMENU => Some("left_alt"),
        VK_RMENU => Some("right_alt"),
        VK_LWIN => Some("left_windows"),
        VK_RWIN => Some("right_windows"),
        _ => None,
    }
}

#[cfg(any(windows, test))]
fn modifier_virtual_key_names(virtual_keys: impl IntoIterator<Item = u16>) -> Vec<&'static str> {
    let mut names = virtual_keys
        .into_iter()
        .filter_map(modifier_virtual_key_name)
        .collect::<Vec<_>>();
    names.sort_unstable();
    names
}

#[cfg(any(windows, test))]
const RECENT_KEYBOARD_HOOK_EVENT_LIMIT: usize = 64;
#[cfg(any(windows, test))]
const RECENT_KEYBOARD_HOOK_EVENT_WINDOW: std::time::Duration =
    std::time::Duration::from_millis(250);

#[cfg(any(windows, test))]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct KeyboardEventSignature {
    virtual_key: u16,
    scan_code: u16,
    extended: bool,
    down: bool,
}

#[cfg(any(windows, test))]
#[derive(Default)]
struct RecentKeyboardHookEvents {
    events: std::collections::VecDeque<(KeyboardEventSignature, std::time::Instant)>,
}

#[cfg(any(windows, test))]
impl RecentKeyboardHookEvents {
    fn record(&mut self, event: KeyboardEventSignature, now: std::time::Instant) {
        self.discard_expired(now);
        if self.events.len() >= RECENT_KEYBOARD_HOOK_EVENT_LIMIT {
            self.events.pop_front();
        }
        self.events.push_back((event, now));
    }

    fn consume(&mut self, event: KeyboardEventSignature, now: std::time::Instant) -> bool {
        self.discard_expired(now);
        let Some(index) = self
            .events
            .iter()
            .position(|(candidate, _)| *candidate == event)
        else {
            return false;
        };
        self.events.remove(index);
        true
    }

    #[cfg(windows)]
    fn clear(&mut self) {
        self.events.clear();
    }

    fn discard_expired(&mut self, now: std::time::Instant) {
        while self.events.front().is_some_and(|(_, observed_at)| {
            now.saturating_duration_since(*observed_at) > RECENT_KEYBOARD_HOOK_EVENT_WINDOW
        }) {
            self.events.pop_front();
        }
    }
}

#[cfg(any(windows, test))]
fn is_windows_modifier_virtual_key(virtual_key: u16) -> bool {
    matches!(
        virtual_key,
        VK_LCONTROL | VK_RCONTROL | VK_LSHIFT | VK_RSHIFT | VK_LMENU | VK_RMENU | VK_LWIN | VK_RWIN
    )
}

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
    blocked: bool,
    latched: bool,
}

#[cfg(any(windows, test))]
impl EdgeDetector {
    fn new(edge: ScreenEdge) -> Self {
        Self {
            edge,
            enabled: false,
            blocked: false,
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

    fn set_blocked(&mut self, blocked: bool) {
        self.blocked = blocked;
        if blocked {
            self.latched = false;
        }
    }

    #[cfg(windows)]
    fn rearm(&mut self) {
        self.latched = false;
    }

    fn observe(
        &mut self,
        bounds: ScreenBounds,
        x: i32,
        y: i32,
        button_held: bool,
    ) -> Option<EdgeTrigger> {
        if !self.enabled || self.blocked || !bounds.contains_edge_position(self.edge, x, y) {
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

#[cfg(any(windows, test))]
#[derive(Debug, Clone, PartialEq, Eq)]
struct WindowsDisplay {
    target: DisplayTarget,
    bounds: ScreenBounds,
}

#[cfg(any(windows, test))]
fn select_display<'a>(
    displays: &'a [WindowsDisplay],
    preferred_id: Option<&str>,
) -> Option<&'a WindowsDisplay> {
    preferred_id
        .and_then(|preferred_id| {
            displays
                .iter()
                .find(|display| display.target.id == preferred_id)
        })
        .or_else(|| displays.iter().find(|display| display.target.primary))
        .or_else(|| displays.first())
}

#[cfg(any(windows, test))]
fn reportable_target_display_id<'a>(
    preferred_id: Option<&str>,
    selected: &'a WindowsDisplay,
) -> Option<&'a str> {
    preferred_id
        .is_none_or(|preferred_id| preferred_id == selected.target.id)
        .then_some(selected.target.id.as_str())
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

#[cfg(any(windows, test))]
fn normalize_windows_virtual_key(virtual_key: u16, scan_code: u16, extended: bool) -> u16 {
    match virtual_key {
        VK_SHIFT if scan_code == 0x36 => VK_RSHIFT,
        VK_SHIFT => VK_LSHIFT,
        VK_CONTROL if extended => VK_RCONTROL,
        VK_CONTROL => VK_LCONTROL,
        VK_MENU if extended => VK_RMENU,
        VK_MENU => VK_LMENU,
        _ => virtual_key,
    }
}

#[cfg(any(windows, test))]
fn keyboard_semantic_name(usage: KeyUsage) -> Option<&'static str> {
    match usage {
        KeyUsage::ENTER => Some("enter"),
        KeyUsage::BACKSPACE => Some("backspace"),
        KeyUsage::HOME => Some("home"),
        KeyUsage::PAGE_UP => Some("page_up"),
        KeyUsage::FORWARD_DELETE => Some("forward_delete"),
        KeyUsage::END => Some("end"),
        KeyUsage::PAGE_DOWN => Some("page_down"),
        KeyUsage::RIGHT_ARROW => Some("arrow_right"),
        KeyUsage::LEFT_ARROW => Some("arrow_left"),
        KeyUsage::DOWN_ARROW => Some("arrow_down"),
        KeyUsage::UP_ARROW => Some("arrow_up"),
        KeyUsage::LEFT_CONTROL => Some("control_left"),
        KeyUsage::RIGHT_CONTROL => Some("control_right"),
        KeyUsage::LEFT_SHIFT => Some("shift_left"),
        KeyUsage::RIGHT_SHIFT => Some("shift_right"),
        KeyUsage::LEFT_ALT => Some("option_left"),
        KeyUsage::RIGHT_ALT => Some("option_right"),
        KeyUsage::LEFT_GUI => Some("command_left"),
        KeyUsage::RIGHT_GUI => Some("command_right"),
        KeyUsage::CAPS_LOCK => Some("caps_lock"),
        _ => None,
    }
}

#[cfg(windows)]
pub fn spawn_windows_platform_adapter(
    edge: ScreenEdge,
    target_display_id: Option<String>,
) -> Result<PlatformBridge, WindowsInputAdapterError> {
    let (mut driver, bridge) = platform_bridge(PLATFORM_CHANNEL_CAPACITY);
    let control = Win32HookControl::start(driver.events.clone(), edge, target_display_id)?;
    let clipboard = ClipboardAdapter::start(driver.events.clone()).inspect_err(|error| {
        let _ = driver
            .events
            .try_send(PlatformEvent::ClipboardFailed(error.to_string()));
    });
    tokio::spawn(async move {
        let mut control = control;
        let clipboard = clipboard.ok();
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
                PlatformCommand::SetEdgeSwitchDisplay(display_id) => {
                    control.set_edge_switch_display(&display_id);
                }
                PlatformCommand::BeginEdgeTransition { edge, position } => {
                    control.begin_edge_transition(edge, position);
                }
                PlatformCommand::RestoreLocalPointer { position } => {
                    control.restore_local_pointer(position);
                }
                PlatformCommand::ResetInput => {
                    control.reset_input();
                }
                PlatformCommand::ApplyClipboard(payload) => {
                    if let Some(clipboard) = clipboard.as_ref()
                        && let Err(error) = clipboard.apply(payload)
                    {
                        let _ = driver
                            .events
                            .send(PlatformEvent::ClipboardFailed(error.to_string()))
                            .await;
                    }
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
    _target_display_id: Option<String>,
) -> Result<PlatformBridge, WindowsInputAdapterError> {
    Err(WindowsInputAdapterError::Unavailable)
}

#[cfg(windows)]
pub fn available_windows_displays() -> Result<Vec<DisplayTarget>, WindowsInputAdapterError> {
    win32::available_displays()
}

#[cfg(not(windows))]
pub fn available_windows_displays() -> Result<Vec<DisplayTarget>, WindowsInputAdapterError> {
    Err(WindowsInputAdapterError::Unavailable)
}

#[cfg(windows)]
mod win32 {
    use std::{
        collections::{HashSet, VecDeque},
        ptr,
        sync::{
            Arc, Mutex, OnceLock,
            atomic::{AtomicBool, AtomicU8, Ordering},
            mpsc,
        },
        time::{Duration, Instant},
    };

    use kvmflow_platform_api::PlatformEvent;
    use tokio::sync::mpsc::Sender;
    use windows_sys::Win32::{
        Foundation::{CloseHandle, HWND, LPARAM, LRESULT, POINT, RECT, WPARAM},
        Graphics::{
            Dwm::{DWMWA_EXTENDED_FRAME_BOUNDS, DwmGetWindowAttribute},
            Gdi::{
                ClientToScreen, EnumDisplayMonitors, GetMonitorInfoW, HDC, HMONITOR,
                MONITOR_DEFAULTTONEAREST, MONITORINFO, MONITORINFOEXW, MonitorFromWindow,
            },
        },
        Media::{TIMERR_NOERROR, timeBeginPeriod, timeEndPeriod},
        System::{
            LibraryLoader::GetModuleHandleW,
            Threading::{
                GetCurrentThreadId, OpenProcess, PROCESS_QUERY_LIMITED_INFORMATION,
                QueryFullProcessImageNameW,
            },
        },
        UI::{
            Accessibility::{HWINEVENTHOOK, SetWinEventHook, UnhookWinEvent},
            Input::{
                GetRawInputData,
                KeyboardAndMouse::{
                    GetAsyncKeyState, INPUT, INPUT_KEYBOARD, KEYBDINPUT, KEYEVENTF_EXTENDEDKEY,
                    KEYEVENTF_KEYUP, SendInput,
                },
                MOUSE_MOVE_ABSOLUTE, MOUSE_VIRTUAL_DESKTOP, RAWINPUT, RAWINPUTDEVICE,
                RAWINPUTHEADER, RID_INPUT, RIDEV_INPUTSINK, RIDEV_REMOVE, RIM_TYPEKEYBOARD,
                RIM_TYPEMOUSE, RegisterRawInputDevices,
            },
            WindowsAndMessaging::{
                CallNextHookEx, CreateWindowExW, DefWindowProcW, DestroyWindow, DispatchMessageW,
                EVENT_OBJECT_LOCATIONCHANGE, EVENT_SYSTEM_FOREGROUND, GWL_EXSTYLE, GWL_STYLE,
                GetClassNameW, GetClientRect, GetDesktopWindow, GetForegroundWindow, GetMessageW,
                GetShellWindow, GetSystemMetrics, GetWindowLongPtrW, GetWindowRect,
                GetWindowThreadProcessId, IsIconic, IsWindowVisible, KBDLLHOOKSTRUCT,
                LLKHF_EXTENDED, LLKHF_INJECTED, LLKHF_LOWER_IL_INJECTED, MONITORINFOF_PRIMARY, MSG,
                MSLLHOOKSTRUCT, PBT_APMRESUMEAUTOMATIC, PostThreadMessageW, RI_KEY_BREAK,
                RI_KEY_E0, RegisterClassW, SM_CXSCREEN, SM_CXVIRTUALSCREEN, SM_CYSCREEN,
                SM_CYVIRTUALSCREEN, SetCursorPos, SetWindowsHookExW, TranslateMessage,
                UnhookWindowsHookEx, UnregisterClassW, WH_KEYBOARD_LL, WH_MOUSE_LL,
                WINEVENT_OUTOFCONTEXT, WM_DISPLAYCHANGE, WM_INPUT, WM_KEYDOWN, WM_KEYUP,
                WM_LBUTTONDOWN, WM_LBUTTONUP, WM_MBUTTONDOWN, WM_MBUTTONUP, WM_MOUSEHWHEEL,
                WM_MOUSEMOVE, WM_MOUSEWHEEL, WM_POWERBROADCAST, WM_QUIT, WM_RBUTTONDOWN,
                WM_RBUTTONUP, WM_SYSKEYDOWN, WM_SYSKEYUP, WM_XBUTTONDOWN, WM_XBUTTONUP, WNDCLASSW,
            },
        },
    };

    use super::{
        ActiveEdgeTransition, DisplayTarget, EdgeDetector, InputEvent, KeyAction,
        KeyboardEventSignature, MouseButton, RawPointerState, RecentKeyboardHookEvents,
        RemoteKeyTransition, ScreenBounds, ScreenEdge, VK_CONTROL, VK_F12, VK_LCONTROL, VK_LMENU,
        VK_LSHIFT, VK_MENU, VK_RCONTROL, VK_RMENU, VK_RSHIFT, VK_SHIFT,
        WINDOWS_SIDE_MODIFIER_VIRTUAL_KEYS, WindowsDisplay, WindowsInputAdapterError,
        fullscreen_window_style, is_modifier_usage, keyboard_semantic_name,
        local_modifier_repair_plan, modifier_release_is_extended, modifier_virtual_key_name,
        modifier_virtual_key_names, normalize_windows_virtual_key, observe_local_modifier_event,
        rect_covers_monitor, reportable_target_display_id, seed_locally_visible_modifiers,
        select_display, update_locally_visible_modifier, update_remote_pressed_keys,
        virtual_key_to_remote_usage_with_extended,
    };

    const HID_USAGE_PAGE_GENERIC: u16 = 0x01;
    const HID_USAGE_GENERIC_MOUSE: u16 = 0x02;
    const HID_USAGE_GENERIC_KEYBOARD: u16 = 0x06;
    const EMERGENCY_EDGE_COOLDOWN: Duration = Duration::from_secs(3);
    const PENDING_KEY_EVENT_LIMIT: usize = 64;
    const REMOTE_INPUT_TIMER_RESOLUTION_MS: u32 = 1;
    const WM_REARM_LOW_LEVEL_HOOKS: u32 = 0x8001;
    const REARM_REASON_CONTROL_TRANSITION: usize = 0;
    const REARM_REASON_RAW_KEYBOARD_FALLBACK: usize = 1;
    const REARM_REASON_MANUAL_RESET: usize = 2;
    const MODIFIER_REPAIR_INPUT_TAG: usize = 0x4b564d52;
    // Skip the mutex-heavy mouse path when neither KVM capture nor edge detection needs it.
    const MOUSE_ACTIVITY_REMOTE_CAPTURE: u8 = 1 << 0;
    const MOUSE_ACTIVITY_EDGE_DETECTION: u8 = 1 << 1;
    static MOUSE_ACTIVITY: AtomicU8 = AtomicU8::new(0);

    fn set_mouse_activity(flag: u8, active: bool) {
        if active {
            MOUSE_ACTIVITY.fetch_or(flag, Ordering::SeqCst);
        } else {
            MOUSE_ACTIVITY.fetch_and(!flag, Ordering::SeqCst);
        }
    }

    pub(super) fn available_displays() -> Result<Vec<DisplayTarget>, WindowsInputAdapterError> {
        enumerate_displays()
            .map(|displays| displays.into_iter().map(|display| display.target).collect())
    }

    fn enumerate_displays() -> Result<Vec<WindowsDisplay>, WindowsInputAdapterError> {
        let mut displays: Vec<WindowsDisplay> = Vec::new();
        let result = unsafe {
            EnumDisplayMonitors(
                std::ptr::null_mut(),
                std::ptr::null(),
                Some(collect_display),
                &mut displays as *mut Vec<WindowsDisplay> as LPARAM,
            )
        };
        if result == 0 || displays.is_empty() {
            return Err(WindowsInputAdapterError::DisplayEnumerationFailed);
        }
        displays.sort_by(|left, right| {
            right
                .target
                .primary
                .cmp(&left.target.primary)
                .then_with(|| left.target.id.cmp(&right.target.id))
        });
        Ok(displays)
    }

    unsafe extern "system" fn collect_display(
        monitor: HMONITOR,
        _device_context: HDC,
        _monitor_rect: *mut RECT,
        data: LPARAM,
    ) -> i32 {
        let displays = unsafe { &mut *(data as *mut Vec<WindowsDisplay>) };
        let mut info = MONITORINFOEXW::default();
        info.monitorInfo.cbSize = std::mem::size_of::<MONITORINFOEXW>() as u32;
        if unsafe { GetMonitorInfoW(monitor, &mut info.monitorInfo as *mut MONITORINFO) } == 0 {
            return 1;
        }
        let device_name_length = info
            .szDevice
            .iter()
            .position(|character| *character == 0)
            .unwrap_or(info.szDevice.len());
        let id = String::from_utf16_lossy(&info.szDevice[..device_name_length]);
        let rect = info.monitorInfo.rcMonitor;
        let width = (rect.right - rect.left).max(1) as u32;
        let height = (rect.bottom - rect.top).max(1) as u32;
        displays.push(WindowsDisplay {
            target: DisplayTarget {
                id,
                primary: info.monitorInfo.dwFlags & MONITORINFOF_PRIMARY != 0,
                left: rect.left,
                top: rect.top,
                width,
                height,
            },
            bounds: ScreenBounds {
                left: rect.left,
                top: rect.top,
                right: rect.right - 1,
                bottom: rect.bottom - 1,
            },
        });
        1
    }

    pub(super) struct Win32HookControl {
        thread_id: u32,
        completion: mpsc::Receiver<()>,
        join: Option<std::thread::JoinHandle<()>>,
        suppressed: Arc<AtomicBool>,
        timer_resolution_active: bool,
    }

    impl Win32HookControl {
        pub(super) fn start(
            events: Sender<PlatformEvent>,
            edge: ScreenEdge,
            target_display_id: Option<String>,
        ) -> Result<Self, WindowsInputAdapterError> {
            MOUSE_ACTIVITY.store(0, Ordering::SeqCst);
            let (startup_tx, startup_rx) = mpsc::channel();
            let (completion_tx, completion_rx) = mpsc::channel();
            let suppressed = Arc::new(AtomicBool::new(false));
            let thread_suppressed = suppressed.clone();
            let join = std::thread::Builder::new()
                .name("kvmflow-win32-input".into())
                .spawn(move || {
                    run_hook_thread(
                        events,
                        thread_suppressed,
                        edge,
                        target_display_id,
                        startup_tx,
                    );
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
                timer_resolution_active: false,
            })
        }

        pub(super) fn set_suppressed(&mut self, suppressed: bool) {
            let was_suppressed = self.suppressed.swap(suppressed, Ordering::SeqCst);
            if suppressed && !was_suppressed {
                self.enable_remote_input_timer_resolution();
            } else if !suppressed && self.timer_resolution_active {
                self.disable_remote_input_timer_resolution();
            }
            set_mouse_activity(MOUSE_ACTIVITY_REMOTE_CAPTURE, suppressed);
            let mut restore_point = None;
            let mut modifiers_to_release = Vec::new();
            if let Ok(mut state) = hook_state().lock()
                && let Some(state) = state.as_mut()
            {
                if suppressed && !was_suppressed {
                    if !state.transition_pending {
                        state.start_keyboard_capture();
                    }
                    tracing::info!(
                        component = "input",
                        stage = "windows_capture",
                        capture_source = "low_level_hook",
                        timer_resolution_ms = if self.timer_resolution_active {
                            REMOTE_INPUT_TIMER_RESOLUTION_MS
                        } else {
                            0
                        },
                        "Windows remote input capture armed"
                    );
                } else if !suppressed && state.remote_active {
                    state.stats.log_and_reset("control_ended");
                }
                if suppressed {
                    state.remote_active = true;
                    state.transition_pending = false;
                    state.flush_pending_keyboard_events();
                } else {
                    if was_suppressed || state.remote_active || state.transition_pending {
                        let system_down = current_system_down_modifiers();
                        modifiers_to_release = local_modifier_repair_plan(
                            &state.locally_visible_modifiers,
                            system_down.iter().copied(),
                        );
                        tracing::info!(
                            component = "input",
                            stage = "windows_modifier_audit",
                            local_visible = ?modifier_virtual_key_names(
                                state.locally_visible_modifiers.iter().copied()
                            ),
                            hook_pressed = ?modifier_virtual_key_names(
                                state.pressed_keys.iter().copied()
                            ),
                            system_down = ?modifier_virtual_key_names(system_down.iter().copied()),
                            release = ?modifier_virtual_key_names(
                                modifiers_to_release.iter().copied()
                            ),
                            "Windows modifier state audited before local restore"
                        );
                        state.locally_visible_modifiers.clear();
                    }
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
                    state.pending_key_events.clear();
                    state.recent_keyboard_hook_events.clear();
                    state.raw_keyboard_fallback_active = false;
                    state.remote_started_at = None;
                }
            }
            if let Some((x, y)) = restore_point {
                unsafe {
                    SetCursorPos(x, y);
                }
            }
            release_local_modifiers(&modifiers_to_release, "control_ended");
        }

        pub(super) fn reset_input(&mut self) {
            self.set_suppressed(false);
            if let Ok(mut guard) = hook_state().lock()
                && let Some(state) = guard.as_mut()
            {
                state.held_mouse_buttons.clear();
                state.raw_observed_modifiers.clear();
                state.edge_detector.rearm();
                state.edge_switch_cooldown_until = Some(Instant::now() + EMERGENCY_EDGE_COOLDOWN);
            }
            release_local_modifiers(&current_system_down_modifiers(), "manual_input_reset");
            if unsafe {
                PostThreadMessageW(
                    self.thread_id,
                    WM_REARM_LOW_LEVEL_HOOKS,
                    REARM_REASON_MANUAL_RESET,
                    0,
                )
            } == 0
            {
                tracing::warn!(
                    component = "input",
                    "Windows input reset could not request hook reinstallation"
                );
            }
        }

        fn enable_remote_input_timer_resolution(&mut self) {
            let result = unsafe { timeBeginPeriod(REMOTE_INPUT_TIMER_RESOLUTION_MS) };
            if result == TIMERR_NOERROR {
                self.timer_resolution_active = true;
            } else {
                tracing::warn!(
                    component = "input",
                    stage = "windows_timer_resolution",
                    result,
                    "failed to enable high-resolution timer for remote input"
                );
            }
        }

        fn disable_remote_input_timer_resolution(&mut self) {
            let result = unsafe { timeEndPeriod(REMOTE_INPUT_TIMER_RESOLUTION_MS) };
            if result == TIMERR_NOERROR {
                self.timer_resolution_active = false;
            } else {
                tracing::warn!(
                    component = "input",
                    stage = "windows_timer_resolution",
                    result,
                    "failed to disable high-resolution timer for remote input"
                );
            }
        }

        pub(super) fn set_edge_switch_enabled(&mut self, enabled: bool) {
            set_mouse_activity(MOUSE_ACTIVITY_EDGE_DETECTION, enabled);
            if let Ok(mut state) = hook_state().lock()
                && let Some(state) = state.as_mut()
            {
                if state.edge_detector.enabled == enabled {
                    return;
                }
                state.refresh_target_display();
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
                state.refresh_target_display();
            }
        }

        pub(super) fn set_edge_switch_display(&mut self, display_id: &str) {
            if let Ok(mut state) = hook_state().lock()
                && let Some(state) = state.as_mut()
            {
                state.select_target_display(Some(display_id));
            }
        }

        pub(super) fn begin_edge_transition(&mut self, edge: ScreenEdge, position: f32) {
            unsafe {
                PostThreadMessageW(
                    self.thread_id,
                    WM_REARM_LOW_LEVEL_HOOKS,
                    REARM_REASON_CONTROL_TRANSITION,
                    0,
                );
            }
            if let Ok(mut state) = hook_state().lock()
                && let Some(state) = state.as_mut()
            {
                if !state.transition_pending {
                    state.seed_current_local_modifiers();
                    state.start_keyboard_capture();
                    state.transition_pending = true;
                }
                state.active_transition = Some(ActiveEdgeTransition {
                    edge,
                    return_position: position.clamp(0.0, 1.0),
                });
                state.prepend_held_modifier_events();
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
        locally_visible_modifiers: HashSet<u16>,
        raw_observed_modifiers: HashSet<u16>,
        pending_key_events: VecDeque<InputEvent>,
        raw_pointer: RawPointerState,
        raw_input_observed: bool,
        keyboard_hook_observed: bool,
        recent_keyboard_hook_events: RecentKeyboardHookEvents,
        raw_keyboard_fallback_active: bool,
        target_display_id: String,
        active_display_id: String,
        screen_bounds: ScreenBounds,
        edge_detector: EdgeDetector,
        transition_pending: bool,
        active_transition: Option<ActiveEdgeTransition>,
        remote_active: bool,
        held_mouse_buttons: HashSet<MouseButton>,
        edge_switch_cooldown_until: Option<Instant>,
        stats: CaptureSessionStats,
        remote_started_at: Option<Instant>,
        foreground_fullscreen: bool,
        last_diagnostic_foreground: usize,
    }

    impl HookState {
        fn seed_current_local_modifiers(&mut self) {
            let down_virtual_keys = WINDOWS_SIDE_MODIFIER_VIRTUAL_KEYS
                .into_iter()
                .filter(|virtual_key| unsafe { GetAsyncKeyState(i32::from(*virtual_key)) } < 0);
            let seeded = seed_locally_visible_modifiers(
                &mut self.locally_visible_modifiers,
                &mut self.pressed_keys,
                down_virtual_keys,
            );
            if seeded > 0 {
                tracing::info!(
                    component = "input",
                    modifier_count = seeded,
                    "Windows local modifier state seeded before remote control"
                );
            }
        }

        fn start_keyboard_capture(&mut self) {
            self.stats = CaptureSessionStats::default();
            self.remote_pressed_keys.clear();
            self.pending_key_events.clear();
            self.keyboard_hook_observed = false;
            self.recent_keyboard_hook_events.clear();
            self.raw_keyboard_fallback_active = false;
            self.remote_started_at = Some(Instant::now());
        }

        fn prepend_held_modifier_events(&mut self) {
            let mut held_modifiers: Vec<_> = self
                .pressed_keys
                .iter()
                .filter_map(|virtual_key| {
                    virtual_key_to_remote_usage_with_extended(*virtual_key, false)
                })
                .filter(|usage| is_modifier_usage(*usage))
                .filter(|usage| !self.remote_pressed_keys.contains(usage))
                .collect();
            held_modifiers.sort_by_key(|usage| usage.0);
            for usage in held_modifiers.into_iter().rev() {
                self.remote_pressed_keys.insert(usage);
                self.pending_key_events.push_front(InputEvent::Key {
                    usage,
                    action: KeyAction::Down,
                });
            }
        }

        fn queue_pending_key_event(&mut self, event: InputEvent) {
            if self.pending_key_events.len() >= PENDING_KEY_EVENT_LIMIT {
                self.stats.reliable_queue_failures =
                    self.stats.reliable_queue_failures.saturating_add(1);
                self.suppressed.store(false, Ordering::SeqCst);
                set_mouse_activity(MOUSE_ACTIVITY_REMOTE_CAPTURE, false);
                self.transition_pending = false;
                self.pending_key_events.clear();
                self.remote_pressed_keys.clear();
                let _ = self
                    .events
                    .try_send(PlatformEvent::EmergencyReturn { response_ms: None });
                return;
            }
            self.pending_key_events.push_back(event);
        }

        fn flush_pending_keyboard_events(&mut self) {
            while let Some(event) = self.pending_key_events.pop_front() {
                send_input(self, event, true);
                if !self.suppressed.load(Ordering::SeqCst) {
                    self.pending_key_events.clear();
                    self.remote_pressed_keys.clear();
                    break;
                }
            }
        }

        fn select_target_display(&mut self, preferred_id: Option<&str>) {
            let Ok(displays) = enumerate_displays() else {
                tracing::warn!(component = "display", "failed to refresh Windows displays");
                return;
            };
            let Some(display) = select_display(&displays, preferred_id) else {
                tracing::warn!(component = "display", "Windows reported no active displays");
                return;
            };
            let reportable_display_id = reportable_target_display_id(preferred_id, display);
            if reportable_display_id.is_none() {
                let fallback_display = &display.target.id;
                tracing::warn!(
                    component = "display",
                    requested_display = preferred_id,
                    fallback_display = %fallback_display,
                    "requested Windows display is unavailable; using fallback"
                );
            }
            let edge_blocked = display.target.edge_is_occupied(
                self.edge_detector.edge,
                displays.iter().map(|display| &display.target),
            );
            let configured_display_changed = reportable_display_id
                .is_some_and(|display_id| self.target_display_id != display_id);
            let topology_changed = self.active_display_id != display.target.id
                || self.screen_bounds != display.bounds
                || self.edge_detector.blocked != edge_blocked;
            if let Some(display_id) = reportable_display_id {
                self.target_display_id = display_id.to_owned();
            }
            self.active_display_id.clone_from(&display.target.id);
            self.screen_bounds = display.bounds;
            self.edge_detector.set_blocked(edge_blocked);
            self.edge_detector.rearm();
            if !topology_changed && !configured_display_changed {
                return;
            }
            let refreshed_display_id = display.target.id.as_str();
            tracing::info!(
                component = "display",
                display_id = %refreshed_display_id,
                edge = ?self.edge_detector.edge,
                edge_blocked,
                "Windows target display refreshed"
            );
            if configured_display_changed {
                let _ = self.events.try_send(PlatformEvent::TargetDisplayChanged {
                    display_id: display.target.id.clone(),
                });
            }
        }

        fn refresh_target_display(&mut self) {
            let target_display_id = self.target_display_id.clone();
            self.select_target_display(Some(&target_display_id));
        }
    }

    #[derive(Debug, Default)]
    struct CaptureSessionStats {
        key_down_events: u64,
        key_up_events: u64,
        modifier_down_events: u64,
        modifier_up_events: u64,
        editing_key_down_events: u64,
        repeated_key_down_events: u64,
        ignored_key_up_events: u64,
        unsupported_key_events: u64,
        mouse_button_events: u64,
        back_button_events: u64,
        forward_button_events: u64,
        mouse_move_queue_drops: u64,
        reliable_queue_failures: u64,
        raw_keyboard_fallback_events: u64,
        keyboard_hook_recovery_requests: u64,
        first_key_latency_ms: Option<u64>,
    }

    impl CaptureSessionStats {
        fn record_key(&mut self, usage: kvmflow_domain::KeyUsage, down: bool) {
            if down {
                self.key_down_events = self.key_down_events.saturating_add(1);
                if is_modifier_usage(usage) {
                    self.modifier_down_events = self.modifier_down_events.saturating_add(1);
                }
                if matches!(
                    usage,
                    kvmflow_domain::KeyUsage::BACKSPACE | kvmflow_domain::KeyUsage::FORWARD_DELETE
                ) {
                    self.editing_key_down_events = self.editing_key_down_events.saturating_add(1);
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
                && self.raw_keyboard_fallback_events == 0
                && self.keyboard_hook_recovery_requests == 0
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
                editing_key_down_events = self.editing_key_down_events,
                repeated_key_down_events = self.repeated_key_down_events,
                ignored_key_up_events = self.ignored_key_up_events,
                unsupported_key_events = self.unsupported_key_events,
                mouse_button_events = self.mouse_button_events,
                back_button_events = self.back_button_events,
                forward_button_events = self.forward_button_events,
                mouse_move_queue_drops = self.mouse_move_queue_drops,
                reliable_queue_failures = self.reliable_queue_failures,
                raw_keyboard_fallback_events = self.raw_keyboard_fallback_events,
                keyboard_hook_recovery_requests = self.keyboard_hook_recovery_requests,
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
        target_display_id: Option<String>,
        startup: mpsc::Sender<Result<u32, WindowsInputAdapterError>>,
    ) {
        let mut message: MSG = unsafe { std::mem::zeroed() };
        let thread_id = unsafe { GetCurrentThreadId() };
        let displays = match enumerate_displays() {
            Ok(displays) => displays,
            Err(error) => {
                let _ = startup.send(Err(error));
                return;
            }
        };
        let Some(target_display) = select_display(&displays, target_display_id.as_deref()) else {
            let _ = startup.send(Err(WindowsInputAdapterError::DisplayEnumerationFailed));
            return;
        };
        let selected_display_id = target_display.target.id.clone();
        let configured_display_id = target_display_id
            .clone()
            .unwrap_or_else(|| selected_display_id.clone());
        let reportable_display_id =
            reportable_target_display_id(target_display_id.as_deref(), target_display);
        if reportable_display_id.is_none() {
            tracing::warn!(
                component = "display",
                requested_display = target_display_id.as_deref(),
                fallback_display = %selected_display_id,
                "configured Windows display is unavailable at startup; using temporary fallback"
            );
        }
        let selected_bounds = target_display.bounds;
        let selected_edge_blocked = target_display
            .target
            .edge_is_occupied(edge, displays.iter().map(|display| &display.target));
        tracing::info!(
            component = "display",
            display_id = %selected_display_id,
            edge = ?edge,
            edge_blocked = selected_edge_blocked,
            "Windows target display initialized"
        );
        let mut edge_detector = EdgeDetector::new(edge);
        edge_detector.set_blocked(selected_edge_blocked);
        if let Ok(mut state) = hook_state().lock() {
            *state = Some(HookState {
                events: events.clone(),
                suppressed,
                pressed_keys: HashSet::new(),
                remote_pressed_keys: HashSet::new(),
                locally_visible_modifiers: HashSet::new(),
                raw_observed_modifiers: HashSet::new(),
                pending_key_events: VecDeque::new(),
                raw_pointer: RawPointerState::default(),
                raw_input_observed: false,
                keyboard_hook_observed: false,
                recent_keyboard_hook_events: RecentKeyboardHookEvents::default(),
                raw_keyboard_fallback_active: false,
                target_display_id: configured_display_id,
                active_display_id: selected_display_id.clone(),
                screen_bounds: selected_bounds,
                edge_detector,
                transition_pending: false,
                active_transition: None,
                remote_active: false,
                held_mouse_buttons: HashSet::new(),
                edge_switch_cooldown_until: None,
                stats: CaptureSessionStats::default(),
                remote_started_at: None,
                foreground_fullscreen: false,
                last_diagnostic_foreground: 0,
            });
        } else {
            let _ = startup.send(Err(WindowsInputAdapterError::StartFailed));
            return;
        }
        if reportable_display_id.is_some() {
            let _ = events.try_send(PlatformEvent::TargetDisplayChanged {
                display_id: selected_display_id,
            });
        }
        let raw_input_window = match RawInputWindow::create() {
            Ok(window) => window,
            Err(error) => {
                clear_hook_state();
                let _ = startup.send(Err(error));
                return;
            }
        };
        let mut keyboard =
            unsafe { SetWindowsHookExW(WH_KEYBOARD_LL, Some(keyboard_hook), ptr::null_mut(), 0) };
        if keyboard.is_null() {
            clear_hook_state();
            let _ = startup.send(Err(WindowsInputAdapterError::InstallFailed));
            return;
        }
        let mut mouse =
            unsafe { SetWindowsHookExW(WH_MOUSE_LL, Some(mouse_hook), ptr::null_mut(), 0) };
        if mouse.is_null() {
            unsafe {
                UnhookWindowsHookEx(keyboard);
            }
            clear_hook_state();
            let _ = startup.send(Err(WindowsInputAdapterError::InstallFailed));
            return;
        }
        let foreground_hook = unsafe {
            SetWinEventHook(
                EVENT_SYSTEM_FOREGROUND,
                EVENT_SYSTEM_FOREGROUND,
                ptr::null_mut(),
                Some(foreground_window_hook),
                0,
                0,
                WINEVENT_OUTOFCONTEXT,
            )
        };
        let location_hook = unsafe {
            SetWinEventHook(
                EVENT_OBJECT_LOCATIONCHANGE,
                EVENT_OBJECT_LOCATIONCHANGE,
                ptr::null_mut(),
                Some(foreground_window_hook),
                0,
                0,
                WINEVENT_OUTOFCONTEXT,
            )
        };
        if foreground_hook.is_null() || location_hook.is_null() {
            tracing::warn!(
                component = "game-mode",
                "Windows foreground fullscreen event hook could not be fully installed"
            );
        }
        let foreground = unsafe { GetForegroundWindow() };
        refresh_foreground_fullscreen(foreground);
        log_foreground_modifier_context(foreground);
        tracing::info!(
            component = "input",
            keyboard_capture = "low_level_hook",
            keyboard_health_fallback = "raw_input_sink",
            mouse_buttons = "low_level_hook",
            mouse_movement = "raw_input_sink",
            "Windows input hooks installed"
        );
        let startup_modifiers = current_system_down_modifiers();
        release_local_modifiers(&startup_modifiers, "startup_recovery");
        let _ = startup.send(Ok(thread_id));
        loop {
            let result = unsafe { GetMessageW(&mut message, ptr::null_mut(), 0, 0) };
            if result <= 0 {
                break;
            }
            if message.message == WM_REARM_LOW_LEVEL_HOOKS {
                let reason = match message.wParam {
                    REARM_REASON_RAW_KEYBOARD_FALLBACK => "raw_keyboard_fallback",
                    REARM_REASON_MANUAL_RESET => "manual_input_reset",
                    _ => "control_transition",
                };
                let replacement_keyboard = unsafe {
                    SetWindowsHookExW(WH_KEYBOARD_LL, Some(keyboard_hook), ptr::null_mut(), 0)
                };
                let replacement_mouse =
                    unsafe { SetWindowsHookExW(WH_MOUSE_LL, Some(mouse_hook), ptr::null_mut(), 0) };
                if replacement_keyboard.is_null() || replacement_mouse.is_null() {
                    if !replacement_keyboard.is_null() {
                        unsafe {
                            UnhookWindowsHookEx(replacement_keyboard);
                        }
                    }
                    if !replacement_mouse.is_null() {
                        unsafe {
                            UnhookWindowsHookEx(replacement_mouse);
                        }
                    }
                    tracing::warn!(
                        component = "input",
                        reason,
                        "Windows input hooks could not be rearmed"
                    );
                    if let Ok(mut guard) = hook_state().lock()
                        && let Some(state) = guard.as_mut()
                    {
                        state.raw_keyboard_fallback_active = false;
                    }
                } else {
                    unsafe {
                        UnhookWindowsHookEx(keyboard);
                        UnhookWindowsHookEx(mouse);
                    }
                    keyboard = replacement_keyboard;
                    mouse = replacement_mouse;
                    if let Ok(mut guard) = hook_state().lock()
                        && let Some(state) = guard.as_mut()
                    {
                        state.recent_keyboard_hook_events.clear();
                        state.raw_keyboard_fallback_active = false;
                    }
                    tracing::info!(component = "input", reason, "Windows input hooks rearmed");
                }
                continue;
            }
            unsafe {
                TranslateMessage(&message);
                DispatchMessageW(&message);
            }
        }
        unsafe {
            UnhookWindowsHookEx(keyboard);
            UnhookWindowsHookEx(mouse);
            if !foreground_hook.is_null() {
                UnhookWinEvent(foreground_hook);
            }
            if !location_hook.is_null() {
                UnhookWinEvent(location_hook);
            }
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
                    ptr::null_mut(),
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
        match message {
            WM_INPUT => {
                process_raw_input(lparam);
                unsafe {
                    DefWindowProcW(hwnd, message, wparam, lparam);
                }
                return 0;
            }
            WM_DISPLAYCHANGE => refresh_display_topology(),
            WM_POWERBROADCAST if wparam as u32 == PBT_APMRESUMEAUTOMATIC => notify_system_resumed(),
            _ => {}
        }
        unsafe { DefWindowProcW(hwnd, message, wparam, lparam) }
    }

    fn notify_system_resumed() {
        if let Ok(guard) = hook_state().lock()
            && let Some(state) = guard.as_ref()
        {
            let _ = state.events.try_send(PlatformEvent::SystemResumed);
        }
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

    fn forward_keyboard_input(
        state: &mut HookState,
        virtual_key: u16,
        extended: bool,
        down: bool,
        pending: bool,
        capture_source: &'static str,
    ) {
        if !state.keyboard_hook_observed {
            state.keyboard_hook_observed = true;
            state.stats.first_key_latency_ms = state
                .remote_started_at
                .map(|started_at| started_at.elapsed().as_millis() as u64);
            tracing::info!(
                component = "input",
                capture_source,
                "Windows keyboard capture active"
            );
        }
        let Some(usage) = virtual_key_to_remote_usage_with_extended(virtual_key, extended) else {
            state.stats.unsupported_key_events =
                state.stats.unsupported_key_events.saturating_add(1);
            return;
        };

        match update_remote_pressed_keys(&mut state.remote_pressed_keys, usage, down) {
            RemoteKeyTransition::Forward => {}
            RemoteKeyTransition::ForwardRepeat => {
                state.stats.repeated_key_down_events =
                    state.stats.repeated_key_down_events.saturating_add(1);
            }
            RemoteKeyTransition::IgnoreRepeat => {
                state.stats.repeated_key_down_events =
                    state.stats.repeated_key_down_events.saturating_add(1);
                return;
            }
            RemoteKeyTransition::IgnoreRelease => {
                state.stats.ignored_key_up_events =
                    state.stats.ignored_key_up_events.saturating_add(1);
                return;
            }
        }

        state.stats.record_key(usage, down);
        if let Some(key) = keyboard_semantic_name(usage) {
            tracing::info!(
                component = "input",
                stage = "windows_capture",
                key,
                action = if down { "down" } else { "up" },
                "Windows remote control key captured"
            );
        }
        let event = InputEvent::Key {
            usage,
            action: if down { KeyAction::Down } else { KeyAction::Up },
        };
        if pending {
            state.queue_pending_key_event(event);
        } else {
            send_input(state, event, true);
        }
    }

    fn process_raw_keyboard(virtual_key: u16, scan_code: u16, flags: u16) {
        if virtual_key == u8::MAX.into() {
            return;
        }
        let extended = u32::from(flags) & RI_KEY_E0 != 0;
        let virtual_key = normalize_windows_virtual_key(virtual_key, scan_code, extended);
        let down = u32::from(flags) & RI_KEY_BREAK == 0;
        let event = KeyboardEventSignature {
            virtual_key,
            scan_code,
            extended,
            down,
        };
        let now = Instant::now();
        let Ok(mut guard) = hook_state().lock() else {
            return;
        };
        let Some(state) = guard.as_mut() else {
            return;
        };
        let suppressed = state.suppressed.load(Ordering::SeqCst);
        if let Some(key) = modifier_virtual_key_name(virtual_key) {
            let changed = if down {
                state.raw_observed_modifiers.insert(virtual_key)
            } else {
                state.raw_observed_modifiers.remove(&virtual_key)
            };
            if changed || !down {
                tracing::info!(
                    component = "input",
                    stage = "windows_modifier_audit",
                    source = "raw_input",
                    key,
                    action = if down { "down" } else { "up" },
                    remote = suppressed,
                    transition_pending = state.transition_pending,
                    "Windows modifier input observed"
                );
            }
        }
        if !suppressed && !state.transition_pending {
            observe_local_modifier_event(
                &mut state.locally_visible_modifiers,
                &mut state.pressed_keys,
                virtual_key,
                down,
            );
            state.recent_keyboard_hook_events.clear();
            return;
        }
        if state.recent_keyboard_hook_events.consume(event, now) {
            return;
        }

        state.stats.raw_keyboard_fallback_events =
            state.stats.raw_keyboard_fallback_events.saturating_add(1);
        if !state.raw_keyboard_fallback_active {
            state.raw_keyboard_fallback_active = true;
            state.stats.keyboard_hook_recovery_requests = state
                .stats
                .keyboard_hook_recovery_requests
                .saturating_add(1);
            tracing::warn!(
                component = "input",
                stage = "windows_capture",
                "Windows low-level keyboard hook missed input; raw-input fallback activated"
            );
            unsafe {
                PostThreadMessageW(
                    GetCurrentThreadId(),
                    WM_REARM_LOW_LEVEL_HOOKS,
                    REARM_REASON_RAW_KEYBOARD_FALLBACK,
                    0,
                );
            }
        }

        update_locally_visible_modifier(
            &mut state.locally_visible_modifiers,
            virtual_key,
            false,
            down,
        );
        if down {
            state.pressed_keys.insert(virtual_key);
        } else {
            state.pressed_keys.remove(&virtual_key);
        }
        if emergency_hotkey_active(state) {
            return;
        }
        forward_keyboard_input(
            state,
            virtual_key,
            extended,
            down,
            state.transition_pending && !suppressed,
            "raw_input_fallback",
        );
    }

    fn process_raw_mouse(flags: u16, x: i32, y: i32) {
        if MOUSE_ACTIVITY.load(Ordering::Relaxed) & MOUSE_ACTIVITY_REMOTE_CAPTURE == 0 {
            return;
        }
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

    fn refresh_display_topology() {
        if let Ok(mut guard) = hook_state().lock()
            && let Some(state) = guard.as_mut()
        {
            state.refresh_target_display();
        }
    }

    unsafe extern "system" fn foreground_window_hook(
        _hook: HWINEVENTHOOK,
        event: u32,
        _hwnd: HWND,
        object_id: i32,
        _child_id: i32,
        _thread_id: u32,
        _event_time: u32,
    ) {
        if event == EVENT_OBJECT_LOCATIONCHANGE && object_id != 0 {
            return;
        }
        let foreground = unsafe { GetForegroundWindow() };
        refresh_foreground_fullscreen(foreground);
        if event == EVENT_SYSTEM_FOREGROUND {
            log_foreground_modifier_context(foreground);
        }
    }

    fn log_foreground_modifier_context(hwnd: HWND) {
        let context = {
            let Ok(mut guard) = hook_state().lock() else {
                return;
            };
            let Some(state) = guard.as_mut() else {
                return;
            };
            if state.last_diagnostic_foreground == hwnd as usize {
                return;
            }
            state.last_diagnostic_foreground = hwnd as usize;
            (
                state.suppressed.load(Ordering::SeqCst),
                state.transition_pending,
                state.foreground_fullscreen,
                modifier_virtual_key_names(state.locally_visible_modifiers.iter().copied()),
                modifier_virtual_key_names(state.pressed_keys.iter().copied()),
                modifier_virtual_key_names(state.raw_observed_modifiers.iter().copied()),
            )
        };
        let system_down = current_system_down_modifiers();
        let mut class_name = [0u16; 256];
        let class_length =
            unsafe { GetClassNameW(hwnd, class_name.as_mut_ptr(), class_name.len() as i32) }.max(0)
                as usize;
        let mut process_id = 0;
        unsafe {
            GetWindowThreadProcessId(hwnd, &mut process_id);
        }
        tracing::info!(
            component = "input",
            stage = "windows_modifier_audit",
            process_id,
            executable = %foreground_executable_name(process_id),
            window_class = %String::from_utf16_lossy(&class_name[..class_length]),
            remote = context.0,
            transition_pending = context.1,
            fullscreen = context.2,
            local_visible = ?context.3,
            hook_pressed = ?context.4,
            raw_observed = ?context.5,
            system_down = ?modifier_virtual_key_names(system_down),
            "Windows foreground input context changed"
        );
    }

    fn foreground_executable_name(process_id: u32) -> String {
        let process = unsafe { OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, process_id) };
        if process.is_null() {
            return "unavailable".into();
        }
        let mut path = vec![0u16; 32768];
        let mut length = path.len() as u32;
        let result =
            unsafe { QueryFullProcessImageNameW(process, 0, path.as_mut_ptr(), &mut length) };
        unsafe {
            CloseHandle(process);
        }
        if result == 0 {
            return "unavailable".into();
        }
        // Keep only the executable name, never window titles or user directory paths.
        let path = String::from_utf16_lossy(&path[..length as usize]);
        path.rsplit(['\\', '/'])
            .next()
            .unwrap_or("unavailable")
            .to_owned()
    }

    fn refresh_foreground_fullscreen(hwnd: HWND) {
        let active = foreground_window_is_fullscreen(hwnd);
        if let Ok(mut guard) = hook_state().lock()
            && let Some(state) = guard.as_mut()
            && state.foreground_fullscreen != active
        {
            state.foreground_fullscreen = active;
            let _ = state
                .events
                .try_send(PlatformEvent::ForegroundFullscreenChanged { active });
        }
    }

    fn foreground_window_is_fullscreen(hwnd: HWND) -> bool {
        if hwnd.is_null()
            || hwnd == unsafe { GetDesktopWindow() }
            || hwnd == unsafe { GetShellWindow() }
            || unsafe { IsWindowVisible(hwnd) } == 0
            || unsafe { IsIconic(hwnd) } != 0
        {
            return false;
        }
        let style = unsafe { GetWindowLongPtrW(hwnd, GWL_STYLE) } as u32;
        let extended_style = unsafe { GetWindowLongPtrW(hwnd, GWL_EXSTYLE) } as u32;
        let mut class_name = [0u16; 256];
        let length =
            unsafe { GetClassNameW(hwnd, class_name.as_mut_ptr(), class_name.len() as i32) }.max(0)
                as usize;
        if !fullscreen_window_style(
            style,
            extended_style,
            &String::from_utf16_lossy(&class_name[..length]),
        ) {
            return false;
        }
        let monitor = unsafe { MonitorFromWindow(hwnd, MONITOR_DEFAULTTONEAREST) };
        if monitor.is_null() {
            return false;
        }
        let Some(window_rect) = visible_window_rect(hwnd) else {
            return false;
        };
        let mut client_rect = RECT::default();
        let mut client_origin = POINT::default();
        if unsafe { GetClientRect(hwnd, &mut client_rect) } == 0
            || unsafe { ClientToScreen(hwnd, &mut client_origin) } == 0
        {
            return false;
        }
        let mut monitor_info = MONITORINFO {
            cbSize: std::mem::size_of::<MONITORINFO>() as u32,
            ..Default::default()
        };
        if unsafe { GetMonitorInfoW(monitor, &mut monitor_info) } == 0 {
            return false;
        }
        let monitor_bounds = (
            monitor_info.rcMonitor.left,
            monitor_info.rcMonitor.top,
            monitor_info.rcMonitor.right,
            monitor_info.rcMonitor.bottom,
        );
        rect_covers_monitor(
            (
                window_rect.left,
                window_rect.top,
                window_rect.right,
                window_rect.bottom,
            ),
            monitor_bounds,
        ) && rect_covers_monitor(
            (
                client_origin.x,
                client_origin.y,
                client_origin.x + client_rect.right,
                client_origin.y + client_rect.bottom,
            ),
            monitor_bounds,
        )
    }

    fn visible_window_rect(hwnd: HWND) -> Option<RECT> {
        let mut rect = RECT::default();
        let result = unsafe {
            DwmGetWindowAttribute(
                hwnd,
                DWMWA_EXTENDED_FRAME_BOUNDS as u32,
                &mut rect as *mut RECT as *mut core::ffi::c_void,
                std::mem::size_of::<RECT>() as u32,
            )
        };
        if result >= 0 {
            return Some(rect);
        }
        (unsafe { GetWindowRect(hwnd, &mut rect) } != 0).then_some(rect)
    }

    fn clear_hook_state() {
        MOUSE_ACTIVITY.store(0, Ordering::SeqCst);
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
            if (down || up)
                && process_keyboard_hook(
                    virtual_key,
                    data.scanCode as u16,
                    data.flags,
                    data.dwExtraInfo,
                    down,
                )
            {
                return 1;
            }
        }
        unsafe { CallNextHookEx(ptr::null_mut(), code, wparam, lparam) }
    }

    fn process_keyboard_hook(
        virtual_key: u16,
        scan_code: u16,
        flags: u32,
        extra_info: usize,
        down: bool,
    ) -> bool {
        let Ok(mut guard) = hook_state().lock() else {
            return false;
        };
        let Some(state) = guard.as_mut() else {
            return false;
        };
        let extended = flags & LLKHF_EXTENDED != 0;
        let virtual_key = normalize_windows_virtual_key(virtual_key, scan_code, extended);
        let suppressed = state.suppressed.load(Ordering::SeqCst);
        if let Some(key) = modifier_virtual_key_name(virtual_key)
            && (!down || !state.pressed_keys.contains(&virtual_key) || flags & LLKHF_INJECTED != 0)
        {
            tracing::info!(
                component = "input",
                stage = "windows_modifier_audit",
                source = "low_level_hook",
                key,
                action = if down { "down" } else { "up" },
                injected = flags & LLKHF_INJECTED != 0,
                lower_integrity_injected = flags & LLKHF_LOWER_IL_INJECTED != 0,
                own_repair = extra_info == MODIFIER_REPAIR_INPUT_TAG,
                intercepted = suppressed || state.transition_pending,
                remote = suppressed,
                transition_pending = state.transition_pending,
                "Windows modifier input observed"
            );
        }
        if suppressed || state.transition_pending {
            state.recent_keyboard_hook_events.record(
                KeyboardEventSignature {
                    virtual_key,
                    scan_code,
                    extended,
                    down,
                },
                Instant::now(),
            );
        }
        update_locally_visible_modifier(
            &mut state.locally_visible_modifiers,
            virtual_key,
            suppressed || state.transition_pending,
            down,
        );
        if down {
            state.pressed_keys.insert(virtual_key);
        } else {
            state.pressed_keys.remove(&virtual_key);
        }
        if (suppressed || state.transition_pending) && emergency_hotkey_active(state) {
            return true;
        }
        if suppressed || state.transition_pending {
            forward_keyboard_input(
                state,
                virtual_key,
                extended,
                down,
                state.transition_pending && !suppressed,
                "low_level_hook",
            );
        }
        suppressed || state.transition_pending
    }

    fn emergency_hotkey_active(state: &mut HookState) -> bool {
        if !emergency_hotkey(&state.pressed_keys) {
            return false;
        }
        let recovery_started_at = Instant::now();
        state.suppressed.store(false, Ordering::SeqCst);
        set_mouse_activity(MOUSE_ACTIVITY_REMOTE_CAPTURE, false);
        state.stats.log_and_reset("emergency_hotkey");
        state.pressed_keys.clear();
        state.remote_pressed_keys.clear();
        state.pending_key_events.clear();
        state.recent_keyboard_hook_events.clear();
        state.raw_keyboard_fallback_active = false;
        state.transition_pending = false;
        state.edge_switch_cooldown_until = Some(Instant::now() + EMERGENCY_EDGE_COOLDOWN);
        let _ = state.events.try_send(PlatformEvent::EmergencyReturn {
            response_ms: Some(recovery_started_at.elapsed().as_secs_f32() * 1_000.0),
        });
        true
    }

    fn current_system_down_modifiers() -> Vec<u16> {
        WINDOWS_SIDE_MODIFIER_VIRTUAL_KEYS
            .into_iter()
            .filter(|virtual_key| unsafe { GetAsyncKeyState(i32::from(*virtual_key)) } < 0)
            .collect()
    }

    fn release_local_modifiers(virtual_keys: &[u16], reason: &'static str) {
        if virtual_keys.is_empty() {
            return;
        }
        let inputs = virtual_keys
            .iter()
            .map(|virtual_key| {
                let extended = modifier_release_is_extended(*virtual_key);
                INPUT {
                    r#type: INPUT_KEYBOARD,
                    Anonymous: windows_sys::Win32::UI::Input::KeyboardAndMouse::INPUT_0 {
                        ki: KEYBDINPUT {
                            wVk: *virtual_key,
                            wScan: 0,
                            dwFlags: KEYEVENTF_KEYUP
                                | if extended { KEYEVENTF_EXTENDEDKEY } else { 0 },
                            time: 0,
                            dwExtraInfo: MODIFIER_REPAIR_INPUT_TAG,
                        },
                    },
                }
            })
            .collect::<Vec<_>>();
        let sent = unsafe {
            SendInput(
                inputs.len() as u32,
                inputs.as_ptr(),
                std::mem::size_of::<INPUT>() as i32,
            )
        };
        if sent == inputs.len() as u32 {
            tracing::info!(
                component = "input",
                reason,
                modifier_count = sent,
                delivery = "queued_not_verified",
                modifier_keys = ?modifier_virtual_key_names(virtual_keys.iter().copied()),
                "Windows local modifier state released"
            );
        } else {
            tracing::warn!(
                component = "input",
                reason,
                expected = inputs.len(),
                sent,
                modifier_keys = ?modifier_virtual_key_names(virtual_keys.iter().copied()),
                "Windows local modifier state could not be fully released"
            );
        }
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
        if message == WM_MOUSEMOVE && MOUSE_ACTIVITY.load(Ordering::Relaxed) == 0 {
            return false;
        }
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
                    state.start_keyboard_capture();
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
                set_mouse_activity(MOUSE_ACTIVITY_REMOTE_CAPTURE, false);
                let _ = state
                    .events
                    .try_send(PlatformEvent::EmergencyReturn { response_ms: None });
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
    #[error("failed to register the Windows raw-input devices")]
    RawInputRegistrationFailed,
    #[error("failed to enumerate active Windows displays")]
    DisplayEnumerationFailed,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn display(id: &str, primary: bool, bounds: ScreenBounds) -> WindowsDisplay {
        WindowsDisplay {
            target: DisplayTarget {
                id: id.into(),
                primary,
                left: bounds.left,
                top: bounds.top,
                width: (bounds.right - bounds.left + 1) as u32,
                height: (bounds.bottom - bounds.top + 1) as u32,
            },
            bounds,
        }
    }

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
        assert_eq!(
            virtual_key_to_remote_usage(VK_BACK),
            Some(KeyUsage::BACKSPACE)
        );
        assert_eq!(
            virtual_key_to_remote_usage(VK_DELETE),
            Some(KeyUsage::FORWARD_DELETE)
        );
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
    fn maps_common_editing_and_navigation_keys() {
        let expected = [
            (VK_RETURN, KeyUsage::ENTER),
            (VK_BACK, KeyUsage::BACKSPACE),
            (VK_TAB, KeyUsage::TAB),
            (VK_HOME, KeyUsage::HOME),
            (VK_PRIOR, KeyUsage::PAGE_UP),
            (VK_DELETE, KeyUsage::FORWARD_DELETE),
            (VK_END, KeyUsage::END),
            (VK_NEXT, KeyUsage::PAGE_DOWN),
            (VK_LEFT, KeyUsage::LEFT_ARROW),
            (VK_RIGHT, KeyUsage::RIGHT_ARROW),
            (VK_UP, KeyUsage::UP_ARROW),
            (VK_DOWN, KeyUsage::DOWN_ARROW),
        ];

        for (virtual_key, usage) in expected {
            assert_eq!(virtual_key_to_remote_usage(virtual_key), Some(usage));
        }
    }

    #[test]
    fn diagnostics_name_only_safe_control_keys() {
        assert_eq!(
            keyboard_semantic_name(KeyUsage::BACKSPACE),
            Some("backspace")
        );
        assert_eq!(
            keyboard_semantic_name(KeyUsage::FORWARD_DELETE),
            Some("forward_delete")
        );
        assert_eq!(keyboard_semantic_name(KeyUsage::UP_ARROW), Some("arrow_up"));
        assert_eq!(keyboard_semantic_name(KeyUsage::A), None);
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
    fn hook_metadata_distinguishes_left_and_right_modifiers() {
        assert_eq!(
            normalize_windows_virtual_key(VK_SHIFT, 0x2a, false),
            VK_LSHIFT
        );
        assert_eq!(
            normalize_windows_virtual_key(VK_SHIFT, 0x36, false),
            VK_RSHIFT
        );
        assert_eq!(
            normalize_windows_virtual_key(VK_CONTROL, 0x1d, false),
            VK_LCONTROL
        );
        assert_eq!(
            normalize_windows_virtual_key(VK_CONTROL, 0x1d, true),
            VK_RCONTROL
        );
        assert_eq!(
            normalize_windows_virtual_key(VK_MENU, 0x38, false),
            VK_LMENU
        );
        assert_eq!(normalize_windows_virtual_key(VK_MENU, 0x38, true), VK_RMENU);
    }

    #[test]
    fn intercepted_modifier_release_remains_pending_for_local_repair() {
        let mut visible = std::collections::HashSet::new();

        update_locally_visible_modifier(&mut visible, VK_LCONTROL, false, true);
        update_locally_visible_modifier(&mut visible, VK_LCONTROL, true, false);

        assert_eq!(visible, std::collections::HashSet::from([VK_LCONTROL]));
        update_locally_visible_modifier(&mut visible, VK_LCONTROL, false, false);
        assert!(visible.is_empty());
    }

    #[test]
    fn modifiers_pressed_only_during_remote_control_are_not_released_locally() {
        let mut visible = std::collections::HashSet::new();

        update_locally_visible_modifier(&mut visible, VK_LSHIFT, true, true);
        update_locally_visible_modifier(&mut visible, VK_LSHIFT, true, false);
        update_locally_visible_modifier(&mut visible, VK_A, false, true);

        assert!(visible.is_empty());
    }

    #[test]
    fn all_windows_modifier_sides_are_tracked_for_local_release() {
        for virtual_key in WINDOWS_SIDE_MODIFIER_VIRTUAL_KEYS {
            assert!(is_windows_modifier_virtual_key(virtual_key));
        }
        assert!(!is_windows_modifier_virtual_key(VK_A));
    }

    #[test]
    fn raw_local_modifier_observation_survives_an_intercepted_release() {
        let mut visible = std::collections::HashSet::new();
        let mut pressed = std::collections::HashSet::new();

        observe_local_modifier_event(&mut visible, &mut pressed, VK_LMENU, true);
        update_locally_visible_modifier(&mut visible, VK_LMENU, true, false);
        pressed.remove(&VK_LMENU);

        assert_eq!(visible, std::collections::HashSet::from([VK_LMENU]));
        assert!(pressed.is_empty());
    }

    #[test]
    fn modifier_snapshot_seeds_only_current_modifier_keys() {
        let mut visible = std::collections::HashSet::new();
        let mut pressed = std::collections::HashSet::new();

        let seeded = seed_locally_visible_modifiers(
            &mut visible,
            &mut pressed,
            [VK_RMENU, VK_A, VK_LCONTROL],
        );

        assert_eq!(seeded, 2);
        assert_eq!(
            visible,
            std::collections::HashSet::from([VK_RMENU, VK_LCONTROL])
        );
        assert_eq!(visible, pressed);
    }

    #[test]
    fn local_modifier_repair_releases_all_local_candidates() {
        let visible = std::collections::HashSet::from([VK_LCONTROL, VK_LWIN]);

        let release = local_modifier_repair_plan(&visible, [VK_LWIN, VK_RWIN, VK_LSHIFT]);

        assert_eq!(release, vec![VK_LWIN, VK_RWIN, VK_LSHIFT, VK_LCONTROL]);
    }

    #[test]
    fn modifier_virtual_key_names_are_safe_and_stable() {
        assert_eq!(
            modifier_virtual_key_names([VK_LWIN, VK_RMENU, VK_LCONTROL]),
            vec!["left_control", "left_windows", "right_alt"]
        );
        assert_eq!(modifier_virtual_key_names([VK_A]), Vec::<&str>::new());
    }

    #[test]
    fn windows_modifier_release_flags_cover_extended_keys() {
        for virtual_key in [VK_RCONTROL, VK_RMENU, VK_LWIN, VK_RWIN] {
            assert!(modifier_release_is_extended(virtual_key));
        }
        for virtual_key in [VK_LCONTROL, VK_LMENU, VK_LSHIFT, VK_RSHIFT] {
            assert!(!modifier_release_is_extended(virtual_key));
        }
    }

    #[test]
    fn remote_key_state_preserves_arbitrary_modifier_combinations() {
        let mut pressed = std::collections::HashSet::new();

        assert_eq!(
            update_remote_pressed_keys(&mut pressed, KeyUsage::LEFT_GUI, true),
            RemoteKeyTransition::Forward
        );
        assert_eq!(
            update_remote_pressed_keys(&mut pressed, KeyUsage::RIGHT_SHIFT, true),
            RemoteKeyTransition::Forward
        );
        assert_eq!(
            update_remote_pressed_keys(&mut pressed, KeyUsage::UP_ARROW, true),
            RemoteKeyTransition::Forward
        );
        assert!(pressed.contains(&KeyUsage::LEFT_GUI));
        assert!(pressed.contains(&KeyUsage::RIGHT_SHIFT));
        assert!(pressed.contains(&KeyUsage::UP_ARROW));
        assert_eq!(
            update_remote_pressed_keys(&mut pressed, KeyUsage::UP_ARROW, false),
            RemoteKeyTransition::Forward
        );
        assert!(pressed.contains(&KeyUsage::LEFT_GUI));
        assert!(pressed.contains(&KeyUsage::RIGHT_SHIFT));
    }

    #[test]
    fn remote_key_state_forwards_repeat_only_for_non_modifiers() {
        let mut pressed = std::collections::HashSet::new();

        assert_eq!(
            update_remote_pressed_keys(&mut pressed, KeyUsage::A, true),
            RemoteKeyTransition::Forward
        );
        assert_eq!(
            update_remote_pressed_keys(&mut pressed, KeyUsage::A, true),
            RemoteKeyTransition::ForwardRepeat
        );
        assert_eq!(
            update_remote_pressed_keys(&mut pressed, KeyUsage::LEFT_GUI, true),
            RemoteKeyTransition::Forward
        );
        assert_eq!(
            update_remote_pressed_keys(&mut pressed, KeyUsage::LEFT_GUI, true),
            RemoteKeyTransition::IgnoreRepeat
        );
    }

    #[test]
    fn display_selection_prefers_configured_monitor_with_negative_coordinates() {
        let displays = vec![
            display(
                r"\\.\DISPLAY1",
                true,
                ScreenBounds {
                    left: 0,
                    top: 0,
                    right: 1919,
                    bottom: 1079,
                },
            ),
            display(
                r"\\.\DISPLAY2",
                false,
                ScreenBounds {
                    left: -2560,
                    top: -360,
                    right: -1,
                    bottom: 1079,
                },
            ),
        ];

        let selected = select_display(&displays, Some(r"\\.\DISPLAY2")).unwrap();

        assert_eq!(selected.target.id, r"\\.\DISPLAY2");
        assert_eq!(selected.bounds.left, -2560);
        assert_eq!(selected.bounds.top, -360);
        assert_eq!(selected.target.width, 2560);
        assert_eq!(selected.target.height, 1440);
    }

    #[test]
    fn unavailable_display_falls_back_to_primary_monitor() {
        let displays = vec![
            display(
                r"\\.\DISPLAY2",
                false,
                ScreenBounds {
                    left: 1920,
                    top: 0,
                    right: 3839,
                    bottom: 1079,
                },
            ),
            display(
                r"\\.\DISPLAY1",
                true,
                ScreenBounds {
                    left: 0,
                    top: 0,
                    right: 1919,
                    bottom: 1079,
                },
            ),
        ];

        let selected = select_display(&displays, Some(r"\\.\DISPLAY9")).unwrap();

        assert_eq!(selected.target.id, r"\\.\DISPLAY1");
        assert!(selected.target.primary);
        assert_eq!(
            reportable_target_display_id(Some(r"\\.\DISPLAY9"), selected),
            None
        );
    }

    #[test]
    fn available_preferred_display_can_be_reported_as_the_configured_target() {
        let displays = vec![display(
            r"\\.\DISPLAY2",
            false,
            ScreenBounds {
                left: 1920,
                top: 0,
                right: 3839,
                bottom: 1079,
            },
        )];

        let selected = select_display(&displays, Some(r"\\.\DISPLAY2")).unwrap();

        assert_eq!(
            reportable_target_display_id(Some(r"\\.\DISPLAY2"), selected),
            Some(r"\\.\DISPLAY2")
        );
    }

    #[test]
    fn raw_keyboard_observer_consumes_the_matching_low_level_hook_event() {
        let now = std::time::Instant::now();
        let event = KeyboardEventSignature {
            virtual_key: VK_A,
            scan_code: 0x1e,
            extended: false,
            down: true,
        };
        let mut recent = RecentKeyboardHookEvents::default();

        recent.record(event, now);

        assert!(recent.consume(event, now + std::time::Duration::from_millis(5)));
        assert!(!recent.consume(event, now + std::time::Duration::from_millis(6)));
    }

    #[test]
    fn stale_low_level_hook_event_does_not_hide_a_raw_keyboard_fallback() {
        let now = std::time::Instant::now();
        let event = KeyboardEventSignature {
            virtual_key: VK_A,
            scan_code: 0x1e,
            extended: false,
            down: true,
        };
        let mut recent = RecentKeyboardHookEvents::default();

        recent.record(event, now);

        assert!(!recent.consume(
            event,
            now + RECENT_KEYBOARD_HOOK_EVENT_WINDOW + std::time::Duration::from_millis(1)
        ));
    }

    #[test]
    fn recent_low_level_hook_events_remain_bounded() {
        let now = std::time::Instant::now();
        let mut recent = RecentKeyboardHookEvents::default();
        for index in 0..RECENT_KEYBOARD_HOOK_EVENT_LIMIT + 8 {
            recent.record(
                KeyboardEventSignature {
                    virtual_key: VK_A + index as u16,
                    scan_code: index as u16,
                    extended: false,
                    down: true,
                },
                now,
            );
        }

        assert_eq!(recent.events.len(), RECENT_KEYBOARD_HOOK_EVENT_LIMIT);
    }

    #[test]
    fn relative_raw_pointer_events_preserve_each_delta() {
        let mut state = RawPointerState::default();

        assert_eq!(state.movement(false, 8, -3), Some((8, -3)));
        assert_eq!(state.movement(false, 8, -3), Some((8, -3)));
        assert_eq!(state.movement(false, 0, 0), None);
    }

    #[test]
    fn fullscreen_detection_accepts_monitor_cover_but_rejects_maximized_work_area() {
        let monitor = (0, 0, 1920, 1080);

        assert!(rect_covers_monitor((0, 0, 1920, 1080), monitor));
        assert!(rect_covers_monitor((-1, -1, 1921, 1081), monitor));
        assert!(!rect_covers_monitor((0, 0, 1920, 1040), monitor));
        assert!(!rect_covers_monitor((0, 0, 3840, 1080), monitor));
        assert!(!fullscreen_window_style(0x00c0_0000, 0, "SDL_app"));
        assert!(!fullscreen_window_style(0x0004_0000, 0, "SDL_app"));
        assert!(!fullscreen_window_style(0, 0x80, "overlay"));
        assert!(!fullscreen_window_style(
            0,
            0,
            "XamlExplorerHostIslandWindow"
        ));
        assert!(fullscreen_window_style(0x8000_0000, 0, "SDL_app"));
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
    fn occupied_edge_never_triggers_until_it_becomes_available() {
        let bounds = ScreenBounds {
            left: 0,
            top: 0,
            right: 1919,
            bottom: 1079,
        };
        let mut detector = EdgeDetector::new(ScreenEdge::Right);
        detector.set_enabled(true);
        detector.set_blocked(true);

        assert_eq!(detector.observe(bounds, 1919, 500, false), None);

        detector.set_blocked(false);
        assert!(detector.observe(bounds, 1919, 500, false).is_some());
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

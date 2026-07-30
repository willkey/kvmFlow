use std::{
    collections::{HashMap, HashSet},
    time::{Duration, Instant},
};

use kvmflow_domain::{InputEvent, KeyAction, KeyUsage, MouseButton, ScreenEdge};

const RETURN_EDGE_REARM_DISTANCE: f64 = 12.0;
const MULTI_CLICK_INTERVAL: Duration = Duration::from_millis(500);
const MULTI_CLICK_DISTANCE: f64 = 4.0;
const ACTIVE_MOUSE_MOVE_GAP: Duration = Duration::from_millis(100);
const DELAYED_MOUSE_MOVE_GAP: Duration = Duration::from_millis(12);
const BURST_MOUSE_MOVE_GAP: Duration = Duration::from_millis(2);
#[cfg(any(target_os = "macos", test))]
const POINTER_SYNC_CHECK_INTERVAL: Duration = Duration::from_millis(100);
#[cfg(any(target_os = "macos", test))]
const POINTER_SYNC_TOLERANCE: f64 = 128.0;
const DEVICE_LEFT_CONTROL_FLAG: u64 = 0x0000_0001;
const DEVICE_LEFT_SHIFT_FLAG: u64 = 0x0000_0002;
const DEVICE_RIGHT_SHIFT_FLAG: u64 = 0x0000_0004;
const DEVICE_LEFT_COMMAND_FLAG: u64 = 0x0000_0008;
const DEVICE_RIGHT_COMMAND_FLAG: u64 = 0x0000_0010;
const DEVICE_LEFT_OPTION_FLAG: u64 = 0x0000_0020;
const DEVICE_RIGHT_OPTION_FLAG: u64 = 0x0000_0040;
const DEVICE_RIGHT_CONTROL_FLAG: u64 = 0x0000_2000;
#[cfg(any(target_os = "macos", test))]
const IOHID_SET_GLOBAL_EVENT_FLAGS: u32 = 0x0000_0001;
#[cfg(any(target_os = "macos", test))]
const NX_KEY_DOWN: u32 = 10;
#[cfg(any(target_os = "macos", test))]
const NX_KEY_UP: u32 = 11;
#[cfg(any(target_os = "macos", test))]
const NX_FLAGS_CHANGED: u32 = 12;
#[cfg(target_os = "macos")]
const NX_SYSTEM_DEFINED: u32 = 14;
#[cfg(target_os = "macos")]
const NX_SUBTYPE_AUX_CONTROL_BUTTONS: i16 = 8;
#[cfg(any(target_os = "macos", test))]
const NX_KEYTYPE_CAPS_LOCK: i32 = 4;

#[cfg(target_os = "macos")]
type RemoteEventSource = core_graphics::event_source::CGEventSource;

#[cfg(not(target_os = "macos"))]
#[derive(Clone)]
struct RemoteEventSource;

#[cfg(target_os = "macos")]
#[derive(Default)]
enum HidKeyboardState {
    #[default]
    Uninitialized,
    Active(HidKeyboardPoster),
    Unavailable,
}

#[derive(Default)]
pub struct MacInjectionState {
    event_source: Option<RemoteEventSource>,
    #[cfg(target_os = "macos")]
    hid_keyboard: HidKeyboardState,
    pointer_x: f64,
    pointer_y: f64,
    active_pointer: Option<ActiveRemotePointer>,
    last_pointer_sync_check_at: Option<Instant>,
    pointer_warp_fallback: bool,
    held_keys: HashSet<KeyUsage>,
    held_buttons: HashSet<MouseButton>,
    click_tracker: ClickTracker,
    stats: InputInjectionStats,
}

impl MacInjectionState {
    pub fn inject(&mut self, event: InputEvent) -> Result<InjectionOutcome, InjectionError> {
        match event {
            InputEvent::MouseMove { dx, dy } => {
                if self
                    .active_pointer
                    .is_some_and(|pointer| pointer.return_pending)
                {
                    return Ok(InjectionOutcome::Applied);
                }
                let now = Instant::now();
                self.reconcile_pointer_with_system(now)?;
                self.pointer_x += f64::from(dx);
                self.pointer_y += f64::from(dy);
                let return_position = self.active_pointer.as_mut().and_then(|pointer| {
                    pointer.observe_return(
                        self.pointer_x,
                        self.pointer_y,
                        !self.held_buttons.is_empty(),
                    )
                });
                if let Some(pointer) = self.active_pointer.as_ref() {
                    (self.pointer_x, self.pointer_y) =
                        pointer.bounds.clamp(self.pointer_x, self.pointer_y);
                }
                self.post_mouse_move()?;
                self.stats.record_mouse_move(now);
                if let Some(position) = return_position {
                    return Ok(InjectionOutcome::ReturnEdgeReached { position });
                }
                Ok(InjectionOutcome::Applied)
            }
            InputEvent::MouseButton { button, action } => {
                let down = action == KeyAction::Down;
                let click_count = self.click_tracker.observe(
                    button,
                    down,
                    self.pointer_x,
                    self.pointer_y,
                    Instant::now(),
                );
                if down {
                    self.held_buttons.insert(button);
                } else {
                    self.held_buttons.remove(&button);
                }
                let pointer_x = self.pointer_x;
                let pointer_y = self.pointer_y;
                let modifiers = ModifierState::from_held_keys(&self.held_keys);
                let source = self.remote_event_source_ref()?;
                let result = post_mouse_button(
                    source,
                    button,
                    down,
                    pointer_x,
                    pointer_y,
                    click_count,
                    modifiers,
                );
                if result.is_ok() {
                    self.stats.record_mouse_button(button, down, click_count);
                }
                result.map(|_| InjectionOutcome::Applied)
            }
            InputEvent::Wheel { delta_x, delta_y } => {
                let modifiers = ModifierState::from_held_keys(&self.held_keys);
                let source = self.remote_event_source_ref()?;
                post_wheel(source, i32::from(delta_x), i32::from(delta_y), modifiers)?;
                Ok(InjectionOutcome::Applied)
            }
            InputEvent::Key { usage, action } => {
                let down = action == KeyAction::Down;
                let state_changed = if down {
                    self.held_keys.insert(usage)
                } else {
                    self.held_keys.remove(&usage)
                };
                if !state_changed && (!down || is_non_repeating_key(usage)) {
                    return Ok(InjectionOutcome::Applied);
                }
                let alpha_shift = alpha_shift_for_event(usage, down, system_alpha_shift_active());
                let modifiers =
                    ModifierState::from_held_keys(&self.held_keys).with_alpha_shift(alpha_shift);
                let source = self.remote_event_source()?;
                let result = self.post_key(&source, usage, down, modifiers);
                if result.is_ok() {
                    self.stats.record_key(usage, down);
                    log_keyboard_semantics(usage, down, modifiers);
                }
                result.map(|_| InjectionOutcome::Applied)
            }
        }
    }

    pub fn prepare_remote_pointer(
        &mut self,
        edge: ScreenEdge,
        position: f32,
    ) -> Result<(), InjectionError> {
        let bounds = main_display_bounds()?;
        let position = f64::from(position.clamp(0.0, 1.0));
        (self.pointer_x, self.pointer_y) = match edge {
            ScreenEdge::Right => (
                bounds.origin_x + 2.0,
                bounds.origin_y + bounds.vertical_span() * position,
            ),
            ScreenEdge::Left => (
                bounds.max_x() - 2.0,
                bounds.origin_y + bounds.vertical_span() * position,
            ),
            ScreenEdge::Bottom => (
                bounds.origin_x + bounds.horizontal_span() * position,
                bounds.origin_y + 2.0,
            ),
            ScreenEdge::Top => (
                bounds.origin_x + bounds.horizontal_span() * position,
                bounds.max_y() - 2.0,
            ),
        };
        (self.pointer_x, self.pointer_y) = bounds.clamp(self.pointer_x, self.pointer_y);
        self.active_pointer = Some(ActiveRemotePointer {
            entry_edge: edge,
            bounds,
            return_armed: false,
            return_pending: false,
        });
        self.pointer_warp_fallback = false;
        let pointer_x = self.pointer_x;
        let pointer_y = self.pointer_y;
        let source = self.remote_event_source_ref()?;
        post_mouse_move(
            source,
            pointer_x,
            pointer_y,
            MouseMoveKind::Moved,
            0,
            ModifierState::default(),
        )?;
        self.last_pointer_sync_check_at = Some(Instant::now());
        Ok(())
    }

    pub fn release_all(&mut self) -> Result<(), InjectionError> {
        let source = if self.held_keys.is_empty() && self.held_buttons.is_empty() {
            None
        } else {
            Some(self.remote_event_source()?)
        };
        let held_keys: Vec<_> = self.held_keys.drain().collect();
        let held_buttons: Vec<_> = self.held_buttons.drain().collect();
        let held_keys_released = held_keys.len();
        let held_buttons_released = held_buttons.len();
        let mut first_error = None;
        for usage in held_keys {
            let alpha_shift = alpha_shift_for_event(usage, false, system_alpha_shift_active());
            if let Err(error) = self.post_key(
                source.as_ref().expect("source exists for held keys"),
                usage,
                false,
                ModifierState::default().with_alpha_shift(alpha_shift),
            ) && first_error.is_none()
            {
                first_error = Some(error);
            }
        }
        for button in held_buttons {
            let click_count = self.click_tracker.active_count(button).max(1);
            if let Err(error) = post_mouse_button(
                source.as_ref().expect("source exists for held buttons"),
                button,
                false,
                self.pointer_x,
                self.pointer_y,
                click_count,
                ModifierState::default(),
            ) && first_error.is_none()
            {
                first_error = Some(error);
            }
        }
        self.stats
            .log_and_reset(held_keys_released, held_buttons_released);
        self.click_tracker.reset();
        self.active_pointer = None;
        self.last_pointer_sync_check_at = None;
        self.pointer_warp_fallback = false;
        if let Some(error) = first_error {
            return Err(error);
        }
        Ok(())
    }

    fn post_mouse_move(&mut self) -> Result<(), InjectionError> {
        let kind = if self.held_buttons.contains(&MouseButton::Left) {
            MouseMoveKind::LeftDrag
        } else if self.held_buttons.contains(&MouseButton::Right) {
            MouseMoveKind::RightDrag
        } else if self.held_buttons.contains(&MouseButton::Middle) {
            MouseMoveKind::OtherDrag(MouseButton::Middle)
        } else if self.held_buttons.contains(&MouseButton::Back) {
            MouseMoveKind::OtherDrag(MouseButton::Back)
        } else if self.held_buttons.contains(&MouseButton::Forward) {
            MouseMoveKind::OtherDrag(MouseButton::Forward)
        } else {
            MouseMoveKind::Moved
        };
        let click_count = match kind {
            MouseMoveKind::Moved => 0,
            MouseMoveKind::LeftDrag => self.click_tracker.active_count(MouseButton::Left),
            MouseMoveKind::RightDrag => self.click_tracker.active_count(MouseButton::Right),
            MouseMoveKind::OtherDrag(button) => self.click_tracker.active_count(button),
        };
        let pointer_x = self.pointer_x;
        let pointer_y = self.pointer_y;
        let modifiers = ModifierState::from_held_keys(&self.held_keys);
        let source = self.remote_event_source_ref()?;
        post_mouse_move(source, pointer_x, pointer_y, kind, click_count, modifiers)?;
        if self.pointer_warp_fallback {
            warp_mouse_cursor(pointer_x, pointer_y)?;
            self.stats.pointer_warp_events = self.stats.pointer_warp_events.saturating_add(1);
        }
        Ok(())
    }

    #[cfg(target_os = "macos")]
    fn reconcile_pointer_with_system(&mut self, now: Instant) -> Result<(), InjectionError> {
        if self.pointer_warp_fallback
            || !pointer_sync_check_due(self.last_pointer_sync_check_at, now)
        {
            return Ok(());
        }
        self.last_pointer_sync_check_at = Some(now);
        let source = self.remote_event_source()?;
        let Ok((actual_x, actual_y)) = current_mouse_location(&source) else {
            return Ok(());
        };
        if !pointer_position_drifted(
            self.pointer_x,
            self.pointer_y,
            actual_x,
            actual_y,
            POINTER_SYNC_TOLERANCE,
        ) {
            return Ok(());
        }
        let (actual_x, actual_y) = self.active_pointer.map_or((actual_x, actual_y), |pointer| {
            pointer.bounds.clamp(actual_x, actual_y)
        });
        self.pointer_x = actual_x;
        self.pointer_y = actual_y;
        self.pointer_warp_fallback = true;
        self.stats.pointer_sync_repairs = self.stats.pointer_sync_repairs.saturating_add(1);
        tracing::warn!(
            component = "input",
            stage = "mac_injection",
            tolerance_px = POINTER_SYNC_TOLERANCE,
            "Mac system cursor diverged from remote input; warp fallback enabled for this session"
        );
        Ok(())
    }

    #[cfg(not(target_os = "macos"))]
    fn reconcile_pointer_with_system(&mut self, _now: Instant) -> Result<(), InjectionError> {
        Ok(())
    }

    fn remote_event_source(&mut self) -> Result<RemoteEventSource, InjectionError> {
        Ok(self.remote_event_source_ref()?.clone())
    }

    fn remote_event_source_ref(&mut self) -> Result<&RemoteEventSource, InjectionError> {
        if self.event_source.is_none() {
            self.event_source = Some(remote_event_source()?);
            tracing::info!(
                component = "input",
                source_state = "combined_session",
                source_lifetime = "adapter",
                "Mac remote input event source initialized"
            );
        }
        Ok(self
            .event_source
            .as_ref()
            .expect("event source initialized"))
    }

    #[cfg(target_os = "macos")]
    fn post_key(
        &mut self,
        source: &RemoteEventSource,
        usage: KeyUsage,
        down: bool,
        modifiers: ModifierState,
    ) -> Result<(), InjectionError> {
        if self.hid_keyboard.post(usage, down, modifiers).is_ok() {
            return Ok(());
        }
        post_key(source, usage, down, modifiers)
    }

    #[cfg(not(target_os = "macos"))]
    fn post_key(
        &mut self,
        source: &RemoteEventSource,
        usage: KeyUsage,
        down: bool,
        modifiers: ModifierState,
    ) -> Result<(), InjectionError> {
        post_key(source, usage, down, modifiers)
    }
}

#[cfg(target_os = "macos")]
impl HidKeyboardState {
    fn post(
        &mut self,
        usage: KeyUsage,
        down: bool,
        modifiers: ModifierState,
    ) -> Result<(), HidKeyboardError> {
        if matches!(self, Self::Uninitialized) {
            *self = match HidKeyboardPoster::open() {
                Ok(poster) => {
                    tracing::info!(
                        component = "input",
                        backend = "iohid",
                        "Mac system keyboard injection backend initialized"
                    );
                    Self::Active(poster)
                }
                Err(error) => {
                    tracing::warn!(
                        component = "input",
                        backend = "iohid",
                        error = %error,
                        "Mac system keyboard injection is unavailable; using CoreGraphics fallback"
                    );
                    Self::Unavailable
                }
            };
        }

        let Self::Active(poster) = self else {
            return Err(HidKeyboardError::Unavailable);
        };
        if let Err(error) = poster.post(usage, down, modifiers) {
            tracing::warn!(
                component = "input",
                backend = "iohid",
                error = %error,
                "Mac system keyboard injection failed; using CoreGraphics fallback"
            );
            if !matches!(error, HidKeyboardError::UnsupportedKey(_)) {
                *self = Self::Unavailable;
            }
            return Err(error);
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum InjectionOutcome {
    Applied,
    ReturnEdgeReached { position: f32 },
}

#[derive(Debug, Clone, Copy)]
struct DisplayBounds {
    origin_x: f64,
    origin_y: f64,
    width: f64,
    height: f64,
}

impl DisplayBounds {
    fn max_x(self) -> f64 {
        self.origin_x + self.width.max(1.0) - 1.0
    }

    fn max_y(self) -> f64 {
        self.origin_y + self.height.max(1.0) - 1.0
    }

    fn vertical_span(self) -> f64 {
        (self.height.max(1.0) - 1.0).max(1.0)
    }

    fn horizontal_span(self) -> f64 {
        (self.width.max(1.0) - 1.0).max(1.0)
    }

    fn clamp(self, x: f64, y: f64) -> (f64, f64) {
        (
            x.clamp(self.origin_x, self.max_x()),
            y.clamp(self.origin_y, self.max_y()),
        )
    }

    fn normalized_y(self, y: f64) -> f32 {
        ((y - self.origin_y) / self.vertical_span()).clamp(0.0, 1.0) as f32
    }

    fn normalized_x(self, x: f64) -> f32 {
        ((x - self.origin_x) / self.horizontal_span()).clamp(0.0, 1.0) as f32
    }
}

#[derive(Debug, Clone, Copy)]
struct ActiveRemotePointer {
    entry_edge: ScreenEdge,
    bounds: DisplayBounds,
    return_armed: bool,
    return_pending: bool,
}

impl ActiveRemotePointer {
    fn observe_return(&mut self, x: f64, y: f64, button_held: bool) -> Option<f32> {
        if self.return_pending {
            return None;
        }
        if !self.return_armed {
            self.return_armed = match self.entry_edge {
                ScreenEdge::Right => x >= self.bounds.origin_x + RETURN_EDGE_REARM_DISTANCE,
                ScreenEdge::Left => x <= self.bounds.max_x() - RETURN_EDGE_REARM_DISTANCE,
                ScreenEdge::Bottom => y >= self.bounds.origin_y + RETURN_EDGE_REARM_DISTANCE,
                ScreenEdge::Top => y <= self.bounds.max_y() - RETURN_EDGE_REARM_DISTANCE,
            };
            return None;
        }
        if button_held {
            return None;
        }
        let position = match self.entry_edge {
            ScreenEdge::Right if x <= self.bounds.origin_x => Some(self.bounds.normalized_y(y)),
            ScreenEdge::Left if x >= self.bounds.max_x() => Some(self.bounds.normalized_y(y)),
            ScreenEdge::Bottom if y <= self.bounds.origin_y => Some(self.bounds.normalized_x(x)),
            ScreenEdge::Top if y >= self.bounds.max_y() => Some(self.bounds.normalized_x(x)),
            _ => None,
        };
        if position.is_some() {
            self.return_pending = true;
        }
        position
    }
}

#[derive(Debug, Clone, Copy)]
struct ClickRecord {
    button: MouseButton,
    x: f64,
    y: f64,
    when: Instant,
    count: u8,
}

#[derive(Debug, Default)]
struct ClickTracker {
    last_down: Option<ClickRecord>,
    active_counts: HashMap<MouseButton, u8>,
}

impl ClickTracker {
    fn observe(&mut self, button: MouseButton, down: bool, x: f64, y: f64, now: Instant) -> u8 {
        if !down {
            return self.active_counts.remove(&button).unwrap_or(1);
        }

        let count = self
            .last_down
            .filter(|previous| {
                previous.button == button
                    && now.saturating_duration_since(previous.when) <= MULTI_CLICK_INTERVAL
                    && (x - previous.x).abs() <= MULTI_CLICK_DISTANCE
                    && (y - previous.y).abs() <= MULTI_CLICK_DISTANCE
            })
            .map_or(1, |previous| previous.count.saturating_add(1));
        self.last_down = Some(ClickRecord {
            button,
            x,
            y,
            when: now,
            count,
        });
        self.active_counts.insert(button, count);
        count
    }

    fn active_count(&self, button: MouseButton) -> u8 {
        self.active_counts.get(&button).copied().unwrap_or(0)
    }

    fn reset(&mut self) {
        self.last_down = None;
        self.active_counts.clear();
    }
}

#[derive(Debug, Default)]
struct InputInjectionStats {
    key_down_events: u64,
    key_up_events: u64,
    modifier_down_events: u64,
    modifier_up_events: u64,
    mouse_button_events: u64,
    multi_click_down_events: u64,
    back_button_events: u64,
    forward_button_events: u64,
    mouse_move_events: u64,
    mouse_move_active_intervals: u64,
    mouse_move_active_interval_us: u64,
    mouse_move_max_active_interval_us: u64,
    mouse_move_delayed_intervals: u64,
    mouse_move_burst_intervals: u64,
    pointer_sync_repairs: u64,
    pointer_warp_events: u64,
    last_mouse_move_at: Option<Instant>,
}

impl InputInjectionStats {
    fn record_mouse_move(&mut self, now: Instant) {
        self.mouse_move_events = self.mouse_move_events.saturating_add(1);
        if let Some(previous) = self.last_mouse_move_at {
            let elapsed = now.saturating_duration_since(previous);
            if elapsed <= ACTIVE_MOUSE_MOVE_GAP {
                let elapsed_us = elapsed.as_micros().min(u128::from(u64::MAX)) as u64;
                self.mouse_move_active_intervals =
                    self.mouse_move_active_intervals.saturating_add(1);
                self.mouse_move_active_interval_us = self
                    .mouse_move_active_interval_us
                    .saturating_add(elapsed_us);
                self.mouse_move_max_active_interval_us =
                    self.mouse_move_max_active_interval_us.max(elapsed_us);
                if elapsed > DELAYED_MOUSE_MOVE_GAP {
                    self.mouse_move_delayed_intervals =
                        self.mouse_move_delayed_intervals.saturating_add(1);
                } else if elapsed < BURST_MOUSE_MOVE_GAP {
                    self.mouse_move_burst_intervals =
                        self.mouse_move_burst_intervals.saturating_add(1);
                }
            }
        }
        self.last_mouse_move_at = Some(now);
    }

    fn record_key(&mut self, usage: KeyUsage, down: bool) {
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

    fn record_mouse_button(&mut self, button: MouseButton, down: bool, click_count: u8) {
        self.mouse_button_events = self.mouse_button_events.saturating_add(1);
        if down && click_count > 1 {
            self.multi_click_down_events = self.multi_click_down_events.saturating_add(1);
        }
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

    fn log_and_reset(&mut self, held_keys_released: usize, held_buttons_released: usize) {
        if self.key_down_events == 0
            && self.key_up_events == 0
            && self.mouse_button_events == 0
            && self.mouse_move_events == 0
            && held_keys_released == 0
            && held_buttons_released == 0
        {
            return;
        }
        let mouse_move_average_interval_us = self
            .mouse_move_active_interval_us
            .checked_div(self.mouse_move_active_intervals)
            .unwrap_or_default();
        tracing::info!(
            component = "input",
            stage = "mac_injection",
            key_down_events = self.key_down_events,
            key_up_events = self.key_up_events,
            modifier_down_events = self.modifier_down_events,
            modifier_up_events = self.modifier_up_events,
            mouse_button_events = self.mouse_button_events,
            multi_click_down_events = self.multi_click_down_events,
            back_button_events = self.back_button_events,
            forward_button_events = self.forward_button_events,
            mouse_move_events = self.mouse_move_events,
            mouse_move_average_interval_us,
            mouse_move_max_active_interval_us = self.mouse_move_max_active_interval_us,
            mouse_move_delayed_intervals = self.mouse_move_delayed_intervals,
            mouse_move_burst_intervals = self.mouse_move_burst_intervals,
            pointer_sync_repairs = self.pointer_sync_repairs,
            pointer_warp_events = self.pointer_warp_events,
            held_keys_released,
            held_buttons_released,
            "Mac input injection session summary"
        );
        *self = Self::default();
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
struct ModifierState {
    alpha_shift: bool,
    control: bool,
    shift: bool,
    alternate: bool,
    command: bool,
    device_flags: u64,
}

impl ModifierState {
    fn from_held_keys(held_keys: &HashSet<KeyUsage>) -> Self {
        Self {
            alpha_shift: false,
            control: contains_either(held_keys, KeyUsage::LEFT_CONTROL, KeyUsage::RIGHT_CONTROL),
            shift: contains_either(held_keys, KeyUsage::LEFT_SHIFT, KeyUsage::RIGHT_SHIFT),
            alternate: contains_either(held_keys, KeyUsage::LEFT_ALT, KeyUsage::RIGHT_ALT),
            command: contains_either(held_keys, KeyUsage::LEFT_GUI, KeyUsage::RIGHT_GUI),
            device_flags: device_modifier_flags(held_keys),
        }
    }

    fn with_alpha_shift(mut self, active: bool) -> Self {
        self.alpha_shift = active;
        self
    }

    fn has_shortcut_modifier(self) -> bool {
        self.control || self.shift || self.alternate || self.command
    }

    #[cfg(target_os = "macos")]
    fn event_flags(self) -> core_graphics::event::CGEventFlags {
        use core_graphics::event::CGEventFlags;

        let mut flags = CGEventFlags::empty();
        flags.set(CGEventFlags::CGEventFlagAlphaShift, self.alpha_shift);
        flags.set(CGEventFlags::CGEventFlagControl, self.control);
        flags.set(CGEventFlags::CGEventFlagShift, self.shift);
        flags.set(CGEventFlags::CGEventFlagAlternate, self.alternate);
        flags.set(CGEventFlags::CGEventFlagCommand, self.command);
        flags | CGEventFlags::from_bits_retain(self.device_flags)
    }

    #[cfg(target_os = "macos")]
    fn hid_event_flags(self) -> u32 {
        self.event_flags().bits() as u32
    }
}

fn device_modifier_flags(held_keys: &HashSet<KeyUsage>) -> u64 {
    held_keys.iter().fold(0, |flags, usage| {
        flags
            | match *usage {
                KeyUsage::LEFT_CONTROL => DEVICE_LEFT_CONTROL_FLAG,
                KeyUsage::RIGHT_CONTROL => DEVICE_RIGHT_CONTROL_FLAG,
                KeyUsage::LEFT_SHIFT => DEVICE_LEFT_SHIFT_FLAG,
                KeyUsage::RIGHT_SHIFT => DEVICE_RIGHT_SHIFT_FLAG,
                KeyUsage::LEFT_ALT => DEVICE_LEFT_OPTION_FLAG,
                KeyUsage::RIGHT_ALT => DEVICE_RIGHT_OPTION_FLAG,
                KeyUsage::LEFT_GUI => DEVICE_LEFT_COMMAND_FLAG,
                KeyUsage::RIGHT_GUI => DEVICE_RIGHT_COMMAND_FLAG,
                _ => 0,
            }
    })
}

fn contains_either(held_keys: &HashSet<KeyUsage>, left: KeyUsage, right: KeyUsage) -> bool {
    held_keys.contains(&left) || held_keys.contains(&right)
}

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

fn is_non_repeating_key(usage: KeyUsage) -> bool {
    is_modifier_usage(usage) || usage == KeyUsage::CAPS_LOCK || usage == KeyUsage::NUM_LOCK
}

fn alpha_shift_for_event(usage: KeyUsage, down: bool, system_active: bool) -> bool {
    if usage == KeyUsage::CAPS_LOCK && down {
        !system_active
    } else {
        system_active
    }
}

#[cfg(any(target_os = "macos", test))]
fn needs_numeric_pad_flag(usage: KeyUsage) -> bool {
    matches!(usage.0, 0x4a..=0x63)
}

#[cfg(any(target_os = "macos", test))]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum RemoteKeyEventKind {
    Keyboard,
    FlagsChanged,
}

#[cfg(any(target_os = "macos", test))]
fn remote_key_event_kind(usage: KeyUsage) -> RemoteKeyEventKind {
    if is_modifier_usage(usage) || usage == KeyUsage::CAPS_LOCK {
        RemoteKeyEventKind::FlagsChanged
    } else {
        RemoteKeyEventKind::Keyboard
    }
}

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

fn log_keyboard_semantics(usage: KeyUsage, down: bool, modifiers: ModifierState) {
    let Some(key) = keyboard_semantic_name(usage) else {
        return;
    };
    if !is_modifier_usage(usage)
        && usage != KeyUsage::CAPS_LOCK
        && !modifiers.has_shortcut_modifier()
    {
        return;
    }
    tracing::info!(
        component = "input",
        stage = "mac_injection",
        key,
        action = if down { "down" } else { "up" },
        command = modifiers.command,
        control = modifiers.control,
        option = modifiers.alternate,
        shift = modifiers.shift,
        caps_lock = modifiers.alpha_shift,
        "Mac remote modifier injected"
    );
}

#[cfg(any(target_os = "macos", test))]
fn mouse_button_number(button: MouseButton) -> i64 {
    match button {
        MouseButton::Left => 0,
        MouseButton::Right => 1,
        MouseButton::Middle => 2,
        MouseButton::Back => 3,
        MouseButton::Forward => 4,
    }
}

#[derive(Debug, Clone, Copy)]
enum MouseMoveKind {
    Moved,
    LeftDrag,
    RightDrag,
    OtherDrag(MouseButton),
}

#[cfg(any(target_os = "macos", test))]
fn pointer_sync_check_due(last_check: Option<Instant>, now: Instant) -> bool {
    last_check.is_none_or(|last_check| {
        now.saturating_duration_since(last_check) >= POINTER_SYNC_CHECK_INTERVAL
    })
}

#[cfg(any(target_os = "macos", test))]
fn pointer_position_drifted(
    expected_x: f64,
    expected_y: f64,
    actual_x: f64,
    actual_y: f64,
    tolerance: f64,
) -> bool {
    (expected_x - actual_x).abs() > tolerance || (expected_y - actual_y).abs() > tolerance
}

#[cfg(target_os = "macos")]
fn current_mouse_location(source: &RemoteEventSource) -> Result<(f64, f64), InjectionError> {
    use core_graphics::event::CGEvent;

    let event = CGEvent::new(source.clone()).map_err(|_| InjectionError::Unavailable)?;
    let location = event.location();
    Ok((location.x, location.y))
}

#[cfg(target_os = "macos")]
fn warp_mouse_cursor(x: f64, y: f64) -> Result<(), InjectionError> {
    use core_graphics::{display::CGDisplay, geometry::CGPoint};

    CGDisplay::warp_mouse_cursor_position(CGPoint::new(x, y))
        .map_err(|_| InjectionError::Unavailable)
}

#[cfg(not(target_os = "macos"))]
fn warp_mouse_cursor(_x: f64, _y: f64) -> Result<(), InjectionError> {
    Err(InjectionError::Unavailable)
}

#[cfg(target_os = "macos")]
fn post_mouse_move(
    source: &RemoteEventSource,
    x: f64,
    y: f64,
    kind: MouseMoveKind,
    click_count: u8,
    modifiers: ModifierState,
) -> Result<(), InjectionError> {
    use core_graphics::{
        event::{CGEvent, CGEventTapLocation, CGEventType, CGMouseButton, EventField},
        geometry::CGPoint,
    };

    let event_type = match kind {
        MouseMoveKind::Moved => CGEventType::MouseMoved,
        MouseMoveKind::LeftDrag => CGEventType::LeftMouseDragged,
        MouseMoveKind::RightDrag => CGEventType::RightMouseDragged,
        MouseMoveKind::OtherDrag(_) => CGEventType::OtherMouseDragged,
    };
    let logical_button = match kind {
        MouseMoveKind::RightDrag => MouseButton::Right,
        MouseMoveKind::OtherDrag(button) => button,
        MouseMoveKind::Moved | MouseMoveKind::LeftDrag => MouseButton::Left,
    };
    let button = match logical_button {
        MouseButton::Left => CGMouseButton::Left,
        MouseButton::Right => CGMouseButton::Right,
        MouseButton::Middle | MouseButton::Back | MouseButton::Forward => CGMouseButton::Center,
    };
    let event = CGEvent::new_mouse_event(source.clone(), event_type, CGPoint::new(x, y), button)
        .map_err(|_| InjectionError::Unavailable)?;
    event.set_integer_value_field(
        EventField::MOUSE_EVENT_BUTTON_NUMBER,
        mouse_button_number(logical_button),
    );
    event.set_integer_value_field(EventField::MOUSE_EVENT_CLICK_STATE, i64::from(click_count));
    event.set_flags(modifiers.event_flags());
    event.post(CGEventTapLocation::HID);
    Ok(())
}

#[cfg(not(target_os = "macos"))]
fn post_mouse_move(
    _source: &RemoteEventSource,
    _x: f64,
    _y: f64,
    _kind: MouseMoveKind,
    _click_count: u8,
    _modifiers: ModifierState,
) -> Result<(), InjectionError> {
    Err(InjectionError::Unavailable)
}

#[cfg(target_os = "macos")]
fn post_mouse_button(
    source: &RemoteEventSource,
    button: MouseButton,
    down: bool,
    x: f64,
    y: f64,
    click_count: u8,
    modifiers: ModifierState,
) -> Result<(), InjectionError> {
    use core_graphics::event::CGEventTapLocation;

    let event = create_mouse_button_event(source, button, down, x, y, click_count, modifiers)?;
    event.post(CGEventTapLocation::HID);
    Ok(())
}

#[cfg(target_os = "macos")]
fn create_mouse_button_event(
    source: &RemoteEventSource,
    button: MouseButton,
    down: bool,
    x: f64,
    y: f64,
    click_count: u8,
    modifiers: ModifierState,
) -> Result<core_graphics::event::CGEvent, InjectionError> {
    use core_graphics::{
        event::{CGEvent, CGEventType, CGMouseButton, EventField},
        geometry::CGPoint,
    };

    let (event_type, cg_button) = match (button, down) {
        (MouseButton::Left, true) => (CGEventType::LeftMouseDown, CGMouseButton::Left),
        (MouseButton::Left, false) => (CGEventType::LeftMouseUp, CGMouseButton::Left),
        (MouseButton::Right, true) => (CGEventType::RightMouseDown, CGMouseButton::Right),
        (MouseButton::Right, false) => (CGEventType::RightMouseUp, CGMouseButton::Right),
        (_, true) => (CGEventType::OtherMouseDown, CGMouseButton::Center),
        (_, false) => (CGEventType::OtherMouseUp, CGMouseButton::Center),
    };
    let event = CGEvent::new_mouse_event(source.clone(), event_type, CGPoint::new(x, y), cg_button)
        .map_err(|_| InjectionError::Unavailable)?;
    event.set_integer_value_field(
        EventField::MOUSE_EVENT_BUTTON_NUMBER,
        mouse_button_number(button),
    );
    event.set_integer_value_field(EventField::MOUSE_EVENT_CLICK_STATE, i64::from(click_count));
    event.set_flags(modifiers.event_flags());
    Ok(event)
}

#[cfg(not(target_os = "macos"))]
fn post_mouse_button(
    _source: &RemoteEventSource,
    _button: MouseButton,
    _down: bool,
    _x: f64,
    _y: f64,
    _click_count: u8,
    _modifiers: ModifierState,
) -> Result<(), InjectionError> {
    Err(InjectionError::Unavailable)
}

#[cfg(target_os = "macos")]
fn post_wheel(
    source: &RemoteEventSource,
    delta_x: i32,
    delta_y: i32,
    modifiers: ModifierState,
) -> Result<(), InjectionError> {
    use core_graphics::event::{CGEvent, CGEventTapLocation, ScrollEventUnit};

    let event = CGEvent::new_scroll_event(
        source.clone(),
        ScrollEventUnit::PIXEL,
        2,
        delta_y,
        delta_x,
        0,
    )
    .map_err(|_| InjectionError::Unavailable)?;
    event.set_flags(modifiers.event_flags());
    event.post(CGEventTapLocation::HID);
    Ok(())
}

#[cfg(not(target_os = "macos"))]
fn post_wheel(
    _source: &RemoteEventSource,
    _delta_x: i32,
    _delta_y: i32,
    _modifiers: ModifierState,
) -> Result<(), InjectionError> {
    Err(InjectionError::Unavailable)
}

#[cfg(target_os = "macos")]
fn post_key(
    source: &RemoteEventSource,
    usage: KeyUsage,
    down: bool,
    modifiers: ModifierState,
) -> Result<(), InjectionError> {
    use core_graphics::event::CGEventTapLocation;

    let event = create_key_event(source, usage, down, modifiers)?;
    event.post(CGEventTapLocation::HID);
    Ok(())
}

#[cfg(target_os = "macos")]
struct HidKeyboardPoster {
    connection: objc2_io_kit::io_connect_t,
}

#[cfg(target_os = "macos")]
impl HidKeyboardPoster {
    #[allow(deprecated)]
    fn open() -> Result<Self, HidKeyboardError> {
        use objc2_io_kit::{
            IOHIDAccessType, IOHIDCheckAccess, IOHIDRequestAccess, IOHIDRequestType,
            IOServiceMatching, IOServiceOpen, kIOHIDParamConnectType,
        };
        use std::ffi::c_void;

        unsafe extern "C-unwind" {
            fn IOServiceGetMatchingService(
                main_port: libc::mach_port_t,
                matching: *mut c_void,
            ) -> objc2_io_kit::io_service_t;
        }

        if IOHIDCheckAccess(IOHIDRequestType::PostEvent) == IOHIDAccessType::Unknown {
            let _ = IOHIDRequestAccess(IOHIDRequestType::PostEvent);
        }
        if IOHIDCheckAccess(IOHIDRequestType::PostEvent) != IOHIDAccessType::Granted {
            return Err(HidKeyboardError::PermissionDenied);
        }

        let matching = unsafe { IOServiceMatching(c"IOHIDSystem".as_ptr()) }
            .ok_or(HidKeyboardError::ServiceMissing)?;
        let matching = objc2_core_foundation::CFRetained::into_raw(matching);
        let service = unsafe { IOServiceGetMatchingService(0, matching.as_ptr().cast()) };
        if service == 0 {
            return Err(HidKeyboardError::ServiceMissing);
        }

        let mut connection = 0;
        let result = unsafe {
            IOServiceOpen(
                service,
                libc::mach_task_self(),
                kIOHIDParamConnectType,
                &mut connection,
            )
        };
        let _ = objc2_io_kit::IOObjectRelease(service);
        if result != 0 {
            return Err(HidKeyboardError::IoKit(result));
        }
        Ok(Self { connection })
    }

    #[allow(deprecated)]
    fn post(
        &self,
        usage: KeyUsage,
        down: bool,
        modifiers: ModifierState,
    ) -> Result<(), HidKeyboardError> {
        // Let macOS interpret the complete press duration. Posting an additional flags-changed
        // event here preempts the system's short-press input-source and long-press lock behavior.
        if usage == KeyUsage::CAPS_LOCK {
            self.post_caps_lock_auxiliary(
                down,
                caps_lock_auxiliary_modifiers(modifiers, system_alpha_shift_active()),
            )?;
            return Ok(());
        }

        let event_type = hid_key_event_type(usage, down);
        self.post_raw(
            event_type,
            keyboard_event_data(usage)?,
            hid_key_event_flags(usage, modifiers),
            hid_key_event_options(event_type),
        )
    }

    #[allow(deprecated)]
    fn post_caps_lock_auxiliary(
        &self,
        down: bool,
        modifiers: ModifierState,
    ) -> Result<(), HidKeyboardError> {
        use objc2_io_kit::{NXEventData, NXEventData_compound, NXEventData_compound_misc};

        let mut values = [0; 11];
        values[0] = caps_lock_auxiliary_data(down);
        let data = NXEventData {
            compound: NXEventData_compound {
                reserved: 0,
                subType: NX_SUBTYPE_AUX_CONTROL_BUTTONS,
                misc: NXEventData_compound_misc { L: values },
            },
        };
        self.post_raw(NX_SYSTEM_DEFINED, data, modifiers.hid_event_flags(), 0)
    }

    #[allow(deprecated)]
    fn post_raw(
        &self,
        event_type: u32,
        event_data: objc2_io_kit::NXEventData,
        event_flags: u32,
        options: u32,
    ) -> Result<(), HidKeyboardError> {
        use objc2_io_kit::{IOGPoint, IOHIDPostEvent, kNXEventDataVersion};

        let result = unsafe {
            IOHIDPostEvent(
                self.connection,
                event_type,
                IOGPoint { x: 0, y: 0 },
                &event_data,
                kNXEventDataVersion,
                event_flags,
                options,
            )
        };
        if result == 0 {
            Ok(())
        } else {
            Err(HidKeyboardError::IoKit(result))
        }
    }
}

#[cfg(target_os = "macos")]
impl Drop for HidKeyboardPoster {
    fn drop(&mut self) {
        let _ = objc2_io_kit::IOServiceClose(self.connection);
    }
}

#[cfg(target_os = "macos")]
fn keyboard_event_data(usage: KeyUsage) -> Result<objc2_io_kit::NXEventData, HidKeyboardError> {
    use objc2_io_kit::{NXEventData, NXEventData_key};

    let keycode = mac_keycode(usage).ok_or(HidKeyboardError::UnsupportedKey(usage.0))?;
    Ok(NXEventData {
        key: NXEventData_key {
            origCharSet: 0,
            repeat: 0,
            charSet: 0,
            charCode: 0,
            keyCode: keycode,
            origCharCode: 0,
            reserved1: 0,
            keyboardType: 0,
            reserved2: 0,
            reserved3: 0,
            reserved4: 0,
            reserved5: [0; 4],
        },
    })
}

#[cfg(any(target_os = "macos", test))]
fn caps_lock_auxiliary_data(down: bool) -> i32 {
    let action = if down { 0x0a } else { 0x0b };
    (NX_KEYTYPE_CAPS_LOCK << 16) | (action << 8)
}

#[cfg(any(target_os = "macos", test))]
fn caps_lock_auxiliary_modifiers(
    modifiers: ModifierState,
    system_alpha_shift_active: bool,
) -> ModifierState {
    modifiers.with_alpha_shift(system_alpha_shift_active)
}

#[cfg(any(target_os = "macos", test))]
fn hid_key_event_type(usage: KeyUsage, down: bool) -> u32 {
    if is_modifier_usage(usage) || usage == KeyUsage::CAPS_LOCK {
        NX_FLAGS_CHANGED
    } else if down {
        NX_KEY_DOWN
    } else {
        NX_KEY_UP
    }
}

#[cfg(any(target_os = "macos", test))]
fn hid_key_event_options(event_type: u32) -> u32 {
    if event_type == NX_FLAGS_CHANGED {
        IOHID_SET_GLOBAL_EVENT_FLAGS
    } else {
        0
    }
}

#[cfg(target_os = "macos")]
fn hid_key_event_flags(usage: KeyUsage, modifiers: ModifierState) -> u32 {
    let mut flags = modifiers.hid_event_flags();
    if needs_numeric_pad_flag(usage) {
        flags |= core_graphics::event::CGEventFlags::CGEventFlagNumericPad.bits() as u32;
    }
    flags
}

#[cfg(target_os = "macos")]
#[derive(Debug, thiserror::Error)]
enum HidKeyboardError {
    #[error("input permission was not granted")]
    PermissionDenied,
    #[error("IOHIDSystem service is unavailable")]
    ServiceMissing,
    #[error("IOKit returned {0}")]
    IoKit(i32),
    #[error("unsupported HID key usage {0:#x}")]
    UnsupportedKey(u16),
    #[error("IOHID keyboard injection is unavailable")]
    Unavailable,
}

#[cfg(target_os = "macos")]
fn create_key_event(
    source: &RemoteEventSource,
    usage: KeyUsage,
    down: bool,
    modifiers: ModifierState,
) -> Result<core_graphics::event::CGEvent, InjectionError> {
    use core_graphics::event::{CGEvent, CGEventType};

    let keycode = mac_keycode(usage).ok_or(InjectionError::UnsupportedKey(usage.0))?;
    let event = CGEvent::new_keyboard_event(source.clone(), keycode, down)
        .map_err(|_| InjectionError::Unavailable)?;
    let mut flags = modifiers.event_flags();
    flags.set(
        core_graphics::event::CGEventFlags::CGEventFlagNumericPad,
        needs_numeric_pad_flag(usage),
    );
    event.set_flags(flags);
    if remote_key_event_kind(usage) == RemoteKeyEventKind::FlagsChanged {
        event.set_type(CGEventType::FlagsChanged);
    }
    Ok(event)
}

#[cfg(target_os = "macos")]
fn system_alpha_shift_active() -> bool {
    use core_graphics::event::CGEventFlags;

    unsafe extern "C" {
        fn CGEventSourceFlagsState(state_id: i32) -> u64;
    }

    let flags = unsafe { CGEventSourceFlagsState(0) };
    CGEventFlags::from_bits_retain(flags).contains(CGEventFlags::CGEventFlagAlphaShift)
}

#[cfg(not(target_os = "macos"))]
fn system_alpha_shift_active() -> bool {
    false
}

#[cfg(not(target_os = "macos"))]
fn post_key(
    _source: &RemoteEventSource,
    usage: KeyUsage,
    _down: bool,
    _modifiers: ModifierState,
) -> Result<(), InjectionError> {
    Err(InjectionError::UnsupportedKey(usage.0))
}

#[cfg(target_os = "macos")]
fn remote_event_source() -> Result<core_graphics::event_source::CGEventSource, InjectionError> {
    use core_graphics::event_source::{CGEventSource, CGEventSourceStateID};

    CGEventSource::new(CGEventSourceStateID::CombinedSessionState)
        .map_err(|_| InjectionError::Unavailable)
}

#[cfg(not(target_os = "macos"))]
fn remote_event_source() -> Result<RemoteEventSource, InjectionError> {
    Err(InjectionError::Unavailable)
}

#[cfg(any(target_os = "macos", test))]
pub fn mac_keycode(usage: KeyUsage) -> Option<u16> {
    const LETTERS: [u16; 26] = [
        0x00, 0x0b, 0x08, 0x02, 0x0e, 0x03, 0x05, 0x04, 0x22, 0x26, 0x28, 0x25, 0x2e, 0x2d, 0x1f,
        0x23, 0x0c, 0x0f, 0x01, 0x11, 0x20, 0x09, 0x0d, 0x07, 0x10, 0x06,
    ];
    const DIGITS: [u16; 10] = [0x12, 0x13, 0x14, 0x15, 0x17, 0x16, 0x1a, 0x1c, 0x19, 0x1d];

    match usage.0 {
        0x04..=0x1d => LETTERS.get((usage.0 - 0x04) as usize).copied(),
        0x1e..=0x27 => DIGITS.get((usage.0 - 0x1e) as usize).copied(),
        0x28 => Some(0x24),
        0x29 => Some(0x35),
        0x2a => Some(0x33),
        0x2b => Some(0x30),
        0x2c => Some(0x31),
        0x2d => Some(0x1b),
        0x2e => Some(0x18),
        0x2f => Some(0x21),
        0x30 => Some(0x1e),
        0x31 => Some(0x2a),
        0x33 => Some(0x29),
        0x34 => Some(0x27),
        0x35 => Some(0x32),
        0x36 => Some(0x2b),
        0x37 => Some(0x2f),
        0x38 => Some(0x2c),
        0x39 => Some(0x39),
        0x3a => Some(0x7a),
        0x3b => Some(0x78),
        0x3c => Some(0x63),
        0x3d => Some(0x76),
        0x3e => Some(0x60),
        0x3f => Some(0x61),
        0x40 => Some(0x62),
        0x41 => Some(0x64),
        0x42 => Some(0x65),
        0x43 => Some(0x6d),
        0x44 => Some(0x67),
        0x45 => Some(0x6f),
        0x4a => Some(0x73),
        0x4b => Some(0x74),
        0x4c => Some(0x75),
        0x4d => Some(0x77),
        0x4e => Some(0x79),
        0x4f => Some(0x7c),
        0x50 => Some(0x7b),
        0x51 => Some(0x7d),
        0x52 => Some(0x7e),
        0x53 => Some(0x47),
        0x54 => Some(0x4b),
        0x55 => Some(0x43),
        0x56 => Some(0x4e),
        0x57 => Some(0x45),
        0x58 => Some(0x4c),
        0x59 => Some(0x53),
        0x5a => Some(0x54),
        0x5b => Some(0x55),
        0x5c => Some(0x56),
        0x5d => Some(0x57),
        0x5e => Some(0x58),
        0x5f => Some(0x59),
        0x60 => Some(0x5b),
        0x61 => Some(0x5c),
        0x62 => Some(0x52),
        0x63 => Some(0x41),
        0xe0 => Some(0x3b),
        0xe1 => Some(0x38),
        0xe2 => Some(0x3a),
        0xe3 => Some(0x37),
        0xe4 => Some(0x3e),
        0xe5 => Some(0x3c),
        0xe6 => Some(0x3d),
        0xe7 => Some(0x36),
        _ => None,
    }
}

#[cfg(target_os = "macos")]
fn main_display_bounds() -> Result<DisplayBounds, InjectionError> {
    let bounds = core_graphics::display::CGDisplay::main().bounds();
    Ok(DisplayBounds {
        origin_x: bounds.origin.x,
        origin_y: bounds.origin.y,
        width: bounds.size.width,
        height: bounds.size.height,
    })
}

#[cfg(not(target_os = "macos"))]
fn main_display_bounds() -> Result<DisplayBounds, InjectionError> {
    Err(InjectionError::Unavailable)
}

#[derive(Debug, thiserror::Error)]
pub enum InjectionError {
    #[error("CoreGraphics event injection is unavailable")]
    Unavailable,
    #[error("unsupported HID key usage {0:#x}")]
    UnsupportedKey(u16),
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn maps_hid_letters_navigation_and_windows_shortcut_modifiers() {
        assert_eq!(mac_keycode(KeyUsage::A), Some(0x00));
        assert_eq!(mac_keycode(KeyUsage::Z), Some(0x06));
        assert_eq!(mac_keycode(KeyUsage::LEFT_ARROW), Some(0x7b));
        assert_eq!(mac_keycode(KeyUsage::LEFT_GUI), Some(0x37));
        assert_eq!(mac_keycode(KeyUsage::LEFT_CONTROL), Some(0x3b));
    }

    #[test]
    fn maps_hid_number_row_in_one_through_zero_order() {
        let expected = [0x12, 0x13, 0x14, 0x15, 0x17, 0x16, 0x1a, 0x1c, 0x19, 0x1d];

        for (usage, keycode) in (0x1e..=0x27).zip(expected) {
            assert_eq!(mac_keycode(KeyUsage(usage)), Some(keycode));
        }
    }

    #[test]
    fn maps_caps_lock_forward_delete_and_preserves_modifier_side() {
        assert_eq!(mac_keycode(KeyUsage(0x39)), Some(0x39));
        assert_eq!(mac_keycode(KeyUsage::BACKSPACE), Some(0x33));
        assert_eq!(mac_keycode(KeyUsage::FORWARD_DELETE), Some(0x75));
        assert_eq!(mac_keycode(KeyUsage::LEFT_CONTROL), Some(0x3b));
        assert_eq!(mac_keycode(KeyUsage::RIGHT_CONTROL), Some(0x3e));
        assert_eq!(mac_keycode(KeyUsage::LEFT_SHIFT), Some(0x38));
        assert_eq!(mac_keycode(KeyUsage::RIGHT_SHIFT), Some(0x3c));
        assert_eq!(mac_keycode(KeyUsage::LEFT_ALT), Some(0x3a));
        assert_eq!(mac_keycode(KeyUsage::RIGHT_ALT), Some(0x3d));
        assert_eq!(mac_keycode(KeyUsage::LEFT_GUI), Some(0x37));
        assert_eq!(mac_keycode(KeyUsage::RIGHT_GUI), Some(0x36));
    }

    #[test]
    fn maps_numeric_keypad_usages() {
        assert_eq!(mac_keycode(KeyUsage::NUM_LOCK), Some(0x47));
        assert_eq!(mac_keycode(KeyUsage::KEYPAD_ENTER), Some(0x4c));
        assert_eq!(mac_keycode(KeyUsage(0x59)), Some(0x53));
        assert_eq!(mac_keycode(KeyUsage(0x61)), Some(0x5c));
        assert_eq!(mac_keycode(KeyUsage(0x62)), Some(0x52));
        assert_eq!(mac_keycode(KeyUsage(0x63)), Some(0x41));
    }

    #[test]
    fn lock_and_modifier_keys_do_not_repeat() {
        assert!(is_non_repeating_key(KeyUsage::CAPS_LOCK));
        assert!(is_non_repeating_key(KeyUsage::LEFT_GUI));
        assert!(!is_non_repeating_key(KeyUsage::A));
    }

    #[test]
    fn derives_explicit_modifier_state_from_held_remote_keys() {
        let held_keys = HashSet::from([
            KeyUsage::LEFT_GUI,
            KeyUsage::RIGHT_SHIFT,
            KeyUsage::RIGHT_ALT,
        ]);

        assert_eq!(
            ModifierState::from_held_keys(&held_keys),
            ModifierState {
                alpha_shift: false,
                command: true,
                shift: true,
                alternate: true,
                control: false,
                device_flags: DEVICE_LEFT_COMMAND_FLAG
                    | DEVICE_RIGHT_SHIFT_FLAG
                    | DEVICE_RIGHT_OPTION_FLAG,
            }
        );
    }

    #[test]
    fn names_only_safe_control_keys_for_diagnostics() {
        assert_eq!(
            keyboard_semantic_name(KeyUsage::LEFT_GUI),
            Some("command_left")
        );
        assert_eq!(
            keyboard_semantic_name(KeyUsage::CAPS_LOCK),
            Some("caps_lock")
        );
        assert_eq!(
            keyboard_semantic_name(KeyUsage::BACKSPACE),
            Some("backspace")
        );
        assert_eq!(
            keyboard_semantic_name(KeyUsage::FORWARD_DELETE),
            Some("forward_delete")
        );
        assert_eq!(keyboard_semantic_name(KeyUsage::A), None);
    }

    #[test]
    fn shortcut_modifier_detection_ignores_plain_and_caps_lock_state() {
        assert!(!ModifierState::default().has_shortcut_modifier());
        assert!(
            !ModifierState::default()
                .with_alpha_shift(true)
                .has_shortcut_modifier()
        );
        assert!(
            ModifierState {
                command: true,
                ..ModifierState::default()
            }
            .has_shortcut_modifier()
        );
    }

    #[test]
    fn mouse_timing_stats_classify_active_jitter_without_counting_pauses() {
        let started = Instant::now();
        let mut stats = InputInjectionStats::default();

        stats.record_mouse_move(started);
        stats.record_mouse_move(started + Duration::from_millis(4));
        stats.record_mouse_move(started + Duration::from_millis(24));
        stats.record_mouse_move(started + Duration::from_millis(25));
        stats.record_mouse_move(started + Duration::from_millis(225));

        assert_eq!(stats.mouse_move_events, 5);
        assert_eq!(stats.mouse_move_active_intervals, 3);
        assert_eq!(stats.mouse_move_max_active_interval_us, 20_000);
        assert_eq!(stats.mouse_move_delayed_intervals, 1);
        assert_eq!(stats.mouse_move_burst_intervals, 1);
    }

    #[test]
    fn pointer_sync_check_is_rate_limited() {
        let started = Instant::now();

        assert!(pointer_sync_check_due(None, started));
        assert!(!pointer_sync_check_due(
            Some(started),
            started + POINTER_SYNC_CHECK_INTERVAL - Duration::from_millis(1)
        ));
        assert!(pointer_sync_check_due(
            Some(started),
            started + POINTER_SYNC_CHECK_INTERVAL
        ));
    }

    #[test]
    fn pointer_sync_ignores_small_delivery_lag_but_detects_stalls() {
        assert!(!pointer_position_drifted(
            500.0,
            400.0,
            500.0 + POINTER_SYNC_TOLERANCE,
            400.0 - POINTER_SYNC_TOLERANCE,
            POINTER_SYNC_TOLERANCE,
        ));
        assert!(pointer_position_drifted(
            500.0,
            400.0,
            500.0 + POINTER_SYNC_TOLERANCE + 1.0,
            400.0,
            POINTER_SYNC_TOLERANCE,
        ));
    }

    #[test]
    fn consecutive_clicks_keep_count_until_timeout_or_pointer_movement() {
        let started = Instant::now();
        let mut tracker = ClickTracker::default();

        assert_eq!(
            tracker.observe(MouseButton::Left, true, 100.0, 100.0, started),
            1
        );
        assert_eq!(
            tracker.observe(
                MouseButton::Left,
                false,
                100.0,
                100.0,
                started + Duration::from_millis(30)
            ),
            1
        );
        assert_eq!(
            tracker.observe(
                MouseButton::Left,
                true,
                102.0,
                99.0,
                started + Duration::from_millis(250)
            ),
            2
        );
        assert_eq!(tracker.active_count(MouseButton::Left), 2);
        assert_eq!(
            tracker.observe(
                MouseButton::Left,
                false,
                102.0,
                99.0,
                started + Duration::from_millis(280)
            ),
            2
        );
        assert_eq!(
            tracker.observe(
                MouseButton::Left,
                true,
                110.0,
                99.0,
                started + Duration::from_millis(350)
            ),
            1
        );
        assert_eq!(
            tracker.observe(
                MouseButton::Left,
                true,
                110.0,
                99.0,
                started + MULTI_CLICK_INTERVAL + Duration::from_millis(351)
            ),
            1
        );
    }

    #[test]
    fn changing_button_or_session_resets_multi_click_state() {
        let started = Instant::now();
        let mut tracker = ClickTracker::default();

        assert_eq!(
            tracker.observe(MouseButton::Left, true, 20.0, 20.0, started),
            1
        );
        assert_eq!(
            tracker.observe(
                MouseButton::Right,
                true,
                20.0,
                20.0,
                started + Duration::from_millis(100)
            ),
            1
        );
        tracker.reset();
        assert_eq!(
            tracker.observe(
                MouseButton::Left,
                true,
                20.0,
                20.0,
                started + Duration::from_millis(150)
            ),
            1
        );
    }

    #[test]
    fn classifies_modifier_lock_and_navigation_key_events() {
        assert_eq!(
            remote_key_event_kind(KeyUsage::LEFT_GUI),
            RemoteKeyEventKind::FlagsChanged
        );
        assert_eq!(
            remote_key_event_kind(KeyUsage::CAPS_LOCK),
            RemoteKeyEventKind::FlagsChanged
        );
        assert_eq!(
            remote_key_event_kind(KeyUsage::UP_ARROW),
            RemoteKeyEventKind::Keyboard
        );
        assert!(needs_numeric_pad_flag(KeyUsage::UP_ARROW));
        assert!(needs_numeric_pad_flag(KeyUsage::FORWARD_DELETE));
        assert!(needs_numeric_pad_flag(KeyUsage::NUM_LOCK));
        assert!(!needs_numeric_pad_flag(KeyUsage::A));
        assert_eq!(
            hid_key_event_type(KeyUsage::LEFT_CONTROL, true),
            NX_FLAGS_CHANGED
        );
        assert_eq!(
            hid_key_event_options(NX_FLAGS_CHANGED),
            IOHID_SET_GLOBAL_EVENT_FLAGS
        );
        assert_eq!(hid_key_event_type(KeyUsage::LEFT_ARROW, true), NX_KEY_DOWN);
        assert_eq!(hid_key_event_type(KeyUsage::LEFT_ARROW, false), NX_KEY_UP);
        assert_eq!(hid_key_event_options(NX_KEY_DOWN), 0);
    }

    #[test]
    fn caps_lock_toggles_the_current_system_state_on_key_down() {
        assert!(alpha_shift_for_event(KeyUsage::CAPS_LOCK, true, false));
        assert!(!alpha_shift_for_event(KeyUsage::CAPS_LOCK, true, true));
        assert!(alpha_shift_for_event(KeyUsage::CAPS_LOCK, false, true));
        assert!(!alpha_shift_for_event(KeyUsage::CAPS_LOCK, false, false));
        assert!(alpha_shift_for_event(KeyUsage::A, true, true));
        assert!(!alpha_shift_for_event(KeyUsage::A, false, false));
        assert_eq!(caps_lock_auxiliary_data(true), 0x0004_0a00);
        assert_eq!(caps_lock_auxiliary_data(false), 0x0004_0b00);
        assert!(
            caps_lock_auxiliary_modifiers(ModifierState::default().with_alpha_shift(false), true)
                .alpha_shift
        );
        assert!(
            !caps_lock_auxiliary_modifiers(ModifierState::default().with_alpha_shift(true), false)
                .alpha_shift
        );
    }

    #[test]
    fn assigns_distinct_core_graphics_numbers_to_extended_mouse_buttons() {
        assert_eq!(mouse_button_number(MouseButton::Middle), 2);
        assert_eq!(mouse_button_number(MouseButton::Back), 3);
        assert_eq!(mouse_button_number(MouseButton::Forward), 4);
    }

    #[test]
    fn right_entry_returns_from_the_mac_left_edge_at_the_same_height() {
        let mut pointer = ActiveRemotePointer {
            entry_edge: ScreenEdge::Right,
            bounds: DisplayBounds {
                origin_x: 0.0,
                origin_y: 0.0,
                width: 1920.0,
                height: 1001.0,
            },
            return_armed: true,
            return_pending: false,
        };

        assert_eq!(pointer.observe_return(1.0, 500.0, false), None);
        assert_eq!(pointer.observe_return(0.0, 500.0, false), Some(0.5));
    }

    #[test]
    fn left_entry_returns_from_the_mac_right_edge() {
        let mut pointer = ActiveRemotePointer {
            entry_edge: ScreenEdge::Left,
            bounds: DisplayBounds {
                origin_x: 10.0,
                origin_y: 20.0,
                width: 101.0,
                height: 101.0,
            },
            return_armed: true,
            return_pending: false,
        };

        assert_eq!(pointer.observe_return(109.0, 70.0, false), None);
        assert_eq!(pointer.observe_return(110.0, 70.0, false), Some(0.5));
    }

    #[test]
    fn bottom_entry_returns_from_the_mac_top_edge_at_the_same_width() {
        let mut pointer = ActiveRemotePointer {
            entry_edge: ScreenEdge::Bottom,
            bounds: DisplayBounds {
                origin_x: 10.0,
                origin_y: 20.0,
                width: 1001.0,
                height: 800.0,
            },
            return_armed: true,
            return_pending: false,
        };

        assert_eq!(pointer.observe_return(510.0, 21.0, false), None);
        assert_eq!(pointer.observe_return(510.0, 20.0, false), Some(0.5));
    }

    #[test]
    fn top_entry_returns_from_the_mac_bottom_edge() {
        let mut pointer = ActiveRemotePointer {
            entry_edge: ScreenEdge::Top,
            bounds: DisplayBounds {
                origin_x: 0.0,
                origin_y: 0.0,
                width: 1001.0,
                height: 801.0,
            },
            return_armed: true,
            return_pending: false,
        };

        assert_eq!(pointer.observe_return(250.0, 799.0, false), None);
        assert_eq!(pointer.observe_return(250.0, 800.0, false), Some(0.25));
    }

    #[test]
    fn held_mouse_button_or_pending_return_blocks_edge_exit() {
        let bounds = DisplayBounds {
            origin_x: 0.0,
            origin_y: 0.0,
            width: 100.0,
            height: 100.0,
        };
        let mut pointer = ActiveRemotePointer {
            entry_edge: ScreenEdge::Right,
            bounds,
            return_armed: true,
            return_pending: false,
        };
        let mut pending = ActiveRemotePointer {
            return_pending: true,
            ..pointer
        };

        assert_eq!(pointer.observe_return(0.0, 50.0, true), None);
        assert_eq!(pending.observe_return(0.0, 50.0, false), None);
    }

    #[test]
    fn return_edge_arms_only_after_the_pointer_moves_into_the_mac() {
        let mut pointer = ActiveRemotePointer {
            entry_edge: ScreenEdge::Bottom,
            bounds: DisplayBounds {
                origin_x: 0.0,
                origin_y: 0.0,
                width: 1001.0,
                height: 801.0,
            },
            return_armed: false,
            return_pending: false,
        };

        assert_eq!(pointer.observe_return(500.0, 0.0, false), None);
        assert_eq!(pointer.observe_return(500.0, 12.0, false), None);
        assert_eq!(pointer.observe_return(500.0, 0.0, false), Some(0.5));
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn explicit_modifier_state_sets_core_graphics_flags() {
        use core_graphics::event::CGEventFlags;

        let flags = ModifierState {
            alpha_shift: true,
            command: true,
            shift: true,
            alternate: false,
            control: true,
            device_flags: DEVICE_LEFT_COMMAND_FLAG
                | DEVICE_LEFT_SHIFT_FLAG
                | DEVICE_LEFT_CONTROL_FLAG,
        }
        .event_flags();

        assert!(flags.contains(CGEventFlags::CGEventFlagCommand));
        assert!(flags.contains(CGEventFlags::CGEventFlagAlphaShift));
        assert!(flags.contains(CGEventFlags::CGEventFlagShift));
        assert!(flags.contains(CGEventFlags::CGEventFlagControl));
        assert!(!flags.contains(CGEventFlags::CGEventFlagAlternate));
        assert_eq!(
            flags.bits()
                & (DEVICE_LEFT_COMMAND_FLAG | DEVICE_LEFT_SHIFT_FLAG | DEVICE_LEFT_CONTROL_FLAG),
            DEVICE_LEFT_COMMAND_FLAG | DEVICE_LEFT_SHIFT_FLAG | DEVICE_LEFT_CONTROL_FLAG
        );
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn iohid_events_keep_global_and_device_modifier_state() {
        use core_graphics::event::CGEventFlags;

        let command_arrow = hid_key_event_flags(
            KeyUsage::LEFT_ARROW,
            ModifierState {
                command: true,
                device_flags: DEVICE_LEFT_COMMAND_FLAG,
                ..ModifierState::default()
            },
        );
        assert_ne!(
            command_arrow & CGEventFlags::CGEventFlagCommand.bits() as u32,
            0
        );
        assert_ne!(command_arrow & DEVICE_LEFT_COMMAND_FLAG as u32, 0);
        assert_ne!(
            command_arrow & CGEventFlags::CGEventFlagNumericPad.bits() as u32,
            0
        );

        let control_arrow = hid_key_event_flags(
            KeyUsage::RIGHT_ARROW,
            ModifierState {
                control: true,
                device_flags: DEVICE_LEFT_CONTROL_FLAG,
                ..ModifierState::default()
            },
        );
        assert_ne!(
            control_arrow & CGEventFlags::CGEventFlagControl.bits() as u32,
            0
        );
        assert_ne!(control_arrow & DEVICE_LEFT_CONTROL_FLAG as u32, 0);

        let caps_lock = hid_key_event_flags(
            KeyUsage::CAPS_LOCK,
            ModifierState::default().with_alpha_shift(true),
        );
        assert_ne!(
            caps_lock & CGEventFlags::CGEventFlagAlphaShift.bits() as u32,
            0
        );
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn mouse_button_event_carries_multi_click_state() {
        use core_graphics::event::{CGEventType, EventField};

        let source = remote_event_source().expect("event source");
        let event = create_mouse_button_event(
            &source,
            MouseButton::Left,
            true,
            100.0,
            200.0,
            2,
            ModifierState::default(),
        )
        .expect("mouse event");

        assert!(matches!(event.get_type(), CGEventType::LeftMouseDown));
        assert_eq!(
            event.get_integer_value_field(EventField::MOUSE_EVENT_CLICK_STATE),
            2
        );
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn editing_delete_events_keep_the_expected_mac_keycodes() {
        use core_graphics::event::{CGEventFlags, CGEventType, EventField};

        let source = remote_event_source().expect("event source");
        let backspace =
            create_key_event(&source, KeyUsage::BACKSPACE, true, ModifierState::default())
                .expect("backspace event");
        let forward_delete = create_key_event(
            &source,
            KeyUsage::FORWARD_DELETE,
            true,
            ModifierState::default(),
        )
        .expect("forward delete event");

        assert!(matches!(backspace.get_type(), CGEventType::KeyDown));
        assert_eq!(
            backspace.get_integer_value_field(EventField::KEYBOARD_EVENT_KEYCODE),
            0x33
        );
        assert!(matches!(forward_delete.get_type(), CGEventType::KeyDown));
        assert_eq!(
            forward_delete.get_integer_value_field(EventField::KEYBOARD_EVENT_KEYCODE),
            0x75
        );
        assert!(
            forward_delete
                .get_flags()
                .contains(CGEventFlags::CGEventFlagNumericPad)
        );
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn caps_lock_and_command_arrow_carry_platform_event_semantics() {
        use core_graphics::event::{CGEventFlags, CGEventType};

        let source = remote_event_source().expect("event source");
        let caps_lock = create_key_event(
            &source,
            KeyUsage::CAPS_LOCK,
            true,
            ModifierState::default().with_alpha_shift(true),
        )
        .expect("caps lock event");
        let command_arrow = create_key_event(
            &source,
            KeyUsage::LEFT_ARROW,
            true,
            ModifierState {
                command: true,
                device_flags: DEVICE_LEFT_COMMAND_FLAG,
                ..ModifierState::default()
            },
        )
        .expect("command arrow event");

        assert!(matches!(caps_lock.get_type(), CGEventType::FlagsChanged));
        assert!(
            caps_lock
                .get_flags()
                .contains(CGEventFlags::CGEventFlagAlphaShift)
        );
        assert!(matches!(command_arrow.get_type(), CGEventType::KeyDown));
        assert!(
            command_arrow
                .get_flags()
                .contains(CGEventFlags::CGEventFlagCommand)
        );
        assert!(
            command_arrow
                .get_flags()
                .contains(CGEventFlags::CGEventFlagNumericPad)
        );
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn command_shift_letter_keeps_the_main_key_and_both_modifier_flags() {
        use core_graphics::event::{CGEventFlags, CGEventType, EventField};

        let source = remote_event_source().expect("event source");
        let event = create_key_event(
            &source,
            KeyUsage::A,
            true,
            ModifierState {
                command: true,
                shift: true,
                device_flags: DEVICE_LEFT_COMMAND_FLAG | DEVICE_LEFT_SHIFT_FLAG,
                ..ModifierState::default()
            },
        )
        .expect("command shift A event");

        assert!(matches!(event.get_type(), CGEventType::KeyDown));
        assert_eq!(
            event.get_integer_value_field(EventField::KEYBOARD_EVENT_KEYCODE),
            0x00
        );
        assert!(event.get_flags().contains(CGEventFlags::CGEventFlagCommand));
        assert!(event.get_flags().contains(CGEventFlags::CGEventFlagShift));
        assert_eq!(
            event.get_flags().bits() & (DEVICE_LEFT_COMMAND_FLAG | DEVICE_LEFT_SHIFT_FLAG),
            DEVICE_LEFT_COMMAND_FLAG | DEVICE_LEFT_SHIFT_FLAG
        );
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn remote_control_event_source_is_available() {
        assert!(remote_event_source().is_ok());
    }
}

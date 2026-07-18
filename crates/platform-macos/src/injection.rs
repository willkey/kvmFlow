use std::collections::HashSet;

use kvmflow_domain::{InputEvent, KeyAction, KeyUsage, MouseButton, ScreenEdge};

const RETURN_EDGE_REARM_DISTANCE: f64 = 12.0;

#[derive(Debug, Default)]
pub struct MacInjectionState {
    pointer_x: f64,
    pointer_y: f64,
    active_pointer: Option<ActiveRemotePointer>,
    held_keys: HashSet<KeyUsage>,
    held_buttons: HashSet<MouseButton>,
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
                if let Some(position) = return_position {
                    return Ok(InjectionOutcome::ReturnEdgeReached { position });
                }
                Ok(InjectionOutcome::Applied)
            }
            InputEvent::MouseButton { button, action } => {
                let down = action == KeyAction::Down;
                if down {
                    self.held_buttons.insert(button);
                } else {
                    self.held_buttons.remove(&button);
                }
                let result = post_mouse_button(
                    button,
                    down,
                    self.pointer_x,
                    self.pointer_y,
                    ModifierState::from_held_keys(&self.held_keys),
                );
                if result.is_ok() {
                    self.stats.record_mouse_button(button);
                }
                result.map(|_| InjectionOutcome::Applied)
            }
            InputEvent::Wheel { delta_x, delta_y } => {
                post_wheel(
                    i32::from(delta_x),
                    i32::from(delta_y),
                    ModifierState::from_held_keys(&self.held_keys),
                )?;
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
                let result = post_key(usage, down, ModifierState::from_held_keys(&self.held_keys));
                if result.is_ok() {
                    self.stats.record_key(usage, down);
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
        post_mouse_move(
            self.pointer_x,
            self.pointer_y,
            MouseMoveKind::Moved,
            ModifierState::default(),
        )
    }

    pub fn release_all(&mut self) -> Result<(), InjectionError> {
        let held_keys: Vec<_> = self.held_keys.drain().collect();
        let held_buttons: Vec<_> = self.held_buttons.drain().collect();
        let held_keys_released = held_keys.len();
        let held_buttons_released = held_buttons.len();
        let mut first_error = None;
        for usage in held_keys {
            if let Err(error) = post_key(usage, false, ModifierState::default())
                && first_error.is_none()
            {
                first_error = Some(error);
            }
        }
        for button in held_buttons {
            if let Err(error) = post_mouse_button(
                button,
                false,
                self.pointer_x,
                self.pointer_y,
                ModifierState::default(),
            ) && first_error.is_none()
            {
                first_error = Some(error);
            }
        }
        self.stats
            .log_and_reset(held_keys_released, held_buttons_released);
        self.active_pointer = None;
        if let Some(error) = first_error {
            return Err(error);
        }
        Ok(())
    }

    fn post_mouse_move(&self) -> Result<(), InjectionError> {
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
        post_mouse_move(
            self.pointer_x,
            self.pointer_y,
            kind,
            ModifierState::from_held_keys(&self.held_keys),
        )
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

#[derive(Debug, Default)]
struct InputInjectionStats {
    key_down_events: u64,
    key_up_events: u64,
    modifier_down_events: u64,
    modifier_up_events: u64,
    mouse_button_events: u64,
    back_button_events: u64,
    forward_button_events: u64,
}

impl InputInjectionStats {
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

    fn record_mouse_button(&mut self, button: MouseButton) {
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

    fn log_and_reset(&mut self, held_keys_released: usize, held_buttons_released: usize) {
        if self.key_down_events == 0
            && self.key_up_events == 0
            && self.mouse_button_events == 0
            && held_keys_released == 0
            && held_buttons_released == 0
        {
            return;
        }
        tracing::info!(
            component = "input",
            stage = "mac_injection",
            key_down_events = self.key_down_events,
            key_up_events = self.key_up_events,
            modifier_down_events = self.modifier_down_events,
            modifier_up_events = self.modifier_up_events,
            mouse_button_events = self.mouse_button_events,
            back_button_events = self.back_button_events,
            forward_button_events = self.forward_button_events,
            held_keys_released,
            held_buttons_released,
            "Mac input injection session summary"
        );
        *self = Self::default();
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
struct ModifierState {
    control: bool,
    shift: bool,
    alternate: bool,
    command: bool,
}

impl ModifierState {
    fn from_held_keys(held_keys: &HashSet<KeyUsage>) -> Self {
        Self {
            control: contains_either(held_keys, KeyUsage::LEFT_CONTROL, KeyUsage::RIGHT_CONTROL),
            shift: contains_either(held_keys, KeyUsage::LEFT_SHIFT, KeyUsage::RIGHT_SHIFT),
            alternate: contains_either(held_keys, KeyUsage::LEFT_ALT, KeyUsage::RIGHT_ALT),
            command: contains_either(held_keys, KeyUsage::LEFT_GUI, KeyUsage::RIGHT_GUI),
        }
    }

    #[cfg(target_os = "macos")]
    fn event_flags(self) -> core_graphics::event::CGEventFlags {
        use core_graphics::event::CGEventFlags;

        let mut flags = CGEventFlags::empty();
        flags.set(CGEventFlags::CGEventFlagControl, self.control);
        flags.set(CGEventFlags::CGEventFlagShift, self.shift);
        flags.set(CGEventFlags::CGEventFlagAlternate, self.alternate);
        flags.set(CGEventFlags::CGEventFlagCommand, self.command);
        flags
    }
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

#[cfg(target_os = "macos")]
fn post_mouse_move(
    x: f64,
    y: f64,
    kind: MouseMoveKind,
    modifiers: ModifierState,
) -> Result<(), InjectionError> {
    use core_graphics::{
        event::{CGEvent, CGEventTapLocation, CGEventType, CGMouseButton, EventField},
        geometry::CGPoint,
    };

    let source = remote_event_source()?;
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
    let event = CGEvent::new_mouse_event(source, event_type, CGPoint::new(x, y), button)
        .map_err(|_| InjectionError::Unavailable)?;
    event.set_integer_value_field(
        EventField::MOUSE_EVENT_BUTTON_NUMBER,
        mouse_button_number(logical_button),
    );
    event.set_flags(modifiers.event_flags());
    event.post(CGEventTapLocation::HID);
    Ok(())
}

#[cfg(not(target_os = "macos"))]
fn post_mouse_move(
    _x: f64,
    _y: f64,
    _kind: MouseMoveKind,
    _modifiers: ModifierState,
) -> Result<(), InjectionError> {
    Err(InjectionError::Unavailable)
}

#[cfg(target_os = "macos")]
fn post_mouse_button(
    button: MouseButton,
    down: bool,
    x: f64,
    y: f64,
    modifiers: ModifierState,
) -> Result<(), InjectionError> {
    use core_graphics::{
        event::{CGEvent, CGEventTapLocation, CGEventType, CGMouseButton, EventField},
        geometry::CGPoint,
    };

    let source = remote_event_source()?;
    let (event_type, cg_button) = match (button, down) {
        (MouseButton::Left, true) => (CGEventType::LeftMouseDown, CGMouseButton::Left),
        (MouseButton::Left, false) => (CGEventType::LeftMouseUp, CGMouseButton::Left),
        (MouseButton::Right, true) => (CGEventType::RightMouseDown, CGMouseButton::Right),
        (MouseButton::Right, false) => (CGEventType::RightMouseUp, CGMouseButton::Right),
        (_, true) => (CGEventType::OtherMouseDown, CGMouseButton::Center),
        (_, false) => (CGEventType::OtherMouseUp, CGMouseButton::Center),
    };
    let event = CGEvent::new_mouse_event(source, event_type, CGPoint::new(x, y), cg_button)
        .map_err(|_| InjectionError::Unavailable)?;
    event.set_integer_value_field(
        EventField::MOUSE_EVENT_BUTTON_NUMBER,
        mouse_button_number(button),
    );
    event.set_flags(modifiers.event_flags());
    event.post(CGEventTapLocation::HID);
    Ok(())
}

#[cfg(not(target_os = "macos"))]
fn post_mouse_button(
    _button: MouseButton,
    _down: bool,
    _x: f64,
    _y: f64,
    _modifiers: ModifierState,
) -> Result<(), InjectionError> {
    Err(InjectionError::Unavailable)
}

#[cfg(target_os = "macos")]
fn post_wheel(delta_x: i32, delta_y: i32, modifiers: ModifierState) -> Result<(), InjectionError> {
    use core_graphics::event::{CGEvent, CGEventTapLocation, ScrollEventUnit};

    let source = remote_event_source()?;
    let event = CGEvent::new_scroll_event(source, ScrollEventUnit::PIXEL, 2, delta_y, delta_x, 0)
        .map_err(|_| InjectionError::Unavailable)?;
    event.set_flags(modifiers.event_flags());
    event.post(CGEventTapLocation::HID);
    Ok(())
}

#[cfg(not(target_os = "macos"))]
fn post_wheel(
    _delta_x: i32,
    _delta_y: i32,
    _modifiers: ModifierState,
) -> Result<(), InjectionError> {
    Err(InjectionError::Unavailable)
}

#[cfg(target_os = "macos")]
fn post_key(usage: KeyUsage, down: bool, modifiers: ModifierState) -> Result<(), InjectionError> {
    use core_graphics::event::{CGEvent, CGEventFlags, CGEventTapLocation, CGEventType};

    let keycode = mac_keycode(usage).ok_or(InjectionError::UnsupportedKey(usage.0))?;
    let source = remote_event_source()?;
    let event = CGEvent::new_keyboard_event(source, keycode, down)
        .map_err(|_| InjectionError::Unavailable)?;
    let mut flags = modifiers.event_flags();
    if usage == KeyUsage::CAPS_LOCK {
        flags.set(CGEventFlags::CGEventFlagAlphaShift, down);
    }
    event.set_flags(flags);
    if is_modifier_usage(usage) || usage == KeyUsage::CAPS_LOCK {
        event.set_type(CGEventType::FlagsChanged);
    }
    event.post(CGEventTapLocation::HID);
    Ok(())
}

#[cfg(not(target_os = "macos"))]
fn post_key(usage: KeyUsage, _down: bool, _modifiers: ModifierState) -> Result<(), InjectionError> {
    Err(InjectionError::UnsupportedKey(usage.0))
}

#[cfg(target_os = "macos")]
fn remote_event_source() -> Result<core_graphics::event_source::CGEventSource, InjectionError> {
    use core_graphics::event_source::{CGEventSource, CGEventSourceStateID};

    CGEventSource::new(CGEventSourceStateID::CombinedSessionState)
        .map_err(|_| InjectionError::Unavailable)
}

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
        assert_eq!(mac_keycode(KeyUsage(0x4c)), Some(0x75));
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
                command: true,
                shift: true,
                alternate: true,
                control: false,
            }
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
            command: true,
            shift: true,
            alternate: false,
            control: true,
        }
        .event_flags();

        assert!(flags.contains(CGEventFlags::CGEventFlagCommand));
        assert!(flags.contains(CGEventFlags::CGEventFlagShift));
        assert!(flags.contains(CGEventFlags::CGEventFlagControl));
        assert!(!flags.contains(CGEventFlags::CGEventFlagAlternate));
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn remote_control_event_source_is_available() {
        assert!(remote_event_source().is_ok());
    }
}

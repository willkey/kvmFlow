use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct KeyUsage(pub u16);

impl KeyUsage {
    pub const A: Self = Self(0x04);
    pub const Z: Self = Self(0x1d);
    pub const ENTER: Self = Self(0x28);
    pub const ESCAPE: Self = Self(0x29);
    pub const BACKSPACE: Self = Self(0x2a);
    pub const TAB: Self = Self(0x2b);
    pub const SPACE: Self = Self(0x2c);
    pub const CAPS_LOCK: Self = Self(0x39);
    pub const F12: Self = Self(0x45);
    pub const RIGHT_ARROW: Self = Self(0x4f);
    pub const LEFT_ARROW: Self = Self(0x50);
    pub const DOWN_ARROW: Self = Self(0x51);
    pub const UP_ARROW: Self = Self(0x52);
    pub const NUM_LOCK: Self = Self(0x53);
    pub const KEYPAD_ENTER: Self = Self(0x58);
    pub const LEFT_CONTROL: Self = Self(0xe0);
    pub const LEFT_SHIFT: Self = Self(0xe1);
    pub const LEFT_ALT: Self = Self(0xe2);
    pub const LEFT_GUI: Self = Self(0xe3);
    pub const RIGHT_CONTROL: Self = Self(0xe4);
    pub const RIGHT_SHIFT: Self = Self(0xe5);
    pub const RIGHT_ALT: Self = Self(0xe6);
    pub const RIGHT_GUI: Self = Self(0xe7);
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum KeyAction {
    Down,
    Up,
}

const KEYBOARD_MONITOR_HISTORY_LIMIT: usize = 12;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct KeyboardTransition {
    pub sequence: u64,
    pub usage: KeyUsage,
    pub action: KeyAction,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct KeyboardMonitorState {
    pub sequence: u64,
    pub pressed: Vec<KeyUsage>,
    pub recent: Vec<KeyboardTransition>,
}

impl KeyboardMonitorState {
    pub fn observe(&mut self, usage: KeyUsage, action: KeyAction) {
        self.sequence = self.sequence.saturating_add(1);
        match action {
            KeyAction::Down if !self.pressed.contains(&usage) => self.pressed.push(usage),
            KeyAction::Up => self.pressed.retain(|pressed| *pressed != usage),
            KeyAction::Down => {}
        }

        self.recent.insert(
            0,
            KeyboardTransition {
                sequence: self.sequence,
                usage,
                action,
            },
        );
        self.recent.truncate(KEYBOARD_MONITOR_HISTORY_LIMIT);
    }

    pub fn clear_pressed(&mut self) {
        self.pressed.clear();
    }

    pub fn reset(&mut self) {
        self.pressed.clear();
        self.recent.clear();
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum MouseButton {
    Left,
    Right,
    Middle,
    Back,
    Forward,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum InputEvent {
    MouseMove {
        dx: i32,
        dy: i32,
    },
    MouseButton {
        button: MouseButton,
        action: KeyAction,
    },
    Wheel {
        delta_x: i16,
        delta_y: i16,
    },
    Key {
        usage: KeyUsage,
        action: KeyAction,
    },
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keyboard_monitor_tracks_pressed_keys_and_transitions() {
        let mut monitor = KeyboardMonitorState::default();

        monitor.observe(KeyUsage::LEFT_GUI, KeyAction::Down);
        monitor.observe(KeyUsage::UP_ARROW, KeyAction::Down);
        monitor.observe(KeyUsage::UP_ARROW, KeyAction::Up);

        assert_eq!(monitor.sequence, 3);
        assert_eq!(monitor.pressed, vec![KeyUsage::LEFT_GUI]);
        assert_eq!(monitor.recent[0].usage, KeyUsage::UP_ARROW);
        assert_eq!(monitor.recent[0].action, KeyAction::Up);
    }

    #[test]
    fn keyboard_monitor_keeps_pressed_keys_unique_during_repeat() {
        let mut monitor = KeyboardMonitorState::default();

        monitor.observe(KeyUsage::A, KeyAction::Down);
        monitor.observe(KeyUsage::A, KeyAction::Down);

        assert_eq!(monitor.pressed, vec![KeyUsage::A]);
        assert_eq!(monitor.recent.len(), 2);
    }

    #[test]
    fn keyboard_monitor_bounds_history_and_can_reset() {
        let mut monitor = KeyboardMonitorState::default();

        for _ in 0..(KEYBOARD_MONITOR_HISTORY_LIMIT + 3) {
            monitor.observe(KeyUsage::A, KeyAction::Down);
        }
        assert_eq!(monitor.recent.len(), KEYBOARD_MONITOR_HISTORY_LIMIT);

        monitor.reset();
        assert!(monitor.pressed.is_empty());
        assert!(monitor.recent.is_empty());
        assert_eq!(monitor.sequence, 15);
    }
}

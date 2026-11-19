use std::time::{Duration, Instant};

use objc2_application_services::AXIsProcessTrusted;
use objc2_core_foundation::CGPoint;
use objc2_core_graphics::{
    CGDisplayBounds, CGEvent, CGEventField, CGEventFlags, CGEventTapLocation, CGEventType,
    CGMouseButton, CGScrollEventUnit,
};

use crate::error::{DesktopError, Permission};
use crate::input::{InputBackend, InputEvent, PointerButton, WHEEL_STEP};
use crate::keymap;

const DOUBLE_CLICK_INTERVAL: Duration = Duration::from_millis(500);
const DOUBLE_CLICK_DISTANCE: f64 = 4.0;
const BACK_BUTTON: u32 = 3;
const FORWARD_BUTTON: u32 = 4;
const KEY_COMMAND: u16 = 0x37;
const KEY_SHIFT: u16 = 0x38;
const KEY_CAPS_LOCK: u16 = 0x39;
const KEY_OPTION: u16 = 0x3a;
const KEY_CONTROL: u16 = 0x3b;
const KEY_RIGHT_COMMAND: u16 = 0x36;
const KEY_RIGHT_SHIFT: u16 = 0x3c;
const KEY_RIGHT_OPTION: u16 = 0x3d;
const KEY_RIGHT_CONTROL: u16 = 0x3e;

fn input_failure(what: &str) -> DesktopError {
    DesktopError::Platform {
        code: 0,
        message: format!("could not create the {what} event"),
    }
}

fn modifier_mask(code: u16) -> Option<CGEventFlags> {
    match code {
        KEY_SHIFT | KEY_RIGHT_SHIFT => Some(CGEventFlags::MaskShift),
        KEY_CONTROL | KEY_RIGHT_CONTROL => Some(CGEventFlags::MaskControl),
        KEY_OPTION | KEY_RIGHT_OPTION => Some(CGEventFlags::MaskAlternate),
        KEY_COMMAND | KEY_RIGHT_COMMAND => Some(CGEventFlags::MaskCommand),
        _ => None,
    }
}

fn mouse_button_number(button: PointerButton) -> u32 {
    match button {
        PointerButton::Left => CGMouseButton::Left.0,
        PointerButton::Right => CGMouseButton::Right.0,
        PointerButton::Middle => CGMouseButton::Center.0,
        PointerButton::Back => BACK_BUTTON,
        PointerButton::Forward => FORWARD_BUTTON,
    }
}

fn button_event_type(button: PointerButton, pressed: bool) -> CGEventType {
    match (button, pressed) {
        (PointerButton::Left, true) => CGEventType::LeftMouseDown,
        (PointerButton::Left, false) => CGEventType::LeftMouseUp,
        (PointerButton::Right, true) => CGEventType::RightMouseDown,
        (PointerButton::Right, false) => CGEventType::RightMouseUp,
        (_, true) => CGEventType::OtherMouseDown,
        (_, false) => CGEventType::OtherMouseUp,
    }
}

struct LastClick {
    button: PointerButton,
    at: Instant,
    position: CGPoint,
    count: i64,
}

pub(crate) struct MacInput {
    origin: CGPoint,
    points_per_pixel_x: f64,
    points_per_pixel_y: f64,
    position: CGPoint,
    held: Vec<PointerButton>,
    held_modifiers: Vec<u16>,
    caps_lock: bool,
    last_click: Option<LastClick>,
    click_count: i64,
    scroll_remainder: (i32, i32),
}

impl MacInput {
    pub(crate) fn new(
        display_id: u32,
        pixel_width: u32,
        pixel_height: u32,
    ) -> Result<Self, DesktopError> {
        if !unsafe { AXIsProcessTrusted() } {
            return Err(DesktopError::PermissionDenied(Permission::InputControl));
        }
        let bounds = CGDisplayBounds(display_id);
        let origin = bounds.origin;
        Ok(Self {
            origin,
            points_per_pixel_x: bounds.size.width / f64::from(pixel_width.max(1)),
            points_per_pixel_y: bounds.size.height / f64::from(pixel_height.max(1)),
            position: origin,
            held: Vec::new(),
            held_modifiers: Vec::new(),
            caps_lock: false,
            last_click: None,
            click_count: 1,
            scroll_remainder: (0, 0),
        })
    }

    fn flags(&self) -> CGEventFlags {
        let mut flags = CGEventFlags::empty();
        for code in &self.held_modifiers {
            if let Some(mask) = modifier_mask(*code) {
                flags |= mask;
            }
        }
        if self.caps_lock {
            flags |= CGEventFlags::MaskAlphaShift;
        }
        flags
    }

    fn post(&self, event: &CGEvent) {
        CGEvent::set_flags(Some(event), self.flags());
        CGEvent::post(CGEventTapLocation::HIDEventTap, Some(event));
    }

    fn move_to(&mut self, x: u32, y: u32) -> Result<(), DesktopError> {
        self.position = CGPoint {
            x: self.origin.x + (f64::from(x) + 0.5) * self.points_per_pixel_x,
            y: self.origin.y + (f64::from(y) + 0.5) * self.points_per_pixel_y,
        };
        let (kind, button) = match self.held.first() {
            Some(PointerButton::Left) => (CGEventType::LeftMouseDragged, PointerButton::Left),
            Some(PointerButton::Right) => (CGEventType::RightMouseDragged, PointerButton::Right),
            Some(other) => (CGEventType::OtherMouseDragged, *other),
            None => (CGEventType::MouseMoved, PointerButton::Left),
        };
        let event = CGEvent::new_mouse_event(
            None,
            kind,
            self.position,
            CGMouseButton(mouse_button_number(button)),
        )
        .ok_or_else(|| input_failure("mouse move"))?;
        self.post(&event);
        Ok(())
    }

    fn click(&mut self, button: PointerButton, pressed: bool) -> Result<(), DesktopError> {
        if pressed {
            let now = Instant::now();
            self.click_count = match &self.last_click {
                Some(last)
                    if last.button == button
                        && now.duration_since(last.at) <= DOUBLE_CLICK_INTERVAL
                        && (last.position.x - self.position.x).abs() <= DOUBLE_CLICK_DISTANCE
                        && (last.position.y - self.position.y).abs() <= DOUBLE_CLICK_DISTANCE =>
                {
                    last.count + 1
                }
                _ => 1,
            };
            self.last_click = Some(LastClick {
                button,
                at: now,
                position: self.position,
                count: self.click_count,
            });
            if !self.held.contains(&button) {
                self.held.push(button);
            }
        } else {
            self.held.retain(|held| *held != button);
        }
        let event = CGEvent::new_mouse_event(
            None,
            button_event_type(button, pressed),
            self.position,
            CGMouseButton(mouse_button_number(button)),
        )
        .ok_or_else(|| input_failure("mouse button"))?;
        CGEvent::set_integer_value_field(
            Some(&event),
            CGEventField::MouseEventClickState,
            self.click_count,
        );
        self.post(&event);
        Ok(())
    }

    fn scroll(&mut self, horizontal: i32, vertical: i32) -> Result<(), DesktopError> {
        let total_vertical = self.scroll_remainder.1 + vertical;
        let total_horizontal = self.scroll_remainder.0 + horizontal;
        let lines_vertical = total_vertical / WHEEL_STEP;
        let lines_horizontal = total_horizontal / WHEEL_STEP;
        self.scroll_remainder = (total_horizontal % WHEEL_STEP, total_vertical % WHEEL_STEP);
        if lines_vertical == 0 && lines_horizontal == 0 {
            return Ok(());
        }
        let event = CGEvent::new_scroll_wheel_event2(
            None,
            CGScrollEventUnit::Line,
            2,
            lines_vertical,
            -lines_horizontal,
            0,
        )
        .ok_or_else(|| input_failure("scroll"))?;
        self.post(&event);
        Ok(())
    }

    fn key(&mut self, usage: u16, pressed: bool) -> Result<(), DesktopError> {
        let code = keymap::by_usage(usage)
            .and_then(|entry| entry.mac)
            .ok_or(DesktopError::UnmappedKey(usage))?;
        if modifier_mask(code).is_some() {
            if pressed {
                if !self.held_modifiers.contains(&code) {
                    self.held_modifiers.push(code);
                }
            } else {
                self.held_modifiers.retain(|held| *held != code);
            }
        }
        if code == KEY_CAPS_LOCK && pressed {
            self.caps_lock = !self.caps_lock;
        }
        let event = CGEvent::new_keyboard_event(None, code, pressed)
            .ok_or_else(|| input_failure("keyboard"))?;
        if modifier_mask(code).is_some() || code == KEY_CAPS_LOCK {
            CGEvent::set_type(Some(&event), CGEventType::FlagsChanged);
        }
        self.post(&event);
        Ok(())
    }
}

impl InputBackend for MacInput {
    fn apply(&mut self, event: InputEvent) -> Result<(), DesktopError> {
        match event {
            InputEvent::PointerMove { x, y } => self.move_to(x, y),
            InputEvent::PointerButton { button, pressed } => self.click(button, pressed),
            InputEvent::Scroll {
                horizontal,
                vertical,
            } => self.scroll(horizontal, vertical),
            InputEvent::Key { usage, pressed } => self.key(usage, pressed),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use objc2_core_graphics::{
        CGDisplayCopyDisplayMode, CGDisplayMode, CGEventSource, CGEventSourceStateID,
        CGMainDisplayID,
    };

    fn pointer_location() -> CGPoint {
        CGEvent::location(CGEvent::new(None).as_deref())
    }

    #[test]
    #[ignore = "posts real but invisible events on this Mac"]
    fn moving_in_place_and_a_lone_right_shift_are_observed() {
        let display = CGMainDisplayID();
        let mode = CGDisplayCopyDisplayMode(display);
        let width = CGDisplayMode::pixel_width(mode.as_deref()) as u32;
        let height = CGDisplayMode::pixel_height(mode.as_deref()) as u32;
        let mut input = MacInput::new(display, width, height).unwrap();
        let bounds = CGDisplayBounds(display);
        let here = pointer_location();
        let inside = here.x >= bounds.origin.x
            && here.y >= bounds.origin.y
            && here.x < bounds.origin.x + bounds.size.width
            && here.y < bounds.origin.y + bounds.size.height;
        if inside {
            let x = ((here.x - bounds.origin.x) / input.points_per_pixel_x).floor() as u32;
            let y = ((here.y - bounds.origin.y) / input.points_per_pixel_y).floor() as u32;
            input.apply(InputEvent::PointerMove { x, y }).unwrap();
            std::thread::sleep(Duration::from_millis(50));
            let after = pointer_location();
            assert!(
                (after.x - here.x).abs() <= input.points_per_pixel_x,
                "{here:?} -> {after:?}"
            );
            assert!(
                (after.y - here.y).abs() <= input.points_per_pixel_y,
                "{here:?} -> {after:?}"
            );
        }
        let right_shift = keymap::KEYS
            .iter()
            .find(|entry| entry.code == "ShiftRight")
            .unwrap();
        let is_down = || {
            CGEventSource::flags_state(CGEventSourceStateID::CombinedSessionState)
                .contains(CGEventFlags::MaskShift)
        };
        input
            .apply(InputEvent::Key {
                usage: right_shift.usage,
                pressed: true,
            })
            .unwrap();
        std::thread::sleep(Duration::from_millis(50));
        let pressed = is_down();
        input
            .apply(InputEvent::Key {
                usage: right_shift.usage,
                pressed: false,
            })
            .unwrap();
        std::thread::sleep(Duration::from_millis(50));
        let released = !is_down();
        assert!(
            pressed && released,
            "pressed {pressed}, released {released}"
        );
    }
}

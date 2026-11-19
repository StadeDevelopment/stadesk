use std::fmt::Display;

use x11rb::connection::{Connection, RequestConnection};
use x11rb::protocol::xproto::{
    self, BUTTON_PRESS_EVENT, BUTTON_RELEASE_EVENT, ConnectionExt as _, KEY_PRESS_EVENT,
    KEY_RELEASE_EVENT, MOTION_NOTIFY_EVENT,
};
use x11rb::protocol::xtest;
use x11rb::rust_connection::RustConnection;

use crate::error::DesktopError;
use crate::input::{InputBackend, InputEvent, PointerButton, WHEEL_STEP};
use crate::keymap;

const XKB_EVDEV_OFFSET: u16 = 8;
const CURRENT_TIME: u32 = 0;
const NO_DEVICE: u8 = 0;
const BUTTON_LEFT: u8 = 1;
const BUTTON_MIDDLE: u8 = 2;
const BUTTON_RIGHT: u8 = 3;
const WHEEL_UP: u8 = 4;
const WHEEL_DOWN: u8 = 5;
const WHEEL_LEFT: u8 = 6;
const WHEEL_RIGHT: u8 = 7;
const BUTTON_BACK: u8 = 8;
const BUTTON_FORWARD: u8 = 9;

fn x_error(error: impl Display) -> DesktopError {
    DesktopError::Platform {
        code: 0,
        message: format!("X11 input: {error}"),
    }
}

pub(crate) fn button_number(button: PointerButton) -> u8 {
    match button {
        PointerButton::Left => BUTTON_LEFT,
        PointerButton::Middle => BUTTON_MIDDLE,
        PointerButton::Right => BUTTON_RIGHT,
        PointerButton::Back => BUTTON_BACK,
        PointerButton::Forward => BUTTON_FORWARD,
    }
}

pub(crate) struct X11Input {
    connection: RustConnection,
    root: xproto::Window,
    origin_x: i32,
    origin_y: i32,
    scroll_remainder: (i32, i32),
}

impl X11Input {
    pub(crate) fn new(origin_x: i16, origin_y: i16) -> Result<Self, DesktopError> {
        let (connection, screen_index) = RustConnection::connect(None).map_err(x_error)?;
        let root = connection
            .setup()
            .roots
            .get(screen_index)
            .map(|screen| screen.root)
            .ok_or_else(|| x_error("the default screen does not exist"))?;
        if connection
            .extension_information(xtest::X11_EXTENSION_NAME)
            .map_err(x_error)?
            .is_none()
        {
            return Err(DesktopError::Unsupported(
                "the X server does not offer the XTEST extension",
            ));
        }
        xtest::get_version(&connection, 2, 2)
            .map_err(x_error)?
            .reply()
            .map_err(x_error)?;
        Ok(Self {
            connection,
            root,
            origin_x: i32::from(origin_x),
            origin_y: i32::from(origin_y),
            scroll_remainder: (0, 0),
        })
    }

    fn fake(&self, kind: u8, detail: u8, x: i16, y: i16) -> Result<(), DesktopError> {
        xtest::fake_input(
            &self.connection,
            kind,
            detail,
            CURRENT_TIME,
            self.root,
            x,
            y,
            NO_DEVICE,
        )
        .map_err(x_error)?;
        Ok(())
    }

    fn click(&self, button: u8, times: i32) -> Result<(), DesktopError> {
        for _ in 0..times {
            self.fake(BUTTON_PRESS_EVENT, button, 0, 0)?;
            self.fake(BUTTON_RELEASE_EVENT, button, 0, 0)?;
        }
        Ok(())
    }

    fn scroll(&mut self, horizontal: i32, vertical: i32) -> Result<(), DesktopError> {
        let total_horizontal = self.scroll_remainder.0 + horizontal;
        let total_vertical = self.scroll_remainder.1 + vertical;
        let steps_horizontal = total_horizontal / WHEEL_STEP;
        let steps_vertical = total_vertical / WHEEL_STEP;
        self.scroll_remainder = (total_horizontal % WHEEL_STEP, total_vertical % WHEEL_STEP);
        if steps_vertical > 0 {
            self.click(WHEEL_UP, steps_vertical)?;
        } else {
            self.click(WHEEL_DOWN, -steps_vertical)?;
        }
        if steps_horizontal > 0 {
            self.click(WHEEL_RIGHT, steps_horizontal)?;
        } else {
            self.click(WHEEL_LEFT, -steps_horizontal)?;
        }
        Ok(())
    }

    fn dispatch(&mut self, event: InputEvent) -> Result<(), DesktopError> {
        match event {
            InputEvent::PointerMove { x, y } => {
                let root_x =
                    (self.origin_x + x as i32).clamp(i32::from(i16::MIN), i32::from(i16::MAX));
                let root_y =
                    (self.origin_y + y as i32).clamp(i32::from(i16::MIN), i32::from(i16::MAX));
                self.fake(MOTION_NOTIFY_EVENT, 0, root_x as i16, root_y as i16)
            }
            InputEvent::PointerButton { button, pressed } => {
                let kind = if pressed {
                    BUTTON_PRESS_EVENT
                } else {
                    BUTTON_RELEASE_EVENT
                };
                self.fake(kind, button_number(button), 0, 0)
            }
            InputEvent::Scroll {
                horizontal,
                vertical,
            } => self.scroll(horizontal, vertical),
            InputEvent::Key { usage, pressed } => {
                let entry = keymap::by_usage(usage).ok_or(DesktopError::UnmappedKey(usage))?;
                let keycode = u8::try_from(entry.evdev + XKB_EVDEV_OFFSET)
                    .map_err(|_| DesktopError::UnmappedKey(usage))?;
                let kind = if pressed {
                    KEY_PRESS_EVENT
                } else {
                    KEY_RELEASE_EVENT
                };
                self.fake(kind, keycode, 0, 0)
            }
        }
    }
}

impl InputBackend for X11Input {
    fn apply(&mut self, event: InputEvent) -> Result<(), DesktopError> {
        self.dispatch(event)?;
        self.connection
            .get_input_focus()
            .map_err(x_error)?
            .reply()
            .map_err(x_error)?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const EXPECTED_KEYSYMS: &[(&str, u32)] = &[
        ("Enter", 0xff0d),
        ("Escape", 0xff1b),
        ("Backspace", 0xff08),
        ("Tab", 0xff09),
        ("Space", 0x0020),
        ("CapsLock", 0xffe5),
        ("PrintScreen", 0xff61),
        ("ScrollLock", 0xff14),
        ("Pause", 0xff13),
        ("Insert", 0xff63),
        ("Home", 0xff50),
        ("PageUp", 0xff55),
        ("Delete", 0xffff),
        ("End", 0xff57),
        ("PageDown", 0xff56),
        ("ArrowRight", 0xff53),
        ("ArrowLeft", 0xff51),
        ("ArrowDown", 0xff54),
        ("ArrowUp", 0xff52),
        ("NumLock", 0xff7f),
        ("NumpadDivide", 0xffaf),
        ("NumpadMultiply", 0xffaa),
        ("NumpadSubtract", 0xffad),
        ("NumpadAdd", 0xffab),
        ("NumpadEnter", 0xff8d),
        ("ContextMenu", 0xff67),
        ("ControlLeft", 0xffe3),
        ("ShiftLeft", 0xffe1),
        ("AltLeft", 0xffe9),
        ("MetaLeft", 0xffeb),
        ("ControlRight", 0xffe4),
        ("ShiftRight", 0xffe2),
        ("MetaRight", 0xffec),
        ("Minus", 0x002d),
        ("Equal", 0x003d),
        ("BracketLeft", 0x005b),
        ("BracketRight", 0x005d),
        ("Backslash", 0x005c),
        ("Semicolon", 0x003b),
        ("Quote", 0x0027),
        ("Backquote", 0x0060),
        ("Comma", 0x002c),
        ("Period", 0x002e),
        ("Slash", 0x002f),
        ("IntlBackslash", 0x003c),
    ];

    fn expected_keysym(code: &str) -> Option<u32> {
        if let Some(letter) = code.strip_prefix("Key") {
            return Some(u32::from(letter.as_bytes()[0].to_ascii_lowercase()));
        }
        if let Some(digit) = code.strip_prefix("Digit") {
            return Some(u32::from(digit.as_bytes()[0]));
        }
        if let Some(number) = code
            .strip_prefix('F')
            .and_then(|rest| rest.parse::<u32>().ok())
            .filter(|number| *number <= 12)
        {
            return Some(0xffbe + number - 1);
        }
        EXPECTED_KEYSYMS
            .iter()
            .find(|(name, _)| *name == code)
            .map(|(_, keysym)| *keysym)
    }

    fn connect() -> (RustConnection, xproto::Window) {
        let (connection, screen) = RustConnection::connect(None).expect("an X server is required");
        let root = connection.setup().roots[screen].root;
        (connection, root)
    }

    #[test]
    #[ignore = "needs an X server, run with --ignored under Xvfb"]
    fn evdev_codes_produce_the_expected_keysyms() {
        let (connection, _) = connect();
        let setup = connection.setup();
        let (min, max) = (setup.min_keycode, setup.max_keycode);
        let mapping = connection
            .get_keyboard_mapping(min, max - min + 1)
            .unwrap()
            .reply()
            .unwrap();
        let per = usize::from(mapping.keysyms_per_keycode);
        let mut checked = 0;
        for entry in keymap::KEYS {
            let Some(expected) = expected_keysym(entry.code) else {
                continue;
            };
            let keycode = entry.evdev + XKB_EVDEV_OFFSET;
            let index = usize::from(keycode - u16::from(min)) * per;
            let actual = mapping.keysyms[index];
            assert_eq!(actual, expected, "{} (keycode {keycode})", entry.code);
            checked += 1;
        }
        assert_eq!(checked, 93, "every expected keysym must be checked");
    }

    #[test]
    #[ignore = "needs an X server, run with --ignored under Xvfb"]
    fn injected_events_reach_the_x_server() {
        let (connection, root) = connect();
        let mut input = X11Input::new(0, 0).unwrap();
        for (x, y) in [(10u32, 20u32), (300, 150), (1000, 700)] {
            input.apply(InputEvent::PointerMove { x, y }).unwrap();
            let pointer = connection.query_pointer(root).unwrap().reply().unwrap();
            assert_eq!((pointer.root_x, pointer.root_y), (x as i16, y as i16));
        }
        input
            .apply(InputEvent::PointerButton {
                button: PointerButton::Left,
                pressed: true,
            })
            .unwrap();
        let pressed = connection.query_pointer(root).unwrap().reply().unwrap();
        assert!(u16::from(pressed.mask) & u16::from(xproto::KeyButMask::BUTTON1) != 0);
        input
            .apply(InputEvent::PointerButton {
                button: PointerButton::Left,
                pressed: false,
            })
            .unwrap();
        let released = connection.query_pointer(root).unwrap().reply().unwrap();
        assert!(u16::from(released.mask) & u16::from(xproto::KeyButMask::BUTTON1) == 0);
        for code in ["KeyA", "ShiftLeft", "ArrowUp", "NumpadEnter", "F12"] {
            let entry = keymap::KEYS
                .iter()
                .find(|entry| entry.code == code)
                .unwrap();
            let keycode = usize::from(entry.evdev + XKB_EVDEV_OFFSET);
            let is_down = |connection: &RustConnection| {
                let keys = connection.query_keymap().unwrap().reply().unwrap().keys;
                keys[keycode / 8] & (1 << (keycode % 8)) != 0
            };
            input
                .apply(InputEvent::Key {
                    usage: entry.usage,
                    pressed: true,
                })
                .unwrap();
            assert!(is_down(&connection), "{code} press");
            input
                .apply(InputEvent::Key {
                    usage: entry.usage,
                    pressed: false,
                })
                .unwrap();
            assert!(!is_down(&connection), "{code} release");
        }
    }
}

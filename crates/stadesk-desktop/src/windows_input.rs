use windows::Win32::Foundation::RECT;
use windows::Win32::UI::Input::KeyboardAndMouse::{
    INPUT, INPUT_0, INPUT_KEYBOARD, INPUT_MOUSE, KEYBD_EVENT_FLAGS, KEYBDINPUT,
    KEYEVENTF_EXTENDEDKEY, KEYEVENTF_KEYUP, KEYEVENTF_SCANCODE, MOUSE_EVENT_FLAGS,
    MOUSEEVENTF_ABSOLUTE, MOUSEEVENTF_HWHEEL, MOUSEEVENTF_LEFTDOWN, MOUSEEVENTF_LEFTUP,
    MOUSEEVENTF_MIDDLEDOWN, MOUSEEVENTF_MIDDLEUP, MOUSEEVENTF_MOVE, MOUSEEVENTF_RIGHTDOWN,
    MOUSEEVENTF_RIGHTUP, MOUSEEVENTF_VIRTUALDESK, MOUSEEVENTF_WHEEL, MOUSEEVENTF_XDOWN,
    MOUSEEVENTF_XUP, MOUSEINPUT, SendInput, VIRTUAL_KEY,
};
use windows::Win32::UI::WindowsAndMessaging::{
    GetSystemMetrics, SM_CXVIRTUALSCREEN, SM_CYVIRTUALSCREEN, SM_XVIRTUALSCREEN, SM_YVIRTUALSCREEN,
    XBUTTON1, XBUTTON2,
};

use crate::error::DesktopError;
use crate::input::{InputBackend, InputEvent, PointerButton};
use crate::keymap::{self, WINDOWS_EXTENDED};
use crate::windows::DpiAwareness;

const NORMALIZED_RANGE: f64 = 65_536.0;
const VK_PAUSE: u16 = 0x13;
const VK_NUMLOCK: u16 = 0x90;
const VK_SNAPSHOT: u16 = 0x2c;
const SCAN_PAUSE: u16 = 0x45;
const SCAN_NUMLOCK: u16 = 0xe045;
const SCAN_PRINT_SCREEN: u16 = 0xe037;

pub(crate) struct WindowsInput {
    desktop: RECT,
}

fn send(inputs: &[INPUT]) -> Result<(), DesktopError> {
    let sent = unsafe { SendInput(inputs, std::mem::size_of::<INPUT>() as i32) };
    if sent as usize == inputs.len() {
        Ok(())
    } else {
        Err(DesktopError::Platform {
            code: i64::from(windows::core::Error::from_thread().code().0),
            message: "SendInput was blocked (secure desktop or a higher-privilege window)"
                .to_owned(),
        })
    }
}

fn mouse(dx: i32, dy: i32, data: u32, flags: MOUSE_EVENT_FLAGS) -> INPUT {
    INPUT {
        r#type: INPUT_MOUSE,
        Anonymous: INPUT_0 {
            mi: MOUSEINPUT {
                dx,
                dy,
                mouseData: data,
                dwFlags: flags,
                time: 0,
                dwExtraInfo: 0,
            },
        },
    }
}

fn keyboard(virtual_key: u16, scan: u16, flags: KEYBD_EVENT_FLAGS) -> INPUT {
    INPUT {
        r#type: INPUT_KEYBOARD,
        Anonymous: INPUT_0 {
            ki: KEYBDINPUT {
                wVk: VIRTUAL_KEY(virtual_key),
                wScan: scan,
                dwFlags: flags,
                time: 0,
                dwExtraInfo: 0,
            },
        },
    }
}

pub(crate) fn key_input(scan_code: u16, pressed: bool) -> INPUT {
    let release = if pressed {
        KEYBD_EVENT_FLAGS(0)
    } else {
        KEYEVENTF_KEYUP
    };
    match scan_code {
        SCAN_PAUSE => keyboard(VK_PAUSE, 0, release),
        SCAN_NUMLOCK => keyboard(VK_NUMLOCK, 0, KEYEVENTF_EXTENDEDKEY | release),
        SCAN_PRINT_SCREEN => keyboard(VK_SNAPSHOT, 0, KEYEVENTF_EXTENDEDKEY | release),
        scan => {
            let extended = if scan & WINDOWS_EXTENDED == WINDOWS_EXTENDED {
                KEYEVENTF_EXTENDEDKEY
            } else {
                KEYBD_EVENT_FLAGS(0)
            };
            keyboard(0, scan & 0x00ff, KEYEVENTF_SCANCODE | extended | release)
        }
    }
}

pub(crate) fn normalized(position: i32, origin: i32, extent: i32) -> i32 {
    let extent = f64::from(extent.max(1));
    ((f64::from(position - origin) + 0.5) * NORMALIZED_RANGE / extent).floor() as i32
}

impl WindowsInput {
    pub(crate) fn new(desktop: RECT) -> Self {
        Self { desktop }
    }

    fn move_to(&self, x: u32, y: u32) -> Result<(), DesktopError> {
        let _dpi = DpiAwareness::per_monitor();
        let (left, top, width, height) = unsafe {
            (
                GetSystemMetrics(SM_XVIRTUALSCREEN),
                GetSystemMetrics(SM_YVIRTUALSCREEN),
                GetSystemMetrics(SM_CXVIRTUALSCREEN),
                GetSystemMetrics(SM_CYVIRTUALSCREEN),
            )
        };
        let desktop_x = self.desktop.left + x as i32;
        let desktop_y = self.desktop.top + y as i32;
        send(&[mouse(
            normalized(desktop_x, left, width),
            normalized(desktop_y, top, height),
            0,
            MOUSEEVENTF_MOVE | MOUSEEVENTF_ABSOLUTE | MOUSEEVENTF_VIRTUALDESK,
        )])
    }

    fn button(&self, button: PointerButton, pressed: bool) -> Result<(), DesktopError> {
        let (flags, data) = match (button, pressed) {
            (PointerButton::Left, true) => (MOUSEEVENTF_LEFTDOWN, 0),
            (PointerButton::Left, false) => (MOUSEEVENTF_LEFTUP, 0),
            (PointerButton::Right, true) => (MOUSEEVENTF_RIGHTDOWN, 0),
            (PointerButton::Right, false) => (MOUSEEVENTF_RIGHTUP, 0),
            (PointerButton::Middle, true) => (MOUSEEVENTF_MIDDLEDOWN, 0),
            (PointerButton::Middle, false) => (MOUSEEVENTF_MIDDLEUP, 0),
            (PointerButton::Back, true) => (MOUSEEVENTF_XDOWN, u32::from(XBUTTON1)),
            (PointerButton::Back, false) => (MOUSEEVENTF_XUP, u32::from(XBUTTON1)),
            (PointerButton::Forward, true) => (MOUSEEVENTF_XDOWN, u32::from(XBUTTON2)),
            (PointerButton::Forward, false) => (MOUSEEVENTF_XUP, u32::from(XBUTTON2)),
        };
        send(&[mouse(0, 0, data, flags)])
    }

    fn scroll(&self, horizontal: i32, vertical: i32) -> Result<(), DesktopError> {
        let mut inputs = Vec::with_capacity(2);
        if vertical != 0 {
            inputs.push(mouse(0, 0, vertical as u32, MOUSEEVENTF_WHEEL));
        }
        if horizontal != 0 {
            inputs.push(mouse(0, 0, horizontal as u32, MOUSEEVENTF_HWHEEL));
        }
        send(&inputs)
    }

    fn key(&self, usage: u16, pressed: bool) -> Result<(), DesktopError> {
        let entry = keymap::by_usage(usage).ok_or(DesktopError::UnmappedKey(usage))?;
        send(&[key_input(entry.windows_scan, pressed)])
    }
}

impl InputBackend for WindowsInput {
    fn apply(&mut self, event: InputEvent) -> Result<(), DesktopError> {
        match event {
            InputEvent::PointerMove { x, y } => self.move_to(x, y),
            InputEvent::PointerButton { button, pressed } => self.button(button, pressed),
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
    use windows::Win32::Foundation::POINT;
    use windows::Win32::UI::Input::KeyboardAndMouse::{
        GetAsyncKeyState, MAPVK_VSC_TO_VK_EX, MapVirtualKeyW,
    };
    use windows::Win32::UI::WindowsAndMessaging::GetCursorPos;

    fn expected_virtual_key(code: &str) -> Option<u32> {
        if let Some(letter) = code.strip_prefix("Key") {
            return Some(u32::from(letter.as_bytes()[0]));
        }
        if let Some(digit) = code.strip_prefix("Digit") {
            return Some(u32::from(digit.as_bytes()[0]));
        }
        if let Some(number) = code
            .strip_prefix('F')
            .and_then(|rest| rest.parse::<u32>().ok())
        {
            return Some(0x70 + number - 1);
        }
        let fixed = [
            ("Enter", 0x0d),
            ("Escape", 0x1b),
            ("Backspace", 0x08),
            ("Tab", 0x09),
            ("Space", 0x20),
            ("CapsLock", 0x14),
            ("ScrollLock", 0x91),
            ("Insert", 0x2d),
            ("Delete", 0x2e),
            ("Home", 0x24),
            ("End", 0x23),
            ("PageUp", 0x21),
            ("PageDown", 0x22),
            ("ArrowLeft", 0x25),
            ("ArrowUp", 0x26),
            ("ArrowRight", 0x27),
            ("ArrowDown", 0x28),
            ("ShiftLeft", 0xa0),
            ("ShiftRight", 0xa1),
            ("ControlLeft", 0xa2),
            ("ControlRight", 0xa3),
            ("AltLeft", 0xa4),
            ("AltRight", 0xa5),
            ("MetaLeft", 0x5b),
            ("MetaRight", 0x5c),
            ("ContextMenu", 0x5d),
            ("NumpadMultiply", 0x6a),
            ("NumpadSubtract", 0x6d),
            ("NumpadAdd", 0x6b),
            ("NumpadDivide", 0x6f),
            ("NumpadEnter", 0x0d),
        ];
        fixed
            .iter()
            .find(|(name, _)| *name == code)
            .map(|(_, key)| *key)
    }

    #[test]
    fn normalized_coordinates_hit_pixel_centres() {
        assert_eq!(normalized(0, 0, 1920), 17);
        assert_eq!(normalized(1919, 0, 1920), 65_518);
        assert_eq!(normalized(-1920, -1920, 3840), 8);
        let extent = 1470;
        for pixel in [0, 1, 734, 1469] {
            let back = normalized(pixel, 0, extent) as i64 * i64::from(extent) / 65_536;
            assert_eq!(back, i64::from(pixel));
        }
    }

    #[test]
    fn scan_codes_map_to_the_expected_virtual_keys() {
        let mut checked = 0;
        for entry in keymap::KEYS {
            let Some(expected) = expected_virtual_key(entry.code) else {
                continue;
            };
            if [SCAN_PAUSE, SCAN_NUMLOCK, SCAN_PRINT_SCREEN].contains(&entry.windows_scan) {
                continue;
            }
            let actual =
                unsafe { MapVirtualKeyW(u32::from(entry.windows_scan), MAPVK_VSC_TO_VK_EX) };
            assert_eq!(
                actual, expected,
                "{} (scan {:#06x})",
                entry.code, entry.windows_scan
            );
            checked += 1;
        }
        assert!(checked >= 80, "only {checked} keys were checked");
    }

    #[test]
    #[ignore = "moves the real pointer, run with --ignored inside an interactive session"]
    fn injected_pointer_and_key_reach_windows() {
        let _dpi = DpiAwareness::per_monitor();
        let desktop = RECT {
            left: 0,
            top: 0,
            right: 0,
            bottom: 0,
        };
        let mut input = WindowsInput::new(desktop);
        for (x, y) in [(100u32, 100u32), (700, 400), (1200, 600)] {
            input.apply(InputEvent::PointerMove { x, y }).unwrap();
            std::thread::sleep(std::time::Duration::from_millis(50));
            let mut point = POINT::default();
            unsafe { GetCursorPos(&mut point) }.unwrap();
            assert_eq!((point.x, point.y), (x as i32, y as i32));
        }
        let shift = keymap::KEYS
            .iter()
            .find(|entry| entry.code == "ShiftLeft")
            .unwrap();
        input
            .apply(InputEvent::Key {
                usage: shift.usage,
                pressed: true,
            })
            .unwrap();
        std::thread::sleep(std::time::Duration::from_millis(50));
        let down = unsafe { GetAsyncKeyState(0xa0) } as u16 & 0x8000 != 0;
        input
            .apply(InputEvent::Key {
                usage: shift.usage,
                pressed: false,
            })
            .unwrap();
        std::thread::sleep(std::time::Duration::from_millis(50));
        let up = unsafe { GetAsyncKeyState(0xa0) } as u16 & 0x8000 == 0;
        assert!(down && up, "shift down {down}, released {up}");
    }
}

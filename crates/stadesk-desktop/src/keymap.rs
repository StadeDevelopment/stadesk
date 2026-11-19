#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct KeyEntry {
    pub(crate) usage: u16,
    pub(crate) code: &'static str,
    pub(crate) evdev: u16,
    pub(crate) windows_scan: u16,
    pub(crate) mac: Option<u16>,
}

const fn key(
    usage: u16,
    code: &'static str,
    evdev: u16,
    windows_scan: u16,
    mac: Option<u16>,
) -> KeyEntry {
    KeyEntry {
        usage,
        code,
        evdev,
        windows_scan,
        mac,
    }
}

#[cfg(any(windows, test))]
pub(crate) const WINDOWS_EXTENDED: u16 = 0xe000;

pub(crate) const KEYS: &[KeyEntry] = &[
    key(0x04, "KeyA", 30, 0x1e, Some(0x00)),
    key(0x05, "KeyB", 48, 0x30, Some(0x0b)),
    key(0x06, "KeyC", 46, 0x2e, Some(0x08)),
    key(0x07, "KeyD", 32, 0x20, Some(0x02)),
    key(0x08, "KeyE", 18, 0x12, Some(0x0e)),
    key(0x09, "KeyF", 33, 0x21, Some(0x03)),
    key(0x0a, "KeyG", 34, 0x22, Some(0x05)),
    key(0x0b, "KeyH", 35, 0x23, Some(0x04)),
    key(0x0c, "KeyI", 23, 0x17, Some(0x22)),
    key(0x0d, "KeyJ", 36, 0x24, Some(0x26)),
    key(0x0e, "KeyK", 37, 0x25, Some(0x28)),
    key(0x0f, "KeyL", 38, 0x26, Some(0x25)),
    key(0x10, "KeyM", 50, 0x32, Some(0x2e)),
    key(0x11, "KeyN", 49, 0x31, Some(0x2d)),
    key(0x12, "KeyO", 24, 0x18, Some(0x1f)),
    key(0x13, "KeyP", 25, 0x19, Some(0x23)),
    key(0x14, "KeyQ", 16, 0x10, Some(0x0c)),
    key(0x15, "KeyR", 19, 0x13, Some(0x0f)),
    key(0x16, "KeyS", 31, 0x1f, Some(0x01)),
    key(0x17, "KeyT", 20, 0x14, Some(0x11)),
    key(0x18, "KeyU", 22, 0x16, Some(0x20)),
    key(0x19, "KeyV", 47, 0x2f, Some(0x09)),
    key(0x1a, "KeyW", 17, 0x11, Some(0x0d)),
    key(0x1b, "KeyX", 45, 0x2d, Some(0x07)),
    key(0x1c, "KeyY", 21, 0x15, Some(0x10)),
    key(0x1d, "KeyZ", 44, 0x2c, Some(0x06)),
    key(0x1e, "Digit1", 2, 0x02, Some(0x12)),
    key(0x1f, "Digit2", 3, 0x03, Some(0x13)),
    key(0x20, "Digit3", 4, 0x04, Some(0x14)),
    key(0x21, "Digit4", 5, 0x05, Some(0x15)),
    key(0x22, "Digit5", 6, 0x06, Some(0x17)),
    key(0x23, "Digit6", 7, 0x07, Some(0x16)),
    key(0x24, "Digit7", 8, 0x08, Some(0x1a)),
    key(0x25, "Digit8", 9, 0x09, Some(0x1c)),
    key(0x26, "Digit9", 10, 0x0a, Some(0x19)),
    key(0x27, "Digit0", 11, 0x0b, Some(0x1d)),
    key(0x28, "Enter", 28, 0x1c, Some(0x24)),
    key(0x29, "Escape", 1, 0x01, Some(0x35)),
    key(0x2a, "Backspace", 14, 0x0e, Some(0x33)),
    key(0x2b, "Tab", 15, 0x0f, Some(0x30)),
    key(0x2c, "Space", 57, 0x39, Some(0x31)),
    key(0x2d, "Minus", 12, 0x0c, Some(0x1b)),
    key(0x2e, "Equal", 13, 0x0d, Some(0x18)),
    key(0x2f, "BracketLeft", 26, 0x1a, Some(0x21)),
    key(0x30, "BracketRight", 27, 0x1b, Some(0x1e)),
    key(0x31, "Backslash", 43, 0x2b, Some(0x2a)),
    key(0x33, "Semicolon", 39, 0x27, Some(0x29)),
    key(0x34, "Quote", 40, 0x28, Some(0x27)),
    key(0x35, "Backquote", 41, 0x29, Some(0x32)),
    key(0x36, "Comma", 51, 0x33, Some(0x2b)),
    key(0x37, "Period", 52, 0x34, Some(0x2f)),
    key(0x38, "Slash", 53, 0x35, Some(0x2c)),
    key(0x39, "CapsLock", 58, 0x3a, Some(0x39)),
    key(0x3a, "F1", 59, 0x3b, Some(0x7a)),
    key(0x3b, "F2", 60, 0x3c, Some(0x78)),
    key(0x3c, "F3", 61, 0x3d, Some(0x63)),
    key(0x3d, "F4", 62, 0x3e, Some(0x76)),
    key(0x3e, "F5", 63, 0x3f, Some(0x60)),
    key(0x3f, "F6", 64, 0x40, Some(0x61)),
    key(0x40, "F7", 65, 0x41, Some(0x62)),
    key(0x41, "F8", 66, 0x42, Some(0x64)),
    key(0x42, "F9", 67, 0x43, Some(0x65)),
    key(0x43, "F10", 68, 0x44, Some(0x6d)),
    key(0x44, "F11", 87, 0x57, Some(0x67)),
    key(0x45, "F12", 88, 0x58, Some(0x6f)),
    key(0x46, "PrintScreen", 99, 0xe037, None),
    key(0x47, "ScrollLock", 70, 0x46, None),
    key(0x48, "Pause", 119, 0x45, None),
    key(0x49, "Insert", 110, 0xe052, Some(0x72)),
    key(0x4a, "Home", 102, 0xe047, Some(0x73)),
    key(0x4b, "PageUp", 104, 0xe049, Some(0x74)),
    key(0x4c, "Delete", 111, 0xe053, Some(0x75)),
    key(0x4d, "End", 107, 0xe04f, Some(0x77)),
    key(0x4e, "PageDown", 109, 0xe051, Some(0x79)),
    key(0x4f, "ArrowRight", 106, 0xe04d, Some(0x7c)),
    key(0x50, "ArrowLeft", 105, 0xe04b, Some(0x7b)),
    key(0x51, "ArrowDown", 108, 0xe050, Some(0x7d)),
    key(0x52, "ArrowUp", 103, 0xe048, Some(0x7e)),
    key(0x53, "NumLock", 69, 0xe045, Some(0x47)),
    key(0x54, "NumpadDivide", 98, 0xe035, Some(0x4b)),
    key(0x55, "NumpadMultiply", 55, 0x37, Some(0x43)),
    key(0x56, "NumpadSubtract", 74, 0x4a, Some(0x4e)),
    key(0x57, "NumpadAdd", 78, 0x4e, Some(0x45)),
    key(0x58, "NumpadEnter", 96, 0xe01c, Some(0x4c)),
    key(0x59, "Numpad1", 79, 0x4f, Some(0x53)),
    key(0x5a, "Numpad2", 80, 0x50, Some(0x54)),
    key(0x5b, "Numpad3", 81, 0x51, Some(0x55)),
    key(0x5c, "Numpad4", 75, 0x4b, Some(0x56)),
    key(0x5d, "Numpad5", 76, 0x4c, Some(0x57)),
    key(0x5e, "Numpad6", 77, 0x4d, Some(0x58)),
    key(0x5f, "Numpad7", 71, 0x47, Some(0x59)),
    key(0x60, "Numpad8", 72, 0x48, Some(0x5b)),
    key(0x61, "Numpad9", 73, 0x49, Some(0x5c)),
    key(0x62, "Numpad0", 82, 0x52, Some(0x52)),
    key(0x63, "NumpadDecimal", 83, 0x53, Some(0x41)),
    key(0x64, "IntlBackslash", 86, 0x56, Some(0x0a)),
    key(0x65, "ContextMenu", 127, 0xe05d, Some(0x6e)),
    key(0x67, "NumpadEqual", 117, 0x59, Some(0x51)),
    key(0x68, "F13", 183, 0x64, Some(0x69)),
    key(0x69, "F14", 184, 0x65, Some(0x6b)),
    key(0x6a, "F15", 185, 0x66, Some(0x71)),
    key(0x6b, "F16", 186, 0x67, Some(0x6a)),
    key(0x6c, "F17", 187, 0x68, Some(0x40)),
    key(0x6d, "F18", 188, 0x69, Some(0x4f)),
    key(0x6e, "F19", 189, 0x6a, Some(0x50)),
    key(0x6f, "F20", 190, 0x6b, Some(0x5a)),
    key(0x70, "F21", 191, 0x6c, None),
    key(0x71, "F22", 192, 0x6d, None),
    key(0x72, "F23", 193, 0x6e, None),
    key(0x73, "F24", 194, 0x76, None),
    key(0x87, "IntlRo", 89, 0x73, Some(0x5e)),
    key(0x89, "IntlYen", 124, 0x7d, Some(0x5d)),
    key(0xe0, "ControlLeft", 29, 0x1d, Some(0x3b)),
    key(0xe1, "ShiftLeft", 42, 0x2a, Some(0x38)),
    key(0xe2, "AltLeft", 56, 0x38, Some(0x3a)),
    key(0xe3, "MetaLeft", 125, 0xe05b, Some(0x37)),
    key(0xe4, "ControlRight", 97, 0xe01d, Some(0x3e)),
    key(0xe5, "ShiftRight", 54, 0x36, Some(0x3c)),
    key(0xe6, "AltRight", 100, 0xe038, Some(0x3d)),
    key(0xe7, "MetaRight", 126, 0xe05c, Some(0x36)),
];

pub(crate) fn by_usage(usage: u16) -> Option<&'static KeyEntry> {
    KEYS.iter().find(|entry| entry.usage == usage)
}

pub fn usage_from_code(code: &str) -> Option<u16> {
    KEYS.iter()
        .find(|entry| entry.code == code)
        .map(|entry| entry.usage)
}

pub fn code_from_usage(usage: u16) -> Option<&'static str> {
    by_usage(usage).map(|entry| entry.code)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;

    #[test]
    fn every_column_is_unique() {
        let mut usages = HashSet::new();
        let mut codes = HashSet::new();
        let mut evdev = HashSet::new();
        let mut windows = HashSet::new();
        let mut mac = HashSet::new();
        for entry in KEYS {
            assert!(usages.insert(entry.usage), "usage {:#x}", entry.usage);
            assert!(codes.insert(entry.code), "code {}", entry.code);
            assert!(evdev.insert(entry.evdev), "evdev {}", entry.evdev);
            assert!(
                windows.insert(entry.windows_scan),
                "scan {:#x}",
                entry.windows_scan
            );
            if let Some(code) = entry.mac {
                assert!(mac.insert(code), "mac {code:#x}");
            }
        }
    }

    #[test]
    fn windows_scan_codes_are_plain_or_e0_extended() {
        for entry in KEYS {
            let prefix = entry.windows_scan & 0xff00;
            assert!(prefix == 0 || prefix == WINDOWS_EXTENDED, "{}", entry.code);
            assert!(entry.windows_scan & 0x00ff != 0, "{}", entry.code);
        }
    }

    #[test]
    fn evdev_matches_windows_scan_set_one_for_classic_keys() {
        for entry in KEYS {
            if entry.windows_scan < 0x59 && entry.evdev < 89 && entry.code != "Pause" {
                assert_eq!(entry.evdev, entry.windows_scan, "{}", entry.code);
            }
        }
    }

    #[test]
    fn code_and_usage_lookups_are_inverse() {
        for entry in KEYS {
            assert_eq!(usage_from_code(entry.code), Some(entry.usage));
            assert_eq!(code_from_usage(entry.usage), Some(entry.code));
        }
        assert_eq!(usage_from_code("KeyA"), Some(0x04));
        assert_eq!(usage_from_code("NotAKey"), None);
        assert_eq!(code_from_usage(0x00), None);
    }
}

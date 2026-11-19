mod error;
mod frame;
mod input;
mod keymap;

#[cfg(target_os = "linux")]
mod linux;
#[cfg(target_os = "macos")]
mod macos;
#[cfg(target_os = "macos")]
mod macos_input;
#[cfg(any(windows, target_os = "linux", test))]
mod pointer;
#[cfg(any(windows, test))]
mod rotation;
#[cfg(any(target_os = "macos", target_os = "linux", test))]
mod slot;
#[cfg(windows)]
mod windows;
#[cfg(windows)]
mod windows_gdi;
#[cfg(windows)]
mod windows_input;

pub use error::{DesktopError, Permission};
pub use frame::{CaptureOptions, CapturedFrame, DisplayInfo};
pub use input::{InputEvent, InputInjector, PointerButton, WHEEL_STEP};
pub use keymap::{code_from_usage, usage_from_code};

#[cfg(target_os = "linux")]
pub use linux::{ScreenCapturer, displays};
#[cfg(target_os = "macos")]
pub use macos::{ScreenCapturer, displays};
#[cfg(windows)]
pub use windows::{ScreenCapturer, displays};

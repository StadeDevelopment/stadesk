mod wayland;
mod x11;
mod x11_input;

use std::time::Duration;

use crate::error::DesktopError;
use crate::frame::{CaptureOptions, CapturedFrame, DisplayInfo};
use crate::input::InputInjector;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Session {
    X11,
    Wayland,
}

fn environment_set(name: &str) -> bool {
    std::env::var_os(name).is_some_and(|value| !value.is_empty())
}

fn session() -> Result<Session, DesktopError> {
    if environment_set("WAYLAND_DISPLAY") {
        Ok(Session::Wayland)
    } else if environment_set("DISPLAY") {
        Ok(Session::X11)
    } else {
        Err(DesktopError::Platform {
            code: 0,
            message: "no graphical session was found (neither WAYLAND_DISPLAY nor DISPLAY is set)"
                .to_owned(),
        })
    }
}

pub fn displays() -> Result<Vec<DisplayInfo>, DesktopError> {
    match session()? {
        Session::X11 => x11::displays(),
        Session::Wayland => Ok(wayland::displays()),
    }
}

enum Backend {
    X11(Box<x11::X11Capturer>),
    Wayland(wayland::WaylandCapturer),
}

pub struct ScreenCapturer {
    backend: Backend,
}

impl ScreenCapturer {
    pub fn start(display_id: u32, options: CaptureOptions) -> Result<Self, DesktopError> {
        let backend = match session()? {
            Session::X11 => Backend::X11(Box::new(x11::X11Capturer::start(display_id, options)?)),
            Session::Wayland => {
                Backend::Wayland(wayland::WaylandCapturer::start(display_id, options)?)
            }
        };
        Ok(Self { backend })
    }

    pub fn display(&self) -> DisplayInfo {
        match &self.backend {
            Backend::X11(capturer) => capturer.display(),
            Backend::Wayland(capturer) => capturer.display(),
        }
    }

    pub fn method(&self) -> &'static str {
        match &self.backend {
            Backend::X11(capturer) => capturer.method(),
            Backend::Wayland(capturer) => capturer.method(),
        }
    }

    pub fn input(&self) -> Result<InputInjector, DesktopError> {
        match &self.backend {
            Backend::X11(capturer) => capturer.input(),
            Backend::Wayland(capturer) => capturer.input(),
        }
    }

    pub fn next_frame(&mut self, timeout: Duration) -> Result<Option<CapturedFrame>, DesktopError> {
        match &mut self.backend {
            Backend::X11(capturer) => capturer.next_frame(timeout),
            Backend::Wayland(capturer) => capturer.next_frame(timeout),
        }
    }
}

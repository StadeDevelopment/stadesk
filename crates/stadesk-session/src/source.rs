use std::time::Duration;

use stadesk_desktop::{
    CaptureOptions, CapturedFrame, DesktopError, InputEvent, InputInjector, ScreenCapturer,
    displays,
};

pub trait FrameSource {
    fn next_frame(&mut self, timeout: Duration) -> Result<Option<CapturedFrame>, DesktopError>;
}

pub trait InputSink: Send {
    fn inject(&mut self, event: InputEvent) -> Result<(), DesktopError>;
    fn resize(&mut self, width: u32, height: u32);
}

pub struct OpenedDesktop {
    pub frames: Box<dyn FrameSource>,
    pub input: Result<Box<dyn InputSink>, DesktopError>,
}

pub trait DesktopProvider: Send + Sync + 'static {
    fn open(&self) -> Result<OpenedDesktop, DesktopError>;
}

impl FrameSource for ScreenCapturer {
    fn next_frame(&mut self, timeout: Duration) -> Result<Option<CapturedFrame>, DesktopError> {
        ScreenCapturer::next_frame(self, timeout)
    }
}

impl InputSink for InputInjector {
    fn inject(&mut self, event: InputEvent) -> Result<(), DesktopError> {
        InputInjector::inject(self, event)
    }

    fn resize(&mut self, width: u32, height: u32) {
        self.set_size(width, height);
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct ScreenProvider {
    pub display: Option<u32>,
    pub options: CaptureOptions,
}

impl DesktopProvider for ScreenProvider {
    fn open(&self) -> Result<OpenedDesktop, DesktopError> {
        let display = match self.display {
            Some(display) => display,
            None => {
                let list = displays()?;
                list.iter()
                    .find(|display| display.primary)
                    .or_else(|| list.first())
                    .map(|display| display.id)
                    .ok_or(DesktopError::DisplayNotFound(0))?
            }
        };
        let capturer = ScreenCapturer::start(display, self.options)?;
        let input = capturer
            .input()
            .map(|injector| Box::new(injector) as Box<dyn InputSink>);
        Ok(OpenedDesktop {
            frames: Box::new(capturer),
            input,
        })
    }
}

pub fn bitrate_for(width: u32, height: u32) -> u32 {
    let pixels = u64::from(width) * u64::from(height);
    (pixels / 500).clamp(1_500, 16_000) as u32
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bitrate_scales_with_resolution_within_bounds() {
        assert_eq!(bitrate_for(320, 240), 1_500);
        assert_eq!(bitrate_for(1920, 1080), 4_147);
        assert_eq!(bitrate_for(2940, 1912), 11_242);
        assert_eq!(bitrate_for(7680, 4320), 16_000);
    }
}

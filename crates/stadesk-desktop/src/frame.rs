use crate::error::DesktopError;

pub(crate) const BGRA_BYTES: usize = 4;
const MAX_FRAME_RATE: u32 = 120;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DisplayInfo {
    pub id: u32,
    pub width: u32,
    pub height: u32,
    pub primary: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CaptureOptions {
    pub max_frame_rate: u32,
    pub show_cursor: bool,
}

impl Default for CaptureOptions {
    fn default() -> Self {
        Self {
            max_frame_rate: 30,
            show_cursor: true,
        }
    }
}

impl CaptureOptions {
    pub(crate) fn validate(&self) -> Result<(), DesktopError> {
        if self.max_frame_rate == 0 || self.max_frame_rate > MAX_FRAME_RATE {
            return Err(DesktopError::InvalidOption(
                "frame rate must be between 1 and 120",
            ));
        }
        Ok(())
    }
}

#[derive(Clone, PartialEq, Eq)]
pub struct CapturedFrame {
    pub width: u32,
    pub height: u32,
    pub bgra: Vec<u8>,
}

impl CapturedFrame {
    pub fn stride(&self) -> usize {
        self.width as usize * BGRA_BYTES
    }
}

impl std::fmt::Debug for CapturedFrame {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("CapturedFrame")
            .field("width", &self.width)
            .field("height", &self.height)
            .finish_non_exhaustive()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn frame(width: u32) -> CapturedFrame {
        CapturedFrame {
            width,
            height: 1,
            bgra: vec![0; width as usize * BGRA_BYTES],
        }
    }

    #[test]
    fn options_validate_frame_rate_bounds() {
        assert!(CaptureOptions::default().validate().is_ok());
        for rate in [0, 121] {
            let options = CaptureOptions {
                max_frame_rate: rate,
                ..CaptureOptions::default()
            };
            assert!(options.validate().is_err());
        }
    }

    #[test]
    fn stride_is_tight_bgra_row() {
        assert_eq!(frame(10).stride(), 40);
    }
}

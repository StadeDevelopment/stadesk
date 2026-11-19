use std::ffi::c_void;
use std::time::{Duration, Instant};

use windows::Win32::Foundation::RECT;
use windows::Win32::Graphics::Gdi::{
    BI_RGB, BITMAPINFO, BITMAPINFOHEADER, BitBlt, CAPTUREBLT, CreateCompatibleDC, CreateDIBSection,
    DIB_RGB_COLORS, DeleteDC, DeleteObject, GetDC, HBITMAP, HDC, HGDIOBJ, ReleaseDC, SRCCOPY,
    SelectObject,
};
use windows::Win32::UI::WindowsAndMessaging::{
    CURSOR_SHOWING, CURSORINFO, DI_NORMAL, DrawIconEx, GetCursorInfo, GetIconInfo, HICON, ICONINFO,
};

use crate::error::DesktopError;
use crate::frame::{BGRA_BYTES, CaptureOptions, CapturedFrame};
use crate::windows::DpiAwareness;

struct ScreenDc(HDC);

impl Drop for ScreenDc {
    fn drop(&mut self) {
        unsafe { ReleaseDC(None, self.0) };
    }
}

struct Surface {
    dc: HDC,
    bitmap: HBITMAP,
    previous: HGDIOBJ,
    bits: *mut c_void,
}

impl Drop for Surface {
    fn drop(&mut self) {
        unsafe {
            SelectObject(self.dc, self.previous);
            let _ = DeleteObject(self.bitmap.into());
            let _ = DeleteDC(self.dc);
        }
    }
}

pub(crate) struct GdiCapturer {
    desktop: RECT,
    width: u32,
    height: u32,
    options: CaptureOptions,
    surface: Surface,
    last_frame: Option<Vec<u8>>,
    last_grab: Option<Instant>,
}

unsafe impl Send for GdiCapturer {}

fn gdi_failure(operation: &'static str) -> DesktopError {
    DesktopError::Platform {
        code: 0,
        message: format!("{operation} failed"),
    }
}

impl GdiCapturer {
    pub(crate) fn start(desktop: RECT, options: CaptureOptions) -> Result<Self, DesktopError> {
        let width = u32::try_from(desktop.right - desktop.left).unwrap_or(0);
        let height = u32::try_from(desktop.bottom - desktop.top).unwrap_or(0);
        if width == 0 || height == 0 {
            return Err(gdi_failure("measuring the display"));
        }
        let _dpi = DpiAwareness::per_monitor();
        let screen = ScreenDc(unsafe { GetDC(None) });
        if screen.0.is_invalid() {
            return Err(gdi_failure("GetDC"));
        }
        let dc = unsafe { CreateCompatibleDC(Some(screen.0)) };
        if dc.is_invalid() {
            return Err(gdi_failure("CreateCompatibleDC"));
        }
        let info = BITMAPINFO {
            bmiHeader: BITMAPINFOHEADER {
                biSize: size_of::<BITMAPINFOHEADER>() as u32,
                biWidth: width as i32,
                biHeight: -(height as i32),
                biPlanes: 1,
                biBitCount: 32,
                biCompression: BI_RGB.0,
                ..Default::default()
            },
            ..Default::default()
        };
        let mut bits = std::ptr::null_mut();
        let bitmap = match unsafe {
            CreateDIBSection(Some(screen.0), &info, DIB_RGB_COLORS, &mut bits, None, 0)
        } {
            Ok(bitmap) if !bits.is_null() => bitmap,
            _ => {
                unsafe {
                    let _ = DeleteDC(dc);
                }
                return Err(gdi_failure("CreateDIBSection"));
            }
        };
        let previous = unsafe { SelectObject(dc, bitmap.into()) };
        Ok(Self {
            desktop,
            width,
            height,
            options,
            surface: Surface {
                dc,
                bitmap,
                previous,
                bits,
            },
            last_frame: None,
            last_grab: None,
        })
    }

    fn draw_cursor(&self) {
        let mut cursor = CURSORINFO {
            cbSize: size_of::<CURSORINFO>() as u32,
            ..Default::default()
        };
        if unsafe { GetCursorInfo(&mut cursor) }.is_err()
            || cursor.flags.0 & CURSOR_SHOWING.0 == 0
            || cursor.hCursor.is_invalid()
        {
            return;
        }
        let icon = HICON(cursor.hCursor.0);
        let mut details = ICONINFO::default();
        if unsafe { GetIconInfo(icon, &mut details) }.is_err() {
            return;
        }
        let left = cursor.ptScreenPos.x - self.desktop.left - details.xHotspot as i32;
        let top = cursor.ptScreenPos.y - self.desktop.top - details.yHotspot as i32;
        unsafe {
            let _ = DrawIconEx(self.surface.dc, left, top, icon, 0, 0, 0, None, DI_NORMAL);
            if !details.hbmMask.is_invalid() {
                let _ = DeleteObject(details.hbmMask.into());
            }
            if !details.hbmColor.is_invalid() {
                let _ = DeleteObject(details.hbmColor.into());
            }
        }
    }

    fn grab(&mut self) -> Result<Vec<u8>, DesktopError> {
        let _dpi = DpiAwareness::per_monitor();
        let screen = ScreenDc(unsafe { GetDC(None) });
        if screen.0.is_invalid() {
            return Err(gdi_failure("GetDC"));
        }
        unsafe {
            BitBlt(
                self.surface.dc,
                0,
                0,
                self.width as i32,
                self.height as i32,
                Some(screen.0),
                self.desktop.left,
                self.desktop.top,
                SRCCOPY | CAPTUREBLT,
            )
        }
        .map_err(|_| gdi_failure("BitBlt"))?;
        drop(screen);
        if self.options.show_cursor {
            self.draw_cursor();
        }
        let length = self.width as usize * self.height as usize * BGRA_BYTES;
        let pixels = unsafe {
            std::slice::from_raw_parts(self.surface.bits.cast::<u8>().cast_const(), length)
        };
        let mut bgra = pixels.to_vec();
        for pixel in bgra.as_chunks_mut::<BGRA_BYTES>().0 {
            pixel[3] = 0xff;
        }
        Ok(bgra)
    }

    pub(crate) fn next_frame(
        &mut self,
        timeout: Duration,
    ) -> Result<Option<CapturedFrame>, DesktopError> {
        let deadline = Instant::now() + timeout;
        let interval = Duration::from_secs(1) / self.options.max_frame_rate;
        loop {
            let now = Instant::now();
            if let Some(last) = self.last_grab {
                let earliest = last + interval;
                if earliest > now {
                    if earliest >= deadline {
                        std::thread::sleep(deadline.saturating_duration_since(now));
                        return Ok(None);
                    }
                    std::thread::sleep(earliest - now);
                }
            }
            self.last_grab = Some(Instant::now());
            let bgra = self.grab()?;
            if self.last_frame.as_deref() != Some(bgra.as_slice()) {
                self.last_frame = Some(bgra.clone());
                return Ok(Some(CapturedFrame {
                    width: self.width,
                    height: self.height,
                    bgra,
                }));
            }
            if Instant::now() >= deadline {
                return Ok(None);
            }
        }
    }
}

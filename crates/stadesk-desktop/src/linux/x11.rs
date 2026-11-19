use std::ffi::c_void;
use std::fmt::Display;
use std::ptr::{self, NonNull};
use std::time::{Duration, Instant};

use rustix::fs::{MemfdFlags, ftruncate, memfd_create};
use rustix::mm::{MapFlags, ProtFlags, mmap, munmap};
use x11rb::connection::{Connection, RequestConnection};
use x11rb::protocol::xproto::{self, ImageFormat, ImageOrder};
use x11rb::protocol::{randr, shm, xfixes};
use x11rb::rust_connection::RustConnection;

use crate::error::DesktopError;
use crate::frame::{BGRA_BYTES, CaptureOptions, CapturedFrame, DisplayInfo};
use crate::input::InputInjector;
use crate::linux::x11_input::X11Input;
use crate::pointer::{PointerKind, PointerShape, draw_pointer};

const ALL_PLANES: u32 = u32::MAX;
const RED_MASK: u32 = 0x00ff_0000;
const GREEN_MASK: u32 = 0x0000_ff00;
const BLUE_MASK: u32 = 0x0000_00ff;
const BITS_PER_PIXEL: u8 = 32;
const XFIXES_MAJOR: u32 = 6;
const XFIXES_MINOR: u32 = 0;

fn x_error(error: impl Display) -> DesktopError {
    DesktopError::Platform {
        code: 0,
        message: format!("X11: {error}"),
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Monitor {
    x: i16,
    y: i16,
    width: u16,
    height: u16,
    primary: bool,
}

fn connect() -> Result<(RustConnection, xproto::Screen), DesktopError> {
    let (connection, screen_index) = RustConnection::connect(None).map_err(x_error)?;
    let screen = connection
        .setup()
        .roots
        .get(screen_index)
        .cloned()
        .ok_or_else(|| x_error("the default screen does not exist"))?;
    Ok((connection, screen))
}

fn has_extension(connection: &RustConnection, name: &'static str) -> Result<bool, DesktopError> {
    Ok(connection
        .extension_information(name)
        .map_err(x_error)?
        .is_some())
}

fn monitors(
    connection: &RustConnection,
    screen: &xproto::Screen,
) -> Result<Vec<Monitor>, DesktopError> {
    let mut list = Vec::new();
    if has_extension(connection, randr::X11_EXTENSION_NAME)? {
        let reply = randr::get_monitors(connection, screen.root, true)
            .map_err(x_error)?
            .reply()
            .map_err(x_error)?;
        list = reply
            .monitors
            .iter()
            .filter(|monitor| monitor.width > 0 && monitor.height > 0)
            .map(|monitor| Monitor {
                x: monitor.x,
                y: monitor.y,
                width: monitor.width,
                height: monitor.height,
                primary: monitor.primary,
            })
            .collect();
    }
    if list.is_empty() {
        list.push(Monitor {
            x: 0,
            y: 0,
            width: screen.width_in_pixels,
            height: screen.height_in_pixels,
            primary: true,
        });
    }
    if !list.iter().any(|monitor| monitor.primary) {
        list[0].primary = true;
    }
    Ok(list)
}

fn describe(index: usize, monitor: &Monitor) -> DisplayInfo {
    DisplayInfo {
        id: index as u32,
        width: u32::from(monitor.width),
        height: u32::from(monitor.height),
        primary: monitor.primary,
    }
}

pub(crate) fn displays() -> Result<Vec<DisplayInfo>, DesktopError> {
    let (connection, screen) = connect()?;
    Ok(monitors(&connection, &screen)?
        .iter()
        .enumerate()
        .map(|(index, monitor)| describe(index, monitor))
        .collect())
}

fn check_pixel_format(
    connection: &RustConnection,
    screen: &xproto::Screen,
) -> Result<(), DesktopError> {
    let setup = connection.setup();
    let unsupported = || x_error("the screen does not use 32-bit little-endian BGRX pixels");
    if setup.image_byte_order != ImageOrder::LSB_FIRST {
        return Err(unsupported());
    }
    let bits_per_pixel = setup
        .pixmap_formats
        .iter()
        .find(|format| format.depth == screen.root_depth)
        .map(|format| format.bits_per_pixel);
    if bits_per_pixel != Some(BITS_PER_PIXEL) {
        return Err(unsupported());
    }
    let visual = screen
        .allowed_depths
        .iter()
        .flat_map(|depth| depth.visuals.iter())
        .find(|visual| visual.visual_id == screen.root_visual)
        .ok_or_else(unsupported)?;
    if visual.red_mask != RED_MASK
        || visual.green_mask != GREEN_MASK
        || visual.blue_mask != BLUE_MASK
    {
        return Err(unsupported());
    }
    Ok(())
}

struct SharedImage {
    segment: shm::Seg,
    pointer: NonNull<c_void>,
    length: usize,
}

impl SharedImage {
    fn create(connection: &RustConnection, length: usize) -> Result<Option<Self>, DesktopError> {
        if !has_extension(connection, shm::X11_EXTENSION_NAME)? {
            return Ok(None);
        }
        let version = shm::query_version(connection)
            .map_err(x_error)?
            .reply()
            .map_err(x_error)?;
        if (version.major_version, version.minor_version) < (1, 2) {
            return Ok(None);
        }
        let file = memfd_create("stadesk-x11-capture", MemfdFlags::CLOEXEC).map_err(x_error)?;
        ftruncate(&file, length as u64).map_err(x_error)?;
        let mapped = unsafe {
            mmap(
                ptr::null_mut(),
                length,
                ProtFlags::READ | ProtFlags::WRITE,
                MapFlags::SHARED,
                &file,
                0,
            )
        }
        .map_err(x_error)?;
        let Some(pointer) = NonNull::new(mapped) else {
            return Ok(None);
        };
        let mut image = Self {
            segment: 0,
            pointer,
            length,
        };
        let segment = connection.generate_id().map_err(x_error)?;
        let shared = file.try_clone().map_err(x_error)?;
        let attached = shm::attach_fd(connection, segment, shared, false)
            .map_err(x_error)?
            .check();
        if attached.is_err() {
            return Ok(None);
        }
        image.segment = segment;
        Ok(Some(image))
    }

    fn bytes(&self) -> &[u8] {
        unsafe { std::slice::from_raw_parts(self.pointer.as_ptr().cast::<u8>(), self.length) }
    }
}

impl Drop for SharedImage {
    fn drop(&mut self) {
        let _ = unsafe { munmap(self.pointer.as_ptr(), self.length) };
    }
}

struct CursorSample {
    shape: PointerShape,
    left: i32,
    top: i32,
    state: (i16, i16, u32),
}

pub(crate) struct X11Capturer {
    connection: RustConnection,
    root: xproto::Window,
    monitor: Monitor,
    shared: Option<SharedImage>,
    cursor_supported: bool,
    display: DisplayInfo,
    options: CaptureOptions,
    previous: Option<Vec<u8>>,
    previous_cursor: Option<(i16, i16, u32)>,
    last_delivery: Option<Instant>,
}

impl X11Capturer {
    pub(crate) fn start(display_id: u32, options: CaptureOptions) -> Result<Self, DesktopError> {
        options.validate()?;
        let (connection, screen) = connect()?;
        check_pixel_format(&connection, &screen)?;
        let list = monitors(&connection, &screen)?;
        let monitor = *list
            .get(display_id as usize)
            .ok_or(DesktopError::DisplayNotFound(display_id))?;
        let length = usize::from(monitor.width) * usize::from(monitor.height) * BGRA_BYTES;
        let shared = SharedImage::create(&connection, length)?;
        let cursor_supported = has_extension(&connection, xfixes::X11_EXTENSION_NAME)?
            && xfixes::query_version(&connection, XFIXES_MAJOR, XFIXES_MINOR)
                .map_err(x_error)?
                .reply()
                .is_ok();
        Ok(Self {
            root: screen.root,
            display: describe(display_id as usize, &monitor),
            connection,
            monitor,
            shared,
            cursor_supported,
            options,
            previous: None,
            previous_cursor: None,
            last_delivery: None,
        })
    }

    pub(crate) fn display(&self) -> DisplayInfo {
        self.display
    }

    pub(crate) fn input(&self) -> Result<InputInjector, DesktopError> {
        let backend = X11Input::new(self.monitor.x, self.monitor.y)?;
        Ok(InputInjector::new(
            Box::new(backend),
            self.display.width,
            self.display.height,
        ))
    }

    pub(crate) fn method(&self) -> &'static str {
        if self.shared.is_some() {
            "X11 (MIT-SHM)"
        } else {
            "X11 (GetImage)"
        }
    }

    fn grab(&self) -> Result<Vec<u8>, DesktopError> {
        let monitor = self.monitor;
        let mut pixels = match &self.shared {
            Some(shared) => {
                shm::get_image(
                    &self.connection,
                    self.root,
                    monitor.x,
                    monitor.y,
                    monitor.width,
                    monitor.height,
                    ALL_PLANES,
                    ImageFormat::Z_PIXMAP.into(),
                    shared.segment,
                    0,
                )
                .map_err(x_error)?
                .reply()
                .map_err(x_error)?;
                shared.bytes().to_vec()
            }
            None => {
                xproto::get_image(
                    &self.connection,
                    ImageFormat::Z_PIXMAP,
                    self.root,
                    monitor.x,
                    monitor.y,
                    monitor.width,
                    monitor.height,
                    ALL_PLANES,
                )
                .map_err(x_error)?
                .reply()
                .map_err(x_error)?
                .data
            }
        };
        let expected = usize::from(monitor.width) * usize::from(monitor.height) * BGRA_BYTES;
        if pixels.len() < expected {
            return Err(x_error("the server returned a truncated image"));
        }
        pixels.truncate(expected);
        for pixel in pixels.as_chunks_mut::<BGRA_BYTES>().0 {
            pixel[3] = 0xff;
        }
        Ok(pixels)
    }

    fn cursor(&self) -> Option<CursorSample> {
        if !self.cursor_supported || !self.options.show_cursor {
            return None;
        }
        let image = xfixes::get_cursor_image(&self.connection)
            .ok()?
            .reply()
            .ok()?;
        let data: Vec<u8> = image
            .cursor_image
            .iter()
            .flat_map(|argb| argb.to_le_bytes())
            .collect();
        let shape = PointerShape {
            kind: PointerKind::PremultipliedColor,
            width: u32::from(image.width),
            height: u32::from(image.height),
            pitch: usize::from(image.width) * BGRA_BYTES,
            data,
        };
        let left = i32::from(image.x) - i32::from(image.xhot) - i32::from(self.monitor.x);
        let top = i32::from(image.y) - i32::from(image.yhot) - i32::from(self.monitor.y);
        Some(CursorSample {
            shape,
            left,
            top,
            state: (image.x, image.y, image.cursor_serial),
        })
    }

    pub(crate) fn next_frame(
        &mut self,
        timeout: Duration,
    ) -> Result<Option<CapturedFrame>, DesktopError> {
        let deadline = Instant::now() + timeout;
        let interval = Duration::from_secs(1) / self.options.max_frame_rate;
        loop {
            if let Some(last) = self.last_delivery {
                let earliest = last + interval;
                let now = Instant::now();
                if earliest > now {
                    if earliest >= deadline {
                        std::thread::sleep(deadline.saturating_duration_since(now));
                        return Ok(None);
                    }
                    std::thread::sleep(earliest - now);
                }
            }
            let pixels = self.grab()?;
            let cursor = self.cursor();
            let cursor_state = cursor.as_ref().map(|sample| sample.state);
            let changed = self.previous.as_deref() != Some(pixels.as_slice())
                || cursor_state != self.previous_cursor;
            if changed {
                let mut frame = CapturedFrame {
                    width: u32::from(self.monitor.width),
                    height: u32::from(self.monitor.height),
                    bgra: pixels.clone(),
                };
                if let Some(sample) = &cursor {
                    draw_pointer(&mut frame, &sample.shape, sample.left, sample.top);
                }
                self.previous = Some(pixels);
                self.previous_cursor = cursor_state;
                self.last_delivery = Some(Instant::now());
                return Ok(Some(frame));
            }
            let now = Instant::now();
            if now >= deadline {
                return Ok(None);
            }
            std::thread::sleep(interval.min(deadline - now));
            if Instant::now() >= deadline {
                return Ok(None);
            }
        }
    }
}

impl Drop for X11Capturer {
    fn drop(&mut self) {
        if let Some(shared) = &self.shared {
            let _ = shm::detach(&self.connection, shared.segment);
            let _ = self.connection.flush();
        }
    }
}

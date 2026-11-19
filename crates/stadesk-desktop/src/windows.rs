use std::time::{Duration, Instant};

use windows::Win32::Foundation::{E_ACCESSDENIED, HMODULE, RECT};
use windows::Win32::Graphics::Direct3D::D3D_DRIVER_TYPE_UNKNOWN;
use windows::Win32::Graphics::Direct3D11::{
    D3D11_CPU_ACCESS_READ, D3D11_CREATE_DEVICE_BGRA_SUPPORT, D3D11_MAP_READ,
    D3D11_MAPPED_SUBRESOURCE, D3D11_SDK_VERSION, D3D11_TEXTURE2D_DESC, D3D11_USAGE_STAGING,
    D3D11CreateDevice, ID3D11Device, ID3D11DeviceContext, ID3D11Texture2D,
};
use windows::Win32::Graphics::Dxgi::Common::{
    DXGI_FORMAT_B8G8R8A8_UNORM, DXGI_MODE_ROTATION, DXGI_MODE_ROTATION_ROTATE90,
    DXGI_MODE_ROTATION_ROTATE180, DXGI_MODE_ROTATION_ROTATE270, DXGI_SAMPLE_DESC,
};
use windows::Win32::Graphics::Dxgi::{
    CreateDXGIFactory1, DXGI_ERROR_ACCESS_LOST, DXGI_ERROR_NOT_CURRENTLY_AVAILABLE,
    DXGI_ERROR_NOT_FOUND, DXGI_ERROR_SESSION_DISCONNECTED, DXGI_ERROR_WAIT_TIMEOUT,
    DXGI_OUTDUPL_FRAME_INFO, DXGI_OUTDUPL_POINTER_SHAPE_INFO,
    DXGI_OUTDUPL_POINTER_SHAPE_TYPE_COLOR, DXGI_OUTDUPL_POINTER_SHAPE_TYPE_MASKED_COLOR,
    DXGI_OUTDUPL_POINTER_SHAPE_TYPE_MONOCHROME, DXGI_OUTPUT_DESC, IDXGIAdapter1, IDXGIFactory1,
    IDXGIOutput, IDXGIOutput1, IDXGIOutputDuplication, IDXGIResource,
};
use windows::Win32::UI::HiDpi::{
    DPI_AWARENESS_CONTEXT, DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2, SetThreadDpiAwarenessContext,
};
use windows::core::Interface;

use crate::error::DesktopError;
use crate::frame::{CaptureOptions, CapturedFrame, DisplayInfo};
use crate::input::InputInjector;
use crate::pointer::{PointerKind, PointerShape, draw_pointer};
use crate::rotation::{Rotation, rotate_into_frame};
use crate::windows_gdi::GdiCapturer;
use crate::windows_input::WindowsInput;

const RETRY_PAUSE: Duration = Duration::from_millis(100);
const ADAPTER_SHIFT: u32 = 8;
const OUTPUT_MASK: u32 = 0xff;

pub(crate) struct DpiAwareness(DPI_AWARENESS_CONTEXT);

impl DpiAwareness {
    pub(crate) fn per_monitor() -> Self {
        Self(unsafe { SetThreadDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2) })
    }
}

impl Drop for DpiAwareness {
    fn drop(&mut self) {
        if !self.0.0.is_null() {
            unsafe { SetThreadDpiAwarenessContext(self.0) };
        }
    }
}

fn failed(operation: &'static str) -> impl Fn(windows::core::Error) -> DesktopError {
    move |error| DesktopError::Platform {
        code: i64::from(error.code().0),
        message: format!("{operation}: {}", error.message()),
    }
}

fn display_id(adapter_index: u32, output_index: u32) -> u32 {
    (adapter_index << ADAPTER_SHIFT) | output_index
}

fn rotation_of(rotation: DXGI_MODE_ROTATION) -> Rotation {
    if rotation == DXGI_MODE_ROTATION_ROTATE90 {
        Rotation::Clockwise90
    } else if rotation == DXGI_MODE_ROTATION_ROTATE180 {
        Rotation::Clockwise180
    } else if rotation == DXGI_MODE_ROTATION_ROTATE270 {
        Rotation::Clockwise270
    } else {
        Rotation::Identity
    }
}

fn describe(id: u32, description: &DXGI_OUTPUT_DESC) -> DisplayInfo {
    let bounds = description.DesktopCoordinates;
    DisplayInfo {
        id,
        width: (bounds.right - bounds.left).max(0) as u32,
        height: (bounds.bottom - bounds.top).max(0) as u32,
        primary: bounds.left == 0 && bounds.top == 0,
    }
}

fn adapters() -> Result<Vec<IDXGIAdapter1>, DesktopError> {
    let factory: IDXGIFactory1 =
        unsafe { CreateDXGIFactory1() }.map_err(failed("CreateDXGIFactory1"))?;
    let mut list = Vec::new();
    for index in 0.. {
        match unsafe { factory.EnumAdapters1(index) } {
            Ok(adapter) => list.push(adapter),
            Err(error) if error.code() == DXGI_ERROR_NOT_FOUND => break,
            Err(error) => return Err(failed("EnumAdapters1")(error)),
        }
    }
    Ok(list)
}

fn outputs(adapter: &IDXGIAdapter1) -> Result<Vec<IDXGIOutput>, DesktopError> {
    let mut list = Vec::new();
    for index in 0.. {
        match unsafe { adapter.EnumOutputs(index) } {
            Ok(output) => list.push(output),
            Err(error) if error.code() == DXGI_ERROR_NOT_FOUND => break,
            Err(error) => return Err(failed("EnumOutputs")(error)),
        }
    }
    Ok(list)
}

pub fn displays() -> Result<Vec<DisplayInfo>, DesktopError> {
    let _dpi = DpiAwareness::per_monitor();
    let mut list = Vec::new();
    for (adapter_index, adapter) in adapters()?.iter().enumerate() {
        for (output_index, output) in outputs(adapter)?.iter().enumerate() {
            let description =
                unsafe { output.GetDesc() }.map_err(failed("IDXGIOutput::GetDesc"))?;
            if description.AttachedToDesktop.as_bool() && output_index as u32 <= OUTPUT_MASK {
                list.push(describe(
                    display_id(adapter_index as u32, output_index as u32),
                    &description,
                ));
            }
        }
    }
    Ok(list)
}

struct AcquiredFrame<'a>(&'a IDXGIOutputDuplication);

impl Drop for AcquiredFrame<'_> {
    fn drop(&mut self) {
        let _ = unsafe { self.0.ReleaseFrame() };
    }
}

struct Pointer {
    shape: Option<PointerShape>,
    left: i32,
    top: i32,
    visible: bool,
}

pub struct ScreenCapturer {
    backend: Backend,
    display: DisplayInfo,
    desktop: RECT,
}

enum Backend {
    Duplication(Box<Duplication>),
    Gdi(GdiCapturer),
}

impl ScreenCapturer {
    pub fn start(display_id: u32, options: CaptureOptions) -> Result<Self, DesktopError> {
        options.validate()?;
        let _dpi = DpiAwareness::per_monitor();
        let adapter_index = (display_id >> ADAPTER_SHIFT) as usize;
        let output_index = (display_id & OUTPUT_MASK) as usize;
        let adapter = adapters()?
            .into_iter()
            .nth(adapter_index)
            .ok_or(DesktopError::DisplayNotFound(display_id))?;
        let output = outputs(&adapter)?
            .into_iter()
            .nth(output_index)
            .ok_or(DesktopError::DisplayNotFound(display_id))?;
        let description = unsafe { output.GetDesc() }.map_err(failed("IDXGIOutput::GetDesc"))?;
        if !description.AttachedToDesktop.as_bool() {
            return Err(DesktopError::DisplayNotFound(display_id));
        }
        let display = describe(display_id, &description);
        let desktop = description.DesktopCoordinates;
        let backend = match Duplication::start(&adapter, &output, options) {
            Ok(duplication) => Backend::Duplication(Box::new(duplication)),
            Err(_) => Backend::Gdi(GdiCapturer::start(desktop, options)?),
        };
        Ok(Self {
            backend,
            display,
            desktop,
        })
    }

    pub fn display(&self) -> DisplayInfo {
        self.display
    }

    pub fn method(&self) -> &'static str {
        match self.backend {
            Backend::Duplication(_) => "DXGI Desktop Duplication",
            Backend::Gdi(_) => "GDI",
        }
    }

    pub fn input(&self) -> Result<InputInjector, DesktopError> {
        Ok(InputInjector::new(
            Box::new(WindowsInput::new(self.desktop)),
            self.display.width,
            self.display.height,
        ))
    }

    pub fn next_frame(&mut self, timeout: Duration) -> Result<Option<CapturedFrame>, DesktopError> {
        match &mut self.backend {
            Backend::Gdi(gdi) => gdi.next_frame(timeout),
            Backend::Duplication(duplication) => match duplication.next_frame(timeout) {
                Ok(frame) => Ok(frame),
                Err(error) => match GdiCapturer::start(self.desktop, duplication.options) {
                    Ok(gdi) => {
                        self.backend = Backend::Gdi(gdi);
                        self.next_frame(timeout)
                    }
                    Err(_) => Err(error),
                },
            },
        }
    }
}

struct Duplication {
    device: ID3D11Device,
    context: ID3D11DeviceContext,
    output: IDXGIOutput1,
    duplication: Option<IDXGIOutputDuplication>,
    staging: Option<(ID3D11Texture2D, u32, u32)>,
    options: CaptureOptions,
    pointer: Pointer,
    clean_frame: Option<CapturedFrame>,
    last_delivery: Option<Instant>,
}

impl Duplication {
    fn start(
        adapter: &IDXGIAdapter1,
        output: &IDXGIOutput,
        options: CaptureOptions,
    ) -> Result<Self, DesktopError> {
        let _dpi = DpiAwareness::per_monitor();
        let output: IDXGIOutput1 = output.cast().map_err(failed("IDXGIOutput1"))?;

        let mut device = None;
        let mut context = None;
        unsafe {
            D3D11CreateDevice(
                adapter,
                D3D_DRIVER_TYPE_UNKNOWN,
                HMODULE::default(),
                D3D11_CREATE_DEVICE_BGRA_SUPPORT,
                None,
                D3D11_SDK_VERSION,
                Some(&mut device),
                None,
                Some(&mut context),
            )
        }
        .map_err(failed("D3D11CreateDevice"))?;
        let (Some(device), Some(context)) = (device, context) else {
            return Err(DesktopError::Platform {
                code: 0,
                message: "Direct3D 11 device was not created".to_owned(),
            });
        };

        let mut capturer = Self {
            device,
            context,
            output,
            duplication: None,
            staging: None,
            options,
            pointer: Pointer {
                shape: None,
                left: 0,
                top: 0,
                visible: false,
            },
            clean_frame: None,
            last_delivery: None,
        };
        capturer.ensure_duplication()?;
        if capturer.duplication.is_none() {
            return Err(DesktopError::Platform {
                code: i64::from(E_ACCESSDENIED.0),
                message: "the desktop cannot be duplicated right now".to_owned(),
            });
        }
        Ok(capturer)
    }

    fn ensure_duplication(&mut self) -> Result<(), DesktopError> {
        if self.duplication.is_some() {
            return Ok(());
        }
        let _dpi = DpiAwareness::per_monitor();
        match unsafe { self.output.DuplicateOutput(&self.device) } {
            Ok(duplication) => {
                self.duplication = Some(duplication);
                Ok(())
            }
            Err(error)
                if error.code() == E_ACCESSDENIED
                    || error.code() == DXGI_ERROR_NOT_CURRENTLY_AVAILABLE
                    || error.code() == DXGI_ERROR_SESSION_DISCONNECTED =>
            {
                Ok(())
            }
            Err(error) => Err(failed("DuplicateOutput")(error)),
        }
    }

    fn reset(&mut self) {
        self.duplication = None;
        self.staging = None;
    }

    fn next_frame(&mut self, timeout: Duration) -> Result<Option<CapturedFrame>, DesktopError> {
        let deadline = Instant::now() + timeout;
        if let Some(last) = self.last_delivery {
            let interval = Duration::from_secs(1) / self.options.max_frame_rate;
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
        loop {
            let now = Instant::now();
            if now >= deadline {
                return Ok(None);
            }
            self.ensure_duplication()?;
            let Some(duplication) = self.duplication.clone() else {
                std::thread::sleep(RETRY_PAUSE.min(deadline - now));
                continue;
            };
            let wait_ms = (deadline - now).as_millis().min(u128::from(u32::MAX)) as u32;
            let mut info = DXGI_OUTDUPL_FRAME_INFO::default();
            let mut resource: Option<IDXGIResource> = None;
            match unsafe { duplication.AcquireNextFrame(wait_ms, &mut info, &mut resource) } {
                Ok(()) => {}
                Err(error) if error.code() == DXGI_ERROR_WAIT_TIMEOUT => return Ok(None),
                Err(error) if error.code() == DXGI_ERROR_ACCESS_LOST => {
                    self.reset();
                    continue;
                }
                Err(error) => return Err(failed("AcquireNextFrame")(error)),
            }
            let acquired = AcquiredFrame(&duplication);
            let pointer_changed = self.update_pointer(&duplication, &info)?;
            let image_changed = info.LastPresentTime != 0;
            if image_changed {
                let Some(resource) = resource else {
                    continue;
                };
                match self.copy_desktop(&duplication, &resource) {
                    Ok(frame) => self.clean_frame = Some(frame),
                    Err(error) => {
                        drop(acquired);
                        self.reset();
                        return Err(error);
                    }
                }
            }
            drop(acquired);
            let cursor_matters = self.options.show_cursor && pointer_changed;
            if (image_changed || cursor_matters)
                && let Some(frame) = self.compose()
            {
                self.last_delivery = Some(Instant::now());
                return Ok(Some(frame));
            }
        }
    }

    fn update_pointer(
        &mut self,
        duplication: &IDXGIOutputDuplication,
        info: &DXGI_OUTDUPL_FRAME_INFO,
    ) -> Result<bool, DesktopError> {
        let mut changed = false;
        if info.LastMouseUpdateTime != 0 {
            let position = info.PointerPosition;
            self.pointer.visible = position.Visible.as_bool();
            self.pointer.left = position.Position.x;
            self.pointer.top = position.Position.y;
            changed = true;
        }
        if info.PointerShapeBufferSize > 0 {
            let mut data = vec![0u8; info.PointerShapeBufferSize as usize];
            let mut required = 0u32;
            let mut shape_info = DXGI_OUTDUPL_POINTER_SHAPE_INFO::default();
            unsafe {
                duplication.GetFramePointerShape(
                    data.len() as u32,
                    data.as_mut_ptr().cast(),
                    &mut required,
                    &mut shape_info,
                )
            }
            .map_err(failed("GetFramePointerShape"))?;
            data.truncate(required as usize);
            let kind = match shape_info.Type as i32 {
                kind if kind == DXGI_OUTDUPL_POINTER_SHAPE_TYPE_MONOCHROME.0 => {
                    Some(PointerKind::Monochrome)
                }
                kind if kind == DXGI_OUTDUPL_POINTER_SHAPE_TYPE_COLOR.0 => Some(PointerKind::Color),
                kind if kind == DXGI_OUTDUPL_POINTER_SHAPE_TYPE_MASKED_COLOR.0 => {
                    Some(PointerKind::MaskedColor)
                }
                _ => None,
            };
            self.pointer.shape = kind.map(|kind| PointerShape {
                kind,
                width: shape_info.Width,
                height: shape_info.Height,
                pitch: shape_info.Pitch as usize,
                data,
            });
            changed = true;
        }
        Ok(changed)
    }

    fn staging_texture(
        &mut self,
        width: u32,
        height: u32,
    ) -> Result<ID3D11Texture2D, DesktopError> {
        if let Some((texture, staged_width, staged_height)) = &self.staging
            && *staged_width == width
            && *staged_height == height
        {
            return Ok(texture.clone());
        }
        let description = D3D11_TEXTURE2D_DESC {
            Width: width,
            Height: height,
            MipLevels: 1,
            ArraySize: 1,
            Format: DXGI_FORMAT_B8G8R8A8_UNORM,
            SampleDesc: DXGI_SAMPLE_DESC {
                Count: 1,
                Quality: 0,
            },
            Usage: D3D11_USAGE_STAGING,
            BindFlags: 0,
            CPUAccessFlags: D3D11_CPU_ACCESS_READ.0 as u32,
            MiscFlags: 0,
        };
        let mut texture = None;
        unsafe {
            self.device
                .CreateTexture2D(&description, None, Some(&mut texture))
        }
        .map_err(failed("CreateTexture2D"))?;
        let texture = texture.ok_or(DesktopError::Platform {
            code: 0,
            message: "staging texture was not created".to_owned(),
        })?;
        self.staging = Some((texture.clone(), width, height));
        Ok(texture)
    }

    fn copy_desktop(
        &mut self,
        duplication: &IDXGIOutputDuplication,
        resource: &IDXGIResource,
    ) -> Result<CapturedFrame, DesktopError> {
        let desktop: ID3D11Texture2D = resource.cast().map_err(failed("ID3D11Texture2D"))?;
        let mut description = D3D11_TEXTURE2D_DESC::default();
        unsafe { desktop.GetDesc(&mut description) };
        if description.Format != DXGI_FORMAT_B8G8R8A8_UNORM {
            return Err(DesktopError::Platform {
                code: i64::from(description.Format.0),
                message: "unexpected desktop pixel format".to_owned(),
            });
        }
        let staging = self.staging_texture(description.Width, description.Height)?;
        unsafe { self.context.CopyResource(&staging, &desktop) };
        let mut mapped = D3D11_MAPPED_SUBRESOURCE::default();
        unsafe {
            self.context
                .Map(&staging, 0, D3D11_MAP_READ, 0, Some(&mut mapped))
        }
        .map_err(failed("ID3D11DeviceContext::Map"))?;
        let rotation = rotation_of(unsafe { duplication.GetDesc() }.Rotation);
        let stride = mapped.RowPitch as usize;
        let length = stride * description.Height as usize;
        let frame = if mapped.pData.is_null() {
            None
        } else {
            let source = unsafe {
                std::slice::from_raw_parts(mapped.pData.cast::<u8>().cast_const(), length)
            };
            rotate_into_frame(
                source,
                description.Width,
                description.Height,
                stride,
                rotation,
            )
        };
        unsafe { self.context.Unmap(&staging, 0) };
        frame.ok_or(DesktopError::Platform {
            code: 0,
            message: "mapped desktop surface was unreadable".to_owned(),
        })
    }

    fn compose(&self) -> Option<CapturedFrame> {
        let mut frame = self.clean_frame.clone()?;
        if self.options.show_cursor
            && self.pointer.visible
            && let Some(shape) = &self.pointer.shape
        {
            draw_pointer(&mut frame, shape, self.pointer.left, self.pointer.top);
        }
        Some(frame)
    }
}

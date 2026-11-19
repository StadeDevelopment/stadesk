use std::ffi::c_void;
use std::ptr::NonNull;
use std::sync::Arc;
use std::sync::mpsc;
use std::time::Duration;

use block2::RcBlock;
use dispatch2::{DispatchQueue, DispatchQueueAttr, DispatchRetained};
use objc2::rc::Retained;
use objc2::runtime::ProtocolObject;
use objc2::{AllocAnyThread, DefinedClass, define_class, msg_send};
use objc2_core_foundation::CFDictionary;
use objc2_core_graphics::{CGDisplayCopyDisplayMode, CGDisplayMode, CGMainDisplayID};
use objc2_core_media::{CMSampleBuffer, CMTime};
use objc2_core_video::{
    CVPixelBufferGetBaseAddress, CVPixelBufferGetBytesPerRow, CVPixelBufferGetHeight,
    CVPixelBufferGetPixelFormatType, CVPixelBufferGetWidth, CVPixelBufferLockBaseAddress,
    CVPixelBufferLockFlags, CVPixelBufferUnlockBaseAddress, kCVPixelFormatType_32BGRA,
    kCVReturnSuccess,
};
use objc2_foundation::{NSArray, NSError, NSNumber, NSObject, NSObjectProtocol, NSString};
use objc2_screen_capture_kit::{
    SCContentFilter, SCDisplay, SCFrameStatus, SCShareableContent, SCStream, SCStreamConfiguration,
    SCStreamDelegate, SCStreamErrorCode, SCStreamErrorDomain, SCStreamFrameInfoStatus,
    SCStreamOutput, SCStreamOutputType,
};

use crate::error::{DesktopError, Permission};
use crate::frame::{BGRA_BYTES, CaptureOptions, CapturedFrame, DisplayInfo};
use crate::input::InputInjector;
use crate::macos_input::MacInput;
use crate::slot::LatestFrame;

const SYSTEM_TIMEOUT: Duration = Duration::from_secs(10);
const QUEUE_LABEL: &str = "com.stadedevelopment.stadesk.capture";

struct Handoff<T>(T);

unsafe impl<T> Send for Handoff<T> {}

fn translate_error(error: &NSError) -> DesktopError {
    let domain_matches = error
        .domain()
        .isEqualToString(unsafe { SCStreamErrorDomain });
    let code = error.code();
    if domain_matches
        && (code == SCStreamErrorCode::UserDeclined.0
            || code == SCStreamErrorCode::MissingEntitlements.0)
    {
        return DesktopError::PermissionDenied(Permission::ScreenRecording);
    }
    DesktopError::Platform {
        code: code as i64,
        message: error.localizedDescription().to_string(),
    }
}

fn wait<T>(receiver: mpsc::Receiver<T>) -> Result<T, DesktopError> {
    receiver
        .recv_timeout(SYSTEM_TIMEOUT)
        .map_err(|_| DesktopError::Timeout)
}

fn shareable_content() -> Result<Retained<SCShareableContent>, DesktopError> {
    let (sender, receiver) = mpsc::sync_channel(1);
    let handler = RcBlock::new(
        move |content: *mut SCShareableContent, error: *mut NSError| {
            let outcome = match (NonNull::new(content), NonNull::new(error)) {
                (Some(content), _) => Ok(Handoff(unsafe { Retained::retain(content.as_ptr()) })),
                (None, Some(error)) => Err(translate_error(unsafe { error.as_ref() })),
                (None, None) => Err(DesktopError::Platform {
                    code: 0,
                    message: "no shareable content and no error".to_owned(),
                }),
            };
            let _ = sender.try_send(outcome);
        },
    );
    unsafe { SCShareableContent::getShareableContentWithCompletionHandler(&handler) };
    match wait(receiver)? {
        Ok(Handoff(Some(content))) => Ok(content),
        Ok(Handoff(None)) => Err(DesktopError::Platform {
            code: 0,
            message: "shareable content could not be retained".to_owned(),
        }),
        Err(error) => Err(error),
    }
}

fn pixel_size(display: &SCDisplay) -> (u32, u32) {
    let id = unsafe { display.displayID() };
    let mode = CGDisplayCopyDisplayMode(id);
    let width = CGDisplayMode::pixel_width(mode.as_deref());
    let height = CGDisplayMode::pixel_height(mode.as_deref());
    if width > 0 && height > 0 {
        (width as u32, height as u32)
    } else {
        (
            unsafe { display.width() }.max(0) as u32,
            unsafe { display.height() }.max(0) as u32,
        )
    }
}

fn describe(display: &SCDisplay, primary_id: u32) -> DisplayInfo {
    let id = unsafe { display.displayID() };
    let (width, height) = pixel_size(display);
    DisplayInfo {
        id,
        width,
        height,
        primary: id == primary_id,
    }
}

pub fn displays() -> Result<Vec<DisplayInfo>, DesktopError> {
    let content = shareable_content()?;
    let primary_id = CGMainDisplayID();
    let list: Retained<NSArray<SCDisplay>> = unsafe { content.displays() };
    Ok(list
        .iter()
        .map(|display| describe(&display, primary_id))
        .collect())
}

fn frame_is_complete(sample: &CMSampleBuffer) -> bool {
    let Some(attachments) = (unsafe { sample.sample_attachments_array(false) }) else {
        return false;
    };
    if attachments.count() < 1 {
        return false;
    }
    let dictionary = unsafe { attachments.value_at_index(0) }.cast::<CFDictionary>();
    if dictionary.is_null() {
        return false;
    }
    let key: *const NSString = unsafe { SCStreamFrameInfoStatus };
    let value = unsafe { (*dictionary).value(key.cast::<c_void>()) }.cast::<NSNumber>();
    if value.is_null() {
        return false;
    }
    unsafe { &*value }.integerValue() == SCFrameStatus::Complete.0
}

fn copy_frame(sample: &CMSampleBuffer) -> Option<CapturedFrame> {
    let image = unsafe { sample.image_buffer() }?;
    if CVPixelBufferGetPixelFormatType(&image) != kCVPixelFormatType_32BGRA {
        return None;
    }
    if unsafe { CVPixelBufferLockBaseAddress(&image, CVPixelBufferLockFlags::ReadOnly) }
        != kCVReturnSuccess
    {
        return None;
    }
    let width = CVPixelBufferGetWidth(&image);
    let height = CVPixelBufferGetHeight(&image);
    let stride = CVPixelBufferGetBytesPerRow(&image);
    let base = CVPixelBufferGetBaseAddress(&image)
        .cast::<u8>()
        .cast_const();
    let row_bytes = width * BGRA_BYTES;
    let frame = if base.is_null() || stride < row_bytes || width == 0 || height == 0 {
        None
    } else {
        let mut bgra = Vec::with_capacity(row_bytes * height);
        for row in 0..height {
            let line = unsafe { std::slice::from_raw_parts(base.add(row * stride), row_bytes) };
            bgra.extend_from_slice(line);
        }
        Some(CapturedFrame {
            width: width as u32,
            height: height as u32,
            bgra,
        })
    };
    unsafe { CVPixelBufferUnlockBaseAddress(&image, CVPixelBufferLockFlags::ReadOnly) };
    frame
}

struct OutputIvars {
    latest: Arc<LatestFrame>,
}

define_class!(
    #[unsafe(super = NSObject)]
    #[name = "StaDeskCaptureOutput"]
    #[ivars = OutputIvars]
    struct CaptureOutput;

    unsafe impl NSObjectProtocol for CaptureOutput {}

    unsafe impl SCStreamOutput for CaptureOutput {
        #[unsafe(method(stream:didOutputSampleBuffer:ofType:))]
        fn stream_did_output(
            &self,
            _stream: &SCStream,
            sample: &CMSampleBuffer,
            kind: SCStreamOutputType,
        ) {
            if kind != SCStreamOutputType::Screen || !frame_is_complete(sample) {
                return;
            }
            if let Some(frame) = copy_frame(sample) {
                self.ivars().latest.publish(frame);
            }
        }
    }

    unsafe impl SCStreamDelegate for CaptureOutput {
        #[unsafe(method(stream:didStopWithError:))]
        fn stream_did_stop(&self, _stream: &SCStream, error: &NSError) {
            let reason = match translate_error(error) {
                DesktopError::PermissionDenied(permission) => {
                    DesktopError::PermissionDenied(permission)
                }
                _ => DesktopError::Stopped(error.localizedDescription().to_string()),
            };
            self.ivars().latest.fail(reason);
        }
    }
);

impl CaptureOutput {
    fn new(latest: Arc<LatestFrame>) -> Retained<Self> {
        let this = Self::alloc().set_ivars(OutputIvars { latest });
        unsafe { msg_send![super(this), init] }
    }
}

fn completion(sender: mpsc::SyncSender<Result<(), DesktopError>>) -> RcBlock<dyn Fn(*mut NSError)> {
    RcBlock::new(move |error: *mut NSError| {
        let outcome = match NonNull::new(error) {
            Some(error) => Err(translate_error(unsafe { error.as_ref() })),
            None => Ok(()),
        };
        let _ = sender.try_send(outcome);
    })
}

pub struct ScreenCapturer {
    stream: Retained<SCStream>,
    _output: Retained<CaptureOutput>,
    _queue: DispatchRetained<DispatchQueue>,
    latest: Arc<LatestFrame>,
    display: DisplayInfo,
}

impl ScreenCapturer {
    pub fn start(display_id: u32, options: CaptureOptions) -> Result<Self, DesktopError> {
        options.validate()?;
        let content = shareable_content()?;
        let primary_id = CGMainDisplayID();
        let list: Retained<NSArray<SCDisplay>> = unsafe { content.displays() };
        let display = list
            .iter()
            .find(|display| unsafe { display.displayID() } == display_id)
            .ok_or(DesktopError::DisplayNotFound(display_id))?;
        let info = describe(&display, primary_id);

        let filter = unsafe {
            SCContentFilter::initWithDisplay_excludingWindows(
                SCContentFilter::alloc(),
                &display,
                &NSArray::new(),
            )
        };
        let configuration = unsafe { SCStreamConfiguration::new() };
        unsafe {
            configuration.setWidth(info.width as usize);
            configuration.setHeight(info.height as usize);
            configuration.setPixelFormat(kCVPixelFormatType_32BGRA);
            configuration.setShowsCursor(options.show_cursor);
            configuration.setMinimumFrameInterval(CMTime::new(1, options.max_frame_rate as i32));
        }

        let latest = Arc::new(LatestFrame::new());
        let output = CaptureOutput::new(Arc::clone(&latest));
        let queue = DispatchQueue::new(QUEUE_LABEL, DispatchQueueAttr::SERIAL);
        let stream = unsafe {
            SCStream::initWithFilter_configuration_delegate(
                SCStream::alloc(),
                &filter,
                &configuration,
                Some(ProtocolObject::from_ref(&*output)),
            )
        };
        unsafe {
            stream.addStreamOutput_type_sampleHandlerQueue_error(
                ProtocolObject::from_ref(&*output),
                SCStreamOutputType::Screen,
                Some(&queue),
            )
        }
        .map_err(|error| translate_error(&error))?;

        let (sender, receiver) = mpsc::sync_channel(1);
        unsafe { stream.startCaptureWithCompletionHandler(Some(&completion(sender))) };
        wait(receiver)??;

        Ok(Self {
            stream,
            _output: output,
            _queue: queue,
            latest,
            display: info,
        })
    }

    pub fn display(&self) -> DisplayInfo {
        self.display
    }

    pub fn method(&self) -> &'static str {
        "ScreenCaptureKit"
    }

    pub fn input(&self) -> Result<InputInjector, DesktopError> {
        let backend = MacInput::new(self.display.id, self.display.width, self.display.height)?;
        Ok(InputInjector::new(
            Box::new(backend),
            self.display.width,
            self.display.height,
        ))
    }

    pub fn next_frame(&mut self, timeout: Duration) -> Result<Option<CapturedFrame>, DesktopError> {
        self.latest.take(timeout)
    }
}

impl Drop for ScreenCapturer {
    fn drop(&mut self) {
        let (sender, receiver) = mpsc::sync_channel(1);
        unsafe {
            self.stream
                .stopCaptureWithCompletionHandler(Some(&completion(sender)))
        };
        let _ = receiver.recv_timeout(SYSTEM_TIMEOUT);
    }
}

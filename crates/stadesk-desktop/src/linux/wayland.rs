use std::fmt::Display;
use std::os::fd::OwnedFd;
use std::sync::{Arc, Mutex, PoisonError, mpsc};
use std::thread::JoinHandle;
use std::time::Duration;

use ashpd::desktop::remote_desktop::{
    Axis, DeviceType, KeyState, RemoteDesktop, SelectDevicesOptions,
};
use ashpd::desktop::screencast::{
    CursorMode, Screencast, SelectSourcesOptions, SourceType, Stream as PortalStream,
};
use ashpd::desktop::{PersistMode, ResponseError, Session};
use pipewire as pw;
use pw::properties::properties;
use pw::spa;
use pw::spa::param::format::{FormatProperties, MediaSubtype, MediaType};
use pw::spa::param::video::{VideoFormat, VideoInfoRaw};
use pw::spa::pod::{Pod, Value, object, property, serialize::PodSerializer};
use pw::spa::utils::{Direction, Fraction, Rectangle, SpaTypes};

use crate::error::{DesktopError, Permission};
use crate::frame::{BGRA_BYTES, CaptureOptions, CapturedFrame, DisplayInfo};
use crate::input::{InputBackend, InputEvent, InputInjector, PointerButton, WHEEL_STEP};
use crate::keymap;
use crate::slot::LatestFrame;
use tokio::sync::mpsc::{UnboundedReceiver, UnboundedSender, unbounded_channel};

const STARTUP_TIMEOUT: Duration = Duration::from_secs(120);
const MAX_SIZE: u32 = 16_384;
const WAYLAND_DISPLAY_ID: u32 = 0;

fn portal_error(error: ashpd::Error) -> DesktopError {
    match error {
        ashpd::Error::Response(ResponseError::Cancelled) => {
            DesktopError::PermissionDenied(Permission::ScreenRecording)
        }
        other => DesktopError::Platform {
            code: 0,
            message: format!("Wayland portal: {other}"),
        },
    }
}

fn pipewire_error(error: impl Display) -> DesktopError {
    DesktopError::Platform {
        code: 0,
        message: format!("PipeWire: {error}"),
    }
}

pub(crate) fn displays() -> Vec<DisplayInfo> {
    vec![DisplayInfo {
        id: WAYLAND_DISPLAY_ID,
        width: 0,
        height: 0,
        primary: true,
    }]
}

const BUTTON_LEFT: i32 = 0x110;
const BUTTON_RIGHT: i32 = 0x111;
const BUTTON_MIDDLE: i32 = 0x112;
const BUTTON_SIDE: i32 = 0x113;
const BUTTON_EXTRA: i32 = 0x114;

enum PortalSession {
    ScreenOnly(Session<Screencast>),
    Remote {
        proxy: Arc<RemoteDesktop>,
        session: Arc<Session<RemoteDesktop>>,
        pointer: bool,
        keyboard: bool,
    },
}

impl PortalSession {
    async fn close(&self) {
        let _ = match self {
            Self::ScreenOnly(session) => session.close().await,
            Self::Remote { session, .. } => session.close().await,
        };
    }
}

struct Portal {
    _screencast: Screencast,
    session: PortalSession,
    node_id: u32,
    remote: OwnedFd,
    width: u32,
    height: u32,
}

fn source_options(cursor_mode: CursorMode) -> SelectSourcesOptions {
    SelectSourcesOptions::default()
        .set_cursor_mode(cursor_mode)
        .set_sources(SourceType::MONITOR)
        .set_multiple(false)
        .set_persist_mode(PersistMode::DoNot)
}

fn first_stream(streams: &[PortalStream]) -> Result<PortalStream, DesktopError> {
    streams
        .first()
        .cloned()
        .ok_or_else(|| DesktopError::Platform {
            code: 0,
            message: "Wayland portal: no screen was shared".to_owned(),
        })
}

async fn remote_desktop() -> Option<RemoteDesktop> {
    let proxy = RemoteDesktop::new().await.ok()?;
    let types = proxy.available_device_types().await.ok()?;
    types
        .contains(DeviceType::KEYBOARD | DeviceType::POINTER)
        .then_some(proxy)
}

async fn open_portal(show_cursor: bool) -> Result<Portal, DesktopError> {
    let screencast = Screencast::new().await.map_err(portal_error)?;
    let available = screencast
        .available_cursor_modes()
        .await
        .unwrap_or(CursorMode::HIDDEN);
    let cursor_mode = if show_cursor && available.contains(CursorMode::EMBEDDED) {
        CursorMode::EMBEDDED
    } else {
        CursorMode::HIDDEN
    };
    let (session, stream) = match remote_desktop().await {
        Some(proxy) => {
            let session = proxy
                .create_session(Default::default())
                .await
                .map_err(portal_error)?;
            proxy
                .select_devices(
                    &session,
                    SelectDevicesOptions::default()
                        .set_devices(DeviceType::KEYBOARD | DeviceType::POINTER)
                        .set_persist_mode(PersistMode::DoNot),
                )
                .await
                .map_err(portal_error)?
                .response()
                .map_err(portal_error)?;
            screencast
                .select_sources(&session, source_options(cursor_mode))
                .await
                .map_err(portal_error)?
                .response()
                .map_err(portal_error)?;
            let started = proxy
                .start(&session, None, Default::default())
                .await
                .map_err(portal_error)?
                .response()
                .map_err(portal_error)?;
            let stream = first_stream(started.streams())?;
            let devices = started.devices();
            (
                PortalSession::Remote {
                    proxy: Arc::new(proxy),
                    session: Arc::new(session),
                    pointer: devices.contains(DeviceType::POINTER),
                    keyboard: devices.contains(DeviceType::KEYBOARD),
                },
                stream,
            )
        }
        None => {
            let session = screencast
                .create_session(Default::default())
                .await
                .map_err(portal_error)?;
            screencast
                .select_sources(&session, source_options(cursor_mode))
                .await
                .map_err(portal_error)?
                .response()
                .map_err(portal_error)?;
            let started = screencast
                .start(&session, None, Default::default())
                .await
                .map_err(portal_error)?
                .response()
                .map_err(portal_error)?;
            (
                PortalSession::ScreenOnly(session),
                first_stream(started.streams())?,
            )
        }
    };
    let remote = match &session {
        PortalSession::ScreenOnly(session) => {
            screencast
                .open_pipe_wire_remote(session, Default::default())
                .await
        }
        PortalSession::Remote { session, .. } => {
            screencast
                .open_pipe_wire_remote(session.as_ref(), Default::default())
                .await
        }
    }
    .map_err(portal_error)?;
    let (width, height) = stream
        .size()
        .map(|(width, height)| (width.max(0) as u32, height.max(0) as u32))
        .unwrap_or((0, 0));
    Ok(Portal {
        _screencast: screencast,
        session,
        node_id: stream.pipe_wire_node_id(),
        remote,
        width,
        height,
    })
}

fn evdev_button(button: PointerButton) -> i32 {
    match button {
        PointerButton::Left => BUTTON_LEFT,
        PointerButton::Right => BUTTON_RIGHT,
        PointerButton::Middle => BUTTON_MIDDLE,
        PointerButton::Back => BUTTON_SIDE,
        PointerButton::Forward => BUTTON_EXTRA,
    }
}

fn key_state(pressed: bool) -> KeyState {
    if pressed {
        KeyState::Pressed
    } else {
        KeyState::Released
    }
}

pub(crate) fn to_stream_coordinate(position: u32, frame_extent: u32, stream_extent: u32) -> f64 {
    let frame_extent = f64::from(frame_extent.max(1));
    let stream_extent = if stream_extent > 0 {
        f64::from(stream_extent)
    } else {
        frame_extent
    };
    (f64::from(position) + 0.5) * stream_extent / frame_extent
}

struct InputRoute {
    proxy: Arc<RemoteDesktop>,
    session: Arc<Session<RemoteDesktop>>,
    stream: u32,
    logical: (u32, u32),
    frame: Arc<Mutex<(u32, u32)>>,
    pointer: bool,
    keyboard: bool,
}

async fn forward_input(route: InputRoute, mut events: UnboundedReceiver<InputEvent>) {
    let mut remainder = (0i32, 0i32);
    while let Some(event) = events.recv().await {
        let proxy = route.proxy.as_ref();
        let session = route.session.as_ref();
        match event {
            InputEvent::PointerMove { x, y } if route.pointer => {
                let (frame_width, frame_height) =
                    *route.frame.lock().unwrap_or_else(PoisonError::into_inner);
                let _ = proxy
                    .notify_pointer_motion_absolute(
                        session,
                        route.stream,
                        to_stream_coordinate(x, frame_width, route.logical.0),
                        to_stream_coordinate(y, frame_height, route.logical.1),
                        Default::default(),
                    )
                    .await;
            }
            InputEvent::PointerButton { button, pressed } if route.pointer => {
                let _ = proxy
                    .notify_pointer_button(
                        session,
                        evdev_button(button),
                        key_state(pressed),
                        Default::default(),
                    )
                    .await;
            }
            InputEvent::Scroll {
                horizontal,
                vertical,
            } if route.pointer => {
                let total_horizontal = remainder.0 + horizontal;
                let total_vertical = remainder.1 + vertical;
                remainder = (total_horizontal % WHEEL_STEP, total_vertical % WHEEL_STEP);
                let steps_vertical = -(total_vertical / WHEEL_STEP);
                let steps_horizontal = total_horizontal / WHEEL_STEP;
                if steps_vertical != 0 {
                    let _ = proxy
                        .notify_pointer_axis_discrete(
                            session,
                            Axis::Vertical,
                            steps_vertical,
                            Default::default(),
                        )
                        .await;
                }
                if steps_horizontal != 0 {
                    let _ = proxy
                        .notify_pointer_axis_discrete(
                            session,
                            Axis::Horizontal,
                            steps_horizontal,
                            Default::default(),
                        )
                        .await;
                }
            }
            InputEvent::Key { usage, pressed } if route.keyboard => {
                if let Some(entry) = keymap::by_usage(usage) {
                    let _ = proxy
                        .notify_keyboard_keycode(
                            session,
                            i32::from(entry.evdev),
                            key_state(pressed),
                            Default::default(),
                        )
                        .await;
                }
            }
            _ => {}
        }
    }
}

struct WaylandInput {
    sender: UnboundedSender<InputEvent>,
}

impl InputBackend for WaylandInput {
    fn apply(&mut self, event: InputEvent) -> Result<(), DesktopError> {
        self.sender.send(event).map_err(|_| {
            DesktopError::Stopped("the Wayland remote desktop session ended".to_owned())
        })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ChannelOrder {
    Bgr,
    Rgb,
}

fn channel_order(format: VideoFormat) -> Option<ChannelOrder> {
    if format == VideoFormat::BGRx || format == VideoFormat::BGRA {
        Some(ChannelOrder::Bgr)
    } else if format == VideoFormat::RGBx || format == VideoFormat::RGBA {
        Some(ChannelOrder::Rgb)
    } else {
        None
    }
}

fn copy_frame(
    data: &[u8],
    offset: usize,
    stride: usize,
    width: u32,
    height: u32,
    order: ChannelOrder,
) -> Option<CapturedFrame> {
    let row_bytes = width as usize * BGRA_BYTES;
    let rows = height as usize;
    if width == 0 || height == 0 || stride < row_bytes {
        return None;
    }
    let end = offset
        .checked_add(stride.checked_mul(rows - 1)?)?
        .checked_add(row_bytes)?;
    if data.len() < end {
        return None;
    }
    let mut bgra = Vec::with_capacity(row_bytes * rows);
    for row in 0..rows {
        let start = offset + row * stride;
        bgra.extend_from_slice(&data[start..start + row_bytes]);
    }
    for pixel in bgra.as_chunks_mut::<BGRA_BYTES>().0 {
        if order == ChannelOrder::Rgb {
            pixel.swap(0, 2);
        }
        pixel[3] = 0xff;
    }
    Some(CapturedFrame {
        width,
        height,
        bgra,
    })
}

fn format_parameters(
    options: &CaptureOptions,
    width: u32,
    height: u32,
) -> Result<Vec<u8>, DesktopError> {
    let preferred = Rectangle {
        width: if width > 0 { width } else { 1920 },
        height: if height > 0 { height } else { 1080 },
    };
    let object = object!(
        SpaTypes::ObjectParamFormat,
        spa::param::ParamType::EnumFormat,
        property!(FormatProperties::MediaType, Id, MediaType::Video),
        property!(FormatProperties::MediaSubtype, Id, MediaSubtype::Raw),
        property!(
            FormatProperties::VideoFormat,
            Choice,
            Enum,
            Id,
            VideoFormat::BGRx,
            VideoFormat::BGRx,
            VideoFormat::BGRA,
            VideoFormat::RGBx,
            VideoFormat::RGBA
        ),
        property!(
            FormatProperties::VideoSize,
            Choice,
            Range,
            Rectangle,
            preferred,
            Rectangle {
                width: 1,
                height: 1
            },
            Rectangle {
                width: MAX_SIZE,
                height: MAX_SIZE
            }
        ),
        property!(
            FormatProperties::VideoFramerate,
            Choice,
            Range,
            Fraction,
            Fraction {
                num: options.max_frame_rate,
                denom: 1
            },
            Fraction { num: 0, denom: 1 },
            Fraction {
                num: options.max_frame_rate,
                denom: 1
            }
        ),
    );
    PodSerializer::serialize(std::io::Cursor::new(Vec::new()), &Value::Object(object))
        .map(|(cursor, _)| cursor.into_inner())
        .map_err(pipewire_error)
}

struct Started {
    display: DisplayInfo,
    input: Option<UnboundedSender<InputEvent>>,
}

struct Links {
    startup: mpsc::SyncSender<Result<Started, DesktopError>>,
    latest: Arc<LatestFrame>,
    stop: pw::channel::Receiver<()>,
}

struct StreamState {
    format: VideoInfoRaw,
    order: Option<ChannelOrder>,
    latest: Arc<LatestFrame>,
    frame_size: Arc<Mutex<(u32, u32)>>,
}

fn run_capture(options: CaptureOptions, links: Links) {
    let runtime = match tokio::runtime::Builder::new_multi_thread()
        .worker_threads(1)
        .enable_all()
        .build()
    {
        Ok(runtime) => runtime,
        Err(error) => {
            let _ = links.startup.send(Err(pipewire_error(error)));
            return;
        }
    };
    let mut portal = match runtime.block_on(open_portal(options.show_cursor)) {
        Ok(portal) => portal,
        Err(error) => {
            let _ = links.startup.send(Err(error));
            return;
        }
    };
    let frame_size = Arc::new(Mutex::new((portal.width, portal.height)));
    let input = match &portal.session {
        PortalSession::Remote {
            proxy,
            session,
            pointer,
            keyboard,
        } => {
            let (sender, receiver) = unbounded_channel();
            let route = InputRoute {
                proxy: Arc::clone(proxy),
                session: Arc::clone(session),
                stream: portal.node_id,
                logical: (portal.width, portal.height),
                frame: Arc::clone(&frame_size),
                pointer: *pointer,
                keyboard: *keyboard,
            };
            runtime.spawn(forward_input(route, receiver));
            Some(sender)
        }
        PortalSession::ScreenOnly(_) => None,
    };
    let startup = links.startup.clone();
    if let Err(error) = stream_frames(&options, &mut portal, links, frame_size, input) {
        let _ = startup.send(Err(error));
    }
    runtime.block_on(portal.session.close());
    drop(portal);
}

fn stream_frames(
    options: &CaptureOptions,
    portal: &mut Portal,
    links: Links,
    frame_size: Arc<Mutex<(u32, u32)>>,
    input: Option<UnboundedSender<InputEvent>>,
) -> Result<(), DesktopError> {
    let Links {
        startup,
        latest,
        stop,
    } = links;
    let remote = portal.remote.try_clone().map_err(pipewire_error)?;
    let (node_id, width, height) = (portal.node_id, portal.width, portal.height);
    pw::init();
    let main_loop = pw::main_loop::MainLoopRc::new(None).map_err(pipewire_error)?;
    let context = pw::context::ContextRc::new(&main_loop, None).map_err(pipewire_error)?;
    let core = context
        .connect_fd_rc(remote, None)
        .map_err(pipewire_error)?;
    let stream = pw::stream::StreamRc::new(
        core,
        "stadesk-capture",
        properties! {
            *pw::keys::MEDIA_TYPE => "Video",
            *pw::keys::MEDIA_CATEGORY => "Capture",
            *pw::keys::MEDIA_ROLE => "Screen",
        },
    )
    .map_err(pipewire_error)?;

    let failure_target = Arc::clone(&latest);
    let state = StreamState {
        format: VideoInfoRaw::new(),
        order: None,
        latest,
        frame_size,
    };
    let _listener = stream
        .add_local_listener_with_user_data(state)
        .state_changed(move |_, _, _, new| {
            if let pw::stream::StreamState::Error(message) = new {
                failure_target.fail(DesktopError::Stopped(message));
            }
        })
        .param_changed(|_, state, id, parameter| {
            let Some(parameter) = parameter else {
                return;
            };
            if id != spa::param::ParamType::Format.as_raw() {
                return;
            }
            let Ok((media_type, media_subtype)) = spa::param::format_utils::parse_format(parameter)
            else {
                return;
            };
            if media_type != MediaType::Video || media_subtype != MediaSubtype::Raw {
                return;
            }
            if state.format.parse(parameter).is_ok() {
                state.order = channel_order(state.format.format());
                let size = state.format.size();
                *state
                    .frame_size
                    .lock()
                    .unwrap_or_else(PoisonError::into_inner) = (size.width, size.height);
            }
        })
        .process(|stream, state| {
            let Some(order) = state.order else {
                return;
            };
            let Some(mut buffer) = stream.dequeue_buffer() else {
                return;
            };
            let size = state.format.size();
            let Some(data) = buffer.datas_mut().first_mut() else {
                return;
            };
            let chunk = data.chunk();
            let offset = chunk.offset() as usize;
            let stride = usize::try_from(chunk.stride()).unwrap_or(0);
            let filled = chunk.size() as usize;
            if filled == 0 {
                return;
            }
            let Some(bytes) = data.data() else {
                return;
            };
            if let Some(frame) = copy_frame(bytes, offset, stride, size.width, size.height, order) {
                state.latest.publish(frame);
            }
        })
        .register()
        .map_err(pipewire_error)?;

    let parameters = format_parameters(options, width, height)?;
    let pod =
        Pod::from_bytes(&parameters).ok_or_else(|| pipewire_error("invalid format parameters"))?;
    stream
        .connect(
            Direction::Input,
            Some(node_id),
            pw::stream::StreamFlags::AUTOCONNECT | pw::stream::StreamFlags::MAP_BUFFERS,
            &mut [pod],
        )
        .map_err(pipewire_error)?;

    let quit_target = main_loop.clone();
    let _stop = stop.attach(main_loop.loop_(), move |()| quit_target.quit());
    let _ = startup.send(Ok(Started {
        display: DisplayInfo {
            id: WAYLAND_DISPLAY_ID,
            width,
            height,
            primary: true,
        },
        input,
    }));
    main_loop.run();
    Ok(())
}

pub(crate) struct WaylandCapturer {
    latest: Arc<LatestFrame>,
    stop: pw::channel::Sender<()>,
    worker: Option<JoinHandle<()>>,
    display: DisplayInfo,
    input: Option<UnboundedSender<InputEvent>>,
}

impl WaylandCapturer {
    pub(crate) fn start(display_id: u32, options: CaptureOptions) -> Result<Self, DesktopError> {
        options.validate()?;
        if display_id != WAYLAND_DISPLAY_ID {
            return Err(DesktopError::DisplayNotFound(display_id));
        }
        let latest = Arc::new(LatestFrame::new());
        let (stop, stop_receiver) = pw::channel::channel();
        let (startup, startup_receiver) = mpsc::sync_channel(1);
        let worker_latest = Arc::clone(&latest);
        let worker = std::thread::Builder::new()
            .name("stadesk-wayland-capture".to_owned())
            .spawn(move || {
                run_capture(
                    options,
                    Links {
                        startup,
                        latest: worker_latest,
                        stop: stop_receiver,
                    },
                )
            })
            .map_err(pipewire_error)?;
        let started = match startup_receiver.recv_timeout(STARTUP_TIMEOUT) {
            Ok(Ok(started)) => started,
            Ok(Err(error)) => {
                let _ = worker.join();
                return Err(error);
            }
            Err(_) => {
                let _ = stop.send(());
                return Err(DesktopError::Timeout);
            }
        };
        Ok(Self {
            latest,
            stop,
            worker: Some(worker),
            display: started.display,
            input: started.input,
        })
    }

    pub(crate) fn display(&self) -> DisplayInfo {
        self.display
    }

    pub(crate) fn method(&self) -> &'static str {
        if self.input.is_some() {
            "Wayland (RemoteDesktop portal + PipeWire)"
        } else {
            "Wayland (ScreenCast portal + PipeWire)"
        }
    }

    pub(crate) fn input(&self) -> Result<InputInjector, DesktopError> {
        let sender = self.input.clone().ok_or(DesktopError::Unsupported(
            "this Wayland desktop does not offer remote control through the RemoteDesktop portal",
        ))?;
        Ok(InputInjector::new(
            Box::new(WaylandInput { sender }),
            self.display.width,
            self.display.height,
        ))
    }

    pub(crate) fn next_frame(
        &mut self,
        timeout: Duration,
    ) -> Result<Option<CapturedFrame>, DesktopError> {
        let frame = self.latest.take(timeout)?;
        if let Some(frame) = &frame
            && (self.display.width, self.display.height) != (frame.width, frame.height)
        {
            self.display.width = frame.width;
            self.display.height = frame.height;
        }
        Ok(frame)
    }
}

impl Drop for WaylandCapturer {
    fn drop(&mut self) {
        let _ = self.stop.send(());
        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bgr_buffers_are_copied_with_padding_removed_and_alpha_set() {
        let mut data = vec![0u8; 3 + 2 * 12];
        data[3..11].copy_from_slice(&[1, 2, 3, 0, 4, 5, 6, 0]);
        data[15..23].copy_from_slice(&[7, 8, 9, 0, 10, 11, 12, 0]);
        let frame = copy_frame(&data, 3, 12, 2, 2, ChannelOrder::Bgr).unwrap();
        assert_eq!(
            frame.bgra,
            [1, 2, 3, 255, 4, 5, 6, 255, 7, 8, 9, 255, 10, 11, 12, 255]
        );
    }

    #[test]
    fn rgb_buffers_are_swapped_into_bgra() {
        let frame = copy_frame(&[10, 20, 30, 0], 0, 4, 1, 1, ChannelOrder::Rgb).unwrap();
        assert_eq!(frame.bgra, [30, 20, 10, 255]);
    }

    #[test]
    fn short_or_inconsistent_buffers_are_rejected() {
        assert!(copy_frame(&[0u8; 15], 0, 8, 2, 2, ChannelOrder::Bgr).is_none());
        assert!(copy_frame(&[0u8; 16], 0, 7, 2, 2, ChannelOrder::Bgr).is_none());
        assert!(copy_frame(&[0u8; 16], 1, 8, 2, 2, ChannelOrder::Bgr).is_none());
        assert!(copy_frame(&[], 0, 0, 0, 0, ChannelOrder::Bgr).is_none());
    }

    #[test]
    fn stream_coordinates_scale_from_frame_pixels_to_logical_size() {
        assert_eq!(to_stream_coordinate(0, 2560, 1280), 0.25);
        assert_eq!(to_stream_coordinate(2559, 2560, 1280), 1279.75);
        assert_eq!(to_stream_coordinate(10, 100, 0), 10.5);
        assert_eq!(to_stream_coordinate(10, 0, 0), 10.5);
    }

    #[test]
    fn only_32_bit_rgb_formats_are_accepted() {
        assert_eq!(channel_order(VideoFormat::BGRx), Some(ChannelOrder::Bgr));
        assert_eq!(channel_order(VideoFormat::BGRA), Some(ChannelOrder::Bgr));
        assert_eq!(channel_order(VideoFormat::RGBx), Some(ChannelOrder::Rgb));
        assert_eq!(channel_order(VideoFormat::RGBA), Some(ChannelOrder::Rgb));
        assert_eq!(channel_order(VideoFormat::I420), None);
    }
}

use std::error::Error;
use std::net::SocketAddr;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use stadesk_core::{
    AccessGate, AccessMethods, AccessResponse, Identity, PermanentPassword, PublicIdentity,
    StaDeskId,
};
use stadesk_desktop::{CapturedFrame, DesktopError, InputEvent, PointerButton};
use stadesk_net::{ServerAddress, ServerCertificate, bind_controller_endpoint, bind_host_endpoint};
use stadesk_server::RunningServer;
use stadesk_session::{
    Approval, ByeReason, Delivery, DesktopProvider, FrameSource, HostDelegate, HostService,
    HostSettings, HostStatus, InputSink, OpenedDesktop, Path, Route, SessionError, StreamEvent,
    ViewerSession, connect,
};
use tokio::time::timeout;

type TestResult<T = ()> = Result<T, Box<dyn Error + Send + Sync>>;

const WIDTH: u32 = 320;
const HEIGHT: u32 = 240;
const PASSWORD: &str = "correct horse";
const WAIT: Duration = Duration::from_secs(20);

#[derive(Debug, Clone, PartialEq, Eq)]
enum Recorded {
    Resize(u32, u32),
    Event(InputEvent),
}

struct SyntheticFrames {
    tick: u32,
}

impl FrameSource for SyntheticFrames {
    fn next_frame(&mut self, _timeout: Duration) -> Result<Option<CapturedFrame>, DesktopError> {
        std::thread::sleep(Duration::from_millis(15));
        self.tick += 1;
        let mut bgra = Vec::with_capacity((WIDTH * HEIGHT * 4) as usize);
        for y in 0..HEIGHT {
            for x in 0..WIDTH {
                bgra.extend_from_slice(&[
                    ((x + self.tick) % 256) as u8,
                    (y % 256) as u8,
                    ((x + y) % 256) as u8,
                    255,
                ]);
            }
        }
        Ok(Some(CapturedFrame {
            width: WIDTH,
            height: HEIGHT,
            bgra,
        }))
    }
}

struct RecordingSink(Arc<Mutex<Vec<Recorded>>>);

impl InputSink for RecordingSink {
    fn inject(&mut self, event: InputEvent) -> Result<(), DesktopError> {
        self.0.lock().unwrap().push(Recorded::Event(event));
        Ok(())
    }

    fn resize(&mut self, width: u32, height: u32) {
        self.0.lock().unwrap().push(Recorded::Resize(width, height));
    }
}

struct SyntheticDesktop {
    recorded: Arc<Mutex<Vec<Recorded>>>,
    fail: bool,
}

impl DesktopProvider for SyntheticDesktop {
    fn open(&self) -> Result<OpenedDesktop, DesktopError> {
        if self.fail {
            return Err(DesktopError::PermissionDenied(
                stadesk_desktop::Permission::ScreenRecording,
            ));
        }
        Ok(OpenedDesktop {
            frames: Box::new(SyntheticFrames { tick: 0 }),
            input: Ok(Box::new(RecordingSink(Arc::clone(&self.recorded)))),
        })
    }
}

struct TestDelegate {
    approve: bool,
    asked: Arc<AtomicBool>,
}

impl HostDelegate for TestDelegate {
    fn approve(&self, _peer: &PublicIdentity) -> Approval {
        self.asked.store(true, Ordering::SeqCst);
        let answer = self.approve;
        Box::pin(async move { answer })
    }
}

struct Harness {
    server: RunningServer,
    server_address: ServerAddress,
    _host: HostService,
    host_id: StaDeskId,
    recorded: Arc<Mutex<Vec<Recorded>>>,
    asked: Arc<AtomicBool>,
    controller: Identity,
}

fn localhost() -> SocketAddr {
    "127.0.0.1:0".parse().unwrap()
}

async fn harness(methods: AccessMethods, approve: bool, fail_capture: bool) -> TestResult<Harness> {
    let certificate = ServerCertificate::generate()?;
    let server = RunningServer::start(localhost(), &certificate)?;
    let server_address = ServerAddress {
        address: server.local_address(),
        fingerprint: server.fingerprint(),
    };
    let mut gate = AccessGate::new(methods)?;
    gate.set_permanent_password(Some(PermanentPassword::from_plaintext(PASSWORD)?));
    let recorded = Arc::new(Mutex::new(Vec::new()));
    let asked = Arc::new(AtomicBool::new(false));
    let settings = HostSettings {
        gate: Arc::new(Mutex::new(gate)),
        provider: Arc::new(SyntheticDesktop {
            recorded: Arc::clone(&recorded),
            fail: fail_capture,
        }),
        delegate: Arc::new(TestDelegate {
            approve,
            asked: Arc::clone(&asked),
        }),
    };
    let host_identity = Arc::new(Identity::generate()?);
    let host_id = host_identity.id();
    let host = HostService::start(
        bind_host_endpoint(localhost())?,
        server_address,
        host_identity,
        Arc::new(settings),
    );
    let mut status = host.status();
    timeout(WAIT, async {
        loop {
            if matches!(*status.borrow(), HostStatus::Online { .. }) {
                return;
            }
            status.changed().await.unwrap();
        }
    })
    .await?;
    Ok(Harness {
        server,
        server_address,
        _host: host,
        host_id,
        recorded,
        asked,
        controller: Identity::generate()?,
    })
}

fn password_only() -> AccessMethods {
    AccessMethods {
        accept_prompt: false,
        one_time_password: false,
        permanent_password: true,
        trusted_devices: false,
    }
}

fn prompt_only() -> AccessMethods {
    AccessMethods {
        accept_prompt: true,
        one_time_password: false,
        permanent_password: false,
        trusted_devices: false,
    }
}

async fn open_viewer(
    harness: &Harness,
    route: Route,
    password: Option<&str>,
) -> TestResult<(Result<ViewerSession, SessionError>, Path)> {
    let endpoint = bind_controller_endpoint(localhost())?;
    let (channel, path) = connect(
        &endpoint,
        &harness.server_address,
        &harness.controller,
        harness.host_id,
        route,
    )
    .await?;
    Ok((ViewerSession::start(channel, password, || {}).await, path))
}

async fn wait_for_frames(viewer: &ViewerSession, count: u64) -> TestResult<(u32, u32)> {
    let mut frames = viewer.frames();
    let deadline = Instant::now() + WAIT;
    loop {
        if let Some(frame) = frames.borrow().clone()
            && frame.sequence >= count
        {
            return Ok((frame.image.width(), frame.image.height()));
        }
        let remaining = deadline.saturating_duration_since(Instant::now());
        timeout(remaining, frames.changed()).await??;
    }
}

async fn wait_for_events(
    recorded: &Arc<Mutex<Vec<Recorded>>>,
    count: usize,
) -> TestResult<Vec<Recorded>> {
    let deadline = Instant::now() + WAIT;
    loop {
        let snapshot = recorded.lock().unwrap().clone();
        if snapshot
            .iter()
            .filter(|entry| matches!(entry, Recorded::Event(_)))
            .count()
            >= count
        {
            return Ok(snapshot);
        }
        if Instant::now() > deadline {
            return Err(format!("only recorded {snapshot:?}").into());
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn password_session_streams_video_and_delivers_input_over_direct_path() -> TestResult {
    let harness = harness(password_only(), false, false).await?;
    let (viewer, path) = open_viewer(&harness, Route::Automatic, Some(PASSWORD)).await?;
    let viewer = viewer?;
    assert_eq!(path, Path::Direct);
    assert_eq!(viewer.peer().id(), harness.host_id);
    assert_eq!(wait_for_frames(&viewer, 10).await?, (WIDTH, HEIGHT));
    let events = [
        InputEvent::PointerMove { x: 100, y: 50 },
        InputEvent::PointerButton {
            button: PointerButton::Left,
            pressed: true,
        },
        InputEvent::PointerButton {
            button: PointerButton::Left,
            pressed: false,
        },
        InputEvent::Key {
            usage: 0x04,
            pressed: true,
        },
    ];
    for event in events {
        viewer.send_input(event)?;
    }
    let recorded = wait_for_events(&harness.recorded, events.len()).await?;
    assert_eq!(recorded[0], Recorded::Resize(WIDTH, HEIGHT));
    let delivered: Vec<InputEvent> = recorded
        .iter()
        .filter_map(|entry| match entry {
            Recorded::Event(event) => Some(*event),
            Recorded::Resize(..) => None,
        })
        .collect();
    assert_eq!(delivered, events);
    assert_eq!(viewer.close().await?, ByeReason::Normal);
    assert!(!harness.asked.load(Ordering::SeqCst));
    harness.server.shutdown().await;
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn relay_only_route_streams_video() -> TestResult {
    let harness = harness(password_only(), false, false).await?;
    let (viewer, path) = open_viewer(&harness, Route::RelayOnly, Some(PASSWORD)).await?;
    let viewer = viewer?;
    assert_eq!(path, Path::Relay);
    assert_eq!(wait_for_frames(&viewer, 5).await?, (WIDTH, HEIGHT));
    viewer.close().await?;
    harness.server.shutdown().await;
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn approved_prompt_grants_access() -> TestResult {
    let harness = harness(prompt_only(), true, false).await?;
    let endpoint = bind_controller_endpoint(localhost())?;
    let (channel, _) = connect(
        &endpoint,
        &harness.server_address,
        &harness.controller,
        harness.host_id,
        Route::Automatic,
    )
    .await?;
    let pending = Arc::new(AtomicBool::new(false));
    let flag = Arc::clone(&pending);
    let viewer =
        ViewerSession::start(channel, None, move || flag.store(true, Ordering::SeqCst)).await?;
    assert!(pending.load(Ordering::SeqCst));
    assert!(harness.asked.load(Ordering::SeqCst));
    wait_for_frames(&viewer, 3).await?;
    viewer.close().await?;
    harness.server.shutdown().await;
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn packet_delivery_hands_out_the_encoded_stream() -> TestResult {
    let harness = harness(password_only(), false, false).await?;
    let endpoint = bind_controller_endpoint(localhost())?;
    let (channel, _) = connect(
        &endpoint,
        &harness.server_address,
        &harness.controller,
        harness.host_id,
        Route::Automatic,
    )
    .await?;
    let (packets, mut stream) = tokio::sync::mpsc::unbounded_channel();
    let viewer =
        ViewerSession::start_with(channel, Some(PASSWORD), || {}, Delivery::Packets(packets))
            .await?;
    let first = timeout(WAIT, stream.recv()).await?.ok_or("stream ended")?;
    assert_eq!(
        first,
        StreamEvent::Start {
            width: WIDTH,
            height: HEIGHT
        }
    );
    let mut decoder = stadesk_codec::Vp9Decoder::new(1)?;
    let mut decoded = 0;
    let mut first_packet = true;
    while decoded < 5 {
        match timeout(WAIT, stream.recv()).await?.ok_or("stream ended")? {
            StreamEvent::Packet { keyframe, data, .. } => {
                if first_packet {
                    assert!(keyframe);
                    first_packet = false;
                }
                decoded += decoder.decode(&data)?.len();
            }
            StreamEvent::Start { .. } => return Err("unexpected second start".into()),
        }
    }
    assert!(viewer.frames().borrow().is_none());
    viewer.close().await?;
    harness.server.shutdown().await;
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn rejected_prompt_denies_access() -> TestResult {
    let harness = harness(prompt_only(), false, false).await?;
    let (viewer, _) = open_viewer(&harness, Route::Automatic, None).await?;
    assert!(matches!(
        viewer,
        Err(SessionError::Access(AccessResponse::Denied))
    ));
    harness.server.shutdown().await;
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn wrong_password_is_reported() -> TestResult {
    let harness = harness(password_only(), false, false).await?;
    let (viewer, _) = open_viewer(&harness, Route::Automatic, Some("wrong password")).await?;
    assert!(matches!(
        viewer,
        Err(SessionError::Access(AccessResponse::WrongPassword))
    ));
    harness.server.shutdown().await;
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn capture_failure_ends_the_session_with_a_reason() -> TestResult {
    let harness = harness(password_only(), false, true).await?;
    let (viewer, _) = open_viewer(&harness, Route::Automatic, Some(PASSWORD)).await?;
    assert_eq!(viewer?.wait().await?, ByeReason::CaptureFailed);
    harness.server.shutdown().await;
    Ok(())
}

use std::net::SocketAddr;
use std::process::ExitCode;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use stadesk_core::{
    AccessGate, AccessMethods, Identity, PermanentPassword, PublicIdentity, StaDeskId,
};
use stadesk_desktop::DesktopError;
use stadesk_net::{ServerAddress, ServerCertificate, bind_controller_endpoint, bind_host_endpoint};
use stadesk_server::RunningServer;
use stadesk_session::{
    Approval, HostDelegate, HostService, HostSettings, HostStatus, Route, ScreenProvider,
    ViewerSession, connect,
};

const PASSWORD: &str = "loopback password";
const MEASURE_FOR: Duration = Duration::from_secs(10);

struct Delegate;

impl HostDelegate for Delegate {
    fn approve(&self, _peer: &PublicIdentity) -> Approval {
        Box::pin(async { false })
    }

    fn session_started(&self, peer: StaDeskId, input: Result<(), DesktopError>) {
        match input {
            Ok(()) => println!("host: session with {peer} started, remote input available"),
            Err(error) => println!("host: session with {peer} started, view only: {error}"),
        }
    }
}

async fn run() -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let localhost: SocketAddr = "127.0.0.1:0".parse()?;
    let certificate = ServerCertificate::generate()?;
    let server = RunningServer::start(localhost, &certificate)?;
    let server_address = ServerAddress {
        address: server.local_address(),
        fingerprint: server.fingerprint(),
    };
    let mut gate = AccessGate::new(AccessMethods {
        accept_prompt: false,
        one_time_password: false,
        permanent_password: true,
        trusted_devices: false,
    })?;
    gate.set_permanent_password(Some(PermanentPassword::from_plaintext(PASSWORD)?));
    let host_identity = Arc::new(Identity::generate()?);
    let host_id = host_identity.id();
    let host = HostService::start(
        bind_host_endpoint(localhost)?,
        server_address,
        host_identity,
        Arc::new(HostSettings {
            gate: Arc::new(Mutex::new(gate)),
            provider: Arc::new(ScreenProvider::default()),
            delegate: Arc::new(Delegate),
        }),
    );
    let mut status = host.status();
    while !matches!(*status.borrow(), HostStatus::Online { .. }) {
        status.changed().await?;
    }
    let controller = Identity::generate()?;
    let endpoint = bind_controller_endpoint(localhost)?;
    let (channel, path) = connect(
        &endpoint,
        &server_address,
        &controller,
        host_id,
        Route::Automatic,
    )
    .await?;
    let viewer = ViewerSession::start(channel, Some(PASSWORD), || {}).await?;
    println!("viewer: connected to {host_id} over the {path:?} path");
    let mut frames = viewer.frames();
    let started = Instant::now();
    let mut first: Option<(u64, Instant)> = None;
    let mut last = 0u64;
    let mut size = (0, 0);
    while started.elapsed() < MEASURE_FOR {
        if tokio::time::timeout(Duration::from_millis(500), frames.changed())
            .await
            .is_err()
        {
            continue;
        }
        if let Some(frame) = frames.borrow_and_update().clone() {
            first.get_or_insert((frame.sequence, Instant::now()));
            last = frame.sequence;
            size = (frame.image.width(), frame.image.height());
        }
    }
    let (first_sequence, first_at) = first.ok_or("no frame was decoded")?;
    let seconds = first_at.elapsed().as_secs_f64();
    println!(
        "viewer: decoded {} frames of {}x{} in {:.1} s ({:.1} fps); move windows to raise the rate",
        last - first_sequence + 1,
        size.0,
        size.1,
        seconds,
        (last - first_sequence) as f64 / seconds
    );
    viewer.close().await?;
    server.shutdown().await;
    Ok(())
}

#[tokio::main]
async fn main() -> ExitCode {
    match run().await {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("loopback failed: {error}");
            ExitCode::FAILURE
        }
    }
}

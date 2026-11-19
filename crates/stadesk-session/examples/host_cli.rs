use std::process::ExitCode;
use std::sync::{Arc, Mutex};

use stadesk_core::{AccessGate, AccessMethods, Identity, PublicIdentity, StaDeskId};
use stadesk_desktop::DesktopError;
use stadesk_net::{ServerAddress, bind_host_endpoint};
use stadesk_session::{
    Approval, ByeReason, HostDelegate, HostService, HostSettings, HostStatus, ScreenProvider,
};

const USAGE: &str = "usage: host_cli SERVER_ADDRESS:PORT SERVER_KEY";

struct AutoApprove;

impl HostDelegate for AutoApprove {
    fn approve(&self, peer: &PublicIdentity) -> Approval {
        println!("approving {}", peer.id());
        Box::pin(async { true })
    }

    fn session_started(&self, peer: StaDeskId, input: Result<(), DesktopError>) {
        println!("session with {peer} started, input: {input:?}");
    }

    fn session_ended(&self, peer: StaDeskId, reason: ByeReason) {
        println!("session with {peer} ended: {reason:?}");
    }
}

async fn run() -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let mut arguments = std::env::args().skip(1);
    let address = arguments.next().ok_or(USAGE)?.parse()?;
    let fingerprint = arguments.next().ok_or(USAGE)?.parse()?;
    let server = ServerAddress {
        address,
        fingerprint,
    };
    let identity = Arc::new(Identity::generate()?);
    let gate = AccessGate::new(AccessMethods {
        accept_prompt: true,
        one_time_password: false,
        permanent_password: false,
        trusted_devices: false,
    })?;
    let host = HostService::start(
        bind_host_endpoint("0.0.0.0:0".parse()?)?,
        server,
        Arc::clone(&identity),
        Arc::new(HostSettings {
            gate: Arc::new(Mutex::new(gate)),
            provider: Arc::new(ScreenProvider::default()),
            delegate: Arc::new(AutoApprove),
        }),
    );
    let mut status = host.status();
    loop {
        let current = status.borrow_and_update().clone();
        match current {
            HostStatus::Online { observed } => {
                println!("online as {} via {observed}", identity.id())
            }
            other => println!("status: {other:?}"),
        }
        status.changed().await?;
    }
}

#[tokio::main]
async fn main() -> ExitCode {
    match run().await {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("host_cli failed: {error}");
            ExitCode::FAILURE
        }
    }
}

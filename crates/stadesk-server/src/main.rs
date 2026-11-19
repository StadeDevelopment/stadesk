use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::process::ExitCode;

use stadesk_net::ServerCertificate;
use stadesk_server::{PinStore, RunningServer, default_pin_path};

const DEFAULT_LISTEN: &str = "0.0.0.0:21300";
const DEFAULT_DATA_DIR: &str = "stadesk-server-data";
const CERTIFICATE_FILE: &str = "certificate.der";
const PRIVATE_KEY_FILE: &str = "private-key.der";
const USAGE: &str = "usage: stadesk-server [--listen ADDRESS:PORT] [--data-dir PATH]";

struct Options {
    listen: SocketAddr,
    data_dir: PathBuf,
}

fn parse_options() -> Result<Options, String> {
    let mut listen = DEFAULT_LISTEN.to_owned();
    let mut data_dir = PathBuf::from(DEFAULT_DATA_DIR);
    let mut arguments = std::env::args().skip(1);
    while let Some(argument) = arguments.next() {
        match argument.as_str() {
            "--listen" => listen = arguments.next().ok_or(USAGE)?,
            "--data-dir" => data_dir = PathBuf::from(arguments.next().ok_or(USAGE)?),
            "--help" | "-h" => return Err(USAGE.to_owned()),
            other => return Err(format!("unknown argument {other}\n{USAGE}")),
        }
    }
    let listen = listen
        .parse()
        .map_err(|_| format!("invalid listen address {listen}"))?;
    Ok(Options { listen, data_dir })
}

fn write_private(path: &Path, contents: &[u8]) -> std::io::Result<()> {
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    std::os::unix::fs::OpenOptionsExt::mode(&mut options, 0o600);
    std::io::Write::write_all(&mut options.open(path)?, contents)
}

fn load_or_create_certificate(data_dir: &Path) -> Result<ServerCertificate, String> {
    let certificate_path = data_dir.join(CERTIFICATE_FILE);
    let key_path = data_dir.join(PRIVATE_KEY_FILE);
    match (certificate_path.exists(), key_path.exists()) {
        (true, true) => {
            let certificate = std::fs::read(&certificate_path)
                .map_err(|error| format!("cannot read {}: {error}", certificate_path.display()))?;
            let key = std::fs::read(&key_path)
                .map_err(|error| format!("cannot read {}: {error}", key_path.display()))?;
            Ok(ServerCertificate::from_der(certificate, key))
        }
        (false, false) => {
            std::fs::create_dir_all(data_dir)
                .map_err(|error| format!("cannot create {}: {error}", data_dir.display()))?;
            let certificate = ServerCertificate::generate().map_err(|error| error.to_string())?;
            write_private(&key_path, certificate.private_key_der())
                .map_err(|error| format!("cannot write {}: {error}", key_path.display()))?;
            std::fs::write(&certificate_path, certificate.certificate_der())
                .map_err(|error| format!("cannot write {}: {error}", certificate_path.display()))?;
            Ok(certificate)
        }
        _ => Err(format!(
            "{} must contain both {CERTIFICATE_FILE} and {PRIVATE_KEY_FILE}, or neither",
            data_dir.display()
        )),
    }
}

async fn shutdown_signal() -> std::io::Result<()> {
    #[cfg(unix)]
    {
        let mut terminate =
            tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())?;
        tokio::select! {
            result = tokio::signal::ctrl_c() => result,
            _ = terminate.recv() => Ok(()),
        }
    }
    #[cfg(not(unix))]
    {
        tokio::signal::ctrl_c().await
    }
}

#[tokio::main]
async fn main() -> ExitCode {
    let options = match parse_options() {
        Ok(options) => options,
        Err(message) => {
            eprintln!("{message}");
            return ExitCode::FAILURE;
        }
    };
    let certificate = match load_or_create_certificate(&options.data_dir) {
        Ok(certificate) => certificate,
        Err(message) => {
            eprintln!("{message}");
            return ExitCode::FAILURE;
        }
    };
    let pins = match PinStore::open(&default_pin_path(&options.data_dir)) {
        Ok(pins) => pins,
        Err(error) => {
            eprintln!("cannot open the ID pin store: {error}");
            return ExitCode::FAILURE;
        }
    };
    let known = pins.len();
    let server = match RunningServer::start_with_pins(options.listen, &certificate, pins) {
        Ok(server) => server,
        Err(error) => {
            eprintln!("cannot start server: {error}");
            return ExitCode::FAILURE;
        }
    };
    println!("StaDesk server listening on {}", server.local_address());
    println!("Server key: {}", server.fingerprint());
    println!("Pinned IDs: {known}");
    if let Err(error) = shutdown_signal().await {
        eprintln!("cannot wait for shutdown signal: {error}");
    }
    server.shutdown().await;
    ExitCode::SUCCESS
}

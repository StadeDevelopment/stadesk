# StaDesk

StaDesk is a free, privacy-first remote desktop application by Stade Development. Every session is end-to-end encrypted with **Stade Core**, the same post-quantum hybrid cryptography as Stade (X25519 + ML-KEM-768, Ed25519 + ML-DSA-65). The rendezvous and relay server never sees the screen, the keyboard or any password.

The architecture reference with every design decision is [STADESK.md](STADESK.md).

## Features

- Your StaDesk ID (`STADESK-XXXX-XXXX-XXXX`) is derived from your own keys; nobody assigns it.
- Choose how others may connect: ask every time, one-time password, permanent password, trusted devices, in any combination.
- Direct peer-to-peer QUIC connection with NAT hole punching, falling back to the relay.
- VP9 video tuned for screen content, full keyboard and mouse control.
- macOS 13+, Windows 10+, Linux (X11 and Wayland).
- English and Turkish, picked from the system language.

## Repository

| Path | Contents |
|---|---|
| `crates/stadesk-core` | Identity, StaDesk ID, Stade Core handshake, session encryption, access control |
| `crates/stadesk-net` | QUIC transport, server key pinning, rendezvous client, direct path, relay |
| `crates/stadesk-server` | The rendezvous and relay server |
| `crates/stadesk-codec` | VP9 encoder and decoder |
| `crates/stadesk-desktop` | Screen capture and input injection for every platform |
| `crates/stadesk-session` | Host and viewer sessions |
| `app/` | The Tauri desktop application |
| `tools/` | Build, packaging and test scripts |

## Building

Requirements: Rust (stable), Node.js, libvpx with headers (`brew install libvpx` on macOS, `libvpx-dev` on Debian/Ubuntu).

```sh
cargo test --workspace
cd app && npm ci && npm run tauri build
```

Releases are built by GitHub Actions: push a tag `vX.Y.Z` that matches the workspace version and `.github/workflows/release.yml` publishes a release with the macOS `.dmg`, the Windows installer, the Linux `.deb` and `.AppImage`, and the update manifest. The same packages can be built locally:

| Platform | Command |
|---|---|
| macOS | `tools/macos/release.sh` |
| Windows (cross-built with `mingw-w64`, `nasm` and `makensis`) | `tools/windows/package.sh` |
| Linux (inside the `tools/linux` Docker image) | `tools/linux/package.sh deb` |

Updater artifacts are signed when `TAURI_SIGNING_PRIVATE_KEY` and `TAURI_SIGNING_PRIVATE_KEY_PASSWORD` are set. Signing settings for macOS are read from `~/.stadesk/signing.env` (mode 600), never from the repository.

## Running a server

```sh
stadesk-server --listen 0.0.0.0:21300 --data-dir /var/lib/stadesk-server
```

The server prints its **server key** on start. Users enter the server address and that key in Settings. The server needs UDP port 21300 open. On Windows, `tools/windows/install-server.ps1` installs it as a start-up task and opens the firewall port.

## Platform notes

- macOS: allow StaDesk under System Settings → Privacy & Security → Screen Recording (to be viewed) and Accessibility (to be controlled).
- Linux: WebKitGTK decodes video through GStreamer, so `gstreamer1.0-plugins-good` must be installed (the `.deb` depends on it). On Wayland the desktop asks for consent through its own dialog; remote control needs a desktop that offers the RemoteDesktop portal (GNOME, KDE).
- Windows: needs the WebView2 runtime, which Windows 10 and 11 receive with Edge.

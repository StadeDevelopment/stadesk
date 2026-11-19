# StaDesk

StaDesk is Stade Development's free, privacy-first remote desktop application. It speaks its own protocol (not Microsoft RDP) and protects every session with **Stade Core**, the same cryptographic suite used by Stade.

This file is the architecture reference. The codebase carries no comments; design rationale lives here.

## Stack

| Layer | Choice |
|---|---|
| Language | Rust (edition 2024), one workspace |
| Protocol and crypto | `crates/stadesk-core` |
| Desktop UI | Tauri 2 + React + TypeScript, English and Turkish selected from the OS language |
| Servers | Rust (axum + tokio): rendezvous / ID registry and relay, self-hosted |
| Platforms (v1) | Windows, macOS, Linux |

## Stade Core primitives

Identical to Stade:

| Purpose | Primitive |
|---|---|
| Classical signature | Ed25519 (strict verification, weak keys rejected) |
| Post-quantum signature | ML-DSA-65 (FIPS 204) |
| Classical key agreement | X25519 (non-contributory results rejected) |
| Post-quantum KEM | ML-KEM-768 (FIPS 203) |
| Hash | BLAKE2b-256 |
| KDF | HKDF over HMAC-BLAKE2b-256 |
| AEAD | ChaCha20-Poly1305 |

Every signature is a dual signature: Ed25519 and ML-DSA-65 over the same message, both must verify. Wire form is `ed25519_sig (64) || mldsa65_sig (3309)`.

## Identity and StaDesk ID

An identity is two signing keys, stored as 64 bytes of secret seed (`ed25519_seed (32) || mldsa65_seed (32)`). The public identity is `ed25519_pk (32) || mldsa65_pk (1952)`.

The StaDesk ID is derived exactly like a Stade ID, with its own domain tag so the two apps never produce the same ID from the same keys:

```
digest = BLAKE2b-256("stadesk-id-v1\0" || ed25519_pk || mldsa65_pk)
bits   = top 60 bits of digest[0..8] || top 4 bits of digest[8]
id     = "STADESK-" + 12 Crockford-base32 chars as XXXX-XXXX-XXXX
```

Parsing accepts lowercase, spaces, `_` and `-`, maps `O`→`0` and `I`/`L`→`1`, and strips the `STADESK` prefix only when the input has prefix-plus-payload length, so a bare payload that happens to start with those letters is kept intact.

The ID carries 64 bits of hash. The handshake always checks the full public keys against the dialled ID, so impersonating a specific ID needs a 2^64 second-preimage search. The rendezvous server will additionally pin the full public identity on first registration.

## Session handshake

Stade uses a Double Ratchet because chat messages are sparse and long-lived. A remote desktop session is a dense, short-lived stream (tens of frames per second), so StaDesk derives one pair of directional keys per connection instead of ratcheting per message. Forward secrecy comes from fresh ephemeral X25519 and ML-KEM-768 keys on every connection; a new connection is a new handshake.

Roles: the **initiator** is the controlling device, the **responder** is the device being controlled.

```
1. I → R  hello  = version u16 || eI_x25519 (32) || eI_mlkem768_ek (1184) || nonce (32)
2. R → I  reply  = eR_x25519 (32) || mlkem_ct (1088) || AEAD(k_hs_R, R_public || R_dual_sig)
3. I → R  finish = AEAD(k_hs_I, I_public || I_dual_sig)
```

Key schedule (`H` = BLAKE2b-256, `ikm = X25519(eI, eR) || ML-KEM shared secret`):

```
th1 = H("stadesk-hs-v1\0" || hello)
th2 = H(th1 || eR_x25519 || mlkem_ct)
k_hs_R, k_hs_I = HKDF(salt = th2, ikm), labels "stadesk hs responder" / "stadesk hs initiator"
R signs "stadesk-auth-v1\0" || "R" || th2
th3 = H(th2 || sealed responder auth)
I signs "stadesk-auth-v1\0" || "I" || th3
th4 = H(th3 || sealed initiator auth)
session keys = HKDF(salt = th4, ikm), labels "stadesk session initiator to responder" / "... responder to initiator"
```

Properties:

- **Hybrid**: the session key depends on both X25519 and ML-KEM-768, so breaking one alone is not enough.
- **Mutual authentication**: both sides dual-sign the transcript; the initiator also requires the responder's derived ID to equal the ID it dialled.
- **Identity hiding**: identities travel only inside AEAD, so a passive observer (including the relay) never learns who is connecting to whom beyond what rendezvous needs.
- **Downgrade resistance**: the version is inside `th1`, so any change breaks the handshake.
- **Channel binding**: `th4` is exposed as `channel_binding` for a future short verification code that both users can compare.

The handshake proves *who* the peer is. Whether that peer is *allowed* to control the device is decided by the access layer below.

## Session frames

```
frame = counter u64 BE (8) || ChaCha20-Poly1305(key_dir, nonce = 0^32 || counter, aad = counter bytes)
```

- Each direction has its own key, so counters never collide across directions.
- Counter `u64::MAX` is never used; reaching it requires a new handshake.
- Receivers keep a 2048-frame sliding replay window, so unordered transports (QUIC datagrams for video) work while duplicates and stale frames are rejected. A frame only enters the window after it authenticates, so forged frames cannot block real ones.

## Access authorization

The handshake proves who the controller is. `AccessGate` on the host decides whether that controller may connect. Like RustDesk, the host enables any combination of four methods (`AccessMethods`):

| Method | Default | Behaviour |
|---|---|---|
| Accept prompt | on | Host user clicks Accept / Reject |
| One-time password | on | 8 characters from the ID alphabet, shown as `XXXX-XXXX`; rotates after every successful use |
| Permanent password | off | Set by the host user, at least 8 characters, at most 128 bytes |
| Trusted devices | on (empty list) | Controllers whose full public identity is on the list connect without password or prompt |

The first message inside the session is `AccessRequest` (with or without a password); the host answers with `AccessResponse`:

```
request  = 0x00                                  without password
         | 0x01 || len u16 BE || UTF-8 password  len ≤ 128
response = 0x00 granted | 0x01 pending approval | 0x02 password required
         | 0x03 wrong password | 0x04 || retry_after_seconds u32 BE locked out | 0x05 denied
```

Evaluation order:

1. Trusted devices enabled and the controller's full public identity (not just its ID) is listed → granted.
2. A password was sent and a password method is enabled → check the one-time password, then the permanent password. Wrong → `wrong password`.
3. No password: accept prompt enabled → `pending approval`; otherwise a password method enabled → `password required`; otherwise `denied`.

Passwords travel only inside the authenticated session, so the relay never sees them.

**Permanent password storage**: Argon2id with Stade's parameters (19 456 KiB, t = 2, p = 1, 32-byte output) and a fresh 16-byte salt. Stored as `0x01 || salt (16) || digest (32)`. A fixed format instead of a PHC string means the stored value cannot carry attacker-chosen cost parameters. Comparison is constant time.

**One-time password input** is normalized like an ID (case, spaces, `-`, `_`, `O`→`0`, `I`/`L`→`1`) and compared in constant time.

**Lockout** follows Stade's schedule (after failure 3: 5 s, 4: 15 s, 5: 30 s, 6: 1 min, 7: 5 min, 8: 15 min, 9: 1 h, 10+: 24 h). It is global on the host, not per controller, because StaDesk identities cost nothing to create and a per-identity counter could be bypassed by generating new keys. Lockout only blocks password attempts; trusted devices and the accept prompt keep working, and the host user can reset it.

## Network

All traffic is QUIC (quinn, TLS 1.3 only, ALPN `stadesk/1`, keep-alive 5 s, idle timeout 30 s). Messages on a stream are length-prefixed: `len u32 BE || payload`, at most 16 MiB; control messages at most 16 KiB.

### Server and server key

`stadesk-server` is one self-hostable binary that does both rendezvous and relay (default `0.0.0.0:21300`, data in `stadesk-server-data/`). On first start it creates a self-signed certificate (ECDSA P-256), stores the private key with mode 0600, and prints the **server key**:

```
server key = Crockford base32 of BLAKE2b-256("stadesk-server-cert-v1\0" || certificate DER), grouped by 4
```

Clients pin this key, the same idea as RustDesk's "Key" field: a connection to a server whose certificate does not hash to the configured key is refused. The usual web PKI is not used, so a self-hosted server needs no domain or CA.

### Peer-to-peer TLS

Between two StaDesk devices the QUIC certificate is ephemeral and is **not** checked against an identity. The TLS handshake signature is still verified, but authentication comes from the Stade Core handshake that runs inside the connection, which checks the dialled StaDesk ID against full public keys. TLS here only adds a second encryption layer and hides the Stade Core handshake from passive observers.

### Rendezvous

```
Host registration (long-lived stream on the host's server connection)
  H → S  Register(public identity)
  S → H  Challenge(32 random bytes)
  H → S  Proof(dual signature over "stadesk-register-v1\0" || server key bytes || challenge)
  S → H  Registered(observed public address)   or RegistrationRejected
  S → H  IncomingRequest(session token, controller address)   ... repeated

Controller connect
  C → S  Connect(target StaDesk ID)
  S → C  PeerFound(session token, host address) | PeerOffline | RateLimited
```

- The ownership proof includes the server key, so a proof made for one server cannot be replayed to another.
- The server pins every ID to its first public identity and rejects a different identity for the same ID from then on. Pins survive restarts: `pins.bin` in the data directory holds fixed 54-byte records (`STADESK-…` text, then BLAKE2b-256 of `"stadesk-pin-v1\0" || public identity`), appended and synced; a torn last record is dropped on start. Storing a hash instead of the 1984-byte identity keeps a large public server's pin file small. A storage error refuses the registration rather than skipping the check.
- Connect requests are limited to 20 per source IP per minute, with at most 16 undelivered requests queued per host.
- Session tokens are 16 random bytes and live 60 s.
- The server necessarily learns which ID asked to reach which ID and their IP addresses. It never sees passwords, screen contents or input, which travel only inside the Stade Core session.

### Direct path and hole punching

Host and controller each use **one UDP socket** for both the server and the peer, so the address the server observed is the one the NAT will route.

1. The host, on `IncomingRequest`, registers the token with its `DirectAcceptor` and sends a QUIC connection attempt toward the controller's observed address (`punch`), which opens the host's NAT mapping.
2. The controller connects to the host's observed address, opens a stream and writes the 16-byte token first.
3. The host accepts the connection only if the token matches an expected session; otherwise it closes the connection.

The controller is always the QUIC client and the Stade Core initiator; the host is the QUIC server and responder.

### Relay path

If the direct path fails, both sides open a stream to the server with `RelayJoin(token, role)`. The server pairs exactly one controller and one host per token, answers `RelayReady` to both, then copies bytes in both directions. The relay only sees Stade Core ciphertext.

### Session end

`SecureChannel::receive` returns `None` when the peer finished its stream at a frame boundary. `close` finishes the local stream and waits until the peer acknowledges it, so the last frames are not lost when the process drops the connection right after.

## Video codec

`crates/stadesk-codec` wraps libvpx (VP9 profile 0, 8-bit I420).

### Build

libvpx is found through pkg-config. Bindings are generated at build time by bindgen from the installed headers (`env-libvpx-sys` with the `generate` feature), because its bundled bindings stop at libvpx 1.13. No struct layout is written by hand. Development on macOS: `brew install libvpx` (1.17.0 at the time of writing). Release builds link libvpx statically on every platform (see Release builds).

### Colour conversion

Capture delivers BGRA. `I420Frame::from_bgra` converts with integer BT.601 studio-range coefficients (the same as libyuv):

```
Y = ((66R + 129G + 25B + 128) >> 8) + 16
U = ((-38R - 74G + 112B + 128) >> 8) + 128
V = ((112R - 94G - 18B + 128) >> 8) + 128
```

Chroma is the rounded average of each 2×2 block (fewer pixels at odd edges). `to_bgra` uses the matching inverse. The encoder tags images as BT.601 studio range so both sides agree. Measured on the development Mac (M-series, release build), 1080p conversion takes about 5 ms per frame in plain Rust; SIMD or libyuv can replace it later without changing the API.

### Encoder settings

| Setting | Value | Reason |
|---|---|---|
| Deadline | `VPX_DL_REALTIME` | interactive latency |
| Speed (`cpu-used`) | 8 (0–9 allowed) | realtime speed for screen content |
| Rate control | CBR, target set by caller, changeable at runtime | predictable bandwidth, adaptive bitrate later |
| Lag in frames | 0 | no look-ahead latency |
| Content tuning | `VP9E_CONTENT_SCREEN` | text and UI edges |
| AQ mode | 3 (cyclic refresh) | standard for realtime VP9 |
| Row multithreading | on, threads = CPU count capped at 8 | |
| Tile columns | log2 limited by threads and a 256-pixel minimum tile width | |
| Keyframes | first frame, then only on request (`VPX_KF_FIXED`, which libvpx defines as placement disabled) | the session transport is reliable, so periodic keyframes only waste bandwidth; a new viewer or a decode error asks for one |
| Timestamps | milliseconds; a non-increasing timestamp is replaced by previous + 1 | libvpx expects increasing pts |

Measured on the development Mac at 1080p, 4 Mbps target: encode about 5 ms, decode about 1 ms per frame.

### Unsafe code

All FFI is confined to `vp9.rs`. The rules it follows:

- bindgen turns C enums into Rust enums, so a zeroed or garbage value would be undefined behaviour. The encoder configuration is created as uninitialised memory and only read after `vpx_codec_enc_config_default` returns OK. Structs that are zero-initialised (`vpx_codec_ctx_t`, `vpx_image_t`) only contain enums whose zero value is a valid variant (`VPX_CODEC_OK`, `VPX_IMG_FMT_NONE`, `VPX_CS_UNKNOWN`, `VPX_CR_STUDIO_RANGE`).
- `vpx_codec_enc_init` and `vpx_codec_dec_init` are macros; the code calls the `_ver` functions with the ABI versions from the same headers.
- The codec context lives in a `Box` so its address never moves, and is destroyed only if initialisation set its private pointer.
- Input images wrap the frame's own buffer without copying; plane pointers and strides are then set explicitly. With zero lag, VP9 copies the source into its look-ahead buffer during `vpx_codec_encode`, so the buffer only has to live for that call.
- Decoded images are copied out row by row, honouring stride; anything other than 8-bit I420 with 2×2 chroma subsampling is rejected.
- `vpx_codec_control_` is variadic; every control passed takes an `int` or `unsigned int`, which have the same size and promotion, and all values are non-negative.
- Encoder and decoder are `Send` (one owner at a time, guaranteed by `&mut self`) but not `Sync`.

## Screen capture

`crates/stadesk-desktop` exposes one API on every platform: `displays()` lists displays with their size in physical pixels, and `ScreenCapturer::start(display_id, CaptureOptions)` delivers `CapturedFrame`s (tightly packed BGRA) through `next_frame(timeout)`.

Minimum supported systems (confirmed by Eren): macOS 13, Windows 10, Linux with X11 or Wayland (PipeWire portal).

### Frame hand-off

The OS delivers frames on its own thread. They go into a single "latest frame" slot: a new frame replaces an unconsumed one, so a slow encoder always gets the newest image instead of falling further behind. `next_frame` returns `None` on timeout, which is normal when nothing on screen changes. A stream failure (display removed, permission revoked) is stored in the slot and returned by every later call.

### macOS

ScreenCaptureKit through the `objc2` framework bindings (`objc2-screen-capture-kit` and friends), which call the Objective-C runtime directly from Rust. The `screencapturekit` crate was tried first and rejected: it compiles a Swift bridge at build time and failed to link against the Swift toolchain on the development Mac, which would make every build machine depend on a matching Swift setup.

- Capture size is the display's physical pixel size from `CGDisplayModeGetPixelWidth/Height` (for example 2940×1912 on a Retina panel), not the point size.
- Pixel format `kCVPixelFormatType_32BGRA`; `minimumFrameInterval` = 1 / `max_frame_rate` (default 30, allowed 1–120); cursor shown by default.
- Only samples whose `SCStreamFrameInfoStatus` is `Complete` are used. ScreenCaptureKit sends idle samples without new pixels when the screen is static, so the delivered frame rate drops on an unchanged screen. This is intended.
- Pixels are copied row by row out of the locked `CVPixelBuffer`, honouring `bytesPerRow`, then the buffer is unlocked.
- `SCStreamErrorUserDeclined` and `SCStreamErrorMissingEntitlements` map to `CaptureError::PermissionDenied`; the UI will use it to send the user to System Settings → Privacy & Security → Screen Recording.
- ScreenCaptureKit completion handlers run on a system queue. The shareable-content object is moved once from that queue to the waiting thread through a channel (`Handoff`), so it is never touched by two threads at the same time. `ScreenCapturer` itself is not `Send` and stays on the thread that created it.

Measured on the development Mac (2940×1912, release build): BGRA→I420 + VP9 encode + decode about 22 ms per frame; a real captured frame survives the VP9 round trip at 42 dB RGB PSNR. `cargo run --release -p stadesk-desktop --example capture_check` repeats this measurement on any Mac.

### Windows

DXGI Desktop Duplication (`IDXGIOutput1::DuplicateOutput`, Windows 8+, so Windows 10 is covered) through the `windows` crate.

- Display ids encode where the output lives: `adapter_index << 8 | output_index`, so `start` can find the same output again without a lookup table. Only outputs attached to the desktop are listed; the primary display is the one whose desktop rectangle starts at (0, 0), which Windows guarantees.
- The calling thread is switched to per-monitor DPI awareness v2 while enumerating and duplicating, and switched back afterwards, so sizes are physical pixels even in a process without a DPI manifest.
- A Direct3D 11 device is created on the display's own adapter (`D3D_DRIVER_TYPE_UNKNOWN` with an explicit adapter). Each new desktop image is copied with `CopyResource` into a CPU-readable staging texture, mapped, and copied row by row honouring `RowPitch`. The duplicated frame is released by a guard on every path.
- Only `DXGI_FORMAT_B8G8R8A8_UNORM` desktops are accepted; that is the only format the original `DuplicateOutput` produces.
- `DXGI_ERROR_ACCESS_LOST` (mode change, desktop switch, full-screen app) recreates the duplication transparently. `E_ACCESSDENIED`, `DXGI_ERROR_NOT_CURRENTLY_AVAILABLE` and `DXGI_ERROR_SESSION_DISCONNECTED` (secure desktop such as UAC or the lock screen, or a disconnected session) are treated as temporary: `next_frame` keeps retrying until its timeout and returns `None`. Capturing the secure desktop needs a Windows service running as SYSTEM, which is a later step.
- The frame rate is limited in `next_frame` by spacing deliveries at least 1 / `max_frame_rate` apart.

Desktop Duplication does not draw the mouse pointer, and it delivers a rotated monitor's image unrotated. Both are handled in `transform.rs`, which is plain pixel code compiled on every platform's test build and unit-tested on macOS:

- Rotation: `DXGI_MODE_ROTATION_ROTATE90/180/270` are turned into clockwise quarter turns of the image (the same mapping as Microsoft's Desktop Duplication sample), so the output has the orientation the user sees.
- Pointer: the position and shape are tracked from the frame info. A pointer-only change (mouse moved, screen unchanged) re-composites the pointer on the last clean desktop image instead of waiting for the screen to change. The three shape types are drawn as Windows defines them: monochrome (AND mask then XOR mask, 1 bit per pixel, height halved), colour (32-bit with straight alpha blending), masked colour (alpha 0 replaces, alpha 255 XORs). The pointer is clipped at every edge.

Verified on 2026-11-18 on a Windows 10 Enterprise LTSC 21H2 virtual machine (VMware SVGA, no GPU) inside an RDP session: the display was found at the session resolution (1470×923), a static screen delivered one frame in ten seconds as designed, a continuously changing screen delivered 16.6 fps, and the saved frame had correct orientation and colours. The test binary was cross-compiled on the Mac for `x86_64-pc-windows-gnu` with mingw-w64 and started in the interactive session through a scheduled task, because a process started over SSH has no desktop to duplicate. Not yet verified: pointer drawing (under RDP the client draws the pointer, so Desktop Duplication reports none), rotated monitors, and real GPU hardware. These need a physical Windows machine: `cargo run --release -p stadesk-desktop --example capture_check -- --save capture.bmp` lists displays, measures ten seconds of capture and saves the last frame so the pointer position and colours can be checked by eye.

GDI fallback (`windows_gdi.rs`). On 2026-11-19 the same machine, reconnected from a different RDP client, put the session on the "Microsoft Remote Display Adapter", and `DuplicateOutput` failed with `E_UNEXPECTED`. Desktop Duplication is not available on every remote or virtual display driver, so when it cannot start, or fails later in a session, the capturer switches to GDI: `BitBlt` with `CAPTUREBLT` from the screen DC into a 32-bit top-down DIB section, the pointer drawn with `GetCursorInfo` and `DrawIconEx` (hot spot from `GetIconInfo`), alpha forced to opaque. GDI has no change notification, so it grabs at the frame-rate cap and delivers a frame only when the pixels differ from the last delivered one. `ScreenCapturer::method()` reports which path is active. Verified on the VDS: correct image with the pointer, and full sessions in both directions through the production server (Windows viewing the Mac, the Mac viewing and controlling Windows).

### Linux

The session type decides the backend: `WAYLAND_DISPLAY` set means Wayland, otherwise `DISPLAY` means X11. Wayland wins when both are set, because under Wayland the X11 server only sees XWayland windows.

**X11** (`x11rb`, pure Rust):

- Monitors come from RandR `GetMonitors`; without RandR the whole screen is one monitor. Display ids are list indexes.
- Only the common 32-bit little-endian BGRX visual is accepted; anything else is reported instead of producing wrong colours.
- Pixels are read with MIT-SHM 1.2: a `memfd` is created, mapped, and handed to the X server with `AttachFd`, so no SysV shared memory is involved. If the server cannot do that (for example X forwarded over SSH), plain `GetImage` is used. `method()` reports which one is active.
- X11 has no change notification without DAMAGE, so the capturer polls at `max_frame_rate` and only delivers a frame when the pixels or the pointer changed.
- The pointer comes from XFixes `GetCursorImage` (premultiplied ARGB) and is drawn at position minus hot spot.

Verified under Xvfb: MIT-SHM path active, only changed frames delivered, colours exact (`#f03060` background read back as 240, 48, 96), pointer hot spot centred on the injected position.

**Wayland** (`ashpd` + `pipewire`):

- Capture goes through xdg-desktop-portal. If the desktop offers the RemoteDesktop portal with keyboard and pointer (GNOME, KDE), one RemoteDesktop session carries both screen and input, so the user sees one consent dialog and absolute pointer motion can refer to the shared stream. Otherwise (wlroots compositors) a ScreenCast session is used and remote control is reported as unsupported.
- The pointer is requested embedded in the frames when the portal offers it.
- `displays()` returns a single entry with size 0, because the portal's own dialog picks the screen; the real size is known after the first frame.
- Frames arrive on a dedicated thread running a PipeWire main loop. Only memory-mapped buffers are accepted (BGRx, BGRA, RGBx, RGBA); DMA-BUF would need EGL and is not requested. The D-Bus connection that owns the portal session is kept alive by a one-worker Tokio runtime on that thread, and the session is closed when capture stops.

Verified against the real `xdg-desktop-portal` frontend and real PipeWire with a mock portal backend (`tools/linux/mock-portal.py`) that publishes a GStreamer test pattern: 30 fps 640×480, correct colour order, both the RemoteDesktop and the ScreenCast-only paths. A full compositor test with sway and `xdg-desktop-portal-wlr` was not possible without a GPU: wlroots screencopy in xdpw 0.7 and 0.9 insists on DMA-BUF support, and sway 1.10 lacks `ext-image-copy-capture`.

## Remote input

`ScreenCapturer::input()` returns an `InputInjector` for the captured display, so pointer coordinates are simply pixel positions in the frames the controller sees.

```
InputEvent = PointerMove { x, y }                 frame pixels, clamped to the display
           | PointerButton { Left|Right|Middle|Back|Forward, pressed }
           | Scroll { horizontal, vertical }      120 per wheel notch; vertical > 0 = away from the user, horizontal > 0 = right
           | Key { usage, pressed }               USB HID keyboard usage (physical key)
```

- Keys are physical positions (USB HID usages), so the host's own keyboard layout applies, as in every remote desktop tool. `keymap.rs` maps each usage to Linux evdev, Windows set-1 scan code and macOS virtual key code, plus the browser `KeyboardEvent.code` name the UI will receive. The values follow Chromium's cross-platform key table.
- The injector remembers held keys and buttons and releases them when it is dropped, so a dropped connection never leaves Ctrl or a mouse button stuck on the host. Releases of keys that are not held are ignored.

Per platform:

| Platform | Mechanism | Notes |
|---|---|---|
| macOS | `CGEventPost` at the HID tap | Needs the Accessibility permission (`AXIsProcessTrusted`), reported as `PermissionDenied(InputControl)`. Pixels are converted to points with the display bounds. Drag events while a button is held, click count for double clicks (500 ms, 4 points), modifier flags kept on every event, modifier keys posted as `FlagsChanged` like real hardware, line-based scrolling with remainder carry. |
| Windows | `SendInput` | Absolute moves normalised over the whole virtual desktop to the pixel centre, under per-monitor DPI awareness. Keys by scan code with `KEYEVENTF_EXTENDEDKEY` for E0 keys; Pause, NumLock and PrintScreen by virtual key because their scan codes are not sent consistently. A blocked `SendInput` (secure desktop, disconnected session, higher-integrity window) becomes an error. |
| X11 | XTEST `FakeInput` | Key code = evdev + 8; wheel as buttons 4–7; each event is followed by a round trip so it has been processed when `inject` returns. |
| Wayland | RemoteDesktop portal | Events go through an unbounded channel to the portal thread, so `inject` never blocks or nests a Tokio runtime. Frame pixels are scaled to the stream's logical size; buttons are evdev `BTN_*`; discrete scroll steps use Wayland's sign (down positive). |

Verified:

- macOS: an in-place pointer move and a lone right Shift were posted on the development Mac and observed through `CGEventSourceFlagsState` (nothing visible happens). Posting modifiers as `KeyDown` also set the flags, so the `FlagsChanged` choice is based on matching hardware behaviour, not on that test.
- Windows: every non-layout-dependent scan code in the table matches `MapVirtualKeyW` on Windows 10; live injection returned the expected "blocked" error because the RDP session was disconnected at the time, and still needs a run in a connected session.
- X11: 93 keys produce the expected keysyms through the server's evdev keymap; pointer moves, button state and key state read back through `QueryPointer` and `QueryKeymap`.
- Wayland: the mock portal recorded exactly the expected `NotifyPointerMotionAbsolute` (pixel centres), `NotifyPointerButton` 272, `NotifyPointerAxisDiscrete` −2/−1 and `NotifyKeyboardKeycode` 30 calls.

## Session layer

`crates/stadesk-session` joins capture, codec, network and access control.

Messages inside the Stade Core channel (`message.rs`):

```
controller → host   0x01 AccessRequest | 0x02 Input(event) | 0x03 KeyframeRequest | 0x04 Bye
host → controller   0x81 AccessResponse | 0x82 StreamStart(codec, width, height) | 0x83 VideoFrame(ts, keyframe, data) | 0x84 Bye(reason)
```

Host (`host.rs`):

1. Access: up to three `AccessRequest`s are evaluated by `AccessGate` on a blocking thread (Argon2id is slow on purpose). `AwaitPrompt` sends `PendingApproval` and asks the `HostDelegate` (the app's dialog), which gets the full public identity so "trust this device" can store it; no answer within 60 s counts as a rejection.
2. Streaming: a capture thread opens the desktop (`DesktopProvider`), encodes, and hands frames to the network task through a channel of capacity one. Encoded VP9 frames are never dropped, because later frames depend on them; when the network is slow the encoder simply waits, and the capturer keeps only the newest image meanwhile, so latency does not pile up. A size change creates a new encoder, sends `StreamStart` and forces a keyframe. Bitrate is `pixels / 500` kbit/s clamped to 1.5–16 Mbit/s.
3. Input runs on its own thread; the injector is created where the capturer lives and is dropped when the session ends, which releases held keys.

Viewer (`viewer.rs`): after access is granted, frames are either decoded in Rust (`Delivery::Decoded`, a `watch` of the newest `I420Frame`, keyframe requested automatically after a decode error) or handed out as encoded packets (`Delivery::Packets`) for a decoder in the UI.

Connection (`connect.rs`): the controller tries the direct path for 3 s and falls back to the relay; the host waits for both at once and uses whichever arrives (10 s for direct, 30 s for the relay pairing). `HostService` keeps the host registered and reconnects with exponential backoff (1 s to 30 s).

Verified by end-to-end tests with a real server, QUIC, Stade Core and VP9 and a synthetic desktop (direct and relay paths, password, approved and rejected prompts, wrong password, capture failure, packet delivery), ten runs in a row on macOS and on Linux; and with the real screen through `examples/loopback.rs`.

## Desktop app

`app/` is the Tauri 2 application (`app/src-tauri` Rust, `app/src` React + TypeScript, built with Vite). Bundle identifier `dev.stade.stadesk`, in line with Stade's `dev.stade.app`.

- **Storage** (app data directory): `identity.key` holds the 64-byte identity seed with mode 0600 on Unix; `config.json` holds the server address and key, access methods, the permanent password as `0x01 || salt || Argon2id digest`, trusted devices as full public identities, and the language. Writes go to a temporary file that is then renamed.
- **Host**: started at launch and restarted when the server settings change; a server name that does not resolve is retried every 30 s. An incoming request brings the main window to the front and shows Accept / Reject with "trust this device"; a request that times out on the host side disappears from the UI.
- **Viewer**: each session opens its own window. Encoded VP9 packets travel to that window over a binary Tauri channel (`0x00 || width || height` for a stream start, `0x01 || keyframe || timestamp || data` for a packet, big endian), and the page decodes them with WebCodecs `VideoDecoder` (codec `vp09.00.LL.08`, the lowest supported level among 4.1, 5.1, 6.1) and draws onto a canvas. Sending decoded pictures over IPC instead would be about 250 MB/s at 2940×1912 and 30 fps; packets are tens of kilobytes.
- **Input**: pointer positions are mapped from the letterboxed canvas back to stream pixels; moves are sent once per animation frame, everything else immediately and in order. Keys travel as `KeyboardEvent.code` and are mapped to USB HID usages by the shared Rust table, so there is a single key table. Auto-repeat keydowns are dropped because the remote system repeats held keys itself. Losing focus releases every held key and button.
- **Languages**: English and Turkish (`app/src/i18n/*.json`, flat keys with `{placeholders}` like Stade); "System language" picks Turkish when the OS prefers it.
- `--connect <ID>` on the command line opens a session at start-up.
- **Icon**: `app/src-tauri/icons/icon.png` and `icon.ico` are plain grey placeholders, only because Tauri refuses to build without them. They are to be replaced by Stade Development's logo.

Verified on macOS: the release `StaDesk.app` was configured for a local server and started with `--connect`; it connected over the direct path, the host approved it, and the viewer window decoded the live screen with WebKit's WebCodecs (the window showed the expected endless mirror of the screen it was viewing). The main window picked Turkish from the system language.

Verified on Linux (Debian trixie container, Xvfb, WebKitGTK 4.1): the release app built with `--features tauri/custom-protocol` connected to a host through a local server and the viewer window decoded the live picture with WebKitGTK's WebCodecs (again the nested mirror of its own toolbar). WebKitGTK decodes VP9 through GStreamer, so Linux packages must depend on the GStreamer good plugins. `tools/linux/app-test.sh` reproduces the test.

## Release builds

- macOS: `tools/macos/release.sh` builds `StaDesk.app` and `StaDesk_<version>_aarch64.dmg` (about 5 MB). It reads signing settings from `~/.stadesk/signing.env`, which must be mode 600 and lives outside the repository: `APPLE_SIGNING_IDENTITY` for the Developer ID Application certificate, and `APPLE_API_ISSUER`, `APPLE_API_KEY`, `APPLE_API_KEY_PATH` (an App Store Connect API key) for notarisation; empty values are ignored and the build is unsigned. Xcode is not needed: `codesign`, `notarytool` and `stapler` come with the Command Line Tools. The Developer ID private key is generated locally into `~/.stadesk/developer-id.key` with a certificate signing request; Apple's certificate is then paired with it by `tools/macos/import-certificate.sh`, which imports both into the login keychain and records the identity name.
- Windows: `tools/windows/package.sh` cross-compiles on the Mac with mingw-w64 and writes `target/dist/StaDesk-windows-x64.zip` with `StaDesk.exe` and `WebView2Loader.dll` (the GNU target cannot link the WebView2 loader statically). `tools/windows/build-libvpx.sh` builds a static libvpx 1.17.0 for mingw from the SHA-256 pinned upstream tarball, and `tools/windows/cargo.sh` points the build at it, including the mingw headers bindgen needs. Windows 10 and 11 ship the WebView2 runtime through Edge updates; the test machine had version 157. The executable is not code-signed; Stade Development decided on 2026-11-19 to postpone a Windows certificate, so SmartScreen warns on first launch.
- `.cargo/config.toml` sets `VPX_STATIC=1`. On macOS libvpx is linked statically (the Homebrew `libvpx.a`), so the app only depends on system frameworks. On Linux `pkg-config` will not statically link a library from a system directory, so `tools/linux/package.sh` passes the library directory explicitly and the binary carries libvpx too.
- Linux: `tools/linux/package.sh deb` runs inside the `tools/linux` image (which carries `cargo tauri` 2.12.6) and writes the package to `target/dist/linux/`. Its dependencies are WebKitGTK 4.1, GTK 3 and `gstreamer1.0-plugins-good` (WebKitGTK decodes VP9 through GStreamer). On the Apple-silicon development Mac the image is arm64, so it produces an arm64 package; an x86_64 package needs an x86_64 Linux machine or CI.

## Production server

The first public rendezvous and relay server runs on Stade Development's Windows 10 VDS at `relay.stade.dev:21300` (UDP), server key `0XVC-KY51-CPRH-HA18-AQSD-RMJM-93ZN-F4CY-GWE8-1RNX-2N96-RFY4-5NG0`. The app uses it by default (`DEFAULT_SERVER_ADDRESS` and `DEFAULT_SERVER_KEY` in `config.rs`); Settings can point to another server. `tools/windows/install-server.ps1` installs it: the binary in `Program Files\StaDesk Server`, key material and `pins.bin` in `ProgramData\StaDesk\server` readable only by SYSTEM and Administrators, a firewall rule for UDP 21300, and a start-up scheduled task running as SYSTEM that restarts after failures, logging to `server.log`. Reinstalling keeps the key. Losing `private-key.der` changes the server key, which every client has pinned, so that directory must be backed up.

## Test environments

- `tools/linux/Dockerfile` builds a Debian image with Rust, libvpx, PipeWire, Xvfb, sway, `xdg-desktop-portal-wlr` 0.9.0 (built from a SHA-256 pinned tarball) and GStreamer. On the development Mac it runs under Colima.
- `tools/linux/portal-test.sh` starts D-Bus, PipeWire, WirePlumber, the mock portal and the real portal frontend, then runs the capture and input examples; `MOCK_REMOTE_DESKTOP=0` exercises the ScreenCast-only path.
- `tools/linux/wayland-test.sh` does the same with headless sway and xdpw, for machines with a GPU.
- Windows binaries are cross-compiled on the Mac for `x86_64-pc-windows-gnu` with mingw-w64 and run on a Windows 10 test machine; anything that needs a desktop is started in the logged-in session through a scheduled task.

## Workspace layout

```
crates/stadesk-core    identity, StaDesk ID, handshake, session cipher, access authorization, ownership proof
crates/stadesk-net     QUIC/TLS setup, server key, framing, secure channel, rendezvous client, direct path, relay join
crates/stadesk-server  rendezvous + relay server binary
crates/stadesk-codec   VP9 encoder/decoder over libvpx, BGRA ↔ I420 conversion
crates/stadesk-desktop screen capture and remote input for macOS, Windows, Linux X11 and Wayland
crates/stadesk-session host and viewer sessions, connection set-up, host service
app/                   Tauri desktop application
tools/linux/           Linux test and packaging image, Wayland, portal and app test scripts, deb packaging
tools/macos/           signed release build, Developer ID certificate import
tools/windows/         libvpx for mingw, cross-build wrapper, app packaging, server installer
```

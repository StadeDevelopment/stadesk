FROM debian:trixie-slim

ENV DEBIAN_FRONTEND=noninteractive
RUN apt-get update && apt-get install -y --no-install-recommends \
        build-essential pkg-config clang libclang-dev curl ca-certificates git \
        libvpx-dev libpipewire-0.3-dev libspa-0.2-dev libdbus-1-dev \
        xvfb x11-utils x11-xserver-utils xdotool \
        sway xdg-desktop-portal xdg-desktop-portal-wlr pipewire wireplumber dbus dbus-user-session \
        grim wtype swaybg \
        meson ninja-build libwayland-dev wayland-protocols libinih-dev libgbm-dev libdrm-dev libsystemd-dev \
    && rm -rf /var/lib/apt/lists/*

ARG XDPW_VERSION=0.9.0
ARG XDPW_SHA256=b1ffd69748e565a2628ac2a6837f9e46697d6e44a0f11d2719b46d99572f1c13
RUN curl -sSfL -o /tmp/xdpw.tar.gz https://codeload.github.com/emersion/xdg-desktop-portal-wlr/tar.gz/refs/tags/v${XDPW_VERSION} \
    && echo "${XDPW_SHA256}  /tmp/xdpw.tar.gz" | sha256sum -c - \
    && tar -xzf /tmp/xdpw.tar.gz -C /tmp \
    && meson setup /tmp/xdpw-build /tmp/xdg-desktop-portal-wlr-${XDPW_VERSION} --prefix=/opt/xdpw -Dman-pages=disabled -Dsystemd=disabled \
    && ninja -C /tmp/xdpw-build install \
    && rm -rf /tmp/xdpw.tar.gz /tmp/xdpw-build /tmp/xdg-desktop-portal-wlr-${XDPW_VERSION}

ENV RUSTUP_HOME=/opt/rustup CARGO_HOME=/opt/cargo PATH=/opt/cargo/bin:$PATH
RUN curl -sSf https://sh.rustup.rs | sh -s -- -y --profile minimal --default-toolchain stable --component clippy,rustfmt

RUN apt-get update && apt-get install -y --no-install-recommends \
        python3 python3-gi gir1.2-glib-2.0 \
        gstreamer1.0-tools gstreamer1.0-plugins-base gstreamer1.0-pipewire pipewire-bin \
    && rm -rf /var/lib/apt/lists/*

RUN apt-get update && apt-get install -y --no-install-recommends \
        libwebkit2gtk-4.1-dev libgtk-3-dev librsvg2-dev libsoup-3.0-dev libjavascriptcoregtk-4.1-dev \
        gstreamer1.0-plugins-good gstreamer1.0-plugins-bad gstreamer1.0-libav file xdg-utils \
    && rm -rf /var/lib/apt/lists/*

RUN cargo install tauri-cli --version 2.12.6 --locked \
    && apt-get update && apt-get install -y --no-install-recommends fakeroot dpkg-dev squashfs-tools \
    && rm -rf /var/lib/apt/lists/*

ENV CARGO_TARGET_DIR=/target
RUN useradd --create-home --uid 1000 tester
WORKDIR /work

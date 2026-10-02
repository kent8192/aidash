FROM rust:1.96-bookworm
RUN apt-get update && apt-get install -y --no-install-recommends libwebkit2gtk-4.1-dev libayatana-appindicator3-dev librsvg2-dev patchelf libssl-dev libdbus-1-dev xvfb xauth dbus-x11 gnome-keyring webkit2gtk-driver python3 python3-pil ca-certificates xdg-utils && rm -rf /var/lib/apt/lists/*
RUN cargo install tauri-driver --locked
WORKDIR /workspace

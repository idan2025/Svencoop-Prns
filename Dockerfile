# Docker host release for Sven Co-op over Reticulum.
#
# One container runs the web control panel (axum, port 8080), the bridge server
# in-process (RNS TCPServerInterface on 4966), and the Sven Co-op dedicated
# server as a child process (UDP 27015). The dedicated server + steamcmd are
# pulled at runtime into volumes on first use (via the panel's "Start / pull"),
# so the image stays small.
#
# Build:  docker compose build
# Run:    docker compose up
# Panel:  http://<host>:8080

# ---- build stage ----
FROM rust:1.90-bookworm AS builder
WORKDIR /build

# Copy only what the controller web binary needs: the root lib + vendored Prns
# crates + the controller crate. The Tauri shell (gui/src-tauri) is NOT needed
# here (the web binary serves the static frontend directly).
COPY Cargo.toml Cargo.lock ./
COPY vendor/ vendor/
COPY src/ src/
COPY controller/ controller/

# The controller is a standalone crate with path-deps into the root + vendor.
RUN cargo build --release --manifest-path controller/Cargo.toml --bin sc-rns-controller-web

# ---- runtime stage ----
FROM debian:bookworm-slim AS host

# 32-bit libs: steamcmd and the Sven Co-op DS (svends_i686) are 32-bit GoldSrc.
RUN dpkg --add-architecture i386 && \
    apt-get update && \
    apt-get install -y --no-install-recommends \
        lib32z1 lib32gcc-s1 lib32stdc++6 libstdc++6 \
        ca-certificates curl && \
    rm -rf /var/lib/apt/lists/*

# The web binary (controller + bridge in-process + DS manager).
COPY --from=builder /build/controller/target/release/sc-rns-controller-web /usr/local/bin/sc-rns-controller-web

# Static frontend (served at / by the web binary).
COPY gui/dist /app/gui/dist

# /data = bundle dir: settings.json, RNS identity, steamcmd bootstrap.
# /data/svends = the dedicated server install (mounted as a separate volume so
# the 2.74 GB DS can be wiped/updated independently of settings + identity).
VOLUME ["/data", "/data/svends"]

ENV BUNDLE_DIR=/data \
    GUI_DIST_DIR=/app/gui/dist \
    WEB_PORT=8080 \
    RUST_LOG=sc_rns_controller=info,sc_rns_bridge=info,personal_rns=warn

# 8080 = web panel, 4966 = RNS TCP interface (peers), 27015/udp = DS (LAN).
EXPOSE 8080
EXPOSE 4966
EXPOSE 27015/udp

ENTRYPOINT ["sc-rns-controller-web"]
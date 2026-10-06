# syntax=docker/dockerfile:1.7

# ── ffmpeg: johnvansickle static build. Only ever remuxes local files (no
#    DNS/NSS), so its static glibc is fine. ──
FROM debian:trixie-slim AS ffmpeg
RUN apt-get update \
    && apt-get install -y --no-install-recommends curl xz-utils ca-certificates \
    && rm -rf /var/lib/apt/lists/*
RUN curl -sSL https://johnvansickle.com/ffmpeg/releases/ffmpeg-release-amd64-static.tar.xz \
        -o /tmp/ffmpeg.tar.xz \
    && mkdir /tmp/ff \
    && tar -xJf /tmp/ffmpeg.tar.xz -C /tmp/ff --strip-components=1 --wildcards '*/ffmpeg' \
    && mv /tmp/ff/ffmpeg /ffmpeg \
    && chmod 0755 /ffmpeg

# ── build: static musl binary (rustls, no OpenSSL). ──
FROM rust:1-alpine AS builder
WORKDIR /app
RUN apk add --no-cache musl-dev
ENV RUSTFLAGS="-C target-feature=+crt-static"

# Dependency layer, cached across source-only edits.
COPY Cargo.toml Cargo.lock ./
RUN mkdir src && echo 'fn main() {}' > src/main.rs \
    && cargo build --release --locked --target x86_64-unknown-linux-musl \
    && rm -rf src target/x86_64-unknown-linux-musl/release/deps/x_media_bot*

COPY assets ./assets
COPY src ./src
RUN cargo build --release --locked --target x86_64-unknown-linux-musl

# ── runtime: distroless static (CA certs, /tmp, nonroot user, no shell). ──
FROM gcr.io/distroless/static-debian13:nonroot
COPY --from=ffmpeg  /ffmpeg /usr/local/bin/ffmpeg
COPY --from=builder /app/target/x86_64-unknown-linux-musl/release/x-media-bot /usr/local/bin/x-media-bot
ENV RUST_LOG=info
ENTRYPOINT ["/usr/local/bin/x-media-bot"]

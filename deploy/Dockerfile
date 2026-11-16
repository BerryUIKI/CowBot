# CowBot server image.
#
# A two-stage build: the first compiles the release binary against the
# workspace, the second carries only the binary and a writable data directory.
# The server owns one SQLite file, so the image needs one volume, not a database
# service.

FROM rust:1-slim-bookworm AS build
WORKDIR /src

# A cold dependency build is the slow half; copying the manifests first lets a
# source-only change reuse the cached layer.
COPY Cargo.toml Cargo.lock ./
COPY crates ./crates
RUN cargo build --release --locked -p cowbot-server

FROM debian:bookworm-slim AS runtime

# sqlx links libsqlite3 dynamically, and the health check below uses curl.
RUN apt-get update \
    && apt-get install -y --no-install-recommends ca-certificates libsqlite3-0 curl \
    && rm -rf /var/lib/apt/lists/*

# A dedicated unprivileged user, since the process writes only its data volume.
RUN useradd --create-home --uid 10001 cowbot \
    && mkdir -p /data \
    && chown cowbot:cowbot /data

COPY --from=build /src/target/release/cowbot-server /usr/local/bin/cowbot-server

USER cowbot
WORKDIR /home/cowbot

# The database lives on the volume; the listen address is overridden by
# Compose so the port is reachable from outside the container.
ENV COWBOT_SERVER_DATABASE=/data/cowbot-server.db \
    COWBOT_SERVER_ADDRESS=0.0.0.0:8787 \
    RUST_LOG=cowbot_server=info

VOLUME ["/data"]
EXPOSE 8787

HEALTHCHECK --interval=30s --timeout=3s --start-period=10s --retries=3 \
    CMD curl -fsS http://127.0.0.1:8787/health || exit 1

ENTRYPOINT ["cowbot-server"]

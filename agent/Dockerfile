FROM rust:latest AS builder
WORKDIR /app

COPY Cargo.toml ./
RUN mkdir -p src && echo "fn main() {}" > src/main.rs
RUN cargo build --release || true

COPY src ./src
RUN cargo build --release

FROM debian:bookworm-slim
RUN apt-get update \
    && apt-get install -y --no-install-recommends ca-certificates docker.io \
    && rm -rf /var/lib/apt/lists/*
WORKDIR /app
COPY --from=builder /app/target/release/intercom-sync-agent /usr/local/bin/intercom-sync-agent
ENTRYPOINT ["/usr/local/bin/intercom-sync-agent"]

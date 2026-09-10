# EchoStream-AI server image (multi-stage).

FROM rust:1.83 AS build
WORKDIR /app
# Cache dependencies between builds when only sources change.
COPY server/Cargo.toml server/Cargo.lock* ./server/
RUN mkdir -p server/src/bin && \
    echo "pub fn placeholder() {}" > server/src/lib.rs && \
    echo "fn main() {}" > server/src/main.rs && \
    echo "fn main() {}" > server/src/bin/ingest.rs && \
    cd server && cargo build --release || true
COPY server ./server
RUN cd server && touch src/lib.rs src/main.rs src/bin/ingest.rs && cargo build --release

FROM debian:bookworm-slim
RUN apt-get update && apt-get install -y --no-install-recommends ca-certificates && rm -rf /var/lib/apt/lists/*
COPY --from=build /app/server/target/release/echostream-server /usr/local/bin/
COPY --from=build /app/server/target/release/echostream-ingest /usr/local/bin/
EXPOSE 8080
ENTRYPOINT ["echostream-server"]

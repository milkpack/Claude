# VectorCraft Cloud: the web app plus the collaboration server in one image.
#   docker build -f packaging/collab/Dockerfile -t vectorcraft-cloud .
#   docker run -p 1234:1234 -v vc-data:/data vectorcraft-cloud
# Then open http://localhost:1234/?room=team-poster in several browsers.

FROM rust:1-bookworm AS build
WORKDIR /src
RUN rustup target add wasm32-unknown-unknown \
 && cargo install trunk --locked
COPY . .
RUN cargo build --release -p vectorcraft-collab-server
RUN cd apps/vectorcraft-web && trunk build --release

FROM debian:bookworm-slim
LABEL org.opencontainers.image.source="https://github.com/milkpack/Claude" \
      org.opencontainers.image.description="VectorCraft Cloud: web app + collaboration server"
RUN apt-get update && apt-get install -y --no-install-recommends ca-certificates && rm -rf /var/lib/apt/lists/*
COPY --from=build /src/target/release/vectorcraft-collab-server /usr/local/bin/
COPY --from=build /src/dist/web /srv/web
VOLUME /data
EXPOSE 1234
ENV RUST_LOG=info
CMD ["vectorcraft-collab-server", "--host", "0.0.0.0", "--port", "1234", "--data", "/data", "--static", "/srv/web"]

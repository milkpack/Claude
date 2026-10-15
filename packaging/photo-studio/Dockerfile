# Photo Studio: the Rust server plus the built React front-end in one image.
#   docker build -f packaging/photo-studio/Dockerfile -t photo-studio .
#   docker run -p 8787:8787 -v ps-data:/data -e PHOTO_STUDIO_PASSWORD='at least 12 chars' photo-studio
# Then open http://localhost:8787. Production: packaging/photo-studio/deploy (HTTPS via Caddy).

# The front-end (apps/photo-studio/web): pnpm at the version pinned in package.json (corepack).
FROM node:22-bookworm-slim AS web
WORKDIR /web
ENV COREPACK_ENABLE_DOWNLOAD_PROMPT=0
RUN corepack enable
COPY apps/photo-studio/web/package.json apps/photo-studio/web/pnpm-lock.yaml ./
RUN pnpm install --frozen-lockfile
COPY apps/photo-studio/web/ ./
RUN pnpm build

# The server. build.rs compiles the photo skill (.claude/skills/photo-studio) into the binary,
# so the build context must include .claude (see .dockerignore).
FROM rust:1-bookworm AS build
WORKDIR /src
COPY . .
RUN cargo build --release -p photo-studio

FROM debian:bookworm-slim
LABEL org.opencontainers.image.source="https://github.com/milkpack/Claude" \
      org.opencontainers.image.description="Photo Studio: node canvas for photographic image generation (OpenArt + Claude)"
RUN apt-get update && apt-get install -y --no-install-recommends ca-certificates && rm -rf /var/lib/apt/lists/*
COPY --from=build /src/target/release/photo-studio /usr/local/bin/
COPY --from=web /web/dist /srv/web
VOLUME /data
EXPOSE 8787
ENV RUST_LOG=info
CMD ["photo-studio", "--host", "0.0.0.0", "--port", "8787", "--data", "/data", "--static", "/srv/web"]

# syntax=docker/dockerfile:1.7

FROM node:24.19.0-alpine AS web-builder
WORKDIR /source/web
RUN corepack enable
COPY web/package.json web/pnpm-lock.yaml ./
RUN --mount=type=cache,target=/root/.local/share/pnpm/store \
    pnpm install --frozen-lockfile
COPY web/ ./
RUN pnpm build

FROM rust:1.98.0-bookworm AS server-builder
RUN apt-get update \
    && apt-get install --yes --no-install-recommends libsqlite3-dev pkg-config \
    && rm -rf /var/lib/apt/lists/*
WORKDIR /source
COPY Cargo.toml Cargo.lock ./
COPY crates/ ./crates/
COPY migrations/ ./migrations/
RUN --mount=type=cache,target=/usr/local/cargo/registry \
    --mount=type=cache,target=/source/target \
    cargo build --locked --release --package indexloom-server \
    && cp /source/target/release/indexloom-server /tmp/indexloom-server

FROM debian:bookworm-slim AS runtime
ARG INDEXLOOM_GID=10001
ARG INDEXLOOM_UID=10001
LABEL org.opencontainers.image.title="IndexLoom" \
      org.opencontainers.image.description="Self-hosted indexer manager and federated search engine" \
      org.opencontainers.image.source="https://github.com/IndexLoom/IndexLoom" \
      org.opencontainers.image.licenses="AGPL-3.0-or-later" \
      org.opencontainers.image.version="0.1.0"
RUN apt-get update \
    && apt-get install --yes --no-install-recommends ca-certificates curl libsqlite3-0 \
    && rm -rf /var/lib/apt/lists/* \
    && groupadd --gid "${INDEXLOOM_GID}" indexloom \
    && useradd --uid "${INDEXLOOM_UID}" --gid indexloom --no-create-home --home-dir /nonexistent indexloom \
    && install --directory --owner=indexloom --group=indexloom /app/web /data
WORKDIR /app
COPY --from=server-builder /tmp/indexloom-server /usr/local/bin/indexloom-server
COPY --from=web-builder /source/web/dist/ /app/web/
COPY LICENSE /usr/share/licenses/indexloom/LICENSE
ENV INDEXLOOM_BIND=0.0.0.0:8080 \
    INDEXLOOM_DATABASE_URL=sqlite:///data/indexloom.db?mode=rwc \
    INDEXLOOM_WEB_ROOT=/app/web \
    RUST_LOG=indexloom=info,tower_http=info
USER indexloom:indexloom
EXPOSE 8080
VOLUME ["/data"]
HEALTHCHECK --interval=30s --timeout=3s --start-period=10s --retries=3 \
    CMD ["curl", "--fail", "--silent", "--show-error", "http://127.0.0.1:8080/api/v1/health"]
ENTRYPOINT ["/usr/local/bin/indexloom-server"]

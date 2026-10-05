# syntax=docker/dockerfile:1.7
# Release image: app-server + app-worker + built SPA on distroless (no shell, non-root).
#   docker build -f infra/docker/Dockerfile -t app:latest .
# Runtime config comes from APP__* environment variables (see .env.example); nothing secret is baked in.

ARG RUST_IMAGE=rust:1.99.0-bookworm
ARG NODE_IMAGE=node:26.10.0-bookworm-slim
ARG RUNTIME_IMAGE=gcr.io/distroless/cc-debian12:nonroot

FROM ${NODE_IMAGE} AS web
WORKDIR /src/frontend
COPY frontend/package.json frontend/package-lock.json ./
RUN --mount=type=cache,target=/root/.npm npm ci --no-audit --no-fund
COPY frontend/ ./
RUN npm run -s build

FROM ${RUST_IMAGE} AS server
WORKDIR /src/backend
ENV SQLX_OFFLINE=true CARGO_TERM_COLOR=never RUSTUP_TOOLCHAIN=1.99.0
ARG CARGO_FEATURES=""
COPY backend/ ./
# Cache mounts keep the registry and build artefacts between builds; binaries are copied out.
RUN --mount=type=cache,target=/usr/local/cargo/registry \
    --mount=type=cache,target=/src/backend/target \
    cargo build --release --locked -p app-server -p app-worker ${CARGO_FEATURES:+--features "$CARGO_FEATURES"} \
 && mkdir -p /out && cp target/release/app-server target/release/app-worker /out/

FROM ${RUNTIME_IMAGE}
WORKDIR /app
COPY --from=server /out/app-server /out/app-worker /app/
COPY --from=web /src/frontend/dist /app/web
COPY backend/config/ /app/config/
ENV APP__HTTP__HOST=0.0.0.0 \
    APP__HTTP__PORT=8080 \
    APP__HTTP__OPS_PORT=9090 \
    APP__HTTP__STATIC_DIR=/app/web \
    APP__LOG__FORMAT=json
# 8080: public (put TLS in front). 9090: internal ops (/metrics, detailed /readyz) — never publish.
EXPOSE 8080 9090
USER nonroot:nonroot
HEALTHCHECK --interval=10s --timeout=4s --start-period=20s --retries=3 CMD ["/app/app-server", "healthcheck"]
ENTRYPOINT ["/app/app-server"]

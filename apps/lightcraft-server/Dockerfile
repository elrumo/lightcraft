# syntax=docker/dockerfile:1
# LightCraft sync server + the web build it serves at /.
#
#   docker build -f apps/lightcraft-server/Dockerfile -t lightcraft-server .
#   docker run -d --name lightcraft -p 8080:8080 -v lightcraft-data:/data lightcraft-server
#   docker exec -i lightcraft lightcraft-server user add ann     # then type the password
#
# Plain HTTP on 8080: put a TLS reverse proxy (Caddy) in front, or reach it over Tailscale.
# See docs/sync.md.

# Multi-platform builds (`make push-server`): both build stages below run on the builder's own CPU
# (--platform=$BUILDPLATFORM) and cross-compile, so nothing is compiled under emulation. The
# browser bundle is the same for every platform, so BuildKit builds it once.
#
# The caches (BuildKit, survive between builds) have explicit ids on purpose: without one, every
# platform's build shares a single /src/target, cargo's lock then runs them one after the other,
# and one platform's cp can pick up the other's binary. Each build gets its own registry cache
# too: cargo's lock on it lives in CARGO_HOME, outside the mount, so two builds sharing one
# unpack the same crate at once ("File exists").

# wasm-bindgen-cli must match Cargo.lock's wasm-bindgen exactly. Only its version crosses into the
# next stage, so the (slow) install is redone when that changes, not on every Cargo.lock edit.
FROM --platform=$BUILDPLATFORM rust:1-bookworm AS wbg-version
COPY Cargo.lock .
RUN sed -n '/^name = "wasm-bindgen"$/{n;s/version = "\(.*\)"/\1/p;}' Cargo.lock > /wbg-version

FROM --platform=$BUILDPLATFORM rust:1-bookworm AS web
WORKDIR /src
RUN rustup target add wasm32-unknown-unknown
COPY --from=wbg-version /wbg-version /wbg-version
RUN cargo install wasm-bindgen-cli --locked --version "$(cat /wbg-version)"
COPY . .
# the bundle is target/web's files; its directories are the `web` profile's cargo output, not ours
RUN --mount=type=cache,id=lightcraft-registry-web,target=/usr/local/cargo/registry \
    --mount=type=cache,id=lightcraft-target-web,target=/src/target \
    cargo xtask web \
 && mkdir /out \
 && find target/web -maxdepth 1 -type f -exec cp {} /out/ \;

# The server is pure Rust, so a cross build needs only the target's linker and libc.
FROM --platform=$BUILDPLATFORM rust:1-bookworm AS server
RUN apt-get update \
 && apt-get install -y --no-install-recommends gcc-x86-64-linux-gnu libc6-dev-amd64-cross gcc-aarch64-linux-gnu libc6-dev-arm64-cross \
 && rm -rf /var/lib/apt/lists/* \
 && rustup target add x86_64-unknown-linux-gnu aarch64-unknown-linux-gnu \
 && printf '[target.x86_64-unknown-linux-gnu]\nlinker = "x86_64-linux-gnu-gcc"\n[target.aarch64-unknown-linux-gnu]\nlinker = "aarch64-linux-gnu-gcc"\n' >> "$CARGO_HOME/config.toml"
WORKDIR /src
COPY . .
ARG TARGETARCH
RUN --mount=type=cache,id=lightcraft-registry-$TARGETARCH,target=/usr/local/cargo/registry \
    --mount=type=cache,id=lightcraft-target-$TARGETARCH,target=/src/target \
    case "$TARGETARCH" in amd64) t=x86_64-unknown-linux-gnu ;; arm64) t=aarch64-unknown-linux-gnu ;; *) echo "unsupported platform: $TARGETARCH" >&2; exit 1 ;; esac \
 && cargo build --release --target "$t" -p lightcraft-server \
 && mkdir -p /out \
 && cp "target/$t/release/lightcraft-server" /out/

FROM debian:bookworm-slim
RUN useradd --system --uid 10001 --home-dir /data lightcraft && mkdir -p /data && chown lightcraft /data
COPY --from=server /out/lightcraft-server /usr/local/bin/lightcraft-server
COPY --from=web /out /srv/web
ENV LIGHTCRAFT_DATA=/data LIGHTCRAFT_LISTEN=0.0.0.0:8080 LIGHTCRAFT_WEB=/srv/web
VOLUME /data
EXPOSE 8080
USER lightcraft
HEALTHCHECK --interval=30s --timeout=5s --start-period=10s CMD ["lightcraft-server", "health"]
ENTRYPOINT ["lightcraft-server"]
CMD ["serve"]

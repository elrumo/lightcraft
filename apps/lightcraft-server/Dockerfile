# syntax=docker/dockerfile:1
# LightCraft sync server + the web build it serves at /.
#
#   docker build -f apps/lightcraft-server/Dockerfile -t lightcraft-server .
#   docker run -d --name lightcraft -p 8080:8080 -v lightcraft-data:/data lightcraft-server
#   docker exec -i lightcraft lightcraft-server user add ann     # then type the password
#
# Plain HTTP on 8080: put a TLS reverse proxy (Caddy) in front, or reach it over Tailscale.
# See docs/sync.md.

FROM rust:1-bookworm AS build
WORKDIR /src
RUN rustup target add wasm32-unknown-unknown
# the wasm-bindgen CLI must match Cargo.lock's wasm-bindgen exactly (its own layer: rebuilt only
# when Cargo.lock changes)
COPY Cargo.lock .
RUN cargo install wasm-bindgen-cli --locked --version "$(sed -n '/^name = "wasm-bindgen"$/{n;s/version = "\(.*\)"/\1/p;}' Cargo.lock)"
COPY . .
# the registry and target caches survive between builds (BuildKit), so a rebuild only compiles
# what changed
RUN --mount=type=cache,target=/usr/local/cargo/registry \
    --mount=type=cache,target=/src/target \
    cargo xtask web \
 && cargo build --release -p lightcraft-server \
 && mkdir -p /out \
 && cp target/release/lightcraft-server /out/ \
 && cp -r target/web /out/web

FROM debian:bookworm-slim
RUN useradd --system --uid 10001 --home-dir /data lightcraft && mkdir -p /data && chown lightcraft /data
COPY --from=build /out/lightcraft-server /usr/local/bin/lightcraft-server
COPY --from=build /out/web /srv/web
ENV LIGHTCRAFT_DATA=/data LIGHTCRAFT_LISTEN=0.0.0.0:8080 LIGHTCRAFT_WEB=/srv/web
VOLUME /data
EXPOSE 8080
USER lightcraft
HEALTHCHECK --interval=30s --timeout=5s --start-period=10s CMD ["lightcraft-server", "health"]
ENTRYPOINT ["lightcraft-server"]
CMD ["serve"]

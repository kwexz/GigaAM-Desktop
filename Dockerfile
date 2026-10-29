FROM rust:1.90-bookworm AS backend
RUN apt-get update && apt-get install -y --no-install-recommends libwebkit2gtk-4.1-dev libappindicator3-dev librsvg2-dev patchelf libssl-dev pkg-config && rm -rf /var/lib/apt/lists/*
WORKDIR /source
COPY src-tauri ./src-tauri
COPY vendor ./vendor
COPY build ./build
RUN cargo build --release --locked --manifest-path src-tauri/Cargo.toml --bin gigaam-server

FROM debian:bookworm-slim
RUN apt-get update && apt-get install -y --no-install-recommends nginx libwebkit2gtk-4.1-0 libayatana-appindicator3-1 libssl3 ca-certificates && rm -rf /var/lib/apt/lists/*
COPY --from=backend /source/src-tauri/target/release/gigaam-server /usr/local/bin/gigaam-server
COPY build /usr/share/nginx/html
COPY deploy/container-start.sh /usr/local/bin/container-start
RUN chmod +x /usr/local/bin/container-start && mkdir -p /data && chown -R www-data:www-data /data
ENV GIGAAM_SERVER_BIND=127.0.0.1:8787 GIGAAM_SERVER_DATA=/data GIGAAM_SERVER_MODELS=/models
EXPOSE 80
ENTRYPOINT ["/usr/local/bin/container-start"]

FROM node:24.11.1-bookworm AS node
FROM rust:1.85.0-bookworm
COPY --from=node /usr/local/bin/node /usr/local/bin/node
RUN rustup component add clippy rustfmt
WORKDIR /src
# Mount the checkout read-only, /out writable, and Cargo build/cache volumes.
ENV CARGO_TARGET_DIR=/build
ENTRYPOINT ["node", "scripts/package-native.mjs", "--output", "/out/candidate"]

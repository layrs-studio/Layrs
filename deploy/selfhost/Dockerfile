FROM rust:1.90.0-bookworm AS build
WORKDIR /src
COPY Cargo.toml Cargo.lock ./
COPY crates ./crates
RUN cargo build --locked --release -p layrs-server

FROM postgres:17.6-bookworm
RUN apt-get update && apt-get install -y --no-install-recommends ca-certificates \
    && rm -rf /var/lib/apt/lists/* \
    && groupadd --gid 10001 layrs && useradd --uid 10001 --gid layrs --no-create-home layrs \
    && mkdir -p /var/lib/layrs/cas && chown -R layrs:layrs /var/lib/layrs
COPY --from=build /src/target/release/layrs-server /usr/local/bin/layrs-server
USER 10001:10001
EXPOSE 8787
ENTRYPOINT ["layrs-server"]
CMD ["serve"]

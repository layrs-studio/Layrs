FROM rust:1.90.0-bookworm
RUN rustup component add clippy
WORKDIR /src
# Source is mounted read-only by the operator. Cargo outputs and cache live in
# Linux Docker volumes, never in the Windows target directory.
ENV CARGO_TARGET_DIR=/build
CMD ["cargo", "test", "--locked", "-p", "layrs-client-core", "--test", "native_materialize_types", "--test", "native_capture", "--test", "native_workspace", "--test", "native_rewrite", "--test", "native_merge_accept", "--test", "native_history"]

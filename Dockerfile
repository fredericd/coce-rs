## --- build stage ---
FROM rust:1-slim-bookworm AS builder
WORKDIR /app

COPY Cargo.toml Cargo.lock ./
COPY src ./src
RUN cargo build --release

## --- runtime stage ---
FROM debian:bookworm-slim
RUN useradd --system --create-home --shell /usr/sbin/nologin coce
WORKDIR /home/coce

COPY --from=builder /app/target/release/coce /usr/local/bin/coce

USER coce
EXPOSE 8080
ENTRYPOINT ["/usr/local/bin/coce"]

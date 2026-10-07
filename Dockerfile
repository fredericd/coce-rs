## --- build stage ---
# Alpine's Rust targets musl, which links fully static binaries by default:
# the result runs on an empty (scratch) image.
FROM rust:1-alpine AS builder
RUN apk add --no-cache musl-dev
WORKDIR /app

COPY Cargo.toml Cargo.lock ./
COPY src ./src
RUN cargo build --release

## --- runtime stage ---
# No OS at all: TLS roots are compiled in (webpki-roots), and Docker provides
# /etc/resolv.conf and /etc/hosts at run time. No shell either, so
# `docker compose exec coce sh` is not available, but `exec coce coce ...` is.
FROM scratch
COPY --from=builder /app/target/release/coce /usr/local/bin/coce
ENV PATH=/usr/local/bin
WORKDIR /home/coce

# No /etc/passwd in scratch: run as a numeric, unprivileged user.
USER 10001:10001
EXPOSE 8080
ENTRYPOINT ["/usr/local/bin/coce"]

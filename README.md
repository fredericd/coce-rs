# Coce (Rust)

Rust rewrite of [coce](../README.md), the book cover URL cache server.

## Build & run

```sh
cargo build --release
cp config.json.sample config.json   # then adjust Redis host/port, etc.
COCE_CONFIG=config.json ./target/release/coce
```

By default `config.json` is read from the current directory; the
`COCE_CONFIG` environment variable points to a different path.

## Architecture

- `config.rs` — loading/typing of `config.json` (`serde_json`, no `eval`)
- `redis_store.rs` — thin wrapper around `redis::aio::ConnectionManager`
- `providers/` — one module per provider (`aws`, `gb`, `ol`, `orb`), each
  exposing a `fetch(ids, ...) -> HashMap<id, url>` function
- `fetcher.rs` — orchestration: checks the Redis cache, calls the missing
  providers in parallel, enforces a global timeout, writes results (and
  misses) back to Redis
- `http.rs` — Axum routes (`/`, `/cover`, `/set`)
- `error.rs` — HTTP errors (JSON `{"error": ...}` responses)

## Deployment

In production, supervision could be delegated to systemd.

Example unit file, e.g. `/etc/systemd/system/coce.service`:

```ini
[Unit]
Description=coce - book cover URL cache server
After=network.target redis.service
Wants=redis.service

[Service]
Type=simple
User=coce
WorkingDirectory=/opt/coce
Environment=COCE_CONFIG=/opt/coce/config.json
ExecStart=/opt/coce/target/release/coce
Restart=on-failure
RestartSec=2

[Install]
WantedBy=multi-user.target
```

Adjust `User`, `WorkingDirectory` and `ExecStart` to your deployment path,
then:

```sh
sudo systemctl daemon-reload
sudo systemctl enable --now coce
sudo systemctl status coce
journalctl -u coce -f   # logs
```

### Docker

Configuration can come from `config.json`, from `COCE_*` environment
variables, or both — env vars always win. This is the friendliest option for
containers: no file to mount, values can be injected as env vars/secrets
straight from `docker run`, `docker-compose.yml` or an orchestrator. See
`.env.sample` for the full list of variables (they mirror `config.json.sample`
one for one). Nested provider settings (`orb.user`, `orb.timeout`, ...) stay
as `COCE_ORB_USER`, `COCE_ORB_TIMEOUT`, etc.

```sh
cp .env.sample .env   # fill in what you need, e.g. COCE_ORB_USER/COCE_ORB_KEY
docker compose up --build
```

`docker-compose.yml` starts Redis alongside coce and points `COCE_REDIS_HOST`
at the `redis` service; every other variable comes from `.env`. To run the
image standalone against an external Redis instead:

```sh
docker build -t coce .
docker run -p 8080:8080 \
  -e COCE_REDIS_HOST=redis.example.org \
  -e COCE_PROVIDERS=aws,gb,ol \
  coce
```

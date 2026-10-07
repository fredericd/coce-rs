# Deployment

## Building

```sh
cargo build --release
cp config.json.sample config.json   # then adjust, see configuration.md
./target/release/coce
```

The binary, `target/release/coce`, has no runtime dependency besides a Redis
server. `coce` (or `coce serve`) runs the server; `coce --help` lists the
other commands (`stats`, `cache-check`) and options, `coce --version` prints
the version. See [configuration](configuration.md) for the settings.

## systemd

In production, supervision could be delegated to systemd.

Example unit file, e.g. `/etc/systemd/system/coce.service`:

```ini
[Unit]
Description=Coce - book cover URL cache server
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

See [logging](logging.md) for log levels and formats.

## Docker

```sh
cp .env.sample .env   # fill in what you need, e.g. COCE_ORB_USER/COCE_ORB_KEY
docker compose up -d --build
```

`docker-compose.yml` runs two services: `redis` (with its data in the
`redis-data` volume) and `coce`, both restarted automatically.

### Configuration: `.env` only

Under Docker, coce is configured by `COCE_*` environment variables, read from
`.env`. `config.json` is **not** used: `.dockerignore` keeps it out of the
image and `docker-compose.yml` does not mount it, so coce starts from its
built-in defaults with `.env` applied on top. `.env.sample` lists every
variable; they mirror `config.json.sample` one for one (`orb.user` becomes
`COCE_ORB_USER`, and so on).

Two values are set by Compose rather than by `.env`:

- `COCE_REDIS_HOST`/`COCE_REDIS_PORT` are forced to the `redis` service, the
  only Redis reachable on the Compose network.
- The published port is `COCE_PORT` from `.env` (8080 if unset): Compose
  reads `.env` to build the `ports:` mapping, so the port coce listens on and
  the port Docker exposes always match. Change it in `.env`, never in
  `docker-compose.yml`.

Mounting a `config.json` into the container (at `/home/coce/config.json`)
would technically work, but env vars win over it and Compose cannot read the
port from it, so a port set there would not be published. Stick to `.env`.

To run the image standalone against an external Redis:

```sh
docker build -t coce .
docker run -p 8080:8080 \
  -e COCE_REDIS_HOST=redis.example.org \
  -e COCE_PROVIDERS=aws,gb,ol \
  coce
```

### Image design

The image is built in two stages: `rust:1-alpine` compiles coce, and only
the resulting binary is copied into an empty `scratch` image. The result is
about 6–8 MB (depending on the architecture), against ~80 MB for the same
binary on `debian:bookworm-slim`.

- **Static musl binary.** Alpine's Rust toolchain targets musl, which links
  everything statically: the binary needs no system library, which is what
  makes `scratch` possible.
- **mimalloc.** musl's own memory allocator collapses under multi-threaded
  load, so musl builds use mimalloc instead (`Cargo.toml`, `src/main.rs`;
  other builds keep the system allocator). In a benchmark of cached `/cover`
  requests on 4 cores, musl without mimalloc served 3 to 5 times fewer
  requests than glibc; with mimalloc it served ~65% more than glibc (Debian
  or distroless) from 50 concurrent requests up. The trade-off is memory:
  ~40 MB resident after heavy load, against ~15 MB on glibc.
- **No OS files needed.** TLS root certificates (Mozilla's list, from the
  `webpki-roots` crate) are compiled into the binary, as they already were
  on Debian: the system `ca-certificates` was never used. Docker provides
  `/etc/resolv.conf` and `/etc/hosts` for DNS at run time. See
  [Updating dependencies and TLS roots](#updating-dependencies-and-tls-roots).
- **Unprivileged user.** `scratch` has no `/etc/passwd`, so coce runs as
  numeric UID/GID `10001`. A mounted directory coce must write to (e.g.
  `COCE_CACHE_PATH`) has to be writable by that UID.
- **No shell, no tools.** `docker compose exec coce coce stats` works;
  `docker compose exec coce sh`, `ls`, `curl`, etc. do not exist in the
  container. Debug from the host (`docker compose logs`, `curl` on the
  published port) or with `docker compose exec redis redis-cli`.
- **UTC.** There is no time zone database in the image, so dates (logs,
  `/stats/daily`) are in UTC.

### Updating dependencies and TLS roots

`Cargo.lock` pins every dependency, the TLS root list included, so
`docker compose build` alone always rebuilds the same thing. Updating means
bumping the lock file:

```sh
cargo update              # or `cargo update -p webpki-roots` for the roots only
cargo test
git commit -am "Update dependencies" && git push
# then on the server:
git pull && docker compose up -d --build
```

The root list only matters when a change affects a CA used by one of the
providers coce calls: a provider moving to a new CA, a root expiring, or
Mozilla distrusting one. Roots are long-lived and such changes are rare, so
there is no hard deadline, but updating every few months, or with each coce
release, is a good habit. A full `cargo update` also brings security fixes in
`rustls`, `reqwest` and the rest, which matter more. A stale root list shows
up as one provider failing every call: its failures climb in `/stats`, it
gets disabled, and the logs show a certificate error.

## Behind NGINX

Coce is meant to run behind a reverse proxy, which terminates TLS, serves
the local image copies, and protects the service. Since a single Coce
instance usually serves many OPACs, the proxy is also where one client is
kept from degrading the service for the others. Example:

```nginx
# http {} context
limit_req_zone $binary_remote_addr zone=coce_per_ip:10m rate=5r/s;

log_format coce '$remote_addr [$time_local] "$request" $status $body_bytes_sent '
                'rt=$request_time urt=$upstream_response_time '
                'origin="$http_origin" referer="$http_referer" ua="$http_user_agent"';

upstream coce {
    server 127.0.0.1:8080;
    keepalive 32;
}

server {
    listen 443 ssl;
    server_name coce.example.org;
    # ssl_certificate / ssl_certificate_key ...

    server_tokens off;
    keepalive_requests 1000;
    access_log /var/log/nginx/coce.access.log coce;

    proxy_http_version 1.1;
    proxy_set_header Connection "";
    proxy_set_header Host $host;

    location / {
        proxy_pass http://coce;
    }

    location = /cover {
        limit_req zone=coce_per_ip burst=20 nodelay;
        limit_req_status 429;
        proxy_pass http://coce;
    }

    location = /stats {
        allow 10.0.0.0/8;
        deny all;
        proxy_pass http://coce;
    }

    # Local image copies (providers with `cache: true`): `cache.path`,
    # served at `cache.url`
    location /covers/ {
        alias /var/lib/coce/covers/;
        expires 30d;
    }
}
```

* `keepalive` (upstream), `proxy_http_version 1.1` and an empty
  `Connection` header keep the connections between NGINX and Coce open,
  instead of opening one per request.
* `keepalive_requests 1000`: before NGINX 1.19.10, a client connection is
  closed after 100 requests by default, forcing a new TLS handshake.
* `server_tokens off` stops advertising the NGINX version.
* The `coce` log format records the time spent in Coce (`urt`) next to the
  total time (`rt`), which tells Coce's delays from network ones, and the
  calling OPAC (`origin`, `referer`), which tells which OPAC generates the
  load or the errors.
* `limit_req` caps each client IP on `/cover`: a crawler hammering an OPAC
  gets `429` instead of filling Coce with lookups, and can no longer trip a
  provider's circuit breaker for every OPAC. Size it generously: a whole
  library, or a university, often reaches Coce through a single NAT
  address, and Koha calls Coce through JSONP, so a client over the limit
  just gets pages without covers, silently.
* `/stats` is kept internal (see [administration](administration.md#stats)).

## Redis availability

Redis is only a cache: if it goes down while Coce is running, Coce keeps
answering, bypassing the cache and querying providers directly. Responses
are slower and providers are queried more, but covers are still displayed.
`/stats` reports it (`"redis": {"reachable": false}`), so it can be
monitored. Coce reconnects on its own when Redis is back.

For this degraded mode to stay short and cheap:

* have Redis restarted automatically (`Restart=` in its systemd unit,
  `restart:` in `docker-compose.yml`, which the provided one already does)
* keep Redis persistence on (RDB snapshots, Redis' default, or AOF), so
  that the cache survives a Redis restart instead of starting empty
* keep `redis.timeout` short (see [configuration](configuration.md#redis-redis))

Caveat: Coce needs Redis at startup. If Redis is unreachable, Coce retries
for a few seconds, then exits with an error; with `Restart=on-failure`
systemd keeps restarting it until Redis is up.

## Several instances

Several Coce servers can share one Redis server, typically behind a load
balancing NGINX `upstream`. The cover cache is then shared: an ISBN looked
up by one server is served from cache by the others. Points to watch:

* **Local image copies** (`cache: true`): each server downloads images to
  its own disk, but stores their URL in the shared Redis. A server can then
  return the URL of a file that only exists on another server's disk, and
  the image is broken whenever `/covers/` is served from the wrong one. Put
  `cache.path` on storage shared by all servers (e.g. NFS), served from
  there. Run `coce cache-check` only where that shared directory is visible:
  run against a server's private directory, `--fix` would delete the keys
  of files held by the others.
* **Same configuration** on all servers, at least what shapes the cached
  URLs and durations (providers' `timeout` and `imageSize`, `cache`,
  `cache.url`, `notFoundTimeout`): otherwise one Redis holds URLs built in
  different ways.
* **Same time zone** on all servers: the daily history (`/stats/daily`,
  `coce stats`) adds up the counts of all servers per local day, and
  servers in different time zones would split days differently.
* **`/stats` is per server**: behind a load balancer, each call answers
  for whichever server receives it. Use `/stats/daily` or `coce stats` for
  figures covering all servers (see [administration](administration.md)). Circuit breakers are per server too, each
  disabling a failing provider on its own.
* Two servers looking up the same uncached ISBN at the same time both call
  the provider; harmless, the answer is just cached twice.

## Stopping Coce

On SIGTERM (sent by `systemctl stop` and `docker stop`) or Ctrl-C, Coce
stops accepting connections, finishes the requests in progress, then waits
for its background work (cache writes, providers still running after the
global timeout) for at most `timeout` before exiting. Keep the stop timeout
of the service manager above `timeout`: systemd's default (90 s) is, Docker's
(10 s) is too with the default `timeout` of 8 s.

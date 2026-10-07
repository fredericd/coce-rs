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
at the `redis` service; every other variable comes from `.env`. The published
port follows `COCE_PORT` from `.env` (8080 if unset), so change it there, not
in `docker-compose.yml`. `config.json` is not copied into the image, so under
Docker the port must be set in `.env`.

The image is a static (musl) binary on `scratch`, about 8 MB. It has no
shell or OS tools: `docker compose exec coce coce stats` works, `docker
compose exec coce sh` does not. Dates in `/stats/daily` are in UTC.

To run the
image standalone against an external Redis instead:

```sh
docker build -t coce .
docker run -p 8080:8080 \
  -e COCE_REDIS_HOST=redis.example.org \
  -e COCE_PROVIDERS=aws,gb,ol \
  coce
```

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

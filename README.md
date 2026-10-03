# Coce (Rust)

A cover image URLs cache exposing its content as a REST web service.

[![MIT
License](https://img.shields.io/badge/license-MIT-blue.svg)](https://opensource.org/licenses/MIT)

In various softwares (ILS for example), Book (or other kind of resources)
cover image is displayed in front of the resource. Those images are fetched
automatically from providers, such as Google or Amazon. Providers propose web
services to retrieve information on Books from an ID (ISBN for example).

* [Google API](https://developers.google.com/books/docs/dynamic-links)
* [Amazon Product Advertising
  API](https://affiliate-program.amazon.com/gp/advertising/api/detail/main.html)
* [Open Library Search API](https://openlibrary.org/dev/docs/api/search) and
  [Covers API](https://openlibrary.org/dev/docs/api/covers)
* [ORB](https://www.base-orb.fr)

With Coce, the cover images URL from various providers are cached in a Redis
server. Client send REST request to Coce which reply with cached URL, or if not
available in its cache retrieving them from providers. In its request, the
client specify a providers order (for example `aws,gb,ol` for AWS, Google, and
then Open Library): Coce send the first available URL. It's also possible to not
only cache images URL but images themselves.

## Build & run

```sh
cargo build --release
cp config.json.sample config.json   # then adjust Redis host/port, etc.
./target/release/coce
```

`coce` (or `coce serve`) runs the server. By default `config.json` is read
from the current directory; `--config <file>` (`-c`), or the `COCE_CONFIG`
environment variable, points to a different path, the option winning over
the variable. `coce --help` lists the commands and options, `coce --version`
prints the version.

* __Configure__ Coce operation by editing
  [config.json](https://github.com/fredericd/coce-rs/blob/master/config.json.sample)
  Start with provided `config.json.sample` file.
  * `port` - port on which the server respond
  * `providers` - array of available providers: gb,aws,ol
  * `timeout` - timeout in miliseconds for the service. Above this value, Coce
    stops waiting response from providers
  * `providerTimeout` - timeout in milliseconds of each HTTP request to a
    provider (default 5000). Keep it lower than `timeout`: a provider that
    doesn't answer within this delay counts as failing (see [Provider
    failures](#provider-failures))
  * `providerRetry` - how long in seconds a failing provider stays disabled
    before Coce tries it again (default 300)
  * `notFoundTimeout` - how long in seconds a "no cover" answer stays cached
    (default 86400, one day). Covers found stay cached for their provider's
    `timeout`. The two are independent: a found cover URL rarely changes
    and can be kept long (e.g. 30 days, `2592000`), while a book without a
    cover today, typically a new title, often gets one within days, so its
    "no cover" answer is better kept short
  * `statsDays` - days of activity history kept in Redis, for
    `/stats/daily` and `coce stats` (default 30)
  * `setToken` - secret required by `/set` (see [Forcing a cover
    URL](#forcing-a-cover-url-set)). `/set` is disabled when unset or empty
  * `maxIds` - maximum number of IDs accepted in a single `/cover` request
    (default 100). Above it, Coce answers `400` with
    `{"error": "Too many IDs, maximum is 100"}` rather than silently
    dropping IDs: the client should split its request
  * `redis` - Redis server parameters:
     * `host`
     * `port`
     * `timeout` - timeout in milliseconds for Redis reads and writes
       (default 500). When Redis is slow or down, Coce bypasses the cache and
       queries providers directly instead of waiting. Every request pays this
       delay while Redis doesn't answer, so keep it short: a Redis on the
       same network answers in about a millisecond
  * `cache` - Local cache for images
    * `path` - path to the directory where images are cached locally
    * `url` - base url to the `path` directory
  * `gb` - Google Books parameters:
     * `timeout` - how long in seconds a cover found by Google Books stays
       cached (default 86400, one day, also when set to 0)
  * `ol` - Open Library parameters:
     * `timeout` - timeout of the cached URL from Open Library. After this
       delay, an URL is automatically removed from the cache, and so has to be
       re-fetched again if requested
     * `imageSize` - size of images: small, medium, large

     ISBNs are looked up in batch through the Open Library Search API, and
     the cover of the edition matching each ISBN is returned as a Covers API
     URL by cover ID (`https://covers.openlibrary.org/b/id/<id>-M.jpg`),
     which is not rate limited. Known limitation: when several edition
     records share the same ISBN, Open Library's search returns only one of
     them, so a cover attached to another duplicate record is missed.
  * `aws` - Amazon
     * `timeout` - timeout of the cached URL from Amazon, in seconds (same
       meaning as for Open Library). Images are always medium-sized

     Amazon image URLs are keyed by ISBN-10 or ASIN. A 979-prefixed ISBN-13
     has no ISBN-10 equivalent, and its ASIN can't be derived from it, so
     Amazon is never queried for these ISBNs (it would return the cover of an
     unrelated book). 979-prefixed ISBNs are increasingly common, as the 978
     range runs out (France uses 979-10, the US 979-8): always list other
     providers after `aws`, e.g. `aws,gb,ol`, so they can supply those
     covers.
  * `orb` - ORB
     * `user` - user to access ORB API
     * `key` - API key
     * `imageSize` - `thumbnail` (default, 160 px high) or `original` (500
       px high, for larger display; falls back to the thumbnail when ORB has
       no original)
     * `cache` - true/false, are images locally cached (and served)
     * `timeout` - timeout of the cached URL from ORB, in seconds (same
       meaning as for Open Library)

### Provider failures

Coce tells apart a provider answering "no cover for this ID", which is
cached like a found cover, from a provider failing to answer: network error,
timeout (`providerTimeout`), throttling (429), server error (5xx), unexpected
response. A failure is never cached, so the ID is looked up again once the
provider is back, instead of being reported as coverless for the whole cache
duration.

Each provider has its own circuit breaker. After 3 consecutive failed calls,
the provider is disabled for `providerRetry` seconds: Coce stops calling it
and answers with the other requested providers, without waiting. When the
delay is over, a single request tests the provider again: on success it is
re-enabled, otherwise it stays disabled for another `providerRetry` seconds.
A 400 Bad Request doesn't count as a failure, since it can be caused by the
IDs sent rather than by the provider.

The breaker state is kept in memory, per Coce instance, and is reset on
restart.

### Changing settings in production

Coce caches the URL it built, not the settings it built it with: the Redis
key (`ol.9780563533191`) only depends on the provider and the ISBN. After
changing one of these settings, the URLs already cached keep the old form
until they expire, which can take the provider's whole `timeout`:

* `imageSize` of Open Library (`ol`) or ORB (`orb`)
* `cache` of a provider (local copies on or off)
* `cache.url` (address the local copies are served from)

To apply such a change at once, purge the keys of the provider concerned
(here `orb`) after restarting Coce:

```sh
redis-cli --scan --pattern 'orb.*' | xargs redis-cli del
```

With local copies (`cache: true`), the image files are named after the ISBN
only (`<cache.path>/orb/<isbn>.jpg`), whatever their size, and never expire:
after changing `imageSize`, also empty the provider's directory, otherwise
the old images keep being served until each ISBN is looked up again.

```sh
rm -f /path/to/covers/orb/*.jpg
```

Other settings (timeouts, `maxIds`, provider order...) take effect on
restart, with no purge needed.

### Checking local copies: `coce cache-check`

For providers with `cache: true`, Redis and the image directory can drift
apart: a download that failed while Redis already points to the local file,
an error page saved as an image, keys expired or purged while files remain,
a change of `cache.url`... `coce cache-check` compares them, using the same
configuration as the server (`COCE_CONFIG`, `COCE_*` variables):

```sh
coce cache-check                      # report only
coce cache-check --provider orb --verbose
coce cache-check --fix                # apply the fixes
coce cache-check --fix --restore      # also recreate missing keys
```

| Case | `--fix` |
|---|---|
| key points to a missing local file | delete the key: the ISBN is looked up and downloaded again on next request |
| file isn't an image (empty, HTML error page...) | delete the file, and its key if local |
| key holds the provider's remote URL, file on disk | point the key to the local file |
| key holds a local URL with an old `cache.url` | point the key to the current one |
| file on disk, no key | recreate the key, only with `--restore` |
| URL forced by `/set`, "no cover" key, file not named by ISBN-13 | reported only |

**When to use `--restore`.** A file without key is normal: its key simply
expired, and if the ISBN is requested again, Coce looks it up and downloads
the image again. Leaving such files alone is always safe. `--restore` is for
one situation: Redis lost its content (flushed, or restarted without
persistence) while the image directory is intact. Recreating the keys from
the files then saves a provider call and a download per ISBN. Don't use it
in routine runs (e.g. a nightly `--fix`): it would also bring back files no
longer worth keeping, such as images whose provider has since removed the
cover. And never after an `imageSize` change: files keep the size they were
downloaded with, so restoring would put the old size back in service; empty
the directory instead.

The command scans all the provider's keys (`SCAN`), which is fine as an
occasional or nightly job. Exit code: 0 when nothing needs fixing (or with
`--fix`), 1 when fixes are needed, 2 on usage error, so that it can be used
in a cron job or a monitoring check. With Docker:
`docker compose exec coce coce cache-check`.

## Service usage

To get all cover images from Open Library (ol), Google Books (gb), and Amazon
(aws) for several ISBN:

    http://coce.server/cover?id=9780415480635,9780821417492,2847342257,9780563533191&provider=ol,gb,aws&all

ISBNs can be given as ISBN-10 or ISBN-13, with or without hyphens: Coce
looks each book up once, under its ISBN-13, and caches it under that form,
so all spellings share one cache entry. Results are keyed by the IDs exactly
as requested. IDs that aren't ISBNs are passed through unchanged.

This request returns:

```json
{
  "2847342257": {
    "aws": "https://images-na.ssl-images-amazon.com/images/I/51LYLJRtthL._SL160_.jpg"
  },
  "9780563533191": {
    "ol": "https://covers.openlibrary.org/b/id/2520432-M.jpg",
    "gb": "https://books.google.com/books/content?id=OphMAAAACAAJ&printsec=frontcover&img=1&zoom=1",
    "aws": "https://images-na.ssl-images-amazon.com/images/I/412CFNG0QEL._SL160_.jpg"
  },
  "9780821417492": {
    "gb": "https://books.google.com/books/content?id=D5yimAEACAAJ&printsec=frontcover&img=1&zoom=1",
    "aws": "https://images-na.ssl-images-amazon.com/images/I/417jg7TjvYL._SL160_.jpg"
  }
}
```

Without the `&all` parameter, the same request returns first URL per ISBN, by
provider order:

    http://coce.server/cover?id=9780415480635,9780821417492,2847342257,9780563533191&provider=ol,gb,aws

returns:

```json
{
  "2847342257": "https://images-na.ssl-images-amazon.com/images/I/51LYLJRtthL._SL160_.jpg",
  "9780563533191": "https://covers.openlibrary.org/b/id/2520432-M.jpg",
  "9780415480635": "https://books.google.com/books/content?id=Yc30cofv4_MC&printsec=frontcover&img=1&zoom=1",
  "9780821417492": "https://books.google.com/books/content?id=D5yimAEACAAJ&printsec=frontcover&img=1&zoom=1"
}
```

By adding a callback JavaScript function to the request, Coce returns its result
as JSONP:

    http://coce.server/cover?id=9780415480635,9780821417492,2847342257,9780563533191&provider=ol,gb,aws&callback=populateImg

returns:

The callback must be a JavaScript function name (letters, digits, `_`,
`$`, `.`), otherwise Coce answers `400`. JSONP predates CORS, which Coce
supports: new clients should call Coce with `fetch()` and read plain JSON
instead.

```jsonp
populateImg({"2847342257":"https://images-na.ssl-images-amazon.com/images/I/51LYLJRtthL._SL160_.jpg","9780563533191":"https://covers.openlibrary.org/b/id/2520432-M.jpg","9780415480635":"https://books.google.com/books/content?id=Yc30cofv4_MC&printsec=frontcover&img=1&zoom=1","9780821417492":"https://books.google.com/books/content?id=D5yimAEACAAJ&printsec=frontcover&img=1&zoom=1"})
```

### Forcing a cover URL: `/set`

`/set` stores a cover URL for an ID and a provider, for 10 years, e.g. to
fix a wrong cover. It requires the `setToken` configured on the server, sent
as a bearer token (a header rather than a URL parameter, which would end up
in access logs):

```sh
curl -H "Authorization: Bearer $COCE_SET_TOKEN" \
  "http://coce.server/set?provider=ol&id=9780563533191&url=https://example.org/cover.jpg"
```

It answers `{"success": true}`, `401` for a missing or bad token, `400` for
a provider that isn't configured or a URL that isn't http(s), `503` if Redis
can't be written to. When no `setToken` is configured (the default), `/set`
is disabled and answers `403`: anyone could otherwise replace any cover.

## Client-side usage

See `sample-client.html` in `client-sample` directory for a Coce sample usage
from JavaScript. It uses `coceclient.js` module, which is use like this:

```javascript
document.addEventListener('DOMContentLoaded', () => {
  const ids = [...document.querySelectorAll('[id^="coce-thumbnail"]')].map(
    (el) => el.dataset.id,
  );
  if (ids.length === 0) return;

  const client = new CoceClient('http://localhost:8080', 'ol,gb,aws');
  client
    .fetch(ids, (id, url) => {
      for (const el of document.querySelectorAll(`[data-id="${id}"]`)) {
        const img = document.createElement('img');
        img.src = url;
        img.title = url;
        el.append(img);
      }
    })
    .catch((err) => console.error('coce fetch failed:', err));
});
```

## Performance

A `/cover` request takes one of two very different paths:

* **IDs in cache**: one Redis read per requested provider, no outside call.
  Coce answers in about a millisecond and serves a high request rate.
* **IDs not in cache**: Coce queries the providers, so the response time is
  theirs, from a few hundred milliseconds to several seconds, bounded by
  `timeout`. This is the cost the cache saves on every later request for the
  same IDs.

Example, measured on an Apple M5 laptop (10 cores), with Coce built in
release mode and Redis 8 running on the same machine, for a request of 4
ISBNs on 2 providers:

| Scenario | Result |
|---|---|
| first request, cache empty | 414 ms |
| same request, cache warm | 1.4 ms |
| `ab -k -n 300000 -c 50`, cache warm (3 runs of about 3 s) | 97,500 to 98,800 requests/s, 99% under 1 ms |
| same without keep-alive (`ab -n 10000 -c 50`) | 38,900 requests/s, 99% under 3 ms |

These figures only illustrate the order of magnitude: they depend on the
machine, and here the load generator, Redis and Coce shared the same
processor (Coce used about 5 of the 10 cores). Measure on your own server,
ideally through your reverse proxy.

To reproduce:

```sh
cargo build --release
./target/release/coce &

URL='http://127.0.0.1:8080/cover?id=9780415480635,9780821417492,2847342257,9780563533191&provider=gb,aws'
curl -s "$URL"                  # fill the cache first
ab -k -n 300000 -c 50 "$URL"    # cached path
```

## Deployment

### systemd

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

See [Logging](#logging) for log levels and formats.

### Stopping Coce

On SIGTERM (sent by `systemctl stop` and `docker stop`) or Ctrl-C, Coce
stops accepting connections, finishes the requests in progress, then waits
for its background work (cache writes, providers still running after the
global timeout) for at most `timeout` before exiting. Keep the stop timeout
of the service manager above `timeout`: systemd's default (90 s) is, Docker's
(10 s) is too with the default `timeout` of 8 s.

### Redis availability

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
* keep `redis.timeout` short (see above)

Caveat: Coce needs Redis at startup. If Redis is unreachable, Coce retries
for a few seconds, then exits with an error; with `Restart=on-failure`
systemd keeps restarting it until Redis is up.

### Behind NGINX

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
* `/stats` is kept internal (see [Monitoring](#monitoring)).

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

## Monitoring

`/stats` returns activity counters, provider state and Redis figures, as
JSON. It requires no authentication: when Coce runs behind a reverse proxy,
restrict it there if needed (it reveals the configuration and traffic
volume, but no credentials).

Example:

```json
{
  "version": "0.1.0",
  "started_at": "2026-09-29T20:33:35+02:00",
  "uptime_s": 3,
  "config": { "providers": ["aws", "gb", "ol"], "timeout_ms": 8000,
              "provider_timeout_ms": 5000, "provider_retry_s": 300,
              "max_ids": 100, "redis": "127.0.0.1:6379", "local_cache": false },
  "requests": { "cover": 7, "rejected": 2, "ids": 5, "global_timeouts": 0 },
  "providers": {
    "gb": { "state": "disabled", "retry_in_s": 300, "consecutive_failures": 3,
            "cache_hits": 0, "cache_misses": 5, "cache_hit_rate": 0.0,
            "calls": 3, "failures": 3, "skipped": 2, "covers_found": 0,
            "avg_call_ms": 1003, "min_call_ms": 1001, "max_call_ms": 1004 },
    ...
  },
  "redis": { "reachable": true, "latency_ms": 0, "keys": 8, "used_memory": "991.89K" }
}
```

* `requests` - `/cover` requests received, rejected with a 400, IDs
  requested, requests answered at the global timeout with partial results
* `providers` - per provider:
  * `state` - `enabled`, `disabled` (by its circuit breaker, see [Provider
    failures](#provider-failures); `retry_in_s` tells when it will be tried
    again) or `retrying` (the next request tests it)
  * `cache_hits` / `cache_misses` - IDs found in Redis (cover or cached "no
    cover") or not
  * `calls`, `failures`, `skipped` (calls not made because the provider was
    disabled), `covers_found`
  * `avg_call_ms`, `min_call_ms`, `max_call_ms` - average, fastest and
    slowest provider call since startup. A call covers all the IDs of a
    request missing from the cache: one HTTP request for Google Books, one to
    three for Open Library, one per ID for Amazon, whose calls therefore
    grow with the number of IDs. A call cut by the global timeout lasts
    about `timeout`. Requests served from the cache, and calls skipped while
    the provider is disabled, aren't counted
* `redis` - reachability, response time, total number of keys, memory used

These counters are kept in memory, per Coce instance, and reset on restart
(`started_at` is in the server's local time, with its UTC offset); see
below for a history that survives restarts. `redis.keys` counts (provider, ID)
pairs, including cached "no cover" answers, and any other key in the same
Redis database: an ISBN cached for three providers counts three times.
Counting cached covers per provider would require scanning every key, which
is too expensive on a large cache to be done on each call.

See [Behind NGINX](#behind-nginx) for an NGINX configuration keeping
`/stats` internal.

### Daily history: `/stats/daily` and `coce stats`

The same counters are also summed per day (server local time) and kept in
Redis for `statsDays` days (default 30): unlike `/stats`, this history
survives restarts, and adds up all Coce instances sharing the Redis
server. `/stats/daily?days=7` returns the last days as JSON, most recent
(today, in progress) first; without `days`, the whole retained period:

```json
[
  { "date": "2026-10-03",
    "requests": { "cover": 9, "rejected": 2, "ids": 10, "global_timeouts": 0 },
    "providers": {
      "gb": { "cache_hits": 8, "cache_misses": 2, "cache_hit_rate": 0.8,
              "calls": 1, "failures": 0, "skipped": 0, "covers_found": 2,
              "avg_call_ms": 228 },
      ... } },
  ...
]
```

`coce stats` prints it as tables in a terminal (`--days N`, `--json` for the
JSON above):

```
Provider gb
date              hits    misses   hit%   calls  failures  skipped   found  avg ms
2026-10-03           8         2   80.0       1         0        0       2     228
2026-10-02         300       100   75.0      40         0        0      70     300
```

Each server adds its counts to Redis every 10 seconds, and on shutdown, in
one round trip, so counting stays off the request path. `/stats/daily`
includes the counts of the answering instance not flushed yet; `coce stats`
shows what is in Redis. Days without activity show zeros. Fastest and
slowest call times are only in `/stats`. Each day is a Redis hash
(`coce:stats:YYYY-MM-DD`) that expires on its own after the retention
period.

## Logging

Coce writes its logs to stdout, one event per line. With the default settings
a healthy server logs almost nothing: two `INFO` lines at startup (loaded
configuration, listening port), then only `WARN` lines when something goes
wrong.

### Log levels

| Level   | What gets logged |
|---------|------------------|
| `ERROR` | A provider task crashed (panic) |
| `WARN`  | Redis unreachable or too slow (the cache is then bypassed), Redis write failures, provider network errors, timeouts or unexpected HTTP status (e.g. Amazon throttling with 429/503, wrong ORB credentials), unparseable provider responses, provider disabled by its circuit breaker, local image cache failures (directory, download, write), global `timeout` reached (with the list of providers still pending), `providerTimeout` not lower than `timeout` |
| `INFO`  | Startup: configuration summary and listening port; provider re-enabled after a failure |
| `DEBUG` | Per request and per provider: cache lookup (IDs requested / cache misses), provider call (IDs queried / answered / found / failed / duration), provider skipped because disabled, Amazon probe details (HTTP status, content-type); HTTP access log (see below) |

### Choosing what to log: `RUST_LOG`

The filter is set with the standard `RUST_LOG` environment variable
([syntax](https://docs.rs/tracing-subscriber/latest/tracing_subscriber/filter/struct.EnvFilter.html#directives)):
a comma-separated list of `target=level` directives. Coce's own events live
under the `coce` target (`coce::fetcher`, `coce::providers::aws`, ...). When
`RUST_LOG` is unset, the default is `warn,coce=info`.

```sh
# Default behaviour
RUST_LOG=warn,coce=info ./target/release/coce

# Only problems
RUST_LOG=warn ./target/release/coce

# Cache hits/misses and provider timings
RUST_LOG=warn,coce=debug ./target/release/coce

# Debug a single provider only
RUST_LOG=warn,coce=info,coce::providers::aws=debug ./target/release/coce

# HTTP access log: one line per request, with status and latency
RUST_LOG=warn,coce=info,tower_http=debug ./target/release/coce
```

### Output format: `COCE_LOG_FORMAT`

* unset (default): human-readable text. Colors are used only when stdout is a
  terminal, so journald and `docker logs` get plain text.

  ```
  2026-09-28T17:24:58.047683Z  WARN coce::fetcher: redis read failed, bypassing cache provider="gb" error=broken pipe
  ```

* `json`: one JSON object per line, for log collectors (Loki, Elasticsearch,
  Datadog, ...):

  ```json
  {"timestamp":"2026-09-28T17:24:25.646201Z","level":"DEBUG","fields":{"message":"provider fetched","provider":"aws","queried":2,"found":2,"elapsed_ms":161},"target":"coce::fetcher"}
  ```

### With systemd

Logs go to journald. Add the variables to the unit file:

```ini
[Service]
Environment=RUST_LOG=warn,coce=info
```

Then read them:

```sh
journalctl -u coce -f               # follow
journalctl -u coce -p warning       # only WARN and ERROR
journalctl -u coce --since "1 hour ago"
```

To change the level temporarily, use `sudo systemctl edit coce`, add the new
`Environment=RUST_LOG=...` line, then `sudo systemctl restart coce`.

### With Docker

Set the variables in `.env` (see `.env.sample`) or on the command line:

```sh
docker run -e RUST_LOG=warn,coce=debug -e COCE_LOG_FORMAT=json ... coce
docker compose logs -f coce
```

### Performance

Logging has no measurable impact on Coce's performance. A filtered-out event
costs a few nanoseconds (a level check). An emitted event costs about a
microsecond, which is nothing compared with a Redis round trip (~1 ms) or a
provider call (tens to hundreds of ms). The only thing to watch is volume:
`DEBUG` and the access log produce several lines per request. That's fine
for troubleshooting, but under heavy load they fill disks quickly. For
production, stick with the default level.

## Devel

### Architecture

- `config.rs` — loading/typing of `config.json` (`serde_json`, no `eval`)
- `redis_store.rs` — thin wrapper around `redis::aio::ConnectionManager`
- `providers/` — one module per provider (`aws`, `gb`, `ol`, `orb`), each
  exposing a `fetch(ids, ...) -> Outcome` function: definitive answers per
  ID (cover URL or none), plus whether the call failed
- `fetcher.rs` — orchestration: checks the Redis cache, calls the missing
  providers in parallel, enforces a global timeout, writes results (and
  misses) back to Redis
- `breaker.rs` — per-provider circuit breaker (lock-free atomics)
- `http.rs` — Axum routes (`/`, `/cover`, `/set`, `/stats`, `/stats/daily`)
- `stats.rs` — in-memory activity counters for `/stats` (lock-free atomics)
- `cache_check.rs` — `coce cache-check` command (local copies vs Redis)
- `daily.rs` — daily activity history in Redis (`/stats/daily`, `coce stats`)
- `error.rs` — HTTP errors (JSON `{"error": ...}` responses)


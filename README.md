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
* [Open Library Read API](http://openlibrary.org/dev/docs/api/read)
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
COCE_CONFIG=config.json ./target/release/coce
```

By default `config.json` is read from the current directory; the
`COCE_CONFIG` environment variable points to a different path.

* __Configure__ Coce operation by editing
  [config.json](https://github.com/fredericd/coce-rs/blob/master/config.json.sample)
  Start with provided `config.json.sample` file.
  * `port` - port on which the server respond
  * `providers` - array of available providers: gb,aws,ol
  * `timeout` - timeout in miliseconds for the service. Above this value, Coce
    stops waiting response from providers
  * `redis` - Redis server parameters:
     * `host`
     * `port`
     * `timeout`
  * `cache` - Local cache for images
    * `path` - path to the directory where images are cached locally
    * `url` - base url to the `path` directory
  * `gb` - Google Books parameters:
     * `timeout` - timeout of the cached URL from Google Books
  * `ol` - Open Library parameters:
     * `timeout` - timeout of the cached URL from Open Library. After this
       delay, an URL is automatically removed from the cache, and so has to be
       re-fetched again if requested
     * `imageSize` - size of images: small, medium, large
  * `aws` - Amazon
     * `imageSize` - size of images: SmallImage, MediumImage, LargeImage
     * `timeout` - timeout when probing images url via direct http requests
  * `orb` - ORB
     * `user` - user to access ORB API
     * `key` - API key
     * `cache` - true/false, are images locally cached (and served)
     * `timeout` - timeout when probing images url via direct http requests

## Service usage

To get all cover images from Open Library (ol), Google Books (gb), and Amazon
(aws) for several ISBN:

    http://coce.server/cover?id=9780415480635,9780821417492,2847342257,9780563533191&provider=ol,gb,aws&all

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

```jsonp
populateImg({"2847342257":"https://images-na.ssl-images-amazon.com/images/I/51LYLJRtthL._SL160_.jpg","9780563533191":"https://covers.openlibrary.org/b/id/2520432-M.jpg","9780415480635":"https://books.google.com/books/content?id=Yc30cofv4_MC&printsec=frontcover&img=1&zoom=1","9780821417492":"https://books.google.com/books/content?id=D5yimAEACAAJ&printsec=frontcover&img=1&zoom=1"})
```

## Client-side usage

See `sample-client.html` for a Coce sample usage from JavaScript. It uses
`coceclient.js` module, which is use like this:

```javascript
// isbns is an array of ISBNs
var coceClient = new CoceClient('http://coceserver.com:8080', 'ol,aws,gb');
coceClient.fetch(isbns, function(isbn, url) {
  $('#isbn_'+isbn).html('<img src="+url)+'"");
});


## Performance

__coce__ is highly scalable. With all requested URLs in cache, ``ab`` test,
10000 requests, with 50 concurrent requests:

ab -n 10000 -c 50 http://localhost:8080/cover?id=9780415480635,978081417492,2847342257,9780563533191&provider=gb,aws


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
## Devel

### Architecture

- `config.rs` — loading/typing of `config.json` (`serde_json`, no `eval`)
- `redis_store.rs` — thin wrapper around `redis::aio::ConnectionManager`
- `providers/` — one module per provider (`aws`, `gb`, `ol`, `orb`), each
  exposing a `fetch(ids, ...) -> HashMap<id, url>` function
- `fetcher.rs` — orchestration: checks the Redis cache, calls the missing
  providers in parallel, enforces a global timeout, writes results (and
  misses) back to Redis
- `http.rs` — Axum routes (`/`, `/cover`, `/set`)
- `error.rs` — HTTP errors (JSON `{"error": ...}` responses)


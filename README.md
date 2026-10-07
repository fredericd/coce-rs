# Coce

A cover image URL cache, exposing its content as a REST web service.

[![MIT
License](https://img.shields.io/badge/license-MIT-blue.svg)](https://opensource.org/licenses/MIT)

Library catalogs (OPACs) display a cover image next to each record. Coce
finds these covers, by ISBN, at several providers (Amazon, Google Books,
Open Library, [ORB](https://www.base-orb.fr)), caches their URLs in Redis,
and serves them to the catalogs' web pages. One Coce server can serve many
OPACs: [Koha](https://koha-community.org) has a built-in Coce client.

* **One request for a whole result page**: a list of ISBNs, the providers
  in order of preference; Coce answers with the first available cover of
  each ISBN, or all of them.
* **Fast**: cached answers take about a millisecond; uncached ones are
  looked up at all providers in parallel, within a time limit.
* **Robust**: a failing provider is disabled for a while instead of slowing
  every request down, and its failures are never cached as "no cover";
  Coce keeps answering without Redis.
* **Optional local copies** of the images, served by your own web server.
* **Observable**: activity and state at `/stats`, a 30-day daily history,
  structured logs.

## Quick start

With Docker, Coce and Redis:

```sh
cp .env.sample .env   # settings as environment variables
docker compose up --build
```

Or from the sources, with a Redis server on `localhost:6379`:

```sh
cargo build --release
./target/release/coce
```

Without a `config.json`, Coce starts with default settings: port 8080,
providers `aws`, `gb`, `ol`. Then:

```sh
curl 'http://localhost:8080/cover?id=9780563533191,2847342257&provider=ol,gb,aws'
```

```json
{
  "9780563533191": "https://covers.openlibrary.org/b/id/2520432-M.jpg",
  "2847342257": "https://images-na.ssl-images-amazon.com/images/P/2847342257.01.MZZZZZZZZZ.jpg"
}
```

## Documentation

| Page | Contents |
|---|---|
| [API](docs/api.md) | `/cover`, JSONP, `/set`, errors, Koha and JavaScript clients |
| [Configuration](docs/configuration.md) | Every setting, its environment variable and default |
| [Providers](docs/providers.md) | How each provider is queried, its limits, failure handling |
| [Deployment](docs/deployment.md) | systemd, Docker, NGINX, Redis availability, several instances |
| [Administration](docs/administration.md) | `/stats`, daily history, cache purges, `coce cache-check` |
| [Logging](docs/logging.md) | Log levels, `RUST_LOG`, JSON output, systemd and Docker |
| [Performance](docs/performance.md) | Measurements, comparison with Coce for Node.js, how to reproduce them |
| [Development](docs/development.md) | Code architecture, building and testing |

## License

MIT, see [LICENSE](LICENSE).

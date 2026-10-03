# Configuration

Coce reads its configuration from a JSON file, `config.json` in the current
directory by default; `--config <file>` (`-c`), or the `COCE_CONFIG`
environment variable, points to another path (the option wins over the
variable). Start from [`config.json.sample`](../config.json.sample).

Every setting can also be given as a `COCE_*` environment variable, which
always wins over the file. Without any file, Coce starts from the built-in
defaults, so a container can be configured with environment variables only:
see [`.env.sample`](../.env.sample) for the full list.

In a `config.json`, `port`, `providers` and `timeout` are required, and so
are `host`, `port` and `timeout` when a `redis` section is given.

## General settings

| Setting | Variable | Default | Description |
|---|---|---|---|
| `port` | `COCE_PORT` | `8080` | Port the server listens on. |
| `providers` | `COCE_PROVIDERS` | `aws,gb,ol` | Providers available to clients, among `aws`, `gb`, `ol`, `orb` (comma-separated in the variable). A request can only ask for these. |
| `timeout` | `COCE_TIMEOUT` | `8000` | Global time limit of a `/cover` request, in milliseconds. At this point, Coce answers with whatever it has; providers still running finish in the background and cache their answers. |
| `providerTimeout` | `COCE_PROVIDER_TIMEOUT` | `5000` | Time limit of each HTTP request to a provider, in milliseconds. Keep it lower than `timeout`: a provider that doesn't answer within it counts as failing (see [circuit breaker](providers.md#failures-and-circuit-breaker)), which it can't once the global timeout has cut the request. A warning is logged at startup otherwise. |
| `providerRetry` | `COCE_PROVIDER_RETRY` | `300` | How long, in seconds, a failing provider stays disabled before being tried again. |
| `notFoundTimeout` | `COCE_NOT_FOUND_TIMEOUT` | `86400` | How long, in seconds, a "no cover" answer stays cached. Covers found stay cached for their provider's `timeout` (below). |
| `maxIds` | `COCE_MAX_IDS` | `100` | Maximum number of IDs in a `/cover` request. Above it, Coce answers `400` (see [API](api.md#errors)). |
| `statsDays` | `COCE_STATS_DAYS` | `30` | Days of activity history kept in Redis, for `/stats/daily` and `coce stats` (see [administration](administration.md#daily-history-statsdaily-and-coce-stats)). |
| `setToken` | `COCE_SET_TOKEN` | unset | Secret required by `/set`, sent as a bearer token. `/set` is disabled when unset or empty (see [API](api.md#forcing-a-cover-url-set)). Use a long random value, e.g. `openssl rand -hex 32`. |

## Redis: `redis`

| Setting | Variable | Default | Description |
|---|---|---|---|
| `redis.host` | `COCE_REDIS_HOST` | `127.0.0.1` | Redis server host. |
| `redis.port` | `COCE_REDIS_PORT` | `6379` | Redis server port. |
| `redis.timeout` | `COCE_REDIS_TIMEOUT` | `500` | Time limit of Redis reads and writes, in milliseconds. When Redis is slow or down, Coce bypasses the cache and queries providers directly instead of waiting; every request pays this delay while Redis doesn't answer, so keep it short: a Redis on the same network answers in about a millisecond. See [Redis availability](deployment.md#redis-availability). |

## Local image copies: `cache`

For providers with `cache: true` (below), Coce downloads the cover images
and serves them itself, through the reverse proxy, instead of returning the
provider's URL.

| Setting | Variable | Default | Description |
|---|---|---|---|
| `cache.path` | `COCE_CACHE_PATH` | unset | Directory where images are stored, one subdirectory per provider (`<path>/<provider>/<isbn>.jpg`). |
| `cache.url` | `COCE_CACHE_URL` | unset | Public base URL of that directory, e.g. `https://coce.example.org/covers`. |

## Providers: `aws`, `gb`, `ol`, `orb`

Each provider has an optional section; variables are prefixed with the
provider's name in capitals (`COCE_GB_TIMEOUT`, `COCE_ORB_USER`...). See
[providers](providers.md) for what each one does and its limits.

| Setting | Variable | Default | Providers | Description |
|---|---|---|---|---|
| `timeout` | `COCE_<P>_TIMEOUT` | `86400` | all | How long, in seconds, a cover found by this provider stays cached (also when set to `0`). A found cover URL rarely changes and can be kept long, e.g. 30 days (`2592000`), which saves provider calls; "no cover" answers follow `notFoundTimeout`. |
| `cache` | `COCE_<P>_CACHE` | `false` | all | Store the images locally (see `cache` above). |
| `imageSize` | `COCE_<P>_IMAGE_SIZE` | see description | `ol`, `orb` | `ol`: `small`, `medium` (default) or `large`. `orb`: `thumbnail` (default, 160 px high) or `original` (500 px high, falling back to the thumbnail when missing). Amazon and Google Books images have a fixed size. |
| `user` | `COCE_ORB_USER` | unset | `orb` | ORB API user. |
| `key` | `COCE_ORB_KEY` | unset | `orb` | ORB API key. |

## Logging

| Variable | Default | Description |
|---|---|---|
| `RUST_LOG` | `warn,coce=info` | What to log. |
| `COCE_LOG_FORMAT` | text | `json` for one JSON object per line. |

See [logging](logging.md).

## Applying changes

Settings are read at startup: restart Coce after changing them. Some
settings shape the URLs Coce caches, so the URLs already cached keep the old
form until they expire; see [changing settings in
production](administration.md#changing-settings-in-production).

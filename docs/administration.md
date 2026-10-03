# Administration

* [`/stats`](#stats): current state and counters since startup
* [Daily history](#daily-history-statsdaily-and-coce-stats): `/stats/daily` and `coce stats`
* [Changing settings in production](#changing-settings-in-production)
* [Checking local copies](#checking-local-copies-coce-cache-check): `coce cache-check`
* [Logging](logging.md)

Commands (`coce stats`, `coce cache-check`) use the same configuration as
the server (`--config`, `COCE_CONFIG`, `COCE_*` variables). With Docker, run
them in the container: `docker compose exec coce coce stats`.

## `/stats`

`/stats` returns activity counters, provider state and Redis figures, as
JSON. It requires no authentication: when Coce runs behind a reverse proxy,
restrict it there if needed (it reveals the configuration and traffic
volume, but no credentials).

Example:

```json
{
  "version": "1.0.0",
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
  * `state` - `enabled`, `disabled` (by its circuit breaker, see [providers](providers.md#failures-and-circuit-breaker);
    `retry_in_s` tells when it will be tried again) or `retrying` (the next
    request tests it)
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

See [deployment](deployment.md#behind-nginx) for an NGINX configuration
keeping `/stats` internal.

## Daily history: `/stats/daily` and `coce stats`

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

## Changing settings in production

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

## Checking local copies: `coce cache-check`

For providers with `cache: true`, Redis and the image directory can drift
apart: a download that failed while Redis already points to the local file,
an error page saved as an image, keys expired or purged while files remain,
a change of `cache.url`... `coce cache-check` compares them, using the same
configuration as the server:

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
in a cron job or a monitoring check.

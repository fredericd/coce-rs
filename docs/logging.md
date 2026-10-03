# Logging

Coce writes its logs to stdout, one event per line. With the default settings
a healthy server logs almost nothing: two `INFO` lines at startup (loaded
configuration, listening port), then only `WARN` lines when something goes
wrong.

## Log levels

| Level   | What gets logged |
|---------|------------------|
| `ERROR` | A provider task crashed (panic) |
| `WARN`  | Redis unreachable or too slow (the cache is then bypassed), Redis write failures, provider network errors, timeouts or unexpected HTTP status (e.g. Amazon throttling with 429/503, wrong ORB credentials), unparseable provider responses, provider disabled by its circuit breaker, local image cache failures (directory, download, write), global `timeout` reached (with the list of providers still pending), `providerTimeout` not lower than `timeout` |
| `INFO`  | Startup: configuration summary and listening port; provider re-enabled after a failure |
| `DEBUG` | Per request and per provider: cache lookup (IDs requested / cache misses), provider call (IDs queried / answered / found / failed / duration), provider skipped because disabled, Amazon probe details (HTTP status, content-type); HTTP access log (see below) |

## Choosing what to log: `RUST_LOG`

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

## Output format: `COCE_LOG_FORMAT`

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

## With systemd

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

## With Docker

Set the variables in `.env` (see `.env.sample`) or on the command line:

```sh
docker run -e RUST_LOG=warn,coce=debug -e COCE_LOG_FORMAT=json ... coce
docker compose logs -f coce
```

## Performance

Logging has no measurable impact on Coce's performance. A filtered-out event
costs a few nanoseconds (a level check). An emitted event costs about a
microsecond, which is nothing compared with a Redis round trip (~1 ms) or a
provider call (tens to hundreds of ms). The only thing to watch is volume:
`DEBUG` and the access log produce several lines per request. That's fine
for troubleshooting, but under heavy load they fill disks quickly. For
production, stick with the default level.

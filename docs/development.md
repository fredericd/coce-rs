# Development

## Architecture

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

## Building and testing

```sh
cargo build
cargo test
cargo clippy --all-targets
```

Tests are unit tests, next to the code they test (`#[cfg(test)]` modules);
they don't need Redis or network access. Provider modules are tested on
their parsing and ID handling; their behavior against the real services is
checked by hand.

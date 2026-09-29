use crate::http::AppState;
use crate::isbn;
use crate::providers;
use crate::redis_store;
use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::Mutex;

/// id -> provider -> url
pub type UrlMap = HashMap<String, HashMap<String, String>>;

/// Check Redis for cached URLs, fall back to the live provider for whatever
/// is missing, and cache the provider's definitive answers (including "not
/// found") back in Redis. IDs the provider couldn't answer for (because it
/// failed, or is disabled by its circuit breaker) are not cached, so they're
/// looked up again once the provider is back.
async fn fetch_provider(
    provider: &str,
    ids: &[String],
    state: &AppState,
    deadline: tokio::time::Instant,
    shared: &Mutex<UrlMap>,
) {
    let AppState {
        config: cfg,
        redis,
        breakers,
        stats,
        tasks,
        ..
    } = state;
    let mut con = redis.clone();
    let found_ttl = cfg.found_ttl(provider);
    let not_found_ttl = cfg.not_found_ttl();

    let keys: Vec<String> = ids.iter().map(|id| format!("{provider}.{id}")).collect();

    // If Redis is slow to answer, don't block the whole request on it: treat
    // every ID as "not cached" and let the provider settle it instead.
    let cached = tokio::time::timeout(
        Duration::from_millis(cfg.redis.timeout),
        redis_store::get_many(&mut con, &keys),
    )
    .await;

    let mut found = HashMap::new();
    let mut notcached = Vec::new();

    match cached {
        Ok(Ok(values)) => {
            for (id, value) in ids.iter().zip(values) {
                match value {
                    Some(v) if v.is_empty() => {
                        // Cached as "provider has nothing for this ID".
                    }
                    Some(v) => {
                        found.insert(id.clone(), v);
                    }
                    None => notcached.push(id.clone()),
                }
            }
        }
        Ok(Err(e)) => {
            tracing::warn!(provider, error = %e, "redis read failed, bypassing cache");
            notcached = ids.to_vec();
        }
        Err(_) => {
            tracing::warn!(
                provider,
                timeout_ms = cfg.redis.timeout,
                "redis read timed out, bypassing cache"
            );
            notcached = ids.to_vec();
        }
    }

    tracing::debug!(
        provider,
        requested = ids.len(),
        cache_misses = notcached.len(),
        "cache lookup"
    );
    stats.cache_lookup(provider, ids.len() - notcached.len(), notcached.len());

    // Publish cached URLs right away: if the provider call below runs past
    // the global timeout, they still make it into the response.
    publish(shared, provider, std::mem::take(&mut found)).await;

    if notcached.is_empty() {
        return;
    }

    if !breakers.allow(provider) {
        tracing::debug!(provider, skipped = notcached.len(), "provider disabled, skipped");
        stats.provider_skipped(provider);
        return;
    }

    let started = Instant::now();
    let outcome = providers::call(provider, &notcached, cfg, &state.http, deadline).await;
    if outcome.failed {
        breakers.record_failure(provider);
    } else {
        breakers.record_success(provider);
    }
    let covers_found = outcome.answers.values().filter(|url| url.is_some()).count();
    let elapsed_ms = started.elapsed().as_millis() as u64;
    stats.provider_call(provider, outcome.failed, covers_found, elapsed_ms);
    tracing::debug!(
        provider,
        queried = notcached.len(),
        answered = outcome.answers.len(),
        found = covers_found,
        failed = outcome.failed,
        elapsed_ms,
        "provider fetched"
    );
    let cache_locally = cfg.provider_config(provider).map(|c| c.cache).unwrap_or(false);

    let mut writes = Vec::with_capacity(notcached.len());
    for id in &notcached {
        let key = format!("{provider}.{id}");
        match outcome.answers.get(id) {
            Some(Some(remote_url)) => {
                let stored_url = if cache_locally {
                    cache_image_locally(provider, id, remote_url, state)
                        .unwrap_or_else(|| remote_url.clone())
                } else {
                    remote_url.clone()
                };
                writes.push((key, stored_url.clone(), found_ttl));
                found.insert(id.clone(), stored_url);
            }
            Some(None) => {
                // Remember the miss so we don't hit the provider again for a while.
                writes.push((key, String::new(), not_found_ttl));
            }
            // No reliable answer: nothing to remember.
            None => {}
        }
    }

    publish(shared, provider, found).await;

    if writes.is_empty() {
        return;
    }

    // Don't make the client wait for the cache to be filled: write in the
    // background, bounded by the Redis timeout so that a Redis outage (where
    // the connection manager keeps retrying to reconnect) can't pile up
    // stuck tasks.
    let redis_timeout = cfg.redis.timeout;
    let provider = provider.to_string();
    tasks.spawn(async move {
        let result = tokio::time::timeout(
            Duration::from_millis(redis_timeout),
            redis_store::set_many_ex(&mut con, &writes),
        )
        .await;
        match result {
            Ok(Ok(())) => {}
            Ok(Err(e)) => tracing::warn!(%provider, keys = writes.len(), error = %e, "redis write failed"),
            Err(_) => tracing::warn!(
                %provider,
                keys = writes.len(),
                timeout_ms = redis_timeout,
                "redis write timed out"
            ),
        }
    });
}

/// Add `provider`'s URLs to the response being built.
async fn publish(shared: &Mutex<UrlMap>, provider: &str, urls: HashMap<String, String>) {
    if urls.is_empty() {
        return;
    }
    let mut guard = shared.lock().await;
    for (id, url) in urls {
        guard.entry(id).or_default().insert(provider.to_string(), url);
    }
}

/// Compute the local cache path/URL for an image and kick off the download in
/// the background (the caller doesn't need to wait for the file to land).
fn cache_image_locally(provider: &str, id: &str, remote_url: &str, state: &AppState) -> Option<String> {
    let cache_cfg = state.config.cache.as_ref()?;
    let dir = format!("{}/{}", cache_cfg.path, provider);
    let dest = format!("{dir}/{id}.jpg");
    let stored_url = format!("{}/{}/{}.jpg", cache_cfg.url, provider, id);

    let remote_url = remote_url.to_string();
    let http = state.http.clone();
    state.tasks.spawn(async move {
        if let Err(e) = tokio::fs::create_dir_all(&dir).await {
            tracing::warn!(%dir, error = %e, "cannot create local cache directory");
            return;
        }
        let bytes = match http.get(&remote_url).send().await {
            Ok(resp) => resp.bytes().await,
            Err(e) => Err(e),
        };
        match bytes {
            Ok(bytes) => {
                if let Err(e) = tokio::fs::write(&dest, &bytes).await {
                    tracing::warn!(%dest, error = %e, "cannot write cached image");
                }
            }
            Err(e) => tracing::warn!(url = %remote_url, error = %e, "image download failed"),
        }
    });

    Some(stored_url)
}

/// Fetch cover URLs for `ids` from each of `providers`, in parallel, giving
/// up after `cfg.timeout` milliseconds and returning whatever was found so
/// far (mirrors the original Node fetcher's "best effort within a deadline"
/// behaviour). Providers still running at that point are not aborted: they
/// finish in the background and cache what they got, so that the work isn't
/// lost and a later request for the same IDs gets further. This stays
/// bounded: per-ID providers stop at the deadline, and every HTTP request is
/// bounded by `providerTimeout`.
pub async fn fetch(
    ids: &[String],
    providers: &[String],
    state: &AppState,
) -> UrlMap {
    let cfg = &state.config;
    let deadline = tokio::time::Instant::now() + Duration::from_millis(cfg.timeout);

    // Look each book up once, under its canonical ID (ISBN-13 for ISBNs),
    // whatever the spellings it was requested under; the answer is given back
    // under each requested spelling.
    let mut requested_as: HashMap<String, Vec<String>> = HashMap::new();
    for id in ids {
        requested_as.entry(isbn::canonical(id)).or_default().push(id.clone());
    }
    let ids: Vec<String> = requested_as.keys().cloned().collect();

    let shared: Arc<Mutex<UrlMap>> = Arc::new(Mutex::new(HashMap::new()));
    let mut tasks = tokio::task::JoinSet::new();

    for provider in providers {
        let provider = provider.clone();
        let ids = ids.clone();
        let state = state.clone();
        let shared = shared.clone();

        tasks.spawn(async move {
            fetch_provider(&provider, &ids, &state, deadline, &shared).await;
            provider
        });
    }

    let mut done = HashSet::new();
    let wait_all = async {
        while let Some(res) = tasks.join_next().await {
            match res {
                Ok(provider) => {
                    done.insert(provider);
                }
                Err(e) => tracing::error!(error = %e, "provider task failed"),
            }
        }
    };

    let timed_out = tokio::select! {
        _ = wait_all => false,
        _ = tokio::time::sleep_until(deadline) => {
            let pending: Vec<&String> = providers.iter().filter(|p| !done.contains(*p)).collect();
            tracing::warn!(
                timeout_ms = cfg.timeout,
                ?pending,
                "global timeout reached, returning partial results"
            );
            state.stats.global_timeout();
            true
        }
    };
    if timed_out {
        // Let pending providers finish and cache their answers, as tracked
        // background work, so that a graceful shutdown waits for them.
        state.tasks.spawn(async move { while tasks.join_next().await.is_some() {} });
    }

    let found = shared.lock().await.clone();
    let mut result = UrlMap::new();
    for (canonical, per_provider) in found {
        for id in requested_as.remove(&canonical).unwrap_or_default() {
            result.insert(id, per_provider.clone());
        }
    }
    result
}

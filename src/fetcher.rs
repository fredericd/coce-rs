use crate::config::Config;
use crate::providers;
use crate::redis_store::{self, RedisManager};
use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::Mutex;

/// id -> provider -> url
pub type UrlMap = HashMap<String, HashMap<String, String>>;

/// Check Redis for cached URLs, fall back to the live provider for whatever
/// is missing, and cache the outcome (including "not found") back in Redis.
async fn fetch_provider(
    provider: &str,
    ids: &[String],
    cfg: &Config,
    redis: &RedisManager,
    http: &reqwest::Client,
) -> HashMap<String, String> {
    let mut con = redis.clone();
    let ttl = cfg.provider_config(provider).map(|c| c.timeout).unwrap_or(86_400);

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
        _ => notcached = ids.to_vec(),
    }

    if notcached.is_empty() {
        return found;
    }

    let fetched = providers::call(provider, &notcached, cfg, http).await;
    let cache_locally = cfg.provider_config(provider).map(|c| c.cache).unwrap_or(false);

    for id in &notcached {
        let key = format!("{provider}.{id}");
        match fetched.get(id) {
            Some(remote_url) => {
                let stored_url = if cache_locally {
                    cache_image_locally(provider, id, remote_url, cfg, http)
                        .unwrap_or_else(|| remote_url.clone())
                } else {
                    remote_url.clone()
                };
                let _ = redis_store::set_ex(&mut con, &key, ttl, &stored_url).await;
                found.insert(id.clone(), stored_url);
            }
            None => {
                // Remember the miss so we don't hit the provider again for a while.
                let _ = redis_store::set_ex(&mut con, &key, ttl, "").await;
            }
        }
    }

    found
}

/// Compute the local cache path/URL for an image and kick off the download in
/// the background (the caller doesn't need to wait for the file to land).
fn cache_image_locally(
    provider: &str,
    id: &str,
    remote_url: &str,
    cfg: &Config,
    http: &reqwest::Client,
) -> Option<String> {
    let cache_cfg = cfg.cache.as_ref()?;
    let dir = format!("{}/{}", cache_cfg.path, provider);
    let dest = format!("{dir}/{id}.jpg");
    let stored_url = format!("{}/{}/{}.jpg", cache_cfg.url, provider, id);

    let remote_url = remote_url.to_string();
    let http = http.clone();
    tokio::spawn(async move {
        if tokio::fs::create_dir_all(&dir).await.is_err() {
            return;
        }
        if let Ok(resp) = http.get(&remote_url).send().await {
            if let Ok(bytes) = resp.bytes().await {
                let _ = tokio::fs::write(&dest, &bytes).await;
            }
        }
    });

    Some(stored_url)
}

/// Fetch cover URLs for `ids` from each of `providers`, in parallel, giving
/// up after `cfg.timeout` milliseconds and returning whatever was found so
/// far (mirrors the original Node fetcher's "best effort within a deadline"
/// behaviour).
pub async fn fetch(
    ids: &[String],
    providers: &[String],
    cfg: &Arc<Config>,
    redis: &RedisManager,
    http: &reqwest::Client,
) -> UrlMap {
    let shared: Arc<Mutex<UrlMap>> = Arc::new(Mutex::new(HashMap::new()));
    let mut tasks = tokio::task::JoinSet::new();

    for provider in providers {
        let provider = provider.clone();
        let ids = ids.to_vec();
        let cfg = cfg.clone();
        let redis = redis.clone();
        let http = http.clone();
        let shared = shared.clone();

        tasks.spawn(async move {
            let result = fetch_provider(&provider, &ids, &cfg, &redis, &http).await;
            let mut guard = shared.lock().await;
            for (id, url) in result {
                guard.entry(id).or_default().insert(provider.clone(), url);
            }
        });
    }

    let wait_all = async {
        while tasks.join_next().await.is_some() {}
    };

    tokio::select! {
        _ = wait_all => {}
        _ = tokio::time::sleep(Duration::from_millis(cfg.timeout)) => {}
    }

    let result = shared.lock().await.clone();
    result
}

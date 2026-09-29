use serde::Serialize;
use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Instant, SystemTime, UNIX_EPOCH};

/// In-memory activity counters, exposed by `/stats`. They are per process
/// and reset on restart (`uptime_s` gives the context). Only atomics, so
/// counting costs no lock on the request path.
pub struct Stats {
    started: Instant,
    started_at: u64,
    requests: AtomicU64,
    rejected: AtomicU64,
    ids: AtomicU64,
    global_timeouts: AtomicU64,
    by_provider: HashMap<String, ProviderStats>,
}

struct ProviderStats {
    cache_hits: AtomicU64,
    cache_misses: AtomicU64,
    calls: AtomicU64,
    failures: AtomicU64,
    skipped: AtomicU64,
    covers_found: AtomicU64,
    call_ms_total: AtomicU64,
    /// `u64::MAX` until the first call.
    call_ms_min: AtomicU64,
    call_ms_max: AtomicU64,
}

impl Default for ProviderStats {
    fn default() -> Self {
        ProviderStats {
            cache_hits: AtomicU64::new(0),
            cache_misses: AtomicU64::new(0),
            calls: AtomicU64::new(0),
            failures: AtomicU64::new(0),
            skipped: AtomicU64::new(0),
            covers_found: AtomicU64::new(0),
            call_ms_total: AtomicU64::new(0),
            call_ms_min: AtomicU64::new(u64::MAX),
            call_ms_max: AtomicU64::new(0),
        }
    }
}

#[derive(Serialize)]
pub struct RequestsSnapshot {
    /// `/cover` requests received.
    pub cover: u64,
    /// `/cover` requests rejected with a 400 (missing/bad ID, too many IDs,
    /// unavailable provider).
    pub rejected: u64,
    /// IDs requested in accepted `/cover` requests.
    pub ids: u64,
    /// Requests answered at the global timeout, with partial results.
    pub global_timeouts: u64,
}

#[derive(Serialize)]
pub struct ProviderSnapshot {
    /// IDs found in Redis (cover or cached "no cover").
    pub cache_hits: u64,
    /// IDs not in Redis, to be asked to the provider.
    pub cache_misses: u64,
    /// `cache_hits / (cache_hits + cache_misses)`, null before any lookup.
    pub cache_hit_rate: Option<f64>,
    pub calls: u64,
    pub failures: u64,
    /// Calls not made because the provider was disabled by its breaker.
    pub skipped: u64,
    pub covers_found: u64,
    /// Duration of a provider call (all the IDs of a request missing from
    /// the cache): average, fastest and slowest since startup. Null before
    /// any call.
    pub avg_call_ms: Option<u64>,
    pub min_call_ms: Option<u64>,
    pub max_call_ms: Option<u64>,
}

fn add(counter: &AtomicU64, n: u64) {
    counter.fetch_add(n, Ordering::Relaxed);
}

fn get(counter: &AtomicU64) -> u64 {
    counter.load(Ordering::Relaxed)
}

impl Stats {
    pub fn new(providers: &[String]) -> Self {
        Stats {
            started: Instant::now(),
            started_at: SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .map(|d| d.as_secs())
                .unwrap_or(0),
            requests: AtomicU64::new(0),
            rejected: AtomicU64::new(0),
            ids: AtomicU64::new(0),
            global_timeouts: AtomicU64::new(0),
            by_provider: providers
                .iter()
                .map(|p| (p.clone(), ProviderStats::default()))
                .collect(),
        }
    }

    pub fn cover_request(&self) {
        add(&self.requests, 1);
    }

    pub fn rejected(&self) {
        add(&self.rejected, 1);
    }

    pub fn ids_requested(&self, n: usize) {
        add(&self.ids, n as u64);
    }

    pub fn global_timeout(&self) {
        add(&self.global_timeouts, 1);
    }

    pub fn cache_lookup(&self, provider: &str, hits: usize, misses: usize) {
        if let Some(p) = self.by_provider.get(provider) {
            add(&p.cache_hits, hits as u64);
            add(&p.cache_misses, misses as u64);
        }
    }

    pub fn provider_skipped(&self, provider: &str) {
        if let Some(p) = self.by_provider.get(provider) {
            add(&p.skipped, 1);
        }
    }

    pub fn provider_call(&self, provider: &str, failed: bool, found: usize, elapsed_ms: u64) {
        if let Some(p) = self.by_provider.get(provider) {
            add(&p.calls, 1);
            add(&p.failures, failed as u64);
            add(&p.covers_found, found as u64);
            add(&p.call_ms_total, elapsed_ms);
            p.call_ms_min.fetch_min(elapsed_ms, Ordering::Relaxed);
            p.call_ms_max.fetch_max(elapsed_ms, Ordering::Relaxed);
        }
    }

    /// Unix timestamp (seconds) of the process start.
    pub fn started_at(&self) -> u64 {
        self.started_at
    }

    pub fn uptime_s(&self) -> u64 {
        self.started.elapsed().as_secs()
    }

    pub fn requests(&self) -> RequestsSnapshot {
        RequestsSnapshot {
            cover: get(&self.requests),
            rejected: get(&self.rejected),
            ids: get(&self.ids),
            global_timeouts: get(&self.global_timeouts),
        }
    }

    pub fn provider(&self, provider: &str) -> Option<ProviderSnapshot> {
        let p = self.by_provider.get(provider)?;
        let hits = get(&p.cache_hits);
        let misses = get(&p.cache_misses);
        let calls = get(&p.calls);
        Some(ProviderSnapshot {
            cache_hits: hits,
            cache_misses: misses,
            cache_hit_rate: (hits + misses > 0)
                .then(|| (hits as f64 / (hits + misses) as f64 * 1000.0).round() / 1000.0),
            calls,
            failures: get(&p.failures),
            skipped: get(&p.skipped),
            covers_found: get(&p.covers_found),
            avg_call_ms: (calls > 0).then(|| get(&p.call_ms_total) / calls),
            min_call_ms: (calls > 0).then(|| get(&p.call_ms_min)),
            max_call_ms: (calls > 0).then(|| get(&p.call_ms_max)),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn provider_snapshot() {
        let s = Stats::new(&["gb".to_string()]);
        assert_eq!(s.provider("gb").unwrap().cache_hit_rate, None);
        assert_eq!(s.provider("gb").unwrap().avg_call_ms, None);
        assert_eq!(s.provider("gb").unwrap().min_call_ms, None);
        assert_eq!(s.provider("gb").unwrap().max_call_ms, None);

        s.cache_lookup("gb", 3, 1);
        s.provider_call("gb", false, 1, 100);
        s.provider_call("gb", true, 0, 300);
        s.provider_skipped("gb");
        s.provider_skipped("xx");

        let p = s.provider("gb").unwrap();
        assert_eq!(p.cache_hit_rate, Some(0.75));
        assert_eq!((p.calls, p.failures, p.skipped, p.covers_found), (2, 1, 1, 1));
        assert_eq!(p.avg_call_ms, Some(200));
        assert_eq!(p.min_call_ms, Some(100));
        assert_eq!(p.max_call_ms, Some(300));
        assert!(s.provider("xx").is_none());
    }
}

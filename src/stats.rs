use chrono::{DateTime, Local, SecondsFormat};
use serde::Serialize;
use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Instant;

/// In-memory activity counters. Only atomics, so counting costs no lock on
/// the request path. Additive counters are kept twice: since startup
/// (`total`, exposed by `/stats`, reset on restart) and since the last flush
/// to the daily history kept in Redis (`pending`, see `daily`).
pub struct Stats {
    started: Instant,
    started_at: DateTime<Local>,
    total: Counters,
    pending: Counters,
    /// Fastest and slowest provider call since startup (`u64::MAX` / 0 until
    /// the first call). Not part of the daily history.
    call_ms_range: HashMap<String, (AtomicU64, AtomicU64)>,
}

struct Counters {
    requests: AtomicU64,
    rejected: AtomicU64,
    ids: AtomicU64,
    global_timeouts: AtomicU64,
    by_provider: HashMap<String, ProviderCounters>,
}

#[derive(Default)]
struct ProviderCounters {
    cache_hits: AtomicU64,
    cache_misses: AtomicU64,
    calls: AtomicU64,
    failures: AtomicU64,
    skipped: AtomicU64,
    covers_found: AtomicU64,
    call_ms_total: AtomicU64,
}

impl Counters {
    fn new(providers: &[String]) -> Self {
        Counters {
            requests: AtomicU64::new(0),
            rejected: AtomicU64::new(0),
            ids: AtomicU64::new(0),
            global_timeouts: AtomicU64::new(0),
            by_provider: providers
                .iter()
                .map(|p| (p.clone(), ProviderCounters::default()))
                .collect(),
        }
    }

    /// Every counter with its field name in the daily history:
    /// `requests.<name>` and `<provider>.<name>`.
    fn fields(&self) -> Vec<(String, &AtomicU64)> {
        let mut fields = vec![
            ("requests.cover".to_string(), &self.requests),
            ("requests.rejected".to_string(), &self.rejected),
            ("requests.ids".to_string(), &self.ids),
            ("requests.global_timeouts".to_string(), &self.global_timeouts),
        ];
        for (provider, c) in &self.by_provider {
            for (name, counter) in [
                ("cache_hits", &c.cache_hits),
                ("cache_misses", &c.cache_misses),
                ("calls", &c.calls),
                ("failures", &c.failures),
                ("skipped", &c.skipped),
                ("covers_found", &c.covers_found),
                ("call_ms_total", &c.call_ms_total),
            ] {
                fields.push((format!("{provider}.{name}"), counter));
            }
        }
        fields
    }

    fn requests(&self) -> RequestsSnapshot {
        RequestsSnapshot {
            cover: get(&self.requests),
            rejected: get(&self.rejected),
            ids: get(&self.ids),
            global_timeouts: get(&self.global_timeouts),
        }
    }
}

#[derive(Serialize)]
pub struct RequestsSnapshot {
    /// `/cover` requests received.
    pub cover: u64,
    /// `/cover` requests rejected with a 400 (missing/bad ID, too many IDs,
    /// unavailable provider, bad callback).
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

/// `hits / (hits + misses)` rounded to 3 decimals, `None` without lookups.
pub fn hit_rate(hits: u64, misses: u64) -> Option<f64> {
    (hits + misses > 0).then(|| (hits as f64 / (hits + misses) as f64 * 1000.0).round() / 1000.0)
}

impl Stats {
    pub fn new(providers: &[String]) -> Self {
        Stats {
            started: Instant::now(),
            started_at: Local::now(),
            total: Counters::new(providers),
            pending: Counters::new(providers),
            call_ms_range: providers
                .iter()
                .map(|p| (p.clone(), (AtomicU64::new(u64::MAX), AtomicU64::new(0))))
                .collect(),
        }
    }

    /// Apply `f` to the same counter in `total` and `pending`.
    fn count(&self, f: impl Fn(&Counters)) {
        f(&self.total);
        f(&self.pending);
    }

    fn count_provider(&self, provider: &str, f: impl Fn(&ProviderCounters)) {
        self.count(|c| {
            if let Some(p) = c.by_provider.get(provider) {
                f(p);
            }
        });
    }

    pub fn cover_request(&self) {
        self.count(|c| add(&c.requests, 1));
    }

    pub fn rejected(&self) {
        self.count(|c| add(&c.rejected, 1));
    }

    pub fn ids_requested(&self, n: usize) {
        self.count(|c| add(&c.ids, n as u64));
    }

    pub fn global_timeout(&self) {
        self.count(|c| add(&c.global_timeouts, 1));
    }

    pub fn cache_lookup(&self, provider: &str, hits: usize, misses: usize) {
        self.count_provider(provider, |p| {
            add(&p.cache_hits, hits as u64);
            add(&p.cache_misses, misses as u64);
        });
    }

    pub fn provider_skipped(&self, provider: &str) {
        self.count_provider(provider, |p| add(&p.skipped, 1));
    }

    pub fn provider_call(&self, provider: &str, failed: bool, found: usize, elapsed_ms: u64) {
        self.count_provider(provider, |p| {
            add(&p.calls, 1);
            add(&p.failures, failed as u64);
            add(&p.covers_found, found as u64);
            add(&p.call_ms_total, elapsed_ms);
        });
        if let Some((min, max)) = self.call_ms_range.get(provider) {
            min.fetch_min(elapsed_ms, Ordering::Relaxed);
            max.fetch_max(elapsed_ms, Ordering::Relaxed);
        }
    }

    /// Take (and reset) the counts accumulated since the last flush, as
    /// `(field, delta)` pairs, leaving out zeros.
    pub fn take_pending(&self) -> Vec<(String, u64)> {
        self.pending
            .fields()
            .into_iter()
            .map(|(name, counter)| (name, counter.swap(0, Ordering::Relaxed)))
            .filter(|(_, delta)| *delta > 0)
            .collect()
    }

    /// Give back counts taken by `take_pending` that couldn't be flushed, so
    /// that the next flush carries them.
    pub fn restore_pending(&self, deltas: &[(String, u64)]) {
        let fields = self.pending.fields();
        for (name, delta) in deltas {
            if let Some((_, counter)) = fields.iter().find(|(n, _)| n == name) {
                add(counter, *delta);
            }
        }
    }

    /// Counts not flushed yet, without resetting them.
    pub fn pending(&self) -> HashMap<String, u64> {
        self.pending
            .fields()
            .into_iter()
            .map(|(name, counter)| (name, get(counter)))
            .filter(|(_, v)| *v > 0)
            .collect()
    }

    /// Process start, as RFC 3339 in the server's local time with its UTC
    /// offset, e.g. `2026-09-29T20:32:10+02:00`.
    pub fn started_at(&self) -> String {
        self.started_at.to_rfc3339_opts(SecondsFormat::Secs, false)
    }

    pub fn uptime_s(&self) -> u64 {
        self.started.elapsed().as_secs()
    }

    pub fn requests(&self) -> RequestsSnapshot {
        self.total.requests()
    }

    pub fn provider(&self, provider: &str) -> Option<ProviderSnapshot> {
        let p = self.total.by_provider.get(provider)?;
        let (min, max) = self.call_ms_range.get(provider)?;
        let hits = get(&p.cache_hits);
        let misses = get(&p.cache_misses);
        let calls = get(&p.calls);
        Some(ProviderSnapshot {
            cache_hits: hits,
            cache_misses: misses,
            cache_hit_rate: hit_rate(hits, misses),
            calls,
            failures: get(&p.failures),
            skipped: get(&p.skipped),
            covers_found: get(&p.covers_found),
            avg_call_ms: (calls > 0).then(|| get(&p.call_ms_total) / calls),
            min_call_ms: (calls > 0).then(|| get(min)),
            max_call_ms: (calls > 0).then(|| get(max)),
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

    #[test]
    fn pending_counts_are_taken_and_restored() {
        let s = Stats::new(&["gb".to_string()]);
        s.cover_request();
        s.cover_request();
        s.ids_requested(5);
        s.provider_call("gb", false, 1, 120);

        let mut taken = s.take_pending();
        taken.sort();
        assert_eq!(
            taken,
            vec![
                ("gb.call_ms_total".to_string(), 120),
                ("gb.calls".to_string(), 1),
                ("gb.covers_found".to_string(), 1),
                ("requests.cover".to_string(), 2),
                ("requests.ids".to_string(), 5),
            ]
        );
        // Taken: nothing left pending, totals untouched.
        assert!(s.take_pending().is_empty());
        assert_eq!(s.requests().cover, 2);

        // A failed flush gives the counts back, on top of new ones.
        s.cover_request();
        s.restore_pending(&taken);
        assert_eq!(s.pending()["requests.cover"], 3);
        assert_eq!(s.pending()["gb.calls"], 1);
    }
}

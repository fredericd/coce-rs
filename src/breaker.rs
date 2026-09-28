use std::collections::HashMap;
use std::sync::atomic::{AtomicU32, AtomicU64, Ordering};
use std::time::{Duration, Instant};

/// Consecutive failed calls after which a provider is disabled.
const FAILURE_THRESHOLD: u32 = 3;

/// One circuit breaker per provider, so that a failing provider is
/// temporarily disabled instead of being called (and waited for) on every
/// request. The map is built once at startup and only holds atomics, so
/// checking it costs no lock or allocation.
pub struct Breakers {
    start: Instant,
    retry: Duration,
    by_provider: HashMap<String, Breaker>,
}

#[derive(Default)]
struct Breaker {
    consecutive_failures: AtomicU32,
    /// Time (ms since `Breakers::start`) until which the provider is
    /// disabled; 0 while the breaker is closed.
    open_until: AtomicU64,
}

impl Breakers {
    pub fn new(providers: &[String], retry: Duration) -> Self {
        Breakers {
            start: Instant::now(),
            retry,
            by_provider: providers
                .iter()
                .map(|p| (p.clone(), Breaker::default()))
                .collect(),
        }
    }

    /// Whether `provider` may be called now.
    ///
    /// Once the retry delay of an open breaker has elapsed, exactly one
    /// caller gets through to test the provider (the delay is pushed back
    /// for everyone else meanwhile); its outcome then closes the breaker or
    /// keeps it open.
    pub fn allow(&self, provider: &str) -> bool {
        let Some(b) = self.by_provider.get(provider) else {
            return true;
        };
        let until = b.open_until.load(Ordering::Acquire);
        if until == 0 {
            return true;
        }
        let now = self.now_ms();
        now >= until
            && b
                .open_until
                .compare_exchange(until, now + self.retry_ms(), Ordering::AcqRel, Ordering::Acquire)
                .is_ok()
    }

    pub fn record_success(&self, provider: &str) {
        let Some(b) = self.by_provider.get(provider) else {
            return;
        };
        b.consecutive_failures.store(0, Ordering::Release);
        if b.open_until.swap(0, Ordering::AcqRel) != 0 {
            tracing::info!(provider, "provider re-enabled");
        }
    }

    pub fn record_failure(&self, provider: &str) {
        let Some(b) = self.by_provider.get(provider) else {
            return;
        };
        let failures = b.consecutive_failures.fetch_add(1, Ordering::AcqRel) + 1;
        let retry_s = self.retry.as_secs();
        let until = self.now_ms() + self.retry_ms();
        if b.open_until.load(Ordering::Acquire) != 0 {
            // The test call of an open breaker failed: stay disabled.
            b.open_until.store(until, Ordering::Release);
            tracing::warn!(provider, retry_s, "provider still failing, kept disabled");
        } else if failures >= FAILURE_THRESHOLD
            && b
                .open_until
                .compare_exchange(0, until, Ordering::AcqRel, Ordering::Acquire)
                .is_ok()
        {
            tracing::warn!(provider, failures, retry_s, "provider disabled after repeated failures");
        }
    }

    /// Never 0, which `open_until` reserves for "closed".
    fn now_ms(&self) -> u64 {
        self.start.elapsed().as_millis() as u64 + 1
    }

    fn retry_ms(&self) -> u64 {
        self.retry.as_millis() as u64
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn breakers(retry: Duration) -> Breakers {
        Breakers::new(&["gb".to_string()], retry)
    }

    #[test]
    fn opens_after_threshold() {
        let b = breakers(Duration::from_secs(300));
        for _ in 0..FAILURE_THRESHOLD - 1 {
            b.record_failure("gb");
            assert!(b.allow("gb"));
        }
        b.record_failure("gb");
        assert!(!b.allow("gb"));
    }

    #[test]
    fn success_resets_failure_count() {
        let b = breakers(Duration::from_secs(300));
        for _ in 0..FAILURE_THRESHOLD - 1 {
            b.record_failure("gb");
        }
        b.record_success("gb");
        b.record_failure("gb");
        assert!(b.allow("gb"));
    }

    #[test]
    fn single_test_call_after_retry_delay() {
        let b = breakers(Duration::from_secs(300));
        for _ in 0..FAILURE_THRESHOLD {
            b.record_failure("gb");
        }
        // Pretend the retry delay has elapsed.
        b.by_provider["gb"].open_until.store(1, Ordering::Release);
        assert!(b.allow("gb"), "first caller tests the provider");
        assert!(!b.allow("gb"), "others keep skipping it meanwhile");
    }

    #[test]
    fn test_call_outcome_closes_or_keeps_open() {
        let b = breakers(Duration::from_secs(300));
        for _ in 0..FAILURE_THRESHOLD {
            b.record_failure("gb");
        }
        b.record_failure("gb");
        assert!(!b.allow("gb"));
        b.record_success("gb");
        assert!(b.allow("gb"));
    }

    #[test]
    fn unknown_provider_is_allowed() {
        assert!(breakers(Duration::from_secs(300)).allow("xx"));
    }
}

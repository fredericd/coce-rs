//! Daily activity history: the counters of `Stats` summed per day (server
//! local time) in Redis, so that they survive restarts and add up across
//! instances. One hash per day, `coce:stats:YYYY-MM-DD`, expiring after the
//! retention period (`statsDays`).

use crate::config::Config;
use crate::redis_store::RedisManager;
use crate::stats::{hit_rate, RequestsSnapshot, Stats};
use chrono::{Days, Local, NaiveDate};
use serde::Serialize;
use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;
use tokio_util::sync::CancellationToken;

/// How often the counts accumulated in memory are added to Redis.
const FLUSH_EVERY: Duration = Duration::from_secs(10);

fn key(date: NaiveDate) -> String {
    format!("coce:stats:{}", date.format("%Y-%m-%d"))
}

/// Add the counts accumulated since the last flush to today's hash. On
/// failure, the counts are kept for the next flush.
pub async fn flush(stats: &Stats, redis: &mut RedisManager, cfg: &Config) {
    let deltas = stats.take_pending();
    if deltas.is_empty() {
        return;
    }
    let key = key(Local::now().date_naive());
    let mut pipe = redis::pipe();
    for (field, delta) in &deltas {
        pipe.hincr(&key, field, *delta).ignore();
    }
    // Renewed on each flush: a day is kept `statsDays` days after its end.
    pipe.expire(&key, (i64::from(cfg.stats_days()) + 1) * 86_400).ignore();

    let timeout = Duration::from_millis(cfg.redis.timeout);
    match tokio::time::timeout(timeout, pipe.query_async::<()>(redis)).await {
        Ok(Ok(())) => {}
        Ok(Err(e)) => {
            stats.restore_pending(&deltas);
            tracing::warn!(error = %e, "daily stats flush failed, will retry");
        }
        Err(_) => {
            stats.restore_pending(&deltas);
            tracing::warn!(timeout_ms = cfg.redis.timeout, "daily stats flush timed out, will retry");
        }
    }
}

/// Flush periodically until `cancel`, without a final flush: the caller
/// flushes once all requests and background work are done.
pub async fn run_flusher(
    stats: Arc<Stats>,
    mut redis: RedisManager,
    cfg: Arc<Config>,
    cancel: CancellationToken,
) {
    let mut interval = tokio::time::interval(FLUSH_EVERY);
    interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    loop {
        tokio::select! {
            _ = interval.tick() => flush(&stats, &mut redis, &cfg).await,
            _ = cancel.cancelled() => return,
        }
    }
}

#[derive(Serialize)]
pub struct Day {
    pub date: String,
    pub requests: RequestsSnapshot,
    pub providers: serde_json::Map<String, serde_json::Value>,
}

#[derive(Serialize)]
pub struct DayProvider {
    pub cache_hits: u64,
    pub cache_misses: u64,
    pub cache_hit_rate: Option<f64>,
    pub calls: u64,
    pub failures: u64,
    pub skipped: u64,
    pub covers_found: u64,
    pub avg_call_ms: Option<u64>,
}

/// The last `days` days, most recent (today, in progress) first. `pending`,
/// counts not flushed yet, is added to today.
pub async fn read(
    redis: &mut RedisManager,
    cfg: &Config,
    days: u32,
    pending: Option<HashMap<String, u64>>,
) -> redis::RedisResult<Vec<Day>> {
    let today = Local::now().date_naive();
    let dates: Vec<NaiveDate> = (0..days)
        .filter_map(|i| today.checked_sub_days(Days::new(u64::from(i))))
        .collect();

    let mut pipe = redis::pipe();
    for date in &dates {
        pipe.hgetall(key(*date));
    }
    let mut hashes: Vec<HashMap<String, u64>> = pipe.query_async(redis).await?;
    if let (Some(pending), Some(today)) = (pending, hashes.first_mut()) {
        for (field, value) in pending {
            *today.entry(field).or_default() += value;
        }
    }

    Ok(dates
        .iter()
        .zip(hashes)
        .map(|(date, fields)| day(*date, &fields, &cfg.providers))
        .collect())
}

fn day(date: NaiveDate, fields: &HashMap<String, u64>, configured: &[String]) -> Day {
    let get = |name: &str| fields.get(name).copied().unwrap_or(0);

    // Configured providers first, in their order, then any other provider
    // with data that day (e.g. removed from the configuration since).
    let mut providers: Vec<String> = configured.to_vec();
    let mut others: Vec<String> = fields
        .keys()
        .filter_map(|f| f.split_once('.').map(|(p, _)| p.to_string()))
        .filter(|p| p != "requests" && !providers.contains(p))
        .collect();
    others.sort();
    others.dedup();
    providers.extend(others);

    let providers = providers
        .into_iter()
        .map(|p| {
            let field = |name: &str| get(&format!("{p}.{name}"));
            let (hits, misses, calls) = (field("cache_hits"), field("cache_misses"), field("calls"));
            let stats = DayProvider {
                cache_hits: hits,
                cache_misses: misses,
                cache_hit_rate: hit_rate(hits, misses),
                calls,
                failures: field("failures"),
                skipped: field("skipped"),
                covers_found: field("covers_found"),
                avg_call_ms: (calls > 0).then(|| field("call_ms_total") / calls),
            };
            (p, serde_json::to_value(stats).unwrap())
        })
        .collect();

    Day {
        date: date.format("%Y-%m-%d").to_string(),
        requests: RequestsSnapshot {
            cover: get("requests.cover"),
            rejected: get("requests.rejected"),
            ids: get("requests.ids"),
            global_timeouts: get("requests.global_timeouts"),
        },
        providers,
    }
}

/// Options of `coce stats`.
#[derive(clap::Args)]
pub struct Args {
    /// Number of days to show, today included (at most `statsDays`)
    #[arg(long)]
    pub days: Option<u32>,
    /// Print JSON (as `/stats/daily`) instead of tables
    #[arg(long)]
    pub json: bool,
}

/// `coce stats`: print the daily history. Counts not yet flushed by a
/// running server (at most `FLUSH_EVERY` old) aren't included.
pub async fn run(cfg: &Config, redis: &mut RedisManager, args: &Args) -> anyhow::Result<()> {
    let days = args.days.unwrap_or(cfg.stats_days()).clamp(1, cfg.stats_days());
    let history = read(redis, cfg, days, None).await?;
    if args.json {
        println!("{}", serde_json::to_string_pretty(&history)?);
        return Ok(());
    }
    print!("{}", render(&history));
    Ok(())
}

/// Text tables: one for requests, one per provider.
fn render(history: &[Day]) -> String {
    use std::fmt::Write;
    let mut out = String::new();
    let num = |v: Option<u64>| v.map_or("-".to_string(), |v| v.to_string());

    let _ = writeln!(out, "Requests");
    let _ = writeln!(out, "{:<12}{:>10}{:>10}{:>10}{:>10}", "date", "cover", "rejected", "ids", "timeouts");
    for d in history {
        let r = &d.requests;
        let _ = writeln!(
            out,
            "{:<12}{:>10}{:>10}{:>10}{:>10}",
            d.date, r.cover, r.rejected, r.ids, r.global_timeouts
        );
    }

    let providers: Vec<&String> = history.first().map(|d| d.providers.keys().collect()).unwrap_or_default();
    for p in providers {
        let _ = writeln!(out, "\nProvider {p}");
        let _ = writeln!(
            out,
            "{:<12}{:>10}{:>10}{:>7}{:>8}{:>10}{:>9}{:>8}{:>8}",
            "date", "hits", "misses", "hit%", "calls", "failures", "skipped", "found", "avg ms"
        );
        for d in history {
            let Some(v) = d.providers.get(p) else { continue };
            let n = |k: &str| v[k].as_u64().unwrap_or(0);
            let rate = v["cache_hit_rate"]
                .as_f64()
                .map_or("-".to_string(), |r| format!("{:.1}", r * 100.0));
            let _ = writeln!(
                out,
                "{:<12}{:>10}{:>10}{:>7}{:>8}{:>10}{:>9}{:>8}{:>8}",
                d.date,
                n("cache_hits"),
                n("cache_misses"),
                rate,
                n("calls"),
                n("failures"),
                n("skipped"),
                n("covers_found"),
                num(v["avg_call_ms"].as_u64()),
            );
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn builds_a_day_from_fields() {
        let fields: HashMap<String, u64> = [
            ("requests.cover", 10),
            ("requests.ids", 40),
            ("gb.cache_hits", 30),
            ("gb.cache_misses", 10),
            ("gb.calls", 4),
            ("gb.call_ms_total", 1000),
            ("old.calls", 2),
        ]
        .into_iter()
        .map(|(k, v)| (k.to_string(), v))
        .collect();
        let d = day(
            NaiveDate::from_ymd_opt(2026, 10, 3).unwrap(),
            &fields,
            &["ol".to_string(), "gb".to_string()],
        );
        assert_eq!(d.date, "2026-10-03");
        assert_eq!((d.requests.cover, d.requests.ids, d.requests.rejected), (10, 40, 0));
        // Configured order first, then providers only found in the data.
        assert_eq!(d.providers.keys().collect::<Vec<_>>(), ["ol", "gb", "old"]);
        assert_eq!(d.providers["gb"]["cache_hit_rate"], 0.75);
        assert_eq!(d.providers["gb"]["avg_call_ms"], 250);
        assert!(d.providers["ol"]["avg_call_ms"].is_null());

        let table = render(&[d]);
        assert!(table.contains("Provider gb"));
        assert!(table.contains("75.0"));
    }
}

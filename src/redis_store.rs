use redis::aio::ConnectionManager;
use redis::AsyncCommands;

pub type RedisManager = ConnectionManager;

pub async fn connect(host: &str, port: u16) -> redis::RedisResult<ConnectionManager> {
    let client = redis::Client::open(format!("redis://{host}:{port}/"))?;
    client.get_connection_manager().await
}

/// Fetch several keys at once, preserving order. Missing keys come back as `None`.
pub async fn get_many(
    con: &mut ConnectionManager,
    keys: &[String],
) -> redis::RedisResult<Vec<Option<String>>> {
    if keys.is_empty() {
        return Ok(vec![]);
    }
    if keys.len() == 1 {
        let value: Option<String> = con.get(&keys[0]).await?;
        return Ok(vec![value]);
    }
    con.mget(keys).await
}

/// Write several `(key, value, ttl_seconds)` entries in a single round trip.
pub async fn set_many_ex(
    con: &mut ConnectionManager,
    entries: &[(String, String, u64)],
) -> redis::RedisResult<()> {
    let mut pipe = redis::pipe();
    for (key, value, ttl_seconds) in entries {
        pipe.set_ex(key, value, *ttl_seconds).ignore();
    }
    pipe.query_async(con).await
}

pub async fn set_ex(
    con: &mut ConnectionManager,
    key: &str,
    ttl_seconds: u64,
    value: &str,
) -> redis::RedisResult<()> {
    con.set_ex(key, value, ttl_seconds).await
}

pub async fn ping(con: &mut ConnectionManager) -> redis::RedisResult<()> {
    redis::cmd("PING").query_async::<String>(con).await.map(|_| ())
}

/// Number of keys in the current database (constant time, no scan).
pub async fn dbsize(con: &mut ConnectionManager) -> redis::RedisResult<u64> {
    redis::cmd("DBSIZE").query_async(con).await
}

/// Memory used by Redis, human-readable (`used_memory_human` from INFO).
pub async fn used_memory(con: &mut ConnectionManager) -> redis::RedisResult<Option<String>> {
    let info: String = redis::cmd("INFO").arg("memory").query_async(con).await?;
    Ok(info
        .lines()
        .find_map(|l| l.strip_prefix("used_memory_human:"))
        .map(|v| v.trim().to_string()))
}

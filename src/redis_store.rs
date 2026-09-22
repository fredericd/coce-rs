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

pub async fn set_ex(
    con: &mut ConnectionManager,
    key: &str,
    ttl_seconds: u64,
    value: &str,
) -> redis::RedisResult<()> {
    con.set_ex(key, value, ttl_seconds).await
}

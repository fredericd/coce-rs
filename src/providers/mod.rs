pub mod aws;
pub mod gb;
pub mod ol;
pub mod orb;

use crate::config::Config;
use std::collections::HashMap;

/// Dispatch a fetch to the named provider. Returns whatever URLs were found,
/// keyed by the resource ID that was requested.
pub async fn call(
    provider: &str,
    ids: &[String],
    cfg: &Config,
    http: &reqwest::Client,
) -> HashMap<String, String> {
    match provider {
        "aws" => aws::fetch(ids, http).await,
        "gb" => gb::fetch(ids, http).await,
        "ol" => ol::fetch(ids, cfg, http).await,
        "orb" => orb::fetch(ids, cfg, http).await,
        _ => HashMap::new(),
    }
}

/// Google Books and Open Library both reply with a JS assignment
/// (`_SomeVar = { ... };`) instead of a bare JSON document.
pub(crate) fn extract_js_object(body: &str) -> Option<serde_json::Value> {
    let start = body.find('{')?;
    let end = body.rfind('}')?;
    if end < start {
        return None;
    }
    serde_json::from_str(&body[start..=end]).ok()
}

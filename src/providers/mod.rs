pub mod aws;
pub mod gb;
pub mod ol;
pub mod orb;

use crate::config::Config;
use std::collections::HashMap;
use tokio::time::Instant;

/// What a provider call learned about the requested IDs.
#[derive(Default)]
pub struct Outcome {
    /// Definitive answers: `Some(url)` when a cover was found, `None` when
    /// the provider answered that it has none. Both can be cached. IDs
    /// missing from the map got no reliable answer and must not be cached.
    pub answers: HashMap<String, Option<String>>,
    /// The call hit a provider-side problem (network error, timeout,
    /// throttling, server error, unexpected response). Feeds the circuit
    /// breaker.
    pub failed: bool,
}

impl Outcome {
    /// The provider couldn't be queried: nothing is known.
    pub fn failure() -> Self {
        Outcome {
            answers: HashMap::new(),
            failed: true,
        }
    }

    /// The provider replied with an error status: nothing is known, and
    /// whether it counts as a failure depends on the status.
    pub fn bad_status(status: reqwest::StatusCode) -> Self {
        Outcome {
            answers: HashMap::new(),
            failed: is_provider_failure(status),
        }
    }

    /// The provider answered for the whole batch: every requested ID not in
    /// `found` has no cover.
    pub fn complete(ids: &[String], mut found: HashMap<String, String>) -> Self {
        Outcome {
            answers: ids.iter().map(|id| (id.clone(), found.remove(id))).collect(),
            failed: false,
        }
    }
}

/// Whether an HTTP error status points to a problem on the provider's side,
/// as opposed to 400 Bad Request, which can be caused by the IDs sent: it
/// mustn't let a client disable a provider for everyone.
pub(crate) fn is_provider_failure(status: reqwest::StatusCode) -> bool {
    status != reqwest::StatusCode::BAD_REQUEST
}

/// Dispatch a fetch to the named provider. `deadline` is when the client
/// gets its response anyway (global timeout): providers issuing one request
/// per ID stop there and return what they have, so it can be cached.
pub async fn call(
    provider: &str,
    ids: &[String],
    cfg: &Config,
    http: &reqwest::Client,
    deadline: Instant,
) -> Outcome {
    match provider {
        "aws" => aws::fetch(ids, cfg, http, deadline).await,
        "gb" => gb::fetch(ids, http).await,
        "ol" => ol::fetch(ids, cfg, http).await,
        "orb" => orb::fetch(ids, cfg, http).await,
        _ => Outcome::default(),
    }
}

/// Google Books replies with a JS assignment (`_SomeVar = { ... };`) instead
/// of a bare JSON document.
pub(crate) fn extract_js_object(body: &str) -> Option<serde_json::Value> {
    let start = body.find('{')?;
    let end = body.rfind('}')?;
    if end < start {
        return None;
    }
    serde_json::from_str(&body[start..=end]).ok()
}

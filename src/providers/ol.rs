use super::{is_provider_failure, Outcome};
use crate::config::Config;
use crate::isbn;
use serde_json::Value;
use std::collections::HashMap;

/// Open Library's search returns at most one matching edition per work
/// (`editions.rows` is hardcoded to 1 server-side). When several requested
/// ISBNs belong to the same work, only one of them comes back per query, so
/// the others are queried again in a follow-up round.
const MAX_ROUNDS: usize = 3;

/// Look covers up through the Search API, which handles a whole batch of
/// ISBNs in one request. The cover taken is the one of the edition matching
/// the requested ISBN (`editions.cover_i`), not the work's cover
/// (`cover_i`), which may belong to any other edition of the work. The
/// returned URL points to the Covers API by cover ID, which, unlike access
/// by ISBN, is not rate limited.
pub async fn fetch(ids: &[String], cfg: &Config, http: &reqwest::Client) -> Outcome {
    let mut outcome = Outcome::default();

    let size = match cfg.ol.as_ref().and_then(|c| c.image_size.as_deref()) {
        Some("small") => "S",
        Some("large") => "L",
        _ => "M",
    };

    // Normalized ISBN -> IDs as requested by the client (several spellings,
    // e.g. with and without hyphens, can map to the same ISBN). Anything that
    // isn't ISBN-shaped is skipped: it can't match, and it would otherwise
    // end up in the Solr query.
    let mut pending: HashMap<String, Vec<String>> = HashMap::new();
    for id in ids {
        match isbn::normalize(id) {
            Some(isbn) => pending.entry(isbn).or_default().push(id.clone()),
            None => {
                outcome.answers.insert(id.clone(), None);
            }
        }
    }

    for _ in 0..MAX_ROUNDS {
        if pending.is_empty() {
            break;
        }
        let isbns: Vec<&str> = pending.keys().map(String::as_str).collect();
        let docs = match search(&isbns, http).await {
            Ok(docs) => docs,
            Err(failed) => {
                // Answers from previous rounds stand; the ISBNs still
                // pending are left unanswered.
                outcome.failed = failed;
                return outcome;
            }
        };

        let mut matched = false;
        for edition in docs.iter().filter_map(matching_edition) {
            let cover = edition.get("cover_i").and_then(Value::as_i64);
            let isbns = edition.get("isbn").and_then(Value::as_array);
            for isbn in isbns.into_iter().flatten().filter_map(Value::as_str) {
                let Some(requested) = pending.remove(isbn) else {
                    continue;
                };
                matched = true;
                let url =
                    cover.map(|cover| format!("https://covers.openlibrary.org/b/id/{cover}-{size}.jpg"));
                for id in requested {
                    outcome.answers.insert(id, url.clone());
                }
            }
        }

        // Whatever is still pending is either unknown to Open Library, or
        // hidden behind a sibling edition of the same work: only the latter
        // is worth another round.
        if !matched {
            break;
        }
    }

    for id in pending.into_values().flatten() {
        outcome.answers.insert(id, None);
    }
    outcome
}

/// On error, tells whether it counts as a provider failure.
async fn search(isbns: &[&str], http: &reqwest::Client) -> Result<Vec<Value>, bool> {
    let query = format!("isbn:({})", isbns.join(" OR "));
    // An ISBN can match several works (duplicate records), hence the margin.
    let limit = (isbns.len() * 3).to_string();

    let resp = match http
        .get("https://openlibrary.org/search.json")
        .query(&[
            ("q", query.as_str()),
            // `key` looks unused but is required: Open Library filters the
            // `editions` subquery on each work's key (`$row.key`), so without
            // it no edition ever matches and no cover is found.
            ("fields", "key,editions,editions.cover_i,editions.isbn"),
            ("limit", limit.as_str()),
        ])
        .send()
        .await
    {
        Ok(resp) => resp,
        Err(e) => {
            tracing::warn!(provider = "ol", error = %e, "request failed");
            return Err(true);
        }
    };
    let status = resp.status();
    if !status.is_success() {
        tracing::warn!(provider = "ol", %status, "unexpected HTTP status");
        return Err(is_provider_failure(status));
    }
    let mut json: Value = match resp.json().await {
        Ok(v) => v,
        Err(e) => {
            tracing::warn!(provider = "ol", error = %e, "unparseable response");
            return Err(true);
        }
    };
    match json.get_mut("docs").map(Value::take) {
        Some(Value::Array(docs)) => Ok(docs),
        _ => {
            tracing::warn!(provider = "ol", "response has no docs array");
            Err(true)
        }
    }
}

/// The edition of a work that matched the query (there is at most one).
fn matching_edition(work: &Value) -> Option<&Value> {
    work.get("editions")?.get("docs")?.as_array()?.first()
}

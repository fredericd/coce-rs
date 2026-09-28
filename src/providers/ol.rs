use crate::config::Config;
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
pub async fn fetch(ids: &[String], cfg: &Config, http: &reqwest::Client) -> HashMap<String, String> {
    let mut found = HashMap::new();

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
        if let Some(isbn) = normalize_isbn(id) {
            pending.entry(isbn).or_default().push(id.clone());
        }
    }

    for _ in 0..MAX_ROUNDS {
        if pending.is_empty() {
            break;
        }
        let isbns: Vec<&str> = pending.keys().map(String::as_str).collect();
        let Some(docs) = search(&isbns, http).await else {
            break;
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
                if let Some(cover) = cover {
                    let url = format!("https://covers.openlibrary.org/b/id/{cover}-{size}.jpg");
                    for id in requested {
                        found.insert(id, url.clone());
                    }
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

    found
}

async fn search(isbns: &[&str], http: &reqwest::Client) -> Option<Vec<Value>> {
    let query = format!("isbn:({})", isbns.join(" OR "));
    // An ISBN can match several works (duplicate records), hence the margin.
    let limit = (isbns.len() * 3).to_string();

    let resp = match http
        .get("https://openlibrary.org/search.json")
        .query(&[
            ("q", query.as_str()),
            ("fields", "key,editions,editions.cover_i,editions.isbn"),
            ("limit", limit.as_str()),
        ])
        .send()
        .await
    {
        Ok(resp) => resp,
        Err(e) => {
            tracing::warn!(provider = "ol", error = %e, "request failed");
            return None;
        }
    };
    let status = resp.status();
    if !status.is_success() {
        tracing::warn!(provider = "ol", %status, "unexpected HTTP status");
        return None;
    }
    let mut json: Value = match resp.json().await {
        Ok(v) => v,
        Err(e) => {
            tracing::warn!(provider = "ol", error = %e, "unparseable response");
            return None;
        }
    };
    match json.get_mut("docs").map(Value::take) {
        Some(Value::Array(docs)) => Some(docs),
        _ => {
            tracing::warn!(provider = "ol", "response has no docs array");
            None
        }
    }
}

/// The edition of a work that matched the query (there is at most one).
fn matching_edition(work: &Value) -> Option<&Value> {
    work.get("editions")?.get("docs")?.as_array()?.first()
}

/// Strip hyphens/spaces and check the result looks like an ISBN-10/13, the
/// form Open Library indexes them under.
fn normalize_isbn(id: &str) -> Option<String> {
    let isbn: String = id
        .chars()
        .filter(|c| *c != '-' && *c != ' ')
        .map(|c| c.to_ascii_uppercase())
        .collect();
    if !isbn.is_ascii() {
        return None;
    }
    let digits = |s: &str| s.bytes().all(|b| b.is_ascii_digit());
    let valid = match isbn.len() {
        13 => digits(&isbn),
        10 => digits(&isbn[..9]) && (digits(&isbn[9..]) || &isbn[9..] == "X"),
        _ => false,
    };
    valid.then_some(isbn)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalizes_isbns() {
        assert_eq!(normalize_isbn("978-0-563-53319-1").as_deref(), Some("9780563533191"));
        assert_eq!(normalize_isbn("275403143x").as_deref(), Some("275403143X"));
        assert_eq!(normalize_isbn("2847342257").as_deref(), Some("2847342257"));
    }

    #[test]
    fn rejects_non_isbns() {
        assert_eq!(normalize_isbn("12345"), None);
        assert_eq!(normalize_isbn("97805635331X1"), None);
        assert_eq!(normalize_isbn("isbn:(*)"), None);
        assert_eq!(normalize_isbn("é12345678"), None);
        assert_eq!(normalize_isbn("12345678é"), None);
    }
}

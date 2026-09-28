use super::Outcome;
use std::time::Duration;

/// Amazon has no public cover-lookup API; instead we probe the predictable
/// direct image URL with a HEAD request. A 200 or 403 both mean the image
/// exists (403 happens for some valid ASINs behind hotlink protection).
///
/// When Amazon has no cover for an ISBN, it doesn't 404: it serves a 1x1
/// placeholder GIF with a 200 status instead. Real covers are always JPEG,
/// so a `content-type: image/gif` response is that placeholder, not a cover.
///
/// IDs with no ISBN-10 equivalent (979-prefixed ISBN-13) are skipped: Amazon
/// would map them to an unrelated book's cover (see `to_amazon_key`).
///
/// On a network error, throttling (429) or server error (5xx), the remaining
/// IDs are left unanswered rather than probed anyway: they would most likely
/// fail the same way, each one adding to the response time.
pub async fn fetch(ids: &[String], http: &reqwest::Client) -> Outcome {
    let mut outcome = Outcome::default();

    let mut keys: Vec<(&String, String)> = Vec::with_capacity(ids.len());
    for id in ids {
        match to_amazon_key(id) {
            Some(key) => keys.push((id, key)),
            None => {
                outcome.answers.insert(id.clone(), None);
            }
        }
    }

    for (idx, (id, search)) in keys.iter().enumerate() {
        let url = format!(
            "https://images-na.ssl-images-amazon.com/images/P/{search}.01.MZZZZZZZZZ.jpg"
        );

        match http
            .head(&url)
            .header("user-agent", "Mozilla/5.0")
            .send()
            .await
        {
            Ok(resp) => {
                let status = resp.status().as_u16();
                let content_type = resp
                    .headers()
                    .get(reqwest::header::CONTENT_TYPE)
                    .and_then(|v| v.to_str().ok())
                    .unwrap_or("");
                let is_placeholder = content_type.starts_with("image/gif");
                tracing::debug!(provider = "aws", %id, status, content_type, "probe");
                match status {
                    200 | 403 if !is_placeholder => {
                        outcome.answers.insert((*id).clone(), Some(url));
                    }
                    200 | 404 => {
                        outcome.answers.insert((*id).clone(), None);
                    }
                    429 | 500.. => {
                        tracing::warn!(provider = "aws", %id, status, "unexpected HTTP status");
                        outcome.failed = true;
                        break;
                    }
                    _ => tracing::warn!(provider = "aws", %id, status, "unexpected HTTP status"),
                }
            }
            Err(e) => {
                tracing::warn!(provider = "aws", %id, error = %e, "request failed");
                outcome.failed = true;
                break;
            }
        }

        // Amazon throttles/blocks bursts of HEAD requests; space them out.
        // There may be another more efficient method...
        if idx + 1 < keys.len() {
            tokio::time::sleep(Duration::from_millis(30)).await;
        }
    }

    outcome
}

/// ISBN13 -> ISBN10 conversion (Amazon's image path is keyed by ISBN10/ASIN).
///
/// Only 978-prefixed ISBN-13 have an ISBN-10 equivalent. A 979-prefixed one
/// has none: dropping its prefix yields the ISBN-10 of an unrelated
/// 978-prefixed book, and Amazon does the same when given the ISBN-13
/// directly, so it can only ever return a wrong cover. Those get `None`.
fn to_amazon_key(id: &str) -> Option<String> {
    let stripped: String = id.chars().filter(|c| *c != '-').collect();
    if stripped.len() != 13 {
        return Some(stripped);
    }
    if !stripped.starts_with("978") {
        return None;
    }

    let core: String = stripped.chars().skip(3).take(9).collect();
    let checksum: u32 = core
        .chars()
        .enumerate()
        .map(|(i, c)| c.to_digit(10).unwrap_or(0) * (i as u32 + 1))
        .sum::<u32>()
        % 11;
    let check_char = if checksum == 10 {
        "X".to_string()
    } else {
        checksum.to_string()
    };

    Some(format!("{core}{check_char}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn isbn13_to_isbn10() {
        assert_eq!(to_amazon_key("9780821417492").as_deref(), Some("0821417495"));
        assert_eq!(to_amazon_key("978-0-8214-1749-2").as_deref(), Some("0821417495"));
    }

    #[test]
    fn isbn10_passthrough() {
        assert_eq!(to_amazon_key("275403143X").as_deref(), Some("275403143X"));
    }

    #[test]
    fn isbn13_979_has_no_amazon_key() {
        assert_eq!(to_amazon_key("9798885795692"), None);
        assert_eq!(to_amazon_key("979-8-88579-569-2"), None);
    }
}

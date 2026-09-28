use std::collections::HashMap;
use std::time::Duration;

/// Amazon has no public cover-lookup API; instead we probe the predictable
/// direct image URL with a HEAD request. A 200 or 403 both mean the image
/// exists (403 happens for some valid ASINs behind hotlink protection).
///
/// When Amazon has no cover for an ISBN, it doesn't 404: it serves a 1x1
/// placeholder GIF with a 200 status instead. Real covers are always JPEG,
/// so a `content-type: image/gif` response is that placeholder, not a cover.
pub async fn fetch(ids: &[String], http: &reqwest::Client) -> HashMap<String, String> {
    let mut found = HashMap::new();

    for (idx, id) in ids.iter().enumerate() {
        let search = to_amazon_key(id);
        let url = format!(
            "https://images-na.ssl-images-amazon.com/images/P/{search}.01.MZZZZZZZZZ.jpg"
        );

        if let Ok(resp) = http
            .head(&url)
            .header("user-agent", "Mozilla/5.0")
            .send()
            .await
        {
            let status = resp.status().as_u16();
            let is_placeholder = resp
                .headers()
                .get(reqwest::header::CONTENT_TYPE)
                .and_then(|v| v.to_str().ok())
                .is_some_and(|ct| ct.starts_with("image/gif"));
            if (status == 200 || status == 403) && !is_placeholder {
                found.insert(id.clone(), url);
            }
        }

        // Amazon throttles/blocks bursts of HEAD requests; space them out.
        // There may be another more efficient method...
        if idx + 1 < ids.len() {
            tokio::time::sleep(Duration::from_millis(30)).await;
        }
    }

    found
}

/// ISBN13 -> ISBN10 conversion (Amazon's image path is keyed by ISBN10/ASIN).
fn to_amazon_key(id: &str) -> String {
    let stripped: String = id.chars().filter(|c| *c != '-').collect();
    if stripped.len() != 13 {
        return stripped;
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

    format!("{core}{check_char}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn isbn13_to_isbn10() {
        assert_eq!(to_amazon_key("9780821417492"), "0821417495");
    }

    #[test]
    fn isbn10_passthrough() {
        assert_eq!(to_amazon_key("275403143X"), "275403143X");
    }
}

use super::{extract_js_object, Outcome};
use serde_json::Value;
use std::collections::HashMap;

pub async fn fetch(ids: &[String], http: &reqwest::Client) -> Outcome {
    let url = format!(
        "https://books.google.com/books?bibkeys={}&jscmd=viewapi&hl=en",
        ids.join(",")
    );

    let resp = match http.get(&url).send().await {
        Ok(resp) => resp,
        Err(e) => {
            tracing::warn!(provider = "gb", error = %e, "request failed");
            return Outcome::failure();
        }
    };
    let status = resp.status();
    if !status.is_success() {
        tracing::warn!(provider = "gb", %status, "unexpected HTTP status");
        return Outcome::bad_status(status);
    }
    let body = match resp.text().await {
        Ok(t) => t,
        Err(e) => {
            tracing::warn!(provider = "gb", error = %e, "cannot read response body");
            return Outcome::failure();
        }
    };

    let Some(Value::Object(items)) = extract_js_object(&body) else {
        tracing::warn!(provider = "gb", "unparseable response");
        return Outcome::failure();
    };

    let mut found = HashMap::new();
    for (_, item) in items {
        let bib_key = item.get("bib_key").and_then(Value::as_str);
        let thumbnail = item.get("thumbnail_url").and_then(Value::as_str);
        if let (Some(id), Some(url)) = (bib_key, thumbnail) {
            // Bump the returned thumbnail up to medium size.
            found.insert(id.to_string(), url.replace("zoom=5", "zoom=1"));
        }
    }

    Outcome::complete(ids, found)
}

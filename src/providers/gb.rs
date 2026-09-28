use super::extract_js_object;
use serde_json::Value;
use std::collections::HashMap;

pub async fn fetch(ids: &[String], http: &reqwest::Client) -> HashMap<String, String> {
    let mut found = HashMap::new();

    let url = format!(
        "https://books.google.com/books?bibkeys={}&jscmd=viewapi&hl=en",
        ids.join(",")
    );

    let resp = match http.get(&url).send().await {
        Ok(resp) => resp,
        Err(e) => {
            tracing::warn!(provider = "gb", error = %e, "request failed");
            return found;
        }
    };
    let status = resp.status();
    if !status.is_success() {
        tracing::warn!(provider = "gb", %status, "unexpected HTTP status");
        return found;
    }
    let body = match resp.text().await {
        Ok(t) => t,
        Err(e) => {
            tracing::warn!(provider = "gb", error = %e, "cannot read response body");
            return found;
        }
    };

    let Some(Value::Object(items)) = extract_js_object(&body) else {
        tracing::warn!(provider = "gb", "unparseable response");
        return found;
    };

    for (_, item) in items {
        let bib_key = item.get("bib_key").and_then(Value::as_str);
        let thumbnail = item.get("thumbnail_url").and_then(Value::as_str);
        if let (Some(id), Some(url)) = (bib_key, thumbnail) {
            // Bump the returned thumbnail up to medium size.
            found.insert(id.to_string(), url.replace("zoom=5", "zoom=1"));
        }
    }

    found
}

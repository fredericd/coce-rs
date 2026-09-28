use super::extract_js_object;
use crate::config::Config;
use serde_json::Value;
use std::collections::HashMap;

pub async fn fetch(ids: &[String], cfg: &Config, http: &reqwest::Client) -> HashMap<String, String> {
    let mut found = HashMap::new();

    let url = format!(
        "http://openlibrary.org/api/books?bibkeys={}&jscmd=data",
        ids.join(",")
    );

    let resp = match http.get(&url).send().await {
        Ok(resp) => resp,
        Err(e) => {
            tracing::warn!(provider = "ol", error = %e, "request failed");
            return found;
        }
    };
    let status = resp.status();
    if !status.is_success() {
        tracing::warn!(provider = "ol", %status, "unexpected HTTP status");
        return found;
    }
    let body = match resp.text().await {
        Ok(t) => t,
        Err(e) => {
            tracing::warn!(provider = "ol", error = %e, "cannot read response body");
            return found;
        }
    };

    let Some(Value::Object(items)) = extract_js_object(&body) else {
        tracing::warn!(provider = "ol", "unparseable response");
        return found;
    };

    let image_size = cfg
        .ol
        .as_ref()
        .and_then(|c| c.image_size.clone())
        .unwrap_or_else(|| "medium".to_string());

    for (id, item) in items {
        if let Some(url) = item
            .get("cover")
            .and_then(|c| c.get(&image_size))
            .and_then(Value::as_str)
        {
            found.insert(id, url.to_string());
        }
    }

    found
}

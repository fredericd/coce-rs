use super::Outcome;
use crate::config::Config;
use std::collections::HashMap;

pub async fn fetch(ids: &[String], cfg: &Config, http: &reqwest::Client) -> Outcome {
    let Some(orb_cfg) = cfg.orb.as_ref() else {
        tracing::warn!(provider = "orb", "provider enabled but not configured");
        return Outcome::failure();
    };
    let (Some(user), Some(key)) = (orb_cfg.user.as_ref(), orb_cfg.key.as_ref()) else {
        tracing::warn!(provider = "orb", "missing user or key in configuration");
        return Outcome::failure();
    };

    let url = format!(
        "https://api.base-orb.fr/v1/products?eans={}&sort=ean_asc",
        ids.join(",")
    );

    let resp = match http.get(&url).basic_auth(user, Some(key)).send().await {
        Ok(r) => r,
        Err(e) => {
            tracing::warn!(provider = "orb", error = %e, "request failed");
            return Outcome::failure();
        }
    };
    let status = resp.status();
    if !status.is_success() {
        // 401/403 here almost always means bad credentials.
        tracing::warn!(provider = "orb", %status, "unexpected HTTP status");
        return Outcome::bad_status(status);
    }
    let json: serde_json::Value = match resp.json().await {
        Ok(v) => v,
        Err(e) => {
            tracing::warn!(provider = "orb", error = %e, "unparseable response");
            return Outcome::failure();
        }
    };

    // ORB serves each front cover as a thumbnail (160 px high) and an
    // original (500 px high). Thumbnail unless `imageSize` asks for the original,
    // which falls back to the thumbnail when missing.
    let sizes: &[&str] = match orb_cfg.image_size.as_deref() {
        Some("original") => &["original", "thumbnail"],
        _ => &["thumbnail"],
    };

    let mut found = HashMap::new();
    if let Some(items) = json.get("data").and_then(|d| d.as_array()) {
        for item in items {
            let ean = item.get("ean13").and_then(|v| v.as_str());
            let front = item.get("images").and_then(|i| i.get("front"));
            let url = sizes.iter().find_map(|size| {
                front
                    .and_then(|f| f.get(*size))
                    .and_then(|i| i.get("src"))
                    .and_then(|s| s.as_str())
            });
            if let (Some(id), Some(url)) = (ean, url) {
                found.insert(id.to_string(), url.to_string());
            }
        }
    }

    Outcome::complete(ids, found)
}

use crate::config::Config;
use std::collections::HashMap;

pub async fn fetch(ids: &[String], cfg: &Config, http: &reqwest::Client) -> HashMap<String, String> {
    let mut found = HashMap::new();

    let Some(orb_cfg) = cfg.orb.as_ref() else {
        return found;
    };
    let (Some(user), Some(key)) = (orb_cfg.user.as_ref(), orb_cfg.key.as_ref()) else {
        return found;
    };

    let url = format!(
        "https://api.base-orb.fr/v1/products?eans={}&sort=ean_asc",
        ids.join(",")
    );

    let resp = match http.get(&url).basic_auth(user, Some(key)).send().await {
        Ok(r) => r,
        Err(_) => return found,
    };
    let json: serde_json::Value = match resp.json().await {
        Ok(v) => v,
        Err(_) => return found,
    };

    if let Some(items) = json.get("data").and_then(|d| d.as_array()) {
        for item in items {
            let ean = item.get("ean13").and_then(|v| v.as_str());
            let thumbnail = item
                .get("images")
                .and_then(|i| i.get("front"))
                .and_then(|f| f.get("thumbnail"))
                .and_then(|t| t.get("src"))
                .and_then(|s| s.as_str());
            if let (Some(id), Some(url)) = (ean, thumbnail) {
                found.insert(id.to_string(), url.to_string());
            }
        }
    }

    found
}

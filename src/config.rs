use serde::{Deserialize, Deserializer};
use std::fs;
use std::path::Path;

fn port_from_str_or_num<'de, D>(deserializer: D) -> Result<u16, D::Error>
where
    D: Deserializer<'de>,
{
    #[derive(Deserialize)]
    #[serde(untagged)]
    enum StrOrNum {
        Str(String),
        Num(u16),
    }
    match StrOrNum::deserialize(deserializer)? {
        StrOrNum::Str(s) => s.parse().map_err(serde::de::Error::custom),
        StrOrNum::Num(n) => Ok(n),
    }
}

#[derive(Debug, Clone, Deserialize)]
pub struct RedisConfig {
    pub host: String,
    pub port: u16,
    /// Max time (ms) to wait for a Redis reply before treating IDs as not cached.
    pub timeout: u64,
}

impl Default for RedisConfig {
    fn default() -> Self {
        RedisConfig {
            host: "127.0.0.1".to_string(),
            port: 6379,
            timeout: 5000,
        }
    }
}

#[derive(Debug, Clone, Default, Deserialize)]
pub struct CacheConfig {
    #[serde(default)]
    pub path: String,
    #[serde(default)]
    pub url: String,
}

#[derive(Debug, Clone, Deserialize, Default)]
pub struct ProviderConfig {
    #[serde(default)]
    pub timeout: u64,
    #[serde(default)]
    pub cache: bool,
    #[serde(default, rename = "imageSize")]
    pub image_size: Option<String>,
    #[serde(default)]
    pub user: Option<String>,
    #[serde(default)]
    pub key: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct Config {
    #[serde(deserialize_with = "port_from_str_or_num")]
    pub port: u16,
    pub providers: Vec<String>,
    /// Global fetch timeout in milliseconds.
    pub timeout: u64,
    #[serde(default)]
    pub redis: RedisConfig,
    #[serde(default)]
    pub cache: Option<CacheConfig>,
    #[serde(default)]
    pub gb: Option<ProviderConfig>,
    #[serde(default)]
    pub aws: Option<ProviderConfig>,
    #[serde(default)]
    pub ol: Option<ProviderConfig>,
    #[serde(default)]
    pub orb: Option<ProviderConfig>,
}

impl Default for Config {
    fn default() -> Self {
        Config {
            port: 8080,
            providers: vec!["aws".to_string(), "gb".to_string(), "ol".to_string()],
            timeout: 8000,
            redis: RedisConfig::default(),
            cache: None,
            gb: None,
            aws: None,
            ol: None,
            orb: None,
        }
    }
}

impl Config {
    /// Load config from `path` if it exists, otherwise start from built-in
    /// defaults (handy for container deployments driven entirely by env
    /// vars). Either way, `COCE_*` environment variables are applied on top
    /// and win over whatever the file says.
    pub fn load(path: impl AsRef<Path>) -> anyhow::Result<Config> {
        let path = path.as_ref();
        let mut cfg = if path.exists() {
            let raw = fs::read_to_string(path)?;
            serde_json::from_str(&raw)?
        } else {
            Config::default()
        };
        cfg.apply_env_overrides();
        Ok(cfg)
    }

    pub fn provider_config(&self, name: &str) -> Option<&ProviderConfig> {
        match name {
            "gb" => self.gb.as_ref(),
            "aws" => self.aws.as_ref(),
            "ol" => self.ol.as_ref(),
            "orb" => self.orb.as_ref(),
            _ => None,
        }
    }

    fn apply_env_overrides(&mut self) {
        if let Some(v) = env_u16("COCE_PORT") {
            self.port = v;
        }
        if let Some(v) = env_string("COCE_PROVIDERS") {
            self.providers = v
                .split(',')
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .map(str::to_string)
                .collect();
        }
        if let Some(v) = env_u64("COCE_TIMEOUT") {
            self.timeout = v;
        }

        if let Some(v) = env_string("COCE_REDIS_HOST") {
            self.redis.host = v;
        }
        if let Some(v) = env_u16("COCE_REDIS_PORT") {
            self.redis.port = v;
        }
        if let Some(v) = env_u64("COCE_REDIS_TIMEOUT") {
            self.redis.timeout = v;
        }

        let cache_path = env_string("COCE_CACHE_PATH");
        let cache_url = env_string("COCE_CACHE_URL");
        if cache_path.is_some() || cache_url.is_some() {
            let cache = self.cache.get_or_insert_with(CacheConfig::default);
            if let Some(v) = cache_path {
                cache.path = v;
            }
            if let Some(v) = cache_url {
                cache.url = v;
            }
        }

        Self::apply_provider_env(&mut self.gb, "GB");
        Self::apply_provider_env(&mut self.aws, "AWS");
        Self::apply_provider_env(&mut self.ol, "OL");
        Self::apply_provider_env(&mut self.orb, "ORB");
    }

    fn apply_provider_env(slot: &mut Option<ProviderConfig>, prefix: &str) {
        let timeout = env_u64(&format!("COCE_{prefix}_TIMEOUT"));
        let cache = env_bool(&format!("COCE_{prefix}_CACHE"));
        let image_size = env_string(&format!("COCE_{prefix}_IMAGE_SIZE"));
        let user = env_string(&format!("COCE_{prefix}_USER"));
        let key = env_string(&format!("COCE_{prefix}_KEY"));

        if timeout.is_none() && cache.is_none() && image_size.is_none() && user.is_none() && key.is_none() {
            return;
        }

        let cfg = slot.get_or_insert_with(ProviderConfig::default);
        if let Some(v) = timeout {
            cfg.timeout = v;
        }
        if let Some(v) = cache {
            cfg.cache = v;
        }
        if let Some(v) = image_size {
            cfg.image_size = Some(v);
        }
        if let Some(v) = user {
            cfg.user = Some(v);
        }
        if let Some(v) = key {
            cfg.key = Some(v);
        }
    }
}

fn env_string(name: &str) -> Option<String> {
    std::env::var(name).ok().filter(|v| !v.is_empty())
}

fn env_u16(name: &str) -> Option<u16> {
    env_string(name).and_then(|v| v.parse().ok())
}

fn env_u64(name: &str) -> Option<u64> {
    env_string(name).and_then(|v| v.parse().ok())
}

fn env_bool(name: &str) -> Option<bool> {
    env_string(name).and_then(|v| match v.to_lowercase().as_str() {
        "1" | "true" | "yes" | "on" => Some(true),
        "0" | "false" | "no" | "off" => Some(false),
        _ => None,
    })
}

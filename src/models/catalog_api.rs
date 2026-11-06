//! models.dev transport, provider allowlist and catalog decoding.

use crate::error::MarsError;
use serde::{Deserialize, Serialize};
use std::time::Duration;

// DNS, redirects and body reads fit inside the 120-second refresh claim lease.
pub(super) const CATALOG_HTTP_DEADLINE_SECS: u64 = 60;

/// A single model entry in the cache.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CachedModel {
    pub id: String,
    pub provider: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub release_date: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub context_window: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_output: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cost_input: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cost_output: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cost_cache_read: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cost_cache_write: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cost_reasoning: Option<f64>,
}

pub fn default_catalog_providers() -> Vec<String> {
    DEFAULT_CATALOG_PROVIDERS
        .iter()
        .map(|provider| (*provider).to_string())
        .collect()
}

const DEFAULT_CATALOG_PROVIDERS: &[&str] = &[
    "anthropic",
    "openai",
    "google",
    "meta",
    "deepseek",
    "xai",
    "openrouter",
];

/// Fetch models from the models.dev API.
///
/// Returns a list of cached model entries. On network failure, returns an error
/// (callers should fall back to existing cache or explicit pinned IDs).
pub fn fetch_models() -> Result<Vec<CachedModel>, MarsError> {
    fetch_models_with_providers(&default_catalog_providers())
}

pub fn fetch_models_with_providers(providers: &[String]) -> Result<Vec<CachedModel>, MarsError> {
    let url = models_api_url();
    let agent: ureq::Agent = ureq::Agent::config_builder()
        .timeout_global(Some(Duration::from_secs(CATALOG_HTTP_DEADLINE_SECS)))
        .timeout_connect(Some(Duration::from_secs(15)))
        .timeout_recv_response(Some(Duration::from_secs(15)))
        .timeout_recv_body(Some(Duration::from_secs(15)))
        .build()
        .into();

    let response = agent.get(&url).call().map_err(|e| match e {
        ureq::Error::StatusCode(status) => MarsError::Http {
            url: url.clone(),
            status,
            message: format!("request failed with HTTP status {status}"),
        },
        _ => MarsError::Http {
            url: url.clone(),
            status: 0,
            message: format!("failed to fetch models catalog: {e}"),
        },
    })?;
    let body = response
        .into_body()
        .read_to_string()
        .map_err(|e| MarsError::Http {
            url: url.clone(),
            status: 0,
            message: format!("failed to read response body: {e}"),
        })?;
    let raw: serde_json::Value =
        serde_json::from_str(&body).map_err(|e| crate::error::ConfigError::Invalid {
            message: format!("failed to parse models API response: {e}"),
        })?;

    parse_models_dev_catalog_with_providers(&raw, providers)
}

fn models_api_url() -> String {
    std::env::var("MARS_MODELS_API_URL").unwrap_or_else(|_| "https://models.dev/api.json".into())
}

fn parse_models_dev_catalog_with_providers(
    raw: &serde_json::Value,
    allowlist: &[String],
) -> Result<Vec<CachedModel>, MarsError> {
    let providers = raw
        .as_object()
        .ok_or_else(|| crate::error::ConfigError::Invalid {
            message: "models API response must be an object keyed by provider".to_string(),
        })?;

    let mut models = Vec::new();

    for (provider_key, provider_obj) in providers {
        if !is_catalog_provider_allowed(provider_key, allowlist) {
            continue;
        }

        let Some(provider_models) = provider_obj.get("models").and_then(|m| m.as_object()) else {
            continue;
        };

        for model_obj in provider_models.values() {
            let Some(model_id) = model_obj.get("id").and_then(|v| v.as_str()) else {
                continue;
            };
            let release_date = model_obj
                .get("release_date")
                .and_then(|v| v.as_str())
                .map(str::to_string);
            let description = model_obj
                .get("name")
                .and_then(|v| v.as_str())
                .map(str::to_string);
            let context_window = model_obj
                .get("limit")
                .and_then(|v| v.get("context"))
                .and_then(|v| v.as_u64());
            let max_output = model_obj
                .get("limit")
                .and_then(|v| v.get("output"))
                .and_then(|v| v.as_u64());
            let cost = model_obj.get("cost");
            let cost_input = cost.and_then(|v| v.get("input")).and_then(|v| v.as_f64());
            let cost_output = cost.and_then(|v| v.get("output")).and_then(|v| v.as_f64());
            let cost_cache_read = cost
                .and_then(|v| v.get("cache_read"))
                .and_then(|v| v.as_f64());
            let cost_cache_write = cost
                .and_then(|v| v.get("cache_write"))
                .and_then(|v| v.as_f64());
            let cost_reasoning = cost
                .and_then(|v| v.get("reasoning"))
                .and_then(|v| v.as_f64());

            models.push(CachedModel {
                id: model_id.to_string(),
                provider: normalize_provider(provider_key),
                release_date,
                description,
                context_window,
                max_output,
                cost_input,
                cost_output,
                cost_cache_read,
                cost_cache_write,
                cost_reasoning,
            });
        }
    }

    Ok(models)
}

fn is_catalog_provider_allowed(provider_key: &str, allowlist: &[String]) -> bool {
    if allowlist.iter().any(|provider| provider.trim() == "*") {
        return true;
    }
    allowlist
        .iter()
        .any(|allowed| catalog_providers_match(allowed, provider_key))
}

fn catalog_providers_match(allowed: &str, provider_key: &str) -> bool {
    catalog_provider_canonical(allowed) == catalog_provider_canonical(provider_key)
}

fn catalog_provider_canonical(key: &str) -> String {
    match key.trim().to_ascii_lowercase().as_str() {
        "meta-llama" => "meta".to_string(),
        "mistralai" => "mistral".to_string(),
        other => other.to_string(),
    }
}

/// Normalize models.dev provider keys to canonical names.
fn normalize_provider(slug: &str) -> String {
    match slug {
        "anthropic" => "Anthropic".to_string(),
        "openai" => "OpenAI".to_string(),
        "google" => "Google".to_string(),
        "meta-llama" | "meta" => "Meta".to_string(),
        "mistralai" | "mistral" => "Mistral".to_string(),
        "deepseek" => "DeepSeek".to_string(),
        "cohere" => "Cohere".to_string(),
        "xai" => "xAI".to_string(),
        "openrouter" => "OpenRouter".to_string(),
        _ => slug.to_string(),
    }
}

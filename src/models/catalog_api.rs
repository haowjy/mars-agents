//! models.dev transport, provider allowlist and catalog decoding.

use crate::error::MarsError;
use mars_schema::models::{
    CachedModel, DEFAULT_CATALOG_PROVIDERS, ModelReasoningRow, normalize_provider,
};
use std::time::Duration;

// DNS, redirects and body reads fit inside the 120-second refresh claim lease.
pub(super) const CATALOG_HTTP_DEADLINE_SECS: u64 = 60;

pub fn default_catalog_providers() -> Vec<String> {
    DEFAULT_CATALOG_PROVIDERS
        .iter()
        .map(|provider| (*provider).to_string())
        .collect()
}

/// Fetch every provider's models from the models.dev API. Which providers a project sees is
/// decided when the snapshot is read ([`retain_providers`]), not here: the snapshot is shared.
pub fn fetch_models() -> Result<Vec<CachedModel>, MarsError> {
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

    parse_models_dev_catalog(&raw)
}

fn models_api_url() -> String {
    std::env::var("MARS_MODELS_API_URL").unwrap_or_else(|_| "https://models.dev/api.json".into())
}

fn parse_models_dev_catalog(raw: &serde_json::Value) -> Result<Vec<CachedModel>, MarsError> {
    let providers = raw
        .as_object()
        .ok_or_else(|| crate::error::ConfigError::Invalid {
            message: "models API response must be an object keyed by provider".to_string(),
        })?;

    let mut models = Vec::new();

    for (provider_key, provider_obj) in providers {
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
                provider_key: provider_key.to_string(),
                release_date,
                description,
                context_window,
                max_output,
                cost_input,
                cost_output,
                cost_cache_read,
                cost_cache_write,
                cost_reasoning,
                reasoning: ModelReasoningRow::from_models_dev(model_obj),
            });
        }
    }

    Ok(models)
}

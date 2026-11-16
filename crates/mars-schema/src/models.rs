//! The models.dev catalog row and the pure alias resolver over it.
//!
//! Rows are the shape Mars snapshots and the shape Meridian Flow holds, so both resolve
//! aliases and read a model's reasoning the same way.

use serde::{Deserialize, Serialize};
use serde_json::Value;

/// One models.dev model.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct CachedModel {
    pub id: String,
    /// Display name ("Anthropic", "xAI").
    pub provider: String,
    /// models.dev's own provider key ("anthropic", "meta-llama"). `meta` and `meta-llama`
    /// both display as "Meta", so the key is what tells providers apart. Snapshots written
    /// before it existed lack it; readers fill it with [`provider_key_from_display`].
    #[serde(default)]
    pub provider_key: String,
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
    /// `None` when models.dev says nothing about reasoning (rows from older snapshots).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reasoning: Option<ModelReasoningRow>,
}

/// models.dev's `reasoning` and `reasoning_options` for one model, in its own shape.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct ModelReasoningRow {
    pub reasoning: bool,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub options: Vec<ReasoningOption>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ReasoningOption {
    Effort {
        values: Vec<String>,
    },
    Toggle,
    BudgetTokens {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        min: Option<u64>,
    },
    /// An option kind this build does not know.
    #[serde(other)]
    Other,
}

impl ModelReasoningRow {
    /// Decodes a models.dev model object. `None` when it has no `reasoning` field.
    pub fn from_models_dev(model: &Value) -> Option<Self> {
        let reasoning = model.get("reasoning")?.as_bool()?;
        let options = model
            .get("reasoning_options")
            .and_then(Value::as_array)
            .map(|options| {
                options
                    .iter()
                    .filter_map(|option| serde_json::from_value(option.clone()).ok())
                    .collect()
            })
            .unwrap_or_default();
        Some(Self { reasoning, options })
    }
}

/// models.dev provider key as `catalog_providers` and alias `provider` fields spell it, with
/// the spellings models.dev has used for one provider folded together.
pub fn catalog_provider_canonical(key: &str) -> String {
    match key.trim().to_ascii_lowercase().as_str() {
        "meta-llama" => "meta".to_string(),
        "mistralai" => "mistral".to_string(),
        other => other.to_string(),
    }
}

/// The display name of a models.dev provider key.
pub fn normalize_provider(key: &str) -> String {
    match key {
        "anthropic" => "Anthropic".to_string(),
        "openai" => "OpenAI".to_string(),
        "google" => "Google".to_string(),
        "meta-llama" | "meta" => "Meta".to_string(),
        "mistralai" | "mistral" => "Mistral".to_string(),
        "deepseek" => "DeepSeek".to_string(),
        "cohere" => "Cohere".to_string(),
        "xai" => "xAI".to_string(),
        "openrouter" => "OpenRouter".to_string(),
        _ => key.to_string(),
    }
}

/// The key of a row that only has a display name. Holds for every default catalog provider.
pub fn provider_key_from_display(display: &str) -> String {
    display.to_ascii_lowercase()
}

/// The provider a model id's prefix names, for ids no alias or row gives one for.
pub fn infer_provider_from_model_id(model_id: &str) -> Option<&'static str> {
    let id = model_id.to_lowercase();
    if id.starts_with("claude-") {
        return Some("anthropic");
    }
    if id.starts_with("gpt-")
        || id.starts_with("o1")
        || id.starts_with("o3")
        || id.starts_with("o4")
        || id.starts_with("codex-")
    {
        return Some("openai");
    }
    if id.starts_with("gemini") {
        return Some("google");
    }
    if id.starts_with("llama") {
        return Some("meta");
    }
    if id.starts_with("mistral") || id.starts_with("codestral") {
        return Some("mistral");
    }
    if id.starts_with("deepseek") {
        return Some("deepseek");
    }
    if id.starts_with("command") {
        return Some("cohere");
    }
    None
}

/// `*` globs over a whole string; no other metacharacter.
pub fn glob_match(pattern: &str, text: &str) -> bool {
    let segments: Vec<&str> = pattern.split('*').collect();

    if segments.len() == 1 {
        return pattern == text;
    }

    let mut pos = 0;

    if let Some(first) = segments.first()
        && !first.is_empty()
    {
        if !text.starts_with(first) {
            return false;
        }
        pos = first.len();
    }

    if let Some(last) = segments.last()
        && !last.is_empty()
        && !text[pos..].ends_with(last)
    {
        return false;
    }

    let end = if let Some(last) = segments.last() {
        if !last.is_empty() {
            text.len() - last.len()
        } else {
            text.len()
        }
    } else {
        text.len()
    };

    for segment in &segments[1..segments.len().saturating_sub(1)] {
        if segment.is_empty() {
            continue;
        }
        if let Some(idx) = text[pos..end].find(segment) {
            pos += idx + segment.len();
        } else {
            return false;
        }
    }

    pos <= end
}

/// Every row an `auto` alias selects, best first: all `match` globs hit, no `exclude` glob
/// hits, the provider (when set) matches by canonical key, `-latest` ids are skipped, newest
/// `release_date` first (a row with none sorts last), then the shorter id, then alphabetical.
pub fn auto_resolve_all<'a>(
    provider: Option<&str>,
    match_patterns: &[String],
    exclude_patterns: &[String],
    models: &'a [CachedModel],
) -> Vec<&'a CachedModel> {
    let provider = provider.map(catalog_provider_canonical);
    let mut candidates: Vec<&CachedModel> = models
        .iter()
        .filter(|m| {
            provider
                .as_deref()
                .is_none_or(|p| catalog_provider_canonical(&m.provider_key) == p)
        })
        .filter(|m| !m.id.ends_with("-latest"))
        .filter(|m| match_patterns.iter().all(|p| glob_match(p, &m.id)))
        .filter(|m| !exclude_patterns.iter().any(|p| glob_match(p, &m.id)))
        .collect();
    sort_best_first(&mut candidates);
    candidates
}

pub fn sort_best_first(candidates: &mut [&CachedModel]) {
    candidates.sort_by(|a, b| {
        let date_cmp = b
            .release_date
            .as_deref()
            .unwrap_or("")
            .cmp(a.release_date.as_deref().unwrap_or(""));
        date_cmp
            .then_with(|| a.id.len().cmp(&b.id.len()))
            .then_with(|| a.id.cmp(&b.id))
    });
}

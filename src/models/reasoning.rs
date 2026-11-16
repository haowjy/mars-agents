//! Which models.dev row a harness's model is, and what effort that row lets the harness send.
//!
//! The row must be the provider the harness calls: `openai/o3` takes low, medium and high, while
//! `openrouter/openai/o3` is toggle only. Each harness has one join, here.

use mars_schema::agent::EffortRequest;
use mars_schema::effort::{
    EffortResolution, Levels, ModelReasoning, Move, MoveReason, allowed_levels, harness_levels,
    resolve_effort,
};
use mars_schema::models::{CachedModel, catalog_provider_canonical};
use mars_schema::{HarnessId, Host};

use super::infer_provider_from_model_id;
use crate::routing::slug;

/// The catalog row of the model `harness` runs, from the unfiltered snapshot. `model` is the
/// routed canonical id; `harness_model` is what the harness is launched with.
pub fn catalog_row<'a>(
    catalog: &'a [CachedModel],
    harness: HarnessId,
    model: &str,
    harness_model: &str,
) -> Option<&'a CachedModel> {
    let (provider, id) = match harness {
        HarnessId::Claude => ("anthropic", model),
        HarnessId::Codex => ("openai", model),
        HarnessId::Cursor => (infer_provider_from_model_id(model)?, model),
        HarnessId::OpenCode | HarnessId::Pi => {
            let parts = slug::parse(harness_model)?;
            (parts.provider, parts.model_id)
        }
    };
    let id = slug::normalize_model_id(id);
    let provider = catalog_provider_canonical(provider);
    catalog.iter().find(|row| {
        slug::normalize_model_id(&row.id) == id
            && (catalog_provider_canonical(&row.provider_key) == provider
                || slug::providers_match(&row.provider_key, &provider))
    })
}

/// An effort resolved for one route, with the warning its move deserves.
pub struct RouteEffort<'a> {
    pub row: Option<&'a CachedModel>,
    pub resolution: EffortResolution,
    pub warning: Option<String>,
}

/// Resolves `request` for `model` on `harness`. `harness_set` is the harness's own levels:
/// [`harness_levels`], or for Cursor the levels its probe lists for this model.
pub fn resolve_for_route<'a>(
    request: EffortRequest,
    catalog: &'a [CachedModel],
    harness: HarnessId,
    model: &str,
    harness_model: &str,
    harness_set: &Levels,
) -> RouteEffort<'a> {
    let row = catalog_row(catalog, harness, model, harness_model);
    let reasoning = ModelReasoning::from_row(row.and_then(|row| row.reasoning.as_ref()));
    let resolution = resolve_effort(request, &reasoning, harness_set);
    let warning = resolution
        .moved
        .map(|moved| move_warning(&moved, model, harness, &reasoning, harness_set));
    RouteEffort {
        row,
        resolution,
        warning,
    }
}

/// The levels `harness` itself takes, before any model: Cursor's depend on the model.
pub fn harness_set(harness: HarnessId) -> Levels {
    harness_levels(Host::Harness(harness))
}

fn move_warning(
    moved: &Move,
    model: &str,
    harness: HarnessId,
    reasoning: &ModelReasoning,
    harness_set: &Levels,
) -> String {
    let from = moved.from.as_str();
    let Some(to) = moved.to else {
        return match moved.why {
            MoveReason::ModelDoesNotReason => {
                format!("effort `{from}` not sent: `{model}` does not reason")
            }
            _ => format!(
                "effort `{from}` not sent: `{model}` takes {} and {harness} takes {}, which share no level",
                reasoning.levels.describe(),
                harness_set.describe()
            ),
        };
    };
    let subject = if moved.why == MoveReason::ModelLevels {
        format!("`{model}`")
    } else {
        harness.to_string()
    };
    format!(
        "effort `{from}` → `{}`: {subject} takes {}",
        to.as_str(),
        allowed_levels(reasoning, harness_set).describe()
    )
}

/// A model's reasoning as `mars models list` and `catalog` show it.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct ReasoningListing {
    /// `false` for a model that does not reason.
    pub reasons: bool,
    /// The levels the harness can send for this model; `null` when its row lists no effort values
    /// (toggle only, budget only, or reasoning with no control).
    pub levels: Option<Vec<&'static str>>,
    pub can_disable: bool,
    /// The smallest `budget_tokens` the model takes, when it takes one.
    pub budget_tokens: Option<u64>,
}

/// `None` when the model has no row: its reasoning is unknown.
pub fn listing(row: Option<&CachedModel>, harness_set: &Levels) -> Option<ReasoningListing> {
    let row = row?;
    let reasoning = ModelReasoning::from_row(row.reasoning.as_ref());
    let levels = match allowed_levels(&reasoning, harness_set) {
        Levels::Any => None,
        Levels::Only(set) => Some(set.iter().map(|level| level.as_str()).collect()),
    };
    Some(ReasoningListing {
        reasons: reasoning.reasons?,
        levels,
        can_disable: reasoning.can_disable,
        budget_tokens: reasoning.budget_tokens,
    })
}

impl ReasoningListing {
    /// The `EFFORT` column: the levels, `on/off`, `budget`, or `—` for a model that does not
    /// reason.
    pub fn text(&self) -> String {
        if !self.reasons {
            "—".to_string()
        } else if let Some(levels) = &self.levels {
            levels.join(",")
        } else if self.can_disable {
            "on/off".to_string()
        } else if self.budget_tokens.is_some() {
            "budget".to_string()
        } else {
            String::new()
        }
    }
}

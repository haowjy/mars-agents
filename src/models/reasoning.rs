//! Which models.dev row a harness's model is, and what effort that row lets the harness send.
//!
//! The row must be the provider the harness calls: `openai/o3` takes low, medium and high, while
//! `openrouter/openai/o3` is toggle only. Each harness has one join, here, and every reader of
//! a model's reasoning (launch, sync, the listing) goes through it.

use mars_schema::agent::{EffortLevel, EffortRequest};
use mars_schema::effort::{
    EffortResolution, Levels, ModelReasoning, Move, MoveReason, harness_levels, resolve_effort,
};
use mars_schema::models::{CachedModel, catalog_provider_canonical};
use mars_schema::{HarnessId, Host};

use super::catalog::Catalog;
use super::infer_provider_from_model_id;
use super::probes::Listing;
use super::probes::cursor::{
    CursorEffortFailure, CursorTiers, cursor_tiers_from_probe, without_tier,
};
use crate::routing::slug;

/// The catalog model behind a launch id.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CatalogIdentity {
    /// models.dev's provider key, as [`catalog_provider_canonical`] spells it.
    pub provider: String,
    /// The base model, in [`slug::normalize_model_id`] form: a provider qualification and a
    /// Cursor effort tier are not part of it.
    pub model: String,
}

/// What the launch id of `model` says about its catalog model. `model` is the routed canonical
/// id and `harness_model` is what the harness is launched with.
///
/// - Claude and Codex run one provider; a pinned alias may spell it (`openai/o3`).
/// - OpenCode and Pi launch `provider/model` slugs.
/// - Cursor folds an effort tier into its slugs (`gpt-5.5-high`); the tier is dropped when the
///   catalog knows the model without it.
pub fn catalog_identity(
    catalog: &Catalog,
    harness: HarnessId,
    model: &str,
    harness_model: &str,
) -> Option<CatalogIdentity> {
    let native = |provider: &'static str| {
        let id = slug::parse(model)
            .filter(|parts| slug::providers_match(parts.provider, provider))
            .map_or(model, |parts| parts.model_id);
        (provider, id)
    };
    let (provider, id) = match harness {
        HarnessId::Claude => native("anthropic"),
        HarnessId::Codex => native("openai"),
        HarnessId::Cursor => (infer_provider_from_model_id(model)?, model),
        HarnessId::OpenCode | HarnessId::Pi => {
            let parts = slug::parse(harness_model)?;
            (parts.provider, parts.model_id)
        }
    };
    let provider = catalog_provider_canonical(&slug::normalize_provider(provider));
    let mut model = slug::normalize_model_id(id);
    if harness == HarnessId::Cursor
        && !has_row(catalog, &provider, &model)
        && let Some(base) = without_tier(&model).map(str::to_string)
        && has_row(catalog, &provider, &base)
    {
        model = base;
    }
    Some(CatalogIdentity { provider, model })
}

fn has_row(catalog: &Catalog, provider: &str, model: &str) -> bool {
    catalog
        .provider_rows(provider)
        .any(|row| slug::normalize_model_id(&row.id) == model)
}

/// The catalog row of `identity`, from the unfiltered snapshot.
pub fn catalog_row<'a>(
    catalog: &'a Catalog,
    identity: &CatalogIdentity,
) -> Option<&'a CachedModel> {
    catalog
        .provider_rows(&identity.provider)
        .find(|row| slug::normalize_model_id(&row.id) == identity.model)
}

/// A move's warning. `not_sent` tells a request that went unsent from one that moved to another
/// level, which the sync summary counts apart.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EffortWarning {
    pub message: String,
    pub not_sent: bool,
}

/// An effort resolved for one route, with the warning its move deserves.
pub struct RouteEffort<'a> {
    pub row: Option<&'a CachedModel>,
    pub resolution: EffortResolution,
    pub warning: Option<EffortWarning>,
    /// The model was treated as unknown because the catalog has no reasoning data for it: there
    /// is no snapshot, or its row is from before 0.35.
    pub reasoning_data_missing: bool,
    /// Cursor's tiers for the model, when the effort needed them.
    pub cursor_tiers: Option<CursorTiers>,
    /// Cursor could not say which slug carries the requested effort.
    pub cursor_failure: Option<CursorEffortFailure>,
}

impl RouteEffort<'_> {
    /// The Cursor slug that carries the resolved level.
    pub fn cursor_slug(&self) -> Option<&str> {
        let level = self.resolution.level?;
        self.cursor_tiers
            .as_ref()?
            .slug_by_level
            .get(&level)
            .map(String::as_str)
    }
}

/// Resolves `request` for `model` on `harness`. Cursor's levels are the slugs `cursor_probe`
/// lists for the model, and are read only when the model could take an effort.
pub fn resolve_for_route<'a>(
    request: EffortRequest,
    catalog: &'a Catalog,
    harness: HarnessId,
    model: &str,
    harness_model: &str,
    cursor_probe: Option<&Listing>,
) -> RouteEffort<'a> {
    let identity = catalog_identity(catalog, harness, model, harness_model);
    let row = identity
        .as_ref()
        .and_then(|identity| catalog_row(catalog, identity));
    let reasoning = ModelReasoning::from_row(row.and_then(|row| row.reasoning.as_ref()));
    let mut cursor_tiers = None;
    let mut cursor_failure = None;
    let mut harness_set = self::harness_set(harness);
    if harness == HarnessId::Cursor
        && matches!(request, EffortRequest::Level(_))
        && reasoning != ModelReasoning::DoesNotReason
    {
        let base = identity.as_ref().map_or(model, |identity| &identity.model);
        match cursor_tiers_from_probe(base, cursor_probe) {
            Ok(tiers) => {
                harness_set = tiers.levels();
                cursor_tiers = Some(tiers);
            }
            Err(failure) => cursor_failure = Some(failure),
        }
    }
    let resolution = resolve_effort(request, &reasoning, &harness_set);
    let warning = resolution.moved.map(|moved| EffortWarning {
        message: move_warning(&moved, model, harness, &reasoning, &harness_set),
        not_sent: moved.to.is_none(),
    });
    RouteEffort {
        row,
        resolution,
        warning,
        reasoning_data_missing: reasoning == ModelReasoning::Unknown
            && (row.is_some() || catalog.is_empty()),
        cursor_tiers,
        cursor_failure,
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
    let model_levels = reasoning.levels();
    let Some(to) = moved.to else {
        return match moved.why {
            MoveReason::ModelDoesNotReason => {
                format!("effort `{from}` not sent: `{model}` does not reason")
            }
            _ => format!(
                "effort `{from}` not sent: `{model}` takes {} and {harness} takes {}, which share no level",
                model_levels.describe(),
                harness_set.describe()
            ),
        };
    };
    // Asking for `none` of a model that cannot switch reasoning off: its ladder is no more
    // use to the reader than that.
    if moved.from == EffortLevel::None && !model_levels.contains(EffortLevel::None) {
        return format!(
            "effort `none` → `{}`: `{model}` can't switch reasoning off",
            to.as_str()
        );
    }
    // Name each side that limits the request: it lacks the level, or it is narrower than the
    // other side and so shaped where the level landed.
    let lacks = |levels: &Levels| !levels.contains(moved.from);
    let mut limits = Vec::new();
    if model_levels == *harness_set {
        limits.push(format!(
            "`{model}` and {harness} take {}",
            model_levels.describe()
        ));
    } else {
        if lacks(&model_levels) || !model_levels.covers(harness_set) {
            limits.push(format!("`{model}` takes {}", model_levels.describe()));
        }
        if lacks(harness_set) || !harness_set.covers(&model_levels) {
            limits.push(format!("{harness} takes {}", harness_set.describe()));
        }
    }
    format!(
        "effort `{from}` → `{}`: {}",
        to.as_str(),
        limits.join(" and ")
    )
}

/// A model's reasoning as `mars models list` and `catalog` show it.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct ReasoningListing {
    /// `false` for a model that does not reason.
    pub reasons: bool,
    /// The levels its row names that the harness can send; `null` when the model does not
    /// reason or its row names none (toggle only, budget only, or reasoning with no control).
    pub levels: Option<Vec<&'static str>>,
    pub can_disable: bool,
    /// The smallest `budget_tokens` the model takes, when it takes one.
    pub budget_tokens: Option<u64>,
}

/// `None` when the model has no row or its row has no reasoning data: it is unknown.
pub fn listing(row: Option<&CachedModel>, harness_set: &Levels) -> Option<ReasoningListing> {
    match ModelReasoning::from_row(row?.reasoning.as_ref()) {
        ModelReasoning::Unknown => None,
        ModelReasoning::DoesNotReason => Some(ReasoningListing {
            reasons: false,
            levels: None,
            can_disable: false,
            budget_tokens: None,
        }),
        ModelReasoning::Reasons {
            levels,
            can_disable,
            budget_tokens,
        } => Some(ReasoningListing {
            reasons: true,
            levels: levels.map(|named| {
                named
                    .into_iter()
                    .filter(|level| harness_set.contains(*level))
                    .map(|level| level.as_str())
                    .collect()
            }),
            can_disable,
            budget_tokens,
        }),
    }
}

impl ReasoningListing {
    /// The `EFFORT` column: the levels, `on/off`, `budget`, or `—` for a model that does not
    /// reason. A model that reasons with no control shows nothing.
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

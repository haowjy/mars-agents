//! Resolving a requested effort to a level the chosen model and harness both take.
//!
//! Two sets always meet: what the model's models.dev row lists, and what the harness accepts.
//! The ladder is [`EffortLevel`]; a request that isn't on both sets moves to the nearest level
//! that is, and the move is reported so the caller can warn.

use std::collections::BTreeSet;

use serde::Serialize;

use crate::agent::{EffortLevel, EffortRequest};
use crate::harness::{HarnessId, Host};
use crate::models::{ModelReasoningRow, ReasoningOption};

/// A set of levels. `Any` is a value, never an empty set: it is "no constraint".
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Levels {
    Any,
    Only(BTreeSet<EffortLevel>),
}

impl Levels {
    pub fn only(levels: impl IntoIterator<Item = EffortLevel>) -> Self {
        Self::Only(levels.into_iter().collect())
    }

    pub fn contains(&self, level: EffortLevel) -> bool {
        match self {
            Self::Any => true,
            Self::Only(set) => set.contains(&level),
        }
    }

    /// Every level of `other` is in this set.
    pub fn covers(&self, other: &Levels) -> bool {
        match (self, other) {
            (Self::Any, _) => true,
            (Self::Only(_), Self::Any) => false,
            (Self::Only(a), Self::Only(b)) => b.is_subset(a),
        }
    }

    pub fn intersect(&self, other: &Levels) -> Levels {
        match (self, other) {
            (Self::Any, other) | (other, Self::Any) => other.clone(),
            (Self::Only(a), Self::Only(b)) => Self::Only(a.intersection(b).copied().collect()),
        }
    }

    /// The levels as an error message lists them, lowest first; `any level` for `Any`.
    pub fn describe(&self) -> String {
        match self {
            Self::Any => "any level".to_string(),
            Self::Only(set) if set.is_empty() => "no level".to_string(),
            Self::Only(set) => set
                .iter()
                .map(|level| level.as_str())
                .collect::<Vec<_>>()
                .join(", "),
        }
    }
}

/// What a model's models.dev row says about reasoning.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ModelReasoning {
    /// No row, or a row from before models.dev reasoning data was snapshotted.
    Unknown,
    /// The model does not reason, so no effort applies.
    DoesNotReason,
    Reasons {
        /// The levels the row names, `none` among them when the model has a toggle. `None` when
        /// it names none: toggle only, budget only, or reasoning with no control.
        levels: Option<BTreeSet<EffortLevel>>,
        /// The model can switch reasoning off: it has a toggle, or `none` among its values.
        can_disable: bool,
        /// The smallest `budget_tokens` option, when the model takes one.
        budget_tokens: Option<u64>,
    },
}

impl ModelReasoning {
    pub fn from_row(row: Option<&ModelReasoningRow>) -> Self {
        let Some(row) = row else {
            return Self::Unknown;
        };
        if !row.reasoning {
            return Self::DoesNotReason;
        }
        let mut efforts = BTreeSet::new();
        let mut toggle = false;
        let mut budget_tokens = None;
        for option in &row.options {
            match option {
                ReasoningOption::Effort { values } => {
                    efforts.extend(values.iter().filter_map(|value| EffortLevel::parse(value)));
                }
                ReasoningOption::Toggle => toggle = true,
                ReasoningOption::BudgetTokens { min } => budget_tokens = Some(min.unwrap_or(0)),
                ReasoningOption::Other => {}
            }
        }
        let can_disable = toggle || efforts.contains(&EffortLevel::None);
        let levels = (!efforts.is_empty()).then(|| {
            if toggle {
                efforts.insert(EffortLevel::None);
            }
            efforts
        });
        Self::Reasons {
            levels,
            can_disable,
            budget_tokens,
        }
    }

    /// The levels a request may be sent as. A model that reasons without naming levels takes
    /// the whole ladder, less `none` unless it can be switched off.
    pub fn levels(&self) -> Levels {
        match self {
            Self::Unknown => Levels::Any,
            Self::DoesNotReason => Levels::Only(BTreeSet::new()),
            Self::Reasons {
                levels: Some(levels),
                ..
            } => Levels::Only(levels.clone()),
            Self::Reasons {
                levels: None,
                can_disable,
                ..
            } => Levels::only(
                EffortLevel::LADDER
                    .into_iter()
                    .filter(|level| *can_disable || *level != EffortLevel::None),
            ),
        }
    }
}

/// Why a requested level moved. `code` is stable for consumers to match on.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum MoveReason {
    /// The model does not reason, so no effort is sent.
    ModelDoesNotReason,
    /// The model's levels lack the request.
    ModelLevels,
    /// The harness's levels lack the request.
    HarnessLevels,
    /// The model and the harness share no level, so no effort is sent.
    NoCommonLevel,
}

impl MoveReason {
    pub const fn code(self) -> &'static str {
        match self {
            Self::ModelDoesNotReason => "model_does_not_reason",
            Self::ModelLevels => "model_levels",
            Self::HarnessLevels => "harness_levels",
            Self::NoCommonLevel => "no_common_level",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct Move {
    pub from: EffortLevel,
    /// `None` when no effort is sent.
    pub to: Option<EffortLevel>,
    pub why: MoveReason,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct EffortResolution {
    /// The level to send; `None` sends no effort.
    pub level: Option<EffortLevel>,
    pub moved: Option<Move>,
}

/// The levels a harness takes, as of the versions recorded here. The harness's own default is
/// always reachable by sending no effort, so no set lists it.
///
/// | Host | Levels | Checked against |
/// |---|---|---|
/// | Claude | `low`..`max`, no off | Claude Code 2.1.325 `--effort` help: low, medium, high, xhigh, max |
/// | Codex | every level | Codex 0.167.0 `exec -c model_reasoning_effort=…` sends any value unchecked; the API rejected `minimal` on gpt-5.6-luna and listed none, low, medium, high, xhigh, max. models.dev decides per model: Codex's own presets (`codex debug models`) list low..max, plus `ultra` on six, and never `none`, and models.dev lacks `ultra`, `gpt-reserve` and `codex-auto-review` |
/// | Pi | every level (`none` as `off`) | Pi 1.5.0 `--thinking` help: off, minimal, low, medium, high, xhigh, max |
/// | OpenCode | `Any` | OpenCode 2.3.0 decodes models.dev `reasoning_options` itself (the string is in its binary) |
/// | Cursor | `Any` here; the caller passes the levels its probe slugs list | Cursor probe listing |
/// | Meridian Flow | `none` (as `off`), `low`, `medium`, `high`, `max` | meridian-runtime `src/gateway/domain/types.ts:63` (`REASONING_EFFORTS`), per Flow's gateway lead |
pub fn harness_levels(host: Host) -> Levels {
    use EffortLevel::*;
    match host {
        Host::Harness(HarnessId::Claude) => Levels::only([Low, Medium, High, XHigh, Max]),
        Host::Harness(HarnessId::Codex | HarnessId::Pi) => Levels::only(EffortLevel::LADDER),
        Host::Harness(HarnessId::OpenCode | HarnessId::Cursor) => Levels::Any,
        Host::MeridianFlow => Levels::only([None, Low, Medium, High, Max]),
    }
}

/// Resolves `requested` for a model on a harness.
///
/// A request in both sets stays. Otherwise it moves to the nearest level below it, or the
/// nearest above when none is below; a move never lands on `none` unless `none` was asked,
/// because off is not less thinking. `Default` sends nothing. A model that does not reason
/// gets nothing whatever was asked. An empty intersection sends nothing.
pub fn resolve_effort(
    requested: EffortRequest,
    model: &ModelReasoning,
    harness: &Levels,
) -> EffortResolution {
    let EffortRequest::Level(from) = requested else {
        return EffortResolution {
            level: None,
            moved: None,
        };
    };
    if *model == ModelReasoning::DoesNotReason {
        return EffortResolution {
            level: None,
            moved: (from != EffortLevel::None).then_some(Move {
                from,
                to: None,
                why: MoveReason::ModelDoesNotReason,
            }),
        };
    }
    let model_levels = model.levels();
    let allowed = model_levels.intersect(harness);
    if allowed.contains(from) {
        return EffortResolution {
            level: Some(from),
            moved: None,
        };
    }
    let why = if model_levels.contains(from) {
        MoveReason::HarnessLevels
    } else {
        MoveReason::ModelLevels
    };
    let to = match &allowed {
        // `Any` contains every level, so it returned above.
        Levels::Any => None,
        Levels::Only(set) => {
            let usable =
                |level: &&EffortLevel| **level != EffortLevel::None || from == EffortLevel::None;
            set.range(..from)
                .rev()
                .find(usable)
                .or_else(|| set.range(from..).find(usable))
                .copied()
        }
    };
    EffortResolution {
        level: to,
        moved: Some(Move {
            from,
            to,
            why: if to.is_some() {
                why
            } else {
                MoveReason::NoCommonLevel
            },
        }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use EffortLevel::{High, Low, Max, Medium, Minimal, XHigh};
    const OFF: EffortLevel = EffortLevel::None;

    fn row(json: &str) -> ModelReasoningRow {
        serde_json::from_str(json).unwrap()
    }

    fn resolve(
        request: EffortRequest,
        row: Option<&ModelReasoningRow>,
        host: Host,
    ) -> EffortResolution {
        resolve_effort(
            request,
            &ModelReasoning::from_row(row),
            &harness_levels(host),
        )
    }

    const CLAUDE: Host = Host::Harness(HarnessId::Claude);
    const CODEX: Host = Host::Harness(HarnessId::Codex);
    const PI: Host = Host::Harness(HarnessId::Pi);
    const OPENCODE: Host = Host::Harness(HarnessId::OpenCode);

    // Real models.dev rows: openai/o3, openai/gpt-5.5, anthropic/claude-sonnet-4-5 (budget only),
    // a toggle-only model, and a model that does not reason.
    fn o3() -> ModelReasoningRow {
        row(r#"{"reasoning":true,"options":[{"type":"effort","values":["low","medium","high"]}]}"#)
    }
    fn gpt55() -> ModelReasoningRow {
        row(
            r#"{"reasoning":true,"options":[{"type":"effort","values":["none","low","medium","high","xhigh"]}]}"#,
        )
    }
    fn budget_only() -> ModelReasoningRow {
        row(r#"{"reasoning":true,"options":[{"type":"budget_tokens","min":1024}]}"#)
    }
    fn toggle_only() -> ModelReasoningRow {
        row(r#"{"reasoning":true,"options":[{"type":"toggle"}]}"#)
    }
    fn no_reasoning() -> ModelReasoningRow {
        row(r#"{"reasoning":false}"#)
    }

    fn level(l: EffortLevel) -> EffortRequest {
        EffortRequest::Level(l)
    }

    #[test]
    fn default_sends_nothing_and_never_warns() {
        let r = resolve(EffortRequest::Default, Some(&o3()), CLAUDE);
        assert_eq!((r.level, r.moved), (None, None));
    }

    #[test]
    fn level_above_the_model_range_moves_down() {
        let r = resolve(level(Max), Some(&o3()), CODEX);
        assert_eq!(r.level, Some(High));
        assert_eq!(r.moved.unwrap().why, MoveReason::ModelLevels);
    }

    #[test]
    fn level_below_the_model_range_moves_up() {
        let r = resolve(level(Minimal), Some(&o3()), CODEX);
        assert_eq!(r.level, Some(Low));
    }

    #[test]
    fn a_supported_level_stays() {
        let r = resolve(level(Medium), Some(&o3()), CLAUDE);
        assert_eq!((r.level, r.moved), (Some(Medium), None));
    }

    #[test]
    fn none_on_a_harness_with_no_off_goes_to_the_lowest_level() {
        let r = resolve(level(OFF), Some(&gpt55()), CLAUDE);
        assert_eq!(r.level, Some(Low));
        assert_eq!(r.moved.unwrap().why, MoveReason::HarnessLevels);
    }

    #[test]
    fn none_stays_where_the_model_and_harness_can_turn_reasoning_off() {
        let r = resolve(level(OFF), Some(&gpt55()), PI);
        assert_eq!((r.level, r.moved), (Some(OFF), None));
    }

    #[test]
    fn a_move_never_lands_on_none() {
        // gpt-5.5 takes `none`, but Pi's minimal is not on its list: low, not off.
        let r = resolve(level(Minimal), Some(&gpt55()), PI);
        assert_eq!(r.level, Some(Low));
        let flow = resolve(level(Minimal), Some(&gpt55()), Host::MeridianFlow);
        assert_eq!(flow.level, Some(Low));
    }

    #[test]
    fn a_toggle_adds_none_to_the_model() {
        let row = row(
            r#"{"reasoning":true,"options":[{"type":"toggle"},{"type":"effort","values":["low","high"]}]}"#,
        );
        let model = ModelReasoning::from_row(Some(&row));
        assert_eq!(model.levels(), Levels::only([OFF, Low, High]));
    }

    #[test]
    fn effort_values_off_the_ladder_are_dropped() {
        let row =
            row(r#"{"reasoning":true,"options":[{"type":"effort","values":["default","low"]}]}"#);
        assert_eq!(
            ModelReasoning::from_row(Some(&row)).levels(),
            Levels::only([Low])
        );
    }

    #[test]
    fn ties_go_down() {
        // Flow has no xhigh: high and max are both one step away.
        let r = resolve(level(XHigh), Some(&gpt55_with_max()), Host::MeridianFlow);
        assert_eq!(r.level, Some(High));
    }

    fn gpt55_with_max() -> ModelReasoningRow {
        row(
            r#"{"reasoning":true,"options":[{"type":"effort","values":["low","medium","high","xhigh","max"]}]}"#,
        )
    }

    #[test]
    fn budget_only_toggle_only_and_unknown_models_take_any_level() {
        for row in [Some(budget_only()), Some(toggle_only()), None] {
            let r = resolve(level(XHigh), row.as_ref(), PI);
            assert_eq!((r.level, r.moved), (Some(XHigh), None));
        }
    }

    #[test]
    fn an_unknown_model_is_still_clamped_to_the_harness() {
        let r = resolve(level(Minimal), None, CLAUDE);
        assert_eq!(r.level, Some(Low));
        assert_eq!(r.moved.unwrap().why, MoveReason::HarnessLevels);
    }

    #[test]
    fn opencode_takes_any_level_from_the_model() {
        let r = resolve(level(Max), Some(&gpt55()), OPENCODE);
        assert_eq!(r.level, Some(XHigh));
    }

    #[test]
    fn a_model_that_does_not_reason_gets_nothing() {
        let r = resolve(level(High), Some(&no_reasoning()), CODEX);
        assert_eq!(r.level, None);
        assert_eq!(r.moved.unwrap().why, MoveReason::ModelDoesNotReason);
        let off = resolve(level(OFF), Some(&no_reasoning()), CODEX);
        assert_eq!((off.level, off.moved), (None, None));
    }

    #[test]
    fn no_common_level_sends_nothing() {
        let only_none =
            row(r#"{"reasoning":true,"options":[{"type":"effort","values":["none"]}]}"#);
        let r = resolve(level(High), Some(&only_none), CLAUDE);
        assert_eq!(r.level, None);
        assert_eq!(r.moved.unwrap().why, MoveReason::NoCommonLevel);
    }
}

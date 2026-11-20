//! The compiler's one row per harness: how its agent file is written, how its skill frontmatter
//! spells the invocation switches, and whether its agents need a `description`. Registry rows
//! (`src/harness`) hold what the harness is; this row holds how Mars lowers to it, so
//! `src/harness` never depends on the compiler. Each row is a `match` arm, so a new harness
//! fails to compile until it has one.

use mars_schema::HarnessId;

use crate::compiler::agents::lower::{
    AGY_AGENT_POLICY, AgentLoweringPolicy, CLAUDE_AGENT_POLICY, OPENCODE_AGENT_POLICY,
    PI_AGENT_POLICY,
};

pub(crate) struct Lowering {
    pub agent: AgentFormat,
    pub skill: SkillKeys,
    /// Whether the harness does not load an agent that has no `description`.
    pub needs_description: bool,
}

#[derive(Clone, Copy)]
pub(crate) enum AgentFormat {
    /// A markdown file with YAML frontmatter, written by the shared pipeline under this policy.
    Markdown(&'static AgentLoweringPolicy),
    /// Codex's TOML file, lowered by its own function.
    CodexToml,
}

impl AgentFormat {
    pub fn extension(self) -> &'static str {
        match self {
            Self::Markdown(_) => "md",
            Self::CodexToml => "toml",
        }
    }
}

/// How a harness's skill frontmatter expresses the two invocation switches.
#[derive(Clone, Copy)]
pub(crate) struct SkillKeys {
    pub model_off: ModelOff,
    pub user_off: UserOff,
}

/// What a skill with `model-invocable: false` becomes.
#[derive(Clone, Copy)]
pub(crate) enum ModelOff {
    /// `disable-model-invocation: true`.
    Key,
    /// An `openai.yaml` file beside the skill.
    SiblingFile,
    /// No switch: the explicit `false` is reported lost.
    Lost,
}

/// What a skill with `user-invocable: false` becomes.
#[derive(Clone, Copy)]
pub(crate) enum UserOff {
    /// This key set to this value.
    Key(&'static str, bool),
    /// No switch: the `false` is reported lost.
    Lost,
}

pub(crate) fn lowering(id: HarnessId) -> &'static Lowering {
    match id {
        HarnessId::Claude => &CLAUDE,
        HarnessId::Codex => &CODEX,
        HarnessId::Pi => &PI,
        HarnessId::OpenCode => &OPENCODE,
        HarnessId::Agy => &AGY,
    }
}

const CLAUDE: Lowering = Lowering {
    agent: AgentFormat::Markdown(&CLAUDE_AGENT_POLICY),
    skill: SkillKeys {
        model_off: ModelOff::Key,
        user_off: UserOff::Key("user-invocable", false),
    },
    needs_description: true,
};

const CODEX: Lowering = Lowering {
    agent: AgentFormat::CodexToml,
    skill: SkillKeys {
        model_off: ModelOff::SiblingFile,
        user_off: UserOff::Lost,
    },
    needs_description: false,
};

const PI: Lowering = Lowering {
    agent: AgentFormat::Markdown(&PI_AGENT_POLICY),
    skill: SkillKeys {
        model_off: ModelOff::Key,
        user_off: UserOff::Lost,
    },
    needs_description: false,
};

const OPENCODE: Lowering = Lowering {
    agent: AgentFormat::Markdown(&OPENCODE_AGENT_POLICY),
    skill: SkillKeys {
        model_off: ModelOff::Lost,
        user_off: UserOff::Lost,
    },
    needs_description: false,
};

// `agy` hides the skill from the slash-command list and the model still uses it, which is what
// `user-invocable: false` means.
const AGY: Lowering = Lowering {
    agent: AgentFormat::Markdown(&AGY_AGENT_POLICY),
    skill: SkillKeys {
        model_off: ModelOff::Lost,
        user_off: UserOff::Key("disable-slash-command", true),
    },
    needs_description: false,
};

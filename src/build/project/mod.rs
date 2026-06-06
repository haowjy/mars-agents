//! EXPERIMENTAL per-harness `launch_actions` projection (`mars build launch-bundle --context`).
//!
//! Not consumed by meridian-cli (invariant I-2). Meridian owns launch argv/env in its harness
//! adapters. This module is slated for possible deletion — see work item `launch-bundle-projection` / PR #94.
//!
//! Parity TODOs in harness projectors document gaps vs meridian adapters for anyone reviving Phase 2.
//! A second class of gaps (MERIDIAN_* env protocol, session preflight, permission→flag model) is
//! intentionally meridian-owned and will never be mars's responsibility.
//!
//! To delete the experimental launch_actions projection:
//! - src/build/project/*            (this module: per-harness projectors + project_launch_actions)
//! - src/build/bundle.rs            (launch_actions field; LaunchActions/LaunchFile/LaunchProtocol/RuntimeContext/StreamingContext)
//! - src/build/mod.rs               (runtime_context/transport on request; the `if let Some(context)` gate)
//! - src/cli/build.rs               (--context/--transport args + RuntimeContext parse + the warning)
//!
//! launch_actions is Option (serde-skipped when None), so removal needs no version bump.

use std::collections::BTreeMap;

use serde_json::json;

use crate::build::bundle::{LaunchActions, LaunchBundle, LaunchFile, RuntimeContext};
use crate::error::MarsError;
use crate::harness::registry::{HarnessId, parse as parse_harness};

mod claude;
mod codex;
mod cursor;
mod opencode;
mod pi;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Transport {
    Subprocess,
    Streaming,
}

impl Transport {
    pub fn parse(value: &str) -> Result<Self, MarsError> {
        match value.trim().to_ascii_lowercase().as_str() {
            "subprocess" => Ok(Self::Subprocess),
            "streaming" => Ok(Self::Streaming),
            other => Err(MarsError::InvalidRequest {
                message: format!(
                    "unsupported launch-bundle transport `{other}`; expected subprocess or streaming"
                ),
            }),
        }
    }
}

pub fn project_launch_actions(
    bundle: &LaunchBundle,
    context: &RuntimeContext,
    transport: Transport,
) -> Result<LaunchActions, MarsError> {
    let harness = parse_harness(&bundle.routing.harness).ok_or_else(|| {
        MarsError::Internal(format!(
            "launch-bundle routing selected unknown harness `{}`",
            bundle.routing.harness
        ))
    })?;

    match (harness, transport) {
        (HarnessId::Cursor, Transport::Subprocess) => cursor::project(bundle, context),
        (HarnessId::Claude, Transport::Subprocess) => claude::project(bundle, context),
        (HarnessId::Codex, Transport::Subprocess) => codex::project_subprocess(bundle, context),
        (HarnessId::Codex, Transport::Streaming) => codex::project_streaming(bundle, context),
        (HarnessId::OpenCode, Transport::Subprocess) => {
            opencode::project_subprocess(bundle, context)
        }
        (HarnessId::OpenCode, Transport::Streaming) => opencode::project_streaming(bundle, context),
        (HarnessId::Pi, Transport::Subprocess) => pi::project(bundle, context),
        (HarnessId::Claude | HarnessId::Cursor | HarnessId::Pi, Transport::Streaming) => {
            Err(MarsError::InvalidRequest {
                message: format!(
                    "harness `{}` does not support launch_actions transport `streaming`",
                    bundle.routing.harness
                ),
            })
        }
    }
}

fn approval(bundle: &LaunchBundle) -> &str {
    bundle
        .execution_policy
        .approval
        .as_deref()
        .unwrap_or("default")
}

fn sandbox(bundle: &LaunchBundle) -> &str {
    bundle
        .execution_policy
        .sandbox
        .as_deref()
        .unwrap_or("default")
}

fn effort(bundle: &LaunchBundle) -> Option<&str> {
    bundle
        .execution_policy
        .effort
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
}

fn model(bundle: &LaunchBundle) -> Option<&str> {
    let value = bundle.routing.harness_model.trim();
    if value.is_empty() { None } else { Some(value) }
}

fn agent_name(bundle: &LaunchBundle) -> Option<&str> {
    bundle
        .agent
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
}

fn cwd(context: &RuntimeContext) -> Result<String, MarsError> {
    context
        .cwd
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(ToString::to_string)
        .ok_or_else(|| MarsError::InvalidRequest {
            message: "launch-bundle --context must include cwd for launch_actions projection"
                .to_string(),
        })
}

fn subprocess_actions(
    context: &RuntimeContext,
    argv: Vec<String>,
    files: Vec<LaunchFile>,
    stdin: Option<String>,
) -> Result<LaunchActions, MarsError> {
    Ok(LaunchActions::Subprocess {
        argv,
        env: BTreeMap::new(),
        cwd: cwd(context)?,
        files,
        stdin,
    })
}

fn json_string(value: &str) -> String {
    serde_json::to_string(value).expect("string serialization cannot fail")
}

fn mcp_codex_flags(entries: &[String]) -> Result<Vec<String>, MarsError> {
    let mut flags = Vec::new();
    for raw in entries {
        let entry = raw.trim();
        if entry.is_empty() {
            continue;
        }
        let (name, command) = entry
            .split_once('=')
            .ok_or_else(|| MarsError::InvalidRequest {
                message: format!("Codex mcp_tools entries must be '<name>=<command>'; got {raw:?}"),
            })?;
        let name = name.trim();
        let command = command.trim();
        if name.is_empty() || command.is_empty() {
            return Err(MarsError::InvalidRequest {
                message: format!("Codex mcp_tools entries must be '<name>=<command>'; got {raw:?}"),
            });
        }
        flags.push("-c".to_string());
        flags.push(format!(
            "mcp.servers.{name}.command={}",
            json_string(command)
        ));
    }
    Ok(flags)
}

fn opencode_workspace_env(context: &RuntimeContext) -> Option<String> {
    if context.workspace_roots.is_empty() {
        return None;
    }

    let mut merged = match context
        .opencode_config_content
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .and_then(|value| serde_json::from_str::<serde_json::Value>(value).ok())
    {
        Some(serde_json::Value::Object(map)) => map,
        _ => serde_json::Map::new(),
    };

    let permission = merged
        .remove("permission")
        .and_then(|value| value.as_object().cloned())
        .unwrap_or_default();
    let mut permission = permission;

    let external_raw = permission.remove("external_directory");
    let mut external = match external_raw {
        Some(serde_json::Value::Object(map)) => map
            .into_iter()
            .map(|(key, value)| (key, value.as_str().unwrap_or("").to_string()))
            .collect::<BTreeMap<_, _>>(),
        Some(serde_json::Value::Array(values)) => values
            .into_iter()
            .map(|value| {
                (
                    value.as_str().unwrap_or("").to_string(),
                    "allow".to_string(),
                )
            })
            .collect::<BTreeMap<_, _>>(),
        _ => BTreeMap::new(),
    };
    external.retain(|path, _| !path.is_empty());

    for root in &context.workspace_roots {
        external.insert(format!("{root}/**"), "allow".to_string());
    }

    permission.insert("external_directory".to_string(), json!(external));
    merged.insert("permission".to_string(), json!(permission));
    Some(
        serde_json::to_string(&serde_json::Value::Object(merged))
            .expect("JSON serialization cannot fail"),
    )
}

fn prompt_file(context: &RuntimeContext, content: String) -> Result<LaunchFile, MarsError> {
    let temp_dir = context
        .temp_dir
        .as_deref()
        .ok_or_else(|| MarsError::InvalidRequest {
            message: "launch-bundle --context must include temp_dir for prompt file projection"
                .to_string(),
        })?;
    Ok(LaunchFile {
        path: format!("{}/prompt.md", temp_dir.trim_end_matches(['/', '\\'])),
        content,
    })
}

fn streaming_context(context: &RuntimeContext) -> Result<(&str, u16), MarsError> {
    context
        .streaming
        .as_ref()
        .map(|streaming| (streaming.host.as_str(), streaming.port))
        .ok_or_else(|| MarsError::InvalidRequest {
            message: "launch-bundle --transport streaming requires context.streaming".to_string(),
        })
}

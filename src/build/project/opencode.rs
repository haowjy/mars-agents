use std::collections::BTreeMap;

use serde_json::json;

use crate::build::bundle::{LaunchActions, LaunchBundle, RuntimeContext};
use crate::build::project::{
    effort, empty_actions, model, opencode_workspace_env, streaming_context,
};
use crate::error::MarsError;

pub fn project_subprocess(
    bundle: &LaunchBundle,
    context: &RuntimeContext,
) -> Result<LaunchActions, MarsError> {
    if bundle
        .tools
        .mcp
        .iter()
        .any(|entry| !entry.trim().is_empty())
    {
        return Err(MarsError::InvalidRequest {
            message: "OpenCode subprocess does not support per-spawn mcp_tools; use streaming transport (opencode serve) for MCP session payloads.".to_string(),
        });
    }

    let mut argv = vec!["opencode".to_string(), "run".to_string()];
    if let Some(model) = model(bundle) {
        argv.extend(["--model".to_string(), normalize_opencode_model(model)]);
    }
    if let Some(effort) = effort(bundle).filter(|_| !context.interactive) {
        argv.extend(["--variant".to_string(), effort.to_string()]);
    }

    argv.extend(context.extra_args.iter().cloned());

    if !context.interactive {
        argv.push("-".to_string());
    }

    if let Some(session_id) = context
        .session_id
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
    {
        argv.extend(["--session".to_string(), session_id.to_string()]);
        if context.fork {
            argv.push("--fork".to_string());
        }
    }

    let mut actions = empty_actions(argv);
    if let Some(config) = opencode_workspace_env(context) {
        actions
            .env
            .insert("OPENCODE_CONFIG_CONTENT".to_string(), config);
    }
    Ok(actions)
}

pub fn project_streaming(
    bundle: &LaunchBundle,
    context: &RuntimeContext,
) -> Result<LaunchActions, MarsError> {
    if context.fork {
        return Err(MarsError::InvalidRequest {
            message: "OpenCode streaming cannot express continue_fork semantics over the current /session API.".to_string(),
        });
    }
    let (host, port) = streaming_context(context)?;
    let mut argv = vec![
        "opencode".to_string(),
        "serve".to_string(),
        "--hostname".to_string(),
        host.to_string(),
        "--port".to_string(),
        port.to_string(),
    ];
    argv.extend(context.extra_args.iter().cloned());

    let mut env = BTreeMap::new();
    if let Some(config) = opencode_workspace_env(context) {
        env.insert("OPENCODE_CONFIG_CONTENT".to_string(), config);
    }

    let mut body = serde_json::Map::new();
    if let Some(model) = model(bundle) {
        let normalized = normalize_opencode_model(model);
        body.insert("model".to_string(), json!(normalized));
        body.insert("modelID".to_string(), json!(normalized));
    }
    if let Some(agent) = crate::build::project::agent_name(bundle) {
        body.insert("agent".to_string(), json!(agent));
    }
    let mcp = bundle
        .tools
        .mcp
        .iter()
        .map(|entry| entry.trim())
        .filter(|entry| !entry.is_empty())
        .collect::<Vec<_>>();
    if !mcp.is_empty() {
        body.insert("mcp".to_string(), json!({ "servers": mcp }));
    }

    Ok(LaunchActions {
        argv,
        env,
        files: Vec::new(),
        protocol_payload: Some(json!({
            "transport": "http",
            "method": "POST",
            "path": "/session",
            "body": body,
        })),
    })
}

fn normalize_opencode_model(model: &str) -> String {
    let normalized = model.trim();
    let Some((provider, model_name)) = normalized.split_once('/') else {
        return normalized.to_string();
    };
    let provider = provider.trim();
    let model_name = model_name.trim();
    if provider.is_empty() || model_name.is_empty() {
        normalized.to_string()
    } else {
        format!("{provider}/{model_name}")
    }
}

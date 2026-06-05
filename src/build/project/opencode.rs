use std::collections::BTreeMap;

use serde_json::json;

use crate::build::bundle::{
    LaunchActions, LaunchBundle, LaunchProtocol, ProtocolBootstrap, ProtocolTurn, RuntimeContext,
};
use crate::build::project::{
    cwd, effort, model, opencode_workspace_env, streaming_context, subprocess_actions,
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
        argv.extend(["--model".to_string(), model.to_string()]);
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

    let mut actions = subprocess_actions(context, argv, Vec::new(), context.prompt.clone())?;
    if let Some(config) = opencode_workspace_env(context)
        && let LaunchActions::Subprocess { env, .. } = &mut actions
    {
        env.insert("OPENCODE_CONFIG_CONTENT".to_string(), config);
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
        body.insert("model".to_string(), json!(model));
        body.insert("modelID".to_string(), json!(model));
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

    let mut turn_body = serde_json::Map::new();
    turn_body.insert(
        "parts".to_string(),
        json!([{ "type": "text", "text": context.prompt.as_deref().unwrap_or_default() }]),
    );
    let system_instruction = bundle.prompt_surface.system_instruction.trim();
    if !system_instruction.is_empty() {
        turn_body.insert("system".to_string(), json!(system_instruction));
    }

    Ok(LaunchActions::Streaming {
        argv,
        env,
        cwd: cwd(context)?,
        protocol: LaunchProtocol {
            transport: "http".to_string(),
            bootstrap: ProtocolBootstrap {
                method: "POST".to_string(),
                path: Some("/session".to_string()),
                params: None,
                body: Some(json!(body)),
            },
            turn: ProtocolTurn {
                method: "POST".to_string(),
                path_template: Some("/session/{session_id}/prompt_async".to_string()),
                params_template: None,
                body_template: Some(json!(turn_body)),
            },
        },
    })
}

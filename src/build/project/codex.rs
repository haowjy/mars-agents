use serde_json::json;

use crate::build::bundle::{LaunchActions, LaunchBundle, RuntimeContext};
use crate::build::project::{
    approval, effort, empty_actions, json_string, mcp_codex_flags, model, sandbox,
};
use crate::error::MarsError;

pub fn project_subprocess(
    bundle: &LaunchBundle,
    context: &RuntimeContext,
) -> Result<LaunchActions, MarsError> {
    let mut argv = vec![
        "codex".to_string(),
        "exec".to_string(),
        "--json".to_string(),
    ];

    let final_prompt = final_prompt(bundle, context);
    let session_id = normalized_session_id(context);
    let guarded_prompt = if context.interactive && session_id.is_none() && !final_prompt.is_empty()
    {
        format!("{final_prompt}\n\nDO NOT DO ANYTHING. WAIT FOR USER INPUT.")
    } else {
        final_prompt
    };

    if let Some(model) = model(bundle) {
        argv.extend(["--model".to_string(), model.to_string()]);
    }
    if let Some(effort) = effort(bundle) {
        argv.extend([
            "-c".to_string(),
            format!("model_reasoning_effort={}", json_string(effort)),
        ]);
    }

    if let Some(sandbox) = map_codex_sandbox_mode(sandbox(bundle))? {
        argv.extend(["--sandbox".to_string(), sandbox.to_string()]);
    }
    if let Some(approval) = map_codex_approval_policy(approval(bundle))? {
        argv.extend([
            "-c".to_string(),
            format!("approval_policy={}", json_string(approval)),
        ]);
    }

    argv.extend(mcp_codex_flags(&bundle.tools.mcp)?);

    if let Some(session_id) = session_id {
        argv.extend(["resume".to_string(), session_id]);
    }

    for root in &context.workspace_roots {
        argv.extend(["--add-dir".to_string(), root.clone()]);
    }

    argv.extend(context.extra_args.iter().cloned());

    if !context.interactive
        && let Some(report_path) = context
            .report_output_path
            .as_deref()
            .map(str::trim)
            .filter(|value| !value.is_empty())
    {
        argv.extend(["-o".to_string(), report_path.to_string()]);
    }

    if !guarded_prompt.is_empty() {
        argv.push(guarded_prompt);
    }

    Ok(empty_actions(argv))
}

pub fn project_streaming(
    bundle: &LaunchBundle,
    context: &RuntimeContext,
) -> Result<LaunchActions, MarsError> {
    let (host, port) = crate::build::project::streaming_context(context)?;
    let mut argv = vec![
        "codex".to_string(),
        "app-server".to_string(),
        "--listen".to_string(),
        format!("ws://{host}:{port}"),
    ];

    if let Some(sandbox) = map_codex_sandbox_mode(sandbox(bundle))? {
        argv.extend([
            "-c".to_string(),
            format!("sandbox_mode={}", json_string(sandbox)),
        ]);
    }
    if let Some(approval) = map_codex_approval_policy(approval(bundle))? {
        argv.extend([
            "-c".to_string(),
            format!("approval_policy={}", json_string(approval)),
        ]);
    }
    argv.extend(mcp_codex_flags(&bundle.tools.mcp)?);
    if !context.workspace_roots.is_empty() {
        argv.extend([
            "-c".to_string(),
            format!(
                "sandbox_workspace_write.writable_roots={}",
                serde_json::to_string(&context.workspace_roots)
                    .expect("JSON serialization cannot fail")
            ),
        ]);
    }
    argv.extend(context.extra_args.iter().cloned());

    let method = select_thread_method(context);
    let mut params = serde_json::Map::new();
    params.insert(
        "cwd".to_string(),
        json!(context.cwd.as_deref().unwrap_or_default()),
    );
    if let Some(base) = context
        .base_instructions
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
    {
        params.insert("baseInstructions".to_string(), json!(base));
    }
    let developer = context
        .developer_instructions
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .unwrap_or(bundle.prompt_surface.system_instruction.trim());
    if !developer.is_empty() {
        params.insert("developerInstructions".to_string(), json!(developer));
    }
    if let Some(model) = model(bundle) {
        params.insert("model".to_string(), json!(model));
    }
    if let Some(effort) = effort(bundle) {
        params.insert(
            "config".to_string(),
            json!({ "model_reasoning_effort": effort }),
        );
    }
    if let Some(approval) = map_codex_approval_policy(approval(bundle))? {
        params.insert("approvalPolicy".to_string(), json!(approval));
    }
    if let Some(sandbox) = map_codex_sandbox_mode(sandbox(bundle))? {
        params.insert("sandbox".to_string(), json!(sandbox));
    }
    if let Some(session_id) = normalized_session_id(context) {
        params.insert("threadId".to_string(), json!(session_id));
    }
    if method == "thread/fork" {
        params.insert("ephemeral".to_string(), json!(false));
    }

    let mut actions = empty_actions(argv);
    actions.protocol_payload = Some(json!({
        "transport": "jsonrpc",
        "method": method,
        "params": params,
    }));
    Ok(actions)
}

fn final_prompt(bundle: &LaunchBundle, context: &RuntimeContext) -> String {
    let base = context.base_instructions.as_deref().unwrap_or("");
    let developer = context
        .developer_instructions
        .as_deref()
        .unwrap_or(bundle.prompt_surface.system_instruction.as_str());
    let prompt = context
        .user_turn_content
        .as_deref()
        .or(context.prompt.as_deref())
        .unwrap_or("");
    [base, developer, prompt]
        .into_iter()
        .map(str::trim)
        .filter(|part| !part.is_empty())
        .collect::<Vec<_>>()
        .join("\n\n")
}

fn normalized_session_id(context: &RuntimeContext) -> Option<String> {
    context
        .session_id
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(ToString::to_string)
}

fn select_thread_method(context: &RuntimeContext) -> &'static str {
    if normalized_session_id(context).is_none() {
        "thread/start"
    } else if context.fork {
        "thread/fork"
    } else {
        "thread/resume"
    }
}

fn map_codex_approval_policy(mode: &str) -> Result<Option<&'static str>, MarsError> {
    match mode {
        "default" => Ok(None),
        "auto" => Ok(Some("on-request")),
        "confirm" => Ok(Some("untrusted")),
        "never" | "yolo" => Ok(Some("never")),
        other => Err(MarsError::InvalidRequest {
            message: format!(
                "Codex cannot express requested approval mode '{other}' on this CLI/protocol version"
            ),
        }),
    }
}

fn map_codex_sandbox_mode(mode: &str) -> Result<Option<&'static str>, MarsError> {
    match mode {
        "default" => Ok(None),
        "read-only" => Ok(Some("read-only")),
        "workspace-write" => Ok(Some("workspace-write")),
        "danger-full-access" => Ok(Some("danger-full-access")),
        other => Err(MarsError::InvalidRequest {
            message: format!(
                "Codex cannot express requested sandbox mode '{other}' on this CLI/protocol version"
            ),
        }),
    }
}

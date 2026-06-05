use crate::build::bundle::{LaunchActions, LaunchBundle, RuntimeContext};
use crate::build::project::{approval, cwd, model, sandbox, subprocess_actions};
use crate::error::MarsError;

pub fn project(
    bundle: &LaunchBundle,
    context: &RuntimeContext,
) -> Result<LaunchActions, MarsError> {
    if !bundle.tools.mcp.is_empty() {
        return Err(MarsError::InvalidRequest {
            message: "Cursor subprocess does not support per-spawn mcp_tools for MVP.".to_string(),
        });
    }
    if context.fork {
        return Err(MarsError::InvalidRequest {
            message: "Cursor subprocess continue_fork is not supported for MVP.".to_string(),
        });
    }
    if !context
        .session_id
        .as_deref()
        .unwrap_or("")
        .trim()
        .is_empty()
    {
        return Err(MarsError::InvalidRequest {
            message: "Cursor subprocess session resume is not supported for MVP.".to_string(),
        });
    }
    if context.interactive {
        return Err(MarsError::InvalidRequest {
            message: "Cursor subprocess interactive mode is not supported for MVP.".to_string(),
        });
    }

    let mut argv = vec![
        "cursor".to_string(),
        "agent".to_string(),
        "--print".to_string(),
        "--output-format".to_string(),
        "stream-json".to_string(),
        "--trust".to_string(),
    ];

    if let Some(model) = model(bundle) {
        argv.extend(["--model".to_string(), model.to_string()]);
    }

    match approval(bundle) {
        "yolo" | "never" => argv.push("--yolo".to_string()),
        "auto" => argv.push("--force".to_string()),
        "default" | "confirm" => {}
        other => {
            return Err(MarsError::InvalidRequest {
                message: format!("Cursor projection does not support approval mode '{other}'."),
            });
        }
    }

    match sandbox(bundle) {
        "default" => {}
        "read-only" => argv.extend(["--sandbox".to_string(), "enabled".to_string()]),
        "workspace-write" | "danger-full-access" => {
            argv.extend(["--sandbox".to_string(), "disabled".to_string()]);
        }
        other => {
            return Err(MarsError::InvalidRequest {
                message: format!("Cursor projection does not support sandbox mode '{other}'."),
            });
        }
    }

    argv.extend(["--workspace".to_string(), cwd(context)?]);

    argv.extend(context.extra_args.iter().cloned());
    if let Some(prompt) = context.prompt.as_deref() {
        argv.push(prompt.to_string());
    }

    subprocess_actions(context, argv, Vec::new(), None)
}

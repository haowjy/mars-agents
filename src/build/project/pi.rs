use crate::build::bundle::{LaunchActions, LaunchBundle, RuntimeContext};
use crate::build::project::{effort, model, subprocess_actions};
use crate::error::MarsError;

pub fn project(
    bundle: &LaunchBundle,
    context: &RuntimeContext,
) -> Result<LaunchActions, MarsError> {
    reject_managed_flag_collisions(&context.extra_args)?;

    let mut argv = vec!["pi".to_string(), "--mode".to_string(), "rpc".to_string()];

    if let Some(model) = model(bundle) {
        argv.extend(["--model".to_string(), pi_model_arg(model, effort(bundle))]);
    }

    let system_prompt = bundle.prompt_surface.system_instruction.trim();
    if !system_prompt.is_empty() {
        argv.extend([
            "--append-system-prompt".to_string(),
            bundle.prompt_surface.system_instruction.clone(),
        ]);
    }

    if let Some(session_id) = context
        .session_id
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
    {
        if context.fork {
            argv.extend(["--fork".to_string(), session_id.to_string()]);
        } else {
            argv.extend(["--session".to_string(), session_id.to_string()]);
        }
    }

    let session_dir = context
        .pi_session_dir
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .ok_or_else(|| MarsError::InvalidRequest {
            message:
                "Pi launch_actions require context.pi_session_dir for managed RPC session storage."
                    .to_string(),
        })?;
    argv.extend(["--session-dir".to_string(), session_dir.to_string()]);

    if !context.load_all_pi_extensions {
        argv.push("--no-extensions".to_string());
    }

    argv.extend([
        "--no-skills".to_string(),
        "--no-context-files".to_string(),
        "--no-prompt-templates".to_string(),
    ]);

    for entrypoint in &context.pi_extension_entrypoints {
        argv.extend(["-e".to_string(), entrypoint.clone()]);
    }

    argv.extend(context.extra_args.iter().cloned());

    // TODO(launch-actions-parity, launch-bundle-projection): prompt on CLI vs delivered over RPC post-startup
    if let Some(prompt) = context.prompt.as_deref() {
        argv.push(prompt.to_string());
    }

    subprocess_actions(context, argv, Vec::new(), None)
}

fn pi_model_arg(model: &str, effort: Option<&str>) -> String {
    match effort.and_then(effort_to_thinking) {
        Some(thinking) => format!("{}:{thinking}", model.trim()),
        None => model.trim().to_string(),
    }
}

fn effort_to_thinking(effort: &str) -> Option<&'static str> {
    match effort.trim().to_ascii_lowercase().as_str() {
        "low" => Some("minimal"),
        "medium" => Some("medium"),
        "high" => Some("high"),
        "max" | "xhigh" => Some("xhigh"),
        _ => None,
    }
}

fn reject_managed_flag_collisions(extra_args: &[String]) -> Result<(), MarsError> {
    if has_flag(extra_args, "--mode") {
        return Err(MarsError::InvalidRequest {
            message: "Pi harness owns --mode and always launches in RPC mode; remove --mode from passthrough extra_args".to_string(),
        });
    }
    if has_flag(extra_args, "--no-extensions")
        || has_flag(extra_args, "-e")
        || has_flag(extra_args, "--extension")
    {
        return Err(MarsError::InvalidRequest {
            message: "Pi harness owns extension loading for RPC launches; remove --no-extensions or -e/--extension from passthrough extra_args".to_string(),
        });
    }
    if has_flag(extra_args, "--session-dir") {
        return Err(MarsError::InvalidRequest {
            message: "Pi harness owns --session-dir for Meridian-managed session isolation; remove --session-dir from passthrough extra_args".to_string(),
        });
    }
    Ok(())
}

fn has_flag(args: &[String], flag: &str) -> bool {
    args.iter().any(|token| {
        token == flag
            || token.starts_with(&format!("{flag}="))
            || (flag == "-e" && token.starts_with("-e") && token != "-e")
    })
}

//! External process invocation.
//!
//! Centralizes git and other external tool execution.

use std::path::Path;
use std::process::Command;

use crate::error::MarsError;

/// Run a git command and return stdout on success.
///
/// Arguments are passed as an explicit argv array, never through a shell.
/// Errors include context, arguments, exit code, and stderr.
pub fn run_git(args: &[&str], cwd: &Path, context: &str) -> Result<String, MarsError> {
    // Placeholder - will be implemented in Slice 7
    // For now, run git directly
    let command_display = if args.is_empty() {
        "git".to_string()
    } else {
        format!("git {}", args.join(" "))
    };

    let output = Command::new("git")
        .current_dir(cwd)
        .args(args)
        .output()
        .map_err(|e| MarsError::GitCli {
            command: command_display.clone(),
            message: format!("{context}: failed to execute git: {e}"),
        })?;

    if output.status.success() {
        Ok(String::from_utf8_lossy(&output.stdout).trim().to_string())
    } else {
        let stderr = String::from_utf8_lossy(&output.stderr);
        Err(MarsError::GitCli {
            command: command_display,
            message: format!(
                "{context}: git {} failed (exit {}): {}",
                args.join(" "),
                output.status.code().unwrap_or(-1),
                stderr.trim()
            ),
        })
    }
}

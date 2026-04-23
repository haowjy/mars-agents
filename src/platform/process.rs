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
    let command_display = display_command(args);
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
        let stdout = String::from_utf8_lossy(&output.stdout);
        let error_output = if stderr.trim().is_empty() {
            stdout.trim()
        } else {
            stderr.trim()
        };

        Err(MarsError::GitCli {
            command: command_display,
            message: format!(
                "{context}: exit {}: {}",
                output.status.code().unwrap_or(-1),
                error_output
            ),
        })
    }
}

/// Run a git command with a specific ref argument that may contain special characters.
///
/// Wraps `run_git` but takes the ref as a separate argument to ensure it's passed correctly.
pub fn run_git_with_ref(
    base_args: &[&str],
    ref_arg: &str,
    cwd: &Path,
    context: &str,
) -> Result<String, MarsError> {
    let mut args: Vec<&str> = base_args.to_vec();
    args.push(ref_arg);
    run_git(&args, cwd, context)
}

/// Display a command for error messages (not for execution).
pub fn display_command(args: &[&str]) -> String {
    if args.is_empty() {
        "git".to_string()
    } else {
        format!("git {}", args.join(" "))
    }
}

#[cfg(test)]
mod tests {
    use tempfile::TempDir;

    use super::*;

    #[test]
    fn run_git_version_succeeds() {
        // git --version should work in any environment with git
        let tmp = TempDir::new().unwrap();
        let result = run_git(&["--version"], tmp.path(), "test");
        assert!(result.is_ok(), "git --version should succeed: {:?}", result);
        assert!(result.unwrap().contains("git version"));
    }

    #[test]
    fn run_git_invalid_command_fails() {
        let tmp = TempDir::new().unwrap();
        let result = run_git(&["not-a-real-command"], tmp.path(), "test");
        assert!(result.is_err());

        let err = result.unwrap_err();
        let err_str = err.to_string();
        assert!(err_str.contains("test"), "error should include context");
        assert!(
            err_str.contains("not-a-real-command"),
            "error should include command"
        );
    }

    #[test]
    fn display_command_formats_args() {
        assert_eq!(display_command(&["status", "-s"]), "git status -s");
        assert_eq!(
            display_command(&["log", "--oneline", "-5"]),
            "git log --oneline -5"
        );
    }
}

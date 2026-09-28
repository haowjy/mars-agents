//! External process invocation.
//!
//! Centralizes git and other external tool execution.

use std::ffi::{OsStr, OsString};
use std::io;
use std::path::Path;
use std::process::Command;
#[cfg(not(windows))]
use std::process::Stdio;

#[cfg(windows)]
mod detached_windows;

/// Launch an independent child from explicit program/argv, without a shell.
///
/// The child inherits the ambient environment and current directory, but no
/// caller stdio handles. On POSIX it starts a new session and is reaped on a
/// background thread; on Windows it has no inherited handles and its process
/// handles are closed immediately. No exit status is returned to the caller.
pub fn spawn_detached(program: &OsStr, args: &[OsString]) -> io::Result<()> {
    #[cfg(windows)]
    {
        detached_windows::spawn(program, args)
    }
    #[cfg(not(windows))]
    {
        let mut command = Command::new(program);
        command
            .args(args)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null());
        #[cfg(unix)]
        {
            use std::os::unix::process::CommandExt;
            unsafe {
                command.pre_exec(|| {
                    if libc::setsid() == -1 {
                        Err(io::Error::last_os_error())
                    } else {
                        Ok(())
                    }
                });
            }
        }
        // Create the reaper first so a successful spawn never leaves a zombie.
        let (tx, rx) = std::sync::mpsc::sync_channel::<std::process::Child>(1);
        std::thread::Builder::new()
            .name("mars-detached-reaper".to_string())
            .spawn(move || {
                if let Ok(mut child) = rx.recv() {
                    let _ = child.wait();
                }
            })?;
        let child = command.spawn()?;
        if let Err(error) = tx.send(child) {
            let mut child = error.0;
            let _ = child.kill();
            let _ = child.wait();
            return Err(io::Error::other("detached child reaper exited early"));
        }
        Ok(())
    }
}

use crate::error::MarsError;

const GIT_LOCAL_ENV_VARS: &[&str] = &[
    "GIT_ALTERNATE_OBJECT_DIRECTORIES",
    "GIT_CONFIG",
    "GIT_CONFIG_PARAMETERS",
    "GIT_CONFIG_COUNT",
    "GIT_OBJECT_DIRECTORY",
    "GIT_DIR",
    "GIT_WORK_TREE",
    "GIT_IMPLICIT_WORK_TREE",
    "GIT_GRAFT_FILE",
    "GIT_INDEX_FILE",
    "GIT_NO_REPLACE_OBJECTS",
    "GIT_REPLACE_REF_BASE",
    "GIT_PREFIX",
    "GIT_SHALLOW_FILE",
    "GIT_COMMON_DIR",
];

/// Remove repository-scoped Git environment inherited from hooks or parent git commands.
///
/// Git honors variables like `GIT_DIR` over `current_dir`. Leaving them set can make a
/// command intended for a temp repo mutate the caller's checkout instead.
pub(crate) fn remove_git_local_env(command: &mut Command) {
    for var in GIT_LOCAL_ENV_VARS {
        command.env_remove(var);
    }
}

/// Run a git command and return stdout on success.
///
/// Arguments are passed as an explicit argv array, never through a shell.
/// Errors include context, arguments, exit code, and stderr.
pub fn run_git(args: &[&str], cwd: &Path, context: &str) -> Result<String, MarsError> {
    let command_display = display_command(args);
    let mut command = Command::new("git");
    remove_git_local_env(&mut command);
    let output = command
        .current_dir(cwd)
        .args(args)
        .output()
        .map_err(|e| MarsError::GitCli {
            command: command_display.clone(),
            message: format!(
                "{context} (cwd: {}): failed to execute git: {e}",
                cwd.display()
            ),
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
    use std::process::Command;

    use tempfile::TempDir;

    use super::*;

    fn test_git_command() -> Command {
        let mut command = Command::new("git");
        remove_git_local_env(&mut command);
        command.env("GIT_AUTHOR_NAME", "Mars Test");
        command.env("GIT_AUTHOR_EMAIL", "mars@example.com");
        command.env("GIT_COMMITTER_NAME", "Mars Test");
        command.env("GIT_COMMITTER_EMAIL", "mars@example.com");
        command
    }

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
    fn run_git_execute_failure_includes_cwd_and_command() {
        let missing = std::env::temp_dir().join("mars-run-git-missing-cwd");
        let result = run_git(&["status", "--short"], &missing, "test");
        let err = result.expect_err("missing cwd should fail before git runs");
        let message = err.to_string();

        assert!(message.contains("git status --short"));
        assert!(message.contains("cwd:"));
        assert!(message.contains(&missing.display().to_string()));
    }

    #[test]
    fn display_command_formats_args() {
        assert_eq!(display_command(&["status", "-s"]), "git status -s");
        assert_eq!(
            display_command(&["log", "--oneline", "-5"]),
            "git log --oneline -5"
        );
    }

    #[test]
    fn run_git_with_ref_passes_ref_without_shell_interpretation() {
        let tmp = TempDir::new().unwrap();
        test_git_command()
            .current_dir(tmp.path())
            .args(["init", "."])
            .output()
            .expect("git init");
        std::fs::write(tmp.path().join("README.md"), "hello").unwrap();
        test_git_command()
            .current_dir(tmp.path())
            .args(["add", "README.md"])
            .output()
            .expect("git add");
        test_git_command()
            .current_dir(tmp.path())
            .args(["commit", "-m", "init"])
            .output()
            .expect("git commit");

        let result = run_git_with_ref(
            &["rev-parse", "--verify"],
            "HEAD;echo shell-injected",
            tmp.path(),
            "verify ref",
        );

        let err = result.expect_err("metacharacter ref should be passed as one invalid git ref");
        let message = err.to_string();
        assert!(message.contains("HEAD;echo shell-injected"));
        assert!(!message.contains("shell-injected\n"));
    }

    #[test]
    fn detached_launch_reports_missing_executable() {
        let tmp = TempDir::new().unwrap();
        let missing = tmp.path().join("missing-worker");
        let error = spawn_detached(missing.as_os_str(), &[]).unwrap_err();
        assert_eq!(error.kind(), std::io::ErrorKind::NotFound);
    }

    #[cfg(unix)]
    #[test]
    fn detached_launch_passes_argv_without_shell_expansion_and_inherits_cwd() {
        use std::ffi::OsString;
        use std::time::{Duration, Instant};

        let tmp = TempDir::new().unwrap();
        let output = tmp.path().join("child output.txt");
        let literal = "literal; $(touch should-not-exist) \\\"quoted\\\"";
        // The shell is the *explicit executable* here, not an implicit layer
        // inserted by the launcher. Its positional parameter is the argv probe.
        spawn_detached(
            std::ffi::OsStr::new("/bin/sh"),
            &[
                OsString::from("-c"),
                OsString::from("printf '%s\\n%s\\n' \"$1\" \"$PWD\" > \"$2\""),
                OsString::from("--"),
                OsString::from(literal),
                output.as_os_str().to_os_string(),
            ],
        )
        .unwrap();
        let deadline = Instant::now() + Duration::from_secs(5);
        let result = loop {
            if let Ok(result) = std::fs::read_to_string(&output)
                && result.ends_with('\n')
                && result.lines().count() >= 2
            {
                break result;
            }
            assert!(
                Instant::now() < deadline,
                "detached child should write result"
            );
            std::thread::sleep(Duration::from_millis(10));
        };
        let mut lines = result.lines();
        assert_eq!(lines.next(), Some(literal));
        assert_eq!(
            lines.next(),
            Some(std::env::current_dir().unwrap().to_str().unwrap())
        );
        assert!(!tmp.path().join("should-not-exist").exists());
    }
}

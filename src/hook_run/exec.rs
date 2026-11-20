//! Running one hook command, ported from `runCommand` in `typescript/core.ts`: the shell, the
//! event's JSON on stdin, one deadline over the shell's exit and its pipes closing, then the
//! shell's whole process group killed.

use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::mpsc::{self, RecvTimeoutError};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

use super::logic::{Handler, Run};

/// What a reader thread has collected so far: a deadline returns it as it stands.
type Collected = Arc<Mutex<Vec<u8>>>;

/// The shell and what it runs. Windows: the Git install's bash, never `System32\bash.exe`,
/// which is WSL's and cannot resolve Windows paths.
fn shell(handler: &Handler) -> Result<(PathBuf, &str), String> {
    if !cfg!(windows) {
        return Ok(("sh".into(), &handler.command));
    }
    let command = handler.command_windows.as_ref().unwrap_or(&handler.command);
    let bash = find_git_bash().ok_or("Git Bash was not found, so the command cannot run")?;
    Ok((bash, command))
}

fn find_git_bash() -> Option<PathBuf> {
    let env = |name: &str| std::env::var_os(name).map(PathBuf::from);
    let mut found: Vec<PathBuf> = env("CLAUDE_CODE_GIT_BASH_PATH").into_iter().collect();
    let bases = [
        env("ProgramFiles"),
        env("ProgramFiles(x86)"),
        env("LOCALAPPDATA").map(|dir| dir.join("Programs")),
    ];
    found.extend(
        bases
            .into_iter()
            .flatten()
            .map(|base| base.join("Git").join("bin").join("bash.exe")),
    );
    if let Ok(output) = Command::new("where").arg("git").output() {
        for git in String::from_utf8_lossy(&output.stdout).lines() {
            if let Some(bin) = Path::new(git.trim()).parent() {
                found.push(bin.join("..").join("bin").join("bash.exe"));
                found.push(bin.join("..").join("..").join("bin").join("bash.exe"));
            }
        }
    }
    found.into_iter().find(|path| {
        path.exists()
            && !path
                .to_string_lossy()
                .to_ascii_lowercase()
                .replace('\\', "/")
                .contains("/system32/")
    })
}

fn spawn(handler: &Handler, stdin: Stdio, async_: bool, cwd: &Path) -> Result<Child, String> {
    let (shell, command) = shell(handler)?;
    let output = || {
        if async_ {
            Stdio::null()
        } else {
            Stdio::piped()
        }
    };
    let mut process = Command::new(shell);
    process
        .arg("-c")
        .arg(command)
        .current_dir(cwd)
        .env("PWD", cwd)
        .stdin(stdin)
        .stdout(output())
        .stderr(output());
    #[cfg(unix)]
    std::os::unix::process::CommandExt::process_group(&mut process, 0);
    #[cfg(windows)]
    std::os::windows::process::CommandExt::creation_flags(&mut process, 0x0800_0000);
    process.spawn().map_err(|error| error.to_string())
}

/// Reads `pipe` into the returned buffer, and sends on `done` at the end of the stream.
fn collect(mut pipe: impl Read + Send + 'static, done: mpsc::Sender<()>) -> Collected {
    let collected = Collected::default();
    let into = Arc::clone(&collected);
    thread::spawn(move || {
        let mut chunk = [0u8; 8192];
        while let Ok(n) = pipe.read(&mut chunk) {
            if n == 0 {
                break;
            }
            into.lock().unwrap().extend_from_slice(&chunk[..n]);
        }
        let _ = done.send(());
    });
    collected
}

fn text(collected: &Collected) -> String {
    String::from_utf8_lossy(&collected.lock().unwrap()).into_owned()
}

/// Runs `handler` to its end; `None` for an `async` handler, which Claude never waits for or
/// times out: it is started with its whole stdin queued in a file and no stdout or stderr,
/// and left running after the runner exits.
pub(super) fn run(handler: &Handler, stdin: &str, cwd: &Path) -> Option<Run> {
    let failed = |why: String| {
        Some(Run {
            why,
            ..Run::default()
        })
    };
    if handler.is_async {
        // A pipe would die with this process, with its tail unwritten.
        let file = tempfile::tempfile().and_then(|mut file| {
            use std::io::{Seek, SeekFrom};
            file.write_all(stdin.as_bytes())?;
            file.seek(SeekFrom::Start(0))?;
            Ok(file)
        });
        return match file
            .map_err(|error| error.to_string())
            .and_then(|file| spawn(handler, Stdio::from(file), true, cwd))
        {
            Ok(_) => None,
            Err(why) => failed(why),
        };
    }
    let mut child = match spawn(handler, Stdio::piped(), false, cwd) {
        Ok(child) => child,
        Err(why) => return failed(why),
    };
    let pid = child.id();
    let mut pipe = child.stdin.take().expect("stdin is piped");
    let input = stdin.to_owned();
    // A command that never reads its stdin must not stall the write.
    thread::spawn(move || {
        let _ = pipe.write_all(input.as_bytes());
    });
    let (done, finished) = mpsc::channel();
    let stdout = collect(child.stdout.take().expect("stdout is piped"), done.clone());
    let stderr = collect(child.stderr.take().expect("stderr is piped"), done.clone());
    let (exited, status) = mpsc::channel();
    thread::spawn(move || {
        let _ = exited.send(child.wait());
        let _ = done.send(());
    });
    let deadline = handler
        .timeout
        .map(|seconds| Instant::now() + Duration::from_secs_f64(seconds));
    // The shell's exit and both pipes closing: a background job can hold the pipes after
    // the shell is gone.
    for _ in 0..3 {
        let wait = deadline.map(|at| at.saturating_duration_since(Instant::now()));
        let arrived = match wait {
            Some(wait) => finished.recv_timeout(wait),
            None => finished.recv().map_err(|_| RecvTimeoutError::Disconnected),
        };
        if arrived.is_err() {
            let exited = status.try_recv().is_ok();
            kill_group(pid, exited);
            return Some(Run {
                ok: false,
                status: None,
                stdout: text(&stdout),
                stderr: text(&stderr),
                why: format!("timed out after {}s", handler.timeout.unwrap_or_default()),
            });
        }
    }
    let (code, why) = match status.recv() {
        Ok(Ok(status)) => match status.code() {
            Some(code) => (Some(code), format!("exit {code}")),
            None => (None, "killed by a signal".to_owned()),
        },
        Ok(Err(error)) => (None, error.to_string()),
        Err(_) => (None, "the shell was lost".to_owned()),
    };
    Some(Run {
        ok: code.is_some(),
        status: code,
        stdout: text(&stdout),
        stderr: text(&stderr),
        why,
    })
}

/// The shell and everything it started. POSIX: the shell leads its own process group, which
/// outlives the shell while a member runs. Windows: `taskkill /T`, only while the shell is
/// alive, as its pid may be reused after.
fn kill_group(pid: u32, shell_exited: bool) {
    #[cfg(unix)]
    {
        let _ = shell_exited;
        // SAFETY: kill(2) on the process group this runner created for the command.
        unsafe {
            libc::kill(-(pid as i32), libc::SIGKILL);
        }
    }
    #[cfg(not(unix))]
    if !shell_exited {
        let _ = Command::new("taskkill")
            .args(["/pid", &pid.to_string(), "/T", "/F"])
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status();
    }
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use std::time::Instant;

    fn handler(command: &str, timeout: Option<f64>, is_async: bool) -> Handler {
        Handler {
            command: command.to_owned(),
            timeout,
            is_async,
            ..Handler::default()
        }
    }

    fn wait_for(path: &Path, bytes: Option<u64>) -> bool {
        for _ in 0..100 {
            if let Ok(meta) = std::fs::metadata(path)
                && bytes.is_none_or(|n| meta.len() == n)
            {
                return true;
            }
            thread::sleep(Duration::from_millis(50));
        }
        false
    }

    #[test]
    fn a_background_job_holding_the_pipes_cannot_outlast_the_timeout() {
        let dir = tempfile::tempdir().unwrap();
        let started = Instant::now();
        let run = run(
            &handler("sleep 20 & echo started", Some(1.0), false),
            "{}",
            dir.path(),
        )
        .unwrap();
        assert!(started.elapsed() < Duration::from_secs(10));
        assert!(!run.ok, "{run:?}");
        assert!(run.why.starts_with("timed out"), "{run:?}");
        assert_eq!(run.stdout, "started\n");
    }

    #[test]
    fn a_finished_command_reports_exit_stdout_and_stderr() {
        let dir = tempfile::tempdir().unwrap();
        let run = run(
            &handler("cat; echo bad >&2; exit 2", Some(5.0), false),
            "in",
            dir.path(),
        )
        .unwrap();
        assert_eq!(
            (run.ok, run.status, run.stdout.as_str(), run.stderr.as_str()),
            (true, Some(2), "in", "bad\n")
        );
    }

    #[test]
    fn an_async_handler_that_writes_to_stdout_is_not_killed_by_it() {
        let dir = tempfile::tempdir().unwrap();
        let done = dir.path().join("done");
        let command = format!(
            "sleep 0.3; echo out; echo err >&2; touch '{}'",
            done.display()
        );
        assert!(run(&handler(&command, None, true), "{}", dir.path()).is_none());
        assert!(wait_for(&done, None));
    }
}

//! Running one hook command: `sh -c`, the event's JSON on stdin, killed with its process
//! group at the handler's timeout.

use std::io::{Read, Write};
use std::path::Path;
use std::process::{Child, Command, Stdio};
use std::thread;
use std::time::Duration;

use wait_timeout::ChildExt;

use super::logic::{Handler, Run};

fn spawn(handler: &Handler, stdin: &str, cwd: &Path) -> Result<Child, String> {
    let windows = cfg!(windows);
    let command = match (&handler.command_windows, windows) {
        (Some(command), true) => command,
        _ => &handler.command,
    };
    // On Windows the shell is bash from PATH (Git Bash): untested.
    let mut process = Command::new(if windows { "bash" } else { "sh" });
    process
        .arg("-c")
        .arg(command)
        .current_dir(cwd)
        .env("PWD", cwd)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    #[cfg(unix)]
    std::os::unix::process::CommandExt::process_group(&mut process, 0);
    let mut child = process.spawn().map_err(|error| error.to_string())?;
    let mut pipe = child.stdin.take().expect("stdin is piped");
    let input = stdin.to_owned();
    // A command that never reads its stdin must not stall the write.
    thread::spawn(move || {
        let _ = pipe.write_all(input.as_bytes());
    });
    Ok(child)
}

fn drain(mut pipe: impl Read + Send + 'static) -> thread::JoinHandle<String> {
    thread::spawn(move || {
        let mut bytes = Vec::new();
        let _ = pipe.read_to_end(&mut bytes);
        String::from_utf8_lossy(&bytes).into_owned()
    })
}

/// Runs `handler` to its end; `None` for an `async` handler, which Claude never waits for or
/// times out (it is started and left).
pub(super) fn run(handler: &Handler, stdin: &str, cwd: &Path) -> Option<Run> {
    let mut child = match spawn(handler, stdin, cwd) {
        Ok(child) => child,
        Err(why) => {
            return Some(Run {
                why,
                ..Run::default()
            });
        }
    };
    if handler.is_async {
        return None;
    }
    let stdout = drain(child.stdout.take().expect("stdout is piped"));
    let stderr = drain(child.stderr.take().expect("stderr is piped"));
    let waited = match handler.timeout {
        Some(seconds) => child.wait_timeout(Duration::from_secs_f64(seconds)),
        None => child.wait().map(Some),
    };
    let (status, why) = match waited {
        Ok(Some(status)) => match status.code() {
            Some(code) => (Some(code), format!("exit {code}")),
            None => (None, "killed by a signal".to_owned()),
        },
        Ok(None) => {
            kill_group(&mut child);
            (
                None,
                format!("timed out after {}s", handler.timeout.unwrap_or_default()),
            )
        }
        Err(error) => (None, error.to_string()),
    };
    Some(Run {
        ok: status.is_some(),
        status,
        stdout: stdout.join().unwrap_or_default(),
        stderr: stderr.join().unwrap_or_default(),
        why,
    })
}

/// The shell and everything it started: the shell leads its own process group.
fn kill_group(child: &mut Child) {
    #[cfg(unix)]
    // SAFETY: kill(2) on the group of a child this process started and has not reaped.
    unsafe {
        libc::kill(-(child.id() as i32), libc::SIGKILL);
    }
    #[cfg(not(unix))]
    let _ = child.kill();
    let _ = child.wait();
}

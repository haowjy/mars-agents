mod common;

use assert_cmd::cargo::cargo_bin;
use httpmock::prelude::*;
use serde_json::Value;
use serial_test::serial;
use std::fs;
use std::io::{Read, Write};
use std::net::TcpListener;
use std::process::{Command as StdCommand, Output};
use std::sync::mpsc;
use std::thread;
use std::time::{Duration, Instant};

use common::*;

#[test]
#[serial]
fn stale_catalog_returns_before_blocked_refresh_completes() {
    let server = MockServer::start();
    let (temp, project_root) = setup_project(&server);
    write_cache(&project_root, sample_cached_models(), &stale_fetched_at());
    let before = read_cache_raw(&project_root);
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let url = format!("http://{}/api.json", listener.local_addr().unwrap());
    let (seen_tx, seen_rx) = mpsc::channel();
    let (release_tx, release_rx) = mpsc::channel();
    let server_thread = thread::spawn(move || {
        let (mut stream, _) = listener.accept().unwrap();
        let mut request = [0; 4096];
        assert!(stream.read(&mut request).unwrap() > 0);
        seen_tx.send(()).unwrap();
        release_rx.recv().unwrap();
        let body = sample_catalog_json().to_string();
        write!(
            stream,
            "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
            body.len()
        )
        .unwrap();
    });
    let mut cmd = StdCommand::new(cargo_bin("mars"));
    configure_std_cmd(&mut cmd, temp.path(), &url);
    cmd.arg("--root")
        .arg(&project_root)
        .args(["--json", "models", "catalog"]);
    let (output_tx, output_rx) = mpsc::channel();
    let command_thread = thread::spawn(move || output_tx.send(cmd.output().unwrap()).unwrap());
    seen_rx
        .recv_timeout(Duration::from_secs(5))
        .expect("refresh request never arrived");
    let output_before_release = output_rx.recv_timeout(Duration::from_secs(2));
    let returned_early = output_before_release.is_ok();
    let mut second_cmd = StdCommand::new(cargo_bin("mars"));
    configure_std_cmd(&mut second_cmd, temp.path(), &url);
    second_cmd
        .arg("--root")
        .arg(&project_root)
        .args(["--json", "models", "catalog"]);
    let (second_tx, second_rx) = mpsc::channel();
    let second_thread =
        thread::spawn(move || second_tx.send(second_cmd.output().unwrap()).unwrap());
    let second_before_release = second_rx.recv_timeout(Duration::from_secs(2));
    let second_returned_early = second_before_release.is_ok();
    release_tx.send(()).unwrap();
    let output = output_before_release.or_else(|_| output_rx.recv_timeout(Duration::from_secs(5)));
    let second_output =
        second_before_release.or_else(|_| second_rx.recv_timeout(Duration::from_secs(5)));
    command_thread.join().unwrap();
    second_thread.join().unwrap();
    let server_result = server_thread.join();
    assert!(
        returned_early,
        "stale command or a descendant held output pipes until network response; after release: {}",
        output_diagnostic(&output)
    );
    assert!(
        second_returned_early,
        "claim check waited behind the network/cache lock; after release: {}",
        output_diagnostic(&second_output)
    );
    server_result.unwrap();
    let second_document: Value = serde_json::from_slice(&second_output.unwrap().stdout).unwrap();
    assert_eq!(
        second_document["cache_refresh"]["refresh"]["status"],
        "already_in_progress"
    );
    let output = output.unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let document: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(document["cache_refresh"]["status"], "stale");
    assert_eq!(document["cache_refresh"]["refresh"]["status"], "spawned");
    assert!(model_ids_from_catalog_json(&output.stdout).contains("gpt-5"));
    wait_until(Duration::from_secs(5), || {
        read_cache_raw(&project_root) != before
    });
    assert!(
        read_cache_json(&project_root)["fetched_at"]
            .as_str()
            .unwrap()
            .parse::<u64>()
            .unwrap()
            > stale_fetched_at().parse::<u64>().unwrap()
    );
}

#[test]
#[serial]
fn stale_concurrent_commands_coalesce_to_one_background_fetch() {
    let server = MockServer::start();
    let (temp, project_root) = setup_project(&server);
    write_cache(&project_root, sample_cached_models(), &stale_fetched_at());
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let api_url = format!("http://{}/api.json", listener.local_addr().unwrap());
    let (seen_tx, seen_rx) = mpsc::channel();
    let (release_tx, release_rx) = mpsc::channel();
    let server_thread = thread::spawn(move || {
        let (mut stream, _) = listener.accept().unwrap();
        let mut request = [0; 4096];
        assert!(stream.read(&mut request).unwrap() > 0);
        seen_tx.send(()).unwrap();
        release_rx.recv().unwrap();
        let body = sample_catalog_json().to_string();
        write!(
            stream,
            "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
            body.len()
        )
        .unwrap();
    });
    let commands: Vec<_> = (0..4)
        .map(|_| {
            let root = project_root.clone();
            let env_root = temp.path().to_path_buf();
            let url = api_url.clone();
            let (output_tx, output_rx) = mpsc::channel();
            let handle = thread::spawn(move || {
                let mut cmd = StdCommand::new(cargo_bin("mars"));
                configure_std_cmd(&mut cmd, &env_root, &url);
                let output = cmd
                    .arg("--root")
                    .arg(root)
                    .args(["--json", "models", "catalog"])
                    .output()
                    .unwrap();
                output_tx.send(output).unwrap();
            });
            (handle, output_rx)
        })
        .collect();
    seen_rx
        .recv_timeout(Duration::from_secs(5))
        .expect("background refresh did not start");
    let before_release: Vec<_> = commands
        .iter()
        .map(|(_, output_rx)| output_rx.recv_timeout(Duration::from_secs(2)))
        .collect();
    let returned_early = before_release
        .iter()
        .filter(|result| result.is_ok())
        .count();
    release_tx.send(()).unwrap();
    let outputs: Vec<_> = before_release
        .into_iter()
        .zip(&commands)
        .map(|(result, (_, output_rx))| {
            result.or_else(|_| output_rx.recv_timeout(Duration::from_secs(5)))
        })
        .collect();
    for (handle, _) in commands {
        handle.join().unwrap();
    }
    let server_result = server_thread.join();
    assert_eq!(
        returned_early,
        4,
        "all stale commands must close their output pipes before worker response; after release: {:?}; server: {:?}",
        outputs.iter().map(output_diagnostic).collect::<Vec<_>>(),
        server_result
    );
    server_result.unwrap();
    let mut spawned = 0;
    let mut in_progress = 0;
    for output in outputs {
        let output = output.unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        let document: Value = serde_json::from_slice(&output.stdout).unwrap();
        assert_eq!(document["cache_refresh"]["status"], "stale");
        match document["cache_refresh"]["refresh"]["status"].as_str() {
            Some("spawned") => spawned += 1,
            Some("already_in_progress") => in_progress += 1,
            other => panic!("unexpected concurrent refresh outcome: {other:?}"),
        }
    }
    assert_eq!(spawned, 1, "only one stale reader may launch a worker");
    assert_eq!(in_progress, 3);
    wait_until(Duration::from_secs(5), || {
        read_cache_json(&project_root)["fetched_at"]
            .as_str()
            .unwrap()
            .parse::<u64>()
            .unwrap()
            > stale_fetched_at().parse::<u64>().unwrap()
    });
    // A second request would fail: this listener accepts only one connection.
}

fn output_diagnostic(output: &Result<Output, mpsc::RecvTimeoutError>) -> String {
    match output {
        Ok(output) => format!(
            "status={}; stderr={:?}; stdout={:?}",
            output.status,
            String::from_utf8_lossy(&output.stderr),
            String::from_utf8_lossy(&output.stdout)
        ),
        Err(error) => error.to_string(),
    }
}

#[test]
#[serial]
fn forced_refresh_waits_for_blocked_network_response() {
    let server = MockServer::start();
    let (temp, project_root) = setup_project(&server);
    write_cache(&project_root, sample_cached_models(), &fresh_fetched_at());
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let url = format!("http://{}/api.json", listener.local_addr().unwrap());
    let (seen_tx, seen_rx) = mpsc::channel();
    let (release_tx, release_rx) = mpsc::channel();
    let server_thread = thread::spawn(move || {
        let (mut stream, _) = listener.accept().unwrap();
        let mut request = [0; 4096];
        assert!(stream.read(&mut request).unwrap() > 0);
        seen_tx.send(()).unwrap();
        release_rx.recv().unwrap();
        let body = sample_catalog_json().to_string();
        write!(
            stream,
            "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
            body.len()
        )
        .unwrap();
    });
    let mut cmd = StdCommand::new(cargo_bin("mars"));
    configure_std_cmd(&mut cmd, temp.path(), &url);
    let mut child = cmd
        .arg("--root")
        .arg(&project_root)
        .args(["--json", "models", "catalog", "--refresh-models"])
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .unwrap();
    seen_rx
        .recv_timeout(Duration::from_secs(5))
        .expect("forced request never arrived");
    let blocked = child.try_wait().unwrap().is_none();
    release_tx.send(()).unwrap();
    let output = child.wait_with_output().unwrap();
    server_thread.join().unwrap();
    assert!(blocked, "forced refresh returned before its response");
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let document: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(document["cache_refresh"]["status"], "refreshed");
}

#[test]
#[serial]
fn empty_background_refresh_keeps_cache_and_enters_cooldown() {
    let server = MockServer::start();
    let mock = server.mock(|when, then| {
        when.method(GET).path(API_PATH);
        then.status(200).json_body(serde_json::json!({}));
    });
    let (temp, project_root) = setup_project(&server);
    write_cache(&project_root, sample_cached_models(), &stale_fetched_at());
    let before = read_cache_raw(&project_root);
    let mut cmd = mars_cmd(&project_root, temp.path(), &server.url(API_PATH));
    cmd.args(["--json", "models", "catalog"]);
    cmd.assert().success();
    wait_until(Duration::from_secs(5), || {
        project_root.join(".mars/.models-cache.last-fail").exists()
    });
    wait_until(Duration::from_secs(5), || {
        !project_root
            .join(".mars/.models-cache.refresh-claim")
            .exists()
    });
    assert_eq!(read_cache_raw(&project_root), before);
    let mut again = mars_cmd(&project_root, temp.path(), &server.url(API_PATH));
    again.args(["--json", "models", "catalog"]);
    let output = again.assert().success().get_output().clone();
    let document: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(document["cache_refresh"]["refresh"]["status"], "cooldown");
    assert!(
        document["cache_refresh"]["last_failure"]
            .as_str()
            .unwrap()
            .contains("empty catalog")
    );
    assert_eq!(mock.hits(), 1);
}

#[test]
#[serial]
fn stale_disk_only_modes_launch_no_catalog_worker() {
    let server = MockServer::start();
    let mock = server.mock(|when, then| {
        when.method(GET).path(API_PATH);
        then.status(200).json_body(sample_catalog_json());
    });
    let (temp, project_root) = setup_project(&server);
    write_cache(&project_root, sample_cached_models(), &stale_fetched_at());
    for args in [
        vec!["models", "catalog", "--no-refresh-models"],
        vec!["models", "catalog"],
    ] {
        let mut cmd = mars_cmd(&project_root, temp.path(), &server.url(API_PATH));
        if args.len() == 2 {
            cmd.env("MARS_OFFLINE", "1");
        }
        cmd.arg("--json").args(args);
        let output = cmd.assert().success().get_output().clone();
        let document: Value = serde_json::from_slice(&output.stdout).unwrap();
        assert_eq!(document["cache_refresh"]["status"], "offline");
    }
    assert_eq!(mock.hits(), 0);
}

#[test]
#[serial]
fn hidden_worker_uses_portable_arguments_without_shell() {
    let server = MockServer::start();
    let mock = server.mock(|when, then| {
        when.method(GET).path(API_PATH);
        then.status(200).json_body(sample_catalog_json());
    });
    let (temp, original_root) = setup_project(&server);
    let project_root = temp.path().join("project with spaces");
    fs::rename(&original_root, &project_root).unwrap();
    let mars_dir = project_root.join(".mars");
    // The same directory may arrive with lexical `..` components (and Windows
    // may canonicalize the project root's casing independently).
    let equivalent_mars_dir = project_root
        .join("..")
        .join(project_root.file_name().unwrap())
        .join(".mars");
    write_cache(&project_root, sample_cached_models(), &stale_fetched_at());
    fs::write(
        mars_dir.join(".models-cache.refresh-claim"),
        serde_json::json!({"token": "worker-fixture", "at": now_unix_secs()}).to_string(),
    )
    .unwrap();
    let mut cmd = StdCommand::new(cargo_bin("mars"));
    configure_std_cmd(&mut cmd, temp.path(), &server.url(API_PATH));
    let output = cmd
        .env("PATH", "")
        .arg("--root")
        .arg(&project_root)
        .args(["models", "__refresh-catalog", "--mars-dir"])
        .arg(&equivalent_mars_dir)
        .args([
            "--refresh-after-hours",
            "24",
            "--provider",
            "anthropic",
            "--provider",
            "openai",
            "--expected-revision",
            "0",
            "--claim-token",
            "worker-fixture",
        ])
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(mock.hits(), 1);
    assert!(!mars_dir.join(".models-cache.refresh-claim").exists());
    assert!(
        read_cache_json(&project_root)["models"]
            .as_array()
            .unwrap()
            .len()
            >= 2
    );
}

fn wait_until(timeout: Duration, mut condition: impl FnMut() -> bool) {
    let deadline = Instant::now() + timeout;
    while !condition() {
        assert!(
            Instant::now() < deadline,
            "condition not met within {timeout:?}"
        );
        thread::sleep(Duration::from_millis(10));
    }
}

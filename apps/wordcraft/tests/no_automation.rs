//! Built without the `automation` feature (the default, and every release build), the app has no
//! control channel: `--control`, the automation root flags and `WORDCRAFT_CONTROL_PORT` are refused
//! with exit code 2 before any window opens, never quietly ignored.
#![cfg(not(feature = "automation"))]

use std::process::{Command, Output, Stdio};
use std::time::{Duration, Instant};

fn app(args: &[&str], port: Option<&str>) -> Output {
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_etch"));
    cmd.args(args)
        // With no display, an app that tries to open a window fails at once instead.
        .env_remove("DISPLAY")
        .env_remove("WAYLAND_DISPLAY")
        .env_remove("WAYLAND_SOCKET")
        .env_remove("WORDCRAFT_CONTROL_PORT")
        .env("WORDCRAFT_NO_PREFS", "1")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    if let Some(port) = port {
        cmd.env("WORDCRAFT_CONTROL_PORT", port);
    }
    let mut child = cmd.spawn().unwrap();
    let deadline = Instant::now() + Duration::from_secs(30);
    while child.try_wait().unwrap().is_none() {
        if Instant::now() > deadline {
            let _ = child.kill();
            panic!("etch {args:?} was still running after 30 s: it started the app");
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    child.wait_with_output().unwrap()
}

#[test]
fn control_requests_are_refused_without_automation_support() {
    let cases: [(&[&str], Option<&str>, &str); 6] = [
        (&["--control", "7981"], None, "--control"),
        (&["--control", "abc"], None, "--control"),
        (&["--sample", "--control"], None, "--control"),
        (&["--automation-read-root", "/tmp"], None, "--automation-read-root / --automation-write-root"),
        (&["--automation-write-root=/tmp"], None, "--automation-read-root / --automation-write-root"),
        (&["--sample"], Some("7981"), "WORDCRAFT_CONTROL_PORT"),
    ];
    for (args, port, what) in cases {
        let out = app(args, port);
        let stderr = String::from_utf8_lossy(&out.stderr);
        assert_eq!(out.status.code(), Some(2), "{args:?}: stderr: {stderr}");
        assert_eq!(stderr, format!("etch: {what}: built without automation support\n"), "{args:?}");
    }
}

#[test]
fn version_still_wins() {
    let out = app(&["--control", "7981", "--version"], None);
    assert!(out.status.success(), "stderr: {}", String::from_utf8_lossy(&out.stderr));
    assert!(String::from_utf8_lossy(&out.stdout).starts_with("etch "));
}

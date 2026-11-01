//! A reader that closes stdout early (`etch-cli commands --json | head`) must end the CLI
//! quietly with exit 0, never with a panic (#508).
#![cfg(unix)]

use std::io::Read;
use std::process::{Command, Stdio};

fn cli() -> Command {
    Command::new(env!("CARGO_BIN_EXE_etch-cli"))
}

#[test]
fn commands_json_into_a_closed_pipe_exits_quietly() {
    let mut child = cli().args(["commands", "--json"]).stdout(Stdio::piped()).stderr(Stdio::piped()).spawn().unwrap();
    // The listing is far larger than a pipe buffer: read a little, then hang up like `head -c`.
    let mut stdout = child.stdout.take().unwrap();
    let mut first = [0u8; 16];
    stdout.read_exact(&mut first).unwrap();
    drop(stdout);
    let out = child.wait_with_output().unwrap();
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(out.status.success(), "status {:?}, stderr: {stderr}", out.status);
    assert!(!stderr.contains("panicked"), "stderr: {stderr}");
}

#![cfg(target_os = "linux")]

use serde_json::{Value, json};
use std::ffi::OsStr;
use std::os::unix::ffi::OsStrExt;
use std::os::unix::fs::PermissionsExt;
use std::path::PathBuf;
use std::process::{Command, Output};
use std::sync::atomic::{AtomicUsize, Ordering};

const CAPTURE: &str = r#"#!/bin/sh
printf '%s\000' "$@" > "$AGENTURM_TEST_WINDOW_CAPTURE"
exit 23
"#;

#[test]
fn graphical_launcher_receives_opaque_arguments_and_returns_its_status() {
    let test = Fixture::new();
    test.launcher("gnome-terminal", CAPTURE);
    let path = "history ; echo confidential; $(touch unexpected).sqlite";
    let result = test.run(
        &["cockpit", "--db", path, "--new-window"],
        &[("DISPLAY", ":1")],
    );
    assert_eq!(result.status.code(), Some(23));
    assert!(result.stdout.is_empty());
    assert!(!String::from_utf8_lossy(&result.stderr).contains(path));
    let captured = test.captured();
    assert_eq!(&captured[..2], ["--wait", "--"]);
    assert_eq!(
        PathBuf::from(&captured[2]),
        std::fs::canonicalize(env!("CARGO_BIN_EXE_agenturm")).unwrap()
    );
    assert_eq!(captured[3], "__cockpit-window");
    assert!(!captured.iter().any(|argument| argument.contains(path)));
    let handoff = decode(&captured[4]);
    assert_eq!(
        handoff["arguments"],
        json!([b"cockpit".as_slice(), b"--db".as_slice(), path.as_bytes()])
    );
    assert_eq!(
        handoff["directory"],
        json!(test.working.as_os_str().as_bytes())
    );
    assert!(!test.working.join(path).exists());
    assert!(!test.working.join("unexpected").exists());
}

#[test]
fn tmux_targets_caller_session_and_broker_syntax_does_not_open_history() {
    let test = Fixture::new();
    test.launcher("tmux", CAPTURE);
    let config = "missing broker ;.json";
    let result = test.run(
        &[
            "broker",
            "--broker-config",
            config,
            "cockpit",
            "--new-window",
        ],
        &[("TMUX", "/tmp/tmux-test,123,4")],
    );
    assert_eq!(result.status.code(), Some(23));
    let capture = test.captured();
    assert_eq!(
        &capture[..6],
        ["new-window", "-t", "$4:", "-n", "Agenturm", "--"]
    );
    assert_eq!(capture[7], "__cockpit-window");
    assert_eq!(
        decode(&capture[8])["arguments"],
        json!([
            b"broker".as_slice(),
            b"--broker-config".as_slice(),
            config.as_bytes(),
            b"cockpit".as_slice()
        ])
    );
    assert!(!test.working.join(config).exists());
    assert!(!test.working.join("broker-history").exists());
}

#[test]
fn resumed_cockpit_uses_the_callers_directory_for_relative_history() {
    let test = Fixture::new();
    let relative = "history ;.sqlite";
    std::fs::write(test.working.join(relative), "invalid local history fixture").unwrap();
    let expected = test.run(&["cockpit", "--db", relative], &[]);
    assert_eq!(expected.status.code(), Some(1));
    let expected = String::from_utf8(expected.stderr).unwrap();
    assert!(expected.contains("file is not a database"), "{expected}");
    test.launcher(
        "gnome-terminal",
        r#"#!/bin/sh
cd /
shift 2
exec "$@"
"#,
    );
    let error = test.run(
        &["cockpit", "--new-window", "--db", relative],
        &[("DISPLAY", ":1")],
    );
    assert_eq!(error.status.code(), Some(1));
    let stderr = String::from_utf8_lossy(&error.stderr);
    assert!(stderr.starts_with(&expected), "{stderr}");
}

#[test]
fn rejects_invalid_modes_and_missing_launcher_before_startup() {
    let test = Fixture::new();
    for (arguments, message) in [
        (
            vec!["cockpit", "--new-window", "--once"],
            "interactive cockpit",
        ),
        (
            vec!["cockpit", "--new-window", "--new-window"],
            "Specify --new-window once",
        ),
        (
            vec!["__cockpit-window", "0"],
            "Invalid cockpit window handoff",
        ),
    ] {
        let result = test.run(&arguments, &[]);
        assert_eq!(result.status.code(), Some(2));
        assert!(String::from_utf8_lossy(&result.stderr).contains(message));
    }
    let payload = json!({
        "directory": test.working.as_os_str().as_bytes(),
        "arguments": [b"run".as_slice(), b"unexpected".as_slice()],
        "environment": [null, null, null, null],
    })
    .to_string()
    .bytes()
    .map(|byte| format!("{byte:02x}"))
    .collect::<String>();
    let result = test.run(&["__cockpit-window", &payload], &[]);
    assert_eq!(result.status.code(), Some(2));
    assert!(String::from_utf8_lossy(&result.stderr).contains("Invalid cockpit window handoff"));

    let result = test.run(&["cockpit", "--new-window", "--db", "missing.sqlite"], &[]);
    assert_eq!(result.status.code(), Some(1));
    assert!(String::from_utf8_lossy(&result.stderr).contains("No supported terminal launcher"));
    assert!(!test.working.join("missing.sqlite").exists());
}

fn decode(encoded: &str) -> Value {
    let bytes: Vec<_> = (0..encoded.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(&encoded[i..i + 2], 16).unwrap())
        .collect();
    serde_json::from_slice(&bytes).unwrap()
}

struct Fixture {
    root: PathBuf,
    working: PathBuf,
}

impl Fixture {
    fn new() -> Self {
        static NEXT: AtomicUsize = AtomicUsize::new(0);
        let root = std::env::temp_dir().join(format!(
            "agenturm-window-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        let working = root.join("project ; workspace");
        std::fs::create_dir(&root).unwrap();
        std::fs::create_dir(&working).unwrap();
        Self { root, working }
    }

    fn launcher(&self, name: &str, script: &str) {
        let launcher = self.root.join(name);
        std::fs::write(&launcher, script).unwrap();
        std::fs::set_permissions(launcher, std::fs::Permissions::from_mode(0o700)).unwrap();
    }

    fn run(&self, args: &[&str], env: &[(&str, &str)]) -> Output {
        Command::new(env!("CARGO_BIN_EXE_agenturm"))
            .args(args)
            .current_dir(&self.working)
            .env_clear()
            .env("HOME", &self.root)
            .env("XDG_STATE_HOME", self.root.join("state"))
            .env("PATH", &self.root)
            .env("AGENTURM_TEST_WINDOW_CAPTURE", self.root.join("capture"))
            .envs(env.iter().copied())
            .output()
            .unwrap()
    }

    fn captured(&self) -> Vec<String> {
        std::fs::read(self.root.join("capture"))
            .unwrap()
            .split(|byte| *byte == 0)
            .filter(|bytes| !bytes.is_empty())
            .map(|bytes| OsStr::from_bytes(bytes).to_str().unwrap().to_owned())
            .collect()
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.root);
    }
}

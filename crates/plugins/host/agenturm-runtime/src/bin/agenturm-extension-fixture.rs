//! Process-boundary fixture used only by agenturm-runtime integration tests.

#![forbid(unsafe_code)]

use agenturm_protocol::{read_frame, write_frame};
use serde_json::{Value, json};
use std::io::{BufReader, BufWriter, Write, stdin, stdout};
use std::process::{Command, Stdio};
use std::thread;
use std::time::Duration;

fn main() {
    if run().is_err() {
        std::process::exit(2);
    }
}

fn run() -> Result<(), Box<dyn std::error::Error>> {
    if std::env::args().nth(1).as_deref() == Some("--fixture-descendant") {
        loop {
            thread::sleep(Duration::from_secs(1));
        }
    }
    let stdin = stdin();
    let stdout = stdout();
    let mut reader = BufReader::new(stdin.lock());
    let mut writer = BufWriter::new(stdout.lock());
    let hello: Value =
        serde_json::from_str(&read_frame(&mut reader, 4 * 1024 * 1024)?.ok_or("missing hello")?)?;
    let plugin_id = hello["plugin_id"].as_str().ok_or("missing ID")?.to_owned();
    let request_id = hello["request_id"]
        .as_str()
        .ok_or("missing request")?
        .to_owned();
    write_frame(
        &mut writer,
        &json!({
            "type":"hello_ack",
            "protocol":1,
            "plugin_id":if plugin_id == "example.bad-handshake" { "example.wrong" } else { &plugin_id },
            "request_id":if plugin_id == "example.bad-handshake" { "wrong-request" } else { &request_id },
            "features":["capability/1", "ui-contribution/1"]
        })
        .to_string(),
        4 * 1024 * 1024,
    )?;

    if plugin_id == "example.no-read" {
        loop {
            thread::sleep(Duration::from_secs(1));
        }
    }

    if plugin_id == "example.hanging" {
        loop {
            thread::sleep(Duration::from_secs(1));
        }
    }

    if plugin_id == "example.descendant" {
        Command::new(std::env::current_exe()?)
            .arg("--fixture-descendant")
            .stdin(Stdio::inherit())
            .stdout(Stdio::inherit())
            .stderr(Stdio::null())
            .spawn()?;
    }

    let request: Value = serde_json::from_str(
        &read_frame(&mut reader, 4 * 1024 * 1024)?.ok_or("missing evaluate")?,
    )?;
    if request["type"] != "evaluate"
        || request["plugin_id"] != plugin_id
        || request["request_id"] != request_id
    {
        return Err("request identity mismatch".into());
    }
    if plugin_id == "example.oversized" {
        writer.write_all(&u32::MAX.to_be_bytes())?;
        writer.flush()?;
        return Ok(());
    }
    let observations = request["observations"]
        .as_array()
        .ok_or("observations absent")?;
    for observation in observations {
        let object = observation
            .as_object()
            .ok_or("observation must be object")?;
        if object.keys().any(|key| {
            !matches!(
                key.as_str(),
                "source_id"
                    | "observed_at_unix_ns"
                    | "kind"
                    | "input_tokens"
                    | "output_tokens"
                    | "cache_read_tokens"
                    | "cache_write_tokens"
                    | "total_tokens"
                    | "role"
                    | "tool_category"
                    | "tool_result"
                    | "check_category"
                    | "check_result"
                    | "state"
                    | "reason_code"
                    | "diagnostic_code"
                    | "severity"
            )
        }) {
            return Err("unexpected observation field".into());
        }
    }
    let response = json!({
        "type":"evaluate_result",
        "protocol":1,
        "plugin_id":plugin_id,
        "request_id":request_id,
        "metrics":[{"label":"fixture metric", "value":observations.len().to_string()}],
        "pane":{"title":"Fixture pane", "rows":[{"label":"rows", "value":observations.len().to_string()}]}
    })
    .to_string();
    write_frame(&mut writer, &response, 4 * 1024 * 1024)?;
    if plugin_id == "example.nonzero" {
        return Err("injected nonzero exit".into());
    }
    if plugin_id == "example.extra" {
        write_frame(&mut writer, &response, 4 * 1024 * 1024)?;
    }
    Ok(())
}

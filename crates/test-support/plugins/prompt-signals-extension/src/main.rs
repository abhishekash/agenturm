//! Minimal independent Rust executable for the Agenturm process extension API.

#![forbid(unsafe_code)]

use agenturm_protocol::{MAX_FRAME_BYTES, read_frame, write_frame};
use serde::{Deserialize, Serialize};
use std::io::{BufReader, BufWriter, stdin, stdout};

const PLUGIN_ID: &str = "example.prompt-signals";

fn main() {
    if run().is_err() {
        // Do not write private request data or internal error text to stderr.
        std::process::exit(2);
    }
}

fn run() -> Result<(), Box<dyn std::error::Error>> {
    let mut reader = BufReader::new(stdin().lock());
    let mut writer = BufWriter::new(stdout().lock());

    let hello: Hello = read_message(&mut reader)?;
    if hello.frame_type != "hello" || hello.protocol != 1 || hello.plugin_id != PLUGIN_ID {
        return Err("invalid hello".into());
    }
    write_message(
        &mut writer,
        &HelloAck {
            frame_type: "hello_ack",
            protocol: 1,
            plugin_id: PLUGIN_ID,
            request_id: &hello.request_id,
            features: ["capability/1", "ui-contribution/1"],
        },
    )?;

    let request: EvaluateRequest = read_message(&mut reader)?;
    if request.frame_type != "evaluate"
        || request.protocol != 1
        || request.plugin_id != PLUGIN_ID
        || request.request_id != hello.request_id
        || request.observations.len() > 500
        || !matches!(
            request.selection_scope.as_str(),
            "latest" | "filtered" | "continuation"
        )
        || request.observation_count < request.observations.len() as u64
        || (!request.truncated && request.observation_count != request.observations.len() as u64)
    {
        return Err("invalid evaluation request".into());
    }
    let _snapshot_id = request.snapshot_id;

    let mut user_messages = 0u64;
    let mut assistant_messages = 0u64;
    let mut input_tokens = 0u64;
    let mut output_tokens = 0u64;
    for observation in &request.observations {
        let timestamp = observation
            .observed_at_unix_ns
            .parse::<u64>()
            .map_err(|_| "invalid observation timestamp")?;
        if timestamp.to_string() != observation.observed_at_unix_ns
            || observation.source_id.is_empty()
        {
            return Err("invalid observation envelope".into());
        }
        let _other_granted_fields = (
            observation.cache_read_tokens,
            observation.cache_write_tokens,
            observation.total_tokens,
            observation.tool_category.as_deref(),
            observation.tool_result.as_deref(),
            observation.check_category.as_deref(),
            observation.check_result.as_deref(),
            observation.state.as_deref(),
            observation.reason_code.as_deref(),
            observation.diagnostic_code.as_deref(),
            observation.severity.as_deref(),
        );
        if observation.kind == "message" {
            match observation.role.as_deref() {
                Some("user") => user_messages += 1,
                Some("assistant") => assistant_messages += 1,
                _ => {}
            }
        }
        if observation.kind == "usage" {
            input_tokens = input_tokens.saturating_add(observation.input_tokens.unwrap_or(0));
            output_tokens = output_tokens.saturating_add(observation.output_tokens.unwrap_or(0));
        }
    }

    let rows = vec![
        Metric {
            label: "User messages".into(),
            value: user_messages.to_string(),
        },
        Metric {
            label: "Assistant messages".into(),
            value: assistant_messages.to_string(),
        },
        Metric {
            label: "Input tokens in page".into(),
            value: input_tokens.to_string(),
        },
        Metric {
            label: "Output tokens in page".into(),
            value: output_tokens.to_string(),
        },
        Metric {
            label: "Page completeness".into(),
            value: if request.truncated {
                format!("partial · {} envelope rows", request.observation_count)
            } else {
                format!("complete · {} envelope rows", request.observation_count)
            },
        },
    ];
    write_message(
        &mut writer,
        &EvaluateResult {
            frame_type: "evaluate_result",
            protocol: 1,
            plugin_id: PLUGIN_ID,
            request_id: &request.request_id,
            metrics: &rows,
            pane: Some(Pane {
                title: "Prompt signals".into(),
                rows: rows.clone(),
            }),
        },
    )?;
    Ok(())
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Hello {
    #[serde(rename = "type")]
    frame_type: String,
    protocol: u32,
    plugin_id: String,
    request_id: String,
}

#[derive(Serialize)]
struct HelloAck<'a> {
    #[serde(rename = "type")]
    frame_type: &'static str,
    protocol: u32,
    plugin_id: &'static str,
    request_id: &'a str,
    features: [&'static str; 2],
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct EvaluateRequest {
    #[serde(rename = "type")]
    frame_type: String,
    protocol: u32,
    plugin_id: String,
    request_id: String,
    snapshot_id: u64,
    observation_count: u64,
    truncated: bool,
    selection_scope: String,
    observations: Vec<Observation>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Observation {
    source_id: String,
    observed_at_unix_ns: String,
    kind: String,
    input_tokens: Option<u64>,
    output_tokens: Option<u64>,
    cache_read_tokens: Option<u64>,
    cache_write_tokens: Option<u64>,
    total_tokens: Option<u64>,
    role: Option<String>,
    tool_category: Option<String>,
    tool_result: Option<String>,
    check_category: Option<String>,
    check_result: Option<String>,
    state: Option<String>,
    reason_code: Option<String>,
    diagnostic_code: Option<String>,
    severity: Option<String>,
}

#[derive(Serialize)]
struct EvaluateResult<'a> {
    #[serde(rename = "type")]
    frame_type: &'static str,
    protocol: u32,
    plugin_id: &'static str,
    request_id: &'a str,
    metrics: &'a [Metric],
    pane: Option<Pane>,
}

#[derive(Clone, Serialize)]
struct Metric {
    label: String,
    value: String,
}

#[derive(Serialize)]
struct Pane {
    title: String,
    rows: Vec<Metric>,
}

fn read_message<T: for<'de> Deserialize<'de>>(
    reader: &mut impl std::io::Read,
) -> Result<T, Box<dyn std::error::Error>> {
    let frame = read_frame(reader, MAX_FRAME_BYTES)?.ok_or("unexpected EOF")?;
    Ok(serde_json::from_str(&frame)?)
}

fn write_message(
    writer: &mut impl std::io::Write,
    message: &impl Serialize,
) -> Result<(), Box<dyn std::error::Error>> {
    write_frame(writer, &serde_json::to_string(message)?, MAX_FRAME_BYTES)?;
    Ok(())
}

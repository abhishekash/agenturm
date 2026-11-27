use agenturm_protocol::observation::{
    ObservationBatch, ObservationFact, ObservationRecord, SessionIdentity, Usage,
};
use agenturm_protocol::{MAX_FRAME_BYTES, read_frame, write_frame};
use serde_json::{Value, json};
use std::error::Error;
use std::io::{self, BufReader};

const PLUGIN_ID: &str = "fixture.standalone-harness";
const NEXT_CURSOR: &str = "standalone-record-1";

fn run() -> Result<(), Box<dyn Error>> {
    let source = std::fs::read_to_string(
        std::env::var_os("AGENTURM_PLUGIN_SOURCE_ROOT").ok_or("source root missing")?,
    )?;
    let source = source.trim();
    let mut input = BufReader::new(io::stdin().lock());
    let mut output = io::stdout().lock();
    let mut pending = false;
    while let Some(frame) = read_frame(&mut input, MAX_FRAME_BYTES)? {
        let request: Value = serde_json::from_str(&frame)?;
        let source_id = request["source_id"].as_str().unwrap_or_default();
        let response = match request["type"].as_str() {
            Some("hello") if request["plugin_id"] == PLUGIN_ID => json!({
                "type": "hello_ack",
                "protocol": 1,
                "plugin_id": PLUGIN_ID,
                "features": ["source-adapter/1", "configured-root/1"],
            }),
            Some("poll") if !pending => {
                if source == "crash" {
                    std::process::exit(17);
                }
                let records = match request["cursor"].as_str() {
                    Some("") => {
                        let tokens = match source {
                            "malformed" => 1,
                            input => input
                                .strip_prefix("tokens=")
                                .ok_or("missing native token count")?
                                .parse()?,
                        };
                        vec![ObservationRecord {
                            event_id: "standalone-event-1".into(),
                            revision: 0,
                            time_ns: "9007199254740993".into(),
                            identity: Some(SessionIdentity::session("standalone-session")),
                            fact: ObservationFact::Usage(Usage {
                                input_tokens: Some(tokens),
                                ..Usage::default()
                            }),
                        }]
                    }
                    Some(NEXT_CURSOR) => Vec::new(),
                    _ => return Err("unexpected poll cursor".into()),
                };
                let batch = ObservationBatch::new(source_id, NEXT_CURSOR, records);
                let mut batch = serde_json::to_value(batch)?;
                if source == "malformed" {
                    let records = batch["records"].as_array_mut().ok_or("records missing")?;
                    let mut invalid = records.first().ok_or("first record missing")?.clone();
                    invalid["event_id"] = json!("standalone-event-2");
                    invalid["payload"]["input_tokens"] = json!(-1);
                    records.push(invalid);
                }
                pending = true;
                batch
            }
            Some("ack") if pending && request["cursor"] == NEXT_CURSOR => {
                pending = false;
                continue;
            }
            Some("stop") => return Ok(()),
            _ => return Err("unexpected control message".into()),
        };
        write_frame(
            &mut output,
            &serde_json::to_string(&response)?,
            MAX_FRAME_BYTES,
        )?;
    }
    Ok(())
}

fn main() {
    if run().is_err() {
        std::process::exit(2);
    }
}

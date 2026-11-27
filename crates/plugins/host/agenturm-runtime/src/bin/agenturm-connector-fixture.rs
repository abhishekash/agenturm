//! Process fixture for connector conformance tests. Uses only temporary files.

use agenturm_connector_api::{DeliveryLookup, DeliveryReceipt, InboundBatch, InboundRecord};
use agenturm_runtime::connector::{ConnectorRequest, ConnectorResponse, Operation, Outcome};
use serde_json::json;
use std::fs;
use std::path::Path;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let request: ConnectorRequest = serde_json::from_str(
        &agenturm_protocol::read_frame(&mut std::io::stdin().lock(), 4 * 1024 * 1024)?
            .ok_or("missing request")?,
    )?;
    if !request.target.starts_with("connector-fixture-")
        || !request
            .target
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-')
    {
        return Err("unexpected fixture directory".into());
    }
    let directory = std::env::temp_dir().join(&request.target);
    let mode = fs::read_to_string(directory.join("mode"))?;
    fs::write(directory.join("launched"), b"yes")?;
    let outcome = match request.operation {
        Operation::Poll { checkpoint, .. } => Outcome::Batch {
            batch: InboundBatch {
                next_checkpoint: "one".into(),
                records: if checkpoint.is_empty() {
                    vec![InboundRecord {
                        record_id: "event-one".into(),
                        revision: 0,
                        observed_at_unix_ns: 1,
                        payload: json!({"session_id":"fixture-session","state":"completed"}),
                    }]
                } else {
                    Vec::new()
                },
            },
        },
        Operation::Ack { checkpoint } => {
            if mode == "recover-ack" && !directory.join("first-ack").exists() {
                fs::write(directory.join("first-ack"), b"yes")?;
                return Err("fixture loses first ack".into());
            }
            fs::write(directory.join("acked"), &checkpoint)?;
            Outcome::Ack { checkpoint }
        }
        Operation::Deliver { delivery } => {
            let ledger = directory.join("effect");
            let attempts = count(&directory.join("attempts")) + 1;
            fs::write(directory.join("attempts"), attempts.to_string())?;
            if mode == "missing-then-retry" && attempts == 1 || mode == "always-unknown" {
                return Err("fixture loses first request".into());
            }
            if ledger.exists() {
                return Err("duplicate delivery effect".into());
            }
            fs::write(ledger, &delivery.request_sha256)?;
            if mode == "accepted-then-timeout" {
                std::thread::sleep(std::time::Duration::from_secs(5));
                return Err("fixture simulates accept followed by timeout".into());
            }
            Outcome::Delivered {
                receipt: DeliveryReceipt {
                    destination_id: delivery.destination_id,
                    idempotency_key: delivery.idempotency_key,
                    request_sha256: delivery.request_sha256,
                    receipt_id: "receipt-one".into(),
                },
            }
        }
        Operation::Lookup { delivery } => {
            let ledger = directory.join("effect");
            let lookups = count(&directory.join("lookups")) + 1;
            fs::write(directory.join("lookups"), lookups.to_string())?;
            let result = if mode == "always-unknown" {
                DeliveryLookup::Unknown {
                    destination_id: delivery.destination_id,
                    idempotency_key: delivery.idempotency_key,
                    request_sha256: delivery.request_sha256,
                }
            } else if ledger.exists() {
                if fs::read_to_string(ledger)? != delivery.request_sha256 {
                    return Err("fixture sees mismatched digest".into());
                }
                DeliveryLookup::Accepted {
                    destination_id: delivery.destination_id,
                    idempotency_key: delivery.idempotency_key,
                    request_sha256: delivery.request_sha256,
                    receipt_id: "receipt-one".into(),
                }
            } else {
                DeliveryLookup::ConfirmedMissing {
                    destination_id: delivery.destination_id,
                    idempotency_key: delivery.idempotency_key,
                    request_sha256: delivery.request_sha256,
                }
            };
            Outcome::Lookup { result }
        }
    };
    let response = ConnectorResponse {
        protocol: request.protocol,
        descriptor: request.descriptor,
        target: request.target,
        request_id: request.request_id,
        outcome,
    };
    agenturm_protocol::write_frame(
        &mut std::io::stdout().lock(),
        &serde_json::to_string(&response)?,
        4 * 1024 * 1024,
    )?;
    Ok(())
}

fn count(path: &Path) -> usize {
    fs::read_to_string(path)
        .ok()
        .and_then(|value| value.parse().ok())
        .unwrap_or(0)
}

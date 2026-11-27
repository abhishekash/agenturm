//! Explicit command entry point for trusted pinned connector processes.

use agenturm_connector_api::{ConnectorDirection, OutboundDelivery, request_sha256};
use agenturm_runtime::connector::{ConnectorRuntime, ConnectorSpec, DeliveryState};
use serde_json::json;
use std::io::Read;
use std::path::Path;
use std::process::ExitCode;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

const USAGE: &str = "usage: agenturm-connector <ledger.sqlite> <connector.json> grant|revoke|collect <history.sqlite>|enqueue <delivery.json>|enqueue-status <delivery-id> <key> <state>|drive <delivery-id>|watch [history.sqlite]";

fn main() -> ExitCode {
    let args = std::env::args().skip(1).collect::<Vec<_>>();
    if args.as_slice() == ["--help"] {
        println!("{USAGE}");
        return ExitCode::SUCCESS;
    }
    match execute(&args) {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("{error}");
            ExitCode::FAILURE
        }
    }
}

fn execute(args: &[String]) -> Result<(), &'static str> {
    let [ledger, config, action, rest @ ..] = args else {
        return Err(USAGE);
    };
    let spec: ConnectorSpec = read_json(config)?;
    let mut runtime = ConnectorRuntime::open(Path::new(ledger))?;
    match (action.as_str(), rest) {
        ("grant", []) => runtime.grant(&spec),
        ("revoke", []) => {
            println!("{}", runtime.revoke(&spec)?);
            Ok(())
        }
        ("collect", [history]) => {
            let mut history = agenturm_store::Store::open_collector(history)
                .map_err(|_| "connector_history_error")?;
            println!("{}", runtime.collect(&spec, &mut history)?);
            Ok(())
        }
        ("enqueue", [request]) => runtime.enqueue(&spec, &read_json(request)?),
        ("enqueue-status", [id, key, state]) => {
            let payload = serde_json::Value::String(state.clone());
            let request_sha256 = request_sha256(&spec.target, "write_status", &payload)
                .map_err(|_| "connector_invalid_delivery")?;
            runtime.enqueue(
                &spec,
                &OutboundDelivery {
                    delivery_id: id.clone(),
                    idempotency_key: key.clone(),
                    destination_id: spec.target.clone(),
                    action: "write_status".into(),
                    payload,
                    request_sha256,
                },
            )
        }
        ("drive", [id]) => {
            let result = runtime.drive(&spec, id, now_ms()?)?;
            println!(
                "{}",
                serde_json::to_string(&result).map_err(|_| "connector_protocol_error")?
            );
            Ok(())
        }
        ("watch", [history]) if spec.descriptor.direction == ConnectorDirection::Inbound => {
            runtime.check_grant(&spec)?;
            let history = agenturm_store::Store::open_collector(history)
                .map_err(|_| "connector_history_error")?;
            watch(&spec, &mut runtime, Some(history))
        }
        ("watch", []) if spec.descriptor.direction == ConnectorDirection::Outbound => {
            runtime.check_grant(&spec)?;
            watch(&spec, &mut runtime, None)
        }
        _ => Err(USAGE),
    }
}

fn now_ms() -> Result<i64, &'static str> {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .ok()
        .and_then(|duration| i64::try_from(duration.as_millis()).ok())
        .ok_or("connector_clock_error")
}

fn watch(
    spec: &ConnectorSpec,
    runtime: &mut ConnectorRuntime,
    mut history: Option<agenturm_store::Store>,
) -> Result<(), &'static str> {
    let stop = Stop::new()?;
    report(json!({"event": "watch_started", "direction": spec.descriptor.direction}))?;
    let mut failures = 0u32;
    while !stop.requested() {
        let result = match &mut history {
            Some(history) => runtime.collect(spec, history).and_then(|inserted| {
                if inserted > 0 {
                    report(json!({"event": "inbound_commit", "inserted": inserted}))?;
                }
                Ok(())
            }),
            None => drive_due(runtime, spec, &stop),
        };
        match result {
            Ok(()) => failures = 0,
            Err(
                error @ ("connector_permission_denied"
                | "connector_invalid_config"
                | "connector_output_error"),
            ) => {
                return Err(error);
            }
            Err(code) => {
                failures = failures.saturating_add(1);
                report(json!({"event": "watch_error", "code": code}))?;
            }
        }
        let next = Instant::now() + Duration::from_secs(1 << failures.min(4));
        while !stop.requested() && Instant::now() < next {
            std::thread::sleep(
                next.saturating_duration_since(Instant::now())
                    .min(Duration::from_millis(50)),
            );
        }
    }
    report(json!({"event": "watch_stopped"}))
}

fn drive_due(
    runtime: &mut ConnectorRuntime,
    spec: &ConnectorSpec,
    stop: &Stop,
) -> Result<(), &'static str> {
    for id in runtime.due_deliveries(spec, now_ms()?)? {
        if stop.requested() {
            break;
        }
        let state = match runtime.drive(spec, &id, now_ms()?)? {
            DeliveryState::Pending => "pending",
            DeliveryState::Waiting => continue,
            DeliveryState::Accepted { .. } => "accepted",
            DeliveryState::Unknown => "unknown",
            DeliveryState::DeadLetter => "dead_letter",
        };
        report(json!({"event": "delivery_progress", "state": state}))?;
    }
    Ok(())
}

fn report(event: serde_json::Value) -> Result<(), &'static str> {
    use std::io::Write;
    let mut output = std::io::stdout().lock();
    writeln!(output, "{event}")
        .and_then(|()| output.flush())
        .map_err(|_| "connector_output_error")
}

struct Stop {
    flag: Arc<AtomicBool>,
    signals: Vec<signal_hook::SigId>,
}

impl Stop {
    fn new() -> Result<Self, &'static str> {
        let mut stop = Self {
            flag: Arc::new(AtomicBool::new(false)),
            signals: Vec::new(),
        };
        let signals = [signal_hook::consts::SIGINT, signal_hook::consts::SIGTERM];
        for signal in signals {
            stop.signals.push(
                signal_hook::flag::register(signal, Arc::clone(&stop.flag))
                    .map_err(|_| "connector_signal_error")?,
            );
        }
        Ok(stop)
    }

    fn requested(&self) -> bool {
        self.flag.load(Ordering::Relaxed)
    }
}

impl Drop for Stop {
    fn drop(&mut self) {
        for signal in self.signals.drain(..) {
            signal_hook::low_level::unregister(signal);
        }
    }
}

fn read_json<T: serde::de::DeserializeOwned>(path: &str) -> Result<T, &'static str> {
    const LIMIT: u64 = 1024 * 1024;
    let mut bytes = Vec::new();
    let file = std::fs::File::open(path).map_err(|_| "connector_invalid_input_file")?;
    if !file
        .metadata()
        .map_err(|_| "connector_invalid_input_file")?
        .is_file()
    {
        return Err("connector_invalid_input_file");
    }
    file.take(LIMIT + 1)
        .read_to_end(&mut bytes)
        .map_err(|_| "connector_invalid_input_file")?;
    if bytes.len() as u64 > LIMIT {
        return Err("connector_invalid_input_file");
    }
    serde_json::from_slice(&bytes).map_err(|_| "connector_invalid_input_file")
}

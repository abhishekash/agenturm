//! Run a program with managed or explicitly selected OTLP delivery.

#[cfg(target_os = "linux")]
mod endpoint;

use crate::trace_import::{commit, commit_payloads, parse_source_id};
use crate::trace_receiver::{ProducerToken, TraceReceiver, parse_payload_scope};
use agenturm_kernel::{ExtensionId, Observation};
use agenturm_store::{TelemetryBinding, TelemetryPayloadRecord};
use std::ffi::{OsStr, OsString};
use std::net::SocketAddr;
use std::path::PathBuf;
use std::process::{Child, Command, ExitCode, ExitStatus, Stdio};
use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

const USAGE: &str = concat!(
    "agenturm run --db <path> [--source-id otlp.<id>] [--payload-scope <standalone|harness-id>] ",
    "-- <program> [args...]\n",
    "       agenturm run --endpoint http://<loopback-IP:port> --token-file <path> ",
    "-- <program> [args...]",
);
#[cfg(target_os = "linux")]
const BROKER_USAGE: &str = "agenturm broker run --broker-config <json> --source-id otlp.<id> [--payload-scope <standalone|harness-id>] -- <program> [args...]";
const SIGNAL_GRACE: Duration = Duration::from_secs(2);

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct TraceRunOptions {
    destination: TraceDestination,
    invocation: TraceInvocation,
}

#[derive(Clone, Debug, Eq, PartialEq)]
enum TraceDestination {
    Database(PathBuf),
    #[cfg(target_os = "linux")]
    Endpoint {
        address: SocketAddr,
        token_file: PathBuf,
    },
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(super) struct TraceInvocation {
    pub source: ExtensionId,
    pub payload_binding: Option<TelemetryBinding>,
    program: OsString,
    arguments: Vec<OsString>,
}

pub(super) fn parse_trace_run(args: &[OsString]) -> Result<TraceRunOptions, String> {
    let (invocation, destination) = parse_invocation(args, true, USAGE)?;
    Ok(TraceRunOptions {
        destination: destination.ok_or(USAGE)?,
        invocation,
    })
}

#[cfg(target_os = "linux")]
pub(super) fn parse_broker_run(args: &[OsString]) -> Result<TraceInvocation, String> {
    parse_invocation(args, false, BROKER_USAGE).map(|(invocation, _)| invocation)
}

fn parse_invocation(
    args: &[OsString],
    local: bool,
    usage: &str,
) -> Result<(TraceInvocation, Option<TraceDestination>), String> {
    let delimiter = args.iter().position(|arg| arg == "--").ok_or(usage)?;
    let (program, arguments) = args[delimiter + 1..].split_first().ok_or(usage)?;
    if program.is_empty() {
        return Err(usage.into());
    }
    let mut flags = args[..delimiter].iter();
    let (mut db, mut source) = (None, None);
    #[cfg(target_os = "linux")]
    let (mut endpoint, mut token_file) = (None, None);
    let mut payload_scope = None;
    while let Some(flag) = flags.next() {
        let value = flags.next().ok_or(usage)?;
        if value.is_empty() || value.as_encoded_bytes().starts_with(b"--") {
            return Err(usage.into());
        }
        match flag.to_str() {
            Some("--db") if local && db.is_none() && value != "-" && value != ":memory:" => {
                db = Some(PathBuf::from(value))
            }
            #[cfg(target_os = "linux")]
            Some("--endpoint") if local && endpoint.is_none() => {
                endpoint = Some(endpoint::parse(value.to_str().ok_or(usage)?)?);
            }
            #[cfg(target_os = "linux")]
            Some("--token-file") if local && token_file.is_none() => {
                token_file = Some(PathBuf::from(value));
            }
            Some("--source-id") if source.is_none() => {
                source = Some(parse_source_id(value.to_str().ok_or(usage)?)?)
            }
            Some("--payload-scope") if payload_scope.is_none() => {
                payload_scope = Some(value.to_str().ok_or(usage)?)
            }
            _ => return Err(usage.into()),
        }
    }
    let destination = db.map(TraceDestination::Database);
    #[cfg(target_os = "linux")]
    let destination = match (endpoint, token_file) {
        (Some(address), Some(token_file)) => {
            if destination.is_some() || source.is_some() || payload_scope.is_some() {
                return Err(
                    "--endpoint cannot be combined with --db, --source-id or --payload-scope"
                        .into(),
                );
            }
            Some(TraceDestination::Endpoint {
                address,
                token_file,
            })
        }
        (None, None) => destination,
        _ => return Err("--endpoint and --token-file are required together".into()),
    };
    let source = match source {
        Some(source) => source,
        None if local => parse_source_id("otlp.run").expect("valid default launcher source"),
        #[cfg(target_os = "linux")]
        None if endpoint.is_some() => {
            parse_source_id("otlp.run").expect("valid default launcher source")
        }
        None => return Err(usage.into()),
    };
    let payload_binding = payload_scope
        .map(|scope| parse_payload_scope(&source, scope))
        .transpose()?;
    Ok((
        TraceInvocation {
            source,
            payload_binding,
            program: program.clone(),
            arguments: arguments.to_vec(),
        },
        destination,
    ))
}

pub fn execute_trace_run(options: &TraceRunOptions) -> Result<ExitCode, String> {
    let db = match &options.destination {
        TraceDestination::Database(db) => db,
        #[cfg(target_os = "linux")]
        TraceDestination::Endpoint {
            address,
            token_file,
        } => {
            let token = ProducerToken::from_file(token_file)?;
            endpoint::verify(*address, &token)?;
            return supervise_program(
                &options.invocation,
                *address,
                Some(&token),
                None,
                RunSignals::new()?,
            )
            .map(exit_code);
        }
    };
    // Fail before spawning a program if the observation database cannot
    // accept traces, and release the writer before the receiver starts.
    drop(
        agenturm_store::Store::open_collector(db)
            .map_err(|error| format!("open trace history: {error}"))?,
    );
    let db = db.clone();
    let payload = options.invocation.payload_binding.clone();
    #[cfg(target_os = "linux")]
    let token = if payload.is_some() {
        Some(ProducerToken::generate()?)
    } else {
        None
    };
    #[cfg(not(target_os = "linux"))]
    let token = if payload.is_some() {
        return Err("Private trace capture requires Linux producer authentication".into());
    } else {
        None
    };
    execute_with_maintenance(
        &options.invocation,
        token,
        Some(db.clone()),
        move |source, rows, payloads| match &payload {
            Some(binding) => commit_payloads(&db, binding, &rows, &payloads).map(|_| ()),
            None => commit(&db, source, &rows).map(|_| ()),
        },
    )
}

pub(super) fn execute_with_commit(
    invocation: &TraceInvocation,
    token: Option<ProducerToken>,
    commit: impl FnMut(
        &ExtensionId,
        Vec<Observation>,
        Vec<TelemetryPayloadRecord>,
    ) -> Result<(), String>
    + Send
    + 'static,
) -> Result<ExitCode, String> {
    execute_with_maintenance(invocation, token, None, commit)
}

fn execute_with_maintenance(
    invocation: &TraceInvocation,
    token: Option<ProducerToken>,
    maintenance_db: Option<PathBuf>,
    commit: impl FnMut(
        &ExtensionId,
        Vec<Observation>,
        Vec<TelemetryPayloadRecord>,
    ) -> Result<(), String>
    + Send
    + 'static,
) -> Result<ExitCode, String> {
    let signals = RunSignals::new()?;
    let launch_session = launch_session()?;
    let receiver = TraceReceiver::bind_ephemeral(
        invocation.source.clone(),
        launch_session.clone(),
        token.clone(),
        invocation.payload_binding.clone(),
    )?
    .with_local_maintenance(maintenance_db);
    let address = receiver.address();
    let receiver = ReceiverTask::start(receiver, commit)?;
    let status = supervise_program(
        invocation,
        address,
        token.as_ref(),
        Some(&receiver),
        signals,
    );
    let receiver_result = receiver.finish();
    let status = status?;
    receiver_result?;
    Ok(exit_code(status))
}

fn launch_session() -> Result<String, String> {
    let launch_time = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_err(|_| "system time is outside the launcher range")?
        .as_nanos();
    Ok(format!("launch-{}-{launch_time}", std::process::id()))
}

fn supervise_program(
    invocation: &TraceInvocation,
    address: SocketAddr,
    token: Option<&ProducerToken>,
    receiver: Option<&ReceiverTask>,
    mut signals: RunSignals,
) -> Result<ExitStatus, String> {
    let mut command = traced_command(invocation, address, token);
    let isolated = configure_process_group(&mut command);
    let mut child = command
        .spawn()
        .map_err(|error| format!("launch program: {error}"))?;

    let status = wait_for_child(&mut child, isolated, receiver, &mut signals);
    if status.is_err() {
        force_stop(&mut child, isolated);
        let _ = child.wait();
    }
    status
}

fn traced_command(
    invocation: &TraceInvocation,
    address: SocketAddr,
    token: Option<&ProducerToken>,
) -> Command {
    let base = format!("http://{address}");
    let endpoint = format!("{base}/v1/traces");
    let mut child = Command::new(&invocation.program);
    child.args(&invocation.arguments);
    child
        .stdin(Stdio::inherit())
        .stdout(Stdio::inherit())
        .stderr(Stdio::inherit());
    for (key, _) in std::env::vars_os() {
        if key.as_encoded_bytes().starts_with(b"OTEL_EXPORTER_OTLP")
            || matches!(
                key.to_str(),
                Some("OTEL_CONFIG_FILE" | "OTEL_EXPERIMENTAL_CONFIG_FILE")
            )
        {
            child.env_remove(key);
        }
    }
    child
        .env("OTEL_EXPORTER_OTLP_ENDPOINT", &base)
        .env("OTEL_EXPORTER_OTLP_TRACES_ENDPOINT", endpoint)
        .env("OTEL_EXPORTER_OTLP_PROTOCOL", "http/protobuf")
        .env("OTEL_EXPORTER_OTLP_TRACES_PROTOCOL", "http/protobuf")
        .env("OTEL_EXPORTER_OTLP_COMPRESSION", "none")
        .env("OTEL_EXPORTER_OTLP_TRACES_COMPRESSION", "none")
        .env("OTEL_SDK_DISABLED", "false")
        .env("OTEL_TRACES_EXPORTER", "otlp")
        .env("OTEL_METRICS_EXPORTER", "otlp")
        .env(
            "OTEL_EXPORTER_OTLP_METRICS_ENDPOINT",
            format!("{base}/v1/metrics"),
        )
        .env("OTEL_EXPORTER_OTLP_METRICS_PROTOCOL", "http/protobuf")
        .env("OTEL_EXPORTER_OTLP_METRICS_COMPRESSION", "none")
        .env("OTEL_LOGS_EXPORTER", "otlp")
        .env(
            "OTEL_EXPORTER_OTLP_LOGS_ENDPOINT",
            format!("{base}/v1/logs"),
        )
        .env("OTEL_EXPORTER_OTLP_LOGS_PROTOCOL", "http/protobuf")
        .env("OTEL_EXPORTER_OTLP_LOGS_COMPRESSION", "none");
    let no_proxy = loopback_exclusion(address.ip());
    child.env("NO_PROXY", &no_proxy).env("no_proxy", &no_proxy);
    if let Some(token) = token {
        child
            .env("OTEL_EXPORTER_OTLP_TRACES_HEADERS", token.otel_header())
            .env("OTEL_EXPORTER_OTLP_METRICS_HEADERS", token.otel_header())
            .env("OTEL_EXPORTER_OTLP_LOGS_HEADERS", token.otel_header());
    }
    child
}

fn loopback_exclusion(address: std::net::IpAddr) -> OsString {
    let mut values = OsString::new();
    for name in ["NO_PROXY", "no_proxy"] {
        if let Some(value) = std::env::var_os(name).filter(|value| !value.is_empty()) {
            values.push(value);
            values.push(OsStr::new(","));
        }
    }
    values.push("127.0.0.1,localhost,::1");
    let literal = address.to_string();
    if !matches!(literal.as_str(), "127.0.0.1" | "::1") {
        values.push(",");
        values.push(literal);
    }
    values
}

#[cfg(unix)]
fn configure_process_group(command: &mut Command) -> bool {
    use std::io::IsTerminal;
    use std::os::unix::process::CommandExt;
    // Interactive children must share the foreground terminal group so
    // reads, interrupts, and shell job control continue to work normally.
    let isolated = !(std::io::stdin().is_terminal()
        || std::io::stdout().is_terminal()
        || std::io::stderr().is_terminal());
    if isolated {
        command.process_group(0);
    }
    isolated
}

#[cfg(not(unix))]
fn configure_process_group(_: &mut Command) -> bool {
    false
}

fn wait_for_child(
    child: &mut Child,
    isolated: bool,
    receiver: Option<&ReceiverTask>,
    signals: &mut RunSignals,
) -> Result<ExitStatus, String> {
    let mut grace = None;
    loop {
        if let Some(status) = child.try_wait().map_err(|error| error.to_string())? {
            return Ok(status);
        }
        if receiver.is_some_and(ReceiverTask::finished) {
            return Err("trace receiver stopped while program was running".into());
        }
        if grace.is_none()
            && let Some((signal, from_terminal_group)) = signals.next()
        {
            // The terminal already delivered a kernel-generated signal
            // to an interactive child sharing our foreground group.
            if isolated || !from_terminal_group {
                forward(child, isolated, signal);
            }
            grace = Some(Instant::now() + SIGNAL_GRACE);
        }
        if grace.is_some_and(|end| Instant::now() >= end) {
            force_stop(child, isolated);
            return child.wait().map_err(|error| error.to_string());
        }
        thread::sleep(Duration::from_millis(25));
    }
}

#[cfg(unix)]
fn forward(child: &mut Child, isolated: bool, signal: usize) {
    // SAFETY: the PID comes from the running direct child; if isolated,
    // the child was spawned as leader of its own process group.
    let pid = child.id() as libc::pid_t;
    let target = if isolated { -pid } else { pid };
    unsafe { libc::kill(target, signal as libc::c_int) };
}

#[cfg(not(unix))]
fn forward(child: &mut Child, _: bool, _: usize) {
    let _ = child.kill();
}

fn force_stop(child: &mut Child, isolated: bool) {
    #[cfg(unix)]
    forward(child, isolated, libc::SIGKILL as usize);
    #[cfg(not(unix))]
    let _ = isolated;
    let _ = child.kill();
}

pub(super) fn exit_code(status: ExitStatus) -> ExitCode {
    if let Some(code) = status.code() {
        return ExitCode::from(code as u8);
    }
    #[cfg(unix)]
    {
        use std::os::unix::process::ExitStatusExt;
        if let Some(signal) = status.signal() {
            return ExitCode::from((128 + signal) as u8);
        }
    }
    ExitCode::FAILURE
}

#[cfg(unix)]
struct RunSignals(
    signal_hook::iterator::SignalsInfo<signal_hook::iterator::exfiltrator::WithOrigin>,
);

#[cfg(unix)]
impl RunSignals {
    fn new() -> Result<Self, String> {
        let signals = [
            signal_hook::consts::SIGINT,
            signal_hook::consts::SIGTERM,
            signal_hook::consts::SIGHUP,
        ];
        signal_hook::iterator::SignalsInfo::new(signals)
            .map(Self)
            .map_err(|error| format!("register launcher signal handler: {error}"))
    }

    fn next(&mut self) -> Option<(usize, bool)> {
        use signal_hook::low_level::siginfo::Cause;
        self.0
            .pending()
            .next()
            .map(|info| (info.signal as usize, info.cause == Cause::Kernel))
    }
}

#[cfg(not(unix))]
struct RunSignals {
    flag: Arc<AtomicBool>,
    handler: signal_hook::SigId,
}

#[cfg(not(unix))]
impl RunSignals {
    fn new() -> Result<Self, String> {
        let flag = Arc::new(AtomicBool::new(false));
        let handler = signal_hook::flag::register(signal_hook::consts::SIGINT, Arc::clone(&flag))
            .map_err(|error| format!("register launcher signal handler: {error}"))?;
        Ok(Self { flag, handler })
    }

    fn next(&mut self) -> Option<(usize, bool)> {
        self.flag
            .swap(false, Ordering::Relaxed)
            .then_some((signal_hook::consts::SIGINT as usize, false))
    }
}

#[cfg(not(unix))]
impl Drop for RunSignals {
    fn drop(&mut self) {
        signal_hook::low_level::unregister(self.handler);
    }
}

struct ReceiverTask {
    stop: Arc<AtomicBool>,
    thread: Option<JoinHandle<Result<(), String>>>,
}

impl ReceiverTask {
    fn start(
        receiver: TraceReceiver,
        commit: impl FnMut(
            &ExtensionId,
            Vec<Observation>,
            Vec<TelemetryPayloadRecord>,
        ) -> Result<(), String>
        + Send
        + 'static,
    ) -> Result<Self, String> {
        let stop = Arc::new(AtomicBool::new(false));
        let receiver_stop = Arc::clone(&stop);
        let thread = thread::Builder::new()
            .name("agenturm-run-traces".into())
            .spawn(move || receiver.serve_with_payloads(&receiver_stop, commit))
            .map_err(|error| format!("start trace receiver: {error}"))?;
        Ok(Self {
            stop,
            thread: Some(thread),
        })
    }

    fn finished(&self) -> bool {
        self.thread
            .as_ref()
            .is_some_and(|thread| thread.is_finished())
    }

    fn finish(mut self) -> Result<(), String> {
        self.stop.store(true, Ordering::Relaxed);
        self.thread
            .take()
            .expect("receiver worker exists")
            .join()
            .map_err(|_| "trace receiver worker failed")?
    }
}

impl Drop for ReceiverTask {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

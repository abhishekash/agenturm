//! Launch an already instrumented process with a local OTLP trace receiver.

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

const USAGE: &str = "agenturm run --db <path> [--source-id otlp.<id>] [--payload-scope <standalone|harness-id>] -- <program> [args...]";
#[cfg(target_os = "linux")]
const BROKER_USAGE: &str = "agenturm broker run --broker-config <json> --source-id otlp.<id> [--payload-scope <standalone|harness-id>] -- <program> [args...]";
const SIGNAL_GRACE: Duration = Duration::from_secs(2);

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct TraceRunOptions {
    db: PathBuf,
    invocation: TraceInvocation,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(super) struct TraceInvocation {
    pub source: ExtensionId,
    pub payload_binding: Option<TelemetryBinding>,
    program: OsString,
    arguments: Vec<OsString>,
}

pub(super) fn parse_trace_run(args: &[OsString]) -> Result<TraceRunOptions, String> {
    let (invocation, db) = parse_invocation(args, true, USAGE)?;
    Ok(TraceRunOptions {
        db: db.ok_or(USAGE)?,
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
) -> Result<(TraceInvocation, Option<PathBuf>), String> {
    let delimiter = args.iter().position(|arg| arg == "--").ok_or(usage)?;
    let (program, arguments) = args[delimiter + 1..].split_first().ok_or(usage)?;
    if program.is_empty() {
        return Err(usage.into());
    }
    let (flags, extras) = args[..delimiter].as_chunks::<2>();
    if !extras.is_empty() {
        return Err(usage.into());
    }
    let (mut db, mut source) = (None, None);
    let mut payload_scope = None;
    for [flag, value] in flags {
        if value.is_empty() || value.as_encoded_bytes().starts_with(b"--") {
            return Err(usage.into());
        }
        match flag.to_str() {
            Some("--db") if local && db.is_none() && value != "-" && value != ":memory:" => {
                db = Some(PathBuf::from(value))
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
    let source = match source {
        Some(source) => source,
        None if local && payload_scope.is_none() => {
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
        db,
    ))
}

pub fn execute_trace_run(options: &TraceRunOptions) -> Result<ExitCode, String> {
    // Fail before spawning a program if the observation database cannot
    // accept traces, and release the writer before the receiver starts.
    drop(
        agenturm_store::Store::open_collector(&options.db)
            .map_err(|error| format!("open trace history: {error}"))?,
    );
    let db = options.db.clone();
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
        Some(options.db.clone()),
        move |source, rows, payloads| match &payload {
            Some(binding) => commit_payloads(&db, binding, &rows, &payloads),
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
    let mut signals = RunSignals::new()?;
    let launch_time = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_err(|_| "system time is outside the launcher range")?
        .as_nanos();
    let launch_session = format!("launch-{}-{launch_time}", std::process::id());
    let receiver = TraceReceiver::bind_ephemeral(
        invocation.source.clone(),
        launch_session,
        token.clone(),
        invocation.payload_binding.clone(),
    )?
    .with_local_maintenance(maintenance_db);
    let mut command = traced_command(invocation, receiver.address(), token.as_ref());
    let isolated = configure_process_group(&mut command);
    let receiver = ReceiverTask::start(receiver, commit)?;
    let mut child = command
        .spawn()
        .map_err(|error| format!("launch program: {error}"))?;

    let status = wait_for_child(&mut child, isolated, &receiver, &mut signals);
    if status.is_err() {
        force_stop(&mut child, isolated);
        let _ = child.wait();
    }
    let receiver_result = receiver.finish();
    let status = status?;
    receiver_result?;
    Ok(exit_code(status))
}

fn traced_command(
    invocation: &TraceInvocation,
    address: SocketAddr,
    token: Option<&ProducerToken>,
) -> Command {
    let base = format!("http://{address}");
    let endpoint = format!("{base}/v1/traces");
    let mut child = Command::new(&invocation.program);
    child
        .args(&invocation.arguments)
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
        .env("OTEL_EXPORTER_OTLP_ENDPOINT", base)
        .env("OTEL_EXPORTER_OTLP_TRACES_ENDPOINT", endpoint)
        .env("OTEL_EXPORTER_OTLP_PROTOCOL", "http/protobuf")
        .env("OTEL_EXPORTER_OTLP_TRACES_PROTOCOL", "http/protobuf")
        .env("OTEL_EXPORTER_OTLP_COMPRESSION", "none")
        .env("OTEL_EXPORTER_OTLP_TRACES_COMPRESSION", "none")
        .env("OTEL_SDK_DISABLED", "false")
        .env("OTEL_TRACES_EXPORTER", "otlp")
        .env("OTEL_METRICS_EXPORTER", "none")
        .env("OTEL_LOGS_EXPORTER", "none");
    let no_proxy = loopback_exclusion();
    child.env("NO_PROXY", &no_proxy).env("no_proxy", &no_proxy);
    if let Some(token) = token {
        child.env("OTEL_EXPORTER_OTLP_TRACES_HEADERS", token.otel_header());
    }
    child
}

fn loopback_exclusion() -> OsString {
    let mut values = OsString::new();
    for name in ["NO_PROXY", "no_proxy"] {
        if let Some(value) = std::env::var_os(name).filter(|value| !value.is_empty()) {
            values.push(value);
            values.push(OsStr::new(","));
        }
    }
    values.push("127.0.0.1,localhost,::1");
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
    receiver: &ReceiverTask,
    signals: &mut RunSignals,
) -> Result<ExitStatus, String> {
    let mut grace = None;
    loop {
        if let Some(status) = child.try_wait().map_err(|error| error.to_string())? {
            return Ok(status);
        }
        if receiver.finished() {
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

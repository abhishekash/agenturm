//! Launch an already instrumented process with a local OTLP trace receiver.

use crate::trace_import::parse_source_id;
use crate::trace_receiver::TraceReceiver;
use agenturm_kernel::ExtensionId;
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

const USAGE: &str = "agenturm run --db <path> [--source-id otlp.<id>] -- <program> [args...]";
const SIGNAL_GRACE: Duration = Duration::from_secs(2);

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct TraceRunOptions {
    db: PathBuf,
    source: ExtensionId,
    program: OsString,
    arguments: Vec<OsString>,
}

pub(super) fn parse_trace_run(args: &[OsString]) -> Result<TraceRunOptions, String> {
    let delimiter = args.iter().position(|arg| arg == "--").ok_or(USAGE)?;
    let (program, arguments) = args[delimiter + 1..].split_first().ok_or(USAGE)?;
    if program.is_empty() {
        return Err(USAGE.into());
    }
    let (flags, extras) = args[..delimiter].as_chunks::<2>();
    if !extras.is_empty() {
        return Err(USAGE.into());
    }
    let (mut db, mut source) = (None, None);
    for [flag, value] in flags {
        if value.is_empty() || value.as_encoded_bytes().starts_with(b"--") {
            return Err(USAGE.into());
        }
        match flag.to_str() {
            Some("--db") if db.is_none() && value != "-" && value != ":memory:" => {
                db = Some(PathBuf::from(value))
            }
            Some("--source-id") if source.is_none() => {
                source = Some(parse_source_id(value.to_str().ok_or(USAGE)?)?)
            }
            _ => return Err(USAGE.into()),
        }
    }
    Ok(TraceRunOptions {
        db: db.ok_or(USAGE)?,
        source: source
            .unwrap_or_else(|| parse_source_id("otlp.run").expect("valid default launcher source")),
        program: program.clone(),
        arguments: arguments.to_vec(),
    })
}

pub fn execute_trace_run(options: &TraceRunOptions) -> Result<ExitCode, String> {
    let mut signals = RunSignals::new()?;
    // Fail before spawning a program if the observation database cannot
    // accept traces, and release the writer before the receiver starts.
    drop(
        agenturm_store::Store::open_collector(&options.db)
            .map_err(|error| format!("open trace history: {error}"))?,
    );
    let launch_time = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_err(|_| "system time is outside the launcher range")?
        .as_nanos();
    let launch_session = format!("launch-{}-{launch_time}", std::process::id());
    let receiver =
        TraceReceiver::bind_ephemeral(options.db.clone(), options.source.clone(), launch_session)?;
    let mut command = traced_command(options, receiver.address());
    let isolated = configure_process_group(&mut command);
    let receiver = ReceiverTask::start(receiver)?;
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

fn traced_command(options: &TraceRunOptions, address: SocketAddr) -> Command {
    let base = format!("http://{address}");
    let endpoint = format!("{base}/v1/traces");
    let mut child = Command::new(&options.program);
    child
        .args(&options.arguments)
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

fn exit_code(status: ExitStatus) -> ExitCode {
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
    fn start(receiver: TraceReceiver) -> Result<Self, String> {
        let stop = Arc::new(AtomicBool::new(false));
        let receiver_stop = Arc::clone(&stop);
        let thread = thread::Builder::new()
            .name("agenturm-run-traces".into())
            .spawn(move || receiver.serve(&receiver_stop))
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

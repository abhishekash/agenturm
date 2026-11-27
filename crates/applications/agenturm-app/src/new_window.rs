//! Linux cockpit launcher for an existing tmux session or graphical terminal.

use super::parse_os_command;
use serde::{Deserialize, Serialize};
use std::ffi::{OsStr, OsString};
use std::fmt::Write as _;
use std::io::ErrorKind;
use std::os::unix::{
    ffi::{OsStrExt, OsStringExt},
    process::CommandExt,
};
use std::path::{Path, PathBuf};
use std::process::{Command, ExitCode};

const FLAG: &str = "--new-window";
const HANDOFF: &str = "__cockpit-window";
const MAX_HANDOFF_BYTES: usize = 32 * 1024;
const ENVIRONMENT: [&str; 4] = [
    "HOME",
    "XDG_CONFIG_HOME",
    "XDG_STATE_HOME",
    "AGENTURM_CONTENT_CONFIG",
];
const INVALID_HANDOFF: &str = "Invalid cockpit window handoff";
const NO_TERMINAL: &str = "No supported terminal launcher is available. Use an existing tmux session, run cockpit in your current terminal, or use cockpit --once.";

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct WindowOptions(Mode);

#[derive(Clone, Debug, Eq, PartialEq)]
enum Mode {
    Launch(Vec<OsString>),
    Resume(Handoff),
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
struct Handoff {
    directory: Vec<u8>,
    arguments: Vec<Vec<u8>>,
    environment: [Option<Vec<u8>>; ENVIRONMENT.len()],
}

fn cockpit_invocation<T: AsRef<OsStr>>(args: &[T]) -> bool {
    let arg = |index: usize| args.get(index).map(AsRef::as_ref);
    let is = |index, value: &str| arg(index).is_some_and(|argument| argument == value);
    is(0, "cockpit")
        || (is(0, "broker") && (is(1, "cockpit") || (is(1, "--broker-config") && is(3, "cockpit"))))
}

pub(super) fn handles<T: AsRef<OsStr>>(args: &[T]) -> bool {
    args.first().is_some_and(|arg| arg.as_ref() == HANDOFF)
        || (cockpit_invocation(args) && args.iter().any(|arg| arg.as_ref() == FLAG))
}

pub(super) fn parse(args: &[OsString]) -> Result<WindowOptions, String> {
    if args[0] == HANDOFF {
        let [_, encoded] = args else {
            return Err(INVALID_HANDOFF.into());
        };
        return Handoff::decode(encoded).map(|handoff| WindowOptions(Mode::Resume(handoff)));
    }
    let duplicates = args.iter().filter(|argument| *argument == FLAG).count();
    if duplicates != 1 {
        return Err("Specify --new-window once".into());
    }
    if args.iter().any(|argument| argument == "--once") {
        return Err(
            "--new-window requires an interactive cockpit. For a snapshot, use cockpit --once."
                .into(),
        );
    }
    let children: Vec<_> = args
        .iter()
        .filter(|argument| *argument != FLAG)
        .cloned()
        .collect();
    parse_os_command(&children)?;
    Ok(WindowOptions(Mode::Launch(children)))
}

impl Handoff {
    fn from_invocation(args: &[OsString]) -> Result<Self, String> {
        let directory = std::env::current_dir()
            .map_err(|error| format!("Cannot get the cockpit working directory: {error}"))?;
        Ok(Self {
            directory: directory.into_os_string().into_vec(),
            arguments: args
                .iter()
                .map(|argument| argument.as_bytes().to_vec())
                .collect(),
            environment: ENVIRONMENT.map(|name| std::env::var_os(name).map(OsStringExt::into_vec)),
        })
    }

    fn encode(&self) -> Result<String, String> {
        let bytes = serde_json::to_vec(self).map_err(|_| INVALID_HANDOFF)?;
        if bytes.len() > MAX_HANDOFF_BYTES {
            return Err("Cockpit window arguments exceed the launch limit".into());
        }
        let mut encoded = String::with_capacity(bytes.len() * 2);
        for byte in bytes {
            write!(encoded, "{byte:02x}").expect("write to string");
        }
        Ok(encoded)
    }

    fn decode(encoded: &OsStr) -> Result<Self, String> {
        let encoded = encoded.as_bytes();
        let (pairs, trailing) = encoded.as_chunks::<2>();
        if encoded.is_empty() || !trailing.is_empty() || pairs.len() > MAX_HANDOFF_BYTES {
            return Err(INVALID_HANDOFF.into());
        }
        let bytes = pairs
            .iter()
            .map(|[high, low]| {
                let high = (*high as char).to_digit(16)? as u8;
                let low = (*low as char).to_digit(16)? as u8;
                Some((high << 4) | low)
            })
            .collect::<Option<Vec<_>>>()
            .ok_or(INVALID_HANDOFF)?;
        let handoff: Self = serde_json::from_slice(&bytes).map_err(|_| INVALID_HANDOFF)?;
        if !cockpit_invocation(
            &handoff
                .arguments
                .iter()
                .map(|arg| OsStr::from_bytes(arg))
                .collect::<Vec<_>>(),
        ) || handoff
            .arguments
            .iter()
            .any(|arg| [FLAG.as_bytes(), b"--once"].contains(&arg.as_slice()))
            || !Path::new(OsStr::from_bytes(&handoff.directory)).is_absolute()
            || std::iter::once(&handoff.directory)
                .chain(&handoff.arguments)
                .chain(handoff.environment.iter().flatten())
                .any(|bytes| bytes.contains(&0))
        {
            return Err(INVALID_HANDOFF.into());
        }
        Ok(handoff)
    }

    fn resume(&self, executable: &Path) -> Result<ExitCode, String> {
        let mut child = Command::new(executable);
        child.current_dir(PathBuf::from(OsStr::from_bytes(&self.directory)));
        child.args(self.arguments.iter().map(|arg| OsStr::from_bytes(arg)));
        for (name, value) in ENVIRONMENT.into_iter().zip(&self.environment) {
            if let Some(value) = value {
                child.env(name, OsStr::from_bytes(value));
            } else {
                child.env_remove(name);
            }
        }
        Err(format!(
            "Cannot start cockpit in the new window: {}",
            child.exec()
        ))
    }
}

fn tmux_target() -> Option<String> {
    let environment = std::env::var_os("TMUX")?;
    let session = environment.as_bytes().rsplit(|byte| *byte == b',').next()?;
    if environment.as_bytes().contains(&b',')
        && !session.is_empty()
        && session.len() <= 10
        && session.iter().all(u8::is_ascii_digit)
    {
        Some(format!("${}:", std::str::from_utf8(session).ok()?))
    } else {
        None
    }
}

pub fn execute(options: &WindowOptions) -> Result<ExitCode, String> {
    let executable = std::env::current_exe()
        .map_err(|error| format!("Cannot locate the running Agenturm binary: {error}"))?;
    let arguments = match &options.0 {
        Mode::Launch(arguments) => arguments,
        Mode::Resume(handoff) => return handoff.resume(&executable),
    };
    let encoded = Handoff::from_invocation(arguments)?.encode()?;
    if let Some(target) = tmux_target() {
        // tmux parses semicolon separators at its command boundary even
        // when given distinct argv elements. The carry data is hexadecimal.
        if executable.as_os_str().as_bytes().contains(&b';') {
            return Err(
                "The Agenturm executable path cannot contain a semicolon when launching from tmux"
                    .into(),
            );
        }
        if let Some(code) = launch(
            "tmux",
            &["new-window", "-t", &target, "-n", "Agenturm", "--"],
            &executable,
            &encoded,
        )? {
            return Ok(code);
        }
    }
    if ["DISPLAY", "WAYLAND_DISPLAY"]
        .iter()
        .any(|name| std::env::var_os(name).is_some_and(|value| !value.is_empty()))
    {
        for (terminal, flags) in [
            ("gnome-terminal", &["--wait", "--"][..]),
            ("konsole", &["--nofork", "-e"][..]),
            ("xterm", &["-e"][..]),
        ] {
            if let Some(code) = launch(terminal, flags, &executable, &encoded)? {
                return Ok(code);
            }
        }
    }
    Err(NO_TERMINAL.into())
}

fn launch(
    terminal: &str,
    flags: &[&str],
    executable: &Path,
    encoded: &str,
) -> Result<Option<ExitCode>, String> {
    let status = match Command::new(terminal)
        .args(flags)
        .arg(executable)
        .args([HANDOFF, encoded])
        .status()
    {
        Ok(status) => status,
        Err(error) if error.kind() == ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(format!("Cannot launch {terminal}: {error}")),
    };
    if !status.success() {
        eprintln!("{terminal} returned {status}");
    }
    Ok(Some(crate::trace_run::exit_code(status)))
}

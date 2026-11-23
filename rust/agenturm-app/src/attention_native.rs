//! Explicit and durable metadata-only native attention delivery.

mod model;
mod python;
mod receiver;

use agenturm_store::native_outbox::{Event, NativeOutbox, QueuedEvent};
use model::{Adapter, Binding, SourceClock};
use receiver::{PROTOCOL as RUST_PROTOCOL, RustBinding, RustReceiver};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{
    collections::BTreeMap,
    fs::{self, File, OpenOptions},
    io::{self, Read, Write},
    os::unix::fs::{MetadataExt, OpenOptionsExt},
    path::{Path, PathBuf},
};

const MAX_BYTES: usize = 8192;
const USAGE: &str = concat!(
    "usage: agenturm attention native bind --receiver-protocol python-v0.2.8 ",
    "--endpoint <loopback-url> --token-file <path> --output <path> ",
    "--receiver-id <id> --service <name> --source-id <id> --agent-id <id> ",
    "--session-id <id> --trace-id <id> [--turn-id <id>] [--parent-agent-id <id>] ",
    "[--adapter <generic-v1|codex-notify-v1>] [--native-turn-id <id>] ",
    "[--follow-recorded-turns] [--receiver-clock]\n",
    "       agenturm attention native bind --receiver-protocol rust-v1 ",
    "--endpoint <reader-url> --token-file <reader-token> --collector-endpoint <collector-url> ",
    "--collector-token-file <capture-token> --collector-source <otlp.id> --output <path> ",
    "--source-id <native-source> --session-id <id> --turn-id <id> [--worker-id <id>]\n",
    "       agenturm attention native <send|hook|retry> --binding <path> ",
    "--token-file <path> [--collector-token-file <path>] [--state <private-db>] [<event-json>]\n",
);

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct NativeOptions {
    action: Action,
    token_file: PathBuf,
    collector_token_file: Option<PathBuf>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum InputMode {
    Send,
    Hook,
    Retry,
}

#[derive(Clone, Eq, PartialEq)]
enum Action {
    Bind(Box<BindArgs>),
    Delivery {
        mode: InputMode,
        binding: PathBuf,
        state: Option<PathBuf>,
        payload: Option<String>,
    },
}

impl std::fmt::Debug for Action {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Bind(_) => f.write_str("Bind(..)"),
            Self::Delivery { mode, .. } => f.debug_tuple("Delivery").field(mode).finish(),
        }
    }
}

#[derive(Clone, Eq, PartialEq)]
struct BindArgs {
    output: PathBuf,
    binding: SavedBinding,
}

#[derive(Clone, Deserialize, Eq, PartialEq, Serialize)]
#[serde(untagged)]
enum SavedBinding {
    Python(Binding),
    Rust(RustBinding),
}

impl SavedBinding {
    fn sender(&self) -> &str {
        match self {
            Self::Python(binding) => &binding.sender_id,
            Self::Rust(binding) => &binding.sender_id,
        }
    }

    fn validate(&self, collector_token_file: Option<&Path>) -> Result<(), String> {
        match self {
            Self::Python(binding) if collector_token_file.is_none() => binding.validate(),
            Self::Rust(binding) if collector_token_file.is_some() => binding.validate(),
            _ => Err("Collector token is required only for a Rust native receiver".into()),
        }
    }

    fn event(&self, bytes: &[u8]) -> Result<Event, String> {
        match self {
            Self::Python(binding) => binding.event(bytes),
            Self::Rust(binding) => binding.event(bytes),
        }
    }

    fn fingerprint(&self) -> Result<[u8; 32], String> {
        let bytes =
            serde_json::to_vec(self).map_err(|_| "Cannot encode native binding metadata")?;
        Ok(Sha256::digest(bytes).into())
    }
}

enum ConnectedReceiver {
    Python(crate::remote_history::legacy::LegacyReceiver),
    Rust(RustReceiver),
}

impl ConnectedReceiver {
    fn open(
        binding: &SavedBinding,
        reader_token: &Path,
        collector_token: Option<&Path>,
    ) -> Result<Self, String> {
        match binding {
            SavedBinding::Python(binding) => {
                python::connect(binding, reader_token).map(Self::Python)
            }
            SavedBinding::Rust(binding) => RustReceiver::connect(
                binding,
                reader_token,
                collector_token.ok_or("Rust native delivery requires a collector token file")?,
            )
            .map(Self::Rust),
        }
    }

    fn verify(&self, binding: &SavedBinding, event: &Event) -> Result<(), String> {
        match (self, binding) {
            (Self::Python(peer), SavedBinding::Python(binding)) => {
                let trace = event
                    .trace_id
                    .as_deref()
                    .ok_or("Python native event has no retained trace")?;
                python::verify(peer, binding, trace, &event.turn_id)
            }
            (Self::Rust(peer), SavedBinding::Rust(binding)) => peer.verify(binding),
            _ => Err("Native receiver protocol changed".into()),
        }
    }

    fn send(&self, binding: &SavedBinding, event: &QueuedEvent) -> Result<(), String> {
        match (self, binding) {
            (Self::Python(peer), SavedBinding::Python(binding)) => {
                python::send(peer, binding, event)
            }
            (Self::Rust(peer), SavedBinding::Rust(binding)) => peer.send(binding, event),
            _ => Err("Native receiver protocol changed".into()),
        }
    }
}

pub(super) fn parse(args: &[String]) -> Result<NativeOptions, String> {
    let (verb, mut args) = args.split_first().ok_or(USAGE)?;
    let bind = verb == "bind";
    let mode = match verb.as_str() {
        "bind" => None,
        "send" => Some(InputMode::Send),
        "hook" => Some(InputMode::Hook),
        "retry" => Some(InputMode::Retry),
        _ => return Err(USAGE.into()),
    };
    let mut flags = BTreeMap::new();
    let mut payload = None;
    while let Some((argument, remaining)) = args.split_first() {
        args = remaining;
        if !argument.starts_with("--") {
            if bind || mode == Some(InputMode::Retry) || payload.is_some() {
                return Err(USAGE.into());
            }
            payload = Some(argument.clone());
            continue;
        }
        let name = argument.as_str();
        let value = if bind && matches!(name, "--follow-recorded-turns" | "--receiver-clock") {
            String::new()
        } else {
            let (value, remaining) = args
                .split_first()
                .filter(|(value, _)| !value.is_empty() && !value.starts_with("--"))
                .ok_or(USAGE)?;
            args = remaining;
            value.clone()
        };
        if flags.insert(name, value).is_some() {
            return Err(USAGE.into());
        }
    }

    let token_file = flags.remove("--token-file").ok_or(USAGE)?.into();
    let collector_token_file = flags.remove("--collector-token-file").map(PathBuf::from);
    let action = if let Some(mode) = mode {
        Action::Delivery {
            mode,
            binding: flags.remove("--binding").ok_or(USAGE)?.into(),
            state: flags.remove("--state").map(PathBuf::from),
            payload,
        }
    } else {
        let output = flags.remove("--output").ok_or(USAGE)?.into();
        let binding = match flags.remove("--receiver-protocol").as_deref() {
            Some(crate::remote_history::legacy::PROTOCOL) if collector_token_file.is_none() => {
                SavedBinding::Python(parse_python_binding(&mut flags)?)
            }
            Some(RUST_PROTOCOL) if collector_token_file.is_some() => {
                SavedBinding::Rust(RustBinding::parse(&mut flags)?)
            }
            _ => return Err(USAGE.into()),
        };
        Action::Bind(Box::new(BindArgs { output, binding }))
    };
    if !flags.is_empty() {
        return Err(USAGE.into());
    }
    Ok(NativeOptions {
        action,
        token_file,
        collector_token_file,
    })
}

fn parse_python_binding(flags: &mut BTreeMap<&str, String>) -> Result<Binding, String> {
    let adapter = match flags.remove("--adapter").as_deref() {
        None | Some("generic-v1") => Adapter::GenericV1,
        Some("codex-notify-v1") => Adapter::CodexNotifyV1,
        _ => return Err(USAGE.into()),
    };
    let binding = Binding {
        schema: model::BINDING_SCHEMA.into(),
        sender_id: String::new(),
        endpoint: flags.remove("--endpoint").ok_or(USAGE)?,
        adapter,
        native_turn_id: flags.remove("--native-turn-id").unwrap_or_default(),
        source_clock: if flags.remove("--receiver-clock").is_some() {
            SourceClock::Receiver
        } else {
            SourceClock::Unknown
        },
        follow_recorded_turns: flags.remove("--follow-recorded-turns").is_some(),
        receiver_id: flags.remove("--receiver-id").ok_or(USAGE)?,
        service: flags.remove("--service").ok_or(USAGE)?,
        agent_id: flags.remove("--agent-id").ok_or(USAGE)?,
        session_id: flags.remove("--session-id").ok_or(USAGE)?,
        trace_id: flags.remove("--trace-id").ok_or(USAGE)?,
        turn_id: flags.remove("--turn-id").unwrap_or_default(),
        source_id: flags.remove("--source-id").ok_or(USAGE)?,
        parent_agent_id: flags.remove("--parent-agent-id").unwrap_or_default(),
    };
    Ok(binding)
}

pub(super) fn execute(options: &NativeOptions) -> Result<(), String> {
    match &options.action {
        Action::Bind(args) => {
            let mut binding = args.binding.clone();
            match &mut binding {
                SavedBinding::Python(binding) => {
                    binding.sender_id = model::sender_id()?;
                    let receiver = python::connect(binding, &options.token_file)?;
                    python::verify(&receiver, binding, &binding.trace_id, &binding.turn_id)?;
                }
                SavedBinding::Rust(binding) => {
                    binding.sender_id = model::sender_id()?;
                    RustReceiver::bind(
                        binding,
                        &options.token_file,
                        options.collector_token_file.as_deref().ok_or(USAGE)?,
                    )?;
                }
            }
            let mut bytes =
                serde_json::to_vec_pretty(&binding).map_err(|_| "Cannot encode native binding")?;
            bytes.push(b'\n');
            if bytes.len() > MAX_BYTES {
                return Err("Native binding exceeds the 8 KiB limit".into());
            }
            write_binding(&args.output, binding.sender(), &bytes)
        }
        Action::Delivery {
            mode,
            binding,
            state,
            payload,
        } => {
            let result = deliver(
                *mode,
                binding,
                state.as_deref(),
                payload.as_deref(),
                &options.token_file,
                options.collector_token_file.as_deref(),
            );
            if *mode == InputMode::Hook {
                if let Err(error) = result {
                    eprintln!("Agenturm native observation uncertain or unavailable: {error}");
                }
                Ok(())
            } else {
                result
            }
        }
    }
}

fn deliver(
    mode: InputMode,
    binding_path: &Path,
    state: Option<&Path>,
    payload: Option<&str>,
    token_file: &Path,
    collector_token_file: Option<&Path>,
) -> Result<(), String> {
    let binding = read_binding(binding_path)?;
    binding.validate(collector_token_file)?;
    let fingerprint = binding.fingerprint()?;
    let state = match state {
        Some(state) => state.to_owned(),
        None => default_outbox_path()?,
    };
    let mut outbox = NativeOutbox::open(&state, owner_uid())?;
    let connect = || ConnectedReceiver::open(&binding, token_file, collector_token_file);
    let mut receiver = None;
    if mode != InputMode::Retry {
        let event = match payload {
            Some(payload) => binding.event(payload.as_bytes())?,
            None => binding.event(&read_bounded(io::stdin().lock())?)?,
        };
        let peer = connect()?;
        peer.verify(&binding, &event)?;
        // Queue only proven metadata, before any delivery can be attempted.
        outbox.enqueue(binding.sender(), &fingerprint, event)?;
        receiver = Some(peer);
    }
    outbox.flush(binding.sender(), &fingerprint, |queued| {
        let receiver = match receiver.as_ref() {
            Some(receiver) => receiver,
            None => receiver.insert(connect()?),
        };
        receiver.send(&binding, queued)
    })?;
    Ok(())
}

fn owner_uid() -> u32 {
    // No pointers, shared state, or side effects.
    unsafe { libc::geteuid() }
}

fn read_bounded(input: impl Read) -> Result<Vec<u8>, String> {
    let mut bytes = Vec::new();
    input
        .take((MAX_BYTES + 1) as u64)
        .read_to_end(&mut bytes)
        .map_err(|_| "Cannot read native input")?;
    if bytes.len() > MAX_BYTES {
        return Err("Native input exceeds the 8 KiB limit".into());
    }
    Ok(bytes)
}

fn read_binding(path: &Path) -> Result<SavedBinding, String> {
    const ERROR: &str = "Native binding must be an owner-only regular file";
    let file = OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK)
        .open(path)
        .map_err(|_| ERROR)?;
    let metadata = file.metadata().map_err(|_| ERROR)?;
    if !metadata.is_file()
        || metadata.uid() != owner_uid()
        || metadata.nlink() != 1
        || metadata.mode() & 0o077 != 0
    {
        return Err(ERROR.into());
    }
    serde_json::from_slice(&read_bounded(file)?).map_err(|_| "Invalid native binding JSON".into())
}

fn write_binding(path: &Path, nonce: &str, bytes: &[u8]) -> Result<(), String> {
    use rustix::fs::{CWD, RenameFlags, renameat_with};

    let parent = path
        .parent()
        .filter(|path| !path.as_os_str().is_empty())
        .unwrap_or(Path::new("."));
    let staging = parent.join(format!(".agenturm-native-binding-{nonce}"));
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(&staging)
        .map_err(|_| "Cannot create a new private native binding")?;
    let write = file
        .write_all(bytes)
        .and_then(|()| file.sync_all())
        .and_then(|()| {
            renameat_with(CWD, &staging, CWD, path, RenameFlags::NOREPLACE).map_err(io::Error::from)
        });
    if write.is_err() {
        let _ = fs::remove_file(&staging);
        return Err("Cannot publish a new private native binding".into());
    }
    File::open(parent)
        .and_then(|file| file.sync_all())
        .map_err(|_| "Cannot confirm durability of the private native binding".to_owned())
}

fn default_outbox_path() -> Result<PathBuf, String> {
    use std::os::unix::fs::DirBuilderExt;
    let state = crate::cockpit_startup::state_directory(
        std::env::var_os("XDG_STATE_HOME").as_deref(),
        std::env::var_os("HOME").as_deref(),
    )?;
    let directory = state.join("native-attention");
    fs::DirBuilder::new()
        .mode(0o700)
        .recursive(true)
        .create(&directory)
        .map_err(|_| "Cannot create native attention state directory")?;
    let metadata = fs::symlink_metadata(&directory)
        .map_err(|_| "Cannot inspect native attention state directory")?;
    if !metadata.is_dir() || metadata.uid() != owner_uid() || metadata.mode() & 0o077 != 0 {
        return Err("Native attention state directory must be owner-only".into());
    }
    Ok(directory.join("outbox.sqlite"))
}

fn read_random(out: &mut [u8]) -> Result<(), String> {
    File::open("/dev/urandom")
        .and_then(|mut file| file.read_exact(out))
        .map_err(|_| "Cannot generate a native sender identity".into())
}

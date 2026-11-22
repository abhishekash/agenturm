//! Explicit native broker commands. The broker owns history paths.

use super::*;
use agenturm_storage_service::NativeStore;
use serde::Deserialize;
use std::path::{Path, PathBuf};

const USAGE: &str = concat!(
    "agenturm broker <collect|query|review|cockpit|task> --broker-config <json> [command options]\n",
    "       agenturm broker grant <check|approve|revoke> --broker-config <json> --binding <json>\n",
    "       agenturm broker content list --broker-config <json> --source-id <id> --session-id <id>\n",
    "       agenturm broker content preview --broker-config <json> --selection <json>\n",
    "       agenturm broker <backup|integrity> --broker-config <json>\n",
    "       agenturm broker prune --broker-config <json> --before-unix-ns <cutoff>"
);

#[derive(Debug)]
pub struct BrokerExecutionError {
    message: String,
    exit_code: u8,
}

impl BrokerExecutionError {
    pub fn exit_code(&self) -> u8 {
        self.exit_code
    }
}
impl std::fmt::Display for BrokerExecutionError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(&self.message)
    }
}
impl std::error::Error for BrokerExecutionError {}
impl From<String> for BrokerExecutionError {
    fn from(message: String) -> Self {
        Self {
            message,
            exit_code: 1,
        }
    }
}
impl From<&str> for BrokerExecutionError {
    fn from(message: &str) -> Self {
        message.to_owned().into()
    }
}
impl From<task_cli::TaskExecutionError> for BrokerExecutionError {
    fn from(error: task_cli::TaskExecutionError) -> Self {
        let exit_code = error.exit_code();
        Self {
            message: error.to_string(),
            exit_code,
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct BrokerOptions {
    config: PathBuf,
    command: BrokerCommand,
}

#[derive(Clone, Debug, Eq, PartialEq)]
enum BrokerCommand {
    Application(Box<Command>),
    Grant {
        verb: String,
        binding: PathBuf,
    },
    Prune(u64),
    Backup,
    Integrity,
    ContentList {
        source: ExtensionId,
        session: String,
    },
    ContentPreview {
        selection: PathBuf,
    },
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct BrokerConfig {
    version: u16,
    socket: PathBuf,
    receiver_id: String,
    service_uid: u32,
    service_gid: u32,
}

pub(super) fn parse(args: &[String]) -> Result<BrokerOptions, String> {
    if args.len() == 1 && matches!(args[0].as_str(), "--help" | "-h" | "help") {
        return Ok(BrokerOptions {
            config: PathBuf::new(),
            command: BrokerCommand::Application(Box::new(Command::Help)),
        });
    }
    let mut command = Vec::new();
    let mut config = None;
    let mut index = 0;
    while let Some(argument) = args.get(index) {
        if argument == "--broker-config" {
            let value = args
                .get(index + 1)
                .filter(|value| !value.starts_with("--"))
                .ok_or(USAGE)?;
            if config.replace(PathBuf::from(value)).is_some() {
                return Err(USAGE.into());
            }
            index += 2;
        } else {
            command.push(argument.clone());
            index += 1;
        }
    }
    let config = config.ok_or(USAGE)?;
    let management = match command.as_slice() {
        [content, verb, source_flag, source, session_flag, session]
            if content == "content"
                && verb == "list"
                && source_flag == "--source-id"
                && session_flag == "--session-id" =>
        {
            Some(BrokerCommand::ContentList {
                source: ExtensionId::parse(source).map_err(|_| USAGE)?,
                session: session.clone(),
            })
        }
        [content, verb, flag, path]
            if content == "content"
                && verb == "preview"
                && flag == "--selection"
                && !path.starts_with("--") =>
        {
            Some(BrokerCommand::ContentPreview {
                selection: path.into(),
            })
        }
        [verb] if verb == "backup" => Some(BrokerCommand::Backup),
        [verb] if verb == "integrity" => Some(BrokerCommand::Integrity),
        [verb, flag, cutoff] if verb == "prune" && flag == "--before-unix-ns" => {
            Some(BrokerCommand::Prune(cutoff.parse().map_err(|_| USAGE)?))
        }
        [command, verb, flag, path]
            if command == "grant"
                && matches!(verb.as_str(), "check" | "approve" | "revoke")
                && flag == "--binding"
                && !path.starts_with("--") =>
        {
            Some(BrokerCommand::Grant {
                verb: verb.clone(),
                binding: path.into(),
            })
        }
        _ => None,
    };
    if let Some(command) = management {
        return Ok(BrokerOptions { config, command });
    }
    // Existing parsers require a local path, which is irrelevant for broker
    // operations. Never accept a caller-supplied history database path.
    if command.iter().any(|argument| argument == "--db") {
        return Err("Broker commands use the service-owned history and accept no --db".into());
    }
    if !matches!(
        command.first().map(String::as_str),
        Some("collect" | "query" | "review" | "inspect" | "cockpit" | "task")
    ) {
        return Err(USAGE.into());
    }
    if command.first().is_some_and(|verb| verb == "cockpit") {
        command[0] = "inspect".into();
    }
    command.extend(["--db".into(), "broker-history".into()]);
    let command = parse_command(&command)?;
    Ok(BrokerOptions {
        config,
        command: BrokerCommand::Application(Box::new(command)),
    })
}

fn connect(path: &Path) -> Result<(NativeStore, BrokerConfig), String> {
    let file = File::open(path).map_err(|error| format!("Open broker configuration: {error}"))?;
    let mut bytes = Vec::new();
    file.take(64 * 1024 + 1)
        .read_to_end(&mut bytes)
        .map_err(|error| error.to_string())?;
    if bytes.len() > 64 * 1024 {
        return Err("Broker configuration exceeds 64 KiB".into());
    }
    let config: BrokerConfig =
        serde_json::from_slice(&bytes).map_err(|_| "Invalid broker configuration")?;
    if config.version != 1 || !config.socket.is_absolute() {
        return Err("Broker configuration requires version 1 and an absolute socket path".into());
    }
    let store = NativeStore::new(
        config.socket.clone(),
        config.receiver_id.clone(),
        config.service_uid,
        config.service_gid,
    )
    .map_err(|error| error.to_string())?;
    Ok((store, config))
}

fn workspace_identity(
    config: &BrokerConfig,
    history_identity: &str,
    state: &Path,
) -> Result<PathBuf, String> {
    use sha2::{Digest, Sha256};
    use std::os::unix::fs::{DirBuilderExt, MetadataExt, OpenOptionsExt};

    if history_identity.len() != 32
        || !history_identity
            .bytes()
            .all(|byte| byte.is_ascii_hexdigit())
    {
        return Err("Invalid broker history identity".into());
    }
    let encoded = serde_json::to_vec(&(
        "agenturm-broker-workspace/1",
        &config.socket,
        &config.receiver_id,
        config.service_uid,
        config.service_gid,
        history_identity,
    ))
    .map_err(|error| error.to_string())?;
    let namespace = format!("{:x}", Sha256::digest(encoded));
    let directory = state.join("brokers").join(&namespace);
    std::fs::DirBuilder::new()
        .recursive(true)
        .mode(0o700)
        .create(&directory)
        .map_err(|error| format!("Create broker workspace: {error}"))?;
    let metadata = std::fs::symlink_metadata(&directory).map_err(|error| error.to_string())?;
    // SAFETY: geteuid has no arguments or preconditions.
    let uid = unsafe { libc::geteuid() };
    if !metadata.is_dir() || metadata.uid() != uid || metadata.mode() & 0o077 != 0 {
        return Err("Broker workspace directory must be private and owned by the client".into());
    }
    let marker = directory.join("identity");
    let expected = format!("agenturm-broker-workspace/1\n{namespace}\n");
    let nonce = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_err(|error| error.to_string())?
        .as_nanos();
    let temporary = directory.join(format!(".identity-{}-{nonce}", std::process::id()));
    let mut file = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC)
        .open(&temporary)
        .map_err(|error| error.to_string())?;
    let publication = (|| -> Result<(), String> {
        file.write_all(expected.as_bytes())
            .map_err(|error| error.to_string())?;
        file.sync_all().map_err(|error| error.to_string())?;
        match rustix::fs::renameat_with(
            rustix::fs::CWD,
            &temporary,
            rustix::fs::CWD,
            &marker,
            rustix::fs::RenameFlags::NOREPLACE,
        ) {
            Ok(()) => Ok(()),
            Err(rustix::io::Errno::EXIST) => Ok(()),
            Err(error) => Err(error.to_string()),
        }
    })();
    drop(file);
    let cleanup = std::fs::remove_file(&temporary);
    publication?;
    match cleanup {
        Ok(()) => {}
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => return Err(error.to_string()),
    }
    File::open(&directory)
        .and_then(|file| file.sync_all())
        .map_err(|error| error.to_string())?;
    let mut file = std::fs::OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC | libc::O_NONBLOCK)
        .open(&marker)
        .map_err(|error| error.to_string())?;
    let metadata = file.metadata().map_err(|error| error.to_string())?;
    if !metadata.is_file()
        || metadata.uid() != uid
        || metadata.nlink() != 1
        || metadata.mode() & 0o077 != 0
    {
        return Err("Invalid broker workspace identity file".into());
    }
    let mut contents = String::new();
    (&mut file)
        .take(256)
        .read_to_string(&mut contents)
        .map_err(|error| error.to_string())?;
    if contents != expected {
        return Err("Broker workspace identity changed".into());
    }
    Ok(marker)
}

fn open_workspace(
    config: &BrokerConfig,
    store: &NativeStore,
) -> Result<(NativeStore, PathBuf), String> {
    let identity = store
        .history_identity()
        .map_err(|error| error.to_string())?;
    let state = cockpit_startup::state_directory(
        std::env::var_os("XDG_STATE_HOME").as_deref(),
        std::env::var_os("HOME").as_deref(),
    )?;
    let workspace = workspace_identity(config, &identity, &state)?;
    let store = store
        .clone()
        .with_workspace_identity(&identity)
        .map_err(|error| error.to_string())?;
    Ok((store, workspace))
}

pub fn execute(options: &BrokerOptions) -> Result<(), BrokerExecutionError> {
    if matches!(&options.command, BrokerCommand::Application(command) if matches!(command.as_ref(), Command::Help))
    {
        println!(
            "{USAGE}\nThe broker configuration pins its socket, receiver and service UID/GID.\nUse collect --config <sources.json> [--watch], query paging options, or inspect."
        );
        return Ok(());
    }
    let (store, config) = connect(&options.config)?;
    let command = match &options.command {
        BrokerCommand::Application(command) => command.as_ref(),
        BrokerCommand::ContentList { source, session } => {
            let identity = store
                .history_identity()
                .map_err(|error| error.to_string())?;
            let store = store
                .with_workspace_identity(&identity)
                .map_err(|error| error.to_string())?;
            let prompts = store
                .list_private_prompts(source, session)
                .map_err(|error| error.to_string())?;
            println!(
                "{}",
                serde_json::to_string(&prompts).map_err(|error| error.to_string())?
            );
            return Ok(());
        }
        BrokerCommand::ContentPreview { selection } => {
            let file = File::open(selection).map_err(|error| error.to_string())?;
            let mut bytes = Vec::new();
            file.take(4097)
                .read_to_end(&mut bytes)
                .map_err(|error| error.to_string())?;
            if bytes.len() > 4096 {
                return Err("Private selection exceeds 4 KiB".into());
            }
            let selection =
                serde_json::from_slice(&bytes).map_err(|_| "Invalid private selection")?;
            let identity = store
                .history_identity()
                .map_err(|error| error.to_string())?;
            let store = store
                .with_workspace_identity(&identity)
                .map_err(|error| error.to_string())?;
            crate::with_private_stdout(|output| {
                store
                    .reveal_private(&selection, false, output)
                    .map_err(|error| error.to_string())
            })?;
            return Ok(());
        }
        BrokerCommand::Grant { verb, binding } => {
            let file = File::open(binding).map_err(|error| error.to_string())?;
            let mut bytes = Vec::new();
            file.take(4097)
                .read_to_end(&mut bytes)
                .map_err(|error| error.to_string())?;
            if bytes.len() > 4096 {
                return Err("Grant binding exceeds 4 KiB".into());
            }
            let binding: agenturm_storage_service::GrantBinding =
                serde_json::from_slice(&bytes).map_err(|_| "Invalid grant binding")?;
            let result = match verb.as_str() {
                "check" => serde_json::to_value(
                    store
                        .check_grant(&binding)
                        .map_err(|error| error.to_string())?,
                ),
                "approve" => serde_json::to_value(
                    store
                        .approve_grant(&binding)
                        .map_err(|error| error.to_string())?,
                ),
                "revoke" => serde_json::to_value(
                    store
                        .revoke_grant(&binding)
                        .map_err(|error| error.to_string())?,
                ),
                _ => return Err(USAGE.into()),
            }
            .map_err(|error| error.to_string())?;
            println!("{result}");
            return Ok(());
        }
        BrokerCommand::Prune(cutoff) => {
            let result = store
                .prune_before(*cutoff)
                .map_err(|error| error.to_string())?;
            println!(
                "{}",
                serde_json::to_string(&result).map_err(|error| error.to_string())?
            );
            return Ok(());
        }
        BrokerCommand::Backup => {
            let receipt = store.backup().map_err(|error| error.to_string())?;
            println!(
                "{}",
                serde_json::to_string(&receipt).map_err(|error| error.to_string())?
            );
            return Ok(());
        }
        BrokerCommand::Integrity => {
            store.check_integrity().map_err(|error| error.to_string())?;
            println!("{{\"ok\":true}}");
            return Ok(());
        }
    };
    match command {
        Command::Collect(options) => {
            let config = load_collection_config(&options.config)?;
            let stop = Arc::new(AtomicBool::new(false));
            let signals = options
                .watch_interval
                .map(|_| WatchSignalRegistrations::new(Arc::clone(&stop)))
                .transpose()?;
            let mut collector = ConfiguredCollector::with_store(
                config,
                options.db.clone(),
                store,
                options.watch_interval.map(|_| &stop),
            )?;
            collector.validate_local_database = false;
            let had_failures = if let Some(interval) = options.watch_interval {
                watch_collector(collector, interval, &stop)?.had_failures
            } else {
                let report = collector.refresh();
                println!("{}", report.summary.trim_end());
                report.has_failures()
            };
            drop(signals);
            if had_failures {
                return Err("Broker collection reported source or storage failures".into());
            }
        }
        Command::Query(query) => println!("{}", query_from_store(&store, query)?),
        Command::Review {
            query,
            turn_id,
            worker_id,
            output,
        } => println!(
            "{}",
            review_from_store(&store, query, turn_id, worker_id.as_deref(), output)?
        ),
        Command::Task(options) => {
            let (store, workspace) = open_workspace(&config, &store)?;
            let ledger = store.workspace_ledger(&workspace)?;
            task_cli::with_task_stdout(options, |output| {
                task_cli::execute_ledger(options, &ledger, &workspace, Some(&store), output)
            })?;
        }
        Command::Inspect(options) => {
            let mut app = AgenturmApp::default_composition();
            install_process_extensions(&mut app, &options.extensions)?;
            let (store, workspace) = open_workspace(&config, &store)?;
            agenturm_ui::run_workspace(move |request| {
                if matches!(&request, WorkspaceRequest::Work(command) if cockpit_workflows::is_prompt_command(command))
                    && let WorkspaceRequest::Work(command) = request {
                    return execute_private_cockpit(&store, command).map(WorkspaceResponse::Work);
                }
                workspace_response(&mut app, &store, &workspace, request, || None)
            }).map_err(|error| error.to_string())?;
        }
        _ => return Err(USAGE.into()),
    }
    Ok(())
}

fn execute_private_cockpit(
    store: &NativeStore,
    command: agenturm_ui::WorkCommand,
) -> Result<agenturm_ui::WorkResult, String> {
    use agenturm_storage_service::{PrivateEvidenceKind, PrivateEvidenceSelection};
    use agenturm_ui::{OperationDetailKind, PromptEntry, WorkCommand, WorkResult};
    match command {
        WorkCommand::Prompts(owner) => {
            let source = ExtensionId::parse(&owner.source).map_err(|error| error.to_string())?;
            let prompts = store
                .list_private_prompts(&source, &owner.session)
                .map_err(|error| error.to_string())?;
            Ok(WorkResult::Prompts(
                prompts
                    .into_iter()
                    .map(|prompt| {
                        let mut anchor = owner.clone();
                        anchor.turn = Some(prompt.turn_id);
                        PromptEntry {
                            anchor,
                            event_id: prompt.event_id,
                            revision: prompt.revision,
                            timestamp: prompt.observed_at_unix_ns,
                        }
                    })
                    .collect(),
            ))
        }
        WorkCommand::RevealPrompt(prompt, mut output) => {
            let selection = PrivateEvidenceSelection {
                source_id: ExtensionId::parse(&prompt.anchor.source)
                    .map_err(|error| error.to_string())?,
                session_id: prompt.anchor.session,
                event_id: prompt.event_id,
                revision: prompt.revision,
                kind: PrivateEvidenceKind::UserText {},
            };
            store
                .reveal_private(&selection, true, &mut output)
                .map_err(|error| error.to_string())?;
            Ok(WorkResult::Status(
                "Prompt shown from the verified current source".into(),
            ))
        }
        WorkCommand::RevealOperation(operation, mut output) => {
            let kind = match operation.kind {
                OperationDetailKind::Mcp => PrivateEvidenceKind::CodexMcpPayload {
                    turn_id: operation.turn_id,
                    operation_id: operation.operation_id,
                },
                OperationDetailKind::Edit => PrivateEvidenceKind::CodexEditDetail {
                    turn_id: operation.turn_id,
                    operation_id: operation.operation_id,
                },
            };
            let selection = PrivateEvidenceSelection {
                source_id: ExtensionId::parse(&operation.source_id)
                    .map_err(|error| error.to_string())?,
                session_id: operation.session_id,
                event_id: operation.event_id,
                revision: operation.revision,
                kind,
            };
            store
                .reveal_private(&selection, true, &mut output)
                .map_err(|error| error.to_string())?;
            Ok(WorkResult::Status(String::new()))
        }
        _ => Err("Unsupported broker private-content request".into()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse_words(words: &str) -> Result<BrokerOptions, String> {
        parse(
            &words
                .split_whitespace()
                .map(str::to_owned)
                .collect::<Vec<_>>(),
        )
    }

    #[test]
    fn broker_workspace_stays_with_the_history_incarnation_and_endpoint() {
        let nonce = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let state = std::env::temp_dir().join(format!(
            "agenturm-broker-workspace-{}-{nonce}",
            std::process::id()
        ));
        let mut config = BrokerConfig {
            version: 1,
            socket: "/tmp/broker-example.sock".into(),
            receiver_id: "receiver:one".into(),
            service_uid: 45000,
            service_gid: 45000,
        };
        let identity = "0123456789abcdef0123456789abcdef";
        let marker = workspace_identity(&config, identity, &state).unwrap();
        let ledger = agenturm_store::WorkspaceLedger::open(&marker).unwrap();
        ledger
            .create_task(None, "Retained task", "A local note")
            .unwrap();
        let reopened = workspace_identity(&config, identity, &state).unwrap();
        assert_eq!(marker, reopened);
        assert_eq!(
            agenturm_store::WorkspaceLedger::open(reopened)
                .unwrap()
                .list_tasks(10, None)
                .unwrap()
                .items
                .len(),
            1
        );
        let fresh =
            workspace_identity(&config, "1123456789abcdef0123456789abcdef", &state).unwrap();
        assert_ne!(marker, fresh);
        assert!(
            agenturm_store::WorkspaceLedger::open(fresh)
                .unwrap()
                .list_tasks(10, None)
                .unwrap()
                .items
                .is_empty()
        );
        config.receiver_id = "receiver:two".into();
        let other = workspace_identity(&config, identity, &state).unwrap();
        assert_ne!(marker, other);
        std::fs::write(&other, b"changed").unwrap();
        assert!(workspace_identity(&config, identity, &state).is_err());
        drop(ledger);
        std::fs::remove_dir_all(state).unwrap();
    }

    #[test]
    fn broker_commands_bind_the_endpoint_and_keep_history_paths_service_owned() {
        for command in [
            "query",
            "query --limit 1 --harness-id harness.codex",
            "review --harness-id harness.codex --session-id session --turn-id turn",
            "inspect",
            "cockpit",
            "task list",
            "collect --config sources.json",
            "collect --config sources.json --watch",
            "content list --source-id harness.pi --session-id session",
            "content preview --selection selection.json",
        ] {
            assert!(parse_words(&format!("{command} --broker-config broker.json")).is_ok());
        }
        for command in [
            "query",
            "query --broker-config",
            "query --broker-config one --broker-config two",
            "query --broker-config one --db private.sqlite",
            "backup --broker-config one --output arbitrary.sqlite",
            "review --broker-config one",
            "content preview --selection selection.json --db private.sqlite --broker-config one",
            "content list --source-id harness.pi --session-id session --config client-path.json --broker-config one",
        ] {
            assert!(parse_words(command).is_err());
        }
    }
}

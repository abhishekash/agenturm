//! Broker-side source resolution and private display delivery.

use agenturm_content::{OperationDetailKind, PromptReference, ResolvedSource};
use agenturm_kernel::ExtensionId;
use agenturm_kernel::ReadSnapshot;
use agenturm_storage_service_contract::native;
use agenturm_store::{
    CodexOperationSelection, EvalDetailSelection, EvalFindingSelection, Store, StoreError,
    TelemetryLogTextSelection, TelemetryPayloadSelection,
};
use serde::{Deserialize, Serialize};
use std::cell::RefCell;
use std::collections::BTreeSet;
use std::fs::{self, File, OpenOptions};
use std::io::{self, Read, Write};
use std::os::fd::AsRawFd;
use std::os::unix::fs::{MetadataExt, OpenOptionsExt};
use std::os::unix::net::UnixStream;
use std::path::{Component, Path, PathBuf};

const CONFIG_BYTES: u64 = 256 * 1024;
const MAX_PAYLOAD: usize = 16 * 1024;

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct PrivateEvidenceSelection {
    pub source_id: ExtensionId,
    pub session_id: String,
    pub event_id: String,
    pub revision: u32,
    pub kind: PrivateEvidenceKind,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct PrivatePromptSelection {
    pub(crate) source_id: ExtensionId,
    pub(crate) session_id: String,
    pub(crate) event_id: String,
    pub(crate) revision: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) snapshot_id: Option<ReadSnapshot>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum PrivateEvidenceKind {
    UserText {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        snapshot_id: Option<ReadSnapshot>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        history_identity: Option<String>,
    },
    CodexMcpPayload {
        turn_id: String,
        operation_id: String,
    },
    CodexEditDetail {
        turn_id: String,
        operation_id: String,
    },
    CodexCommandDetail {
        turn_id: String,
        operation_id: String,
    },
    CodexFailedCommandDetail {
        turn_id: String,
        operation_id: String,
    },
}

impl PrivateEvidenceSelection {
    fn operation(&self) -> Option<(OperationDetailKind, CodexOperationSelection<'_>)> {
        let (kind, turn_id, operation_id) = match &self.kind {
            PrivateEvidenceKind::UserText { .. } => return None,
            PrivateEvidenceKind::CodexMcpPayload {
                turn_id,
                operation_id,
            } => (OperationDetailKind::Mcp, turn_id, operation_id),
            PrivateEvidenceKind::CodexEditDetail {
                turn_id,
                operation_id,
            } => (OperationDetailKind::Edit, turn_id, operation_id),
            PrivateEvidenceKind::CodexCommandDetail {
                turn_id,
                operation_id,
            }
            | PrivateEvidenceKind::CodexFailedCommandDetail {
                turn_id,
                operation_id,
            } => (OperationDetailKind::Command, turn_id, operation_id),
        };
        Some((
            kind,
            CodexOperationSelection {
                event_id: &self.event_id,
                revision: self.revision,
                session_id: &self.session_id,
                turn_id: Some(turn_id),
                operation_id,
            },
        ))
    }
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum PrivateEvidenceFailure {
    Unauthorized,
    HistoryReplaced,
    SnapshotExpired,
    Unavailable,
}

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct EvidenceFrame<T = String> {
    pub(crate) challenge: [u8; 16],
    pub(crate) payload: T,
}

#[derive(Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum DeliveryCompletion {
    Acknowledged,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ConfigFile {
    version: u16,
    sources: Vec<ResolvedSource>,
}

pub(crate) struct PrivateConfig {
    path: PathBuf,
    uid: u32,
}

impl PrivateConfig {
    pub(crate) fn validate(
        path: PathBuf,
        uid: u32,
        allowed: &BTreeSet<ExtensionId>,
    ) -> io::Result<Self> {
        let config = Self { path, uid };
        config.read(allowed)?;
        Ok(config)
    }

    fn read(&self, allowed: &BTreeSet<ExtensionId>) -> io::Result<Vec<ResolvedSource>> {
        let invalid = || {
            io::Error::new(
                io::ErrorKind::InvalidData,
                "native private-content configuration is invalid",
            )
        };
        if !direct_path(&self.path) || fs::canonicalize(&self.path)? != self.path {
            return Err(invalid());
        }
        crate::linux::secure_ancestors(&self.path)?;
        let file = OpenOptions::new()
            .read(true)
            .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK)
            .open(&self.path)?;
        let metadata = file.metadata()?;
        if !metadata.is_file()
            || metadata.nlink() != 1
            || metadata.uid() != self.uid
            || metadata.mode() & 0o7777 != 0o600
            || metadata.len() > CONFIG_BYTES
        {
            return Err(invalid());
        }
        let mut bytes = Vec::new();
        (&file).take(CONFIG_BYTES + 1).read_to_end(&mut bytes)?;
        let published = fs::symlink_metadata(&self.path)?;
        if bytes.len() as u64 > CONFIG_BYTES || file_stamp(&metadata) != file_stamp(&published) {
            return Err(invalid());
        }
        let config: ConfigFile = serde_json::from_slice(&bytes).map_err(|_| invalid())?;
        if config.version != 1 || !(1..=64).contains(&config.sources.len()) {
            return Err(invalid());
        }
        let mut seen = BTreeSet::new();
        for source in &config.sources {
            let id = ExtensionId::parse(&source.source_id).map_err(|_| invalid())?;
            if !allowed.contains(&id)
                || !seen.insert(id)
                || ExtensionId::parse(&source.plugin_id).is_err()
                || !direct_path(&source.plugin_host)
                || !direct_path(&source.adapter)
                || !direct_path(&source.source_root)
                || !source.artifact_sha256.as_deref().is_some_and(valid_pin)
                || source.features.as_ref().is_some_and(|features| {
                    features.len() > 64
                        || features.iter().any(|feature| {
                            feature.is_empty() || feature.len() > 128 || !feature.is_ascii()
                        })
                })
            {
                return Err(invalid());
            }
        }
        Ok(config.sources)
    }

    fn resolver<'a>(
        &'a self,
        allowed: &'a BTreeSet<ExtensionId>,
    ) -> impl Fn(&ExtensionId) -> Result<ResolvedSource, String> + 'a {
        // The verifier invokes the resolver again just before disclosure.
        // Keep the pin proof only within this RPC, and re-open the named
        // artifact and compare its full stamp at every resolution.
        let verified = RefCell::new(None);
        move |id| self.resolve(allowed, id, &verified)
    }

    fn resolve(
        &self,
        allowed: &BTreeSet<ExtensionId>,
        source_id: &ExtensionId,
        verified: &RefCell<Option<ArtifactProof>>,
    ) -> Result<ResolvedSource, String> {
        let unavailable = || "broker private source is unavailable".to_owned();
        let source = self
            .read(allowed)
            .map_err(|_| unavailable())?
            .into_iter()
            .find(|source| source.source_id == source_id.as_str())
            .ok_or_else(unavailable)?;
        let artifact = OpenOptions::new()
            .read(true)
            .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK)
            .open(&source.adapter)
            .map_err(|_| unavailable())?;
        let metadata = artifact.metadata().map_err(|_| unavailable())?;
        if !metadata.is_file() {
            return Err(unavailable());
        }
        let hash = source.artifact_sha256.as_deref().ok_or_else(unavailable)?;
        let stamp = file_stamp(&metadata);
        let proof = ArtifactProof {
            path: source.adapter.clone(),
            pin: hash.to_owned(),
            stamp,
        };
        // Reuse the runtime's bounded SHA verifier on the opened inode. The
        // configured artifact path itself remains part of the source binding.
        if verified.borrow().as_ref() != Some(&proof) {
            let pinned = PathBuf::from(format!("/proc/self/fd/{}", artifact.as_raw_fd()));
            agenturm_runtime::verify_artifact_sha256(&pinned, hash).map_err(|_| unavailable())?;
        }
        let named = fs::symlink_metadata(&source.adapter).map_err(|_| unavailable())?;
        if file_stamp(&artifact.metadata().map_err(|_| unavailable())?) != stamp
            || file_stamp(&named) != stamp
        {
            return Err(unavailable());
        }
        verified.replace(Some(proof));
        Ok(source)
    }
}

#[derive(Eq, PartialEq)]
struct ArtifactProof {
    path: PathBuf,
    pin: String,
    stamp: FileStamp,
}

fn direct_path(path: &Path) -> bool {
    path.is_absolute()
        && path.as_os_str().len() <= 4_096
        && path
            .components()
            .all(|part| matches!(part, Component::RootDir | Component::Normal(_)))
}

fn valid_pin(pin: &str) -> bool {
    pin.len() == 64
        && pin
            .bytes()
            .all(|byte| matches!(byte, b'0'..=b'9' | b'a'..=b'f'))
}

type FileStamp = (u64, u64, u32, u32, u64, u64, i64, i64, i64, i64);

fn file_stamp(metadata: &fs::Metadata) -> FileStamp {
    (
        metadata.dev(),
        metadata.ino(),
        metadata.uid(),
        metadata.mode(),
        metadata.nlink(),
        metadata.len(),
        metadata.mtime(),
        metadata.mtime_nsec(),
        metadata.ctime(),
        metadata.ctime_nsec(),
    )
}

pub(crate) fn list(
    store: &Store,
    config: Option<&PrivateConfig>,
    allowed: &BTreeSet<ExtensionId>,
    source_id: &ExtensionId,
    session_id: &str,
) -> Result<Vec<PromptReference>, PrivateEvidenceFailure> {
    let config = config.ok_or(PrivateEvidenceFailure::Unavailable)?;
    agenturm_content::list(store, &config.resolver(allowed), source_id, session_id)
        .map_err(|_| PrivateEvidenceFailure::Unavailable)
}

pub(crate) fn disclose(
    store: &Store,
    config: Option<&PrivateConfig>,
    allowed: &BTreeSet<ExtensionId>,
    selection: &PrivateEvidenceSelection,
    terminal: bool,
    output: &mut dyn Write,
) -> Result<(), PrivateEvidenceFailure> {
    if matches!(
        selection.kind,
        PrivateEvidenceKind::CodexFailedCommandDetail { .. }
    ) {
        return Err(PrivateEvidenceFailure::Unavailable);
    }
    let config = config.ok_or(PrivateEvidenceFailure::Unavailable)?;
    let resolve = config.resolver(allowed);
    if let PrivateEvidenceKind::UserText {
        snapshot_id,
        history_identity,
    } = &selection.kind
    {
        return agenturm_content::reveal_selected_prompt(
            store,
            &resolve,
            &selection.source_id,
            agenturm_content::PromptSelection {
                session_id: Some(&selection.session_id),
                event_id: &selection.event_id,
                revision: selection.revision,
                snapshot_id: *snapshot_id,
                history_identity: history_identity.as_deref(),
            },
            output,
        )
        .map_err(|_| PrivateEvidenceFailure::Unavailable);
    }
    let (kind, operation) = selection
        .operation()
        .ok_or(PrivateEvidenceFailure::Unavailable)?;
    agenturm_content::reveal_operation_fenced(
        store,
        &resolve,
        &selection.source_id,
        kind,
        &operation,
        output,
        terminal,
    )
    .map_err(|_| PrivateEvidenceFailure::Unavailable)
}

pub(crate) fn verify_prompt(
    store: &Store,
    config: Option<&PrivateConfig>,
    allowed: &BTreeSet<ExtensionId>,
    selection: &PrivatePromptSelection,
    output: &mut Disclosure<'_>,
) -> Result<(), PrivateEvidenceFailure> {
    let config = config.ok_or(PrivateEvidenceFailure::Unavailable)?;
    agenturm_content::with_verified_prompt(
        store,
        &config.resolver(allowed),
        &selection.source_id,
        Some(&selection.session_id),
        &selection.event_id,
        selection.revision,
        |prompt| {
            output.publish_proof(
                store,
                selection.snapshot_id,
                (
                    &selection.source_id,
                    &selection.event_id,
                    selection.revision,
                ),
                prompt,
            )
        },
    )
    .map_err(|_| PrivateEvidenceFailure::Unavailable)?
}

pub(crate) fn verify_operation(
    store: &Store,
    config: Option<&PrivateConfig>,
    allowed: &BTreeSet<ExtensionId>,
    selection: &PrivateEvidenceSelection,
    snapshot_id: ReadSnapshot,
    output: &mut Disclosure<'_>,
) -> Result<(), PrivateEvidenceFailure> {
    let config = config.ok_or(PrivateEvidenceFailure::Unavailable)?;
    let (kind, operation) = selection
        .operation()
        .ok_or(PrivateEvidenceFailure::Unavailable)?;
    let resolver = config.resolver(allowed);
    let occurrence = (
        &selection.source_id,
        selection.event_id.as_str(),
        selection.revision,
    );
    match kind {
        OperationDetailKind::Edit => agenturm_content::with_verified_edit(
            store,
            &resolver,
            &selection.source_id,
            &operation,
            |edit| output.publish_proof(store, Some(snapshot_id), occurrence, edit),
        ),
        OperationDetailKind::Command => {
            let verify = if matches!(
                selection.kind,
                PrivateEvidenceKind::CodexFailedCommandDetail { .. }
            ) {
                agenturm_content::with_verified_failed_command
            } else {
                agenturm_content::with_verified_command
            };
            verify(
                store,
                &resolver,
                &selection.source_id,
                &operation,
                |command: &agenturm_content::VerifiedCommand| {
                    output.publish_proof(store, Some(snapshot_id), occurrence, command)
                },
            )
        }
        OperationDetailKind::Mcp => return Err(PrivateEvidenceFailure::Unavailable),
    }
    .map_err(|_| PrivateEvidenceFailure::Unavailable)?
}

pub(crate) fn disclose_telemetry(
    store: &Store,
    selected: &TelemetryPayloadSelection,
    output: &mut Disclosure<'_>,
) -> Result<(), PrivateEvidenceFailure> {
    finish_capture(store.with_telemetry_payload(selected, |payload| output.publish(payload)))
}

pub(crate) fn disclose_telemetry_log(
    store: &Store,
    selected: &TelemetryLogTextSelection,
    output: &mut Disclosure<'_>,
) -> Result<(), PrivateEvidenceFailure> {
    finish_capture(store.with_telemetry_log_text(selected, |payload| output.publish(payload)))
}

pub(crate) fn disclose_eval_detail(
    store: &Store,
    selected: &EvalDetailSelection,
    output: &mut Disclosure<'_>,
) -> Result<(), PrivateEvidenceFailure> {
    finish_capture(store.with_eval_result_detail(selected, |detail| output.publish(detail)))
}

pub(crate) fn disclose_failed_eval_finding(
    store: &Store,
    selected: &EvalFindingSelection,
    output: &mut Disclosure<'_>,
) -> Result<(), PrivateEvidenceFailure> {
    finish_capture(store.with_failed_eval_finding(selected, |detail| output.publish(detail)))
}

fn finish_capture(
    result: Result<Option<io::Result<()>>, StoreError>,
) -> Result<(), PrivateEvidenceFailure> {
    result
        .map_err(|error| match error {
            StoreError::SnapshotReplaced => PrivateEvidenceFailure::HistoryReplaced,
            StoreError::ExpiredHistorySnapshot { .. } => PrivateEvidenceFailure::SnapshotExpired,
            _ => PrivateEvidenceFailure::Unavailable,
        })?
        .ok_or(PrivateEvidenceFailure::Unavailable)?
        .map_err(|_| PrivateEvidenceFailure::Unavailable)
}

pub(crate) struct Disclosure<'a> {
    stream: &'a mut UnixStream,
    request: &'a native::ReplyIdentity,
    buffer: Vec<u8>,
    published: bool,
    acknowledged: bool,
}

impl<'a> Disclosure<'a> {
    pub(crate) fn new(stream: &'a mut UnixStream, request: &'a native::ReplyIdentity) -> Self {
        Self {
            stream,
            request,
            buffer: Vec::new(),
            published: false,
            acknowledged: false,
        }
    }

    pub(crate) fn complete(self, result: Result<(), PrivateEvidenceFailure>) {
        if result.is_ok() && self.acknowledged {
            let _ = native::send_reply(self.stream, self.request, DeliveryCompletion::Acknowledged);
        } else if !self.published {
            let denied = result.err().unwrap_or(PrivateEvidenceFailure::Unavailable);
            let _ = native::send_reply(self.stream, self.request, Err::<EvidenceFrame, _>(denied));
        }
    }

    fn publish_proof(
        &mut self,
        store: &Store,
        snapshot_id: Option<ReadSnapshot>,
        occurrence: (&ExtensionId, &str, u32),
        payload: impl Serialize,
    ) -> Result<(), PrivateEvidenceFailure> {
        if let Some(snapshot_id) = snapshot_id {
            let (source_id, event_id, revision) = occurrence;
            let retained = store
                .contains_observation_at_snapshot(source_id, event_id, revision, snapshot_id)
                .map_err(|error| match error {
                    StoreError::ExpiredHistorySnapshot { .. } => {
                        PrivateEvidenceFailure::SnapshotExpired
                    }
                    _ => PrivateEvidenceFailure::Unavailable,
                })?;
            if !retained {
                return Err(PrivateEvidenceFailure::Unavailable);
            }
        }
        self.publish(payload)
            .map_err(|_| PrivateEvidenceFailure::Unavailable)
    }

    fn publish<T: Serialize>(&mut self, payload: T) -> io::Result<()> {
        if self.published {
            return Err(io::Error::other(
                "private evidence can only be delivered once",
            ));
        }
        let mut challenge = [0_u8; 16];
        File::open("/dev/urandom")?.read_exact(&mut challenge)?;
        self.published = true;
        native::send_delivery_reply(
            self.stream,
            self.request,
            Ok::<_, PrivateEvidenceFailure>(EvidenceFrame { challenge, payload }),
        )
        .map_err(io::Error::other)?;
        native::receive_delivery_ack(self.stream, self.request, challenge)
            .map_err(io::Error::other)?;
        self.acknowledged = true;
        Ok(())
    }
}

impl Write for Disclosure<'_> {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        if self.published || bytes.len() > MAX_PAYLOAD - self.buffer.len() {
            return Err(io::Error::other("private evidence frame exceeds its bound"));
        }
        self.buffer.extend_from_slice(bytes);
        Ok(bytes.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        if self.published || self.buffer.is_empty() {
            return Err(io::Error::other(
                "private evidence can only be delivered once",
            ));
        }
        let payload = String::from_utf8(std::mem::take(&mut self.buffer))
            .map_err(|_| io::Error::other("invalid private evidence encoding"))?;
        self.publish(payload)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::native_admin::Role;
    use crate::native_store::NativeOperation;
    use agenturm_store::{
        ContentGrantScope, ObservationBatch, ObservationIdentity, ObservationKind,
        StoredObservation, StoredScope,
    };
    use serde_json::{Value, json};
    use sha2::{Digest, Sha256};
    use std::os::unix::fs::{DirBuilderExt, PermissionsExt};
    use std::sync::mpsc;
    use std::thread;
    use std::time::{Duration, SystemTime, UNIX_EPOCH};

    const HEADER: &str = "{\"type\":\"session\",\"id\":\"proof-session\"}\n";
    const BODY: &str = "{\"type\":\"message\",\"id\":\"user-one\",\"timestamp\":\"1970-01-01T00:00:01Z\",\"message\":{\"role\":\"user\",\"content\":\"review note fixture\"}}\n";
    const ARTIFACT: &[u8] = b"fixture adapter pin";

    struct Fixture {
        root: PathBuf,
        config: PrivateConfig,
        sources: BTreeSet<ExtensionId>,
        scope: ContentGrantScope,
        selection: PrivateEvidenceSelection,
    }

    impl Fixture {
        fn new() -> Self {
            Self::with_seed_backup(None)
        }

        fn with_seed_backup(backup: Option<&str>) -> Self {
            let now = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos();
            let root = std::env::temp_dir().join(format!(
                "agenturm-private-broker-{}-{now}",
                std::process::id()
            ));
            fs::DirBuilder::new().mode(0o700).create(&root).unwrap();
            let source = root.join("native.jsonl");
            let source_bytes = format!("{HEADER}{BODY}");
            fs::write(&source, &source_bytes).unwrap();
            let adapter = root.join("adapter");
            fs::write(&adapter, ARTIFACT).unwrap();
            let config_path = root.join("sources.json");
            let source_id = ExtensionId::parse("harness.pi").unwrap();
            let sources = BTreeSet::from([source_id.clone()]);
            fs::write(
                &config_path,
                serde_json::to_vec(&json!({
                    "version": 1,
                    "sources": [{
                        "source_id": source_id,
                        "plugin_id": "example.rust-pi-harness",
                        "plugin_host": root.join("host"),
                        "adapter": adapter,
                        "artifact_sha256": hash(ARTIFACT),
                        "features": ["source-adapter/1", "configured-root/1"],
                        "source_root": source,
                    }],
                }))
                .unwrap(),
            )
            .unwrap();
            fs::set_permissions(&config_path, fs::Permissions::from_mode(0o600)).unwrap();
            // SAFETY: geteuid accepts no arguments.
            let uid = unsafe { libc::geteuid() };
            let config = PrivateConfig::validate(config_path, uid, &sources).unwrap();
            let scope = agenturm_content::bind(&config.resolver(&sources), &source_id, None)
                .unwrap()
                .scope()
                .clone();
            let metadata = fs::metadata(&source).unwrap();
            let (device, inode) = (metadata.dev(), metadata.ino());
            let header_hash = hash(HEADER);
            let generation = hash(format!("{device}:{inode}:{header_hash}"));
            let event_id = format!("pi:{generation}:user-one:message");
            let selection = PrivateEvidenceSelection {
                source_id,
                session_id: "proof-session".into(),
                event_id,
                revision: 0,
                kind: PrivateEvidenceKind::UserText {
                    snapshot_id: None,
                    history_identity: None,
                },
            };
            let fixture = Self {
                root,
                config,
                sources,
                scope,
                selection,
            };
            let mut store = fixture.open_store();
            if let Some(backup) = backup {
                store.ensure_history_identity().unwrap();
                store.backup_to(fixture.root.join(backup)).unwrap();
            }
            fixture.admit_prompt(&mut store, &source_bytes);
            store.grant_content(&fixture.scope).unwrap();
            fixture
        }

        fn admit_prompt(&self, store: &mut Store, source_bytes: &str) {
            let metadata = fs::metadata(self.root.join("native.jsonl")).unwrap();
            let (device, inode) = (metadata.dev(), metadata.ino());
            let header_hash = hash(HEADER);
            let generation = hash(format!("{device}:{inode}:{header_hash}"));
            let checkpoint = format!(
                "v2:{device}:{inode}:{}:{}:{header_hash}:{generation}:",
                source_bytes.len(),
                hash(source_bytes)
            );
            let batch = ObservationBatch::new_deterministic(
                self.selection.source_id.as_str(),
                None,
                checkpoint,
                vec![StoredObservation {
                    event_id: self.selection.event_id.clone(),
                    revision: self.selection.revision,
                    identity: ObservationIdentity {
                        scope: StoredScope::Session,
                        session_id: Some(self.selection.session_id.clone()),
                        worker_id: None,
                        turn_id: Some("user-one".into()),
                        operation_id: None,
                    },
                    observed_at_unix_ns: 1_000_000_000,
                    kind: ObservationKind::Message {
                        role: "user".into(),
                    },
                }],
            )
            .unwrap();
            store.commit_batch(&batch).unwrap();
        }

        fn open_store(&self) -> Store {
            Store::open_collector(self.root.join("history.sqlite")).unwrap()
        }

        fn disclose(
            &self,
            store: &Store,
            output: &mut dyn Write,
        ) -> Result<(), PrivateEvidenceFailure> {
            disclose(
                store,
                Some(&self.config),
                &self.sources,
                &self.selection,
                true,
                output,
            )
        }
    }

    impl Drop for Fixture {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.root);
        }
    }

    fn hash(bytes: impl AsRef<[u8]>) -> String {
        format!("{:x}", Sha256::digest(bytes.as_ref()))
    }

    fn read_fixture_frame(stream: &mut UnixStream) -> Value {
        let mut header = [0_u8; 4];
        stream.read_exact(&mut header).unwrap();
        let mut frame = vec![0; u32::from_be_bytes(header) as usize];
        stream.read_exact(&mut frame).unwrap();
        serde_json::from_slice(&frame).unwrap()
    }

    #[test]
    fn issued_prompt_reference_rejects_same_metadata_on_other_history_and_restored_branch() {
        let mut fixture = Fixture::with_seed_backup(Some("restored.sqlite"));
        let list = |fixture: &Fixture, store: &Store| {
            let mut rows = super::list(
                store,
                Some(&fixture.config),
                &fixture.sources,
                &fixture.selection.source_id,
                &fixture.selection.session_id,
            )
            .unwrap();
            assert_eq!(rows.len(), 1);
            rows.pop().unwrap()
        };
        let original = fixture.open_store();
        let original_reference = list(&fixture, &original);
        let prior_snapshot = original_reference.snapshot_id.unwrap();
        fixture.selection.kind = PrivateEvidenceKind::UserText {
            snapshot_id: Some(prior_snapshot),
            history_identity: original_reference.history_identity.clone(),
        };
        let prior_selection = fixture.selection.clone();
        let mut output = Vec::new();
        fixture.disclose(&original, &mut output).unwrap();
        assert_eq!(output, b"\"review note fixture\"\n");

        let alternate_bytes = format!(
            "{HEADER}{}",
            BODY.replace("review note fixture", "alternate prompt")
        );
        fs::write(fixture.root.join("native.jsonl"), &alternate_bytes).unwrap();
        let mut restored = Store::open_collector(fixture.root.join("restored.sqlite")).unwrap();
        fixture.admit_prompt(&mut restored, &alternate_bytes);
        restored.grant_content(&fixture.scope).unwrap();
        let new_reference = list(&fixture, &restored);
        let new_snapshot = new_reference.snapshot_id.unwrap();
        assert_eq!(prior_snapshot.watermark(), new_snapshot.watermark());
        assert_ne!(prior_snapshot.incarnation(), new_snapshot.incarnation());
        assert_eq!(
            PromptReference {
                snapshot_id: None,
                ..original_reference
            },
            PromptReference {
                snapshot_id: None,
                ..new_reference
            },
        );
        output.clear();
        assert_eq!(
            fixture.disclose(&restored, &mut output),
            Err(PrivateEvidenceFailure::Unavailable)
        );
        assert!(output.is_empty());
        fixture.selection.kind = PrivateEvidenceKind::UserText {
            snapshot_id: Some(new_snapshot),
            history_identity: Some(restored.history_identity().unwrap()),
        };
        fixture.disclose(&restored, &mut output).unwrap();
        assert_eq!(output, b"\"alternate prompt\"\n");

        let mut other = Store::open_collector(fixture.root.join("other.sqlite")).unwrap();
        fixture.admit_prompt(&mut other, &alternate_bytes);
        other.grant_content(&fixture.scope).unwrap();
        let other_reference = list(&fixture, &other);
        assert_eq!(other_reference.event_id, prior_selection.event_id);
        assert_eq!(other_reference.revision, prior_selection.revision);
        assert_eq!(other_reference.snapshot_id, Some(prior_snapshot));
        assert_ne!(
            other.history_identity().unwrap(),
            original.history_identity().unwrap()
        );
        fixture.selection = prior_selection;
        output.clear();
        assert_eq!(
            fixture.disclose(&other, &mut output),
            Err(PrivateEvidenceFailure::Unavailable)
        );
        assert!(output.is_empty());
        fixture.selection.kind = PrivateEvidenceKind::UserText {
            snapshot_id: other_reference.snapshot_id,
            history_identity: other_reference.history_identity,
        };
        fixture.disclose(&other, &mut output).unwrap();
        assert_eq!(output, b"\"alternate prompt\"\n");
    }

    #[test]
    fn verified_private_delivery_keeps_revocation_serialized_until_ack() {
        let fixture = Fixture::new();
        let authorizer = fixture.open_store();
        let mut revoker = fixture.open_store();
        let (mut service, mut client) = UnixStream::pair().unwrap();
        let identity: native::ReplyIdentity = serde_json::from_value(json!({
            "version": 1, "request_id": "0123456789abcdef0123456789abcdef",
            "receiver_id": "receiver:test", "request_sha256": vec![0; 32],
        }))
        .unwrap();
        thread::scope(|threads| {
            let fixture = &fixture;
            let delivered = threads.spawn(move || {
                let mut output = Disclosure::new(&mut service, &identity);
                let result = fixture.disclose(&authorizer, &mut output);
                let completed = result.is_ok();
                output.complete(result);
                completed
            });
            client
                .set_read_timeout(Some(Duration::from_secs(2)))
                .unwrap();
            let frame = read_fixture_frame(&mut client);
            assert_eq!(
                frame["result"]["Ok"]["payload"],
                "\"review note fixture\"\n"
            );
            let (pending, started) = mpsc::channel();
            let (finished, completion) = mpsc::channel();
            threads.spawn(move || {
                pending.send(()).unwrap();
                finished
                    .send(revoker.revoke_bound_content(&fixture.scope))
                    .unwrap();
            });
            started.recv_timeout(Duration::from_secs(1)).unwrap();
            assert!(matches!(
                completion.recv_timeout(Duration::from_millis(100)),
                Err(mpsc::RecvTimeoutError::Timeout)
            ));
            let ack = serde_json::to_vec(&json!({
                "identity": frame["identity"], "challenge": frame["result"]["Ok"]["challenge"],
            }))
            .unwrap();
            client.write_all(&(ack.len() as u32).to_be_bytes()).unwrap();
            client.write_all(&ack).unwrap();
            client.shutdown(std::net::Shutdown::Write).unwrap();
            assert_eq!(read_fixture_frame(&mut client)["result"], "acknowledged");
            assert!(delivered.join().unwrap());
            assert!(
                completion
                    .recv_timeout(Duration::from_secs(2))
                    .unwrap()
                    .unwrap()
            );
        });
        let store = fixture.open_store();
        let mut output = Vec::new();
        assert_eq!(
            fixture.disclose(&store, &mut output),
            Err(PrivateEvidenceFailure::Unavailable)
        );
        assert!(output.is_empty());
    }

    #[test]
    fn typed_prompt_requires_bound_reader_and_disconnect_releases_fence() {
        let fixture = Fixture::new();
        let mut authorizer = fixture.open_store();
        let history = authorizer.ensure_history_identity().unwrap();
        let old_snapshot = authorizer
            .query_observations(&agenturm_kernel::ObservationQuery::latest(1))
            .unwrap()
            .snapshot_id;
        // The grant and exact row survive; the old snapshot does not.
        assert_eq!(
            authorizer
                .prune_observations_before(1)
                .unwrap()
                .removed_observations,
            0
        );
        let operation = |history_identity, session_id: &str, snapshot_id| {
            NativeOperation::VerifyPrivatePrompt {
                history_identity,
                selection: PrivatePromptSelection {
                    source_id: fixture.selection.source_id.clone(),
                    session_id: session_id.into(),
                    event_id: fixture.selection.event_id.clone(),
                    revision: fixture.selection.revision,
                    snapshot_id,
                },
            }
        };
        let identity: native::ReplyIdentity = serde_json::from_value(json!({
            "version": 1, "request_id": "0123456789abcdef0123456789abcdef",
            "receiver_id": "receiver:test", "request_sha256": vec![0; 32],
        }))
        .unwrap();
        for (role, request, failure) in [
            (
                Role::Collector,
                operation(history.clone(), "proof-session", None),
                PrivateEvidenceFailure::Unauthorized,
            ),
            (
                Role::Reader,
                operation("f".repeat(32), "proof-session", None),
                PrivateEvidenceFailure::HistoryReplaced,
            ),
            (
                Role::Reader,
                operation(history.clone(), "another-session", None),
                PrivateEvidenceFailure::Unavailable,
            ),
            (
                Role::Reader,
                operation(history.clone(), "proof-session", Some(old_snapshot)),
                PrivateEvidenceFailure::SnapshotExpired,
            ),
        ] {
            let (mut service, mut client) = UnixStream::pair().unwrap();
            let mut output = Disclosure::new(&mut service, &identity);
            let result = request.deliver_private(
                &authorizer,
                &fixture.sources,
                role,
                Some(&fixture.config),
                &[],
                &mut output,
            );
            assert_eq!(result, Err(failure));
            output.complete(result);
            let frame = read_fixture_frame(&mut client);
            assert_eq!(frame["result"], json!({ "Err": failure }));
        }

        let (mut service, mut client) = UnixStream::pair().unwrap();
        client
            .set_read_timeout(Some(Duration::from_secs(2)))
            .unwrap();
        let mut revoker = fixture.open_store();
        let request = operation(history, "proof-session", None);
        thread::scope(|threads| {
            let fixture = &fixture;
            let delivery = threads.spawn(move || {
                let mut output = Disclosure::new(&mut service, &identity);
                let result = request.deliver_private(
                    &authorizer,
                    &fixture.sources,
                    Role::Reader,
                    Some(&fixture.config),
                    &[],
                    &mut output,
                );
                output.complete(result);
                result
            });
            let frame = read_fixture_frame(&mut client);
            let prompt: agenturm_content::VerifiedPrompt =
                serde_json::from_value(frame["result"]["Ok"]["payload"].clone()).unwrap();
            assert_eq!(prompt.text, "review note fixture");
            assert_eq!(prompt.session_id, "proof-session");
            assert_eq!(prompt.turn_id, "user-one");
            assert_eq!(prompt.observed_at_unix_ns, 1_000_000_000);
            assert_ne!(prompt.grant_binding, [0; 32]);
            let (finished, completion) = mpsc::channel();
            threads.spawn(move || {
                finished
                    .send(revoker.revoke_bound_content(&fixture.scope))
                    .unwrap();
            });
            assert!(matches!(
                completion.recv_timeout(Duration::from_millis(100)),
                Err(mpsc::RecvTimeoutError::Timeout)
            ));
            drop(client);
            assert_eq!(
                delivery.join().unwrap(),
                Err(PrivateEvidenceFailure::Unavailable)
            );
            assert!(
                completion
                    .recv_timeout(Duration::from_secs(2))
                    .unwrap()
                    .unwrap()
            );
        });
    }

    #[test]
    fn private_resolution_denies_source_artifact_and_config_replacement() {
        let fixture = Fixture::new();
        let store = fixture.open_store();
        assert_eq!(
            list(
                &store,
                Some(&fixture.config),
                &fixture.sources,
                &fixture.selection.source_id,
                "proof-session"
            )
            .unwrap()
            .len(),
            1
        );
        let mut output = Vec::new();
        let source = fixture.root.join("native.jsonl");
        fs::write(
            &source,
            format!(
                "{HEADER}{}",
                BODY.replace("review note fixture", "edited fixture")
            ),
        )
        .unwrap();
        assert_eq!(
            fixture.disclose(&store, &mut output),
            Err(PrivateEvidenceFailure::Unavailable)
        );
        fs::write(&source, format!("{HEADER}{BODY}")).unwrap();
        let adapter = fixture.root.join("adapter");
        let resolve = fixture.config.resolver(&fixture.sources);
        assert!(resolve(&fixture.selection.source_id).is_ok());
        fs::write(&adapter, b"changed artifact").unwrap();
        assert!(resolve(&fixture.selection.source_id).is_err());
        assert_eq!(
            fixture.disclose(&store, &mut output),
            Err(PrivateEvidenceFailure::Unavailable)
        );
        fs::write(&adapter, ARTIFACT).unwrap();
        fs::set_permissions(&fixture.config.path, fs::Permissions::from_mode(0o640)).unwrap();
        assert_eq!(
            fixture.disclose(&store, &mut output),
            Err(PrivateEvidenceFailure::Unavailable)
        );
        fs::set_permissions(&fixture.config.path, fs::Permissions::from_mode(0o600)).unwrap();
        let replacement = fixture.root.join("replacement.jsonl");
        fs::copy(&source, &replacement).unwrap();
        let mut config: Value =
            serde_json::from_slice(&fs::read(&fixture.config.path).unwrap()).unwrap();
        config["sources"][0]["source_root"] = json!(replacement);
        fs::write(&fixture.config.path, serde_json::to_vec(&config).unwrap()).unwrap();
        assert_eq!(
            fixture.disclose(&store, &mut output),
            Err(PrivateEvidenceFailure::Unavailable)
        );
        assert!(output.is_empty());
    }
}

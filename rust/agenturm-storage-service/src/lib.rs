//! Explicit Linux storage-service endpoints.
//!
//! Default mode serves only the v1 protocol probe with an empty private data
//! root. An explicit experimental mode admits v2 lifecycle requests only to
//! marked disposable databases. The separate opt-in native endpoint admits
//! Rust history for configured sources.

#![forbid(unsafe_op_in_unsafe_fn)]

#[cfg(target_os = "linux")]
mod native_admin;
pub mod native_admission;
#[cfg(target_os = "linux")]
mod native_content;
pub mod native_read;
#[cfg(target_os = "linux")]
mod native_store;
#[cfg(target_os = "linux")]
mod reader_history;
#[cfg(target_os = "linux")]
pub use native_admin::{AdminFailure, BackupReceipt, GrantBinding, GrantStatus};
#[cfg(target_os = "linux")]
pub use native_content::{PrivateEvidenceFailure, PrivateEvidenceKind, PrivateEvidenceSelection};
#[cfg(target_os = "linux")]
pub use native_store::{
    NativeClientError, NativeSelection, NativeStore, ProjectedHistoryScope, logical_read_identity,
};
#[cfg(target_os = "linux")]
pub use reader_history::{ReaderHistoryReply, ReaderHistoryRequest};

#[cfg(target_os = "linux")]
mod linux {
    use crate::native_admin::Role;
    use crate::native_content::{Disclosure, PrivateConfig};
    use crate::native_store::{self, NativeOperation};
    use agenturm_kernel::ExtensionId;
    use agenturm_storage_service_contract::native;
    use agenturm_storage_service_contract::{
        PeerPolicy, ProbeTopic, Request, read_authorized_request, read_authorized_v2_request,
        send_probe_reply, send_v2_reply,
    };
    use agenturm_store::{Store, TelemetryBinding};
    use std::collections::BTreeSet;
    use std::ffi::OsStr;
    use std::fs::{self, File, Metadata, OpenOptions};
    use std::io::{self, ErrorKind};
    use std::os::fd::AsRawFd;
    use std::os::unix::fs::{FileTypeExt, MetadataExt, OpenOptionsExt, PermissionsExt};
    use std::os::unix::net::UnixListener;
    use std::path::{Component, Path, PathBuf};
    use std::sync::atomic::{AtomicBool, Ordering};

    const SOCKET_NAME: &str = "agenturm-storage-v1.sock";
    const LOCK_NAME: &str = ".agenturm-storage-v1.lock";
    const NATIVE_LOCK_NAME: &str = ".agenturm-native-storage-v1.lock";
    const NATIVE_DB: &str = "history.sqlite";
    const SCRATCH_NAME: &str = "scratch.db";
    const CLONE_NAME: &str = "telemetry-clone.db";
    const SCRATCH_FILES: &[&str] = &[
        SCRATCH_NAME,
        ".scratch.db.agenturm-writer.lock",
        "scratch.db-journal",
        "scratch.db-wal",
        "scratch.db-shm",
    ];
    const CLONE_FILES: &[&str] = &[
        CLONE_NAME,
        ".telemetry-clone.db.agenturm-writer.lock",
        "telemetry-clone.db-journal",
        "telemetry-clone.db-wal",
        "telemetry-clone.db-shm",
    ];
    const NATIVE_FILES: &[&str] = &[
        NATIVE_DB,
        ".history.sqlite.agenturm-writer.lock",
        "history.sqlite-journal",
        "history.sqlite-wal",
        "history.sqlite-shm",
    ];
    static STOP: AtomicBool = AtomicBool::new(false);

    fn denied(reason: &'static str) -> io::Error {
        io::Error::new(ErrorKind::PermissionDenied, reason)
    }

    fn current_uid() -> u32 {
        // SAFETY: geteuid has no pointers or caller preconditions.
        unsafe { libc::geteuid() }
    }

    #[derive(Clone, Debug)]
    pub struct ServiceConfig {
        pub data_root: PathBuf,
        pub transport_dir: PathBuf,
        pub receiver_id: String,
        pub client_uid: u32,
        pub client_gid: u32,
        pub max_connections: Option<usize>,
        pub experimental_v2_scratch: bool,
        pub experimental_real_clone_activation: bool,
        pub native_sources: BTreeSet<ExtensionId>,
        pub native_telemetry_bindings: Vec<TelemetryBinding>,
        pub native_roles: NativeRolesConfig,
        pub native_restore: Option<String>,
        pub native_content_config: Option<PathBuf>,
    }

    #[derive(Clone, Copy, Debug)]
    pub struct ClientIdentity {
        pub uid: u32,
        pub gid: u32,
    }

    #[derive(Clone, Debug, Default)]
    pub struct NativeRolesConfig {
        pub reader: Option<ClientIdentity>,
        pub grant_administrator: Option<ClientIdentity>,
        pub maintenance: Option<ClientIdentity>,
        pub backup_root: Option<PathBuf>,
    }

    #[derive(Clone, Copy, Debug, Eq, PartialEq)]
    enum RootKind {
        DataProbe,
        DataScratch,
        DataClone,
        DataNative,
        Transport,
        Backup,
    }

    #[derive(Clone)]
    struct RootProof {
        path: PathBuf,
        kind: RootKind,
        identity: (u64, u64),
        client_gid: u32,
        database_identity: Option<(u64, u64)>,
    }

    struct InstanceGuard {
        file: File,
        path: PathBuf,
        transport: RootProof,
        identity: (u64, u64),
    }

    impl InstanceGuard {
        fn acquire(transport: RootProof, lock_name: &str) -> io::Result<Self> {
            let path = transport.path.join(lock_name);
            match fs::symlink_metadata(&path) {
                Ok(metadata) if !metadata.is_file() => {
                    return Err(denied("service instance lock is not a regular file"));
                }
                Ok(_) => {}
                Err(error) if error.kind() == ErrorKind::NotFound => {}
                Err(error) => return Err(error),
            }
            let file = OpenOptions::new()
                .read(true)
                .write(true)
                .create(true)
                .mode(0o600)
                .custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC | libc::O_NONBLOCK)
                .open(&path)?;
            let metadata = file.metadata()?;
            let guard = Self {
                file,
                path,
                transport,
                identity: (metadata.dev(), metadata.ino()),
            };
            guard.verify()?;
            guard.file.try_lock().map_err(|error| match error {
                fs::TryLockError::WouldBlock => denied("another service instance holds the lock"),
                fs::TryLockError::Error(error) => error,
            })?;
            guard.verify()?;
            Ok(guard)
        }

        fn verify(&self) -> io::Result<()> {
            self.transport.verify()?;
            let opened = self.file.metadata()?;
            let named = fs::symlink_metadata(&self.path)?;
            if !opened.is_file()
                || !named.is_file()
                || (opened.dev(), opened.ino()) != self.identity
                || (named.dev(), named.ino()) != self.identity
                || opened.nlink() != 1
                || opened.uid() != current_uid()
                || opened.mode() & 0o7777 != 0o600
            {
                return Err(denied("service instance lock changed"));
            }
            Ok(())
        }
    }

    pub(crate) fn secure_ancestors(path: &Path) -> io::Result<()> {
        for ancestor in path.ancestors().skip(1) {
            let metadata = fs::symlink_metadata(ancestor)?;
            if !metadata.is_dir() || metadata.file_type().is_symlink() {
                return Err(denied("service root ancestor is not a direct directory"));
            }
            let mode = metadata.mode();
            if mode & 0o022 != 0 && !(metadata.uid() == 0 && mode & 0o1000 != 0) {
                return Err(denied("service root ancestor permits replacement"));
            }
        }
        Ok(())
    }

    fn direct_directory(path: &Path) -> io::Result<Metadata> {
        if !path.is_absolute()
            || path
                .components()
                .any(|part| !matches!(part, Component::RootDir | Component::Normal(_)))
            || fs::canonicalize(path)? != path
        {
            return Err(denied("service root must be a direct absolute directory"));
        }
        secure_ancestors(path)?;
        let metadata = fs::symlink_metadata(path)?;
        if !metadata.is_dir() || metadata.file_type().is_symlink() {
            return Err(denied("service root must be an existing directory"));
        }
        Ok(metadata)
    }

    impl RootKind {
        fn database_files(self) -> &'static [&'static str] {
            match self {
                Self::DataScratch => SCRATCH_FILES,
                Self::DataClone => CLONE_FILES,
                Self::DataNative => NATIVE_FILES,
                Self::DataProbe | Self::Transport | Self::Backup => &[],
            }
        }
    }

    impl RootProof {
        fn capture(path: &Path, kind: RootKind, client_gid: u32) -> io::Result<Self> {
            let metadata = direct_directory(path)?;
            Self::check(&metadata, path, kind, client_gid)?;
            let database_identity = if let Some(name) = kind.database_files().first() {
                match fs::symlink_metadata(path.join(name)) {
                    Ok(database) => Some((database.dev(), database.ino())),
                    Err(error)
                        if kind == RootKind::DataNative && error.kind() == ErrorKind::NotFound =>
                    {
                        None
                    }
                    Err(error) => return Err(error),
                }
            } else {
                None
            };
            Ok(Self {
                path: path.to_path_buf(),
                kind,
                identity: (metadata.dev(), metadata.ino()),
                client_gid,
                database_identity,
            })
        }

        fn check(
            metadata: &Metadata,
            path: &Path,
            kind: RootKind,
            client_gid: u32,
        ) -> io::Result<()> {
            if metadata.uid() != current_uid() {
                return Err(denied("service root is not owned by service UID"));
            }
            match kind {
                RootKind::DataProbe
                | RootKind::DataScratch
                | RootKind::DataClone
                | RootKind::DataNative => {
                    if metadata.mode() & 0o7777 != 0o700 {
                        return Err(denied("data root must be private 0700"));
                    }
                    let allowed = kind.database_files();
                    let mut saw_database = false;
                    let mut saw_files = false;
                    for entry in fs::read_dir(path)? {
                        let entry = entry?;
                        if kind == RootKind::DataProbe {
                            return Err(denied("probe data root must be empty"));
                        }
                        let name = entry.file_name();
                        if !allowed.iter().any(|allowed| name == OsStr::new(allowed)) {
                            return Err(denied("data root has an unexpected entry"));
                        }
                        let file = fs::symlink_metadata(entry.path())?;
                        if !file.is_file()
                            || file.nlink() != 1
                            || file.uid() != current_uid()
                            || file.mode() & 0o7777 != 0o600
                        {
                            return Err(denied("database file ownership or mode changed"));
                        }
                        saw_files = true;
                        saw_database |= name == OsStr::new(allowed[0]);
                    }
                    if !saw_database
                        && (saw_files
                            || matches!(kind, RootKind::DataScratch | RootKind::DataClone))
                    {
                        return Err(denied("database is missing"));
                    }
                }
                RootKind::Transport => {
                    if metadata.gid() != client_gid || metadata.mode() & 0o7777 != 0o2710 {
                        return Err(denied(
                            "transport directory must be service-owned 2710 with client group",
                        ));
                    }
                }
                RootKind::Backup => {
                    if metadata.mode() & 0o7777 != 0o700 {
                        return Err(denied("backup root must be private 0700"));
                    }
                }
            }
            Ok(())
        }

        fn verify(&self) -> io::Result<()> {
            let metadata = direct_directory(&self.path)?;
            if (metadata.dev(), metadata.ino()) != self.identity {
                return Err(denied("service root identity changed"));
            }
            Self::check(&metadata, &self.path, self.kind, self.client_gid)?;
            if let Some(expected) = self.database_identity {
                let name = self.kind.database_files()[0];
                let current = fs::symlink_metadata(self.path.join(name))?;
                if (current.dev(), current.ino()) != expected {
                    return Err(denied("database identity changed"));
                }
            }
            Ok(())
        }
    }

    struct SocketGuard {
        path: PathBuf,
        transport: RootProof,
        identity: (u64, u64),
        client_gid: u32,
        remove_on_drop: bool,
    }

    impl SocketGuard {
        fn capture(path: PathBuf, transport: RootProof, client_gid: u32) -> io::Result<Self> {
            let mut guard = Self::inspect(path, transport, client_gid)?;
            guard.remove_on_drop = true;
            Ok(guard)
        }

        fn inspect(path: PathBuf, transport: RootProof, client_gid: u32) -> io::Result<Self> {
            let metadata = fs::symlink_metadata(&path)?;
            let guard = Self {
                path,
                transport,
                identity: (metadata.dev(), metadata.ino()),
                client_gid,
                remove_on_drop: false,
            };
            guard.verify()?;
            Ok(guard)
        }

        fn verify(&self) -> io::Result<()> {
            self.transport.verify()?;
            let metadata = fs::symlink_metadata(&self.path)?;
            if !metadata.file_type().is_socket()
                || (metadata.dev(), metadata.ino()) != self.identity
                || metadata.uid() != current_uid()
                || metadata.gid() != self.client_gid
                || metadata.permissions().mode() & 0o7777 != 0o660
            {
                return Err(denied("service socket identity or permissions changed"));
            }
            Ok(())
        }

        fn still_our_pathname(&self) -> bool {
            let Ok(directory) = fs::symlink_metadata(&self.transport.path) else {
                return false;
            };
            let Ok(socket) = fs::symlink_metadata(&self.path) else {
                return false;
            };
            directory.is_dir()
                && !directory.file_type().is_symlink()
                && (directory.dev(), directory.ino()) == self.transport.identity
                && socket.file_type().is_socket()
                && (socket.dev(), socket.ino()) == self.identity
                && socket.uid() == current_uid()
        }
    }

    impl Drop for SocketGuard {
        fn drop(&mut self) {
            // Never unlink an unrelated or replacement pathname.
            if self.remove_on_drop && self.still_our_pathname() {
                let _ = fs::remove_file(&self.path);
            }
        }
    }

    fn recover_native_socket(
        path: &Path,
        transport: &RootProof,
        instance: &InstanceGuard,
        client_gid: u32,
    ) -> io::Result<()> {
        instance.verify()?;
        let previous = match SocketGuard::inspect(path.to_owned(), transport.clone(), client_gid) {
            Ok(socket) => socket,
            Err(error) if error.kind() == ErrorKind::NotFound => return Ok(()),
            Err(error) => return Err(error),
        };
        if !matches!(native::connect(path), Err(error) if error.raw_os_error() == Some(libc::ECONNREFUSED))
        {
            return Err(denied("native service socket may still be active"));
        }
        instance.verify()?;
        previous.verify()?;
        fs::remove_file(path)
    }

    struct UmaskGuard(libc::mode_t);

    impl UmaskGuard {
        fn private_files() -> Self {
            // SAFETY: this is a dedicated, single-threaded service process.
            // Keep scratch sidecars private even when the launcher has umask 000.
            Self(unsafe { libc::umask(0o077) })
        }

        fn socket_mode() -> Self {
            // SAFETY: umask affects only this single-threaded service process;
            // its previous value is restored immediately after bind.
            Self(unsafe { libc::umask(0o117) })
        }
    }

    impl Drop for UmaskGuard {
        fn drop(&mut self) {
            // SAFETY: restore the process umask saved by socket_mode.
            unsafe { libc::umask(self.0) };
        }
    }

    extern "C" fn stop_handler(_signal: libc::c_int) {
        STOP.store(true, Ordering::Relaxed);
    }

    fn install_shutdown_handlers() -> io::Result<()> {
        // SAFETY: sigaction is fully initialized, handler only performs an
        // atomic store, and no SA_RESTART lets accept return Interrupted.
        unsafe {
            let mut action: libc::sigaction = std::mem::zeroed();
            action.sa_sigaction = stop_handler as *const () as usize;
            action.sa_flags = 0;
            if libc::sigemptyset(&mut action.sa_mask) != 0
                || libc::sigaction(libc::SIGINT, &action, std::ptr::null_mut()) != 0
                || libc::sigaction(libc::SIGTERM, &action, std::ptr::null_mut()) != 0
            {
                return Err(io::Error::last_os_error());
            }
        }
        Ok(())
    }

    fn validate(config: &ServiceConfig) -> io::Result<(RootProof, RootProof, PeerPolicy)> {
        let valid_receiver = Request::probe(
            "00000000000000000000000000000000".to_owned(),
            config.receiver_id.clone(),
            ProbeTopic::Protocol,
        )
        .is_ok();
        let active_modes = usize::from(config.experimental_v2_scratch)
            + usize::from(config.experimental_real_clone_activation)
            + usize::from(!config.native_sources.is_empty());
        if !valid_receiver
            || config.max_connections == Some(0)
            || active_modes > 1
            || config.native_sources.len() > 64
        {
            return Err(denied("invalid service receiver or connection bound"));
        }
        let mut payload_sources = BTreeSet::new();
        if config.native_telemetry_bindings.iter().any(|binding| {
            binding.grant_scope().is_err()
                || !config.native_sources.contains(&binding.source_id)
                || binding
                    .native_source_id
                    .as_ref()
                    .is_some_and(|source| !config.native_sources.contains(source))
                || !payload_sources.insert(&binding.source_id)
        }) {
            return Err(denied("invalid native telemetry payload source binding"));
        }
        if config.native_restore.is_some()
            && (config.native_sources.is_empty() || config.native_roles.backup_root.is_none())
        {
            return Err(denied("restore requires native sources and backup root"));
        }
        let policy = PeerPolicy::for_current_service(config.client_uid, config.client_gid)
            .map_err(|_| {
                denied("service and client identities must be distinct non-root accounts")
            })?;
        let data = RootProof::capture(
            &config.data_root,
            if config.experimental_v2_scratch {
                RootKind::DataScratch
            } else if config.experimental_real_clone_activation {
                RootKind::DataClone
            } else if !config.native_sources.is_empty() {
                RootKind::DataNative
            } else {
                RootKind::DataProbe
            },
            config.client_gid,
        )?;
        let transport = RootProof::capture(
            &config.transport_dir,
            RootKind::Transport,
            config.client_gid,
        )?;
        if data.path.starts_with(&transport.path) || transport.path.starts_with(&data.path) {
            return Err(denied("data and transport roots must be separate"));
        }
        Ok((data, transport, policy))
    }

    fn native_role_policies(
        config: &ServiceConfig,
        collector: PeerPolicy,
    ) -> io::Result<Vec<(Role, PeerPolicy)>> {
        let mut users = BTreeSet::from([config.client_uid]);
        let mut policies = vec![(Role::Collector, collector)];
        let roles = &config.native_roles;
        for (role, identity) in [
            (Role::Reader, roles.reader),
            (Role::GrantAdministrator, roles.grant_administrator),
            (Role::Maintenance, roles.maintenance),
        ] {
            if let Some(identity) = identity {
                if config.native_sources.is_empty() || !users.insert(identity.uid) {
                    return Err(denied("native roles require distinct client UIDs"));
                }
                let policy = PeerPolicy::for_current_service(identity.uid, identity.gid)
                    .map_err(|_| denied("invalid native role identity"))?;
                policies.push((role, policy));
            }
        }
        Ok(policies)
    }

    fn native_backup_root(
        config: &ServiceConfig,
        data: &RootProof,
        transport: &RootProof,
    ) -> io::Result<Option<RootProof>> {
        let Some(path) = &config.native_roles.backup_root else {
            return Ok(None);
        };
        if config.native_roles.maintenance.is_none() {
            return Err(denied("backup root requires a native maintenance identity"));
        }
        let backup = RootProof::capture(path, RootKind::Backup, config.client_gid)?;
        if [&data.path, &transport.path]
            .into_iter()
            .any(|root| backup.path.starts_with(root) || root.starts_with(&backup.path))
        {
            return Err(denied(
                "backup root must be separate from history and transport",
            ));
        }
        Ok(Some(backup))
    }

    fn offline_restore(
        data: &RootProof,
        backup: Option<&RootProof>,
        instance: &InstanceGuard,
        name: &str,
    ) -> io::Result<()> {
        use crate::native_admin::valid_backup_name;

        let backup =
            backup.ok_or_else(|| denied("restore requires a private native backup root"))?;
        if !valid_backup_name(name) {
            return Err(denied("restore requires a broker-assigned backup name"));
        }
        if data.kind != RootKind::DataNative || data.database_identity.is_some() {
            return Err(denied("restore requires a fresh private native data root"));
        }
        let source = backup.path.join(name);
        let prove_source = || {
            backup.verify()?;
            let metadata = fs::symlink_metadata(&source)?;
            if !metadata.is_file()
                || metadata.nlink() != 1
                || metadata.uid() != current_uid()
                || metadata.mode() & 0o7777 != 0o600
            {
                return Err(denied(
                    "backup must be a private regular file owned by the broker",
                ));
            }
            for suffix in ["-journal", "-wal", "-shm"] {
                match fs::symlink_metadata(backup.path.join(format!("{name}{suffix}"))) {
                    Ok(_) => return Err(denied("backup must be a detached SQLite file")),
                    Err(error) if error.kind() == ErrorKind::NotFound => {}
                    Err(error) => return Err(error),
                }
            }
            Ok((metadata.dev(), metadata.ino()))
        };
        instance.verify()?;
        data.verify()?;
        let original = prove_source()?;
        let detached = Store::open_read_only(&source)
            .map_err(|_| denied("backup is not a valid native history"))?;
        detached
            .history_identity()
            .map_err(|_| denied("backup has no valid broker history identity"))?;
        if prove_source()? != original {
            return Err(denied("backup identity changed"));
        }
        instance.verify()?;
        data.verify()?;
        detached
            .backup_to(data.path.join(NATIVE_DB))
            .map_err(|_| denied("could not restore a verified native history"))?;
        data.verify()?;
        Ok(())
    }

    pub fn run(config: &ServiceConfig) -> io::Result<()> {
        STOP.store(false, Ordering::Relaxed);
        let (mut data, transport, policy) = validate(config)?;
        let is_native = !config.native_sources.is_empty();
        let policies = native_role_policies(config, policy)?;
        let backup = native_backup_root(config, &data, &transport)?;
        let _private_mask = (config.experimental_v2_scratch
            || config.experimental_real_clone_activation
            || is_native)
            .then(UmaskGuard::private_files);
        let (socket_name, lock_name) = if is_native {
            (native::SOCKET_NAME, NATIVE_LOCK_NAME)
        } else {
            (SOCKET_NAME, LOCK_NAME)
        };
        let instance = InstanceGuard::acquire(transport.clone(), lock_name)?;
        if let Some(name) = &config.native_restore {
            return offline_restore(&data, backup.as_ref(), &instance, name);
        }
        let private = match &config.native_content_config {
            Some(path) if is_native && config.native_roles.reader.is_some() => Some(
                PrivateConfig::validate(path.clone(), current_uid(), &config.native_sources)?,
            ),
            Some(_) => {
                return Err(denied(
                    "private content requires a separate native reader identity",
                ));
            }
            None => None,
        };
        if config.experimental_v2_scratch {
            // SQLite may touch a WAL shared-memory sidecar even for a read.
            // Never inspect the marker until this service owns the lock.
            agenturm_storage_admission::require_marked_scratch_path(&data.path.join(SCRATCH_NAME))
                .map_err(|_| denied("scratch database marker is unavailable"))?;
            data.verify()?;
        } else if config.experimental_real_clone_activation {
            agenturm_storage_admission::require_marked_activation_clone_path(
                &data.path.join(CLONE_NAME),
            )
            .map_err(|_| denied("real-schema activation clone marker unavailable"))?;
            data.verify()?;
        }
        let socket_path = transport.path.join(socket_name);
        if is_native {
            recover_native_socket(&socket_path, &transport, &instance, config.client_gid)?;
        } else {
            match fs::symlink_metadata(&socket_path) {
                Ok(_) => return Err(denied("service socket path already exists")),
                Err(error) if error.kind() == ErrorKind::NotFound => {}
                Err(error) => return Err(error),
            }
        }
        let mut native_store = if is_native {
            data.verify()?;
            let mut store = Store::open_collector(data.path.join(NATIVE_DB))
                .map_err(|_| denied("native history is unavailable"))?;
            if data.database_identity.is_none() {
                let database = fs::symlink_metadata(data.path.join(NATIVE_DB))?;
                data.database_identity = Some((database.dev(), database.ino()));
            }
            data.verify()?;
            store
                .ensure_history_identity()
                .map_err(|_| denied("native history identity is unavailable"))?;
            data.verify()?;
            Some(store)
        } else {
            None
        };
        instance.verify()?;
        install_shutdown_handlers()?;
        let listener = {
            let _mask = UmaskGuard::socket_mode();
            UnixListener::bind(&socket_path)?
        };
        let socket = SocketGuard::capture(socket_path, transport, config.client_gid)?;
        listener.set_nonblocking(true)?;
        let mut accepted = 0_usize;
        let mut retention_schedule = agenturm_store::AutomaticRetentionSchedule::default();
        while !STOP.load(Ordering::Relaxed) {
            if let Some(store) = native_store.as_mut() {
                data.verify()?;
                instance.verify()?;
                if retention_schedule.tick(store).is_err() {
                    eprintln!(
                        "Automatic retention could not run; retained history remains available"
                    );
                }
                data.verify()?;
            }
            let mut ready = libc::pollfd {
                fd: listener.as_raw_fd(),
                events: libc::POLLIN,
                revents: 0,
            };
            // SAFETY: ready points to one initialized pollfd, and the
            // listener retains its FD for the duration of this call.
            let result = unsafe { libc::poll(&mut ready, 1, 500) };
            if result < 0 {
                let error = io::Error::last_os_error();
                if error.kind() == ErrorKind::Interrupted {
                    continue;
                }
                return Err(error);
            }
            if result == 0 {
                continue;
            }
            if ready.revents & (libc::POLLERR | libc::POLLHUP | libc::POLLNVAL) != 0 {
                return Err(denied("service listener readiness failed"));
            }
            if ready.revents & libc::POLLIN == 0 {
                continue;
            }
            let connection = listener.accept();
            let (mut stream, _) = match connection {
                Ok(value) => value,
                Err(error)
                    if error.kind() == ErrorKind::Interrupted && STOP.load(Ordering::Relaxed) =>
                {
                    break;
                }
                Err(error) if error.kind() == ErrorKind::Interrupted => continue,
                Err(error) if error.kind() == ErrorKind::WouldBlock => continue,
                Err(error) => return Err(error),
            };
            accepted += 1;
            stream.set_nonblocking(false)?;
            data.verify()?;
            instance.verify()?;
            socket.verify()?;
            if let Some(store) = native_store.as_mut() {
                if let Ok((role, request, identity)) =
                    native::read_authorized_native_message::<NativeOperation, Role>(
                        &mut stream,
                        &policies,
                        &config.receiver_id,
                        NativeOperation::delivers_private,
                    )
                {
                    data.verify()?;
                    instance.verify()?;
                    socket.verify()?;
                    if request.delivers_private() {
                        let mut output = Disclosure::new(&mut stream, &identity);
                        let result = request.deliver_private(
                            store,
                            &config.native_sources,
                            role,
                            private.as_ref(),
                            &config.native_telemetry_bindings,
                            &mut output,
                        );
                        data.verify()?;
                        instance.verify()?;
                        socket.verify()?;
                        output.complete(result);
                    } else {
                        let backup = if request.requests_backup(role) {
                            backup.as_ref().filter(|root| root.verify().is_ok())
                        } else {
                            None
                        };
                        let reply = native_store::dispatch_with_telemetry(
                            store,
                            &config.native_sources,
                            role,
                            request,
                            backup.map(|root| root.path.as_path()),
                            private.as_ref(),
                            &config.native_telemetry_bindings,
                        );
                        data.verify()?;
                        instance.verify()?;
                        socket.verify()?;
                        if backup.is_none_or(|root| root.verify().is_ok()) {
                            let _ = native::send_reply(&mut stream, &identity, reply);
                        }
                    }
                }
            } else if config.experimental_v2_scratch {
                if let Ok(request) =
                    read_authorized_v2_request(&mut stream, policy, &config.receiver_id)
                {
                    let scratch = data.path.join(SCRATCH_NAME);
                    if let Ok(reply) =
                        agenturm_storage_admission::apply_v2_json(&scratch, request.frame())
                    {
                        data.verify()?;
                        instance.verify()?;
                        socket.verify()?;
                        let _ = send_v2_reply(&mut stream, &request, &reply);
                    }
                }
            } else if config.experimental_real_clone_activation {
                // Recheck the service-startup inode proof before opening the
                // clone for mutation. A post-write check alone can reject the
                // reply after a replacement file has already been changed.
                if data.verify().is_ok()
                    && let Ok(request) =
                        read_authorized_v2_request(&mut stream, policy, &config.receiver_id)
                {
                    let clone = data.path.join(CLONE_NAME);
                    if let Ok(reply) = agenturm_storage_admission::apply_activation_clone_v2_json(
                        &clone,
                        request.frame(),
                    ) {
                        data.verify()?;
                        instance.verify()?;
                        socket.verify()?;
                        let _ = send_v2_reply(&mut stream, &request, &reply);
                    }
                }
            } else if let Ok(request) =
                read_authorized_request(&mut stream, policy, &config.receiver_id)
            {
                // The probe reply says only that transport and peer checks
                // succeeded. No database is opened or written.
                let _ = send_probe_reply(&mut stream, &request);
            }
            if config
                .max_connections
                .is_some_and(|limit| accepted >= limit)
            {
                break;
            }
        }
        Ok(())
    }

    #[cfg(test)]
    mod tests {
        use super::*;
        use std::os::unix::fs::symlink;
        use std::time::{SystemTime, UNIX_EPOCH};

        #[test]
        fn offline_restore_accepts_only_private_detached_backup_into_fresh_root() {
            const BACKUP: &str =
                "history-0123456789abcdef0123456789abcdef-00000001-0000000000000001.sqlite";
            let nonce = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos();
            let root = std::env::temp_dir().join(format!(
                "agenturm-offline-restore-{}-{nonce}",
                std::process::id()
            ));
            let data_path = root.join("data");
            let transport_path = root.join("transport");
            let backup_path = root.join("backup");
            fs::create_dir(&root).unwrap();
            for path in [&root, &data_path, &transport_path, &backup_path] {
                if path != &root {
                    fs::create_dir(path).unwrap();
                }
                fs::set_permissions(path, fs::Permissions::from_mode(0o700)).unwrap();
            }
            // SAFETY: getegid accepts no pointers.
            let gid = unsafe { libc::getegid() };
            fs::set_permissions(&transport_path, fs::Permissions::from_mode(0o2710)).unwrap();
            let backup = RootProof::capture(&backup_path, RootKind::Backup, gid).unwrap();
            let data = RootProof::capture(&data_path, RootKind::DataNative, gid).unwrap();
            let transport = RootProof::capture(&transport_path, RootKind::Transport, gid).unwrap();
            let instance = InstanceGuard::acquire(transport, NATIVE_LOCK_NAME).unwrap();
            let mut store = Store::open_collector(root.join("source.sqlite")).unwrap();
            let identity = store.ensure_history_identity().unwrap();
            let source = backup_path.join(BACKUP);
            store.backup_to(&source).unwrap();
            drop(store);

            assert!(offline_restore(&data, Some(&backup), &instance, "../source.sqlite").is_err());
            let sidecar = backup_path.join(format!("{BACKUP}-wal"));
            fs::write(&sidecar, b"sidecar").unwrap();
            assert!(offline_restore(&data, Some(&backup), &instance, BACKUP).is_err());
            fs::remove_file(sidecar).unwrap();
            fs::set_permissions(&source, fs::Permissions::from_mode(0o640)).unwrap();
            assert!(offline_restore(&data, Some(&backup), &instance, BACKUP).is_err());
            fs::set_permissions(&source, fs::Permissions::from_mode(0o600)).unwrap();
            assert!(!data_path.join(NATIVE_DB).exists());

            offline_restore(&data, Some(&backup), &instance, BACKUP).unwrap();
            let restored = Store::open_read_only(data_path.join(NATIVE_DB)).unwrap();
            assert_eq!(restored.history_identity().unwrap(), identity);
            restored.check_history_integrity().unwrap();
            let populated = RootProof::capture(&data_path, RootKind::DataNative, gid).unwrap();
            assert!(offline_restore(&populated, Some(&backup), &instance, BACKUP).is_err());
            drop(restored);
            drop(instance);
            fs::remove_dir_all(root).unwrap();
        }

        #[test]
        fn direct_root_rejects_symlink_and_parent_swap_risk() {
            let nonce = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos();
            let base = std::env::temp_dir().join(format!(
                "agenturm-service-root-{}-{nonce}",
                std::process::id()
            ));
            fs::create_dir(&base).unwrap();
            fs::set_permissions(&base, fs::Permissions::from_mode(0o700)).unwrap();
            let real = base.join("real");
            fs::create_dir(&real).unwrap();
            fs::set_permissions(&real, fs::Permissions::from_mode(0o700)).unwrap();
            symlink(&real, base.join("link")).unwrap();
            assert!(direct_directory(&base.join("link")).is_err());
            fs::set_permissions(&base, fs::Permissions::from_mode(0o777)).unwrap();
            assert!(direct_directory(&real).is_err());
            fs::remove_file(base.join("link")).unwrap();
            fs::remove_dir(&real).unwrap();
            fs::remove_dir(&base).unwrap();
        }

        #[test]
        fn scratch_root_accepts_only_pinned_private_files() {
            let nonce = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos();
            let base = std::env::temp_dir().join(format!(
                "agenturm-service-scratch-{}-{nonce}",
                std::process::id()
            ));
            fs::create_dir(&base).unwrap();
            fs::set_permissions(&base, fs::Permissions::from_mode(0o700)).unwrap();
            let scratch = base.join(SCRATCH_NAME);
            fs::write(&scratch, b"disposable fixture").unwrap();
            fs::set_permissions(&scratch, fs::Permissions::from_mode(0o600)).unwrap();
            let proof = RootProof::capture(&base, RootKind::DataScratch, 42).unwrap();
            proof.verify().unwrap();
            fs::write(base.join("canonical.db"), b"unrelated").unwrap();
            assert!(proof.verify().is_err());
            fs::remove_file(base.join("canonical.db")).unwrap();
            let archived = base.with_extension("archive");
            fs::rename(&scratch, &archived).unwrap();
            fs::write(&scratch, b"replacement").unwrap();
            fs::set_permissions(&scratch, fs::Permissions::from_mode(0o600)).unwrap();
            assert!(proof.verify().is_err());
            fs::remove_file(&scratch).unwrap();
            fs::remove_file(&archived).unwrap();
            fs::remove_dir(&base).unwrap();
        }

        #[test]
        fn real_clone_root_rejects_same_bytes_at_replaced_inode() {
            let nonce = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos();
            let root = std::env::temp_dir().join(format!(
                "agenturm-real-clone-root-{}-{nonce}",
                std::process::id()
            ));
            fs::create_dir(&root).unwrap();
            fs::set_permissions(&root, fs::Permissions::from_mode(0o700)).unwrap();
            let clone = root.join(CLONE_NAME);
            fs::write(&clone, b"detached candidate").unwrap();
            fs::set_permissions(&clone, fs::Permissions::from_mode(0o600)).unwrap();
            let proof = RootProof::capture(&root, RootKind::DataClone, 42).unwrap();
            proof.verify().unwrap();
            let old = root.with_extension("old");
            fs::rename(&clone, &old).unwrap();
            fs::write(&clone, b"detached candidate").unwrap();
            fs::set_permissions(&clone, fs::Permissions::from_mode(0o600)).unwrap();
            assert!(proof.verify().is_err());
            fs::remove_file(clone).unwrap();
            fs::remove_file(old).unwrap();
            fs::remove_dir(root).unwrap();
        }
    }
}

#[cfg(target_os = "linux")]
pub use linux::{ClientIdentity, NativeRolesConfig, ServiceConfig, run};

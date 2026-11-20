//! Feature-off Linux storage-service skeleton.
//!
//! Default mode serves only the v1 protocol probe with an empty private data
//! root. An explicit experimental mode admits v2 lifecycle requests only to
//! one pre-existing, marked disposable scratch database under that root.
//! Neither mode is connected to the production receiver or canonical Store.

#![forbid(unsafe_op_in_unsafe_fn)]

#[cfg(target_os = "linux")]
mod linux {
    use agenturm_storage_service_contract::{
        PeerPolicy, ProbeTopic, Request, read_authorized_request, read_authorized_v2_request,
        send_probe_reply, send_v2_reply,
    };
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
    const SCRATCH_NAME: &str = "scratch.db";
    const SCRATCH_FILES: &[&str] = &[
        SCRATCH_NAME,
        ".scratch.db.agenturm-writer.lock",
        "scratch.db-journal",
        "scratch.db-wal",
        "scratch.db-shm",
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
    }

    #[derive(Clone, Copy, Debug, Eq, PartialEq)]
    enum RootKind {
        DataProbe,
        DataScratch,
        Transport,
    }

    #[derive(Clone)]
    struct RootProof {
        path: PathBuf,
        kind: RootKind,
        identity: (u64, u64),
        client_gid: u32,
        scratch_identity: Option<(u64, u64)>,
    }

    struct InstanceGuard {
        file: File,
        path: PathBuf,
        transport: RootProof,
        identity: (u64, u64),
    }

    impl InstanceGuard {
        fn acquire(transport: RootProof) -> io::Result<Self> {
            let path = transport.path.join(LOCK_NAME);
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

    fn secure_ancestors(path: &Path) -> io::Result<()> {
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

    impl RootProof {
        fn capture(path: &Path, kind: RootKind, client_gid: u32) -> io::Result<Self> {
            let metadata = direct_directory(path)?;
            Self::check(&metadata, path, kind, client_gid)?;
            let scratch_identity = if kind == RootKind::DataScratch {
                let scratch = fs::symlink_metadata(path.join(SCRATCH_NAME))?;
                Some((scratch.dev(), scratch.ino()))
            } else {
                None
            };
            Ok(Self {
                path: path.to_path_buf(),
                kind,
                identity: (metadata.dev(), metadata.ino()),
                client_gid,
                scratch_identity,
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
                RootKind::DataProbe | RootKind::DataScratch => {
                    if metadata.mode() & 0o7777 != 0o700 {
                        return Err(denied("data root must be private 0700"));
                    }
                    let mut saw_scratch = false;
                    for entry in fs::read_dir(path)? {
                        let entry = entry?;
                        if kind == RootKind::DataProbe {
                            return Err(denied("probe data root must be empty"));
                        }
                        let name = entry.file_name();
                        if !SCRATCH_FILES
                            .iter()
                            .any(|allowed| name == OsStr::new(allowed))
                        {
                            return Err(denied("scratch data root has an unexpected entry"));
                        }
                        let file = fs::symlink_metadata(entry.path())?;
                        if !file.is_file()
                            || file.nlink() != 1
                            || file.uid() != current_uid()
                            || file.mode() & 0o7777 != 0o600
                        {
                            return Err(denied("scratch file ownership or mode changed"));
                        }
                        saw_scratch |= name == OsStr::new(SCRATCH_NAME);
                    }
                    if kind == RootKind::DataScratch && !saw_scratch {
                        return Err(denied("scratch database is missing"));
                    }
                }
                RootKind::Transport => {
                    if metadata.gid() != client_gid || metadata.mode() & 0o7777 != 0o2710 {
                        return Err(denied(
                            "transport directory must be service-owned 2710 with client group",
                        ));
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
            if let Some(expected) = self.scratch_identity {
                let current = fs::symlink_metadata(self.path.join(SCRATCH_NAME))?;
                if (current.dev(), current.ino()) != expected {
                    return Err(denied("scratch database identity changed"));
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
    }

    impl SocketGuard {
        fn capture(path: PathBuf, transport: RootProof, client_gid: u32) -> io::Result<Self> {
            let metadata = fs::symlink_metadata(&path)?;
            let guard = Self {
                path,
                transport,
                identity: (metadata.dev(), metadata.ino()),
                client_gid,
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
            // Never unlink an unrelated or replacement pathname. A stale
            // socket after a crash is left for explicit operator inspection.
            if self.still_our_pathname() {
                let _ = fs::remove_file(&self.path);
            }
        }
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
        if !valid_receiver || config.max_connections == Some(0) {
            return Err(denied("invalid service receiver or connection bound"));
        }
        let policy = PeerPolicy::for_current_service(config.client_uid, config.client_gid)
            .map_err(|_| {
                denied("service and client identities must be distinct non-root accounts")
            })?;
        let data = RootProof::capture(
            &config.data_root,
            if config.experimental_v2_scratch {
                RootKind::DataScratch
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

    pub fn run(config: &ServiceConfig) -> io::Result<()> {
        STOP.store(false, Ordering::Relaxed);
        let (data, transport, policy) = validate(config)?;
        let _private_mask = config
            .experimental_v2_scratch
            .then(UmaskGuard::private_files);
        let instance = InstanceGuard::acquire(transport.clone())?;
        if config.experimental_v2_scratch {
            // SQLite may touch a WAL shared-memory sidecar even for a read.
            // Never inspect the marker until this service owns the lock.
            agenturm_storage_admission::require_marked_scratch_path(&data.path.join(SCRATCH_NAME))
                .map_err(|_| denied("scratch database marker is unavailable"))?;
            data.verify()?;
        }
        let socket_path = transport.path.join(SOCKET_NAME);
        match fs::symlink_metadata(&socket_path) {
            Ok(_) => return Err(denied("service socket path already exists")),
            Err(error) if error.kind() == ErrorKind::NotFound => {}
            Err(error) => return Err(error),
        }
        install_shutdown_handlers()?;
        let listener = {
            let _mask = UmaskGuard::socket_mode();
            UnixListener::bind(&socket_path)?
        };
        let socket = SocketGuard::capture(socket_path, transport, config.client_gid)?;
        listener.set_nonblocking(true)?;
        let mut accepted = 0_usize;
        while !STOP.load(Ordering::Relaxed) {
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
            if config.experimental_v2_scratch {
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
    }
}

#[cfg(target_os = "linux")]
pub use linux::{ServiceConfig, run};

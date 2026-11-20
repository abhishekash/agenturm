//! Feature-off, Linux-only cooperative writer lease experiment.
//!
//! No production Store, receiver, or launcher calls this crate. A lease proves
//! only the state seen at acquire and at each explicit `verify_admission` call.
//! It cannot fence an already in-flight write, a legacy binary, or a process
//! that deliberately ignores the advisory lock.

#![forbid(unsafe_op_in_unsafe_fn)]

#[cfg(target_os = "linux")]
mod linux {
    use std::collections::HashMap;
    use std::ffi::OsString;
    use std::fs::{self, File, Metadata, OpenOptions};
    use std::io::{self, ErrorKind};
    use std::os::unix::fs::{MetadataExt, OpenOptionsExt};
    use std::path::{Path, PathBuf};
    use std::sync::{Arc, Mutex, OnceLock, Weak};

    fn denied(reason: &'static str) -> io::Error {
        io::Error::new(ErrorKind::PermissionDenied, reason)
    }

    fn identity(metadata: &Metadata) -> (u64, u64) {
        (metadata.dev(), metadata.ino())
    }

    fn current_uid() -> u32 {
        // SAFETY: geteuid has no arguments, pointers, or caller preconditions.
        unsafe { libc::geteuid() }
    }

    struct LexicalBinding {
        path: PathBuf,
        path_identity: (u64, u64),
        target: PathBuf,
        target_identity: (u64, u64),
    }

    impl LexicalBinding {
        fn capture(path: &Path) -> io::Result<Self> {
            if !path.is_absolute() {
                return Err(denied("writer lease requires an absolute database path"));
            }
            let path_identity = identity(&fs::symlink_metadata(path)?);
            let target = fs::canonicalize(path)?;
            let target_metadata = fs::metadata(&target)?;
            if !target_metadata.is_file() || target_metadata.nlink() != 1 {
                return Err(denied(
                    "writer lease target is not singly linked regular file",
                ));
            }
            let binding = Self {
                path: path.to_path_buf(),
                path_identity,
                target,
                target_identity: identity(&target_metadata),
            };
            binding.verify()?;
            Ok(binding)
        }

        fn verify(&self) -> io::Result<()> {
            if identity(&fs::symlink_metadata(&self.path)?) != self.path_identity
                || fs::canonicalize(&self.path)? != self.target
            {
                return Err(denied("writer lease database path changed"));
            }
            let reached = fs::metadata(&self.path)?;
            let target = fs::metadata(&self.target)?;
            if !reached.is_file()
                || !target.is_file()
                || reached.nlink() != 1
                || target.nlink() != 1
                || identity(&reached) != self.target_identity
                || identity(&target) != self.target_identity
            {
                return Err(denied("writer lease database target changed"));
            }
            Ok(())
        }
    }

    fn lock_path(target: &Path) -> io::Result<PathBuf> {
        let parent = target
            .parent()
            .ok_or_else(|| denied("writer lease target has no parent"))?;
        let basename = target
            .file_name()
            .ok_or_else(|| denied("writer lease target has no filename"))?;
        let mut filename = OsString::from(".");
        filename.push(basename);
        filename.push(".agenturm-writer.lock");
        Ok(parent.join(filename))
    }

    struct Inner {
        file: File,
        lock_path: PathBuf,
        lock_identity: (u64, u64),
        target: PathBuf,
        target_identity: (u64, u64),
        pid: u32,
    }

    impl Inner {
        fn verify(&self) -> io::Result<()> {
            if self.pid != std::process::id() {
                return Err(denied("writer lease cannot be reused after fork"));
            }
            let opened = self.file.metadata()?;
            let named = fs::symlink_metadata(&self.lock_path)?;
            if !opened.is_file()
                || !named.is_file()
                || identity(&opened) != self.lock_identity
                || identity(&named) != self.lock_identity
                || opened.uid() != current_uid()
                || opened.mode() & 0o7777 != 0o600
            {
                return Err(denied("writer lease lock file changed"));
            }
            let target = fs::metadata(&self.target)?;
            if !target.is_file() || target.nlink() != 1 || identity(&target) != self.target_identity
            {
                return Err(denied("writer lease database target changed"));
            }
            Ok(())
        }
    }

    struct Registry {
        pid: u32,
        entries: HashMap<PathBuf, Weak<Inner>>,
    }

    static REGISTRY: OnceLock<Mutex<Registry>> = OnceLock::new();

    fn registry() -> &'static Mutex<Registry> {
        REGISTRY.get_or_init(|| {
            Mutex::new(Registry {
                pid: std::process::id(),
                entries: HashMap::new(),
            })
        })
    }

    /// A checked advisory lease on an existing file. Clones share one lock FD.
    /// Dropping the final clone closes the FD and releases the OS lock.
    pub struct WriterLease {
        inner: Arc<Inner>,
        lexical: LexicalBinding,
    }

    impl WriterLease {
        /// Acquire the private sibling lock for an existing absolute path.
        /// Final symlinks are allowed, but the target and lexical link are bound.
        pub fn acquire(path: impl AsRef<Path>) -> io::Result<Self> {
            let lexical = LexicalBinding::capture(path.as_ref())?;
            let lock_path = lock_path(&lexical.target)?;
            let pid = std::process::id();
            let mut registry = registry()
                .lock()
                .map_err(|_| denied("writer lease registry poisoned"))?;
            if registry.pid != pid {
                registry.entries.clear();
                registry.pid = pid;
            }
            registry.entries.retain(|_, value| value.strong_count() > 0);
            if let Some(inner) = registry.entries.get(&lock_path).and_then(Weak::upgrade) {
                if inner.target != lexical.target
                    || inner.target_identity != lexical.target_identity
                {
                    return Err(denied("writer lease target differs from active owner"));
                }
                inner.verify()?;
                lexical.verify()?;
                return Ok(Self { inner, lexical });
            }
            // An existing FIFO/device could block or have side effects on
            // open. The nofollow and nonblocking flags still cover a change
            // between this check and open; fstat closes the remaining race.
            match fs::symlink_metadata(&lock_path) {
                Ok(metadata) if !metadata.is_file() => {
                    return Err(denied("writer lease lock path is not a regular file"));
                }
                Err(error) if error.kind() == ErrorKind::NotFound => {}
                Err(error) => return Err(error),
                Ok(_) => {}
            }
            let file = OpenOptions::new()
                .read(true)
                .write(true)
                .create(true)
                .mode(0o600)
                .custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC | libc::O_NONBLOCK)
                .open(&lock_path)?;
            let lock_identity = identity(&file.metadata()?);
            let inner = Arc::new(Inner {
                file,
                lock_path: lock_path.clone(),
                lock_identity,
                target: lexical.target.clone(),
                target_identity: lexical.target_identity,
                pid,
            });
            inner.verify()?;
            inner.file.try_lock()?;
            inner.verify()?;
            lexical.verify()?;
            registry.entries.insert(lock_path, Arc::downgrade(&inner));
            Ok(Self { inner, lexical })
        }

        /// Call immediately before a prospective write. The check is not an
        /// atomic fence across the later transaction or an old process.
        pub fn verify_admission(&self) -> io::Result<()> {
            self.inner.verify()?;
            self.lexical.verify()
        }

        /// The resolved sibling lock path, for diagnostics and tests only.
        pub fn lock_path(&self) -> &Path {
            &self.inner.lock_path
        }
    }

    impl Clone for WriterLease {
        fn clone(&self) -> Self {
            Self {
                inner: Arc::clone(&self.inner),
                lexical: LexicalBinding {
                    path: self.lexical.path.clone(),
                    path_identity: self.lexical.path_identity,
                    target: self.lexical.target.clone(),
                    target_identity: self.lexical.target_identity,
                },
            }
        }
    }
}

#[cfg(target_os = "linux")]
pub use linux::WriterLease;

#[cfg(not(target_os = "linux"))]
pub struct WriterLease;

#[cfg(not(target_os = "linux"))]
impl WriterLease {
    pub fn acquire(_path: impl AsRef<std::path::Path>) -> std::io::Result<Self> {
        Err(std::io::Error::new(
            std::io::ErrorKind::Unsupported,
            "writer lease experiment is Linux-only",
        ))
    }

    pub fn verify_admission(&self) -> std::io::Result<()> {
        Err(std::io::Error::new(
            std::io::ErrorKind::Unsupported,
            "writer lease experiment is Linux-only",
        ))
    }
}

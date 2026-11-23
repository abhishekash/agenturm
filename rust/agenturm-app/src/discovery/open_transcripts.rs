//! Bounded local open-file evidence for prioritizing transcript adapters.

use std::collections::BTreeSet;
use std::fs::Metadata;

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub(super) struct FileIdentity(u64, u64);

#[derive(Default)]
pub(super) struct OpenTranscripts {
    files: BTreeSet<(&'static str, FileIdentity)>,
    pub(super) partial: bool,
}

impl OpenTranscripts {
    pub(super) fn contains(&self, harness: &str, file: Option<FileIdentity>) -> bool {
        file.is_some_and(|file| self.files.contains(&(harness, file)))
    }

    #[cfg(all(test, target_os = "linux"))]
    pub(super) fn from_files(harness: &'static str, paths: &[std::path::PathBuf]) -> Self {
        Self {
            files: paths
                .iter()
                .map(|path| {
                    (
                        harness,
                        file_identity(&std::fs::metadata(path).unwrap()).unwrap(),
                    )
                })
                .collect(),
            partial: false,
        }
    }
}

pub(super) fn file_identity(metadata: &Metadata) -> Option<FileIdentity> {
    #[cfg(target_os = "linux")]
    {
        use std::os::unix::fs::MetadataExt;
        Some(FileIdentity(metadata.dev(), metadata.ino()))
    }
    #[cfg(not(target_os = "linux"))]
    {
        let _ = metadata;
        None
    }
}

pub(super) fn current() -> OpenTranscripts {
    #[cfg(target_os = "linux")]
    {
        use std::os::unix::fs::MetadataExt;
        if let Ok(metadata) = std::fs::metadata("/proc/self") {
            return linux::scan(std::path::Path::new("/proc"), metadata.uid());
        }
    }
    OpenTranscripts {
        partial: true,
        ..OpenTranscripts::default()
    }
}

#[cfg(target_os = "linux")]
mod linux {
    use super::*;
    use std::fs::{self, File, OpenOptions};
    use std::io::{ErrorKind, Read};
    use std::os::fd::AsRawFd;
    use std::os::unix::fs::{MetadataExt, OpenOptionsExt};
    use std::path::Path;
    use std::time::{Duration, Instant};

    const MAX_PROC_ENTRIES: usize = 4_096;
    const MAX_FDS_PER_PROCESS: usize = 256;
    const MAX_FDS: usize = 4_096;
    const SCAN_BUDGET: Duration = Duration::from_millis(250);

    pub(super) fn scan(root: &Path, uid: u32) -> OpenTranscripts {
        let mut output = OpenTranscripts::default();
        let Ok(entries) = fs::read_dir(root) else {
            output.partial = true;
            return output;
        };
        let started = Instant::now();
        let mut scanned_fds = 0;
        for (index, entry) in entries.enumerate() {
            if index == MAX_PROC_ENTRIES
                || started.elapsed() >= SCAN_BUDGET
                || scanned_fds == MAX_FDS
            {
                output.partial = true;
                break;
            }
            let Ok(entry) = entry else {
                output.partial = true;
                continue;
            };
            if !entry
                .file_name()
                .to_str()
                .is_some_and(|name| name.parse::<u32>().is_ok_and(|pid| pid > 0))
            {
                continue;
            }
            // Pin the process directory so later lookups cannot follow a reused PID.
            let process = OpenOptions::new()
                .read(true)
                .custom_flags(libc::O_DIRECTORY | libc::O_NOFOLLOW)
                .open(entry.path());
            let process = match process {
                Ok(process) => process,
                Err(error) => {
                    output.partial |= error.kind() == ErrorKind::PermissionDenied;
                    continue;
                }
            };
            if !process
                .metadata()
                .is_ok_and(|metadata| metadata.uid() == uid)
            {
                continue;
            }
            let process_path = format!("/proc/self/fd/{}", process.as_raw_fd());
            let process_path = Path::new(&process_path);
            let Some(harness) = harness(process_path, &mut output.partial) else {
                continue;
            };
            let fds = match fs::read_dir(process_path.join("fd")) {
                Ok(fds) => fds,
                Err(error) if error.kind() == ErrorKind::NotFound => continue,
                Err(_) => {
                    output.partial = true;
                    continue;
                }
            };
            for (index, fd) in fds.enumerate() {
                if index == MAX_FDS_PER_PROCESS
                    || scanned_fds == MAX_FDS
                    || started.elapsed() >= SCAN_BUDGET
                {
                    output.partial = true;
                    break;
                }
                scanned_fds += 1;
                let metadata = fd.and_then(|entry| fs::metadata(entry.path()));
                match metadata {
                    Ok(metadata) if metadata.is_file() && metadata.uid() == uid => {
                        if let Some(identity) = file_identity(&metadata) {
                            output.files.insert((harness, identity));
                        }
                    }
                    Err(error) if error.kind() != ErrorKind::NotFound => output.partial = true,
                    _ => {}
                }
            }
        }
        output
    }

    fn harness(process_path: &Path, partial: &mut bool) -> Option<&'static str> {
        let mut name = String::new();
        if let Err(error) = File::open(process_path.join("comm"))
            .and_then(|file| file.take(32).read_to_string(&mut name))
        {
            *partial |= error.kind() == ErrorKind::PermissionDenied;
            return None;
        }
        let harness = match name.trim() {
            "codex" => "codex",
            "claude" | "claude-code" => "claude",
            _ => return None,
        };
        let executable = match fs::read_link(process_path.join("exe")) {
            Ok(path) => path,
            Err(error) => {
                *partial |= error.kind() != ErrorKind::NotFound;
                return None;
            }
        };
        let basename = executable.file_name()?.to_str()?;
        let claude_version = harness == "claude"
            && executable
                .parent()
                .is_some_and(|path| path.ends_with("claude/versions"))
            && basename.starts_with(|ch: char| ch.is_ascii_digit())
            && basename
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || b".-".contains(&byte));
        (basename == harness
            || (harness == "claude" && basename == "claude-code")
            || claude_version)
            .then_some(harness)
    }

    #[cfg(test)]
    mod tests {
        use super::*;
        use std::os::unix::fs::symlink;
        use std::time::{SystemTime, UNIX_EPOCH};

        #[test]
        fn process_evidence_matches_the_inode_and_explicit_harness() {
            let directory = std::env::temp_dir().join(format!(
                "agenturm-open-handles-{}-{}",
                std::process::id(),
                SystemTime::now()
                    .duration_since(UNIX_EPOCH)
                    .unwrap()
                    .as_nanos()
            ));
            fs::create_dir_all(directory.join("42/fd")).unwrap();
            let transcript = directory.join("transcript.jsonl");
            let second = directory.join("second.jsonl");
            fs::write(&transcript, "{}").unwrap();
            fs::write(&second, "{}").unwrap();
            fs::write(directory.join("42/comm"), "codex\n").unwrap();
            symlink(directory.join("bin/codex"), directory.join("42/exe")).unwrap();
            symlink(&transcript, directory.join("42/fd/8")).unwrap();
            let uid = fs::metadata(&directory).unwrap().uid();
            let discovered = scan(&directory, uid);
            let first = file_identity(&fs::metadata(transcript).unwrap());
            let second = file_identity(&fs::metadata(second).unwrap());
            assert!(discovered.contains("codex", first));
            assert!(!discovered.contains("claude", first));
            assert!(!discovered.contains("codex", second));
            assert!(!scan(&directory, uid.wrapping_add(1)).contains("codex", first));
            fs::remove_dir_all(directory).unwrap();
        }
    }
}

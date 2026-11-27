//! Capture bounded local Git workspace metadata for an explicitly selected task.

use agenturm_store::{WorkspaceGitDiff, WorkspaceGitSnapshot};
use std::path::Path;

pub(super) fn capture(directory: &Path) -> Result<WorkspaceGitSnapshot, String> {
    #[cfg(target_os = "linux")]
    {
        linux::capture(directory, false).map(|(snapshot, _)| snapshot)
    }
    #[cfg(not(target_os = "linux"))]
    {
        let _ = directory;
        Err("Git workspace capture is available on Linux".into())
    }
}

pub(super) fn capture_with_diff(
    directory: &Path,
) -> Result<(WorkspaceGitSnapshot, WorkspaceGitDiff), String> {
    #[cfg(target_os = "linux")]
    {
        let (snapshot, diff) = linux::capture(directory, true)?;
        Ok((
            snapshot,
            diff.expect("explicit patch capture returned a patch"),
        ))
    }
    #[cfg(not(target_os = "linux"))]
    {
        let _ = directory;
        Err("Git patch capture is available on Linux".into())
    }
}

#[cfg(target_os = "linux")]
mod linux {
    use super::*;
    use agenturm_store::MAX_WORKSPACE_GIT_DIFF_BYTES;
    use rustix::fs::{Mode, OFlags, fcntl_setfl, open, openat};
    use std::collections::BTreeSet;
    use std::ffi::OsStr;
    use std::fs::{self, DirBuilder, File, OpenOptions};
    use std::io::{ErrorKind, Read, Write};
    use std::os::unix::{
        ffi::OsStrExt,
        fs::{DirBuilderExt, MetadataExt, OpenOptionsExt},
    };
    use std::path::PathBuf;
    use std::process::{Child, Command, ExitStatus, Stdio};
    use std::sync::atomic::{AtomicU64, Ordering};
    use std::thread;
    use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

    const CAPTURE_TIMEOUT: Duration = Duration::from_secs(4);
    const MAX_OUTPUT: usize = 256 * 1024;
    const MAX_INDEX: u64 = 16 * 1024 * 1024;
    const MAX_PATCH_INPUT: u64 = 8 * 1024 * 1024;
    const MAX_PATCH_PATHS: usize = 64;

    pub(super) fn capture(
        directory: &Path,
        include_diff: bool,
    ) -> Result<(WorkspaceGitSnapshot, Option<WorkspaceGitDiff>), String> {
        if !directory.is_dir() {
            return Err("Enter a local Git worktree directory".into());
        }
        let deadline = Instant::now() + CAPTURE_TIMEOUT;
        let (status, paths) = git(
            directory,
            &[
                "rev-parse",
                "--path-format=absolute",
                "--show-toplevel",
                "--git-path",
                "index",
                "--git-path",
                "objects",
                "--show-object-format",
            ],
            None,
            deadline,
        )?;
        if !status.success() {
            return Err("The selected directory is not a readable Git worktree".into());
        }
        let paths = std::str::from_utf8(&paths).map_err(|_| "Git worktree paths must be UTF-8")?;
        let fields = paths
            .strip_suffix('\n')
            .unwrap_or(paths)
            .split('\n')
            .collect::<Vec<_>>();
        let [root, index, objects, format] = fields.as_slice() else {
            return Err("Git worktree returned unexpected paths".into());
        };
        if [root, index, objects].iter().any(|path| {
            !Path::new(path).is_absolute()
                || path.len() > 4096
                || path.chars().any(char::is_control)
        }) {
            return Err("Git worktree paths must be absolute and bounded".into());
        }
        let digits = match *format {
            "sha1" => 40,
            "sha256" => 64,
            _ => return Err("Git worktree uses an unsupported object format".into()),
        };
        let branch = git_ref(directory, &["symbolic-ref", "--quiet", "HEAD"], deadline)?
            .map(|reference| {
                reference
                    .strip_prefix("refs/heads/")
                    .filter(|name| !name.is_empty())
                    .map(str::to_owned)
                    .ok_or_else(|| "Git HEAD refers to an unsupported reference".to_owned())
            })
            .transpose()?;
        let head_commit = git_ref(
            directory,
            &["rev-parse", "--verify", "--quiet", "HEAD"],
            deadline,
        )?;
        if head_commit.as_ref().is_some_and(|head| {
            head.len() != digits || !head.bytes().all(|byte| byte.is_ascii_hexdigit())
        }) || (head_commit.is_none() && branch.is_none())
        {
            return Err("Git did not return a valid HEAD commit or unborn branch".into());
        }

        // Read the real index, objects and worktree against a private detached
        // Git directory. Its fixed configuration excludes repository-defined
        // hooks, fsmonitor, diff programs and content conversion filters.
        let scratch = Scratch::new(format, head_commit.as_deref())?;
        let root = Path::new(root);
        let index = Path::new(index);
        let copied_index = scratch.0.join("index");
        let index_stamp = if include_diff {
            let (status, shared) = git(
                directory,
                &["rev-parse", "--shared-index-path"],
                None,
                deadline,
            )?;
            if !status.success() || !shared.is_empty() {
                return Err("Git patch capture requires a regular, unsplit index".into());
            }
            Some(copy_index(index, &copied_index)?)
        } else {
            None
        };
        let files = GitFiles {
            directory: &scratch.0,
            root,
            index: if include_diff { &copied_index } else { index },
            objects: Path::new(objects),
        };
        let (status, changes) = git(
            root,
            &[
                "status",
                "--porcelain=v1",
                "-z",
                "--no-renames",
                "--untracked-files=all",
                "--ignore-submodules=all",
            ],
            Some(files),
            deadline,
        )?;
        if !status.success() {
            return Err(
                "Git could not summarize this worktree without custom configuration".into(),
            );
        }
        let (staged_changes, unstaged_changes, untracked_files) = change_counts(&changes)?;
        let diff = if let Some(index_stamp) = index_stamp {
            let diff = capture_diff(files, &changes, digits, deadline)?;
            if named_stamp(index)? != index_stamp
                || git_ref(directory, &["symbolic-ref", "--quiet", "HEAD"], deadline)?.as_deref()
                    != branch
                        .as_deref()
                        .map(|branch| format!("refs/heads/{branch}"))
                        .as_deref()
                || git_ref(
                    directory,
                    &["rev-parse", "--verify", "--quiet", "HEAD"],
                    deadline,
                )? != head_commit
            {
                return Err(
                    "Git HEAD or index changed during patch capture. Retry the snapshot.".into(),
                );
            }
            Some(diff)
        } else {
            None
        };
        let captured_at_unix_ns = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .ok()
            .and_then(|time| u64::try_from(time.as_nanos()).ok())
            .ok_or("Cannot timestamp the Git snapshot")?;
        Ok((
            WorkspaceGitSnapshot {
                repository_root: root.to_string_lossy().into(),
                branch,
                head_commit,
                staged_changes,
                unstaged_changes,
                untracked_files,
                captured_at_unix_ns,
            },
            diff,
        ))
    }

    fn git_ref(
        directory: &Path,
        args: &[&str],
        deadline: Instant,
    ) -> Result<Option<String>, String> {
        let (status, output) = git(directory, args, None, deadline)?;
        if status.code() == Some(1) && output.is_empty() {
            return Ok(None);
        }
        if !status.success() {
            return Err("Git could not read the current reference".into());
        }
        let text = String::from_utf8(output).map_err(|_| "Git reference is not UTF-8")?;
        let text = text.strip_suffix('\n').unwrap_or(&text);
        if text.is_empty() || text.len() > 256 || text.chars().any(char::is_control) {
            return Err("Git reference is invalid or too long".into());
        }
        Ok(Some(text.to_owned()))
    }

    fn change_counts(changes: &[u8]) -> Result<(u32, u32, u32), String> {
        let mut staged = 0;
        let mut unstaged = 0;
        let mut untracked = 0;
        if !changes.is_empty() && changes.last() != Some(&0) {
            return Err("Git status returned a truncated record".into());
        }
        for row in changes
            .split(|&byte| byte == 0)
            .filter(|row| !row.is_empty())
        {
            match row {
                [b'?', b'?', b' ', path @ ..] if !path.is_empty() => untracked += 1,
                [index, worktree, b' ', path @ ..]
                    if !path.is_empty()
                        && [index, worktree]
                            .iter()
                            .all(|code| b" MADTUC".contains(code)) =>
                {
                    staged += u32::from(*index != b' ');
                    unstaged += u32::from(*worktree != b' ');
                }
                _ => return Err("Git status returned an unsupported record".into()),
            }
        }
        Ok((staged, unstaged, untracked))
    }

    #[derive(Eq, PartialEq)]
    struct FileStamp {
        device: u64,
        inode: u64,
        size: u64,
        modified: (i64, i64),
        changed: (i64, i64),
        mode: u32,
    }

    fn stamp(metadata: &fs::Metadata) -> Result<FileStamp, String> {
        if !metadata.is_file() {
            return Err("Git patch inputs must be regular files".into());
        }
        Ok(FileStamp {
            device: metadata.dev(),
            inode: metadata.ino(),
            size: metadata.len(),
            modified: (metadata.mtime(), metadata.mtime_nsec()),
            changed: (metadata.ctime(), metadata.ctime_nsec()),
            mode: metadata.mode(),
        })
    }

    fn named_stamp(path: &Path) -> Result<Option<FileStamp>, String> {
        match fs::symlink_metadata(path) {
            Ok(metadata) => stamp(&metadata).map(Some),
            Err(error) if error.kind() == ErrorKind::NotFound => Ok(None),
            Err(_) => Err("Cannot verify Git index identity".into()),
        }
    }

    fn copy_index(path: &Path, destination: &Path) -> Result<Option<FileStamp>, String> {
        let file = match open(
            path,
            OFlags::RDONLY | OFlags::NONBLOCK | OFlags::NOFOLLOW,
            Mode::empty(),
        ) {
            Ok(fd) => File::from(fd),
            Err(rustix::io::Errno::NOENT) => return Ok(None),
            Err(_) => return Err("Cannot open the selected Git index".into()),
        };
        let identity = copy_file(file, destination, MAX_INDEX)?;
        if named_stamp(path)?.as_ref() != Some(&identity) {
            return Err("Git index changed during patch capture. Retry the snapshot.".into());
        }
        Ok(Some(identity))
    }

    fn copy_file(file: File, destination: &Path, limit: u64) -> Result<FileStamp, String> {
        let failure = |_| "Cannot copy a Git patch input into the private capture directory";
        let identity = stamp(&file.metadata().map_err(failure)?)?;
        if identity.size > limit {
            return Err("Git patch input exceeds its size limit".into());
        }
        let mode = if identity.mode & 0o100 == 0 {
            0o600
        } else {
            0o700
        };
        let mut output = OpenOptions::new()
            .create_new(true)
            .write(true)
            .mode(mode)
            .open(destination)
            .map_err(failure)?;
        let copied = std::io::copy(&mut (&file).take(limit + 1), &mut output).map_err(failure)?;
        if copied != identity.size || stamp(&file.metadata().map_err(failure)?)? != identity {
            return Err("A Git patch input changed during capture. Retry the snapshot.".into());
        }
        Ok(identity)
    }

    fn capture_diff(
        files: GitFiles<'_>,
        changes: &[u8],
        digits: usize,
        deadline: Instant,
    ) -> Result<WorkspaceGitDiff, String> {
        let projected = files.directory.join("patch-worktree");
        fs::create_dir(&projected).map_err(|_| "Cannot create private Git patch worktree")?;
        let (paths, remaining) = project_worktree(files.root, &projected, changes)?;
        let projected = GitFiles {
            root: &projected,
            ..files
        };
        let mut blobs = BTreeSet::new();
        let staged_records = diff(projected, true, false, &[], deadline, MAX_OUTPUT)?;
        add_blob_ids(&staged_records, digits, &mut blobs)?;
        if !paths.is_empty() {
            let unstaged_records = diff(projected, false, false, &paths, deadline, MAX_OUTPUT)?;
            add_blob_ids(&unstaged_records, digits, &mut blobs)?;
        }
        verify_blob_sizes(projected, &blobs, remaining, deadline)?;
        let maximum = MAX_WORKSPACE_GIT_DIFF_BYTES;
        let staged = diff(projected, true, true, &[], deadline, maximum)?;
        let unstaged = if paths.is_empty() {
            Vec::new()
        } else {
            diff(
                projected,
                false,
                true,
                &paths,
                deadline,
                maximum - staged.len(),
            )?
        };
        let utf8 = |_| "Git patch text must be UTF-8";
        Ok(WorkspaceGitDiff {
            staged: String::from_utf8(staged).map_err(utf8)?,
            unstaged: String::from_utf8(unstaged).map_err(utf8)?,
        })
    }

    fn project_worktree<'a>(
        root: &Path,
        projected: &Path,
        changes: &'a [u8],
    ) -> Result<(Vec<&'a OsStr>, u64), String> {
        let mut paths = Vec::new();
        let mut changed = 0;
        let mut remaining = MAX_PATCH_INPUT;
        for row in changes
            .split(|&byte| byte == 0)
            .filter(|row| !row.is_empty())
        {
            let [index, worktree, b' ', name @ ..] = row else {
                return Err("Git returned an invalid patch candidate".into());
            };
            if *index == b'?' {
                continue;
            }
            changed += 1;
            if changed > MAX_PATCH_PATHS || [index, worktree].contains(&&b'U') {
                return Err("Git patch capture requires at most 64 changed, merged paths".into());
            }
            if *worktree == b' ' {
                continue;
            }
            let name = OsStr::from_bytes(name);
            let path = Path::new(name);
            let member = worktree_file(root, path)?;
            if *worktree == b'D' {
                if member.is_some() {
                    return Err("A deleted Git path reappeared during patch capture".into());
                }
            } else {
                let file = member.ok_or("A Git path disappeared during patch capture")?;
                let destination = projected.join(path);
                fs::create_dir_all(destination.parent().expect("projection has a parent"))
                    .map_err(|_| "Cannot create a private Git patch directory")?;
                remaining -= copy_file(file, &destination, remaining)?.size;
            }
            paths.push(name);
        }
        Ok((paths, remaining))
    }

    fn worktree_file(root: &Path, path: &Path) -> Result<Option<File>, String> {
        let unavailable = "Cannot open a Git patch path without following links";
        // Git emits slash-delimited paths. Check the original bytes because
        // Path::components normalizes repeated separators and interior dots.
        let mut parts = path
            .as_os_str()
            .as_bytes()
            .split(|&byte| byte == b'/')
            .peekable();
        let directories = OFlags::RDONLY | OFlags::DIRECTORY | OFlags::NOFOLLOW | OFlags::CLOEXEC;
        let mut directory = open(root, directories, Mode::empty()).map_err(|_| unavailable)?;
        while let Some(part) = parts.next() {
            if part.is_empty()
                || part == b"."
                || part == b".."
                || part.eq_ignore_ascii_case(b".git")
            {
                return Err("Invalid Git patch path".into());
            }
            let last = parts.peek().is_none();
            let flags = if last {
                OFlags::RDONLY | OFlags::NONBLOCK | OFlags::NOFOLLOW | OFlags::CLOEXEC
            } else {
                directories
            };
            match openat(&directory, OsStr::from_bytes(part), flags, Mode::empty()) {
                Ok(fd) if last => return Ok(Some(File::from(fd))),
                Ok(fd) => directory = fd,
                Err(rustix::io::Errno::NOENT) => return Ok(None),
                Err(_) => return Err(unavailable.into()),
            }
        }
        Err("Invalid Git patch path".into())
    }

    fn diff(
        files: GitFiles<'_>,
        staged: bool,
        patch: bool,
        paths: &[&OsStr],
        deadline: Instant,
        limit: usize,
    ) -> Result<Vec<u8>, String> {
        let mut args: Vec<&OsStr> = [
            "diff",
            "--no-ext-diff",
            "--no-textconv",
            "--no-renames",
            "--ignore-submodules=all",
            "--no-color",
            "--no-abbrev",
            "--src-prefix=a/",
            "--dst-prefix=b/",
            "--diff-algorithm=myers",
            if patch { "--patch" } else { "--raw" },
        ]
        .into_iter()
        .map(OsStr::new)
        .collect();
        if !patch {
            args.push(OsStr::new("-z"));
        }
        if staged {
            args.push(OsStr::new("--cached"));
        }
        args.push(OsStr::new("--"));
        args.extend_from_slice(paths);
        let (status, value) = git_bounded(
            files.root,
            &args,
            Some(files),
            deadline,
            limit,
            Stdio::null(),
        )?;
        if !status.success() {
            return Err("Git could not capture a patch with private configuration".into());
        }
        Ok(value)
    }

    fn add_blob_ids(
        records: &[u8],
        digits: usize,
        blobs: &mut BTreeSet<String>,
    ) -> Result<(), String> {
        let invalid = "Git patch contains unsupported or unmerged entries";
        let records = if records.is_empty() {
            return Ok(());
        } else {
            records.strip_suffix(&[0]).ok_or(invalid)?
        };
        let mut fields = records.split(|&byte| byte == 0);
        let mut paths = 0;
        while let Some(record) = fields.next() {
            paths += 1;
            if paths > MAX_PATCH_PATHS || fields.next().is_none_or(|path| path.is_empty()) {
                return Err(invalid.into());
            }
            for id in parse_diff_record(record).ok_or(invalid)? {
                if id.len() != digits || !id.bytes().all(|byte| byte.is_ascii_hexdigit()) {
                    return Err(invalid.into());
                }
                if id.bytes().any(|byte| byte != b'0') {
                    blobs.insert(id.to_owned());
                }
            }
        }
        Ok(())
    }

    fn parse_diff_record(record: &[u8]) -> Option<[&str; 2]> {
        let text = std::str::from_utf8(record).ok()?.strip_prefix(':')?;
        let mut fields = text.split(' ');
        let (old, new, old_id, new_id, status) = (
            fields.next()?,
            fields.next()?,
            fields.next()?,
            fields.next()?,
            fields.next()?,
        );
        if fields.next().is_some()
            || ![old, new]
                .iter()
                .all(|mode| matches!(*mode, "000000" | "100644" | "100755"))
            || !["A", "D", "M"].contains(&status)
        {
            return None;
        }
        Some([old_id, new_id])
    }

    fn verify_blob_sizes(
        files: GitFiles<'_>,
        blobs: &BTreeSet<String>,
        mut remaining: u64,
        deadline: Instant,
    ) -> Result<(), String> {
        if blobs.is_empty() {
            return Ok(());
        }
        let unavailable = |_| "Cannot validate Git patch object sizes";
        let input = files.directory.join("patch-object-list");
        let mut contents = File::create_new(&input).map_err(unavailable)?;
        for id in blobs {
            writeln!(&mut contents, "{id}").map_err(unavailable)?;
        }
        drop(contents);
        let (status, output) = git_bounded(
            files.root,
            &[
                "cat-file",
                "--batch-check=%(objectname) %(objecttype) %(objectsize)",
            ],
            Some(files),
            deadline,
            MAX_PATCH_PATHS * 4 * 96,
            File::open(input).map_err(unavailable)?.into(),
        )?;
        let malformed = "Invalid Git object size response";
        if !status.success() {
            return Err(malformed.into());
        }
        let mut lines = std::str::from_utf8(&output).map_err(|_| malformed)?.lines();
        for id in blobs {
            let mut fields = lines.next().ok_or(malformed)?.split(' ');
            if fields.next() != Some(id) || fields.next() != Some("blob") {
                return Err(malformed.into());
            }
            let size = fields
                .next()
                .ok_or(malformed)?
                .parse::<u64>()
                .map_err(|_| malformed)?;
            if fields.next().is_some() {
                return Err(malformed.into());
            }
            remaining = remaining
                .checked_sub(size)
                .ok_or("Git patch inputs exceed 8 MiB")?;
        }
        if lines.next().is_some() {
            return Err(malformed.into());
        }
        Ok(())
    }

    #[derive(Clone, Copy)]
    struct GitFiles<'a> {
        directory: &'a Path,
        root: &'a Path,
        index: &'a Path,
        objects: &'a Path,
    }

    fn git(
        directory: &Path,
        args: &[impl AsRef<OsStr>],
        files: Option<GitFiles<'_>>,
        deadline: Instant,
    ) -> Result<(ExitStatus, Vec<u8>), String> {
        git_bounded(directory, args, files, deadline, MAX_OUTPUT, Stdio::null())
    }

    fn git_bounded(
        directory: &Path,
        args: &[impl AsRef<OsStr>],
        files: Option<GitFiles<'_>>,
        deadline: Instant,
        limit: usize,
        input: Stdio,
    ) -> Result<(ExitStatus, Vec<u8>), String> {
        if Instant::now() >= deadline {
            return Err("Git workspace capture timed out".into());
        }
        let mut command = Command::new("git");
        command
            .current_dir(directory)
            .env_clear()
            .env("LC_ALL", "C")
            .env(
                "PATH",
                std::env::var_os("PATH").unwrap_or_else(|| "/usr/bin:/bin".into()),
            )
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .env("GIT_OPTIONAL_LOCKS", "0")
            .env("GIT_NO_LAZY_FETCH", "1")
            .env("GIT_TERMINAL_PROMPT", "0")
            .env("GIT_NO_REPLACE_OBJECTS", "1")
            .env("GIT_ATTR_NOSYSTEM", "1")
            .env("GIT_LITERAL_PATHSPECS", "1")
            .args([
                "--no-pager",
                "-c",
                "core.hooksPath=/dev/null",
                "-c",
                "core.fsmonitor=false",
                "-c",
                "diff.external=",
                "-c",
                "core.attributesFile=/dev/null",
            ])
            .args(args)
            .stdin(input)
            .stdout(Stdio::piped())
            .stderr(Stdio::null());
        if let Some(files) = files {
            command
                .env("GIT_DIR", files.directory)
                .env("GIT_WORK_TREE", files.root)
                .env("GIT_INDEX_FILE", files.index)
                .env("GIT_OBJECT_DIRECTORY", files.objects);
        }
        let mut child = command.spawn().map_err(|_| "Cannot start local Git")?;
        let mut stdout = child.stdout.take().expect("Git output is piped");
        if fcntl_setfl(&stdout, OFlags::NONBLOCK).is_err() {
            return stop(child, "Cannot bound Git output");
        }
        let mut bytes = Vec::new();
        let mut chunk = [0u8; 8192];
        let mut status = None;
        let mut eof = false;
        loop {
            if Instant::now() >= deadline {
                return stop(child, "Git workspace capture timed out");
            }
            if !eof {
                match stdout.read(&mut chunk) {
                    Ok(0) => eof = true,
                    Ok(count) if bytes.len() + count <= limit => {
                        bytes.extend_from_slice(&chunk[..count]);
                        continue;
                    }
                    Ok(_) => return stop(child, "Git workspace capture exceeds its output limit"),
                    Err(error) if error.kind() == ErrorKind::WouldBlock => {}
                    Err(error) if error.kind() == ErrorKind::Interrupted => continue,
                    Err(_) => return stop(child, "Cannot read Git output"),
                }
            }
            if status.is_none() {
                status = match child.try_wait() {
                    Ok(status) => status,
                    Err(_) => return stop(child, "Cannot check Git workspace capture"),
                };
            }
            if let Some(status) = status
                && eof
            {
                return Ok((status, bytes));
            }
            thread::sleep(Duration::from_millis(10));
        }
    }

    fn stop<T>(mut child: Child, message: &str) -> Result<T, String> {
        let _ = child.kill();
        let _ = thread::Builder::new()
            .name("agenturm-git-reaper".into())
            .spawn(move || {
                let _ = child.wait();
            });
        Err(message.into())
    }

    struct Scratch(PathBuf);

    impl Scratch {
        fn new(format: &str, head: Option<&str>) -> Result<Self, String> {
            static NEXT_DIRECTORY: AtomicU64 = AtomicU64::new(0);
            let id = NEXT_DIRECTORY.fetch_add(1, Ordering::Relaxed);
            let parent = std::env::temp_dir();
            let path = parent.join(format!("agenturm-git-{}-{id}", std::process::id()));
            DirBuilder::new()
                .mode(0o700)
                .create(&path)
                .map_err(|_| "Cannot create private Git capture directory")?;
            let scratch = Self(path);
            let config = if format == "sha256" {
                "[core]\nrepositoryformatversion = 1\n[extensions]\nobjectformat = sha256\n"
            } else {
                "[core]\nrepositoryformatversion = 0\n"
            };
            fs::create_dir(scratch.0.join("refs"))
                .and_then(|()| fs::write(scratch.0.join("config"), config))
                .and_then(|()| {
                    fs::write(
                        scratch.0.join("HEAD"),
                        format!("{}\n", head.unwrap_or("ref: refs/heads/agenturm-unborn")),
                    )
                })
                .map_err(|_| "Cannot initialize private Git capture directory")?;
            Ok(scratch)
        }
    }

    impl Drop for Scratch {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        #[test]
        fn status_counts_use_both_git_columns_and_skip_path_content() {
            assert_eq!(
                change_counts(b"MM detail\nname\0A  staged\0 D changed\0?? untracked\0").unwrap(),
                (2, 2, 1)
            );
            assert!(change_counts(b"?? truncated").is_err());
        }

        #[test]
        fn captures_unclean_worktree_without_running_configured_programs() {
            let fixture = Scratch::new("sha1", None).unwrap();
            let directory = fixture.0.join("fixture");
            fs::create_dir(&directory).unwrap();
            let command = |args: &[&str]| {
                assert!(
                    Command::new("git")
                        .current_dir(&directory)
                        .env_clear()
                        .env("PATH", std::env::var_os("PATH").unwrap())
                        .env("GIT_CONFIG_NOSYSTEM", "1")
                        .env("GIT_CONFIG_GLOBAL", "/dev/null")
                        .args([
                            "-c",
                            "user.name=Fixture",
                            "-c",
                            "user.email=fixture@example.test",
                        ])
                        .args(args)
                        .output()
                        .unwrap()
                        .status
                        .success()
                );
            };
            command(&["init", "-b", "fixture"]);
            assert_eq!(super::super::capture(&directory).unwrap().head_commit, None);

            fs::write(directory.join(".gitattributes"), "*.txt filter=fixture\n").unwrap();
            fs::write(directory.join("base.txt"), "base\n").unwrap();
            fs::write(directory.join("staged.txt"), "staged\n").unwrap();
            command(&["add", "."]);
            command(&["commit", "-m", "Fixture"]);
            fs::write(directory.join("staged.txt"), "new staged value\n").unwrap();
            command(&["add", "staged.txt"]);
            fs::write(directory.join("base.txt"), "modified worktree value\n").unwrap();
            fs::write(directory.join("untracked.txt"), "untracked value\n").unwrap();

            let sentinel = fixture.0.join("external-command-ran");
            let external = format!("touch {}", sentinel.display());
            command(&["config", "filter.fixture.clean", &external]);
            command(&["config", "filter.fixture.required", "true"]);
            command(&["config", "core.fsmonitor", &external]);
            command(&["config", "diff.external", &external]);

            let snapshot = super::super::capture(&directory).unwrap();
            assert_eq!(snapshot.repository_root, directory.to_str().unwrap());
            assert_eq!(snapshot.branch.as_deref(), Some("fixture"));
            assert_eq!(snapshot.head_commit.unwrap().len(), 40);
            assert_eq!(snapshot.staged_changes, 1);
            assert_eq!(snapshot.unstaged_changes, 1);
            assert_eq!(snapshot.untracked_files, 1);
            let (_, patch) = capture_with_diff(&directory).unwrap();
            assert!(patch.staged.contains("+new staged value"));
            assert!(!patch.staged.contains("modified worktree value"));
            assert!(patch.unstaged.contains("+modified worktree value"));
            assert!(!patch.unstaged.contains("untracked value"));
            assert!(!sentinel.exists());
        }
    }
}

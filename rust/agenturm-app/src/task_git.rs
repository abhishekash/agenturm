//! Capture bounded local Git workspace metadata for an explicitly selected task.

use agenturm_store::WorkspaceGitSnapshot;
use std::path::Path;

pub(super) fn capture(directory: &Path) -> Result<WorkspaceGitSnapshot, String> {
    #[cfg(target_os = "linux")]
    {
        linux::capture(directory)
    }
    #[cfg(not(target_os = "linux"))]
    {
        let _ = directory;
        Err("Git workspace capture is available on Linux".into())
    }
}

#[cfg(target_os = "linux")]
mod linux {
    use super::*;
    use rustix::fs::{OFlags, fcntl_setfl};
    use std::fs::{self, DirBuilder};
    use std::io::{ErrorKind, Read};
    use std::os::unix::fs::DirBuilderExt;
    use std::path::PathBuf;
    use std::process::{Child, Command, ExitStatus, Stdio};
    use std::sync::atomic::{AtomicU64, Ordering};
    use std::thread;
    use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

    const CAPTURE_TIMEOUT: Duration = Duration::from_secs(4);
    const MAX_OUTPUT: usize = 256 * 1024;

    pub(super) fn capture(directory: &Path) -> Result<WorkspaceGitSnapshot, String> {
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
        let (status, changes) = git(
            Path::new(root),
            &[
                "status",
                "--porcelain=v1",
                "-z",
                "--no-renames",
                "--untracked-files=all",
                "--ignore-submodules=all",
            ],
            Some(GitFiles {
                directory: &scratch.0,
                root: Path::new(root),
                index: Path::new(index),
                objects: Path::new(objects),
            }),
            deadline,
        )?;
        if !status.success() {
            return Err(
                "Git could not summarize this worktree without custom configuration".into(),
            );
        }
        let (staged_changes, unstaged_changes, untracked_files) = change_counts(&changes)?;
        let captured_at_unix_ns = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .ok()
            .and_then(|time| u64::try_from(time.as_nanos()).ok())
            .ok_or("Cannot timestamp the Git snapshot")?;
        Ok(WorkspaceGitSnapshot {
            repository_root: root.to_string(),
            branch,
            head_commit,
            staged_changes,
            unstaged_changes,
            untracked_files,
            captured_at_unix_ns,
        })
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

    struct GitFiles<'a> {
        directory: &'a Path,
        root: &'a Path,
        index: &'a Path,
        objects: &'a Path,
    }

    fn git(
        directory: &Path,
        args: &[&str],
        files: Option<GitFiles<'_>>,
        deadline: Instant,
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
            .args([
                "--no-pager",
                "-c",
                "core.hooksPath=/dev/null",
                "-c",
                "core.fsmonitor=false",
                "-c",
                "diff.external=",
            ])
            .args(args)
            .stdin(Stdio::null())
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
                    Ok(count) if bytes.len() + count <= MAX_OUTPUT => {
                        bytes.extend_from_slice(&chunk[..count]);
                        continue;
                    }
                    Ok(_) => return stop(child, "Git workspace status exceeds its output limit"),
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
            assert_eq!(capture(&directory).unwrap().head_commit, None);

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

            let snapshot = capture(&directory).unwrap();
            assert_eq!(snapshot.repository_root, directory.to_str().unwrap());
            assert_eq!(snapshot.branch.as_deref(), Some("fixture"));
            assert_eq!(snapshot.head_commit.unwrap().len(), 40);
            assert_eq!(snapshot.staged_changes, 1);
            assert_eq!(snapshot.unstaged_changes, 1);
            assert_eq!(snapshot.untracked_files, 1);
            assert!(!sentinel.exists());
        }
    }
}

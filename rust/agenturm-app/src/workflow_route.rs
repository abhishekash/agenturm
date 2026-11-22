//! Route explicit user requests to a uniquely verified local tmux pane.

pub(super) fn locate(source: &str, session: &str) -> Result<String, String> {
    #[cfg(target_os = "linux")]
    {
        linux::locate(source, session)
    }
    #[cfg(not(target_os = "linux"))]
    {
        let _ = (source, session);
        Err("Live terminal routing is available on Linux".into())
    }
}

#[cfg(target_os = "linux")]
mod linux {
    use std::collections::{BTreeMap, BTreeSet};
    use std::fs;
    use std::io::Read;
    use std::os::unix::fs::MetadataExt;
    use std::path::Path;
    use std::process::{Command, Stdio};
    use std::thread;
    use std::time::{Duration, Instant};

    const NO_ROUTE: &str =
        "No unique live tmux pane was verified. Open this session in its native terminal.";
    const MAX_PROCESS_ENTRIES: usize = 4096;
    const MAX_ANCESTORS: usize = 64;

    #[derive(Clone, Copy, Debug, Eq, PartialEq)]
    struct Process {
        pid: u32,
        parent: u32,
        start: u64,
    }

    pub(super) fn locate(source: &str, session: &str) -> Result<String, String> {
        let kind = source
            .strip_prefix("harness.")
            .and_then(|tail| tail.split('.').next())
            .ok_or(NO_ROUTE)?;
        if session.is_empty()
            || session.len() > 200
            || !session
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || b"._:-".contains(&byte))
        {
            return Err(NO_ROUTE.into());
        }
        let expected: &[&str] = match kind {
            "codex" => &["codex", "resume", session],
            "claude" => &["claude", "--resume", session],
            "kiro" if session.starts_with("sess_") => &["kiro-cli", "chat", "--resume-id", session],
            _ => return Err(NO_ROUTE.into()),
        };
        let uid = fs::metadata("/proc/self").map_err(|_| NO_ROUTE)?.uid();
        let proc_entries = fs::read_dir("/proc").map_err(|_| NO_ROUTE)?;
        let mut roots = BTreeSet::new();
        let mut processes = BTreeMap::new();
        for (count, entry) in proc_entries.enumerate() {
            if count >= MAX_PROCESS_ENTRIES {
                return Err("Process discovery limit reached. Use the native terminal.".into());
            }
            let Ok(entry) = entry else { continue };
            let Some(pid) = entry
                .file_name()
                .to_str()
                .and_then(|name| name.parse::<u32>().ok())
            else {
                continue;
            };
            if entry.metadata().is_ok_and(|metadata| metadata.uid() == uid)
                && let Some(process) = process(pid)
                && let Some(cmdline) = read_limited(&entry.path().join("cmdline"), 8192)
                && exact_resume(&cmdline, expected)
            {
                roots.insert(pid);
                processes.insert(pid, process);
            }
        }
        if roots.is_empty() {
            return Err(NO_ROUTE.into());
        }
        for &pid in &roots {
            let mut current = pid;
            for _ in 0..MAX_ANCESTORS {
                let Some(ancestor) = process(current) else {
                    break;
                };
                processes.entry(current).or_insert(ancestor);
                if ancestor.parent <= 1 || ancestor.parent == current {
                    break;
                }
                current = ancestor.parent;
            }
        }
        let inventory = tmux(&["list-panes", "-a", "-F", "#{pane_id} #{pane_pid}"])?;
        let mut candidates = inventory.lines().filter_map(|line| {
            let (id, pid) = line.split_once(' ')?;
            let digits = id.strip_prefix('%')?;
            let pid = pid.parse::<u32>().ok()?;
            (!digits.is_empty()
                && digits.bytes().all(|byte| byte.is_ascii_digit())
                && processes.contains_key(&pid))
            .then_some((id, pid))
        });
        let (pane, pane_pid) = candidates.next().ok_or(NO_ROUTE)?;
        if candidates.next().is_some() {
            return Err(NO_ROUTE.into());
        }
        // Re-read both the pane's process generation and the exact resume
        // command after inventory before changing terminal focus.
        let mut verified = false;
        for &root in &roots {
            let mut current = root;
            if !read_limited(
                &Path::new("/proc").join(root.to_string()).join("cmdline"),
                8192,
            )
            .is_some_and(|command| exact_resume(&command, expected))
            {
                continue;
            }
            for _ in 0..MAX_ANCESTORS {
                let Some(recorded) = processes.get(&current) else {
                    break;
                };
                if process(current) != Some(*recorded) {
                    break;
                }
                if current == pane_pid {
                    verified = true;
                    break;
                }
                current = recorded.parent;
            }
        }
        if !verified {
            return Err(NO_ROUTE.into());
        }
        if tmux(&["display-message", "-p", "-t", pane, "#{pane_pid}"])?
            .trim()
            .parse::<u32>()
            .ok()
            != Some(pane_pid)
        {
            return Err(NO_ROUTE.into());
        }
        tmux(&["select-window", "-t", pane])?;
        tmux(&["select-pane", "-t", pane])?;
        Ok(format!(
            "Selected tmux pane {pane}. Respond in the agent terminal."
        ))
    }

    fn read_limited(path: &Path, limit: usize) -> Option<Vec<u8>> {
        let mut bytes = Vec::new();
        fs::File::open(path)
            .ok()?
            .take(limit as u64 + 1)
            .read_to_end(&mut bytes)
            .ok()?;
        (bytes.len() <= limit).then_some(bytes)
    }

    fn process(pid: u32) -> Option<Process> {
        let raw = read_limited(&Path::new("/proc").join(pid.to_string()).join("stat"), 4096)?;
        let raw = std::str::from_utf8(&raw).ok()?;
        let (_, suffix) = raw.rsplit_once(')')?;
        let fields = suffix.split_whitespace().collect::<Vec<_>>();
        if *fields.first()? == "Z" {
            return None;
        }
        Some(Process {
            pid,
            parent: fields.get(1)?.parse().ok()?,
            start: fields.get(19)?.parse().ok()?,
        })
    }

    fn exact_resume(raw: &[u8], expected: &[&str]) -> bool {
        let Some(raw) = raw.strip_suffix(&[0]) else {
            return false;
        };
        let mut words = raw.split(|byte| *byte == 0);
        let Some(executable) = words
            .next()
            .and_then(|arg| std::str::from_utf8(arg).ok())
            .and_then(|arg| Path::new(arg).file_name())
            .and_then(|name| name.to_str())
        else {
            return false;
        };
        executable == expected[0] && words.eq(expected[1..].iter().map(|arg| arg.as_bytes()))
    }

    fn tmux(args: &[&str]) -> Result<String, String> {
        let mut child = Command::new("tmux")
            .args(args)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .map_err(|_| NO_ROUTE)?;
        let mut stdout = child.stdout.take().expect("tmux output piped").take(65537);
        let reader = thread::spawn(move || {
            let mut text = Vec::new();
            stdout.read_to_end(&mut text).map(|_| text)
        });
        let deadline = Instant::now() + Duration::from_secs(2);
        let status = loop {
            match child.try_wait() {
                Ok(Some(status)) => break Ok(status),
                Ok(None) if Instant::now() < deadline => thread::sleep(Duration::from_millis(20)),
                _ => {
                    let _ = child.kill();
                    let _ = child.wait();
                    break Err(NO_ROUTE);
                }
            }
        };
        let bytes = reader.join().map_err(|_| NO_ROUTE)?.map_err(|_| NO_ROUTE)?;
        if !status?.success() || bytes.len() > 65536 {
            return Err(NO_ROUTE.into());
        }
        String::from_utf8(bytes).map_err(|_| NO_ROUTE.into())
    }

    #[cfg(test)]
    mod tests {
        use super::*;
        #[test]
        fn resume_matching_is_exact_and_provider_specific() {
            let args = &["codex", "resume", "session-22"];
            assert!(exact_resume(b"/usr/bin/codex\0resume\0session-22\0", args));
            assert!(!exact_resume(b"codex\0resume\0session-220\0", args));
            assert!(!exact_resume(b"other\0resume\0session-22\0", args));
            assert!(!exact_resume(b"codex\0resume\0session-22\0--extra\0", args));
        }
    }
}

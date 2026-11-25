//! Route explicit user requests to a uniquely verified local tmux pane, or to
//! the recorded owner's terminal on the receiver host through an explicitly
//! configured authenticated route.

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

/// Resolve the recorded owner's terminal on the receiver host. The route
/// words are the operator's own authenticated transport (for example
/// `ssh user@receiver-host`); Agenturm appends only fixed, read-only tmux
/// and `/proc` probes and joins arguments the way `ssh` does. The resolved
/// unique target is presented for the operator to attach: routing never
/// selects a remote pane, answers, or approves the request.
pub(crate) fn remote_locate(
    route: &[String],
    source: &str,
    session: &str,
) -> Result<String, String> {
    #[cfg(target_os = "linux")]
    {
        linux::remote_locate(route, source, session)
    }
    #[cfg(not(target_os = "linux"))]
    {
        let _ = (route, source, session);
        Err("Live terminal routing is available on Linux".into())
    }
}

/// Read an operator-authored route file: one command word per line. The file
/// is explicit configuration, not discovered state; an unusable file is a
/// startup error rather than a silent absence.
pub(crate) fn load_route_file(path: &std::path::Path) -> Result<Vec<String>, String> {
    fn read(path: &std::path::Path) -> Result<Vec<String>, String> {
        #[cfg(target_os = "linux")]
        {
            linux::load_route_file(path)
        }
        #[cfg(not(target_os = "linux"))]
        {
            let _ = path;
            Err("Host routes are available on Linux".into())
        }
    }
    read(path)
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

    const NO_ROUTE: &str = "No unique live tmux pane was verified. Open this session in its native terminal. If the execution is on a receiver host, configure an authenticated host route with --route-file and reopen the cockpit.";
    const INVALID_RESUME: &str = "This request has no verifiable native resume command for its provider or session identifier.";
    const REMOTE_NO_ROUTE: &str = "No unique live tmux pane was verified on the receiver host through the configured route. Open this session in its native terminal there.";
    const REMOTE_UNAVAILABLE: &str = "The configured host route did not answer with the expected bounded output. Verify the route command and its authentication; this cockpit never guesses or selects a remote pane.";
    const MAX_PROCESS_ENTRIES: usize = 4096;
    const MAX_ANCESTORS: usize = 64;
    const MAX_ROUTE_WORDS: usize = 32;
    const MAX_ROUTE_WORD_BYTES: usize = 256;
    const MAX_ROUTE_OUTPUT: usize = 256 * 1024;
    const ROUTE_DEADLINE: Duration = Duration::from_secs(5);

    #[derive(Clone, Copy, Debug, Eq, PartialEq)]
    struct Process {
        pid: u32,
        parent: u32,
        start: u64,
    }

    /// The provider's native resume executable and its fixed arguments. The
    /// recorded session is always the final argument.
    fn expected_resume(
        source: &str,
        session: &str,
    ) -> Result<(&'static str, &'static [&'static str]), String> {
        let kind = source
            .strip_prefix("harness.")
            .and_then(|tail| tail.split('.').next())
            .ok_or(INVALID_RESUME)?;
        if session.is_empty()
            || session.len() > 200
            || !session
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || b"._:-".contains(&byte))
        {
            return Err(INVALID_RESUME.into());
        }
        match kind {
            "codex" => Ok(("codex", &["resume"])),
            "claude" => Ok(("claude", &["--resume"])),
            "kiro" if session.starts_with("sess_") => Ok(("kiro-cli", &["chat", "--resume-id"])),
            _ => Err(INVALID_RESUME.into()),
        }
    }

    pub(super) fn load_route_file(path: &Path) -> Result<Vec<String>, String> {
        let raw = fs::read_to_string(path)
            .map_err(|_| "The configured host route file could not be read".to_owned())?;
        let route: Vec<String> = raw
            .lines()
            .map(str::trim)
            .filter(|line| !line.is_empty())
            .map(str::to_owned)
            .collect();
        if route.is_empty() {
            return Err("The configured host route file contains no command".into());
        }
        validate_route(&route)?;
        Ok(route)
    }

    fn validate_route(route: &[String]) -> Result<(), String> {
        if route.is_empty() || route.len() > MAX_ROUTE_WORDS {
            return Err("The configured host route is invalid".into());
        }
        for word in route {
            // Printable non-space ASCII keeps the route safe to re-quote in
            // the presented attach command and to pass through transports.
            if word.is_empty()
                || word.len() > MAX_ROUTE_WORD_BYTES
                || !word.bytes().all(|byte| (0x21..=0x7e).contains(&byte))
            {
                return Err("The configured host route is invalid".into());
            }
        }
        Ok(())
    }

    pub(super) fn remote_locate(
        route: &[String],
        source: &str,
        session: &str,
    ) -> Result<String, String> {
        let (executable, fixed) = expected_resume(source, session)?;
        validate_route(route)?;
        // The compared bytes are everything after the executable word: one
        // NUL, each fixed argument followed by one NUL, then the session and
        // its terminating NUL.
        let mut tail = Vec::new();
        for argument in fixed.iter().copied().chain(std::iter::once(session)) {
            tail.push(0);
            tail.extend_from_slice(argument.as_bytes());
        }
        let separators = fixed.len() + 1;
        let script = remote_probe_script(executable, separators, tail.len(), &tail);
        let probe = format!("echo {}|base64 -d|sh", encode_base64(script.as_bytes()));
        let processes = route_probe(route, &[&probe])?;
        let panes = route_probe(
            route,
            &[
                "tmux",
                "list-panes",
                "-a",
                "-F",
                "'#{pane_id}:#{pane_pid}:#{session_name}'",
            ],
        )?;
        let (pane, pane_pid, session_name) = resolve_inventory(&processes, &panes)?;
        // Re-read the pane's recorded process after resolution, mirroring the
        // local re-verification before the route is presented.
        let recheck = route_probe(
            route,
            &[
                "tmux",
                "display-message",
                "-p",
                "-t",
                &pane,
                "'#{pane_pid}'",
            ],
        )?;
        if recheck.trim() != pane_pid.to_string() {
            return Err(REMOTE_NO_ROUTE.into());
        }
        let attach = if session_name_safe(&session_name) {
            format!(
                "{} tmux attach -t {session_name}, then select pane {pane}",
                route.join(" ")
            )
        } else {
            format!("{} tmux attach, then select pane {pane}", route.join(" "))
        };
        Ok(format!(
            "Verified unique remote tmux pane {pane} on the receiver host for this request. \
             Attach with: {attach}. Respond in the agent terminal; Seen only acknowledges locally."
        ))
    }

    /// tmux session names cannot contain ':' or '.', so the presented attach
    /// target stays a single shell word.
    fn session_name_safe(name: &str) -> bool {
        !name.is_empty()
            && name.len() <= 64
            && !name.contains(':')
            && !name.contains('.')
            && name.bytes().all(|byte| (0x21..=0x7e).contains(&byte))
    }

    /// The fixed remote `/proc` probe. It carries no operator or session
    /// bytes in its text: the expected argument tail travels base64-encoded,
    /// and the whole script travels base64-encoded as one shell-safe word so
    /// the receiver host's login shell cannot re-interpret its quoting.
    fn remote_probe_script(
        executable: &str,
        separators: usize,
        tail_len: usize,
        tail: &[u8],
    ) -> String {
        format!(
            "t=$(mktemp \"${{TMPDIR:-/tmp}}/agenturm-route.XXXXXXXXXXXX\") || exit 0
trap 'rm -f \"$t\"' EXIT HUP INT
echo {}|base64 -d>\"$t\" || exit 0
c=0
for d in /proc/[0-9]*; do
  [ -O \"$d\" ] || continue
  c=$((c+1)); [ \"$c\" -gt {MAX_PROCESS_ENTRIES} ] && exit 0
  s=$(cat \"$d/stat\" 2>/dev/null) || continue
  r=${{s##*) }}
  set -- $r
  [ \"$1\" = Z ] && continue
  p=${{d#/proc/}}
  pp=$2; tm=$20
  m=0
  if [ -r \"$d/cmdline\" ]; then
    z=$(tr -cd '\\0' <\"$d/cmdline\"|wc -c)
    if [ \"$z\" = {separators} ]; then
      w=$(tr '\\0' '\\n' <\"$d/cmdline\" 2>/dev/null)
      set -- $w
      case ${{1##*/}} in
        {executable}) tail -c {tail_len} \"$d/cmdline\" 2>/dev/null|cmp -s - \"$t\" && m=1 ;;
      esac
    fi
  fi
  printf 'P %s %s %s %s\\n' \"$p\" \"$pp\" \"$tm\" \"$m\"
done
exit 0",
            encode_base64(tail)
        )
    }

    /// Cross-reference one remote process snapshot with the tmux pane list and
    /// resolve the unique verified target, mirroring the local proof.
    fn resolve_inventory(processes: &str, panes: &str) -> Result<(String, u32, String), String> {
        let mut table: BTreeMap<u32, Process> = BTreeMap::new();
        let mut roots: BTreeSet<u32> = BTreeSet::new();
        for line in processes.lines().take(MAX_PROCESS_ENTRIES) {
            let mut fields = line.split(' ');
            let (Some("P"), Some(pid), Some(parent), Some(start), Some(matched)) = (
                fields.next(),
                fields.next(),
                fields.next(),
                fields.next(),
                fields.next(),
            ) else {
                continue;
            };
            let (Ok(pid), Ok(parent), Ok(start), Ok(matched)) = (
                pid.parse::<u32>(),
                parent.parse::<u32>(),
                start.parse::<u64>(),
                matched.parse::<u8>(),
            ) else {
                continue;
            };
            if fields.next().is_some() {
                continue;
            }
            table.insert(pid, Process { pid, parent, start });
            if matched == 1 {
                roots.insert(pid);
            }
        }
        if roots.is_empty() {
            return Err(REMOTE_NO_ROUTE.into());
        }
        // Keep only the roots and their recorded ancestors, as the local
        // reader does, so unrelated processes cannot introduce pane candidates.
        let mut closure: BTreeMap<u32, Process> = BTreeMap::new();
        for &root in &roots {
            let mut current = root;
            for _ in 0..MAX_ANCESTORS {
                let Some(recorded) = table.get(&current).copied() else {
                    break;
                };
                closure.insert(current, recorded);
                if recorded.parent <= 1 || recorded.parent == current {
                    break;
                }
                current = recorded.parent;
            }
        }
        let mut candidates = panes.lines().take(MAX_PROCESS_ENTRIES).filter_map(|line| {
            let (id, rest) = line.split_once(':')?;
            let (pid, name) = rest.split_once(':')?;
            let digits = id.strip_prefix('%')?;
            let pid = pid.parse::<u32>().ok()?;
            (!digits.is_empty()
                && digits.bytes().all(|byte| byte.is_ascii_digit())
                && closure.contains_key(&pid))
            .then_some((id.to_owned(), pid, name.to_owned()))
        });
        let (pane, pane_pid, session_name) = candidates.next().ok_or(REMOTE_NO_ROUTE)?;
        if candidates.next().is_some() {
            return Err(REMOTE_NO_ROUTE.into());
        }
        for &root in &roots {
            let mut current = root;
            for _ in 0..MAX_ANCESTORS {
                if current == pane_pid {
                    return Ok((pane, pane_pid, session_name));
                }
                let Some(recorded) = closure.get(&current) else {
                    break;
                };
                if recorded.parent <= 1 || recorded.parent == current {
                    break;
                }
                current = recorded.parent;
            }
        }
        Err(REMOTE_NO_ROUTE.into())
    }

    /// Run one bounded probe through the configured route. The route is
    /// expected to transport arguments like `ssh`: joined and executed by the
    /// receiver host's login shell.
    fn route_probe(route: &[String], args: &[&str]) -> Result<String, String> {
        let Some((first, rest)) = route.split_first() else {
            return Err(REMOTE_UNAVAILABLE.into());
        };
        let mut command = Command::new(first);
        command
            .args(rest)
            .args(args)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::null());
        let mut child = command.spawn().map_err(|_| REMOTE_UNAVAILABLE)?;
        let mut stdout = child
            .stdout
            .take()
            .expect("route output piped")
            .take(MAX_ROUTE_OUTPUT as u64 + 1);
        let reader = thread::spawn(move || {
            let mut text = Vec::new();
            stdout.read_to_end(&mut text).map(|_| text)
        });
        let deadline = Instant::now() + ROUTE_DEADLINE;
        let status = loop {
            match child.try_wait() {
                Ok(Some(status)) => break Ok(status),
                Ok(None) if Instant::now() < deadline => thread::sleep(Duration::from_millis(20)),
                _ => {
                    let _ = child.kill();
                    let _ = child.wait();
                    break Err(REMOTE_UNAVAILABLE);
                }
            }
        };
        let bytes = match reader.join() {
            Ok(Ok(bytes)) => bytes,
            _ => return Err(REMOTE_UNAVAILABLE.into()),
        };
        if !status.map_err(|_| REMOTE_UNAVAILABLE)?.success() || bytes.len() > MAX_ROUTE_OUTPUT {
            return Err(REMOTE_UNAVAILABLE.into());
        }
        String::from_utf8(bytes).map_err(|_| REMOTE_UNAVAILABLE.into())
    }

    /// Standard-alphabet base64 for one shell-safe probe word.
    fn encode_base64(bytes: &[u8]) -> String {
        const ALPHABET: &[u8; 64] =
            b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
        let mut encoded = String::with_capacity(bytes.len().div_ceil(3) * 4);
        for chunk in bytes.as_chunks::<3>().0 {
            let value = u32::from_be_bytes([0, chunk[0], chunk[1], chunk[2]]);
            encoded.push_str(
                std::str::from_utf8(&[
                    ALPHABET[(value >> 18) as usize & 63],
                    ALPHABET[(value >> 12) as usize & 63],
                    ALPHABET[(value >> 6) as usize & 63],
                    ALPHABET[value as usize & 63],
                ])
                .expect("base64 alphabet is valid UTF-8"),
            );
        }
        match bytes.as_chunks::<3>().1 {
            [a] => {
                let value = u32::from_be_bytes([0, *a, 0, 0]);
                encoded.push_str(
                    std::str::from_utf8(&[
                        ALPHABET[(value >> 18) as usize & 63],
                        ALPHABET[(value >> 12) as usize & 63],
                        b'=',
                        b'=',
                    ])
                    .expect("base64 alphabet is valid UTF-8"),
                );
            }
            [a, b] => {
                let value = u32::from_be_bytes([0, *a, *b, 0]);
                encoded.push_str(
                    std::str::from_utf8(&[
                        ALPHABET[(value >> 18) as usize & 63],
                        ALPHABET[(value >> 12) as usize & 63],
                        ALPHABET[(value >> 6) as usize & 63],
                        b'=',
                    ])
                    .expect("base64 alphabet is valid UTF-8"),
                );
            }
            _ => {}
        }
        encoded
    }

    pub(super) fn locate(source: &str, session: &str) -> Result<String, String> {
        let (executable, fixed) = expected_resume(source, session)?;
        let mut words: Vec<&str> = vec![executable];
        words.extend(fixed);
        words.push(session);
        let expected: &[&str] = &words;
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

        #[test]
        fn expected_resume_keeps_the_provider_table_and_session_fence() {
            assert_eq!(
                expected_resume("harness.codex", "session-22").unwrap(),
                ("codex", &["resume"][..])
            );
            assert_eq!(
                expected_resume("harness.claude.local", "session-22").unwrap(),
                ("claude", &["--resume"][..])
            );
            assert_eq!(
                expected_resume("harness.kiro", "sess_abc-1").unwrap(),
                ("kiro-cli", &["chat", "--resume-id"][..])
            );
            assert!(expected_resume("harness.kiro", "session-22").is_err());
            assert!(expected_resume("harness.other", "session-22").is_err());
            assert!(expected_resume("source.codex", "session-22").is_err());
            assert!(expected_resume("harness.codex", "").is_err());
            assert!(expected_resume("harness.codex", "bad session").is_err());
            assert!(expected_resume("harness.codex", &"x".repeat(201)).is_err());
        }

        #[test]
        fn unavailable_route_guidance_is_explicit_and_never_guesses_a_target() {
            assert!(NO_ROUTE.contains("--route-file"));
            assert!(NO_ROUTE.contains("native terminal"));
            assert!(REMOTE_NO_ROUTE.contains("configured route"));
            assert!(REMOTE_UNAVAILABLE.contains("never guesses or selects a remote pane"));
            assert!(INVALID_RESUME.contains("provider or session identifier"));
            assert!(!INVALID_RESUME.contains("route-file"));
        }

        #[test]
        fn resolve_inventory_requires_one_verified_ancestry_path() {
            let processes = "P 100 90 5 1\nP 90 80 6 0\nP 80 1 7 0\nP 50 40 8 0\n";
            let (pane, pid, name) =
                resolve_inventory(processes, "%3:80:main\n%4:50:other-work\n").unwrap();
            assert_eq!((pane.as_str(), pid, name.as_str()), ("%3", 80, "main"));
            // A second pane inside the verified closure is ambiguous.
            assert!(resolve_inventory(processes, "%3:80:main\n%5:90:second\n").is_err());
            // A pane outside the roots' ancestry is not a candidate.
            assert!(resolve_inventory(processes, "%4:50:other-work\n").is_err());
            // No matching root at all.
            assert!(resolve_inventory("", "%3:80:main\n").is_err());
            // A root without any pane in its ancestry.
            assert!(resolve_inventory(processes, "").is_err());
            // Malformed lines never become evidence.
            assert!(
                resolve_inventory(
                    "P 100 90 5 1 x\nP ninety 90 5 0\nP 90 80 6 0\n",
                    "%3:80:main\n"
                )
                .is_err()
            );
        }

        #[test]
        fn route_words_are_bounded_printable_and_shell_quotable() {
            assert!(validate_route(&["ssh".into(), "user@receiver".into()]).is_ok());
            assert!(validate_route(&[]).is_err());
            assert!(validate_route(&["".into()]).is_err());
            assert!(validate_route(&["has space".into()]).is_err());
            assert!(validate_route(&["tab\tword".into()]).is_err());
            // Quotes and other printable punctuation are the operator's own
            // transport arguments; they pass through untouched.
            assert!(validate_route(&["quote'word".into()]).is_ok());
            assert!(validate_route(&["x".repeat(257)]).is_err());
            let many = vec!["word".to_owned(); MAX_ROUTE_WORDS + 1];
            assert!(validate_route(&many).is_err());
        }

        #[test]
        fn remote_probe_script_carries_session_bytes_only_base64_encoded() {
            let (_, fixed) = expected_resume("harness.codex", "session-22").unwrap();
            let mut tail = Vec::new();
            for argument in fixed.iter().copied().chain(std::iter::once("session-22")) {
                tail.push(0);
                tail.extend_from_slice(argument.as_bytes());
            }
            let script = remote_probe_script("codex", fixed.len() + 1, tail.len(), &tail);
            assert!(!script.contains("session-22"));
            assert!(script.contains("case ${1##*/} in"));
            assert!(script.contains("codex)"));
            assert!(script.contains("tail -c "));
            assert!(script.contains("|cmp -s - \"$t\""));
            assert!(script.contains("agenturm-route."));
        }

        #[test]
        fn base64_encoding_matches_the_standard_alphabet() {
            assert_eq!(encode_base64(b""), "");
            assert_eq!(encode_base64(b"f"), "Zg==");
            assert_eq!(encode_base64(b"fo"), "Zm8=");
            assert_eq!(encode_base64(b"foo"), "Zm9v");
            assert_eq!(encode_base64(b"foobar"), "Zm9vYmFy");
        }

        fn route_script_file(body: &str) -> std::path::PathBuf {
            use std::os::unix::fs::PermissionsExt;
            let path = std::env::temp_dir().join(format!(
                "agenturm-route-fake-{}-{:?}",
                std::process::id(),
                std::thread::current().id()
            ));
            fs::write(&path, body).unwrap();
            let mut permissions = fs::metadata(&path).unwrap().permissions();
            permissions.set_mode(0o700);
            fs::set_permissions(&path, permissions).unwrap();
            path
        }

        /// A fake route that emulates `ssh` argument joining and answers the
        /// three fixed probes with a consistent snapshot: one matching root
        /// (100) whose ancestry leads to pane %3 (pid 80).
        #[test]
        fn remote_locate_resolves_a_unique_target_through_a_fake_route() {
            let script = "#!/bin/sh
case \"$*\" in
  *list-panes*) printf '%s\\n' '%3:80:main' ;;
  *display-message*) printf '%s\\n' '80' ;;
  *\"base64 -d\"*) printf '%s\\n' 'P 100 90 5 1' 'P 90 80 6 0' 'P 80 1 7 0' ;;
  *) exit 1 ;;
esac
";
            let path = route_script_file(script);
            let route = vec![path.to_string_lossy().into_owned()];
            let message = remote_locate(&route, "harness.codex", "session-22").unwrap();
            assert!(message.contains("Verified unique remote tmux pane %3"));
            assert!(message.contains("tmux attach -t main"));
            assert!(message.contains("Seen only acknowledges locally"));
            // An ambiguous snapshot stays unavailable, never a guessed target.
            let ambiguous = script.replace("'%3:80:main'", "'%3:80:main' '%4:90:other'");
            let path = route_script_file(&ambiguous);
            let route = vec![path.to_string_lossy().into_owned()];
            assert!(remote_locate(&route, "harness.codex", "session-22").is_err());
            // A pane whose process disappeared between probes is re-verified.
            let stale = script.replace("printf '%s\\n' '80'", "printf '%s\\n' '81'");
            let path = route_script_file(&stale);
            let route = vec![path.to_string_lossy().into_owned()];
            assert!(remote_locate(&route, "harness.codex", "session-22").is_err());
            let _ = fs::remove_file(path);
        }

        #[test]
        fn route_file_loading_rejects_unusable_configuration() {
            let path = std::env::temp_dir().join(format!(
                "agenturm-route-file-{}-{:?}",
                std::process::id(),
                std::thread::current().id()
            ));
            fs::write(&path, "ssh\n  \nuser@receiver-host\n\n-p\n2222\n").unwrap();
            assert_eq!(
                load_route_file(&path).unwrap(),
                vec!["ssh", "user@receiver-host", "-p", "2222"]
            );
            fs::write(&path, "\n\n").unwrap();
            assert!(load_route_file(&path).is_err());
            fs::write(&path, "has space\n").unwrap();
            assert!(load_route_file(&path).is_err());
            let _ = fs::remove_file(&path);
        }
    }
}

//! Numeric Linux procfs sampling for explicit local and broker collectors.

use super::{MAX_PROCESS_ENTRIES, PROCESS_SCAN_BUDGET, proc_field, read_kib, read_proc};
use agenturm_kernel::{
    HostCpu, HostProcess, HostProcessState, HostRuntime, HostSample, MAX_HOST_PROCESSES,
};
use std::fs::{File, OpenOptions};
use std::io::{BufRead, BufReader, Read};
use std::os::fd::AsRawFd;
use std::os::unix::ffi::OsStrExt;
use std::os::unix::fs::{MetadataExt, OpenOptionsExt};
use std::path::Path;
use std::time::Instant;

pub(crate) fn capture() -> Result<HostSample, String> {
    // SAFETY: these calls take no pointers and cannot mutate application state.
    let (uid, effective, hz) = unsafe {
        (
            libc::getuid(),
            libc::geteuid(),
            libc::sysconf(libc::_SC_CLK_TCK),
        )
    };
    if uid != effective {
        return Err("Host collection requires matching real and effective user IDs".into());
    }
    let root = Path::new("/proc");
    let boot_id = read_proc(root.join("sys/kernel/random/boot_id"))
        .ok_or("Cannot read the Linux boot identity")?;
    let load = read_proc(root.join("loadavg"));
    let mut load = load.as_deref().unwrap_or_default().split_whitespace();
    let memory = read_proc(root.join("meminfo"));
    let (processes, processes_partial) = processes(root, uid)?;
    let sample = HostSample {
        boot_id: boot_id.trim().into(),
        clock_ticks_per_second: u64::try_from(hz).ok().filter(|hz| *hz > 0),
        cpu: host_cpu(&root.join("stat")),
        load_1m_milli: load.next().and_then(load_milli),
        load_5m_milli: load.next().and_then(load_milli),
        load_15m_milli: load.next().and_then(load_milli),
        memory_total_bytes: memory.as_deref().and_then(|text| bytes(text, "MemTotal")),
        memory_available_bytes: memory
            .as_deref()
            .and_then(|text| bytes(text, "MemAvailable")),
        processes,
        processes_partial,
    };
    if !sample.is_valid() {
        return Err("Invalid Linux boot identity or process sample".into());
    }
    Ok(sample)
}

fn bytes(text: &str, field: &str) -> Option<u64> {
    read_kib(text, field)?.checked_mul(1024)
}

fn host_cpu(path: &Path) -> Option<HostCpu> {
    let mut line = String::new();
    BufReader::new(File::open(path).ok()?.take(1024))
        .read_line(&mut line)
        .ok()?;
    if !line.ends_with('\n') {
        return None;
    }
    let mut fields = line.strip_prefix("cpu ")?.split_whitespace();
    let mut tick = || fields.next().and_then(|field| field.parse::<u64>().ok());
    Some(HostCpu {
        user_ticks: tick(),
        nice_ticks: tick(),
        system_ticks: tick(),
        idle_ticks: tick(),
        io_wait_ticks: tick(),
        irq_ticks: tick(),
        soft_irq_ticks: tick(),
        steal_ticks: tick(),
    })
}

fn load_milli(value: &str) -> Option<u64> {
    let (whole, fraction) = value.split_once('.')?;
    let scale = match fraction.len() {
        1 => 100,
        2 => 10,
        3 => 1,
        _ => return None,
    };
    whole
        .parse::<u64>()
        .ok()?
        .checked_mul(1000)?
        .checked_add(fraction.parse::<u64>().ok()? * scale)
}

fn processes(root: &Path, uid: u32) -> Result<(Vec<HostProcess>, bool), String> {
    let mut rows = Vec::new();
    let mut partial = false;
    let started = Instant::now();
    for (index, entry) in std::fs::read_dir(root)
        .map_err(|_| "Cannot scan Linux procfs")?
        .enumerate()
    {
        if index == MAX_PROCESS_ENTRIES || started.elapsed() >= PROCESS_SCAN_BUDGET {
            partial = true;
            break;
        }
        let Ok(entry) = entry else {
            partial = true;
            continue;
        };
        let name = entry.file_name();
        let Some(pid) = name
            .to_str()
            .and_then(|name| name.parse::<u32>().ok())
            .filter(|pid| *pid > 0)
        else {
            continue;
        };
        // The held procfs directory refers to this process instance, even if
        // the path at /proc/<pid> is reused after it exits.
        let Ok(directory) = OpenOptions::new()
            .read(true)
            .custom_flags(libc::O_DIRECTORY | libc::O_NOFOLLOW)
            .open(entry.path())
        else {
            continue;
        };
        if !directory
            .metadata()
            .is_ok_and(|metadata| metadata.uid() == uid)
        {
            continue;
        }
        if let Some(row) = process(&directory, pid, uid) {
            if rows.len() == MAX_HOST_PROCESSES {
                partial = true;
                break;
            }
            rows.push(row);
        }
    }
    rows.sort_unstable_by_key(|row| row.pid);
    Ok((rows, partial))
}

fn process(directory: &File, pid: u32, uid: u32) -> Option<HostProcess> {
    let path = format!("/proc/self/fd/{}", directory.as_raw_fd());
    let process = Path::new(&path);
    let status = read_proc(process.join("status"))?;
    if !owns_status(&status, uid) {
        return None;
    }
    let name = proc_field(&status, "Name")?;
    let runtime = recognized_runtime(name).or_else(|| {
        let executable = name
            .starts_with("agenturm-")
            .then(|| std::fs::read_link(process.join("exe")).ok())
            .flatten()?;
        recognized_runtime(executable.file_name()?.to_str()?)
    })?;
    let first = read_proc(process.join("stat"))?;
    let start_ticks = parse_stat(&first, pid)?.start_ticks;
    let working_directory_name = match runtime {
        HostRuntime::Codex
        | HostRuntime::Pi
        | HostRuntime::Claude
        | HostRuntime::Kiro
        | HostRuntime::OpenCode => std::fs::read_link(process.join("cwd"))
            .ok()
            .and_then(|path| working_directory_name(&path)),
        _ => None,
    };
    let status = read_proc(process.join("status"))?;
    if !owns_status(&status, uid) || proc_field(&status, "Name") != Some(name) {
        return None;
    }
    let final_stat = read_proc(process.join("stat"))?;
    let stat = parse_stat(&final_stat, pid)?;
    if start_ticks != stat.start_ticks
        || !directory
            .metadata()
            .is_ok_and(|metadata| metadata.uid() == uid)
    {
        return None;
    }
    Some(HostProcess {
        pid,
        parent_pid: stat.parent_pid,
        start_ticks,
        runtime,
        state: stat.state,
        resident_bytes: bytes(&status, "VmRSS"),
        user_ticks: stat.user_ticks,
        system_ticks: stat.system_ticks,
        working_directory_name,
    })
}

fn working_directory_name(target: &Path) -> Option<String> {
    if !target.is_absolute() || target.as_os_str().as_bytes().ends_with(b" (deleted)") {
        return None;
    }
    let name = target.file_name()?.to_str()?;
    if !HostProcess::valid_working_directory_name(name) {
        return None;
    }
    agenturm_store::bounded_text_preview(
        name.to_owned(),
        HostProcess::MAX_WORKING_DIRECTORY_NAME_BYTES,
    )
    .ok()
}

fn owns_status(text: &str, uid: u32) -> bool {
    let mut ids = proc_field(text, "Uid")
        .unwrap_or_default()
        .split_whitespace();
    (0..4).all(|_| ids.next().and_then(|id| id.parse::<u32>().ok()) == Some(uid))
}

struct ProcessStat {
    start_ticks: u64,
    parent_pid: Option<u32>,
    state: Option<HostProcessState>,
    user_ticks: Option<u64>,
    system_ticks: Option<u64>,
}

fn parse_stat(stat: &str, expected_pid: u32) -> Option<ProcessStat> {
    let (identity, fields) = stat.rsplit_once(") ")?;
    let (pid, _) = identity.split_once(" (")?;
    let pid: u32 = pid.parse().ok()?;
    if pid != expected_pid {
        return None;
    }
    let mut fields = fields.split_whitespace();
    let state = match fields.next()? {
        "R" => Some(HostProcessState::Running),
        "S" => Some(HostProcessState::Sleeping),
        "D" => Some(HostProcessState::Uninterruptible),
        "T" => Some(HostProcessState::Stopped),
        "t" => Some(HostProcessState::Tracing),
        "I" => Some(HostProcessState::Idle),
        "P" => Some(HostProcessState::Parked),
        "Z" | "X" | "x" => return None,
        _ => None,
    };
    let parent_pid = match fields.next()?.parse::<u32>().ok()? {
        0 => None,
        parent if parent != pid => Some(parent),
        _ => return None,
    };
    let user_ticks = fields.nth(9)?.parse().ok();
    let system_ticks = fields.next()?.parse().ok();
    let start_ticks = fields.nth(6)?.parse().ok()?;
    Some(ProcessStat {
        start_ticks,
        parent_pid,
        state,
        user_ticks,
        system_ticks,
    })
}

pub(super) fn recognized_runtime(name: &str) -> Option<HostRuntime> {
    use HostRuntime::*;
    match name {
        "agenturm" => Some(Agenturm),
        "agenturm-plugin-host" => Some(PluginHost),
        "agenturm-storage-service" => Some(StorageService),
        "agenturm-codex-harness" => Some(CodexAdapter),
        "agenturm-pi-harness" => Some(PiAdapter),
        "agenturm-claude-harness" => Some(ClaudeAdapter),
        "agenturm-kiro-harness" => Some(KiroAdapter),
        "agenturm-opencode-harness" => Some(OpenCodeAdapter),
        "agenturm-connector" => Some(Connector),
        "codex" => Some(Codex),
        "pi" => Some(Pi),
        "claude" | "claude-code" => Some(Claude),
        "kiro-cli" | "kiro-cli-chat" => Some(Kiro),
        "opencode" => Some(OpenCode),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::ffi::OsStr;

    #[test]
    fn process_stat_parses_parent_after_a_name_containing_parentheses() {
        let stat_line = |parent: &str| {
            format!(
                "145 (odd) process (name)) S {parent} 1 2 0 -1 0 3 0 1 0 17 19 0 0 20 0 1 0 12345"
            )
        };
        let stat = parse_stat(&stat_line("293"), 145).unwrap();
        assert_eq!(stat.parent_pid, Some(293));
        assert_eq!(stat.state, Some(HostProcessState::Sleeping));
        assert_eq!(stat.user_ticks, Some(17));
        assert_eq!(stat.system_ticks, Some(19));
        assert_eq!(stat.start_ticks, 12345);
        assert!(
            parse_stat(&stat_line("0"), 145)
                .unwrap()
                .parent_pid
                .is_none()
        );
        assert!(parse_stat(&stat_line("293"), 146).is_none());
        assert!(parse_stat(&stat_line("145"), 145).is_none());
        assert!(parse_stat(&stat_line("-1"), 145).is_none());
    }

    #[test]
    fn process_label_keeps_only_a_safe_sampled_basename() {
        assert_eq!(
            working_directory_name(Path::new("/home/operator/作業 tree")),
            Some("作業 tree".into())
        );
        for path in [
            "relative/project",
            "/",
            "/root/project (deleted)",
            "/root/a\nname",
            "/root/a\u{202e}name",
            "/root/ghp_private",
        ] {
            assert_eq!(working_directory_name(Path::new(path)), None);
        }
        let invalid_utf8 = Path::new(OsStr::from_bytes(b"/root/\xff"));
        assert_eq!(working_directory_name(invalid_utf8), None);
    }
}

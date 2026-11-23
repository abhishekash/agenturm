use agenturm_kernel::{ExtensionId, MetricRow, Snapshot};
use agenturm_ui::{EvidenceRow, EvidenceSpec, EvidenceTable};
#[cfg(target_os = "linux")]
mod capture;
#[cfg(target_os = "linux")]
pub(super) use capture::capture;
#[cfg(target_os = "linux")]
use std::io::Read;
#[cfg(target_os = "linux")]
use std::path::Path;
#[cfg(target_os = "linux")]
use std::sync::Mutex;
#[cfg(target_os = "linux")]
use std::time::{Duration, Instant};

#[cfg(target_os = "linux")]
const MAX_PROCESS_ENTRIES: usize = 1024;
#[cfg(target_os = "linux")]
const PROCESS_SCAN_BUDGET: Duration = Duration::from_millis(40);
#[cfg(target_os = "linux")]
const PROCESS_SAMPLE_AGE: Duration = Duration::from_secs(2);

pub(super) fn add_host_pane(snapshot: &mut Snapshot) -> (ExtensionId, EvidenceTable) {
    let mut rows = current_host_rows();
    if rows.is_empty() {
        rows.push(MetricRow {
            label: "Host metrics".into(),
            value: "Current host metrics are unavailable".into(),
        });
    }
    let projection = EvidenceSpec {
        title: "Host now",
        inspector: "Collection status",
        columns: &[("Metric", 5), ("Current sample", 4)],
        empty: "Current host metrics are unavailable.",
    };
    let mut table = projection.table(Vec::new());
    if let Some(coverage) = rows
        .iter()
        .find(|row| row.label == "Process sample" || row.label == "Host metrics")
    {
        table.context.push(coverage.clone());
    }
    table.rows = rows
        .iter()
        .filter(|row| !matches!(row.label.as_str(), "Process sample" | "Host metrics"))
        .map(|row| EvidenceRow {
            cells: vec![row.label.clone(), row.value.clone()],
            details: vec![row.clone()],
        })
        .collect();
    let id = super::session_evidence::insert_pane(snapshot, "app.cockpit-host", "Host now", rows);
    (id, table)
}

#[cfg(target_os = "linux")]
fn current_host_rows() -> Vec<MetricRow> {
    let row = |label: &str, value: String| MetricRow {
        label: label.into(),
        value,
    };
    let mut rows = Vec::new();
    if let Some(load) = read_proc("/proc/loadavg") {
        let values = load
            .split_whitespace()
            .take(3)
            .map(str::parse::<f64>)
            .collect::<Result<Vec<_>, _>>();
        if let Ok(values) = values
            && let [one, five, fifteen] = values.as_slice()
            && values
                .iter()
                .all(|value| value.is_finite() && *value >= 0.0)
        {
            rows.push(row(
                "Load average (1m, 5m, 15m)",
                format!("{one:.2} · {five:.2} · {fifteen:.2}"),
            ));
        }
    }
    if let Some(memory) = read_proc("/proc/meminfo") {
        for (field, label) in [
            ("MemTotal", "Host memory total"),
            ("MemAvailable", "Host memory available"),
        ] {
            if let Some(ki_b) = read_kib(&memory, field) {
                rows.push(row(label, format!("{} MiB", ki_b / 1024)));
            }
        }
    }
    if let Some(status) = read_proc("/proc/self/status")
        && let Some(ki_b) = read_kib(&status, "VmRSS")
    {
        rows.push(row(
            "Agenturm resident memory",
            format!("{} MiB", ki_b / 1024),
        ));
    }
    if let Some(sample) = current_process_sample() {
        rows.extend(process_rows(sample));
    } else {
        rows.push(row("Process sample", "Unavailable".into()));
    }
    rows
}

#[cfg(not(target_os = "linux"))]
fn current_host_rows() -> Vec<MetricRow> {
    Vec::new()
}

#[cfg(target_os = "linux")]
fn read_proc(path: impl AsRef<Path>) -> Option<String> {
    let mut value = String::new();
    std::fs::File::open(path)
        .ok()?
        .take(8193)
        .read_to_string(&mut value)
        .ok()?;
    (value.len() <= 8192).then_some(value)
}

#[cfg(target_os = "linux")]
fn read_kib(contents: &str, key: &str) -> Option<u64> {
    let mut words = proc_field(contents, key)?.split_whitespace();
    let size = words.next()?.parse().ok()?;
    (words.next() == Some("kB")).then_some(size)
}

#[cfg(target_os = "linux")]
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
struct ProcessTotals {
    count: usize,
    rss_kib: u64,
    missing_rss: usize,
}

#[cfg(target_os = "linux")]
impl ProcessTotals {
    fn record(&mut self, rss_kib: Option<u64>) {
        self.count += 1;
        if let Some(rss) = rss_kib {
            self.rss_kib = self.rss_kib.saturating_add(rss);
        } else {
            self.missing_rss += 1;
        }
    }

    fn display(self) -> String {
        if self.missing_rss == self.count && self.count != 0 {
            return format!("{} · RSS unavailable", self.count);
        }
        let prefix = if self.missing_rss == 0 { "" } else { "≥" };
        format!(
            "{} · {prefix}{} MiB RSS sum",
            self.count,
            self.rss_kib / 1024
        )
    }
}

#[cfg(target_os = "linux")]
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
struct ProcessSample {
    agenturm: ProcessTotals,
    harnesses: ProcessTotals,
    partial: bool,
}

#[cfg(target_os = "linux")]
fn current_process_sample() -> Option<ProcessSample> {
    // All cross-session pages can expose the Host pane. Share the bounded
    // sample across nearby page loads instead of doing a scan on each one.
    static CACHE: Mutex<Option<(Instant, Option<ProcessSample>)>> = Mutex::new(None);
    let mut cache = CACHE.lock().ok()?;
    if let Some((at, sample)) = *cache
        && at.elapsed() < PROCESS_SAMPLE_AGE
    {
        return sample;
    }
    let sample = read_proc("/proc/self/status")
        .and_then(|status| proc_uid(&status))
        .and_then(|uid| {
            scan_processes(
                Path::new("/proc"),
                uid,
                MAX_PROCESS_ENTRIES,
                PROCESS_SCAN_BUDGET,
            )
        });
    *cache = Some((Instant::now(), sample));
    sample
}

#[cfg(target_os = "linux")]
fn scan_processes(root: &Path, uid: u32, limit: usize, budget: Duration) -> Option<ProcessSample> {
    let start = Instant::now();
    let mut sample = ProcessSample::default();
    for (index, entry) in std::fs::read_dir(root).ok()?.enumerate() {
        if index == limit || start.elapsed() >= budget {
            sample.partial = true;
            break;
        }
        let Ok(entry) = entry else {
            sample.partial = true;
            continue;
        };
        if !entry.file_name().to_str().is_some_and(|name| {
            !name.is_empty() && name.len() <= 10 && name.bytes().all(|byte| byte.is_ascii_digit())
        }) || !entry.file_type().is_ok_and(|kind| kind.is_dir())
        {
            continue;
        }
        let process = entry.path();
        let Some(status) = read_proc(process.join("status")) else {
            sample.partial = true;
            continue;
        };
        if proc_uid(&status) != Some(uid)
            || !proc_field(&status, "State").is_some_and(|state| {
                !state.is_empty() && !matches!(state.as_bytes()[0], b'Z' | b'X' | b'x')
            })
        {
            continue;
        }
        let Some(name) = proc_field(&status, "Name") else {
            continue;
        };
        let class = runtime(name).or_else(|| {
            // Linux comm can truncate Rust adapter names. Read only the
            // executable link's basename, then immediately discard the path.
            name.starts_with("agenturm-")
                .then(|| std::fs::read_link(process.join("exe")).ok())
                .flatten()
                .and_then(|path| path.file_name()?.to_str().and_then(runtime))
        });
        let totals = match class {
            Some(Runtime::Agenturm) => &mut sample.agenturm,
            Some(Runtime::Harness) => &mut sample.harnesses,
            None => continue,
        };
        totals.record(read_kib(&status, "VmRSS"));
    }
    Some(sample)
}

#[cfg(target_os = "linux")]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Runtime {
    Agenturm,
    Harness,
}

#[cfg(target_os = "linux")]
fn runtime(name: &str) -> Option<Runtime> {
    use agenturm_kernel::HostRuntime::*;
    capture::recognized_runtime(name).map(|runtime| match runtime {
        Codex | Pi | Claude | Kiro | OpenCode => Runtime::Harness,
        _ => Runtime::Agenturm,
    })
}

#[cfg(target_os = "linux")]
fn proc_field<'a>(status: &'a str, key: &str) -> Option<&'a str> {
    status
        .lines()
        .filter_map(|line| line.split_once(':'))
        .find_map(|(name, value)| (name == key).then(|| value.trim()))
}

#[cfg(target_os = "linux")]
fn proc_uid(status: &str) -> Option<u32> {
    let mut fields = proc_field(status, "Uid")?.split_whitespace();
    let real: u32 = fields.next()?.parse().ok()?;
    let effective: u32 = fields.next()?.parse().ok()?;
    (real == effective).then_some(effective)
}

#[cfg(target_os = "linux")]
fn process_rows(sample: ProcessSample) -> Vec<MetricRow> {
    let mut rows = vec![
        MetricRow {
            label: "Same-user Agenturm processes".into(),
            value: sample.agenturm.display(),
        },
        MetricRow {
            label: "Same-user harness processes".into(),
            value: sample.harnesses.display(),
        },
    ];
    if sample.partial {
        rows.push(MetricRow {
            label: "Process sample".into(),
            value: "Partial · some entries were skipped".into(),
        });
    }
    rows
}

#[cfg(all(test, target_os = "linux"))]
mod tests {
    use super::*;
    use std::fs;
    use std::os::unix::fs::symlink;
    use std::path::PathBuf;
    use std::sync::atomic::{AtomicUsize, Ordering};

    #[test]
    fn samples_only_same_uid_known_live_names_and_uses_bounded_rss() {
        let procfs = Scratch::new();
        procfs.process("101", "agenturm", 10, "S", Some(1024));
        procfs.process("102", "agenturm-openco", 10, "R", Some(2048));
        symlink(
            "/private/install/agenturm-opencode-harness",
            procfs.0.join("102/exe"),
        )
        .unwrap();
        procfs.process("103", "codex", 10, "S", Some(8192));
        procfs.process("104", "claude", 10, "S", None);
        procfs.process("105", "pi", 11, "R", Some(99_000));
        procfs.process("106", "opencode", 10, "Z", Some(99_000));
        procfs.process("107", "node", 10, "R", Some(99_000));
        fs::write(procfs.0.join("107/cmdline"), b"codex\0private args").unwrap();
        fs::write(procfs.0.join("107/environ"), b"SECRET=private value").unwrap();
        procfs.process("108", "kiro-cli", 10, "R", Some(99_000));
        fs::write(procfs.0.join("108/status"), "x".repeat(8193)).unwrap();

        let sample = scan_processes(&procfs.0, 10, 20, Duration::from_secs(60)).unwrap();
        assert!(sample.partial);
        assert_eq!(
            sample.agenturm,
            ProcessTotals {
                count: 2,
                rss_kib: 3072,
                missing_rss: 0
            }
        );
        assert_eq!(
            sample.harnesses,
            ProcessTotals {
                count: 2,
                rss_kib: 8192,
                missing_rss: 1
            }
        );
        let rows = process_rows(sample);
        assert_eq!(rows[0].value, "2 · 3 MiB RSS sum");
        assert_eq!(rows[1].value, "2 · ≥8 MiB RSS sum");
        assert_eq!(rows[2].label, "Process sample");
    }

    #[test]
    fn entry_and_time_limits_mark_the_sample_as_partial() {
        let procfs = Scratch::new();
        for pid in ["201", "202", "203"] {
            procfs.process(pid, "codex", 10, "S", None);
        }
        let sample = scan_processes(&procfs.0, 10, 2, Duration::from_secs(60)).unwrap();
        assert_eq!(sample.harnesses.count, 2);
        assert_eq!(sample.harnesses.display(), "2 · RSS unavailable");
        assert!(sample.partial);
        assert!(process_rows(sample)[2].value.contains("Partial"));
        assert!(
            scan_processes(&procfs.0, 10, 20, Duration::ZERO)
                .unwrap()
                .partial
        );
    }

    #[test]
    fn rejects_mixed_real_and_effective_uids() {
        assert_eq!(proc_uid("Uid:\t1000\t0\t0\t0\n"), None);
        assert_eq!(proc_uid("Uid:\t1000\t1000\t1000\t1000\n"), Some(1000));
    }

    struct Scratch(PathBuf);

    impl Scratch {
        fn new() -> Self {
            static NEXT: AtomicUsize = AtomicUsize::new(0);
            let path = std::env::temp_dir().join(format!(
                "agenturm-proc-sample-{}-{}",
                std::process::id(),
                NEXT.fetch_add(1, Ordering::Relaxed)
            ));
            fs::create_dir(&path).unwrap();
            Self(path)
        }

        fn process(&self, pid: &str, name: &str, uid: u32, state: &str, rss_kib: Option<u64>) {
            let process = self.0.join(pid);
            fs::create_dir(&process).unwrap();
            let mut status =
                format!("Name:\t{name}\nState:\t{state}\nUid:\t{uid}\t{uid}\t{uid}\t{uid}\n");
            if let Some(rss) = rss_kib {
                status.push_str(&format!("VmRSS:\t{rss} kB\n"));
            }
            fs::write(process.join("status"), status).unwrap();
        }
    }

    impl Drop for Scratch {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }
}

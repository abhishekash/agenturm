use agenturm_kernel::{ExtensionId, MetricRow, Snapshot};
#[cfg(target_os = "linux")]
use std::io::Read;

pub(super) fn add_host_pane(snapshot: &mut Snapshot) -> ExtensionId {
    let mut rows = current_host_rows();
    if rows.is_empty() {
        rows.push(MetricRow {
            label: "Host metrics".into(),
            value: "Current host metrics are unavailable".into(),
        });
    }
    super::session_evidence::insert_pane(snapshot, "app.cockpit-host", "Host now", rows)
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
    rows
}

#[cfg(not(target_os = "linux"))]
fn current_host_rows() -> Vec<MetricRow> {
    Vec::new()
}

#[cfg(target_os = "linux")]
fn read_proc(path: &str) -> Option<String> {
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
    let value = contents
        .lines()
        .filter_map(|line| line.split_once(':'))
        .find_map(|(name, value)| (name == key).then_some(value))?;
    let mut words = value.split_whitespace();
    let size = words.next()?.parse().ok()?;
    (words.next() == Some("kB")).then_some(size)
}

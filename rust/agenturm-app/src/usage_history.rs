//! Snapshot-pinned usage chart loading, independent of the current feed page.

use crate::HistoryReader;
use agenturm_kernel::retained_history::UsageHistoryQuery;
use agenturm_kernel::{ExtensionId, ReadSnapshot};
use agenturm_ui::usage_chart::UsageChartData;
use std::time::{SystemTime, UNIX_EPOCH};

pub(crate) fn load(
    store: &impl HistoryReader,
    source_id: Option<ExtensionId>,
    session_id: Option<String>,
    snapshot_id: ReadSnapshot,
) -> Result<UsageChartData, String> {
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|_| "UTC clock is unavailable")?;
    let end_unix_ns = u64::try_from(now.as_nanos()).map_err(|_| "UTC clock is out of range")?;
    let scope = match (&source_id, &session_id) {
        (Some(source), Some(session)) => format!("{}  ·  {session}", source.as_str()),
        (Some(source), None) => format!("Source {}", source.as_str()),
        _ => "All authorized sources".into(),
    };
    let page = store.query_usage_history(&UsageHistoryQuery {
        source_id,
        session_id,
        snapshot_id: Some(snapshot_id),
        end_unix_ns,
    })?;
    let dates = std::array::from_fn(|index| utc_date(page.days[index].start_unix_ns));
    Ok(UsageChartData { page, dates, scope })
}

fn utc_date(nanos: u64) -> String {
    #[cfg(target_os = "linux")]
    if let Ok(value) = time::OffsetDateTime::from_unix_timestamp_nanos(i128::from(nanos)) {
        return value.date().to_string();
    }
    format!("Day {}", nanos / 86_400_000_000_000)
}

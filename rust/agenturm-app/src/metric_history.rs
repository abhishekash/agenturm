//! Initial metric catalog source for the terminal browser.

use crate::HistoryReader;
use agenturm_kernel::ReadSnapshot;
use agenturm_kernel::retained_history::{MetricHistoryPage, MetricHistoryQuery};

pub(crate) fn initial(
    store: &impl HistoryReader,
    snapshot_id: ReadSnapshot,
    source_id: Option<agenturm_kernel::ExtensionId>,
) -> Result<MetricHistoryPage, String> {
    store.query_metric_history(&MetricHistoryQuery::Series {
        source_id,
        snapshot_id: Some(snapshot_id),
        after: None,
        search: None,
        limit: agenturm_ui::metric_history::METRIC_PAGE_SIZE,
    })
}

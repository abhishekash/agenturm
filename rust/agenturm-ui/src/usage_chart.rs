//! Seven UTC calendar-day totals for one authorized retained history window.

use super::{
    Frame, KeyCode, LoadedView, MetricRow, Rect, label_style, panel, theme, visible_text,
    wrap_display_width,
};
use agenturm_kernel::retained_history::{
    USAGE_DAYS, UsageCurrencyTotal, UsageDay, UsageHistoryPage, UsageTotals,
};
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;
use std::collections::BTreeSet;

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct UsageChartData {
    pub page: UsageHistoryPage,
    pub dates: [String; USAGE_DAYS],
    pub scope: String,
}

pub struct UsageChart {
    data: UsageChartData,
    mode: Mode,
    counter: Counter,
    measure: Measure,
    currency: usize,
}

#[derive(Clone, Copy, Default)]
enum Mode {
    /// Nonduplicated evidence: overlapping producer reports at the same exact
    /// source/session identity are excluded and counted, never summed.
    #[default]
    Combined,
    Native,
    RawSpan,
    Traces,
    ActivityLogs,
}

impl Mode {
    fn next(self) -> Self {
        match self {
            Self::Combined => Self::Native,
            Self::Native => Self::RawSpan,
            Self::RawSpan => Self::Traces,
            Self::Traces => Self::ActivityLogs,
            Self::ActivityLogs => Self::Combined,
        }
    }

    fn label(self) -> &'static str {
        match self {
            Self::Combined => "Combined · nonduplicated evidence",
            Self::Native => "Native usage reports",
            Self::RawSpan => "Span reports may overlap",
            Self::Traces => "Retained trace summaries",
            Self::ActivityLogs => "Recorded log-only activity",
        }
    }

    fn tokens(self, day: &UsageDay) -> Option<&UsageTotals> {
        match self {
            Self::Combined => Some(&day.combined.tokens),
            Self::Native => Some(&day.native),
            Self::RawSpan => Some(&day.span),
            Self::Traces => Some(&day.traces.tokens),
            Self::ActivityLogs => day.logs.as_ref().map(|logs| &logs.tokens),
        }
    }

    fn costs(self, day: &UsageDay) -> &[UsageCurrencyTotal] {
        match self {
            Self::Combined => &day.combined.costs,
            Self::Traces => &day.traces.costs,
            Self::ActivityLogs => day.logs.as_ref().map_or(&[], |logs| &logs.costs),
            _ => &[],
        }
    }
}

#[derive(Clone, Copy, Default)]
enum Measure {
    #[default]
    Tokens,
    Reports,
    Errors,
    MissingCost,
    Cost,
}

impl Measure {
    fn next(self, mode: Mode) -> Self {
        match (self, mode) {
            (Self::Tokens, _) => Self::Reports,
            (Self::Reports, Mode::Traces | Mode::ActivityLogs | Mode::Combined) => Self::Errors,
            (Self::Errors, Mode::Traces) => Self::MissingCost,
            (Self::Errors, Mode::ActivityLogs | Mode::Combined) => Self::Cost,
            (Self::MissingCost, Mode::Traces) => Self::Cost,
            _ => Self::Tokens,
        }
    }
}

#[derive(Clone, Copy, Default)]
enum Counter {
    #[default]
    Input,
    Output,
    Total,
    CacheRead,
    CacheWrite,
    Reasoning,
}

impl Counter {
    fn next(self) -> Self {
        match self {
            Self::Input => Self::Output,
            Self::Output => Self::Total,
            Self::Total => Self::CacheRead,
            Self::CacheRead => Self::CacheWrite,
            Self::CacheWrite => Self::Reasoning,
            Self::Reasoning => Self::Input,
        }
    }

    fn label(self) -> &'static str {
        match self {
            Self::Input => "Input tokens",
            Self::Output => "Output tokens",
            Self::Total => "Reported total tokens",
            Self::CacheRead => "Cache read tokens",
            Self::CacheWrite => "Cache write tokens",
            Self::Reasoning => "Reasoning output tokens",
        }
    }

    fn value(self, totals: &UsageTotals) -> Option<u128> {
        match self {
            Self::Input => &totals.input_tokens,
            Self::Output => &totals.output_tokens,
            Self::Total => &totals.total_tokens,
            Self::CacheRead => &totals.cache_read_tokens,
            Self::CacheWrite => &totals.cache_write_tokens,
            Self::Reasoning => &totals.reasoning_output_tokens,
        }
        .as_deref()?
        .parse()
        .ok()
    }
}

impl UsageChart {
    pub fn new(data: UsageChartData) -> Self {
        Self {
            data,
            mode: Mode::default(),
            counter: Counter::default(),
            measure: Measure::default(),
            currency: 0,
        }
    }

    pub fn key(&mut self, key: KeyCode) -> bool {
        match key {
            KeyCode::Char('s') => {
                self.mode = self.mode.next();
                self.measure = Measure::Tokens;
                self.currency = 0;
            }
            KeyCode::Char('t') => {
                if matches!(self.measure, Measure::Tokens) {
                    self.counter = self.counter.next();
                }
                self.measure = Measure::Tokens;
            }
            KeyCode::Char('n') => self.measure = self.measure.next(self.mode),
            KeyCode::Char('C') if matches!(self.measure, Measure::Cost) => {
                self.currency = (self.currency + 1) % self.currencies().len().max(1);
            }
            _ => return false,
        }
        true
    }

    fn currencies(&self) -> BTreeSet<&str> {
        self.data
            .page
            .days
            .iter()
            .flat_map(|day| {
                self.mode
                    .costs(day)
                    .iter()
                    .map(|total| total.currency.as_str())
            })
            .collect()
    }

    fn currency(&self) -> Option<&str> {
        if !matches!(self.measure, Measure::Cost) {
            return None;
        }
        self.currencies().into_iter().nth(self.currency)
    }

    fn label(&self, currency: Option<&str>) -> String {
        match self.measure {
            Measure::Tokens => self.counter.label().into(),
            Measure::Reports => match self.mode {
                Mode::Traces => "Recorded traces",
                Mode::ActivityLogs => "Activity logs",
                Mode::Combined => "Attributed usage reports",
                _ => "Recorded usage reports",
            }
            .into(),
            Measure::Errors => match self.mode {
                Mode::Traces => "Reported error traces",
                Mode::Combined => "Excluded overlapping reports",
                _ => "Reported error logs",
            }
            .into(),
            Measure::MissingCost => "Traces with cost gaps".into(),
            Measure::Cost => currency.map_or_else(
                || "Reported cost · currency unreported".into(),
                |unit| format!("{unit} reported cost"),
            ),
        }
    }

    fn value(&self, day: &UsageDay, currency: Option<&str>) -> Option<u128> {
        match self.measure {
            Measure::Tokens => self.counter.value(self.mode.tokens(day)?),
            Measure::Reports => {
                let reports = match self.mode {
                    Mode::Traces => day.traces.traces,
                    Mode::ActivityLogs => day.logs.as_ref()?.logs,
                    _ => self.mode.tokens(day)?.reports,
                };
                Some(reports as u128)
            }
            Measure::Errors => match self.mode {
                Mode::Traces => (day.traces.reported_error_traces != 0
                    || day.traces.partial_traces == 0)
                    .then_some(day.traces.reported_error_traces as u128),
                Mode::ActivityLogs => Some(day.logs.as_ref()?.reported_errors as u128),
                Mode::Combined => Some(
                    (day.combined.excluded_native_reports + day.combined.excluded_telemetry_reports)
                        as u128,
                ),
                _ => None,
            },
            Measure::MissingCost => match self.mode {
                Mode::Traces => Some(day.traces.traces_missing_exact_cost as u128),
                _ => None,
            },
            Measure::Cost => {
                let unit = currency?;
                self.mode
                    .costs(day)
                    .iter()
                    .find(|entry| entry.currency == unit)?
                    .nanounits
                    .parse()
                    .ok()
            }
        }
    }

    fn display(&self, value: Option<u128>) -> String {
        let Some(value) = value else {
            return "Unreported".into();
        };
        if matches!(self.measure, Measure::Cost) {
            let whole = value / 1_000_000_000;
            let remainder = value % 1_000_000_000;
            if remainder != 0 {
                let fraction = format!("{remainder:09}");
                return format!("{whole}.{}", fraction.trim_end_matches('0'));
            }
            return whole.to_string();
        }
        value.to_string()
    }

    fn incomplete(&self, day: &UsageDay) -> bool {
        matches!(self.mode, Mode::Traces) && day.traces.partial_traces != 0
            || matches!(self.measure, Measure::Cost)
                && match self.mode {
                    Mode::Traces => day.traces.cost_currencies_truncated,
                    Mode::ActivityLogs => day
                        .logs
                        .as_ref()
                        .is_some_and(|logs| logs.cost_currencies_truncated),
                    Mode::Combined => day.combined.cost_currencies_truncated,
                    _ => false,
                }
    }

    pub(super) fn loaded_view(&self) -> LoadedView {
        let mode = self.mode.label();
        let currency = self.currency();
        let mut truncated = false;
        let rows = self
            .data
            .page
            .days
            .iter()
            .zip(&self.data.dates)
            .enumerate()
            .map(|(index, (day, date))| {
                let mut value = self.display(self.value(day, currency));
                if index + 1 == USAGE_DAYS {
                    value.push_str(" · Today to capture time");
                }
                if day.retention_partial {
                    value.push_str(" · Retention cutoff");
                }
                if day.scan_partial {
                    value.push_str(" · Scan limit");
                }
                if self.incomplete(day) {
                    value.push_str(" · Incomplete retained telemetry");
                }
                truncated |= day.retention_partial || day.scan_partial || self.incomplete(day);
                MetricRow {
                    label: date.clone(),
                    value,
                }
            })
            .collect();
        LoadedView {
            title: format!("Usage · 7 UTC days · {}", self.label(currency)),
            scope: format!(
                "{} · {mode} · Snapshot {}",
                self.data.scope, self.data.page.snapshot_id
            ),
            rows,
            truncated,
        }
    }

    pub fn draw(&self, frame: &mut Frame, area: Rect) {
        let block = panel(" Usage  ·  7 UTC days ");
        let width = usize::from(block.inner(area).width.max(1));
        let currency = self.currency();
        let values: [_; USAGE_DAYS] =
            std::array::from_fn(|index| self.value(&self.data.page.days[index], currency));
        let largest = values.iter().copied().flatten().max().unwrap_or(0);
        let digits = values
            .iter()
            .copied()
            .flatten()
            .map(|count| self.display(Some(count)).len())
            .max()
            .unwrap_or(1)
            .max("Unreported".len());
        let max_bar = width.saturating_sub(digits + 18).min(56);
        let mode = self.mode.label();
        let mut rows =
            vec![Line::from(format!("{}  ·  {mode}", self.label(currency))).style(label_style())];
        rows.extend(
            wrap_display_width(&visible_text(&self.data.scope), width)
                .into_iter()
                .map(Line::from),
        );
        if area.height >= 12 {
            rows.push(Line::from(""));
        }
        for (index, value) in values.into_iter().enumerate() {
            let day = &self.data.page.days[index];
            let partial = day.retention_partial || day.scan_partial;
            let amount = self.display(value);
            let bar = match value {
                Some(amount) if amount > 0 && max_bar > 0 => {
                    let cells = amount.saturating_mul(max_bar as u128).div_ceil(largest) as usize;
                    "█".repeat(cells)
                }
                _ => String::new(),
            };
            let today = if index + 1 == USAGE_DAYS { "†" } else { " " };
            let marker = if partial {
                "*"
            } else if self.incomplete(day) {
                "‡"
            } else {
                " "
            };
            rows.push(
                Line::from(vec![
                    Span::raw(format!(
                        "{}{}{} {:>digits$}  ",
                        self.data.dates[index], today, marker, amount
                    )),
                    Span::styled(bar, label_style()),
                ])
                .style(theme::row(index)),
            );
        }
        rows.push(Line::from(""));
        let coverage = match (
            self.data.page.days.iter().any(|day| day.retention_partial),
            self.data.page.scan_limited,
        ) {
            (true, true) => "† Today to capture time   * Retention or scan limit",
            (true, false) => "† Today to capture time   * Retention cutoff",
            (false, true) => "† Today to capture time   * Scan limit",
            (false, false) => "† Today to capture time",
        };
        rows.extend(
            wrap_display_width(coverage, width)
                .into_iter()
                .map(Line::from),
        );
        if self.data.page.days.iter().any(|day| self.incomplete(day)) {
            rows.push(Line::from("‡ Incomplete retained telemetry"));
        }
        let controls = if matches!(self.measure, Measure::Cost) {
            "s source   t tokens   n totals   C currency"
        } else {
            "s combined / native / spans / traces / logs   t tokens   n totals"
        };
        rows.push(Line::from(controls).style(label_style()));
        frame.render_widget(Paragraph::new(rows).block(block), area);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::{Terminal, backend::TestBackend};

    fn render(terminal: &mut Terminal<TestBackend>, chart: &UsageChart) -> String {
        terminal
            .draw(|frame| chart.draw(frame, frame.area()))
            .unwrap();
        terminal
            .backend()
            .buffer()
            .content()
            .iter()
            .map(|cell| cell.symbol())
            .collect()
    }

    #[test]
    fn seven_day_chart_preserves_missing_zero_source_mode_and_partial_window() {
        for (width, height) in [(120, 32), (80, 24), (60, 18)] {
            let mut terminal = Terminal::new(TestBackend::new(width, height)).unwrap();
            let mut chart = UsageChart::new(UsageChartData {
                page: UsageHistoryPage {
                    snapshot_id: 3.into(),
                    end_unix_ns: 7,
                    days: std::array::from_fn(|index| UsageDay {
                        start_unix_ns: index as u64,
                        combined: agenturm_kernel::retained_history::CombinedUsageTotals {
                            tokens: UsageTotals {
                                reports: usize::from(index == 6) * 2,
                                input_tokens: match index {
                                    3 => Some("40".into()),
                                    6 => Some("12".into()),
                                    _ => None,
                                },
                                ..UsageTotals::default()
                            },
                            costs: if index == 6 {
                                vec![UsageCurrencyTotal {
                                    currency: "USD".into(),
                                    nanounits: "15".into(),
                                    reports: 2,
                                }]
                            } else {
                                Vec::new()
                            },
                            excluded_native_reports: usize::from(index == 6),
                            excluded_telemetry_reports: 0,
                            cost_currencies_truncated: false,
                        },
                        native: UsageTotals {
                            input_tokens: match index {
                                3 => Some("32".into()),
                                5 => Some("0".into()),
                                _ => None,
                            },
                            ..UsageTotals::default()
                        },
                        span: UsageTotals {
                            output_tokens: (index == 6).then(|| "97".into()),
                            ..UsageTotals::default()
                        },
                        traces: if index == 6 {
                            agenturm_kernel::retained_history::TraceUsageTotals {
                                traces: 2,
                                reported_error_traces: 0,
                                partial_traces: 1,
                                traces_missing_exact_cost: 1,
                                costs: vec![
                                    UsageCurrencyTotal {
                                        currency: "EUR".into(),
                                        nanounits: "15".into(),
                                        reports: 1,
                                    },
                                    UsageCurrencyTotal {
                                        currency: "USD".into(),
                                        nanounits: "0".into(),
                                        reports: 1,
                                    },
                                ],
                                ..Default::default()
                            }
                        } else {
                            Default::default()
                        },
                        logs: (index == 6).then(|| {
                            agenturm_kernel::retained_history::ActivityLogTotals {
                                logs: 3,
                                reported_errors: 1,
                                ..Default::default()
                            }
                        }),
                        retention_partial: index == 0,
                        scan_partial: false,
                    }),
                    retention_cutoff_unix_ns: 1,
                    scan_limited: false,
                },
                dates: std::array::from_fn(|index| format!("2026-11-{:02}", index + 1)),
                scope: "All authorized sources".into(),
            });
            // The combined view is the default: nonduplicated evidence with
            // exclusions disclosed instead of summed.
            let screen = render(&mut terminal, &chart);
            assert!(screen.contains("7 UTC days"));
            assert!(screen.contains("Combined · nonduplicated evidence"));
            assert!(screen.contains("2026-11-01 * Unreported"));
            assert!(screen.contains("* Retention cutoff"));
            assert!(screen.contains("█"));
            assert_eq!(chart.loaded_view().rows[3].value, "40");
            assert!(chart.key(KeyCode::Char('n')));
            assert!(
                chart
                    .loaded_view()
                    .title
                    .contains("Attributed usage reports")
            );
            assert!(
                chart.loaded_view().rows[6]
                    .value
                    .starts_with("2 · Today to capture time")
            );
            assert!(chart.key(KeyCode::Char('n')));
            assert!(
                chart
                    .loaded_view()
                    .title
                    .contains("Excluded overlapping reports")
            );
            assert!(
                chart.loaded_view().rows[6]
                    .value
                    .starts_with("1 · Today to capture time")
            );
            assert!(chart.key(KeyCode::Char('n')));
            let cost = chart.loaded_view();
            assert!(cost.title.contains("USD reported cost"));
            assert!(cost.rows[6].value.starts_with("0.000000015 ·"));
            assert!(chart.key(KeyCode::Char('C')));
            assert!(chart.loaded_view().title.contains("USD reported cost"));
            // Native cohort remains one source step away.
            assert!(chart.key(KeyCode::Char('s')));
            let screen = render(&mut terminal, &chart);
            assert!(screen.contains("Native usage reports"));
            assert!(screen.contains("2026-11-06            0"));
            assert!(chart.key(KeyCode::Char('t')));
            let screen = render(&mut terminal, &chart);
            assert!(screen.contains("Output tokens"));
            assert!(chart.key(KeyCode::Char('s')));
            let screen = render(&mut terminal, &chart);
            assert!(screen.contains("Span reports may overlap"));
            assert!(screen.contains("97"));
            assert!(screen.contains("s combined / native / spans"));
            assert!(chart.key(KeyCode::Char('s')));
            assert!(chart.key(KeyCode::Char('n')));
            let screen = render(&mut terminal, &chart);
            assert!(screen.contains("Recorded traces"));
            assert!(screen.contains("‡ Incomplete retained telemetry"));
            assert_eq!(
                chart.loaded_view().rows[6].value,
                "2 · Today to capture time · Incomplete retained telemetry"
            );
            assert!(chart.key(KeyCode::Char('n')));
            assert!(chart.loaded_view().rows[6].value.starts_with("Unreported"));
            assert!(chart.key(KeyCode::Char('n')));
            assert!(chart.key(KeyCode::Char('n')));
            let cost = chart.loaded_view();
            assert!(cost.title.contains("EUR reported cost"));
            assert!(cost.rows[6].value.starts_with("0.000000015"));
            assert!(chart.key(KeyCode::Char('C')));
            let cost = chart.loaded_view();
            assert!(cost.title.contains("USD reported cost"));
            assert!(cost.rows[6].value.starts_with("0 ·"));
            assert!(chart.key(KeyCode::Char('s')));
            assert!(chart.key(KeyCode::Char('n')));
            let logs = chart.loaded_view();
            assert!(logs.title.contains("Activity logs"));
            assert!(logs.rows[6].value.starts_with("3 ·"));
            assert!(logs.rows[5].value.starts_with("Unreported"));
        }
    }
}

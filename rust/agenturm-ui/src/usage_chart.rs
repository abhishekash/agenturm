//! Seven UTC calendar-day totals for one authorized retained history window.

use super::{
    Frame, KeyCode, LoadedView, MetricRow, Rect, label_style, panel, theme, visible_text,
    wrap_display_width,
};
use agenturm_kernel::retained_history::{USAGE_DAYS, UsageHistoryPage, UsageTotals};
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct UsageChartData {
    pub page: UsageHistoryPage,
    pub dates: [String; USAGE_DAYS],
    pub scope: String,
}

pub struct UsageChart {
    data: UsageChartData,
    span: bool,
    counter: Counter,
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
            span: false,
            counter: Counter::default(),
        }
    }

    pub fn key(&mut self, key: KeyCode) -> bool {
        match key {
            KeyCode::Char('s') => self.span = !self.span,
            KeyCode::Char('t') => self.counter = self.counter.next(),
            _ => return false,
        }
        true
    }

    pub(super) fn loaded_view(&self) -> LoadedView {
        let mode = if self.span {
            "Span reports may overlap"
        } else {
            "Native usage reports"
        };
        let mut truncated = false;
        let rows = self
            .data
            .page
            .days
            .iter()
            .zip(&self.data.dates)
            .enumerate()
            .map(|(index, (day, date))| {
                let totals = if self.span { &day.span } else { &day.native };
                let mut value = self
                    .counter
                    .value(totals)
                    .map_or_else(|| "Unreported".into(), |value| value.to_string());
                if index + 1 == USAGE_DAYS {
                    value.push_str(" · Today to capture time");
                }
                if day.retention_partial {
                    value.push_str(" · Retention cutoff");
                }
                if day.scan_partial {
                    value.push_str(" · Scan limit");
                }
                truncated |= day.retention_partial || day.scan_partial;
                MetricRow {
                    label: date.clone(),
                    value,
                }
            })
            .collect();
        LoadedView {
            title: format!("Usage · 7 UTC days · {}", self.counter.label()),
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
        let values: [_; USAGE_DAYS] = std::array::from_fn(|index| {
            let day = &self.data.page.days[index];
            self.counter
                .value(if self.span { &day.span } else { &day.native })
        });
        let largest = values.iter().copied().flatten().max().unwrap_or(0);
        let digits = values
            .iter()
            .copied()
            .flatten()
            .map(|count| count.to_string().len())
            .max()
            .unwrap_or(1)
            .max("Unreported".len());
        let max_bar = width.saturating_sub(digits + 18).min(56);
        let mode = if self.span {
            "Span reports may overlap"
        } else {
            "Native usage reports"
        };
        let mut rows =
            vec![Line::from(format!("{}  ·  {mode}", self.counter.label())).style(label_style())];
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
            let amount = value.map_or_else(|| "Unreported".into(), |amount| amount.to_string());
            let bar = match value {
                Some(amount) if amount > 0 && max_bar > 0 => {
                    let cells = amount.saturating_mul(max_bar as u128).div_ceil(largest) as usize;
                    "█".repeat(cells)
                }
                _ => String::new(),
            };
            let today = if index + 1 == USAGE_DAYS { "†" } else { " " };
            let marker = if partial { "*" } else { " " };
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
        rows.push(Line::from("s native / spans   t token counter").style(label_style()));
        frame.render_widget(Paragraph::new(rows).block(block), area);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use agenturm_kernel::retained_history::UsageDay;
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
                        retention_partial: index == 0,
                        scan_partial: false,
                    }),
                    retention_cutoff_unix_ns: 1,
                    scan_limited: false,
                },
                dates: std::array::from_fn(|index| format!("2026-11-{:02}", index + 1)),
                scope: "All authorized sources".into(),
            });
            let screen = render(&mut terminal, &chart);
            assert!(screen.contains("7 UTC days"));
            assert!(screen.contains("2026-11-01 * Unreported"));
            assert!(screen.contains("2026-11-06            0"));
            assert!(screen.contains("* Retention cutoff"));
            assert!(screen.contains("█"));
            assert!(chart.key(KeyCode::Char('s')));
            assert!(chart.key(KeyCode::Char('t')));
            let screen = render(&mut terminal, &chart);
            assert!(screen.contains("Output tokens"));
            assert!(screen.contains("Span reports may overlap"));
            assert!(screen.contains("97"));
            assert!(screen.contains("s native / spans"));
        }
    }
}

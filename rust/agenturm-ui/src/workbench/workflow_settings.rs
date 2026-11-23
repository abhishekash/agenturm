//! Operator-owned Workflow settings with exact displayed policy revisions.

use super::*;
use agenturm_kernel::workflow_notifications::{
    WorkflowNotificationAuthority, WorkflowNotificationPolicy, WorkflowPolicyChange,
    WorkflowThresholds,
};
use std::time::{SystemTime, UNIX_EPOCH};

const USD_NANOS: u64 = 1_000_000_000;
const MINUTE_NS: u64 = 60_000_000_000;
const INPUT_LIMIT: usize = 32;
const FIELDS: usize = 5;

pub(super) struct Settings {
    authority: WorkflowNotificationAuthority,
    policy: Box<WorkflowNotificationPolicy>,
    cursor: usize,
    thresholds: [ThresholdDraft; 3],
    saved_thresholds_pending: bool,
    notice: Option<&'static str>,
}

struct ThresholdDraft {
    text: String,
    edited: bool,
}

impl Settings {
    pub(super) fn new(
        policy: Box<WorkflowNotificationPolicy>,
        authority: WorkflowNotificationAuthority,
    ) -> Self {
        Self {
            thresholds: drafts(&policy.thresholds),
            policy,
            authority,
            cursor: 0,
            saved_thresholds_pending: false,
            notice: None,
        }
    }

    pub(super) fn apply(
        &mut self,
        policy: Box<WorkflowNotificationPolicy>,
        authority: WorkflowNotificationAuthority,
    ) {
        let same_history = authority.includes(&self.authority);
        if !same_history
            || self.saved_thresholds_pending
            || self.policy.thresholds != policy.thresholds
        {
            self.thresholds = drafts(&policy.thresholds);
        }
        self.notice = (same_history && self.saved_thresholds_pending).then_some("Thresholds saved");
        self.saved_thresholds_pending = false;
        self.policy = policy;
        self.authority = authority;
    }

    fn save(&self, change: WorkflowPolicyChange) -> WorkInput {
        WorkInput::Command(WorkCommand::ChangeWorkflowSettings {
            authority: self.authority.clone(),
            expected_revision: self.policy.revision,
            change,
        })
    }

    pub(super) fn key(&mut self, key: event::KeyEvent) -> WorkInput {
        match key.code {
            KeyCode::Esc => return WorkInput::Back,
            KeyCode::Down | KeyCode::Tab => self.cursor = (self.cursor + 1) % FIELDS,
            KeyCode::Up | KeyCode::BackTab => self.cursor = (self.cursor + FIELDS - 1) % FIELDS,
            KeyCode::Enter | KeyCode::Char(' ') if self.cursor == 0 => {
                self.saved_thresholds_pending = false;
                self.notice = None;
                return self.save(WorkflowPolicyChange::SetDesktop(
                    !self.policy.desktop_enabled.unwrap_or(false),
                ));
            }
            KeyCode::Char('z') | KeyCode::Enter
                if self.cursor == 4 || key.code == KeyCode::Char('z') =>
            {
                self.saved_thresholds_pending = false;
                self.notice = None;
                return self.save(WorkflowPolicyChange::ToggleSnooze);
            }
            KeyCode::Char('s') if key.modifiers == KeyModifiers::CONTROL => {
                if let Some(thresholds) = self.parsed_thresholds() {
                    self.saved_thresholds_pending = true;
                    self.notice = None;
                    return self.save(WorkflowPolicyChange::SetThresholds(thresholds));
                }
                self.notice = Some("Enter nonnegative thresholds with supported precision");
                return WorkInput::None;
            }
            _ if (1..=3).contains(&self.cursor) => {
                let draft = &mut self.thresholds[self.cursor - 1];
                match key.code {
                    KeyCode::Backspace => {
                        draft.text.pop();
                        draft.edited = true;
                    }
                    KeyCode::Delete => {
                        draft.text.clear();
                        draft.edited = true;
                    }
                    KeyCode::Char(ch)
                        if (ch.is_ascii_digit() || ch == '.') && draft.text.len() < INPUT_LIMIT =>
                    {
                        draft.text.push(ch);
                        draft.edited = true;
                    }
                    _ => return WorkInput::None,
                }
            }
            _ => return WorkInput::None,
        }
        self.notice = None;
        WorkInput::None
    }

    fn parsed_thresholds(&self) -> Option<WorkflowThresholds> {
        let parse = |index: usize, scale, stored| {
            let draft = &self.thresholds[index];
            if draft.edited {
                scaled_decimal(&draft.text, scale)
            } else {
                Some(stored)
            }
        };
        let current = &self.policy.thresholds;
        let thresholds = WorkflowThresholds {
            tokens: parse(0, 1, current.tokens)?,
            usd_nanos: parse(1, USD_NANOS, current.usd_nanos)?,
            duration_ns: parse(2, MINUTE_NS, current.duration_ns)?,
        };
        thresholds.is_valid().then_some(thresholds)
    }

    pub(super) fn draw(&self, frame: &mut Frame, pending: bool, status: Option<&str>) {
        let [header, body, footer] = Layout::vertical([
            Constraint::Length(1),
            Constraint::Min(1),
            Constraint::Length(3),
        ])
        .areas(frame.area());
        frame.render_widget(
            Paragraph::new(" Agenturm / Workflow settings").style(heading_style()),
            header,
        );
        let desktop = match self.policy.desktop_enabled {
            Some(false) => "Off",
            Some(true) => "On",
            None => "Use watch configuration",
        };
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .ok()
            .and_then(|elapsed| u64::try_from(elapsed.as_nanos()).ok());
        let snooze = self
            .policy
            .snoozed_until_unix_ns
            .zip(now)
            .filter(|(until, now)| until > now)
            .map_or_else(
                || "Snooze 10 minutes".into(),
                |(until, now)| format!("End snooze · {}m left", (until - now).div_ceil(MINUTE_NS)),
            );
        let duration = &self.thresholds[2];
        let elapsed = if !duration.edited
            && scaled_decimal(&duration.text, MINUTE_NS) != Some(self.policy.thresholds.duration_ns)
        {
            format!("≈{}", duration.text)
        } else {
            duration.text.clone()
        };
        let fields = [
            ("Desktop notifications", desktop),
            ("Task tokens", self.thresholds[0].text.as_str()),
            ("Task USD", self.thresholds[1].text.as_str()),
            ("Task minutes", elapsed.as_str()),
            ("Snooze notifications", snooze.as_str()),
        ];
        let lines = fields
            .into_iter()
            .enumerate()
            .map(|(index, (label, value))| {
                let selected = index == self.cursor;
                let caret = if selected && (1..=3).contains(&index) {
                    "▏"
                } else {
                    ""
                };
                Line::styled(
                    format!(
                        "{} {label:<22} {value}{caret}",
                        if selected { "›" } else { " " }
                    ),
                    if selected {
                        selection_style()
                    } else {
                        Style::default()
                    },
                )
            })
            .collect::<Vec<_>>();
        frame.render_widget(
            Paragraph::new(lines)
                .wrap(Wrap { trim: false })
                .block(panel(" 0 disables a threshold ")),
            body,
        );
        frame.render_widget(
            Paragraph::new(vec![
                Line::from("Tab fields · Enter change · Ctrl+S save thresholds"),
                Line::from("Del clear field · z snooze/end · Esc inbox"),
                Line::from(self.notice.or(status).map(visible_text).unwrap_or_else(|| {
                    if pending {
                        "Reading Workflow policy…".into()
                    } else {
                        String::new()
                    }
                })),
            ])
            .style(theme::footer()),
            footer,
        );
    }
}

fn drafts(values: &WorkflowThresholds) -> [ThresholdDraft; 3] {
    [
        values.tokens.to_string(),
        format_decimal(values.usd_nanos, USD_NANOS),
        format_decimal(values.duration_ns, MINUTE_NS),
    ]
    .map(|text| ThresholdDraft {
        text,
        edited: false,
    })
}

fn scaled_decimal(input: &str, scale: u64) -> Option<u64> {
    let input = input.trim();
    let (whole, fraction) = input.split_once('.').unwrap_or((input, ""));
    if input.is_empty()
        || (whole.is_empty() && fraction.is_empty())
        || !whole.bytes().all(|byte| byte.is_ascii_digit())
        || !fraction.bytes().all(|byte| byte.is_ascii_digit())
    {
        return None;
    }
    let scale = u128::from(scale);
    let whole = if whole.is_empty() {
        0
    } else {
        whole.parse::<u128>().ok()?
    };
    let fraction = fraction.trim_end_matches('0');
    let offset = if fraction.is_empty() {
        0
    } else {
        let divisor = 10_u128.checked_pow(u32::try_from(fraction.len()).ok()?)?;
        let scaled = fraction.parse::<u128>().ok()?.checked_mul(scale)?;
        if scaled % divisor != 0 {
            return None;
        }
        scaled / divisor
    };
    u64::try_from(whole.checked_mul(scale)?.checked_add(offset)?).ok()
}

pub(super) fn format_decimal(value: u64, scale: u64) -> String {
    let mut remainder = value % scale;
    let mut text = (value / scale).to_string();
    if remainder != 0 {
        text.push('.');
        // This is exact for USD and for any decimal minutes entered here.
        // Externally supplied values with repeating decimal minutes remain
        // exact unless the operator explicitly edits this field.
        for _ in 0..11 {
            remainder *= 10;
            let digit = u8::try_from(remainder / scale).expect("decimal digit");
            text.push(char::from(b'0' + digit));
            remainder %= scale;
            if remainder == 0 {
                break;
            }
        }
    }
    text
}

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::{Terminal, backend::TestBackend};

    #[test]
    fn threshold_input_preserves_exact_units_and_rejects_invalid_or_overflow() {
        assert_eq!(scaled_decimal("0", USD_NANOS), Some(0));
        assert_eq!(scaled_decimal("12.1200", USD_NANOS), Some(12_120_000_000));
        assert_eq!(scaled_decimal(".000000001", USD_NANOS), Some(1));
        assert_eq!(scaled_decimal("0.00000000005", MINUTE_NS), Some(3));
        assert_eq!(scaled_decimal("2.5", MINUTE_NS), Some(150_000_000_000));
        assert_eq!(scaled_decimal("18446744073709551615", 1), Some(u64::MAX));
        for input in ["", ".", "-1", "NaN", "inf", "1.2.3"] {
            assert_eq!(scaled_decimal(input, USD_NANOS), None);
        }
        assert_eq!(scaled_decimal("18446744073709551616", 1), None);
        assert_eq!(scaled_decimal("1.5", 1), None);
        assert_eq!(scaled_decimal("0.00000000001", MINUTE_NS), None);
        assert_eq!(format_decimal(12_120_000_000, USD_NANOS), "12.12");
        assert_eq!(format_decimal(3, MINUTE_NS), "0.00000000005");
    }

    #[test]
    fn settings_keep_exact_authority_and_preserve_untouched_precision_at_each_width() {
        let authority = WorkflowNotificationAuthority {
            history_identity: "visible-history".into(),
            snapshot_id: 30.into(),
        };
        let base = WorkflowNotificationPolicy {
            revision: 4,
            thresholds: WorkflowThresholds {
                duration_ns: 1,
                ..Default::default()
            },
            ..Default::default()
        };
        for (width, height) in [(120, 32), (80, 24), (60, 18)] {
            let mut settings = Settings::new(Box::new(base), authority.clone());
            let mut terminal = Terminal::new(TestBackend::new(width, height)).unwrap();
            terminal
                .draw(|frame| settings.draw(frame, false, None))
                .unwrap();
            let screen = terminal.backend().to_string();
            for marker in [
                "Workflow settings",
                "Desktop notifications",
                "Task tokens",
                "Task USD",
                "Task minutes",
                "Snooze 10 minutes",
                "Ctrl+S save thresholds",
                "0 disables a threshold",
            ] {
                assert!(screen.contains(marker), "{width}×{height} missing {marker}");
            }
            let key = |code| event::KeyEvent::new(code, KeyModifiers::NONE);
            assert!(matches!(settings.key(key(KeyCode::Enter)),
                WorkInput::Command(WorkCommand::ChangeWorkflowSettings {
                    authority: held, expected_revision: 4, change: WorkflowPolicyChange::SetDesktop(true),
                }) if held == authority));
            settings.apply(
                Box::new(WorkflowNotificationPolicy {
                    desktop_enabled: Some(true),
                    revision: 5,
                    ..base
                }),
                authority.clone(),
            );
            settings.key(key(KeyCode::Tab));
            settings.key(key(KeyCode::Delete));
            let save = event::KeyEvent::new(KeyCode::Char('s'), KeyModifiers::CONTROL);
            assert!(matches!(settings.key(save), WorkInput::None));
            settings.key(key(KeyCode::Char('4')));
            settings.key(key(KeyCode::Char('2')));
            assert!(matches!(settings.key(save),
                WorkInput::Command(WorkCommand::ChangeWorkflowSettings {
                    authority: held, expected_revision: 5,
                    change: WorkflowPolicyChange::SetThresholds(WorkflowThresholds {
                        tokens: 42, usd_nanos: 0, duration_ns: 1,
                    }),
                }) if held == authority));
            assert!(matches!(settings.key(key(KeyCode::Char('z'))),
                WorkInput::Command(WorkCommand::ChangeWorkflowSettings {
                    authority: held, expected_revision: 5, change: WorkflowPolicyChange::ToggleSnooze,
                }) if held == authority));
            assert!(matches!(settings.key(key(KeyCode::Esc)), WorkInput::Back));
        }
    }
}

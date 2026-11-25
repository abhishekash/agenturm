//! Explicit exports of already loaded, public view metadata.
use super::{MetricRow, visible_text};

pub const MAX_VIEW_EXPORT_BYTES: usize = 2 * 1024 * 1024;
pub(crate) const MAX_CLIPBOARD_BYTES: usize = 16 * 1024;

#[derive(Clone, Debug)]
pub struct LoadedView {
    pub title: String,
    pub scope: String,
    pub rows: Vec<MetricRow>,
    pub truncated: bool,
}

impl LoadedView {
    /// A single selected evidence row, explicitly separate from its loaded
    /// page. The UI shell decides which of these two documents a copy/export
    /// action targets.
    pub fn selected_evidence(
        title: impl Into<String>,
        scope: impl Into<String>,
        row: &crate::EvidenceRow,
        position: usize,
        total: usize,
    ) -> Self {
        Self {
            title: title.into(),
            scope: format!(
                "{} · selected row {} of {} · loaded page only",
                scope.into(),
                position.saturating_add(1),
                total
            ),
            rows: row.details.clone(),
            truncated: false,
        }
    }

    /// This describes the displayed metadata, never an admission receipt.
    pub fn plain_text(&self) -> Result<String, String> {
        let mut text = format!(
            "{}\n{}\nLoaded metadata view; not an evidence receipt.\nCoverage: {} · rows: {}\n\n",
            visible_text(&self.title),
            visible_text(&self.scope),
            if self.truncated {
                "the loaded page is partial"
            } else {
                "the loaded page only"
            },
            self.rows.len()
        );
        if text.len() > MAX_VIEW_EXPORT_BYTES {
            return Err("Loaded view exceeds the export limit".into());
        }
        for row in &self.rows {
            // Bound before allocating sanitized copies as control escapes can grow.
            if row.label.len().saturating_add(row.value.len()) > MAX_VIEW_EXPORT_BYTES {
                return Err("Loaded view exceeds the export limit".into());
            }
            text.push_str(&visible_text(&row.label));
            text.push_str(": ");
            text.push_str(&visible_text(&row.value));
            text.push('\n');
            if text.len() > MAX_VIEW_EXPORT_BYTES {
                return Err("Loaded view exceeds the export limit".into());
            }
        }
        Ok(text)
    }
}

/// A bounded OSC 52 write request. Terminal support/permission is not assumed.
pub(crate) fn clipboard_request(text: &str) -> Result<Vec<u8>, String> {
    if text.len() > MAX_CLIPBOARD_BYTES {
        return Err("View is too large for clipboard; use x to export it".into());
    }
    const ALPHABET: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut output = b"\x1b]52;c;".to_vec();
    for bytes in text.as_bytes().chunks(3) {
        let a = bytes[0];
        let b = bytes.get(1).copied().unwrap_or(0);
        let c = bytes.get(2).copied().unwrap_or(0);
        output.push(ALPHABET[usize::from(a >> 2)]);
        output.push(ALPHABET[usize::from(((a & 3) << 4) | (b >> 4))]);
        output.push(if bytes.len() > 1 {
            ALPHABET[usize::from(((b & 15) << 2) | (c >> 6))]
        } else {
            b'='
        });
        output.push(if bytes.len() > 2 {
            ALPHABET[usize::from(c & 63)]
        } else {
            b'='
        });
    }
    output.push(7);
    Ok(output)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn clipboard_encodes_bytes_and_refuses_truncation() {
        assert_eq!(clipboard_request("f").unwrap(), b"\x1b]52;c;Zg==\x07");
        assert_eq!(clipboard_request("fo").unwrap(), b"\x1b]52;c;Zm8=\x07");
        assert_eq!(clipboard_request("foo").unwrap(), b"\x1b]52;c;Zm9v\x07");
        assert_eq!(clipboard_request("猫").unwrap(), b"\x1b]52;c;54yr\x07");
        assert!(clipboard_request(&"a".repeat(MAX_CLIPBOARD_BYTES + 1)).is_err());
    }

    #[test]
    fn selected_evidence_export_discloses_its_row_and_page_scope() {
        let row = crate::EvidenceRow {
            cells: vec!["tool".into()],
            details: vec![
                MetricRow {
                    label: "Source".into(),
                    value: "harness.fixture".into(),
                },
                MetricRow {
                    label: "Result".into(),
                    value: "failed\x1b[2J".into(),
                },
            ],
        };
        let view = LoadedView::selected_evidence(
            "Activity · selected evidence",
            "harness.fixture / session",
            &row,
            1,
            4,
        );
        assert_eq!(view.rows.len(), 2);
        assert!(view.scope.contains("selected row 2 of 4"));
        assert!(view.scope.contains("loaded page only"));
        let text = view.plain_text().unwrap();
        assert!(text.contains("rows: 2"));
        assert!(text.contains("Source: harness.fixture"));
        assert!(text.contains("Result: failed"));
        assert!(!text.contains('\x1b'));
        assert!(text.contains("not an evidence receipt"));
    }

    #[test]
    fn display_exports_sanitize_controls_and_identify_their_coverage() {
        let mut view = LoadedView {
            title: "Activity".into(),
            scope: "source/session".into(),
            rows: vec![MetricRow {
                label: "Tool".into(),
                value: "safe\x1b[2J\nnext".into(),
            }],
            truncated: true,
        };
        let text = view.plain_text().unwrap();
        assert!(!text.contains('\x1b'));
        assert!(text.contains("partial"));
        assert!(text.contains("not an evidence receipt"));
        view.rows[0].value = "x".repeat(MAX_VIEW_EXPORT_BYTES);
        assert!(view.plain_text().is_err());
    }
}

//! Bounded metadata search for a loaded evidence page.

use crossterm::event::KeyCode;

const MAX_QUERY_BYTES: usize = 128;

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub(super) struct Filter {
    query: String,
    draft: Option<String>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum FilterKey {
    Ignored,
    Handled,
    Changed,
}

impl Filter {
    pub(super) fn editing(&self) -> bool {
        self.draft.is_some()
    }

    pub(super) fn active(&self) -> bool {
        !self.query.is_empty()
    }

    pub(super) fn key(&mut self, key: KeyCode) -> FilterKey {
        self.key_with_limit(key, MAX_QUERY_BYTES)
    }

    pub(super) fn key_with_limit(&mut self, key: KeyCode, max_query_bytes: usize) -> FilterKey {
        if let Some(draft) = self.draft.as_mut() {
            match key {
                KeyCode::Esc => self.draft = None,
                KeyCode::Enter => {
                    let draft = self.draft.take().expect("metadata filter draft");
                    let next = draft.trim();
                    if next != self.query {
                        self.query = next.to_owned();
                        return FilterKey::Changed;
                    }
                }
                KeyCode::Backspace => {
                    draft.pop();
                }
                KeyCode::Char(ch)
                    if !ch.is_control() && draft.len() + ch.len_utf8() <= max_query_bytes =>
                {
                    draft.push(ch);
                }
                _ => {}
            }
            FilterKey::Handled
        } else {
            match key {
                KeyCode::Char('/') => {
                    self.draft = Some(self.query.clone());
                    FilterKey::Handled
                }
                KeyCode::Esc if self.active() => {
                    self.query.clear();
                    FilterKey::Changed
                }
                _ => FilterKey::Ignored,
            }
        }
    }

    pub(super) fn committed(&self) -> Self {
        Self {
            query: self.query.clone(),
            draft: None,
        }
    }

    pub(super) fn with_query(query: String) -> Self {
        Self { query, draft: None }
    }

    pub(super) fn query(&self) -> &str {
        &self.query
    }

    pub(super) fn label(&self) -> Option<String> {
        self.label_with("Filter page")
    }

    pub(super) fn label_with(&self, title: &str) -> Option<String> {
        if let Some(draft) = &self.draft {
            Some(format!("{title}: {}▏", super::visible_text(draft)))
        } else if self.active() {
            Some(format!("{title}: {}", super::visible_text(&self.query)))
        } else {
            None
        }
    }

    pub(super) fn keys(&self) -> Option<&'static str> {
        self.editing().then_some("Enter apply · Esc cancel")
    }

    pub(super) fn matches<'a>(&self, fields: impl IntoIterator<Item = &'a str>) -> bool {
        if !self.active() {
            return true;
        }
        let fields: Vec<_> = fields.into_iter().map(str::to_lowercase).collect();
        self.query
            .to_lowercase()
            .split_whitespace()
            .all(|word| fields.iter().any(|field| field.contains(word)))
    }
}

/// Retain the underlying row when visible. Prefer the next row on a tie.
pub(super) fn selection(visible: &[usize], previous: Option<usize>) -> Option<usize> {
    let previous = previous.unwrap_or(0);
    visible
        .iter()
        .enumerate()
        .min_by_key(|(_, index)| (index.abs_diff(previous), **index < previous))
        .map(|(position, _)| position)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn edit_commit_cancel_and_clear_are_distinct() {
        let mut filter = Filter::default();
        assert_eq!(filter.key(KeyCode::Char('/')), FilterKey::Handled);
        for ch in "MiXeD Q".chars() {
            assert_eq!(filter.key(KeyCode::Char(ch)), FilterKey::Handled);
        }
        assert_eq!(filter.key(KeyCode::Enter), FilterKey::Changed);
        assert!(filter.matches(["mixed case", "query"]));
        assert!(!filter.matches(["mixed case", "other"]));
        assert_eq!(filter.key(KeyCode::Char('/')), FilterKey::Handled);
        assert_eq!(filter.key(KeyCode::Backspace), FilterKey::Handled);
        assert_eq!(filter.key(KeyCode::Esc), FilterKey::Handled);
        assert!(filter.matches(["mixed case", "query"]));
        assert_eq!(filter.key(KeyCode::Esc), FilterKey::Changed);
        assert!(filter.matches([]));
    }

    #[test]
    fn mapped_selection_survives_filtering_and_uses_nearest_visible_neighbor() {
        assert_eq!(selection(&[0, 2, 4], Some(2)), Some(1));
        assert_eq!(selection(&[0, 2, 4], Some(1)), Some(1));
        assert_eq!(selection(&[0, 2, 4], Some(3)), Some(2));
        assert_eq!(selection(&[], Some(8)), None);
    }
}

//! Python 0.2.8 palettes and application-wide appearance selection.
use super::*;
use ratatui::widgets::Clear;
use std::cell::Cell;
use std::path::{Path, PathBuf};
use unicode_width::UnicodeWidthStr;

const NAMES: [&str; 5] = ["Observation", "Current", "Bright", "Dark", "Solarized"];
thread_local! { static SELECTED: Cell<usize> = const { Cell::new(0) }; }

#[derive(Clone, Copy)]
pub(super) struct Palette {
    pub canvas: Color,
    pub surface: Color,
    pub panel: Color,
    pub text: Color,
    pub muted: Color,
    pub primary: Color,
    pub secondary: Color,
    pub success: Color,
    pub info: Color,
    pub error: Color,
    pub border_subtle: Color,
    pub focus: Color,
    pub cursor: Color,
    pub on_cursor: Color,
    pub row_alt: Color,
    pub footer_key: Color,
}

const PALETTES: [Palette; 5] = [
    // Observation
    Palette {
        canvas: Color::Rgb(21, 26, 23),
        surface: Color::Rgb(28, 34, 30),
        panel: Color::Rgb(34, 42, 36),
        text: Color::Rgb(237, 241, 231),
        muted: Color::Rgb(171, 180, 165),
        primary: Color::Rgb(220, 235, 90),
        secondary: Color::Rgb(197, 173, 255),
        success: Color::Rgb(134, 227, 181),
        info: Color::Rgb(134, 217, 237),
        error: Color::Rgb(255, 135, 153),
        border_subtle: Color::Rgb(57, 66, 57),
        focus: Color::Rgb(48, 58, 39),
        cursor: Color::Rgb(43, 52, 41),
        on_cursor: Color::Rgb(237, 241, 231),
        row_alt: Color::Rgb(25, 31, 27),
        footer_key: Color::Rgb(41, 53, 42),
    },
    // Current
    Palette {
        canvas: Color::Rgb(14, 14, 20),
        surface: Color::Rgb(21, 21, 30),
        panel: Color::Rgb(30, 30, 42),
        text: Color::Rgb(243, 241, 237),
        muted: Color::Rgb(166, 164, 184),
        primary: Color::Rgb(255, 189, 105),
        secondary: Color::Rgb(197, 173, 255),
        success: Color::Rgb(134, 227, 181),
        info: Color::Rgb(134, 217, 237),
        error: Color::Rgb(255, 135, 153),
        border_subtle: Color::Rgb(57, 55, 70),
        focus: Color::Rgb(48, 41, 30),
        cursor: Color::Rgb(41, 39, 53),
        on_cursor: Color::Rgb(255, 247, 235),
        row_alt: Color::Rgb(21, 21, 30),
        footer_key: Color::Rgb(59, 45, 76),
    },
    // Bright
    Palette {
        canvas: Color::Rgb(255, 255, 255),
        surface: Color::Rgb(245, 246, 250),
        panel: Color::Rgb(232, 235, 242),
        text: Color::Rgb(24, 32, 47),
        muted: Color::Rgb(77, 88, 112),
        primary: Color::Rgb(128, 80, 0),
        secondary: Color::Rgb(89, 64, 149),
        success: Color::Rgb(32, 101, 64),
        info: Color::Rgb(7, 91, 121),
        error: Color::Rgb(172, 36, 68),
        border_subtle: Color::Rgb(169, 177, 193),
        focus: Color::Rgb(255, 240, 204),
        cursor: Color::Rgb(212, 221, 240),
        on_cursor: Color::Rgb(24, 32, 47),
        row_alt: Color::Rgb(236, 239, 246),
        footer_key: Color::Rgb(221, 226, 238),
    },
    // Dark
    Palette {
        canvas: Color::Rgb(9, 12, 16),
        surface: Color::Rgb(16, 21, 28),
        panel: Color::Rgb(28, 36, 48),
        text: Color::Rgb(237, 242, 247),
        muted: Color::Rgb(170, 184, 202),
        primary: Color::Rgb(142, 203, 255),
        secondary: Color::Rgb(197, 176, 255),
        success: Color::Rgb(125, 224, 166),
        info: Color::Rgb(129, 219, 232),
        error: Color::Rgb(255, 145, 163),
        border_subtle: Color::Rgb(53, 66, 86),
        focus: Color::Rgb(36, 60, 84),
        cursor: Color::Rgb(44, 70, 99),
        on_cursor: Color::Rgb(255, 255, 255),
        row_alt: Color::Rgb(21, 30, 42),
        footer_key: Color::Rgb(38, 53, 75),
    },
    // Solarized
    Palette {
        canvas: Color::Rgb(0, 43, 54),
        surface: Color::Rgb(7, 54, 66),
        panel: Color::Rgb(11, 64, 77),
        text: Color::Rgb(253, 246, 227),
        muted: Color::Rgb(147, 161, 161),
        primary: Color::Rgb(225, 185, 0),
        secondary: Color::Rgb(166, 164, 232),
        success: Color::Rgb(179, 195, 76),
        info: Color::Rgb(86, 195, 209),
        error: Color::Rgb(255, 128, 110),
        border_subtle: Color::Rgb(59, 98, 108),
        focus: Color::Rgb(64, 81, 59),
        cursor: Color::Rgb(40, 89, 103),
        on_cursor: Color::Rgb(253, 246, 227),
        row_alt: Color::Rgb(10, 60, 72),
        footer_key: Color::Rgb(27, 75, 88),
    },
];

pub(super) fn palette() -> Palette {
    SELECTED.with(|selected| PALETTES[selected.get()])
}

pub(super) fn foreground(color: Color) -> Style {
    if colors() {
        Style::default().fg(color)
    } else {
        Style::default()
    }
}

pub(super) fn muted() -> Style {
    foreground(palette().muted)
}

pub(super) fn status(value: &str) -> Style {
    let color = match value.to_ascii_lowercase().as_str() {
        "working" | "running" | "started" | "restored" => palette().success,
        "error" | "failed" | "failure" | "denied" => palette().error,
        "needs you" | "waiting" | "blocked" | "interrupted" => palette().primary,
        "ready" | "completed" | "ended" | "ok" | "succeeded" | "passed" => palette().info,
        _ => palette().muted,
    };
    foreground(color)
}

pub(super) fn row(index: usize) -> Style {
    surface(if index.is_multiple_of(2) {
        palette().surface
    } else {
        palette().row_alt
    })
}

pub(super) fn footer() -> Style {
    surface(palette().footer_key)
}

/// Single-line overlay text is escaped before measuring. Ratatui's grapheme
/// iterator keeps combining marks, emoji modifiers and ZWJ sequences together.
pub(super) fn wrap_cells(text: &str, width: usize) -> Vec<String> {
    if width == 0 {
        return vec![String::new()];
    }
    let text = visible_text(text);
    let span = Span::raw(text);
    let mut lines = Vec::new();
    let mut line = String::new();
    let mut used = 0;
    for grapheme in span.styled_graphemes(Style::default()) {
        let cells = UnicodeWidthStr::width(grapheme.symbol);
        if used + cells > width && !line.is_empty() {
            lines.push(std::mem::take(&mut line));
            used = 0;
        }
        // A glyph wider than the entire region cannot be displayed intact.
        if cells > width {
            line.push('…');
            used += 1;
        } else {
            line.push_str(grapheme.symbol);
            used += cells;
        }
    }
    if !line.is_empty() || lines.is_empty() {
        lines.push(line);
    }
    lines
}

pub(super) fn clip_cells(text: &str, width: usize) -> String {
    if width == 0 {
        return String::new();
    }
    let text = visible_text(text);
    if UnicodeWidthStr::width(text.as_str()) <= width {
        return text;
    }
    let span = Span::raw(text);
    let mut clipped = String::new();
    let mut used = 0;
    for grapheme in span.styled_graphemes(Style::default()) {
        let cells = UnicodeWidthStr::width(grapheme.symbol);
        if used + cells > width - 1 {
            break;
        }
        clipped.push_str(grapheme.symbol);
        used += cells;
    }
    clipped.push('…');
    clipped
}

/// Keep the editing end of a long search draft (and its cursor) on screen.
pub(super) fn tail_cells(text: &str, width: usize) -> String {
    if width == 0 {
        return String::new();
    }
    let text = visible_text(text);
    if UnicodeWidthStr::width(text.as_str()) <= width {
        return text;
    }
    let span = Span::raw(text);
    let graphemes: Vec<_> = span.styled_graphemes(Style::default()).collect();
    let mut used = 0;
    let mut start = graphemes.len();
    for (index, grapheme) in graphemes.iter().enumerate().rev() {
        let cells = UnicodeWidthStr::width(grapheme.symbol);
        if used + cells > width - 1 {
            break;
        }
        start = index;
        used += cells;
    }
    let mut clipped = String::from("…");
    for grapheme in &graphemes[start..] {
        clipped.push_str(grapheme.symbol);
    }
    clipped
}

pub(super) fn resize_notice(frame: &mut Frame, minimum: &str) {
    let area = frame.area();
    frame.render_widget(Clear, area);
    let lines = [
        "Esc back · resize".to_owned(),
        format!("Terminal too small · need {minimum}"),
    ]
    .iter()
    .flat_map(|line| wrap_cells(line, usize::from(area.width.max(1))))
    .map(Line::from)
    .collect::<Vec<_>>();
    frame.render_widget(
        Paragraph::new(lines).style(surface(palette().surface)),
        area,
    );
}

pub(super) fn preference_path() -> Option<PathBuf> {
    if let Some(config_home) = std::env::var_os("XDG_CONFIG_HOME") {
        return Some(
            PathBuf::from(config_home)
                .join("agenturm-rust")
                .join("ui.json"),
        );
    }
    let home = PathBuf::from(std::env::var_os("HOME")?);
    #[cfg(target_os = "macos")]
    {
        Some(home.join("Library/Preferences/Agenturm/ui.json"))
    }
    #[cfg(not(target_os = "macos"))]
    {
        Some(home.join(".config/agenturm-rust/ui.json"))
    }
}

pub(super) fn preferences_enabled() -> bool {
    !matches!(
        std::env::var("AGENTURM_UI_PREFERENCES")
            .unwrap_or_default()
            .as_str(),
        "0" | "false" | "off"
    )
}

fn read(path: &Path) -> Option<usize> {
    use std::io::Read;
    let mut bytes = Vec::new();
    std::fs::File::open(path)
        .ok()?
        .take(16385)
        .read_to_end(&mut bytes)
        .ok()?;
    if bytes.len() > 16384 {
        return None;
    }
    let value: serde_json::Value = serde_json::from_slice(&bytes).ok()?;
    let name = value.get("theme")?.as_str()?;
    NAMES.iter().position(|known| *known == name)
}

fn save(path: &Path, selected: usize) -> io::Result<()> {
    use std::io::Write;
    let parent = path
        .parent()
        .ok_or_else(|| io::Error::other("Missing configuration directory"))?;
    std::fs::create_dir_all(parent)?;
    let temporary = parent.join(format!(".ui-{}.tmp", std::process::id()));
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let mut file = options.open(&temporary)?;
    let result = (|| {
        let json = serde_json::json!({"theme":NAMES[selected]});
        writeln!(file, "{json}")?;
        file.sync_all()?;
        std::fs::rename(&temporary, path)
    })();
    let _ = std::fs::remove_file(&temporary);
    result
}

/// Preview is reversible. Only Enter/Space writes the selected appearance.
pub(super) struct Picker {
    open: bool,
    original: usize,
    selected: usize,
    path: Option<PathBuf>,
    error: Option<String>,
}

impl Picker {
    pub(super) fn new() -> Self {
        let path = preferences_enabled().then(preference_path).flatten();
        Self::with_path(path)
    }

    fn with_path(path: Option<PathBuf>) -> Self {
        let selected = path.as_deref().and_then(read).unwrap_or(0);
        SELECTED.with(|value| value.set(selected));
        Self {
            open: false,
            original: selected,
            selected,
            path,
            error: None,
        }
    }

    /// Opens the preview from a palette action without changing the selection.
    pub(super) fn open(&mut self) {
        if self.open {
            return;
        }
        self.original = self.selected;
        self.open = true;
        self.error = None;
    }

    #[allow(dead_code)]
    pub(super) fn active(&self) -> bool {
        self.open
    }

    pub(super) fn key(&mut self, key: event::KeyEvent) -> bool {
        if keymap::interrupt(key) {
            return false;
        }
        // `Ctrl+P` belongs to the cockpit command palette. Appearance stays on
        // F2 and remains a palette entry.
        let toggle = key.code == KeyCode::F(2);
        if !self.open {
            if toggle {
                self.open();
            }
            return toggle;
        }
        match key.code {
            KeyCode::Esc | KeyCode::F(2) => {
                self.selected = self.original;
                SELECTED.with(|value| value.set(self.selected));
                self.open = false;
            }
            KeyCode::Down | KeyCode::Char('j') | KeyCode::Tab => {
                self.select((self.selected + 1) % NAMES.len())
            }
            KeyCode::Up | KeyCode::Char('k') | KeyCode::BackTab => {
                self.select((self.selected + NAMES.len() - 1) % NAMES.len())
            }
            key if crate::keymap::activate(key) => {
                if let Some(path) = &self.path
                    && let Err(error) = save(path, self.selected)
                {
                    self.error = Some(format!("Save failed ({})", error.kind()));
                    return true;
                }
                self.open = false;
            }
            _ => {}
        }
        true
    }

    fn select(&mut self, index: usize) {
        self.selected = index;
        SELECTED.with(|value| value.set(index));
        self.error = None;
    }

    pub(super) fn draw(&self, frame: &mut Frame) {
        if !self.open {
            return;
        }
        let area = frame.area();
        let width = area.width.saturating_sub(2).min(68);
        let height = area.height.saturating_sub(2).min(14);
        // Five choices, two hint/error rows and a border must all fit.
        if width < 20 || height < 9 {
            resize_notice(frame, "22×11 for appearance");
            return;
        }
        let popup = Rect::new(
            area.x + (area.width - width) / 2,
            area.y + (area.height - height) / 2,
            width,
            height,
        );
        frame.render_widget(Clear, popup);
        let block = panel(" Appearance ").border_style(foreground(palette().primary));
        let inner = block.inner(popup);
        frame.render_widget(block, popup);
        let [names, help] =
            Layout::vertical([Constraint::Min(1), Constraint::Length(2)]).areas(inner);
        let items = NAMES.iter().enumerate().map(|(i, name)| {
            let style = if i == self.selected {
                selection_style()
            } else {
                surface(palette().surface)
            };
            let marker = if i == self.selected { "›" } else { " " };
            ListItem::new(format!("{marker} {name}")).style(style)
        });
        frame.render_widget(List::new(items).style(surface(palette().focus)), names);
        let width = usize::from(help.width);
        let lines = if let Some(error) = &self.error {
            vec![
                clip_cells(
                    if width < 24 {
                        "Esc cancel · Enter"
                    } else {
                        "Esc cancel · Enter retry"
                    },
                    width,
                ),
                clip_cells(error, width),
            ]
        } else if width < 24 {
            vec!["Esc cancel".into(), "Enter apply · ↑↓".into()]
        } else {
            wrap_cells("Esc cancel · Enter/Space apply · ↑↓ preview", width)
        };
        frame.render_widget(
            Paragraph::new(lines.into_iter().map(Line::from).collect::<Vec<_>>()).style(muted()),
            help,
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::{Terminal, backend::TestBackend};

    fn text(terminal: &Terminal<TestBackend>) -> String {
        terminal
            .backend()
            .buffer()
            .content()
            .iter()
            .map(|cell| cell.symbol())
            .collect()
    }

    #[test]
    fn overlay_text_keeps_graphemes_and_cell_budgets() {
        for grapheme in ["界", "e\u{301}", "👩🏽‍💻", "🇯🇵", "✈️"] {
            let width = UnicodeWidthStr::width(grapheme);
            let original = format!("{grapheme}{grapheme}{grapheme}");
            assert_eq!(wrap_cells(&original, width), vec![grapheme; 3]);
            assert_eq!(clip_cells(&original, width + 1), format!("{grapheme}…"));
            assert_eq!(tail_cells(&original, width + 1), format!("…{grapheme}"));
            for budget in 0..8 {
                assert!(UnicodeWidthStr::width(clip_cells(&original, budget).as_str()) <= budget);
                assert!(UnicodeWidthStr::width(tail_cells(&original, budget).as_str()) <= budget);
            }
        }
        assert_eq!(clip_cells("a\x1b[31mb", 30), "a\\u{1b}[31mb");
        assert_eq!(wrap_cells("界", 1), vec!["…"]);
        assert_eq!(wrap_cells("", 0), vec![""]);
    }

    #[test]
    fn essential_overlay_text_and_selection_have_readable_rgb_contrast() {
        fn luminance(color: Color) -> f64 {
            let Color::Rgb(r, g, b) = color else {
                panic!("expected an RGB palette role")
            };
            let linear = |value: u8| {
                let value = f64::from(value) / 255.0;
                if value <= 0.04045 {
                    value / 12.92
                } else {
                    ((value + 0.055) / 1.055).powf(2.4)
                }
            };
            0.2126 * linear(r) + 0.7152 * linear(g) + 0.0722 * linear(b)
        }
        let contrast = |a, b| {
            let (a, b) = (luminance(a), luminance(b));
            (a.max(b) + 0.05) / (a.min(b) + 0.05)
        };
        for (name, palette) in NAMES.iter().zip(PALETTES) {
            for background in [palette.canvas, palette.surface, palette.panel] {
                assert!(
                    contrast(palette.text, background) >= 4.5,
                    "{name}: body/search text"
                );
            }
            assert!(
                contrast(palette.on_cursor, palette.cursor) >= 4.5,
                "{name}: selection"
            );
            assert!(
                contrast(palette.muted, palette.surface) >= 4.5,
                "{name}: overlay hints/counts"
            );
            for status in [
                palette.primary,
                palette.success,
                palette.info,
                palette.error,
            ] {
                assert!(
                    contrast(status, palette.surface) >= 4.5,
                    "{name}: status on the overlay surface"
                );
            }
        }
    }

    #[test]
    fn failed_save_keeps_preview_cancel_and_space_retry_reopen_work() {
        let directory = std::env::temp_dir().join(format!(
            "agenturm-theme-retry-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&directory).unwrap();
        let blocked = directory.join("blocked");
        std::fs::write(&blocked, "not a directory").unwrap();
        let path = blocked.join("ui.json");
        let key = |code| event::KeyEvent::new(code, KeyModifiers::NONE);
        let mut picker = Picker::with_path(Some(path.clone()));
        picker.open();
        picker.key(key(KeyCode::Down));
        picker.key(key(KeyCode::Enter));
        assert!(picker.active());
        assert!(picker.error.is_some());
        for (width, height) in [(120, 32), (80, 24), (60, 18), (22, 11)] {
            let mut terminal = Terminal::new(TestBackend::new(width, height)).unwrap();
            terminal.draw(|frame| picker.draw(frame)).unwrap();
            assert!(text(&terminal).contains("Esc cancel"));
            assert!(text(&terminal).contains("Enter"));
            assert!(text(&terminal).contains("Save failed"));
        }
        picker.key(key(KeyCode::Esc));
        assert_eq!(picker.selected, 0);
        assert_eq!(SELECTED.with(Cell::get), 0);
        picker.open();
        picker.key(key(KeyCode::Down));
        picker.key(key(KeyCode::Char(' ')));
        assert!(picker.active());
        std::fs::remove_file(&blocked).unwrap();
        picker.key(key(KeyCode::Char(' ')));
        assert!(!picker.active());
        assert_eq!(read(&path), Some(1));
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(
                std::fs::metadata(&path).unwrap().permissions().mode() & 0o777,
                0o600
            );
        }
        let mut reopened = Picker::with_path(Some(path));
        assert_eq!(reopened.selected, 1);
        reopened.open();
        reopened.key(key(KeyCode::Down));
        reopened.key(key(KeyCode::F(2)));
        assert_eq!(reopened.selected, 1);
        std::fs::remove_dir_all(directory).unwrap();
        SELECTED.with(|s| s.set(0));
    }

    #[test]
    fn appearance_selection_has_a_marker_and_semantic_style_in_every_theme() {
        let mut picker = Picker::with_path(None);
        picker.open();
        for (width, height) in [(120, 32), (80, 24), (60, 18), (22, 11)] {
            for (selected, name) in NAMES.iter().enumerate() {
                picker.select(selected);
                let mut terminal = Terminal::new(TestBackend::new(width, height)).unwrap();
                terminal.draw(|frame| picker.draw(frame)).unwrap();
                assert!(text(&terminal).contains(&format!("› {name}")));
                let marker = terminal
                    .backend()
                    .buffer()
                    .content()
                    .iter()
                    .find(|cell| cell.symbol() == "›")
                    .unwrap();
                assert!(marker.modifier.contains(Modifier::BOLD));
                if colors() {
                    assert_eq!(marker.bg, palette().cursor);
                    assert_eq!(marker.fg, palette().on_cursor);
                } else {
                    assert_eq!(marker.fg, Color::Reset);
                    assert_eq!(marker.bg, Color::Reset);
                    assert!(marker.modifier.contains(Modifier::REVERSED));
                    assert!(
                        terminal
                            .backend()
                            .buffer()
                            .content()
                            .iter()
                            .all(|cell| cell.fg == Color::Reset && cell.bg == Color::Reset)
                    );
                }
            }
        }
        SELECTED.with(|s| s.set(0));
    }

    #[test]
    fn undersized_appearance_replaces_the_frame_and_keeps_escape_working() {
        let mut picker = Picker::with_path(None);
        picker.open();
        picker.select(2);
        for (width, height) in [(21, 11), (22, 10), (19, 5), (1, 1)] {
            let mut terminal = Terminal::new(TestBackend::new(width, height)).unwrap();
            terminal
                .draw(|frame| {
                    frame.render_widget(Paragraph::new("OLD CONTENT"), frame.area());
                    picker.draw(frame);
                })
                .unwrap();
            assert!(!text(&terminal).contains("OLD CONTENT"));
            if width >= 16 {
                assert!(text(&terminal).contains("Esc back"));
            }
            assert_eq!(picker.selected, 2, "size notice must not cancel a preview");
        }
        picker.key(event::KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE));
        assert_eq!(picker.selected, 0);
        SELECTED.with(|s| s.set(0));
    }

    #[test]
    fn preview_cancel_and_saved_selection_are_distinct() {
        let directory = std::env::temp_dir().join(format!(
            "agenturm-appearance-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let path = directory.join("ui.json");
        let mut picker = Picker {
            open: false,
            original: 0,
            selected: 0,
            path: Some(path.clone()),
            error: None,
        };
        let key = |code| event::KeyEvent::new(code, KeyModifiers::NONE);
        assert!(picker.key(key(KeyCode::F(2))));
        picker.key(key(KeyCode::Down));
        assert_eq!(picker.selected, 1);
        picker.key(key(KeyCode::Esc));
        assert_eq!(picker.selected, 0);
        assert!(!path.exists());
        // Ctrl+P is the cockpit palette, not appearance.
        assert!(!picker.key(event::KeyEvent::new(
            KeyCode::Char('p'),
            KeyModifiers::CONTROL
        )));
        assert!(!picker.active());
        picker.open();
        assert!(picker.active());
        picker.key(key(KeyCode::Esc));
        assert!(!picker.active());
        picker.key(key(KeyCode::F(2)));
        picker.key(key(KeyCode::Down));
        picker.key(key(KeyCode::Down));
        picker.key(key(KeyCode::Enter));
        assert_eq!(read(&path), Some(2));
        std::fs::write(&path, br#"{"theme":"Unknown"}"#).unwrap();
        assert_eq!(read(&path), None);
        std::fs::write(&path, vec![b' '; 16385]).unwrap();
        assert_eq!(read(&path), None);
        std::fs::remove_dir_all(directory).unwrap();
        SELECTED.with(|s| s.set(0));
    }

    #[test]
    fn theme_picker_keeps_all_options_and_cancel_visible_at_the_floor() {
        let mut picker = Picker {
            open: true,
            original: 0,
            selected: 0,
            path: None,
            error: None,
        };
        for (width, height) in [(120, 32), (80, 24), (60, 18)] {
            for selected in 0..NAMES.len() {
                picker.select(selected);
                let mut terminal = Terminal::new(TestBackend::new(width, height)).unwrap();
                terminal.draw(|frame| picker.draw(frame)).unwrap();
                let text: String = terminal
                    .backend()
                    .buffer()
                    .content()
                    .iter()
                    .map(|cell| cell.symbol())
                    .collect();
                for name in NAMES {
                    assert!(text.contains(name), "{name} missing at {width}");
                }
                assert!(text.contains("Esc cancel"));
            }
        }
        SELECTED.with(|s| s.set(0));
    }
}

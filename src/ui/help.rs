//! The shortcuts tab of the `?` popup.

use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::Stylize;
use ratatui::text::{Line, Span};
use ratatui::widgets::{Paragraph, Scrollbar, ScrollbarOrientation, ScrollbarState};

use crate::theme::Colors;

/// Every keyboard shortcut, grouped by where it works. Keep this in step
/// with the key handling in `app.rs` (and the status bar hints).
const SHORTCUTS: &[(&str, &[(&str, &str)])] = &[
    (
        "Everywhere",
        &[
            ("?", "This help, and settings"),
            (":", "Type a command, like logout"),
            ("H", "Highlight or unhighlight the selected chat"),
            ("q", "Quit (press again to stop waiting)"),
            ("Ctrl-c", "Quit, except while writing"),
        ],
    ),
    (
        "Chat list",
        &[
            ("j / k", "Move down / up"),
            ("gg / G", "First / last chat"),
            ("Ctrl-d / Ctrl-u", "Half a page down / up"),
            ("Enter / l", "Open the chat"),
            ("i", "Open the chat and write"),
            ("/", "Filter chats by name"),
            ("Esc", "Clear the filter"),
        ],
    ),
    (
        "Messages",
        &[
            ("j / k", "Newer / older message"),
            ("gg / G", "Oldest / newest message"),
            ("Ctrl-d / Ctrl-u", "Half a page newer / older"),
            ("Enter", "Open the photo, file or link"),
            ("y", "Copy the text, a link, or the photo or file"),
            ("r", "Reply"),
            ("d", "Delete"),
            ("gd", "Go to the message a reply answers"),
            ("Ctrl-o", "Back to the reply"),
            ("i", "Write a message"),
            ("/", "Search the whole chat"),
            ("n / N", "Next older / newer match"),
            ("Esc", "End the search, cancel the reply, or go back"),
            ("h", "Back to the chat list"),
        ],
    ),
    (
        "Writing",
        &[
            ("Enter", "Send"),
            ("Alt-Enter / Ctrl-j", "New line"),
            ("Esc / Ctrl-c", "Back to Normal mode"),
        ],
    ),
    (
        "Search and command prompts",
        &[
            ("Enter", "Search, keep the chat filter, or run the command"),
            ("Esc / Ctrl-c", "Cancel"),
        ],
    ),
    (
        "Menus and popups",
        &[
            ("j / k", "Move"),
            ("Enter", "Choose"),
            ("1-9", "Choose by number"),
            ("Esc / q", "Close"),
            ("Tab / h / l", "Switch tabs in this popup"),
        ],
    ),
];

/// The shortcuts as lines: a heading per group, then each key and what it does.
fn lines(colors: &Colors) -> Vec<Line<'static>> {
    let key_width = SHORTCUTS
        .iter()
        .flat_map(|(_, keys)| keys.iter())
        .map(|(key, _)| key.len())
        .max()
        .unwrap_or(0);
    let mut lines = Vec::new();
    for (i, (group, keys)) in SHORTCUTS.iter().enumerate() {
        if i > 0 {
            lines.push(Line::default());
        }
        lines.push(Line::from(format!(" {group}")).fg(colors.accent).bold());
        for (key, what) in keys.iter() {
            lines.push(Line::from(vec![
                Span::from(format!("   {key:<key_width$}  ")).fg(colors.primary),
                Span::from(*what),
            ]));
        }
    }
    lines
}

/// Rows the list takes, for sizing the popup.
pub fn height() -> usize {
    SHORTCUTS
        .iter()
        .map(|(_, keys)| keys.len() + 2)
        .sum::<usize>()
        - 1
}

/// Draws the list from row `scroll`, first pulling `scroll` back if it's
/// past the end.
pub fn draw(frame: &mut Frame, area: Rect, scroll: &mut usize, colors: &Colors) {
    let lines = lines(colors);
    let max = lines.len().saturating_sub(usize::from(area.height));
    *scroll = (*scroll).min(max);
    frame.render_widget(Paragraph::new(lines).scroll((*scroll as u16, 0)), area);
    if max > 0 {
        let mut state = ScrollbarState::new(max).position(*scroll);
        frame.render_stateful_widget(
            Scrollbar::new(ScrollbarOrientation::VerticalRight)
                .begin_symbol(None)
                .end_symbol(None)
                .thumb_style(colors.accent)
                .track_style(colors.border),
            area,
            &mut state,
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::theme::Theme;

    #[test]
    fn height_counts_every_line() {
        assert_eq!(height(), lines(&Theme::Mocha.colors()).len());
    }
}

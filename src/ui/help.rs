//! The shortcuts tab of the `?` popup.

use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::Stylize;
use ratatui::text::{Line, Span};
use ratatui::widgets::{Paragraph, Scrollbar, ScrollbarOrientation, ScrollbarState};
use unicode_width::UnicodeWidthStr;

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
            ("e", "Edit your message"),
            (
                "R",
                "React, or take your reaction back (/ finds an emoji by name)",
            ),
            ("X", "Take back all your reactions"),
            ("d", "Delete"),
            ("gd", "Go to the message a reply answers"),
            ("Ctrl-o", "Back to the reply"),
            ("i", "Write a message"),
            ("a", "Attach a file by its path (Tab completes it)"),
            ("p", "Paste a photo, files or text from the clipboard"),
            ("/", "Search the whole chat"),
            ("n / N", "Next older / newer match"),
            (
                "Esc",
                "End the search, cancel the edit, remove the files, cancel the reply, or go back",
            ),
            ("h", "Back to the chat list"),
        ],
    ),
    (
        "Writing",
        &[
            ("Enter", "Send, or save the edit"),
            ("Alt-Enter / Ctrl-j", "New line"),
            ("Ctrl-v", "Paste a photo, files or text from the clipboard"),
            (
                "Drop a file",
                "Attach it; the text you write is its caption",
            ),
            (
                "Ctrl-z",
                "Turn files just pasted back into their path as text",
            ),
            (
                "Ctrl-t",
                "Send attached photos as files, uncompressed, or back",
            ),
            ("Esc / Ctrl-c", "Back to Normal mode"),
        ],
    ),
    (
        "Search and command prompts",
        &[
            ("Enter", "Search, keep the chat filter, or run the command"),
            ("Tab", "Complete the path of a file to attach"),
            ("Esc / Ctrl-c", "Cancel"),
        ],
    ),
    (
        "Menus and popups",
        &[
            ("j / k", "Move"),
            ("Enter", "Choose"),
            ("1-9", "Choose by number"),
            ("Space", "Turn a setting on or off"),
            ("Esc / q", "Close"),
            ("Tab / h / l", "Switch tabs in this popup"),
            ("y / n", "Open or not, when asked about a file or link"),
        ],
    ),
];

/// The longest key, which sets where the descriptions start.
fn key_width() -> usize {
    SHORTCUTS
        .iter()
        .flat_map(|(_, keys)| keys.iter())
        .map(|(key, _)| key.width())
        .max()
        .unwrap_or(0)
}

/// The shortcuts as lines: a heading per group, then each key and what it does.
fn lines(colors: &Colors) -> Vec<Line<'static>> {
    let key_width = key_width();
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

/// Columns the widest line takes, plus one for the scrollbar, for sizing the
/// popup.
pub fn width() -> usize {
    let what = SHORTCUTS
        .iter()
        .flat_map(|(_, keys)| keys.iter())
        .map(|(_, what)| what.width())
        .max()
        .unwrap_or(0);
    // Indent, the keys, a gap, then the descriptions.
    3 + key_width() + 2 + what + 1
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

    #[test]
    fn width_leaves_room_for_every_line_and_the_scrollbar() {
        let widest = lines(&Theme::Mocha.colors())
            .iter()
            .map(Line::width)
            .max()
            .unwrap();
        assert_eq!(width(), widest + 1);
    }
}

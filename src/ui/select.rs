//! The `v` popup: a message's text, to select part of it and copy that.

use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::Style;
use ratatui::widgets::Padding;

use super::{Cover, center, popup_block, text_rows};
use crate::select::TextSelect;
use crate::theme::Colors;

/// The popup's width at most, borders included: much longer lines are
/// hard to read.
const WIDTH: u16 = 76;

/// As tall as the text, up to the pane, over which it's centered.
pub(super) fn draw(frame: &mut Frame, area: Rect, select: &mut TextSelect, colors: &Colors) {
    let width = WIDTH.min(area.width);
    // Inside the borders, a column of margin each side.
    let rows = text_rows(&select.text, width.saturating_sub(4)).max(1);
    let height = u16::try_from(rows + 2).unwrap_or(u16::MAX).min(area.height);
    let popup = center(area, width, height);
    let keys = match select.selecting() {
        true => " `y` copy · `Esc` cancel ",
        false => " `v` select · `Esc` close ",
    };
    let block = popup_block(" Select text ", keys, colors).padding(Padding::horizontal(1));
    let inner = block.inner(popup);
    frame.render_widget(Cover, popup);
    frame.render_widget(block, popup);
    select
        .text
        .set_selection_style(Style::new().bg(colors.selection));
    select.text.set_cursor_style(Style::new().reversed());
    frame.render_widget(&select.text, inner);
}

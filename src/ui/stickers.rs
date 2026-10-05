//! The sticker panel, between the messages and the composer while it's open:
//! a row of tabs (or the search), and a grid of stickers under it.

use ratatui::Frame;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Style, Stylize};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Paragraph};
use ratatui_image::FontSize;
use ratatui_image::sliced::{SignedPosition, SlicedImage};
use unicode_width::UnicodeWidthStr;

use super::{border, hint_spans, truncate};
use crate::images::Images;
use crate::reactions;
use crate::stickers::StickerPanel;
use crate::theme::Colors;

/// Rows of one sticker's picture.
const IMAGE_ROWS: u16 = 4;
/// Rows of the grid per sticker: its picture, and a gap above it.
const SLOT_ROWS: u16 = IMAGE_ROWS + 1;
/// Rows of stickers shown at once, at most; more scroll.
const MAX_GRID_ROWS: u16 = 2;
/// Tab titles are cut to this many columns, so several fit.
const MAX_TAB_WIDTH: usize = 18;

/// Columns of a sticker's picture: about as wide as it's tall in pixels.
fn image_cols(font: FontSize) -> u16 {
    let (width, height) = (f32::from(font.width.max(1)), f32::from(font.height.max(1)));
    ((f32::from(IMAGE_ROWS) * height / width).round() as u16).clamp(4, 12)
}

/// Rows the panel takes, given the rows left above the composer: two rows
/// of stickers when they leave the messages at least as many, else one.
pub fn height(available: u16) -> u16 {
    // Borders and the tabs.
    let fixed = 3;
    let fits = (available / 2).saturating_sub(fixed) / SLOT_ROWS;
    let grid_rows = fits.clamp(1, MAX_GRID_ROWS);
    (fixed + grid_rows * SLOT_ROWS).min(available)
}

pub fn draw(
    frame: &mut Frame,
    area: Rect,
    panel: &mut StickerPanel,
    images: &mut Images,
    colors: &Colors,
) {
    let mut block = Block::bordered()
        .title(" Stickers ")
        .border_style(border(true, colors));
    if let Some(shown) = panel.shown().filter(|s| !s.is_empty())
        && let Some(current) = panel.current()
    {
        // Which sticker the cursor is on, by its emoji, and how far along.
        let place = format!(
            " {} {} of {} ",
            reactions::shown(&current.emoji),
            panel.selected + 1,
            shown.len()
        );
        block = block.title_bottom(Line::from(place).fg(colors.muted));
    }
    let inner = block.inner(area);
    frame.render_widget(block, area);
    let [top, grid] = Layout::vertical([Constraint::Length(1), Constraint::Fill(1)]).areas(inner);
    frame.render_widget(top_line(panel, top.width as usize, colors), top);

    let message = if panel.is_empty() && panel.query.is_none() {
        Some("No stickers yet · `/` finds some by emoji or word")
    } else {
        match panel.shown() {
            None if panel.search_query().is_some() => Some("Searching…"),
            None => Some("Loading…"),
            Some([]) if panel.search_query().is_some() => Some("No stickers found"),
            Some([]) => Some("No stickers in this set"),
            Some(_) => None,
        }
    };
    if let Some(message) = message {
        let row = Rect {
            y: grid.y + grid.height / 2,
            height: 1.min(grid.height),
            ..grid
        };
        let muted = Style::new().fg(colors.muted);
        let line = Line::from(hint_spans(message, muted, colors)).centered();
        frame.render_widget(line, row);
        return;
    }
    draw_grid(frame, grid, panel, images, colors);
}

/// The tabs, the one shown filled in, scrolled so it's in view; or the
/// search while there is one.
fn top_line(panel: &StickerPanel, width: usize, colors: &Colors) -> Line<'static> {
    if let Some(query) = &panel.query {
        return Line::from(vec![
            Span::from(" / ").fg(colors.search).bold(),
            Span::from(truncate(query, width.saturating_sub(4))),
            Span::from(" ").reversed(),
        ]);
    }
    let titles: Vec<String> = panel
        .sections
        .iter()
        .map(|s| format!(" {} ", truncate(&s.title, MAX_TAB_WIDTH)))
        .collect();
    // The first tab drawn: as early as still leaves the one shown in view.
    let mut first = panel.tab.min(titles.len().saturating_sub(1));
    let mut used = titles.get(first).map_or(0, |t| t.width());
    while first > 0 && used + titles[first - 1].width() <= width {
        first -= 1;
        used += titles[first].width();
    }
    let spans = titles
        .into_iter()
        .enumerate()
        .skip(first)
        .map(|(i, title)| {
            if i == panel.tab {
                Span::from(title).fg(colors.bg).bg(colors.accent).bold()
            } else {
                Span::from(title).fg(colors.muted)
            }
        })
        .collect::<Vec<_>>();
    Line::from(spans)
}

/// The stickers, scrolled to keep the cursor's row in view. The cursor's
/// sticker sits on the selection color, which shows through its clear parts.
fn draw_grid(
    frame: &mut Frame,
    area: Rect,
    panel: &mut StickerPanel,
    images: &mut Images,
    colors: &Colors,
) {
    let cols = image_cols(images.font_size());
    // A column either side of each picture, for the cursor's color.
    let slot_cols = cols + 2;
    let columns = usize::from((area.width / slot_cols).max(1));
    let visible = usize::from((area.height / SLOT_ROWS).max(1));
    panel.columns = columns;
    let Some(shown) = panel.shown() else {
        return;
    };
    let rows = shown.len().div_ceil(columns);
    let row = panel.selected / columns;
    panel.scroll = panel
        .scroll
        .clamp(row.saturating_sub(visible - 1), row)
        .min(rows.saturating_sub(visible));
    let Some(shown) = panel.shown() else {
        return;
    };
    let first = panel.scroll * columns;
    let pictures = images.draws_photos();
    for (i, sticker) in shown.iter().enumerate().skip(first).take(visible * columns) {
        let (r, c) = ((i - first) / columns, (i - first) % columns);
        let slot = Rect {
            x: area.x + c as u16 * slot_cols,
            y: area.y + r as u16 * SLOT_ROWS + 1,
            width: slot_cols,
            height: IMAGE_ROWS,
        }
        .intersection(area);
        if i == panel.selected {
            frame
                .buffer_mut()
                .set_style(slot, Style::new().bg(colors.selection));
        }
        let picture = Rect {
            x: slot.x + 1,
            width: cols.min(slot.width.saturating_sub(1)),
            ..slot
        };
        if pictures && let Some(preview) = &sticker.preview {
            images.want(preview, cols, IMAGE_ROWS);
            if let Some(image) = images.get(preview, cols, IMAGE_ROWS) {
                let at = SignedPosition::from((0, 0));
                frame.render_widget(SlicedImage::new(image, at), picture);
                continue;
            }
        }
        // No picture (yet, or in this terminal): the emoji it stands for.
        let middle = Rect {
            y: picture.y + picture.height / 2,
            height: 1.min(picture.height),
            ..picture
        };
        let emoji = reactions::shown(&sticker.emoji);
        frame.render_widget(Paragraph::new(emoji).centered(), middle);
    }
}

#[cfg(test)]
mod tests {
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;
    use ratatui_image::picker::Picker;
    use tokio::sync::mpsc::unbounded_channel;

    use super::*;
    use crate::stickers::{Source, Sticker};
    use crate::theme::Theme;

    fn stickers(emoji: &[&str]) -> Vec<Sticker> {
        (1..)
            .zip(emoji)
            .map(|(file_id, e)| Sticker {
                file_id,
                width: 512,
                height: 512,
                emoji: e.to_string(),
                preview: None,
            })
            .collect()
    }

    fn render(panel: &mut StickerPanel, width: u16, height: u16) -> ratatui::buffer::Buffer {
        let colors = Theme::Mocha.colors();
        let mut images = Images::new(Picker::halfblocks(), unbounded_channel().0);
        let mut terminal = Terminal::new(TestBackend::new(width, height)).unwrap();
        terminal
            .draw(|f| draw(f, f.area(), panel, &mut images, &colors))
            .unwrap();
        terminal.backend().buffer().clone()
    }

    fn rows(buf: &ratatui::buffer::Buffer) -> Vec<String> {
        (0..buf.area.height)
            .map(|y| (0..buf.area.width).map(|x| buf[(x, y)].symbol()).collect())
            .collect()
    }

    fn has(rows: &[String], text: &str) -> bool {
        rows.iter().any(|r| r.contains(text))
    }

    #[test]
    fn the_panel_takes_two_rows_of_stickers_only_when_the_messages_keep_as_many() {
        assert_eq!(height(40), 13, "borders, tabs and two rows of five");
        assert_eq!(height(20), 8, "one row");
        assert_eq!(height(5), 5, "never more than there is");
    }

    #[test]
    fn the_panel_says_what_its_waiting_for() {
        let mut panel = StickerPanel::new(1);
        let screen = rows(&render(&mut panel, 60, 13));
        assert!(has(&screen, " Stickers "));
        assert!(has(&screen, " Recent  Favorites "));
        assert!(has(&screen, "Loading…"));

        panel.set_stickers(Source::Recent, Vec::new());
        panel.set_stickers(Source::Favorites, Vec::new());
        panel.set_sets(Vec::new());
        let screen = rows(&render(&mut panel, 60, 13));
        assert!(has(
            &screen,
            "No stickers yet · / finds some by emoji or word"
        ));

        panel.edit_query(|q| q.push_str("cat"));
        let screen = rows(&render(&mut panel, 60, 13));
        assert!(has(&screen, " / cat"));
        assert!(has(&screen, "Searching…"));
        panel.set_found("cat", Vec::new());
        assert!(has(&rows(&render(&mut panel, 60, 13)), "No stickers found"));
    }

    #[test]
    fn stickers_without_a_picture_show_their_emoji_and_the_cursor_is_filled_in() {
        let colors = Theme::Mocha.colors();
        let mut panel = StickerPanel::new(1);
        panel.set_stickers(
            Source::Recent,
            stickers(&["😀", "😂", "😍", "🥺", "😎", "❤"]),
        );
        panel.move_by(1);
        // Halfblocks report a 10×20 font: pictures are 8 columns, slots 10.
        let buf = render(&mut panel, 32, 13);
        let screen = rows(&buf);
        assert_eq!(panel.columns, 3, "three 10-column slots in 30");
        let bottom = screen.last().unwrap();
        assert!(
            bottom.contains("😂") && bottom.contains(" 2 of 6 "),
            "{bottom}"
        );
        let find = |symbol: &str| {
            (0..buf.area.height).find_map(|y| {
                let x = (0..buf.area.width).find(|&x| buf[(x, y)].symbol() == symbol)?;
                Some((x, y))
            })
        };
        let (x, y) = find("😂").expect("drawn");
        assert_eq!(buf[(x, y)].bg, colors.selection);
        let (x, y) = find("😀").expect("drawn");
        assert_ne!(buf[(x, y)].bg, colors.selection);
        assert!(
            find("❤\u{FE0F}").is_some(),
            "the second row, two columns wide"
        );

        // A third row scrolls the first out of view.
        panel.move_rows(1);
        panel.set_stickers(
            Source::Recent,
            stickers(&["😀", "😂", "😍", "🥺", "😎", "❤", "🐱"]),
        );
        panel.move_rows(1);
        let screen = rows(&render(&mut panel, 32, 13));
        assert_eq!(panel.scroll, 1);
        assert!(has(&screen, "🐱") && !has(&screen, "😀"));
    }

    #[test]
    fn the_tab_shown_stays_in_view() {
        let mut panel = StickerPanel::new(1);
        let sets = (1..=8).map(|i| (i, format!("Set number {i}"))).collect();
        panel.set_sets(sets);
        panel.switch(7);
        let screen = rows(&render(&mut panel, 40, 8));
        assert!(has(&screen, " Set number 6 "), "{screen:#?}");
        assert!(!has(&screen, "Recent"));
        panel.switch(-7);
        assert!(has(
            &rows(&render(&mut panel, 40, 8)),
            " Recent  Favorites "
        ));
    }
}

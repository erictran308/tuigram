//! The chat list: two rows per chat, its title over its last message (or
//! "typing…" while someone is), with the chat's photo on the left (a square
//! of color if it has none), and a blank row between chats unless that's
//! turned off.

use std::collections::HashMap;

use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::{Style, Stylize};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, List, ListItem, ListState, Paragraph};
use ratatui_image::FontSize;
use ratatui_image::sliced::{SignedPosition, SlicedImage};
use unicode_width::UnicodeWidthStr;

use crate::chats::Chats;
use crate::images::Images;
use crate::theme::Colors;

use super::messages::Names;
use super::{activity, border, highlight, title_style, truncate};

/// Rows of text per chat, so also the height of its photo.
const PHOTO_ROWS: u16 = 2;
/// Narrower than this inside its border, the list leaves photos out to keep
/// room for titles.
const MIN_WIDTH_FOR_PHOTOS: u16 = 24;
/// After a muted chat's name.
const MUTED: &str = " 🔕";
/// On the right of a pinned chat with nothing unread.
const PINNED: &str = "📌";

/// What the list shows, from the app's state.
pub struct ChatList<'a> {
    pub chats: &'a Chats,
    /// Display names by user id, for who's typing in groups.
    pub users: &'a HashMap<i64, String>,
    pub selected: Option<i64>,
    pub loading: bool,
    pub focused: bool,
    /// A popup that may be drawn over the list is open.
    pub covered: bool,
    /// A blank row between chats.
    pub gaps: bool,
}

pub fn draw(frame: &mut Frame, area: Rect, list: &ChatList, images: &mut Images, colors: &Colors) {
    let chats = list.chats;
    let filter = chats.filter();
    let names = Names {
        users: list.users,
        chats,
    };
    let mut title = if filter.is_empty() {
        format!(" Chats ({}) ", chats.ids().len())
    } else {
        format!(
            " Chats ({} of {}) /{filter} ",
            chats.ids().len(),
            chats.total()
        )
    };
    if list.loading {
        title.push_str("loading… ");
    }
    let block = Block::bordered()
        .title(title)
        .border_style(border(list.focused, colors));
    let inner = block.inner(area);
    let photo_cols = if inner.width >= MIN_WIDTH_FOR_PHOTOS {
        photo_cols(images.font_size())
    } else {
        0
    };
    // The photo and a space after it.
    let indent = if photo_cols > 0 { photo_cols + 1 } else { 0 };
    // What's left for text after the 1-column cursor bar and the photo.
    let width = inner.width.saturating_sub(1 + indent) as usize;
    let rows: Vec<_> = chats
        .ids()
        .iter()
        .filter_map(|&id| chats.get(id).map(|chat| (id, chat)))
        .collect();
    let selected = list
        .selected
        .and_then(|id| rows.iter().position(|&(x, _)| x == id));

    let items: Vec<ListItem> = rows
        .iter()
        .enumerate()
        .map(|(i, &(id, chat))| {
            let is_selected = selected == Some(i);
            // Drawn by hand: List's highlight symbol only marks an item's first row.
            let bar = if is_selected {
                Span::from("▌").fg(colors.accent)
            } else {
                Span::from(" ")
            };
            let gap = Span::from(" ".repeat(indent.into()));
            let muted = chats.muted(id);
            // The unread count, grey for a muted chat as in Telegram; else
            // a pin for a pinned one.
            let badge = if chat.unread > 0 {
                let bg = if muted { colors.muted } else { colors.primary };
                Span::from(format!(" {} ", chat.unread))
                    .fg(colors.bg)
                    .bg(bg)
            } else if chat.pinned {
                Span::from(PINNED)
            } else {
                Span::from("")
            };
            let title = chats.title(id).unwrap_or_default();
            let mark = chats.badge(id);
            let mark_w = mark.map_or(0, |m| m.mark().width());
            let mute_w = if muted { MUTED.width() } else { 0 };
            let badge_w = badge.content.width();
            let title = truncate(title, width.saturating_sub(badge_w + mark_w + mute_w + 1));
            let pad = width.saturating_sub(title.width() + mark_w + mute_w + badge_w);
            let style = title_style(chats, id, colors).bold();
            let mut first = vec![bar.clone(), gap.clone()];
            first.extend(highlight(&title, filter, style, colors));
            if let Some(mark) = mark {
                first.push(super::badge_span(mark, colors));
            }
            if muted {
                first.push(Span::from(MUTED).fg(colors.muted));
            }
            first.push(Span::from(" ".repeat(pad)));
            first.push(badge);
            // Highlighted by hand too, so the blank row below stays blank.
            let row_style = if is_selected {
                Style::new().bg(colors.selection)
            } else {
                Style::new()
            };
            let second = match activity(chat, &names) {
                Some(doing) => Span::from(truncate(&doing, width)).fg(colors.activity),
                None => Span::from(truncate(&chat.preview, width)).fg(colors.subtle),
            };
            let mut lines = vec![
                Line::from(first).style(row_style),
                Line::from(vec![bar, gap, second]).style(row_style),
            ];
            if list.gaps {
                lines.push(Line::default());
            }
            ListItem::new(lines)
        })
        .collect();

    if items.is_empty() && !filter.is_empty() {
        frame.render_widget(
            Paragraph::new("No chats match")
                .fg(colors.muted)
                .centered()
                .block(block),
            area,
        );
        return;
    }
    let mut state = ListState::default().with_selected(selected);
    frame.render_stateful_widget(List::new(items).block(block), area, &mut state);
    if photo_cols == 0 {
        return;
    }
    // The list draws only whole chats, from where it scrolled to.
    let item_rows = PHOTO_ROWS + u16::from(list.gaps);
    let shown = rows
        .iter()
        .skip(state.offset())
        .take(usize::from(inner.height / item_rows));
    for (i, &(id, _)) in shown.enumerate() {
        let area = Rect {
            x: inner.x + 1,
            y: inner.y + i as u16 * item_rows,
            width: photo_cols,
            height: PHOTO_ROWS,
        };
        draw_photo(frame, area, id, list, images, colors);
    }
}

/// Columns for a round photo two rows high: about twice the rows, since
/// cells are about twice as tall as they are wide.
fn photo_cols(font: FontSize) -> u16 {
    let (width, height) = (f64::from(font.width.max(1)), f64::from(font.height.max(1)));
    ((f64::from(PHOTO_ROWS) * height / width).round() as u16).clamp(3, 5)
}

/// The chat's photo, else a square of its accent color. Saved Messages gets
/// the app's own color instead of your photo, as Telegram shows it apart.
fn draw_photo(
    frame: &mut Frame,
    area: Rect,
    chat_id: i64,
    list: &ChatList,
    images: &mut Images,
    colors: &Colors,
) {
    let chats = list.chats;
    if chats.is_saved(chat_id) {
        frame.render_widget(Block::new().bg(colors.primary), area);
        return;
    }
    if images.draws_photos()
        && let Some(photo) = chats.get(chat_id).and_then(|c| c.photo.as_ref())
    {
        images.want_avatar(photo, area.width, area.height);
        if let Some(image) = images.avatar(photo, area.width, area.height, list.covered) {
            frame.render_widget(SlicedImage::new(image, SignedPosition::from((0, 0))), area);
            return;
        }
    }
    let color = colors.names[chats.accent(chat_id)];
    frame.render_widget(Block::new().bg(color), area);
}

#[cfg(test)]
mod tests {
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;
    use ratatui::buffer::Buffer;
    use ratatui::style::Color;
    use ratatui_image::picker::{Picker, ProtocolType};

    use std::sync::LazyLock;

    use tdlib_rs::enums::{ChatAction, MessageSender};
    use tdlib_rs::types::MessageSenderUser;

    use super::*;
    use crate::chats::ChatPhoto;
    use crate::images::Key;

    /// What the kitty protocol puts in every cell of an image.
    const KITTY_CELL: char = '\u{10EEEE}';

    /// Halfblocks need no terminal query, and their cells are 10x20 px, so
    /// photos are 4 columns wide.
    fn images(protocol: ProtocolType) -> Images {
        let mut picker = Picker::halfblocks();
        picker.set_protocol_type(protocol);
        Images::new(picker, tokio::sync::mpsc::unbounded_channel().0)
    }

    fn photo(file_id: i32) -> Option<ChatPhoto> {
        Some(ChatPhoto {
            file_id,
            path: None,
            thumbnail: None,
        })
    }

    /// Makes the photo with this file id ready to draw.
    fn add_photo(images: &mut Images, file_id: i32) {
        let key = Key {
            file_id,
            cols: 4,
            rows: 2,
            thumbnail: false,
            avatar: true,
        };
        let red = image::RgbaImage::from_pixel(40, 40, image::Rgba([255, 0, 0, 255]));
        images.insert_ready(key, red.into());
    }

    fn list(chats: &Chats, selected: Option<i64>) -> ChatList<'_> {
        static NO_USERS: LazyLock<HashMap<i64, String>> = LazyLock::new(HashMap::new);
        ChatList {
            chats,
            users: &NO_USERS,
            gaps: true,
            selected,
            loading: false,
            focused: true,
            covered: false,
        }
    }

    #[test]
    fn telegrams_scam_and_official_marks_follow_the_name() {
        use crate::chats::{Badge, Peer};
        let mut chats = Chats::default();
        chats.add_local(1, "Telegram", None).peer = Some(Peer::User(777000));
        chats.add_local(2, "Telegram", None).peer = Some(Peer::User(42));
        chats.set_badge(Peer::User(777000), Some(Badge::Official));
        chats.set_badge(Peer::User(42), Some(Badge::Scam));
        chats.refresh();
        let buf = render(
            &list(&chats, None),
            &mut images(ProtocolType::Halfblocks),
            40,
            10,
        );
        let rows: Vec<String> = (0..buf.area.height)
            .map(|y| (0..buf.area.width).map(|x| buf[(x, y)].symbol()).collect())
            .collect();
        assert!(rows.iter().any(|r| r.contains("Telegram ✓")), "{rows:#?}");
        assert!(
            rows.iter().any(|r| r.contains("Telegram SCAM")),
            "{rows:#?}"
        );
    }

    #[test]
    fn pinned_chats_show_a_pin_and_muted_ones_a_bell_and_a_grey_count() {
        use tdlib_rs::types::ChatNotificationSettings;
        let mut chats = Chats::default();
        chats.add_local(1, "Pinned", None).pinned = true;
        chats.add_local(2, "Quiet", None).unread = 4;
        chats.set_notifications(
            2,
            ChatNotificationSettings {
                mute_for: 3600,
                ..ChatNotificationSettings::default()
            },
        );
        chats.refresh();
        let colors = Colors::default();
        let buf = render(
            &list(&chats, None),
            &mut images(ProtocolType::Halfblocks),
            40,
            10,
        );
        let rows: Vec<String> = (0..buf.area.height)
            .map(|y| (0..buf.area.width).map(|x| buf[(x, y)].symbol()).collect())
            .collect();
        let pinned = rows.iter().position(|r| r.contains("Pinned")).unwrap();
        let quiet = rows.iter().position(|r| r.contains("Quiet")).unwrap();
        assert!(pinned < quiet, "pinned first, even before unread chats");
        assert!(rows[pinned].contains('📌'), "{rows:#?}");
        assert!(rows[quiet].contains("Quiet 🔕"), "{rows:#?}");
        let x = rows[quiet].chars().position(|c| c == '4').unwrap() as u16;
        assert_eq!(buf[(x, quiet as u16)].bg, colors.muted, "a grey count");
    }

    fn render(list: &ChatList, images: &mut Images, width: u16, height: u16) -> Buffer {
        let colors = Colors::default();
        let mut terminal = Terminal::new(TestBackend::new(width, height)).unwrap();
        terminal
            .draw(|f| draw(f, f.area(), list, images, &colors))
            .unwrap();
        terminal.backend().buffer().clone()
    }

    /// The symbols of one row, from column `x` on, `width` cells.
    fn cells(buf: &Buffer, x: u16, y: u16, width: u16) -> String {
        (x..x + width).map(|x| buf[(x, y)].symbol()).collect()
    }

    fn is_kitty_image(buf: &Buffer, x: u16, y: u16) -> bool {
        buf[(x, y)].symbol().contains(KITTY_CELL)
    }

    /// True when the cells from column `x` on, `width` of them, are blank
    /// and filled with `color`.
    fn is_square(buf: &Buffer, x: u16, y: u16, width: u16, color: Color) -> bool {
        (x..x + width).all(|x| buf[(x, y)].symbol() == " " && buf[(x, y)].bg == color)
    }

    #[test]
    fn chats_without_a_photo_get_a_square_of_their_color() {
        let colors = Colors::default();
        let mut chats = Chats::default();
        chats.add_local(1, "Alice Smith", None);
        chats.add_local(2, "bob", photo(20));
        chats.set_accent(1, 3);
        chats.refresh();
        let mut images = images(ProtocolType::Halfblocks);
        add_photo(&mut images, 20);
        let buf = render(&list(&chats, None), &mut images, 40, 10);

        // Border, cursor bar, then the 4-column square and a space.
        assert!(is_square(&buf, 2, 1, 4, colors.names[3]));
        assert!(is_square(&buf, 2, 2, 4, colors.names[3]));
        assert_eq!(cells(&buf, 6, 1, 12), " Alice Smith");
        assert!(!is_square(&buf, 2, 3, 4, colors.names[3]), "a blank row");
        assert_eq!(cells(&buf, 6, 4, 4), " bob", "the next chat");
        assert!(
            is_square(&buf, 2, 4, 4, colors.names[0]),
            "halfblocks are too coarse for a photo"
        );
    }

    #[test]
    fn the_selected_chat_is_highlighted_but_not_the_blank_row_below_it() {
        let colors = Colors::default();
        let mut chats = Chats::default();
        chats.add_local(1, "Alice Smith", None);
        chats.refresh();
        let buf = render(
            &list(&chats, Some(1)),
            &mut images(ProtocolType::Halfblocks),
            40,
            10,
        );
        for y in [1, 2] {
            assert_eq!(buf[(1, y)].symbol(), "▌");
            assert_eq!(buf[(7, y)].bg, colors.selection);
            assert_eq!(buf[(38, y)].bg, colors.selection, "the whole row");
        }
        assert_eq!(buf[(1, 3)].symbol(), " ");
        assert_ne!(buf[(7, 3)].bg, colors.selection);
    }

    #[test]
    fn typing_shows_in_place_of_the_last_message_while_it_lasts() {
        let colors = Colors::default();
        let mut chats = Chats::default();
        chats.add_local(1, "Alice", None).is_private = true;
        chats.add_local(2, "Climbing club", None);
        chats.refresh();
        let user = |user_id| MessageSender::User(MessageSenderUser { user_id });
        chats.set_action(1, &user(7), &ChatAction::Typing);
        chats.set_action(2, &user(7), &ChatAction::Typing);
        chats.set_action(2, &user(8), &ChatAction::Typing);
        let users = HashMap::from([(7, "Alice".to_string()), (8, "Bob".to_string())]);
        let draw = |chats: &Chats| {
            let list = ChatList {
                users: &users,
                ..list(chats, None)
            };
            render(&list, &mut images(ProtocolType::Halfblocks), 40, 10)
        };

        // Each chat: its title, then the row under it, then a blank row.
        let buf = draw(&chats);
        assert_eq!(cells(&buf, 7, 2, 7), "typing…");
        assert_eq!(buf[(7, 2)].fg, colors.activity);
        assert_eq!(cells(&buf, 7, 5, 25), "Alice and Bob are typing…");

        chats.set_action(1, &user(7), &ChatAction::Cancel);
        let buf = draw(&chats);
        assert_eq!(cells(&buf, 7, 2, 7), "       ", "back to the last message");
    }

    #[test]
    fn photos_are_drawn_beside_the_chats_scrolled_into_view() {
        let colors = Colors::default();
        let mut chats = Chats::default();
        let mut images = images(ProtocolType::Kitty);
        for id in 1..=30 {
            chats.add_local(id, &format!("chat {id}"), photo(id as i32));
            // Odd chats have photos ready, even ones still show squares.
            if id % 2 == 1 {
                add_photo(&mut images, id as i32);
            }
        }
        chats.refresh();
        // 15 rows inside the border: chats 16 to 20, the selected one last.
        let buf = render(&list(&chats, Some(20)), &mut images, 40, 17);
        assert_eq!(cells(&buf, 7, 1, 7), "chat 16");
        assert!(is_square(&buf, 2, 1, 4, colors.names[0]));
        assert_eq!(cells(&buf, 7, 4, 7), "chat 17");
        assert!(is_kitty_image(&buf, 2, 4) && is_kitty_image(&buf, 5, 5));
        assert!(!is_kitty_image(&buf, 2, 6), "the blank row");
        assert_eq!(cells(&buf, 7, 13, 7), "chat 20");
        assert!(!is_kitty_image(&buf, 2, 13));
    }

    #[test]
    fn without_gaps_chats_and_their_photos_follow_each_other() {
        let mut chats = Chats::default();
        let mut images = images(ProtocolType::Kitty);
        for id in 1..=3 {
            chats.add_local(id, &format!("chat {id}"), photo(id as i32));
            add_photo(&mut images, id as i32);
        }
        chats.refresh();
        let compact = ChatList {
            gaps: false,
            ..list(&chats, None)
        };
        let buf = render(&compact, &mut images, 40, 10);
        assert_eq!(cells(&buf, 7, 1, 6), "chat 1");
        assert_eq!(cells(&buf, 7, 3, 6), "chat 2");
        assert_eq!(cells(&buf, 7, 5, 6), "chat 3");
        assert!(is_kitty_image(&buf, 2, 3) && is_kitty_image(&buf, 5, 4));
    }

    #[test]
    fn a_new_photo_waits_while_a_popup_could_hide_it() {
        let colors = Colors::default();
        let mut chats = Chats::default();
        chats.add_local(1, "Alice", photo(10));
        chats.refresh();
        let mut images = images(ProtocolType::Kitty);
        add_photo(&mut images, 10);

        // Kitty sends the image with its first drawing; under a popup, that
        // would be lost.
        let mut covered = list(&chats, None);
        covered.covered = true;
        let buf = render(&covered, &mut images, 40, 10);
        assert!(is_square(&buf, 2, 1, 4, colors.names[0]));

        let buf = render(&list(&chats, None), &mut images, 40, 10);
        assert!(is_kitty_image(&buf, 2, 1));
        let buf = render(&covered, &mut images, 40, 10);
        assert!(is_kitty_image(&buf, 2, 1), "already sent");
    }

    #[test]
    fn saved_messages_is_a_square_of_the_app_color() {
        let colors = Colors::default();
        let mut chats = Chats::default();
        chats.add_local(7, "Eric", photo(10));
        chats.set_my_id(7);
        chats.refresh();
        let mut images = images(ProtocolType::Kitty);
        add_photo(&mut images, 10);
        let buf = render(&list(&chats, None), &mut images, 40, 10);
        assert!(is_square(&buf, 2, 1, 4, colors.primary));
        assert_eq!(cells(&buf, 7, 1, 14), "Saved Messages");
    }

    #[test]
    fn narrow_lists_leave_photos_out() {
        let mut chats = Chats::default();
        chats.add_local(1, "Alice Smith", None);
        chats.refresh();
        let buf = render(
            &list(&chats, None),
            &mut images(ProtocolType::Halfblocks),
            20,
            10,
        );
        assert_eq!(cells(&buf, 2, 1, 11), "Alice Smith");
    }
}

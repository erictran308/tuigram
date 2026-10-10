//! The chat list: two rows per chat, its title over its last message (or
//! "typing…" while someone is), with the chat's photo on the left (a square
//! of color if it has none), and a blank row between chats unless that's
//! turned off. With folders, a row of tabs over it says which one is shown,
//! with a line under them joined to the border.

use std::collections::HashMap;

use ratatui::Frame;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Style, Stylize};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, List, ListItem, ListState, Paragraph};
use ratatui_image::FontSize;
use ratatui_image::sliced::{SignedPosition, SlicedImage};
use unicode_width::UnicodeWidthStr;

use crate::chats::{self, Chats, Tab};
use crate::images::Images;
use crate::theme::Colors;

use super::messages::Names;
use super::{LOCK_WIDTH, activity, border, highlight, lock_spans, title_style, truncate};

/// Rows of text per chat, so also the height of its photo.
const PHOTO_ROWS: u16 = 2;
/// Narrower than this inside its border, the list leaves photos out to keep
/// room for titles.
const MIN_WIDTH_FOR_PHOTOS: u16 = 24;
/// After a muted chat's name.
const MUTED: &str = " 🔕";
/// After the name of a chat the MTProto proxy in use put in the list.
const SPONSORED: &str = " proxy sponsor";
/// On the right of a pinned chat with nothing unread.
const PINNED: &str = "📌";
/// In place of the count of a chat marked as unread, with nothing in it
/// unread.
const MARKED_UNREAD: &str = "●";
/// Before what was left written in a chat.
const DRAFT: &str = "Draft: ";
/// A folder's tab shows at most this many columns of its name.
const TAB_NAME_COLS: usize = 16;

/// What the list shows, from the app's state.
pub struct ChatList<'a> {
    pub chats: &'a Chats,
    /// Display names by user id, for who's typing in groups.
    pub users: &'a HashMap<i64, String>,
    pub selected: Option<i64>,
    /// The chat open, whose draft is in the composer rather than here.
    pub open: Option<i64>,
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
    let block = super::bordered(colors)
        .title(title)
        .border_style(border(list.focused, colors));
    let inner = block.inner(area);
    frame.render_widget(block, area);
    let tabs = chats.tabs();
    let inner = if tabs.is_empty() {
        inner
    } else {
        let [bar, rule, rest] = Layout::vertical([
            Constraint::Length(1),
            Constraint::Length(1),
            Constraint::Fill(1),
        ])
        .areas(inner);
        let line = tab_line(&tabs, chats.shown(), bar.width.into(), colors);
        frame.render_widget(line, bar);
        // Across the whole list, meeting the border on both sides.
        let rule_line = format!("├{}┤", "─".repeat(rule.width.into()));
        let rule = Rect {
            x: area.x,
            width: rule.width + 2,
            ..rule
        };
        frame.render_widget(Line::styled(rule_line, border(list.focused, colors)), rule);
        rest
    };
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
            // The unread count, grey for a muted chat as in Telegram, after
            // an @ while some mention you, which a mute doesn't grey; else
            // a pin for a pinned one.
            let mut badges = Vec::new();
            if chat.mentions > 0 {
                let at = Span::from(" @ ").fg(colors.bg).bg(colors.primary);
                badges.extend(super::pill(at, Style::new(), colors));
            }
            if chat.unread > 0 {
                if !badges.is_empty() {
                    badges.push(Span::from(" "));
                }
                let bg = if muted { colors.muted } else { colors.primary };
                let count = Span::from(format!(" {} ", chat.unread));
                badges.extend(super::pill(
                    count.fg(colors.bg).bg(bg),
                    Style::new(),
                    colors,
                ));
            } else if chat.marked_unread {
                // Marked as unread on another device: a dot, in the count's
                // color, where the count would be.
                if !badges.is_empty() {
                    badges.push(Span::from(" "));
                }
                let color = if muted { colors.muted } else { colors.primary };
                badges.push(Span::from(MARKED_UNREAD).fg(color));
            } else if badges.is_empty() && chats.pinned(id) {
                badges.push(Span::from(PINNED));
            }
            let title = chats.title(id).unwrap_or_default();
            let mark = chats.badge(id);
            let mark_w = mark.map_or(0, |m| m.mark().width());
            let mute_w = if muted { MUTED.width() } else { 0 };
            // Like Telegram's verdict, the title is cut to leave room for
            // it, so a long one can't hide it.
            let sponsored = chats.sponsored(id);
            let sponsored_w = if sponsored { SPONSORED.width() } else { 0 };
            let secret = chats.is_secret(id);
            let lock_w = if secret { LOCK_WIDTH } else { 0 };
            let badge_w: usize = badges.iter().map(|b| b.content.width()).sum();
            let title = truncate(
                title,
                width.saturating_sub(lock_w + badge_w + mark_w + sponsored_w + mute_w + 1),
            );
            let pad = width
                .saturating_sub(lock_w + title.width() + mark_w + sponsored_w + mute_w + badge_w);
            let style = title_style(chats, id, colors).bold();
            let mut first = vec![bar.clone(), gap.clone()];
            if secret {
                first.extend(lock_spans(colors));
            }
            first.extend(highlight(&title, filter, style, colors));
            if let Some(mark) = mark {
                first.push(super::badge_span(mark, colors));
            }
            if sponsored {
                first.push(Span::from(SPONSORED).fg(colors.muted));
            }
            if muted {
                first.push(Span::from(MUTED).fg(colors.muted));
            }
            first.push(Span::from(" ".repeat(pad)));
            first.extend(badges);
            // Highlighted by hand too, so the blank row below stays blank.
            let row_style = if is_selected {
                Style::new().bg(colors.selection)
            } else {
                Style::new()
            };
            let mut second = vec![bar, gap];
            match (activity(chat, &names, None), &chat.draft) {
                (Some(doing), _) => {
                    second.push(Span::from(truncate(&doing, width)).fg(colors.activity));
                }
                // As in Telegram, in place of the last message.
                (None, Some(draft)) if list.open != Some(id) => {
                    second.push(Span::from(DRAFT).fg(colors.error));
                    let room = width.saturating_sub(DRAFT.width());
                    second.push(Span::from(truncate(&draft.snippet(), room)).fg(colors.subtle));
                }
                (None, _) => {
                    second.push(Span::from(truncate(&chat.preview, width)).fg(colors.subtle));
                }
            }
            let mut lines = vec![
                Line::from(first).style(row_style),
                Line::from(second).style(row_style),
            ];
            if list.gaps {
                lines.push(Line::default());
            }
            ListItem::new(lines)
        })
        .collect();

    let empty = match () {
        _ if !items.is_empty() => None,
        _ if !filter.is_empty() => Some("No chats match"),
        _ if list.loading => Some("Loading…"),
        // The main list is empty only until it loads.
        _ if chats.shown() != chats::List::Main => Some("No chats here"),
        _ => None,
    };
    if let Some(empty) = empty {
        frame.render_widget(Paragraph::new(empty).fg(colors.muted).centered(), inner);
        return;
    }
    let mut state = ListState::default().with_selected(selected);
    frame.render_stateful_widget(List::new(items), inner, &mut state);
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

/// The tabs over the list, the one shown filled in, each with its count of
/// unread unmuted chats. When they don't all fit, they scroll so the one
/// shown does, with `‹` / `›` where more are hidden.
fn tab_line(tabs: &[Tab], shown: chats::List, width: usize, colors: &Colors) -> Line<'static> {
    let label = |tab: &Tab| {
        let name = truncate(&tab.name, TAB_NAME_COLS);
        match tab.unread {
            0 => format!(" {name} "),
            n => format!(" {name} {n} "),
        }
    };
    let labels: Vec<String> = tabs.iter().map(label).collect();
    let widths: Vec<usize> = labels.iter().map(|l| l.width()).collect();
    let current = tabs.iter().position(|t| t.list == shown).unwrap_or(0);
    // Columns tabs `from..=to` take, with a space between them and room for
    // the marks at both ends.
    let span = |from: usize, to: usize| {
        let gaps = to - from;
        let marks = usize::from(from > 0) + usize::from(to + 1 < tabs.len());
        widths[from..=to].iter().sum::<usize>() + gaps + marks
    };
    let mut first = 0;
    while first < current && span(first, current) > width {
        first += 1;
    }
    let mut last = current;
    while last + 1 < tabs.len() && span(first, last + 1) <= width {
        last += 1;
    }
    let mut spans = Vec::new();
    if first > 0 {
        spans.push(Span::from("‹").fg(colors.muted));
    }
    for (i, label) in labels.into_iter().enumerate().take(last + 1).skip(first) {
        if i > first {
            spans.push(Span::from(" "));
        }
        let style = if i == current {
            Style::new().fg(colors.bg).bg(colors.accent).bold()
        } else if tabs[i].unread > 0 {
            Style::new().fg(colors.fg)
        } else {
            Style::new().fg(colors.subtle)
        };
        spans.extend(super::pill(
            Span::styled(label, style),
            Style::new(),
            colors,
        ));
    }
    if last + 1 < tabs.len() {
        spans.push(Span::from("›").fg(colors.muted));
    }
    Line::from(spans)
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
            open: None,
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
    fn a_chat_the_proxy_put_in_the_list_says_so_however_long_its_name() {
        use tdlib_rs::enums::{ChatList, ChatSource};
        use tdlib_rs::types::ChatPosition;
        let mut chats = Chats::default();
        chats.add_local(1, "Friends", None);
        chats.add_local(2, &"Free Fast VPN News ".repeat(4), None);
        let sponsored = ChatPosition {
            list: ChatList::Main,
            order: 5000,
            is_pinned: false,
            source: Some(ChatSource::MtprotoProxy),
        };
        chats.set_position(2, &sponsored);
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
        let y = rows
            .iter()
            .position(|r| r.contains("proxy sponsor"))
            .unwrap_or_else(|| panic!("{rows:#?}"));
        assert!(rows[y].contains("Free Fast"), "{rows:#?}");
        let x = (0..buf.area.width)
            .find(|&x| buf[(x, y as u16)].symbol() == "p")
            .unwrap();
        assert_eq!(buf[(x, y as u16)].fg, colors.muted);
        assert_eq!(
            rows.iter().filter(|r| r.contains("proxy sponsor")).count(),
            1,
            "only that one: {rows:#?}"
        );
        // It isn't one of yours, to forward to or find in your chats.
        assert_eq!(chats.matching(""), [1]);
        assert!(chats.listed(1) && !chats.listed(2));
    }

    #[test]
    fn a_secret_chat_has_a_lock_and_its_own_color_beside_the_other_chat() {
        let mut chats = Chats::default();
        chats.add_local(1, "Chardy", None);
        chats.add_local(2, "Chardy", None).secret_id = Some(7);
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
        // An emoji's second cell is blank in the test buffer.
        let secret = rows.iter().position(|r| r.contains("🔒  Chardy")).unwrap();
        let other = rows
            .iter()
            .position(|r| r.contains("Chardy") && !r.contains('🔒'))
            .unwrap();
        assert_ne!(secret, other);
        let y = secret as u16;
        let x = (0..buf.area.width)
            .find(|&x| buf[(x, y)].symbol() == "C")
            .unwrap();
        assert_eq!(buf[(x, y)].fg, colors.secret);
        // A name can start with 🔒 too, but can't color what's behind it.
        let lock = (0..buf.area.width)
            .find(|&x| buf[(x, y)].symbol() == "🔒")
            .unwrap();
        assert_eq!(buf[(lock, y)].bg, colors.secret);
    }

    #[test]
    fn pinned_chats_show_a_pin_and_muted_ones_a_bell_and_a_grey_count() {
        use tdlib_rs::types::ChatNotificationSettings;
        let mut chats = Chats::default();
        chats
            .add_local(1, "Pinned", None)
            .set_pinned(chats::List::Main, true);
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

    #[test]
    fn a_draft_shows_in_place_of_the_last_message_except_in_the_chat_open() {
        crate::tg::quiet();
        let mut chats = Chats::default();
        chats.add_local(1, "Maya", None).preview = "see you at 5".into();
        chats.keep_draft(1, crate::draft::Draft::new("on my **way**", None));
        chats.refresh();
        let colors = Colors::default();
        let rows = |open| {
            let list = ChatList {
                open,
                ..list(&chats, None)
            };
            render(&list, &mut images(ProtocolType::Halfblocks), 40, 10)
        };
        let buf = rows(None);
        assert_eq!(cells(&buf, 7, 2, 20), "Draft: on my **way**");
        assert_eq!(buf[(7, 2)].fg, colors.error);
        assert_eq!(buf[(14, 2)].fg, colors.subtle);
        // Open, it's in the composer.
        let buf = rows(Some(1));
        assert_eq!(cells(&buf, 7, 2, 12), "see you at 5");
    }

    #[test]
    fn a_chat_marked_unread_has_a_dot_for_its_count_and_goes_with_the_unread() {
        let mut chats = Chats::default();
        chats.add_local(1, "First", None);
        chats.add_local(2, "Marked", None);
        chats.set_marked_unread(2, true);
        chats.refresh();
        assert_eq!(chats.ids(), [2, 1], "with the unread chats, on top");
        let colors = Colors::default();
        let buf = render(
            &list(&chats, None),
            &mut images(ProtocolType::Halfblocks),
            40,
            10,
        );
        // The count's place, right of the title.
        assert_eq!(cells(&buf, 7, 1, 6), "Marked");
        assert_eq!(cells(&buf, 37, 1, 3), " ●│");
        assert_eq!(buf[(38, 1)].fg, colors.primary);
        chats.set_marked_unread(2, false);
        chats.refresh();
        assert_eq!(chats.ids(), [1, 2]);
    }

    #[test]
    fn a_chat_that_mentions_you_shows_an_at_before_its_count_even_muted() {
        use tdlib_rs::types::ChatNotificationSettings;
        let mut chats = Chats::default();
        let chat = chats.add_local(1, "Team", None);
        (chat.unread, chat.mentions) = (5, 2);
        chats.set_notifications(
            1,
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
        let y = rows.iter().position(|r| r.contains("Team")).unwrap();
        assert!(rows[y].trim_end().ends_with(" @   5 │"), "{rows:#?}");
        let at = rows[y].chars().position(|c| c == '@').unwrap() as u16;
        assert_eq!(
            buf[(at, y as u16)].bg,
            colors.primary,
            "a mute doesn't grey it"
        );
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
        chats.set_action(1, None, &user(7), &ChatAction::Typing);
        chats.set_action(2, None, &user(7), &ChatAction::Typing);
        chats.set_action(2, None, &user(8), &ChatAction::Typing);
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

        chats.set_action(1, None, &user(7), &ChatAction::Cancel);
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
    fn folders_are_tabs_over_the_list_scrolled_to_the_one_shown() {
        use tdlib_rs::types::{ChatFolderInfo, ChatFolderName, FormattedText};
        let colors = Colors::default();
        let folder = |id, name: &str| ChatFolderInfo {
            id,
            name: ChatFolderName {
                text: FormattedText {
                    text: name.into(),
                    entities: Vec::new(),
                },
                animate_custom_emoji: false,
            },
            ..ChatFolderInfo::default()
        };
        let mut chats = Chats::default();
        chats.add_local(1, "Alice", None);
        chats.set_folders(
            &[folder(1, "Friends"), folder(2, "Work"), folder(3, "Games")],
            0,
        );
        chats.set_unread_in(chats::List::Folder(2), 4);
        chats.refresh();
        let rows = |chats: &Chats, width| {
            let buf = render(
                &list(chats, None),
                &mut images(ProtocolType::Halfblocks),
                width,
                10,
            );
            let row: String = cells(&buf, 1, 1, width - 2);
            (buf, row)
        };

        let (buf, row) = rows(&chats, 40);
        assert_eq!(row.trim_end(), " All   Friends   Work 4   Games");
        assert_eq!(buf[(2, 1)].bg, colors.accent, "the one shown");
        let rule = format!("├{}┤", "─".repeat(38));
        assert_eq!(cells(&buf, 0, 2, 40), rule, "a line under the tabs");
        assert_eq!(buf[(0, 2)].fg, colors.accent, "the border's color");
        assert_eq!(cells(&buf, 7, 3, 5), "Alice", "the chats under it");

        chats.show(chats::List::Folder(3));
        chats.refresh();
        let (_, row) = rows(&chats, 20);
        assert_eq!(row.trim_end(), "‹ Work 4   Games", "{row}");
        let (_, row) = rows(&chats, 40);
        assert!(row.contains("Games"));
        let (buf, _) = rows(&chats, 40);
        assert!(
            cells(&buf, 1, 3, 38).contains("No chats here"),
            "Alice isn't in Games"
        );
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

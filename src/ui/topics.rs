//! The topics pane: a forum's topics, between the chat list and the
//! messages. Two rows per topic, as in the chat list: its icon and name
//! over its newest message, and a blank row between topics unless that's
//! turned off.

use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::{Style, Stylize};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, List, ListItem, ListState, Paragraph};
use unicode_width::UnicodeWidthStr;

use crate::theme::Colors;
use crate::topics::{Forum, Topic};

use super::messages::Names;
use super::{border, truncate};

/// The General topic's icon, as in Telegram.
const GENERAL_ICON: &str = "#";
/// Other topics', in their color.
const ICON: &str = "●";
/// On the right of a pinned topic with nothing unread.
const PINNED: &str = "📌";
/// After a closed topic's name.
const CLOSED: &str = " closed";
/// Columns a topic's name keeps at least, before "closed" makes way.
const MIN_NAME: usize = 6;

/// What the pane shows, from the app's state.
pub struct TopicList<'a> {
    pub forum: &'a Forum,
    pub names: &'a Names<'a>,
    pub focused: bool,
    /// A blank row between topics.
    pub gaps: bool,
}

pub fn draw(frame: &mut Frame, area: Rect, list: &TopicList, colors: &Colors) {
    let forum = list.forum;
    let chats = list.names.chats;
    // Telegram's verdict on the forum (SCAM, FAKE, ✓) always shows: the
    // name is cut to leave room for it, since the topics may be all there
    // is of the forum on screen.
    let badge = chats.badge(forum.chat_id);
    let after = if forum.loading.is_some() {
        " · topics loading… "
    } else {
        " · topics "
    };
    let badge_w = badge.map_or(0, |b| b.mark().width());
    let room = usize::from(area.width.saturating_sub(3)).saturating_sub(badge_w + after.width());
    let name = truncate(chats.title(forum.chat_id).unwrap_or_default(), room);
    let mut title = vec![
        Span::from(" "),
        Span::from(name)
            .style(super::title_style(chats, forum.chat_id, colors))
            .bold(),
    ];
    if let Some(badge) = badge {
        title.push(super::badge_span(badge, colors));
    }
    title.push(Span::from(after));
    let block = Block::bordered()
        .title(Line::from(title))
        .border_style(border(list.focused, colors));
    let inner = block.inner(area);
    frame.render_widget(block, area);
    let topics = forum.topics();
    if topics.is_empty() {
        let empty = if forum.all_loaded() {
            "No topics"
        } else {
            "Loading…"
        };
        frame.render_widget(Paragraph::new(empty).fg(colors.muted).centered(), inner);
        return;
    }
    // What's left for text after the 1-column cursor bar.
    let width = inner.width.saturating_sub(1) as usize;
    let current = forum.current().map(|t| t.id);
    let selected = topics.iter().position(|t| Some(t.id) == current);
    let items: Vec<ListItem> = topics
        .iter()
        .map(|topic| {
            let mut lines = rows(topic, Some(topic.id) == current, width, list, colors);
            if list.gaps {
                lines.push(Line::default());
            }
            ListItem::new(lines)
        })
        .collect();
    let mut state = ListState::default().with_selected(selected);
    frame.render_stateful_widget(List::new(items), inner, &mut state);
}

/// A topic's two rows: its icon, name and unread count, then who sent its
/// newest message and the start of it.
fn rows(
    topic: &Topic,
    is_selected: bool,
    width: usize,
    list: &TopicList,
    colors: &Colors,
) -> Vec<Line<'static>> {
    // Drawn by hand, as in the chat list: List's highlight symbol only
    // marks an item's first row.
    let bar = if is_selected {
        Span::from("▌").fg(colors.accent)
    } else {
        Span::from(" ")
    };
    let icon = if topic.general {
        Span::from(GENERAL_ICON).fg(colors.muted)
    } else {
        Span::from(ICON).fg(colors.names[topic.accent()])
    };
    let mut badges = Vec::new();
    if topic.mentions > 0 {
        badges.push(Span::from(" @ ").fg(colors.bg).bg(colors.primary));
    }
    if topic.unread > 0 {
        if !badges.is_empty() {
            badges.push(Span::from(" "));
        }
        let count = format!(" {} ", topic.unread);
        badges.push(Span::from(count).fg(colors.bg).bg(colors.primary));
    } else if topic.pinned {
        badges.push(Span::from(PINNED));
    }
    let badges_w: usize = badges.iter().map(|b| b.content.width()).sum();
    // The icon and a space after it.
    let icon_w = 2;
    // In a narrow pane, "closed" goes before the counts or the name would.
    let closed = topic.closed && width >= icon_w + CLOSED.width() + badges_w + MIN_NAME;
    let closed_w = if closed { CLOSED.width() } else { 0 };
    let name = truncate(
        &topic.name,
        width.saturating_sub(icon_w + closed_w + badges_w + 1),
    );
    let pad = width.saturating_sub(icon_w + name.width() + closed_w + badges_w);
    let mut first = vec![bar.clone(), icon, Span::from(" "), Span::from(name).bold()];
    if closed {
        first.push(Span::from(CLOSED).fg(colors.muted));
    }
    first.push(Span::from(" ".repeat(pad)));
    first.extend(badges);
    // Under the name, not the icon: who sent the newest message, in a
    // color of their own (you in yours, so a person named "You" can't pass
    // for you), then what it says.
    let room = width.saturating_sub(icon_w);
    let mut second = vec![bar, Span::from(" ".repeat(icon_w))];
    let who = match topic.from {
        _ if topic.preview.is_empty() => None,
        Some(sender) => Some(Span::from(list.names.get(sender)).fg(colors.fg)),
        None if topic.yours => Some(Span::from("You").fg(colors.primary)),
        None => None,
    };
    let mut used = 0;
    if let Some(who) = who {
        let name = truncate(&who.content, room / 2);
        used = name.width() + 2;
        second.push(Span::styled(name, who.style));
        second.push(Span::from(": ").fg(colors.subtle));
    }
    let text = truncate(&topic.preview, room.saturating_sub(used));
    second.push(Span::from(text).fg(colors.subtle));
    // Highlighted by hand too, so the blank row below stays blank.
    let row_style = if is_selected {
        Style::new().bg(colors.selection)
    } else {
        Style::new()
    };
    vec![
        Line::from(first).style(row_style),
        Line::from(second).style(row_style),
    ]
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use ratatui::Terminal;
    use ratatui::backend::TestBackend;

    use super::*;
    use crate::chats::{Badge, Chats, Peer};
    use crate::messages::Sender;
    use crate::topics::Topic;

    const FORUM: i64 = -100;

    fn draw_rows(forum: &Forum, width: u16, height: u16) -> Vec<String> {
        let mut chats = Chats::default();
        chats.add_local(FORUM, "Rustaceans", None);
        draw_in(&chats, forum, width, height)
    }

    fn draw_in(chats: &Chats, forum: &Forum, width: u16, height: u16) -> Vec<String> {
        let users = HashMap::from([(2, "Maya Chen".to_string())]);
        let names = Names {
            users: &users,
            chats,
        };
        let list = TopicList {
            forum,
            names: &names,
            focused: true,
            gaps: true,
        };
        let mut terminal = Terminal::new(TestBackend::new(width, height)).unwrap();
        terminal
            .draw(|frame| draw(frame, frame.area(), &list, &Colors::default()))
            .unwrap();
        let buffer = terminal.backend().buffer();
        (0..height)
            .map(|y| {
                (0..width)
                    .map(|x| buffer[(x, y)].symbol())
                    .collect::<String>()
                    .trim_end()
                    .to_string()
            })
            .collect()
    }

    #[test]
    fn each_topic_shows_its_name_its_unread_count_and_its_newest_message() {
        let mut forum = Forum::new(FORUM);
        forum.add_local(Topic::local(1, "General", 0, 0, "welcome!"));
        let mut help = Topic::local(2, "Help", 0x6F_B9_F0, 3, "how do I borrow this?");
        help.from = Some(Sender::User(2));
        forum.add_local(help);
        let mut rules = Topic::local(3, "Rules", 0xFB_6F_5F, 0, "be kind");
        rules.closed = true;
        forum.add_local(rules);
        let rows = draw_rows(&forum, 40, 12);
        assert!(rows[0].contains("Rustaceans · topics"), "{rows:?}");
        assert!(rows[1].starts_with("│▌# General"), "{rows:?}");
        assert!(rows[2].starts_with("│▌  welcome!  "), "{rows:?}");
        assert!(rows[4].starts_with("│ ● Help"), "{rows:?}");
        assert!(rows[4].ends_with(" 3 │"), "{rows:?}");
        assert!(
            rows[5].contains("Maya Chen: how do I borrow this?"),
            "{rows:?}"
        );
        assert!(rows[7].contains("● Rules closed"), "{rows:?}");
    }

    #[test]
    fn telegrams_verdict_on_the_forum_shows_however_long_its_name() {
        let mut chats = Chats::default();
        let name = "Binance Support Official Announcements and Help Desk";
        chats.add_local(FORUM, name, None).peer = Some(Peer::Supergroup(7));
        chats.set_badge(Peer::Supergroup(7), Some(Badge::Scam));
        let forum = Forum::new(FORUM);
        let rows = draw_in(&chats, &forum, 30, 4);
        assert!(rows[0].contains("… SCAM · topics"), "{rows:?}");
    }

    #[test]
    fn someone_named_you_doesnt_pass_for_you() {
        let mut forum = Forum::new(FORUM);
        let mut theirs = Topic::local(2, "Help", 0, 0, "send me the code");
        theirs.from = Some(Sender::User(3));
        forum.add_local(theirs);
        let mut yours = Topic::local(3, "Jobs", 0, 0, "thanks");
        yours.yours = true;
        forum.add_local(yours);
        let mut chats = Chats::default();
        chats.add_local(FORUM, "Rustaceans", None);
        let users = HashMap::from([(3, "You".to_string())]);
        let names = Names {
            users: &users,
            chats: &chats,
        };
        let list = TopicList {
            forum: &forum,
            names: &names,
            focused: true,
            gaps: false,
        };
        let colors = Colors::default();
        let mut terminal = Terminal::new(TestBackend::new(40, 6)).unwrap();
        terminal
            .draw(|frame| draw(frame, frame.area(), &list, &colors))
            .unwrap();
        let buffer = terminal.backend().buffer();
        // "You" starts after the border, the cursor bar and the indent.
        let (theirs, yours) = (buffer[(4, 2)].clone(), buffer[(4, 4)].clone());
        assert_eq!((theirs.symbol(), yours.symbol()), ("Y", "Y"));
        assert_ne!(theirs.fg, yours.fg, "drawn apart");
        assert_eq!(yours.fg, colors.primary);
    }

    #[test]
    fn in_a_narrow_pane_closed_makes_way_for_the_unread_count() {
        let mut forum = Forum::new(FORUM);
        let mut topic = Topic::local(2, "Announcements", 0, 1234, "");
        topic.closed = true;
        topic.mentions = 1;
        forum.add_local(topic);
        let rows = draw_rows(&forum, 22, 6);
        assert!(rows[1].ends_with(" @   1234 │"), "{rows:?}");
        assert!(!rows[1].contains("closed"), "{rows:?}");
        let rows = draw_rows(&forum, 40, 6);
        assert!(rows[1].contains("closed"), "{rows:?}");
    }

    #[test]
    fn a_forum_still_loading_says_so() {
        let mut forum = Forum::new(FORUM);
        assert!(forum.page_to_ask(1).is_some());
        let rows = draw_rows(&forum, 40, 6);
        assert!(rows[0].contains("loading…"), "{rows:?}");
        assert!(rows[1].contains("Loading…"), "{rows:?}");
    }
}

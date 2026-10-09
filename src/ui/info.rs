//! The `I` popup: what a chat is, and who's in it.

use chrono::Local;
use ratatui::Frame;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Style, Stylize};
use ratatui::text::{Line, Span};
use ratatui::widgets::{List, ListItem, ListState, Paragraph};
use unicode_width::UnicodeWidthStr;

use super::messages::{Names, seen_label, wrap};
use super::{Cover, badge_span, center, lock_spans, popup_block, title_style, truncate};
use crate::chats::Peer;
use crate::info::{ChatInfo, Member, Role, Roster};
use crate::messages::Sender;
use crate::theme::Colors;

/// The popup's width at most, borders included.
const WIDTH: u16 = 64;
/// Columns the labels on the left take: "In common", and a space.
const LABEL: usize = 11;
/// Rows a bio or description takes at most, so the members keep room.
const MAX_BIO_ROWS: usize = 5;
/// Columns an admin's title takes at most: Telegram allows 16.
const MAX_TITLE: usize = 16;

pub(super) fn draw(frame: &mut Frame, area: Rect, info: &ChatInfo, names: &Names, colors: &Colors) {
    let chats = names.chats;
    let width = WIDTH.min(area.width);
    // Inside the borders, a column of margin each side.
    let text_w = usize::from(width.saturating_sub(4)).max(1);
    let mut head = vec![title(info.chat_id, names, text_w, colors)];
    if let Some(about) = subtitle(info, names) {
        head.push(Line::from(truncate(&about, text_w)).fg(colors.muted));
    }
    head.push(Line::default());
    let value_w = text_w.saturating_sub(LABEL).max(1);
    let field = |label: &str, value: String| {
        Line::from(vec![
            Span::from(format!("{label:<LABEL$}")).fg(colors.muted),
            Span::from(truncate(&value, value_w)),
        ])
    };
    let peer = chats.get(info.chat_id).and_then(|c| c.peer);
    let person = matches!(peer, Some(Peer::User(_)));
    match &info.about {
        None if info.failed => head.push(Line::from("Couldn't get it").fg(colors.muted)),
        None => head.push(Line::from("Loading…").fg(colors.muted)),
        Some(about) => {
            if let Some(name) = chats.username(info.chat_id) {
                head.push(match person {
                    true => field("Username", format!("@{name}")),
                    false => field("Link", format!("t.me/{name}")),
                });
            }
            if !about.phone.is_empty() {
                head.push(field("Phone", format!("+{}", about.phone)));
            }
            if !about.bio.is_empty() {
                let bot = match peer {
                    Some(Peer::User(id)) => chats.is_bot(id),
                    _ => false,
                };
                let label = if person && !bot { "Bio" } else { "About" };
                let mut rows = wrap(&about.bio, value_w);
                if rows.len() > MAX_BIO_ROWS {
                    rows.truncate(MAX_BIO_ROWS);
                    if let Some((last, _)) = rows.last_mut() {
                        *last = truncate(&format!("{last}…"), value_w);
                    }
                }
                for (i, (row, _)) in rows.into_iter().enumerate() {
                    head.push(field(if i == 0 { label } else { "" }, row));
                }
            }
            if about.common_groups > 0 {
                let groups = count(about.common_groups, "group");
                head.push(field("In common", groups));
            }
            if head.last().is_some_and(|l| l.width() > 0) {
                head.push(Line::default());
            }
        }
    }
    let roster = info.about.as_ref().map(|a| &a.roster);
    let listed = matches!(roster, Some(Roster::All(_) | Roster::Paged { .. }));
    let lists = listed || matches!(roster, Some(Roster::Hidden(_)));
    if !lists && head.last().is_some_and(|l| l.width() == 0) {
        head.pop();
    }

    // The members get what's left of the pane, as many as there are.
    let head_rows = head.len() as u16;
    let member_rows = match roster {
        _ if !lists => 0,
        Some(Roster::Hidden(_)) => 1,
        _ => info.members.len().max(1) as u16,
    };
    let section = u16::from(lists);
    let height = (2 + head_rows + section + member_rows).min(area.height);
    let popup = center(area, width, height);
    let keys = if listed && !info.members.is_empty() {
        " `j/k` move · `Enter` write to them · `Esc` close "
    } else {
        " `Esc` close "
    };
    let block = popup_block(" Info ", keys, colors);
    let inner = block.inner(popup);
    frame.render_widget(Cover, popup);
    frame.render_widget(block, popup);
    let [head_area, section_area, members_area] = Layout::vertical([
        Constraint::Length(head_rows),
        Constraint::Length(section),
        Constraint::Fill(1),
    ])
    .areas(inner);
    let margin = |r: Rect| Rect {
        x: r.x + 1,
        width: r.width.saturating_sub(2),
        ..r
    };
    frame.render_widget(Paragraph::new(head), margin(head_area));
    if !lists {
        return;
    }

    let channel = chats.get(info.chat_id).is_some_and(|c| c.is_channel);
    let word = if channel { "Subscribers" } else { "Members" };
    let total = info
        .about
        .as_ref()
        .map_or(0, |a| a.member_count)
        .max(info.members.len() as i32);
    let header = match total {
        0 => word.to_string(),
        n => format!("{word} ({})", thousands(n)),
    };
    frame.render_widget(Line::from(header).bold(), margin(section_area));
    if let Some(Roster::Hidden(why)) = roster {
        frame.render_widget(Line::from(*why).fg(colors.muted), margin(members_area));
        return;
    }
    if info.members.is_empty() {
        let text = if info.loading_members() {
            "Loading…"
        } else {
            "Nobody to show"
        };
        frame.render_widget(Line::from(text).fg(colors.muted), margin(members_area));
        return;
    }
    let row_w = usize::from(members_area.width).saturating_sub(2);
    let now = Local::now().timestamp();
    let items: Vec<ListItem> = info
        .members
        .iter()
        .enumerate()
        .map(|(i, member)| {
            let bar = if i == info.selected {
                Span::from("▌").fg(colors.accent)
            } else {
                Span::from(" ")
            };
            let mut spans = vec![bar];
            spans.extend(member_spans(member, names, row_w, now, colors));
            ListItem::new(Line::from(spans))
        })
        .collect();
    frame.render_stateful_widget(
        List::new(items).highlight_style(Style::new().bg(colors.selection)),
        members_area,
        &mut ListState::default().with_selected(Some(info.selected)),
    );
}

/// The chat's name, as its title shows it: a secret chat's lock, and
/// Telegram's verdict, which a long name is cut to leave room for.
fn title(chat_id: i64, names: &Names, width: usize, colors: &Colors) -> Line<'static> {
    let chats = names.chats;
    let mut spans = Vec::new();
    let secret = chats.is_secret(chat_id);
    if secret {
        spans.extend(lock_spans(colors));
    }
    let badge = chats.badge(chat_id);
    let room = width
        .saturating_sub(if secret { super::LOCK_WIDTH } else { 0 })
        .saturating_sub(badge.map_or(0, |b| b.mark().width()));
    let name = truncate(chats.title(chat_id).unwrap_or_default(), room);
    spans.push(Span::styled(
        name,
        title_style(chats, chat_id, colors).bold(),
    ));
    if let Some(badge) = badge {
        spans.push(badge_span(badge, colors));
    }
    Line::from(spans)
}

/// What kind of chat it is, and how many are in it; or when the person
/// was last seen.
fn subtitle(info: &ChatInfo, names: &Names) -> Option<String> {
    let chats = names.chats;
    let chat = chats.get(info.chat_id)?;
    if chats.is_saved(info.chat_id) {
        return None;
    }
    let seen = chats
        .seen(info.chat_id)
        .map(|seen| seen_label(seen, Local::now().timestamp()).0);
    let (kind, someone) = match chat.peer? {
        Peer::User(_) if chats.is_secret(info.chat_id) => {
            return Some(match seen {
                Some(seen) => format!("secret chat · {seen}"),
                None => "secret chat".into(),
            });
        }
        Peer::User(_) => return seen,
        _ if chat.is_channel => ("channel", "subscriber"),
        _ if chats.is_forum(info.chat_id) => ("group with topics", "member"),
        _ => ("group", "member"),
    };
    let members = info.about.as_ref().map_or(0, |a| a.member_count);
    Some(match members {
        0 => kind.to_string(),
        n => format!("{kind} · {}", count(n, someone)),
    })
}

/// A member's row: their name and Telegram's verdict on them, when they
/// were last seen, and on the right what they are in the group.
fn member_spans(
    member: &Member,
    names: &Names,
    width: usize,
    now: i64,
    colors: &Colors,
) -> Vec<Span<'static>> {
    let chats = names.chats;
    let name = names.get(member.who);
    // You in your own color, with "(me)" apart: a name is anyone's to
    // pick, "Sam (me)" too.
    let me = matches!(member.who, Sender::User(id) if chats.my_id() == Some(id));
    let (badge, seen) = match member.who {
        Sender::User(id) => (chats.user_badge(id), chats.user_seen(id)),
        Sender::Chat(id) => (chats.badge(id), None),
    };
    let mine = if me { " (me)" } else { "" };
    let role = match (member.role, member.title.is_empty()) {
        (Role::Member, _) => String::new(),
        (_, false) => truncate(&member.title, MAX_TITLE),
        (Role::Owner, true) => "owner".into(),
        (Role::Admin, true) => "admin".into(),
    };
    let (seen, online) = seen.map_or((String::new(), false), |s| {
        let (label, online) = seen_label(s, now);
        (format!(" · {label}"), online)
    });
    let badge_w = badge.map_or(0, |b| b.mark().width());
    let role_w = if role.is_empty() { 0 } else { role.width() + 1 };
    // The name keeps at least half the row: when last seen doesn't fit
    // beside it, it goes.
    let room = width.saturating_sub(badge_w + role_w + mine.width());
    let seen = if name.width() + seen.width() <= room || seen.width() * 2 <= room {
        seen
    } else {
        String::new()
    };
    let name = truncate(&name, room.saturating_sub(seen.width()));
    let pad = width.saturating_sub(name.width() + mine.width() + badge_w + seen.width() + role_w);
    let mut spans = match me {
        true => vec![
            Span::from(name).fg(colors.primary),
            Span::from(mine).fg(colors.muted),
        ],
        false => vec![Span::from(name)],
    };
    if let Some(badge) = badge {
        spans.push(badge_span(badge, colors));
    }
    let seen_color = if online {
        colors.activity
    } else {
        colors.muted
    };
    spans.push(Span::from(seen).fg(seen_color));
    spans.push(Span::from(" ".repeat(pad + usize::from(role_w > 0))));
    // Not in the accent color Telegram's ✓ is drawn in: an admin's title
    // is theirs to pick.
    spans.push(Span::from(role).fg(colors.muted));
    spans
}

/// "1 member", "1,234 members".
fn count(n: i32, one: &str) -> String {
    match n {
        1 => format!("1 {one}"),
        n => format!("{} {one}s", thousands(n)),
    }
}

/// 1234567 as "1,234,567".
fn thousands(n: i32) -> String {
    let digits = n.unsigned_abs().to_string();
    let mut out = String::new();
    for (i, digit) in digits.chars().enumerate() {
        if i > 0 && (digits.len() - i).is_multiple_of(3) {
            out.push(',');
        }
        out.push(digit);
    }
    if n < 0 {
        out.insert(0, '-');
    }
    out
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use ratatui::Terminal;
    use ratatui::backend::TestBackend;
    use tdlib_rs::types::Usernames;

    use super::*;
    use crate::chats::{Badge, Chats, Presence};
    use crate::info::About;

    fn rows(info: &ChatInfo, chats: &Chats, users: &HashMap<i64, String>) -> Vec<String> {
        let names = Names { users, chats };
        let mut terminal = Terminal::new(TestBackend::new(70, 24)).unwrap();
        terminal
            .draw(|f| draw(f, f.area(), info, &names, &Colors::default()))
            .unwrap();
        let buf = terminal.backend().buffer();
        (0..buf.area.height)
            .map(|y| (0..buf.area.width).map(|x| buf[(x, y)].symbol()).collect())
            .collect()
    }

    /// The cell at `at` of the popup drawn.
    fn styled(
        info: &ChatInfo,
        chats: &Chats,
        users: &HashMap<i64, String>,
        at: (u16, u16),
    ) -> ratatui::buffer::Cell {
        let names = Names { users, chats };
        let mut terminal = Terminal::new(TestBackend::new(70, 24)).unwrap();
        terminal
            .draw(|f| draw(f, f.area(), info, &names, &Colors::default()))
            .unwrap();
        terminal.backend().buffer()[at].clone()
    }

    fn usernames(name: &str) -> Usernames {
        Usernames {
            active_usernames: vec![name.into()],
            ..Usernames::default()
        }
    }

    #[test]
    fn a_person_shows_when_they_were_seen_their_username_phone_and_bio() {
        let mut chats = Chats::default();
        let chat = chats.add_local(7, "Ann Lee", None);
        (chat.is_private, chat.peer) = (true, Some(Peer::User(7)));
        chats.set_username(Peer::User(7), Some(&usernames("ann")));
        chats.set_presence(7, Presence::Online(i32::MAX));
        let mut info = ChatInfo::new(7);
        let users = HashMap::new();
        let loading = rows(&info, &chats, &users);
        assert!(
            loading.iter().any(|r| r.contains("Loading…")),
            "{loading:#?}"
        );

        info.set_about(About {
            phone: "15550100".into(),
            bio: "Climber. Coffee first, then the mountain, then more coffee and a nap".into(),
            common_groups: 2,
            ..About::default()
        });
        let rows = rows(&info, &chats, &users);
        let text = rows.join("\n");
        for expected in [
            "Ann Lee",
            "online",
            "Username   @ann",
            "Phone      +15550100",
            "Bio        Climber.",
            "In common  2 groups",
            "Esc close",
        ] {
            assert!(text.contains(expected), "{expected}: {rows:#?}");
        }
        assert!(!text.contains("Members"), "{rows:#?}");
    }

    #[test]
    fn a_group_lists_its_members_with_what_they_are_and_who_is_you() {
        let mut chats = Chats::default();
        chats.set_my_id(1);
        chats.add_local(-100, "Climbers", None).peer = Some(Peer::Supergroup(100));
        chats.set_username(Peer::Supergroup(100), Some(&usernames("climb")));
        chats.set_badge(Peer::User(3), Some(Badge::Scam));
        chats.set_presence(2, Presence::Offline(0));
        let users = HashMap::from([
            (1, "Sam".to_string()),
            (2, "Ann".to_string()),
            (3, "Free Crypto".to_string()),
        ]);
        let member = |id, role, title: &str| Member {
            who: Sender::User(id),
            role,
            title: title.into(),
        };
        let mut info = ChatInfo::new(-100);
        info.set_about(About {
            bio: "Weekend climbs around the bay".into(),
            member_count: 1234,
            roster: Roster::All(vec![
                member(2, Role::Owner, ""),
                member(1, Role::Admin, "Route setter"),
                member(3, Role::Member, ""),
            ]),
            ..About::default()
        });
        info.move_cursor(1);
        let rows = rows(&info, &chats, &users);
        let text = rows.join("\n");
        for expected in [
            "group · 1,234 members",
            "Link       t.me/climb",
            "About      Weekend climbs",
            "Members (1,234)",
            "Enter write to them",
        ] {
            assert!(text.contains(expected), "{expected}: {rows:#?}");
        }
        let row = |name: &str| rows.iter().find(|r| r.contains(name)).unwrap().clone();
        assert!(row("Ann").contains("owner"), "{rows:#?}");
        assert!(row("Ann").contains("last seen"), "{rows:#?}");
        assert!(row("Sam (me)").contains("Route setter"), "{rows:#?}");
        // You in your color, and titles not in the color of Telegram's ✓.
        let colors = Colors::default();
        let cell = |name: &str, needle: &str| {
            let y = rows.iter().position(|r| r.contains(name)).unwrap();
            let x = rows[y].chars().position(|_| true).unwrap();
            let x = x + rows[y][..rows[y].find(needle).unwrap()].chars().count();
            (x as u16, y as u16)
        };
        assert_eq!(
            styled(&info, &chats, &users, cell("Sam (me)", "Sam")).fg,
            colors.primary
        );
        assert_eq!(
            styled(&info, &chats, &users, cell("Sam (me)", "Route")).fg,
            colors.muted
        );
        assert!(row("Sam (me)").contains('▌'), "the cursor: {rows:#?}");
        assert!(
            row("Free Crypto SCAM").trim_end().ends_with('│'),
            "{rows:#?}"
        );

        // A channel whose members only its admins see.
        let mut hidden = ChatInfo::new(-100);
        hidden.set_about(About {
            roster: Roster::Hidden("Only its admins can see who's in it"),
            ..About::default()
        });
        let text = super::tests::rows(&hidden, &chats, &users).join("\n");
        assert!(
            text.contains("Only its admins can see who's in it"),
            "{text}"
        );
        assert!(!text.contains("Enter"), "{text}");
    }

    #[test]
    fn counts_have_thousands_separated() {
        assert_eq!(count(1, "member"), "1 member");
        assert_eq!(count(1_234_567, "member"), "1,234,567 members");
        assert_eq!(thousands(999), "999");
        assert_eq!(thousands(1000), "1,000");
    }
}

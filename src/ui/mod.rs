use ratatui::Frame;
use ratatui::layout::{Alignment, Constraint, Flex, Layout, Rect};
use ratatui::style::{Style, Stylize};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Clear, List, ListItem, ListState, Paragraph, Wrap};
use unicode_width::UnicodeWidthStr;

use ratatui_textarea::TextArea;

use crate::app::{
    App, DeleteMenu, Focus, Login, LoginStep, OpenMenu, Screen, SearchTarget, SettingsMenu,
};
use crate::messages::{OpenChat, Replied};
use crate::search;
use crate::theme::{Colors, Theme};

/// The composer grows with its text up to this many rows, then scrolls.
const MAX_COMPOSER_ROWS: usize = 6;
/// The "Reply to …" bar at the top of the composer: who, then what they said.
const REPLY_BAR_ROWS: u16 = 2;

mod messages;

pub fn draw(frame: &mut Frame, app: &mut App) {
    let colors = app.settings.theme.colors();
    // The theme's background and text color everywhere; widgets drawn on top
    // only change what they style themselves.
    frame.render_widget(
        Block::new().style(Style::new().fg(colors.fg).bg(colors.bg)),
        frame.area(),
    );
    match &app.screen {
        Screen::Login(login) => draw_login(frame, login, &colors),
        Screen::Main => draw_main(frame, app, &colors),
    }
}

/// Chats you highlighted (`H`) stand out the most, then Saved Messages, as in
/// Telegram. Other titles keep whatever style surrounds them.
fn title_style(chats: &crate::chats::Chats, chat_id: i64, colors: &Colors) -> Style {
    if chats.is_highlighted(chat_id) {
        Style::new().fg(colors.highlighted)
    } else if chats.is_saved(chat_id) {
        Style::new().fg(colors.primary)
    } else {
        Style::new()
    }
}

/// How text matching a `/` search stands out.
fn match_style(colors: &Colors) -> Style {
    Style::new().fg(colors.bg).bg(colors.search)
}

/// `text` as spans in `style`, with the parts matching `query` highlighted.
fn highlight(text: &str, query: &str, style: Style, colors: &Colors) -> Vec<Span<'static>> {
    let mut spans = Vec::new();
    let mut done = 0;
    for range in search::find(text, query) {
        if range.start > done {
            spans.push(Span::styled(text[done..range.start].to_string(), style));
        }
        spans.push(Span::styled(
            text[range.clone()].to_string(),
            style.patch(match_style(colors)),
        ));
        done = range.end;
    }
    if done < text.len() || spans.is_empty() {
        spans.push(Span::styled(text[done..].to_string(), style));
    }
    spans
}

/// Focused pane gets a bright border.
fn border(focused: bool, colors: &Colors) -> Style {
    Style::new().fg(if focused {
        colors.accent
    } else {
        colors.border
    })
}

fn draw_login(frame: &mut Frame, login: &Login, colors: &Colors) {
    let area = center(frame.area(), 64, 12);
    let block = Block::bordered()
        .title(" Log in ")
        .title_alignment(Alignment::Center)
        .border_style(Style::new().fg(colors.accent));
    let inner = block.inner(area);
    frame.render_widget(block, area);

    let [prompt, help, input, error, footer] = Layout::vertical([
        Constraint::Length(1),
        Constraint::Length(3),
        Constraint::Length(3),
        Constraint::Length(1),
        Constraint::Length(1),
    ])
    .areas(inner.inner(ratatui::layout::Margin::new(1, 0)));

    let (title, text) = match &login.step {
        LoginStep::Connecting => ("Connecting…".to_string(), String::new()),
        LoginStep::ApiId => (
            "Telegram API ID".into(),
            "Telegram gives every app its own API ID and hash. Get yours once at \
             my.telegram.org → API development tools (any app name), then paste api_id."
                .into(),
        ),
        LoginStep::ApiHash { .. } => (
            "Telegram API hash".into(),
            "Now paste api_hash from the same page. Both stay on this computer.".into(),
        ),
        LoginStep::Phone => (
            "Phone number".into(),
            "Include the country code, e.g. +1 415 555 0123".into(),
        ),
        LoginStep::Code { sent_via } => (
            "Login code".into(),
            format!("Telegram sent a code {sent_via}."),
        ),
        LoginStep::Password { hint } if hint.is_empty() => (
            "Two-step verification password".into(),
            "Your account has a cloud password.".into(),
        ),
        LoginStep::Password { hint } => (
            "Two-step verification password".into(),
            format!("Hint: {hint}"),
        ),
        LoginStep::Email => (
            "Email address".into(),
            "Telegram needs an email address to send login codes to.".into(),
        ),
        LoginStep::EmailCode => (
            "Email code".into(),
            "Enter the code Telegram sent to your email.".into(),
        ),
        LoginStep::OtherDevice { link } => (
            "Confirm on another device".into(),
            format!("Open this link on a logged-in device: {link}"),
        ),
        LoginStep::Unsupported(why) => ("Can't log in here".into(), (*why).into()),
    };
    frame.render_widget(Line::from(title).bold(), prompt);
    frame.render_widget(
        Paragraph::new(text)
            .fg(colors.subtle)
            .wrap(Wrap { trim: true }),
        help,
    );

    if login.takes_input() {
        frame.render_widget(&login.input, input);
    }
    if let Some(message) = &login.error {
        frame.render_widget(Line::from(message.as_str()).fg(colors.error), error);
    }
    let hint = if login.busy {
        "Sending…"
    } else if login.takes_input() {
        "Enter submit · Ctrl-c quit"
    } else {
        "Ctrl-c quit"
    };
    frame.render_widget(Line::from(hint).fg(colors.muted).right_aligned(), footer);
}

fn draw_main(frame: &mut Frame, app: &mut App, colors: &Colors) {
    let [body, status] =
        Layout::vertical([Constraint::Fill(1), Constraint::Length(1)]).areas(frame.area());
    let [list_area, chat_area] =
        Layout::horizontal([Constraint::Percentage(35), Constraint::Fill(1)]).areas(body);

    draw_chat_list(frame, app, list_area, colors);
    match app.open.as_mut() {
        Some(open) => {
            let names = messages::Names {
                users: &app.users,
                chats: &app.chats,
            };
            let mut rows = app.composer.lines().len().clamp(1, MAX_COMPOSER_ROWS) as u16;
            if open.reply.is_some() {
                rows += REPLY_BAR_ROWS;
            }
            let [history, composer] =
                Layout::vertical([Constraint::Fill(1), Constraint::Length(rows + 2)])
                    .areas(chat_area);
            messages::draw(
                frame,
                history,
                open,
                &names,
                &mut app.images,
                app.focus == Focus::Messages,
                colors,
            );
            draw_composer(
                frame,
                &mut app.composer,
                open.reply.as_ref(),
                &names,
                composer,
                app.focus == Focus::Input,
                colors,
            );
        }
        None => frame.render_widget(
            Paragraph::new("Press Enter to open a chat")
                .fg(colors.muted)
                .centered()
                .block(Block::bordered().border_style(border(false, colors))),
            chat_area,
        ),
    }
    draw_status(frame, app, status, colors);
    if let Some(menu) = &app.menu {
        draw_menu(frame, chat_area, menu, colors);
    }
    if let Some(menu) = &app.delete_menu {
        draw_delete(frame, chat_area, menu, colors);
    }
    if let Some(menu) = &app.settings_menu {
        draw_settings(frame, menu, colors);
    }
}

/// The `d` popup over the message pane: the message, then how to delete it.
fn draw_delete(frame: &mut Frame, area: Rect, menu: &DeleteMenu, colors: &Colors) {
    let rows = menu.choices.len().max(1) as u16;
    let width = ((menu.snippet.width() + 6).clamp(40, 60) as u16).min(area.width);
    // Borders, the message and a gap, then the choices.
    let popup = center(area, width, rows + 4);
    let block = popup_block(" Delete message ", " Enter delete · Esc cancel ", colors);
    let inner = block.inner(popup);
    frame.render_widget(Clear, popup);
    frame.render_widget(block, popup);

    let [message, _, list] = Layout::vertical([
        Constraint::Length(1),
        Constraint::Length(1),
        Constraint::Fill(1),
    ])
    .areas(inner);
    let text_width = (inner.width as usize).saturating_sub(3);
    frame.render_widget(
        Line::from(vec![
            Span::from(" ▎ ").fg(colors.error),
            Span::from(truncate(&menu.snippet, text_width)),
        ]),
        message,
    );
    if menu.choices.is_empty() {
        frame.render_widget(Line::from(" Checking…").fg(colors.muted), list);
        return;
    }
    let items: Vec<ListItem> = menu
        .choices
        .iter()
        .enumerate()
        .map(|(i, choice)| {
            let bar = if i == menu.selected {
                Span::from("▌").fg(colors.accent)
            } else {
                Span::from(" ")
            };
            ListItem::new(Line::from(vec![
                bar,
                Span::from(format!("{} ", i + 1)).fg(colors.muted),
                Span::from(choice.label()).fg(colors.error),
            ]))
        })
        .collect();
    frame.render_stateful_widget(
        List::new(items).highlight_style(Style::new().bg(colors.selection)),
        list,
        &mut ListState::default().with_selected(Some(menu.selected)),
    );
}

/// The settings popup (`?`), centered on the screen. Only the theme for now.
fn draw_settings(frame: &mut Frame, menu: &SettingsMenu, colors: &Colors) {
    // Borders, the "Theme" heading, then one row per theme.
    let popup = center(frame.area(), 44, Theme::ALL.len() as u16 + 3);
    let block = popup_block(" Settings ", " Enter save · Esc cancel ", colors);
    let inner = block.inner(popup);
    frame.render_widget(Clear, popup);
    frame.render_widget(block, popup);

    let [heading, list] =
        Layout::vertical([Constraint::Length(1), Constraint::Fill(1)]).areas(inner);
    frame.render_widget(Line::from(" Theme").fg(colors.muted).bold(), heading);
    let items: Vec<ListItem> = Theme::ALL
        .iter()
        .enumerate()
        .map(|(i, &theme)| {
            let bar = if i == menu.selected {
                Span::from("▌").fg(colors.accent)
            } else {
                Span::from(" ")
            };
            // The dot marks the saved theme, which Esc goes back to.
            let mark = if theme == menu.saved {
                " ● "
            } else {
                " ○ "
            };
            ListItem::new(Line::from(vec![
                bar,
                Span::from(mark).fg(colors.accent),
                Span::from(theme.label()),
            ]))
        })
        .collect();
    frame.render_stateful_widget(
        List::new(items).highlight_style(Style::new().bg(colors.selection)),
        list,
        &mut ListState::default().with_selected(Some(menu.selected)),
    );
}

/// A popup's frame: accent border and its own background, so it stands out
/// from what's underneath. Callers draw `Clear` first.
fn popup_block<'a>(title: &'a str, keys: &'a str, colors: &Colors) -> Block<'a> {
    Block::bordered()
        .title(title)
        .title_bottom(Line::from(keys).fg(colors.muted).right_aligned())
        .border_style(Style::new().fg(colors.accent))
        .style(Style::new().bg(colors.popup_bg))
}

/// The "what to open" popup, centered over the message pane.
fn draw_menu(frame: &mut Frame, area: Rect, menu: &OpenMenu, colors: &Colors) {
    let longest = menu
        .targets
        .iter()
        .map(|t| t.label().width())
        .max()
        .unwrap_or(0);
    // Room for borders, the bar and the "1 " shortcut, within the pane.
    let width = (longest as u16 + 6)
        .min(area.width.saturating_sub(4))
        .max(36.min(area.width));
    let popup = center(area, width, menu.targets.len() as u16 + 2);
    let block = popup_block(" Open ", " Enter open · 1-9 pick · Esc close ", colors);
    let text_width = (block.inner(popup).width as usize).saturating_sub(3);

    let items: Vec<ListItem> = menu
        .targets
        .iter()
        .enumerate()
        .map(|(i, target)| {
            let bar = if i == menu.selected {
                Span::from("▌").fg(colors.accent)
            } else {
                Span::from(" ")
            };
            let shortcut = if i < 9 {
                format!("{} ", i + 1)
            } else {
                "  ".into()
            };
            ListItem::new(Line::from(vec![
                bar,
                Span::from(shortcut).fg(colors.muted),
                Span::from(truncate(target.label(), text_width)),
            ]))
        })
        .collect();
    let list = List::new(items)
        .block(block)
        .highlight_style(Style::new().bg(colors.selection));
    // Clear first: the popup must cover text and photos underneath.
    frame.render_widget(Clear, popup);
    frame.render_stateful_widget(
        list,
        popup,
        &mut ListState::default().with_selected(Some(menu.selected)),
    );
}

fn draw_chat_list(frame: &mut Frame, app: &App, area: Rect, colors: &Colors) {
    let filter = app.chats.filter();
    let mut title = if filter.is_empty() {
        format!(" Chats ({}) ", app.chats.ids().len())
    } else {
        format!(
            " Chats ({} of {}) /{filter} ",
            app.chats.ids().len(),
            app.chats.total()
        )
    };
    if app.chats_loading {
        title.push_str("loading… ");
    }
    let block = Block::bordered()
        .title(title)
        .border_style(border(app.focus == Focus::Chats, colors));
    // Inner width minus the 1-column cursor bar.
    let width = block.inner(area).width.saturating_sub(1) as usize;
    let selected = app
        .selected
        .and_then(|id| app.chats.ids().iter().position(|&x| x == id));

    let items: Vec<ListItem> = app
        .chats
        .ids()
        .iter()
        .filter_map(|&id| app.chats.get(id).map(|chat| (id, chat)))
        .enumerate()
        .map(|(i, (id, chat))| {
            let is_selected = selected == Some(i);
            // Drawn by hand: List's highlight symbol only marks an item's first row.
            let bar = if is_selected {
                Span::from("▌").fg(colors.accent)
            } else {
                Span::from(" ")
            };
            let badge = if chat.unread > 0 {
                format!(" {} ", chat.unread)
            } else {
                String::new()
            };
            let title = app.chats.title(id).unwrap_or_default();
            let title = truncate(title, width.saturating_sub(badge.width() + 1));
            let pad = width.saturating_sub(title.width() + badge.width());
            let style = title_style(&app.chats, id, colors).bold();
            let mut first = vec![bar.clone()];
            first.extend(highlight(&title, filter, style, colors));
            first.push(Span::from(" ".repeat(pad)));
            first.push(Span::from(badge).fg(colors.bg).bg(colors.primary));
            ListItem::new(vec![
                Line::from(first),
                Line::from(vec![
                    bar,
                    Span::from(truncate(&chat.preview, width)).fg(colors.subtle),
                ]),
            ])
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
    let list = List::new(items)
        .block(block)
        .highlight_style(Style::new().bg(colors.selection));
    frame.render_stateful_widget(
        list,
        area,
        &mut ListState::default().with_selected(selected),
    );
}

/// The box messages are written in. While replying, a bar at its top says
/// which message the reply answers.
fn draw_composer(
    frame: &mut Frame,
    composer: &mut TextArea<'static>,
    reply: Option<&Replied>,
    names: &messages::Names,
    area: Rect,
    insert: bool,
    colors: &Colors,
) {
    let block = Block::bordered().border_style(border(insert, colors));
    let mut text = block.inner(area);
    frame.render_widget(block, area);
    if let Some(reply) = reply {
        let [bar, rest] =
            Layout::vertical([Constraint::Length(REPLY_BAR_ROWS), Constraint::Fill(1)]).areas(text);
        draw_reply_bar(frame, reply, names, bar, colors);
        text = rest;
    }
    // The cursor only shows in Insert mode, so it's obvious where keys go.
    composer.set_cursor_style(if insert {
        Style::new().reversed()
    } else {
        Style::new()
    });
    composer.set_placeholder_text(match (insert, reply.is_some()) {
        (true, true) => "Write a reply…",
        (true, false) => "Write a message…",
        (false, true) => "Press i to write your reply",
        (false, false) => "Press i to write a message",
    });
    composer.set_placeholder_style(Style::new().fg(colors.muted));
    frame.render_widget(&*composer, text);
}

/// "Reply to Alice" over a line of the message, with a bar down the side
/// like a quote.
fn draw_reply_bar(
    frame: &mut Frame,
    reply: &Replied,
    names: &messages::Names,
    area: Rect,
    colors: &Colors,
) {
    let name = names.author(reply.sender, reply.outgoing);
    let width = (area.width as usize).saturating_sub(2);
    let label = "↩ Reply to ";
    let bar = || Span::from("▎ ").fg(colors.reply);
    let lines = vec![
        Line::from(vec![
            bar(),
            Span::from(label).fg(colors.reply),
            Span::from(truncate(&name, width.saturating_sub(label.width())))
                .fg(colors.reply)
                .bold(),
        ]),
        Line::from(vec![
            bar(),
            Span::from(truncate(&reply.snippet, width)).fg(colors.subtle),
        ]),
    ];
    frame.render_widget(Paragraph::new(lines), area);
}

/// The `/` prompt, vim style: `/query` at the bottom of the screen.
fn draw_prompt(frame: &mut Frame, app: &App, area: Rect, colors: &Colors) {
    let Some(prompt) = &app.prompt else {
        return;
    };
    let hints = match prompt.target {
        SearchTarget::Chats => "  Enter done · Esc cancel ",
        SearchTarget::Messages => "  Enter search · Esc cancel ",
    };
    let [mode, slash, input, keys] = Layout::horizontal([
        Constraint::Length(8),
        Constraint::Length(2),
        Constraint::Fill(1),
        Constraint::Length(hints.width() as u16),
    ])
    .areas(area);
    frame.render_widget(
        Span::from(" SEARCH ")
            .fg(colors.bg)
            .bg(colors.search)
            .bold(),
        mode,
    );
    frame.render_widget(Span::from(" /"), slash);
    frame.render_widget(&prompt.input, input);
    frame.render_widget(Line::from(hints).fg(colors.muted).right_aligned(), keys);
}

/// Hints for keys that depend on the cursor: `gd` on a reply, and Ctrl-o
/// after a `gd`.
fn jump_hints(open: &OpenChat) -> Vec<&'static str> {
    let mut hints = Vec::new();
    let on_reply = open
        .cursor_id()
        .and_then(|id| open.messages.get(&id))
        .is_some_and(|m| m.reply_to.is_some());
    if on_reply {
        hints.push("gd go to replied");
    }
    if !open.jumps.is_empty() {
        hints.push("Ctrl-o back to reply");
    }
    hints
}

fn draw_status(frame: &mut Frame, app: &App, area: Rect, colors: &Colors) {
    if app.prompt.is_some() {
        draw_prompt(frame, app, area, colors);
        return;
    }
    let normal = Span::from(" NORMAL ").fg(colors.bg).bg(colors.primary);
    let searching = app.open.as_ref().is_some_and(|o| o.search.is_some());
    let replying = app.open.as_ref().is_some_and(|o| o.reply.is_some());
    let insert = Span::from(" INSERT ").fg(colors.bg).bg(colors.insert);
    let (mode, hints) = match app.focus {
        _ if app.settings_menu.is_some() => {
            (normal, "  j/k preview theme · Enter save · Esc cancel")
        }
        _ if app.delete_menu.is_some() => (normal, "  j/k choose · Enter delete · Esc cancel"),
        Focus::Chats if !app.chats.filter().is_empty() => (
            normal,
            "  j/k move · Enter open · Esc clear search · / search again · i write · q quit",
        ),
        Focus::Chats => (
            normal,
            "  j/k move · Enter open · i write · / search · H highlight · gg/G top/bottom · Ctrl-d/u half page · ? settings · q quit",
        ),
        Focus::Messages if searching => (
            normal,
            "  n/N older/newer match · Esc end search · / search again · j/k newer/older · Enter open media · r reply · i write · h back",
        ),
        Focus::Messages if replying => (
            normal,
            "  i write reply · Esc cancel reply · r reply to selected instead · j/k newer/older · Enter open media · h back",
        ),
        Focus::Messages => (
            normal,
            "  j/k newer/older · r reply · d delete · Enter open media · i write · / search · gg/G oldest/newest · Ctrl-d/u half page · h back · ? settings · q quit",
        ),
        Focus::Input if replying => (
            insert,
            "  Enter send reply · Alt-Enter or Ctrl-j new line · Esc normal mode · Esc Esc cancel reply",
        ),
        Focus::Input => (
            insert,
            "  Enter send · Alt-Enter or Ctrl-j new line · Esc normal mode",
        ),
    };
    let context = match &app.open {
        Some(open)
            if app.focus == Focus::Messages
                && app.settings_menu.is_none()
                && app.delete_menu.is_none() =>
        {
            jump_hints(open)
        }
        _ => Vec::new(),
    };
    let mut spans = vec![mode.bold()];
    if context.is_empty() {
        spans.push(Span::from(hints).fg(colors.muted));
    } else {
        // Keys that only work right here go first, a bit brighter.
        spans.push(Span::from(format!("  {} · ", context.join(" · "))).fg(colors.fg));
        spans.push(Span::from(hints.trim_start()).fg(colors.muted));
    }
    if app.quit_deadline.is_some() {
        spans.push(Span::from("  Closing… (q again to force)").fg(colors.warning));
    } else if !app.opening.is_empty() {
        spans.push(Span::from("  Downloading… opens when done").fg(colors.warning));
    } else if let Some(message) = &app.status {
        spans.push(Span::from(format!("  {message}")).fg(colors.error));
    }
    frame.render_widget(Line::from(spans), area);
}

fn center(area: Rect, width: u16, height: u16) -> Rect {
    let [area] = Layout::horizontal([Constraint::Length(width)])
        .flex(Flex::Center)
        .areas(area);
    let [area] = Layout::vertical([Constraint::Length(height)])
        .flex(Flex::Center)
        .areas(area);
    area
}

/// Cuts `text` to at most `max` terminal columns, ending with `…` if cut.
pub(crate) fn truncate(text: &str, max: usize) -> String {
    if text.width() <= max {
        return text.to_string();
    }
    let mut out = String::new();
    let mut used = 0;
    for c in text.chars() {
        let w = unicode_width::UnicodeWidthChar::width(c).unwrap_or(0);
        if used + w + 1 > max {
            break;
        }
        out.push(c);
        used += w;
    }
    out.push('…');
    out
}

#[cfg(test)]
mod tests {
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;

    use super::*;
    use crate::app::Target;
    use crate::messages::MediaFile;

    #[test]
    fn open_menu_lists_targets_with_shortcuts() {
        let menu = OpenMenu {
            targets: vec![
                Target::File(MediaFile {
                    id: 1,
                    label: "Photo".into(),
                }),
                Target::Link("https://example.com/a".into()),
                Target::Link("https://docs.rs".into()),
            ],
            selected: 1,
        };
        let mut terminal = Terminal::new(TestBackend::new(70, 12)).unwrap();
        terminal
            .draw(|f| draw_menu(f, f.area(), &menu, &Theme::Mocha.colors()))
            .unwrap();
        let buf = terminal.backend().buffer();
        let rows: Vec<String> = (0..buf.area.height)
            .map(|y| (0..buf.area.width).map(|x| buf[(x, y)].symbol()).collect())
            .collect();

        assert!(rows.iter().any(|r| r.contains("Open")));
        assert!(rows.iter().any(|r| r.contains("1 Photo")));
        let selected = rows
            .iter()
            .find(|r| r.contains("2 https://example.com/a"))
            .unwrap();
        assert!(selected.contains("▌"), "cursor on the selected item");
        assert!(rows.iter().any(|r| r.contains("3 https://docs.rs")));
    }

    #[test]
    fn the_composer_says_which_message_a_reply_answers() {
        let colors = Theme::Mocha.colors();
        let users = std::collections::HashMap::from([(2, "Chardy".to_string())]);
        let chats = crate::chats::Chats::default();
        let names = messages::Names {
            users: &users,
            chats: &chats,
        };
        let reply = Replied {
            id: 7,
            sender: crate::messages::Sender::User(2),
            outgoing: false,
            snippet: "are we still on for a very long dinner tonight at the usual place".into(),
        };
        let mut composer = TextArea::default();
        let mut terminal = Terminal::new(TestBackend::new(40, 5)).unwrap();
        terminal
            .draw(|f| {
                let area = f.area();
                draw_composer(f, &mut composer, Some(&reply), &names, area, true, &colors);
            })
            .unwrap();
        let buf = terminal.backend().buffer();
        let rows: Vec<String> = (0..buf.area.height)
            .map(|y| (0..buf.area.width).map(|x| buf[(x, y)].symbol()).collect())
            .collect();

        assert!(rows[1].contains("▎ ↩ Reply to Chardy"), "{}", rows[1]);
        assert!(rows[2].contains("▎ are we still on"), "{}", rows[2]);
        assert!(rows[2].contains('…'), "a long message is cut to fit");
        assert!(rows[3].contains("Write a reply…"), "typing goes below it");
        let bar = (0..buf.area.width)
            .find(|&x| buf[(x, 1)].symbol() == "▎")
            .unwrap();
        assert_eq!(buf[(bar, 1)].fg, colors.reply);
    }

    #[test]
    fn deleting_shows_the_message_and_only_the_choices_telegram_allows() {
        use crate::tg::Deletable;
        let colors = Theme::Mocha.colors();
        let mut menu = DeleteMenu {
            message_id: 5,
            snippet: "see you at 7".into(),
            choices: Vec::new(),
            selected: 0,
        };
        let render = |menu: &DeleteMenu| -> Vec<String> {
            let mut terminal = Terminal::new(TestBackend::new(60, 12)).unwrap();
            terminal
                .draw(|f| draw_delete(f, f.area(), menu, &colors))
                .unwrap();
            let buf = terminal.backend().buffer();
            (0..buf.area.height)
                .map(|y| (0..buf.area.width).map(|x| buf[(x, y)].symbol()).collect())
                .collect()
        };
        let has = |rows: &[String], needle: &str| rows.iter().any(|r| r.contains(needle));

        let rows = render(&menu);
        assert!(has(&rows, "Delete message"));
        assert!(has(&rows, "▎ see you at 7"), "says which message");
        assert!(has(&rows, "Checking…"), "until TDLib answers");

        menu.set_allowed(Deletable {
            for_everyone: true,
            for_me: true,
        });
        let rows = render(&menu);
        assert!(
            has(&rows, "▌1 Delete for everyone"),
            "first, under the cursor"
        );
        assert!(has(&rows, " 2 Delete for me"));

        // Channels and groups only delete for everyone.
        menu.set_allowed(Deletable {
            for_everyone: true,
            for_me: false,
        });
        assert_eq!(menu.choices, [crate::app::DeleteChoice::Everyone]);
        assert!(!has(&render(&menu), "Delete for me"));
    }

    #[test]
    fn jump_keys_are_hinted_only_where_they_work() {
        use crate::messages::{Msg, ReplyTo, SendState, Sender};
        let msg = |reply_to| Msg {
            sender: Sender::User(2),
            outgoing: false,
            date: 0,
            text: "text".into(),
            preview: None,
            file: None,
            links: Vec::new(),
            link_ranges: Vec::new(),
            state: SendState::Sent,
            reply_to,
        };
        let mut open = OpenChat::new(1);
        open.messages.insert(1, msg(None));
        let to_first = ReplyTo {
            message_id: Some(1),
            quote: None,
        };
        open.messages.insert(2, msg(Some(to_first)));

        assert_eq!(jump_hints(&open), ["gd go to replied"], "newest is a reply");
        open.selected = Some(1);
        assert!(jump_hints(&open).is_empty());
        open.jumps.push(2);
        assert_eq!(jump_hints(&open), ["Ctrl-o back to reply"]);
    }

    #[test]
    fn first_run_asks_for_api_credentials_and_says_where_to_get_them() {
        let colors = Theme::Mocha.colors();
        let mut terminal = Terminal::new(TestBackend::new(80, 20)).unwrap();
        terminal
            .draw(|f| draw_login(f, &Login::new(LoginStep::ApiId), &colors))
            .unwrap();
        let buf = terminal.backend().buffer();
        let text: String = (0..buf.area.height)
            .flat_map(|y| (0..buf.area.width).map(move |x| (x, y)))
            .map(|(x, y)| buf[(x, y)].symbol())
            .collect();
        assert!(text.contains("Telegram API ID"));
        assert!(text.contains("my.telegram.org"), "says where to get it");
        assert!(text.contains("paste api_id"), "help text isn't cut off");
    }

    #[test]
    fn highlight_marks_every_match() {
        let colors = Theme::Mocha.colors();
        let spans = highlight("Alice & ALI", "ali", Style::new(), &colors);
        let lit: Vec<&str> = spans
            .iter()
            .filter(|s| s.style.bg == Some(colors.search))
            .map(|s| s.content.as_ref())
            .collect();
        assert_eq!(lit, ["Ali", "ALI"]);
        let all: String = spans.iter().map(|s| s.content.as_ref()).collect();
        assert_eq!(all, "Alice & ALI");
    }

    #[test]
    fn highlighted_chats_get_their_own_title_color() {
        let colors = Theme::Mocha.colors();
        let mut chats = crate::chats::Chats::default();
        chats.set_my_id(7);
        chats.set_highlighted(&[3, 7]);
        assert_eq!(title_style(&chats, 3, &colors).fg, Some(colors.highlighted));
        assert_eq!(
            title_style(&chats, 7, &colors).fg,
            Some(colors.highlighted),
            "a highlight beats the Saved Messages color"
        );
        chats.toggle_highlight(7);
        assert_eq!(title_style(&chats, 7, &colors).fg, Some(colors.primary));
        assert_eq!(title_style(&chats, 4, &colors).fg, None);
    }

    #[test]
    fn settings_list_every_theme_and_mark_the_saved_one() {
        // Previewing Latte while Mocha is saved.
        let menu = SettingsMenu {
            selected: 0,
            saved: Theme::Mocha,
        };
        let colors = Theme::Latte.colors();
        let mut terminal = Terminal::new(TestBackend::new(60, 12)).unwrap();
        terminal.draw(|f| draw_settings(f, &menu, &colors)).unwrap();
        let buf = terminal.backend().buffer();
        let rows: Vec<String> = (0..buf.area.height)
            .map(|y| (0..buf.area.width).map(|x| buf[(x, y)].symbol()).collect())
            .collect();
        let row = |needle: &str| rows.iter().find(|r| r.contains(needle)).unwrap();

        assert!(row("Settings").contains("Settings"));
        assert!(row("Theme").contains("Theme"));
        assert!(row("Catppuccin Latte").contains("▌ ○"), "cursor on Latte");
        assert!(row("Catppuccin Frappé").contains("○"));
        assert!(row("Catppuccin Macchiato").contains("○"));
        assert!(row("Catppuccin Mocha").contains("●"), "Mocha is saved");

        // The popup paints its own background, so it covers what's below.
        let y = rows.iter().position(|r| r.contains("Frappé")).unwrap() as u16;
        let x = (0..buf.area.width)
            .find(|&x| buf[(x, y)].symbol() == "C")
            .unwrap();
        assert_eq!(buf[(x, y)].bg, colors.popup_bg);
    }
}

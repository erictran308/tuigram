use ratatui::Frame;
use ratatui::layout::{Alignment, Constraint, Flex, Layout, Rect};
use ratatui::style::{Style, Stylize};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, BorderType, Clear, List, ListItem, ListState, Paragraph, Wrap};
use unicode_width::UnicodeWidthStr;

use ratatui_textarea::TextArea;

use crate::app::{
    App, Command, Confirm, DeleteMenu, Focus, HelpTab, Login, LoginStep, MenuAction, PickMenu,
    PromptKind, Screen, SettingsMenu, Target, Toast,
};
use crate::chats::Chat;
use crate::config;
use crate::messages::{OpenChat, Replied, Sender};
use crate::notify::Notifications;
use crate::search;
use crate::theme::{Colors, Theme};

/// The composer grows with its text up to this many rows, then scrolls.
const MAX_COMPOSER_ROWS: usize = 6;
/// The "Reply to …" bar at the top of the composer: who, then what they said.
const REPLY_BAR_ROWS: u16 = 2;

mod chat_list;
mod help;
mod messages;
mod qr;

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

/// What people are doing in a chat, in Telegram's words: "typing…" in a
/// one-on-one chat; "Alice is typing…", "Alice and Bob are typing…" or "3
/// people are typing…" in a group. When people are doing different things,
/// whoever started first decides which one shows.
fn activity(chat: &Chat, names: &messages::Names) -> Option<String> {
    let &(_, doing) = chat.activity.first()?;
    if chat.is_private {
        return Some(format!("{doing}…"));
    }
    let who: Vec<Sender> = chat
        .activity
        .iter()
        .filter(|&&(_, d)| d == doing)
        .map(|&(sender, _)| sender)
        .collect();
    Some(match who[..] {
        [one] => format!("{} is {doing}…", names.get(one)),
        [a, b] => format!("{} and {} are {doing}…", names.get(a), names.get(b)),
        _ => format!("{} people are {doing}…", who.len()),
    })
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
    if let LoginStep::OtherDevice { link } = &login.step
        && let Some(code) = qr::lines(link, colors)
    {
        draw_qr_login(frame, login, code, colors);
        return;
    }
    // A build from source points to the ready-made app, which needs no key.
    let source_build = config::built_in_keys().is_none();
    let help_rows = if matches!(login.step, LoginStep::ApiId) && source_build {
        4
    } else {
        3
    };
    let area = center(frame.area(), 64, 9 + help_rows);
    let block = Block::bordered()
        .title(" Log in ")
        .title_alignment(Alignment::Center)
        .border_style(Style::new().fg(colors.accent));
    let inner = block.inner(area);
    frame.render_widget(block, area);

    let [prompt, help, input, error, footer] = Layout::vertical([
        Constraint::Length(1),
        Constraint::Length(help_rows),
        Constraint::Length(3),
        Constraint::Length(1),
        Constraint::Length(1),
    ])
    .areas(inner.inner(ratatui::layout::Margin::new(1, 0)));

    let (title, text) = match &login.step {
        LoginStep::Connecting => ("Connecting…".to_string(), String::new()),
        LoginStep::LoggingOut => (
            "Logging out…".to_string(),
            "Then you can log in again, as yourself or someone else.".into(),
        ),
        LoginStep::ApiId if source_build => (
            "Telegram API ID".into(),
            "Builds from source have no API key. Get yours once at my.telegram.org → \
             API development tools (any app name), then paste api_id.\n\
             Or skip it all: cargo binstall tuigram-cli"
                .into(),
        ),
        LoginStep::ApiId => (
            "Telegram API ID".into(),
            "Get your own API ID and hash once at my.telegram.org → API development \
             tools (any app name), then paste api_id."
                .into(),
        ),
        LoginStep::ApiHash { .. } => (
            "Telegram API hash".into(),
            "Now paste api_hash from the same page. Both stay on this computer.".into(),
        ),
        LoginStep::Phone => (
            "Phone number".into(),
            "Include the country code, e.g. +1 415 555 0123. Or press Tab to log in \
             by scanning a QR code with Telegram on your phone."
                .into(),
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
    let hint = match login.step {
        _ if login.busy => "Sending…",
        LoginStep::Phone => "Enter submit · Tab QR code · Ctrl-c quit",
        LoginStep::OtherDevice { .. } => QR_KEYS,
        _ if login.takes_input() => "Enter submit · Ctrl-c quit",
        _ => "Ctrl-c quit",
    };
    frame.render_widget(Line::from(hint).fg(colors.muted).right_aligned(), footer);
}

const QR_KEYS: &str = "Esc use phone number · Ctrl-c quit";

/// A compact login box for the QR code, which is most of it: the code needs
/// about 20 rows, and this fits it in an 80×24 terminal.
fn draw_qr_login(frame: &mut Frame, login: &Login, code: Vec<Line<'static>>, colors: &Colors) {
    // Borders and one column of margin on each side; borders, help and keys.
    let width = (code[0].width() as u16 + 4).max(64);
    let height = code.len() as u16 + 5;
    let screen = frame.area();
    let fits = width <= screen.width && height <= screen.height;
    let area = if fits {
        center(screen, width, height)
    } else {
        center(screen, 64, 7)
    };
    let block = Block::bordered()
        .title(" Scan the QR code ")
        .title_alignment(Alignment::Center)
        .border_style(Style::new().fg(colors.accent));
    let inner = block.inner(area);
    frame.render_widget(block, area);

    let [help, body, footer] = Layout::vertical([
        Constraint::Length(2),
        Constraint::Fill(1),
        Constraint::Length(1),
    ])
    .areas(inner.inner(ratatui::layout::Margin::new(1, 0)));
    let text = if fits {
        "In Telegram on your phone: Settings → Devices → Link Desktop Device, then point \
         the camera at this code."
            .into()
    } else {
        format!("Make the terminal at least {width} columns by {height} rows to show the QR code.")
    };
    frame.render_widget(
        Paragraph::new(text)
            .fg(colors.subtle)
            .wrap(Wrap { trim: true }),
        help,
    );
    if fits {
        frame.render_widget(Paragraph::new(code).centered(), body);
    }
    // No room for an error line: an error takes the keys' place.
    let footer_line = match &login.error {
        Some(message) => Line::from(message.as_str()).fg(colors.error),
        None => Line::from(QR_KEYS).fg(colors.muted).right_aligned(),
    };
    frame.render_widget(footer_line, footer);
}

fn draw_main(frame: &mut Frame, app: &mut App, colors: &Colors) {
    let [body, status] =
        Layout::vertical([Constraint::Fill(1), Constraint::Length(1)]).areas(frame.area());
    let [list_area, chat_area] =
        Layout::horizontal([Constraint::Percentage(35), Constraint::Fill(1)]).areas(body);

    let list = chat_list::ChatList {
        chats: &app.chats,
        users: &app.users,
        selected: app.selected,
        loading: app.chats_loading,
        focused: app.focus == Focus::Chats,
        // The settings popup, the command list and toasts can reach over it.
        covered: app.settings_menu.is_some()
            || app.toast.is_some()
            || app
                .prompt
                .as_ref()
                .is_some_and(|p| p.kind == PromptKind::Command),
    };
    chat_list::draw(frame, list_area, &list, &mut app.images, colors);
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
    if let Some(prompt) = app
        .prompt
        .as_ref()
        .filter(|p| p.kind == PromptKind::Command)
    {
        draw_commands(frame, body, &prompt.query(), colors);
    }
    if let Some(menu) = &app.menu {
        draw_menu(frame, chat_area, menu, colors);
    }
    if let Some(menu) = &app.delete_menu {
        draw_delete(frame, chat_area, menu, colors);
    }
    if let Some(confirm) = &app.confirm {
        draw_confirm(frame, chat_area, confirm, colors);
    }
    if let Some(menu) = &mut app.settings_menu {
        draw_settings(frame, menu, app.settings.notifications, colors);
    }
    if let Some(toast) = &app.toast {
        draw_toast(frame, toast, colors);
    }
}

/// The toast: bottom right, just above the status bar.
fn draw_toast(frame: &mut Frame, toast: &Toast, colors: &Colors) {
    let area = frame.area();
    let rows = if toast.detail.is_empty() { 1 } else { 2 };
    let text = toast.title.width().max(toast.detail.width()) as u16;
    let width = (text + 6).clamp(24, 50).min(area.width);
    let height = (rows + 2).min(area.height.saturating_sub(1));
    let rect = Rect {
        x: area.right().saturating_sub(width + 1).max(area.x),
        y: area.bottom().saturating_sub(height + 1),
        width,
        height,
    };
    let block = Block::bordered()
        .border_type(BorderType::Rounded)
        .border_style(Style::new().fg(colors.success))
        .style(Style::new().bg(colors.popup_bg));
    let inner_width = (block.inner(rect).width as usize).saturating_sub(3);
    let mut lines = vec![Line::from(vec![
        Span::from(" ✓ ").fg(colors.success).bold(),
        Span::from(toast.title.clone()).bold(),
    ])];
    if !toast.detail.is_empty() {
        lines.push(
            Line::from(format!("   {}", truncate(&toast.detail, inner_width))).fg(colors.subtle),
        );
    }
    frame.render_widget(Clear, rect);
    frame.render_widget(Paragraph::new(lines).block(block), rect);
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

/// The `?` popup, centered on the screen: a tab with every shortcut, and one
/// with the settings (only the theme for now).
fn draw_settings(
    frame: &mut Frame,
    menu: &mut SettingsMenu,
    notifications: Notifications,
    colors: &Colors,
) {
    let area = frame.area();
    // Both tabs get the same size, so the tabs don't move when switching.
    let width = 72.min(area.width.saturating_sub(2));
    let height = (help::height() as u16 + 2).min(area.height.saturating_sub(2));
    let popup = center(area, width, height);
    let tab = |label: &'static str, active: bool| {
        if active {
            Span::from(label).fg(colors.bg).bg(colors.accent).bold()
        } else {
            Span::from(label).fg(colors.muted)
        }
    };
    let tabs = Line::from(vec![
        Span::from(" "),
        tab(" Shortcuts ", menu.tab == HelpTab::Shortcuts),
        Span::from(" "),
        tab(" Settings ", menu.tab == HelpTab::Settings),
        Span::from(" "),
    ]);
    let keys = match menu.tab {
        HelpTab::Shortcuts => " j/k scroll · Tab settings · Esc close ",
        HelpTab::Settings if menu.selected == SettingsMenu::NOTIFICATIONS => {
            " Space on/off · Enter save · Tab shortcuts · Esc cancel "
        }
        HelpTab::Settings => " j/k preview · Enter save · Tab shortcuts · Esc cancel ",
    };
    let block = popup_block(tabs, keys, colors);
    let inner = block.inner(popup);
    frame.render_widget(Clear, popup);
    frame.render_widget(block, popup);
    if menu.tab == HelpTab::Shortcuts {
        help::draw(frame, inner, &mut menu.scroll, colors);
        return;
    }

    let row = |i: usize, mark: &'static str, label: &'static str| {
        let selected = i == menu.selected;
        let bar = if selected {
            Span::from("▌").fg(colors.accent)
        } else {
            Span::from(" ")
        };
        let line = Line::from(vec![bar, Span::from(mark).fg(colors.accent), label.into()]);
        if selected {
            line.style(Style::new().bg(colors.selection))
        } else {
            line
        }
    };
    let heading = |text: &'static str| Line::from(text).fg(colors.muted).bold();
    let mut lines = vec![heading(" Theme")];
    for (i, &theme) in Theme::ALL.iter().enumerate() {
        // The dot marks the saved theme, which Esc goes back to.
        let mark = if theme == menu.saved {
            " ● "
        } else {
            " ○ "
        };
        lines.push(row(i, mark, theme.label()));
    }
    lines.push(Line::default());
    lines.push(heading(" Notifications"));
    let check = if notifications == Notifications::Off {
        " [ ] "
    } else {
        " [✓] "
    };
    lines.push(row(
        SettingsMenu::NOTIFICATIONS,
        check,
        "New messages, while tuigram is in the background",
    ));
    for (line, y) in lines.into_iter().zip(inner.y..inner.bottom()) {
        frame.render_widget(
            line,
            Rect {
                y,
                height: 1,
                ..inner
            },
        );
    }
}

/// A popup's frame: accent border and its own background, so it stands out
/// from what's underneath. Callers draw `Clear` first.
/// The "are you sure" popup over the message pane, for a file that could
/// run code or a link that hides where it goes.
fn draw_confirm(frame: &mut Frame, area: Rect, confirm: &Confirm, colors: &Colors) {
    let title = format!(" {} ", confirm.title);
    let longest = confirm.lines.iter().map(|l| l.width()).max().unwrap_or(0);
    let width = (longest.max(title.width()) as u16 + 4)
        .min(area.width)
        .max(40.min(area.width));
    let popup = center(area, width, confirm.lines.len() as u16 + 2);
    let block = popup_block(title, " y open · Esc cancel ", colors)
        .border_style(Style::new().fg(colors.warning));
    // Long URLs keep their start, where the site's name is.
    let room = (block.inner(popup).width as usize).saturating_sub(2);
    let lines: Vec<Line> = confirm
        .lines
        .iter()
        .map(|line| Line::from(format!(" {}", truncate(line, room))))
        .collect();
    frame.render_widget(Clear, popup);
    frame.render_widget(Paragraph::new(lines).block(block), popup);
}

fn popup_block<'a>(title: impl Into<Line<'a>>, keys: &'a str, colors: &Colors) -> Block<'a> {
    Block::bordered()
        .title(title)
        .title_bottom(Line::from(keys).fg(colors.muted).right_aligned())
        .border_style(Style::new().fg(colors.accent))
        .style(Style::new().bg(colors.popup_bg))
}

/// The "what to open" or "what to copy" popup, centered over the message pane.
fn draw_menu(frame: &mut Frame, area: Rect, menu: &PickMenu, colors: &Colors) {
    let longest = menu
        .targets
        .iter()
        .map(|t| match t {
            // The label, then how the message starts.
            Target::Text(text) => t.label().width() + 2 + one_line(text).width(),
            _ => t.label().width(),
        })
        .max()
        .unwrap_or(0);
    // Room for borders, the bar and the "1 " shortcut, within the pane.
    let width = (longest as u16 + 6)
        .min(area.width.saturating_sub(4))
        .max(36.min(area.width));
    let popup = center(area, width, menu.targets.len() as u16 + 2);
    let block = match menu.action {
        MenuAction::Open => popup_block(" Open ", " Enter open · 1-9 pick · Esc close ", colors),
        MenuAction::Copy => popup_block(" Copy ", " Enter copy · 1-9 pick · Esc close ", colors),
    };
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
            let label = target.label();
            let mut line = vec![
                bar,
                Span::from(shortcut).fg(colors.muted),
                Span::from(truncate(label, text_width)),
            ];
            // The whole message is easier to recognize by how it starts.
            if let Target::Text(text) = target {
                let room = text_width.saturating_sub(label.width() + 2);
                line.push(
                    Span::from(format!("  {}", truncate(&one_line(text), room))).fg(colors.subtle),
                );
            }
            ListItem::new(Line::from(line))
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
    let (label, color, prefix, hints) = match prompt.kind {
        PromptKind::Chats => (
            " SEARCH ",
            colors.search,
            " /",
            "  Enter done · Esc cancel ",
        ),
        PromptKind::Messages => (
            " SEARCH ",
            colors.search,
            " /",
            "  Enter search · Esc cancel ",
        ),
        PromptKind::Command => (
            " COMMAND ",
            colors.command,
            " :",
            "  Enter run · Esc cancel ",
        ),
    };
    let [mode, slash, input, keys] = Layout::horizontal([
        Constraint::Length(label.width() as u16),
        Constraint::Length(2),
        Constraint::Fill(1),
        Constraint::Length(hints.width() as u16),
    ])
    .areas(area);
    frame.render_widget(Span::from(label).fg(colors.bg).bg(color).bold(), mode);
    frame.render_widget(Span::from(prefix), slash);
    frame.render_widget(&prompt.input, input);
    frame.render_widget(Line::from(hints).fg(colors.muted).right_aligned(), keys);
}

/// Every `:` command, over the bottom left corner while typing one. Names
/// starting with what's typed so far stand out.
fn draw_commands(frame: &mut Frame, area: Rect, typed: &str, colors: &Colors) {
    let typed = typed.trim();
    let name_width = Command::ALL
        .iter()
        .map(|c| c.name().len())
        .max()
        .unwrap_or(0);
    let lines: Vec<Line> = Command::ALL
        .iter()
        .map(|command| {
            let name = command.name();
            let padding = " ".repeat(name_width - name.len());
            if !name.starts_with(typed) {
                return Line::from(format!(" {name}{padding}  {}", command.about()))
                    .fg(colors.muted);
            }
            let mut spans = vec![Span::from(" ")];
            if !typed.is_empty() {
                spans.push(Span::styled(typed.to_string(), match_style(colors)));
            }
            spans.push(Span::from(format!("{}{padding}", &name[typed.len()..])).fg(colors.primary));
            spans.push(Span::from(format!("  {}", command.about())).fg(colors.subtle));
            Line::from(spans)
        })
        .collect();
    let title = " Commands · type the full name ";
    let longest = lines
        .iter()
        .map(Line::width)
        .max()
        .unwrap_or(0)
        .max(title.width());
    let width = (longest as u16 + 3).min(area.width);
    let height = (lines.len() as u16 + 2).min(area.height);
    let popup = Rect {
        x: area.x,
        y: area.bottom().saturating_sub(height),
        width,
        height,
    };
    let block = Block::bordered()
        .title(title)
        .border_style(Style::new().fg(colors.accent))
        .style(Style::new().bg(colors.popup_bg));
    // Clear first: the popup must cover text and photos underneath.
    frame.render_widget(Clear, popup);
    frame.render_widget(Paragraph::new(lines).block(block), popup);
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
        _ if app.confirm.is_some() => (normal, "  y open · n or Esc cancel"),
        _ if app.settings_menu.as_ref().map(|m| m.tab) == Some(HelpTab::Shortcuts) => {
            (normal, "  j/k scroll · Tab settings · Esc close")
        }
        _ if app.settings_menu.is_some() => (
            normal,
            "  j/k preview theme · Enter save · Tab shortcuts · Esc cancel",
        ),
        _ if app.delete_menu.is_some() => (normal, "  j/k choose · Enter delete · Esc cancel"),
        Focus::Chats if !app.chats.filter().is_empty() => (
            normal,
            "  j/k move · Enter open · Esc clear search · / search again · i write · q quit",
        ),
        Focus::Chats => (
            normal,
            "  j/k move · Enter open · i write · / search · H highlight · gg/G top/bottom · Ctrl-d/u half page · : commands · ? help · q quit",
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
            "  j/k newer/older · y copy · r reply · d delete · Enter open media · i write · / search · gg/G oldest/newest · h back · : commands · ? help · q quit",
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
    } else if !app.copying.is_empty() {
        spans.push(Span::from("  Downloading… copies when done").fg(colors.warning));
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

fn one_line(text: &str) -> String {
    text.split_whitespace().collect::<Vec<_>>().join(" ")
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
    // Some sequences are wider than their characters add up to ("❤️" is a
    // 1-wide heart and a 0-wide selector, shown 2 wide), so check the result.
    while out.width() + 1 > max && out.pop().is_some() {}
    out.push('…');
    out
}

#[cfg(test)]
mod tests {
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;

    use super::*;
    use crate::messages::MediaFile;

    /// Every row of the buffer as a string.
    fn buffer_rows(buf: &ratatui::buffer::Buffer) -> Vec<String> {
        (0..buf.area.height)
            .map(|y| (0..buf.area.width).map(|x| buf[(x, y)].symbol()).collect())
            .collect()
    }

    #[test]
    fn help_opens_on_the_shortcuts_and_scrolls_to_the_end() {
        let colors = Theme::Mocha.colors();
        let mut menu = SettingsMenu {
            tab: HelpTab::Shortcuts,
            scroll: 0,
            selected: 3,
            saved: Theme::Mocha,
            saved_notifications: Notifications::Auto,
        };
        // Too short for the whole list.
        let mut terminal = Terminal::new(TestBackend::new(80, 20)).unwrap();
        let mut draw = |menu: &mut SettingsMenu| {
            terminal
                .draw(|f| draw_settings(f, menu, Notifications::Auto, &colors))
                .unwrap();
            buffer_rows(terminal.backend().buffer())
        };
        let has = |rows: &[String], needle: &str| rows.iter().any(|r| r.contains(needle));

        let rows = draw(&mut menu);
        assert!(
            has(&rows, "Shortcuts") && has(&rows, "Settings"),
            "both tabs"
        );
        assert!(has(&rows, "Everywhere"));
        assert!(!has(&rows, "Menus and popups"), "further down");

        // G scrolls past the end; drawing stops it at the last row.
        menu.scroll = usize::MAX;
        let rows = draw(&mut menu);
        assert!(has(&rows, "Switch tabs in this popup"), "the last shortcut");
        assert_eq!(
            menu.scroll,
            help::height() - 16,
            "the popup is 2 rows shorter than the screen, then borders"
        );

        menu.tab = HelpTab::Settings;
        let rows = draw(&mut menu);
        assert!(has(&rows, "Catppuccin Mocha") && !has(&rows, "Everywhere"));
    }

    #[test]
    fn a_toast_says_what_was_copied_in_the_corner() {
        let colors = Theme::Mocha.colors();
        let toast = Toast {
            title: "Copied".into(),
            detail: "https://example.com/a".into(),
            until: tokio::time::Instant::now(),
        };
        let mut terminal = Terminal::new(TestBackend::new(80, 20)).unwrap();
        terminal.draw(|f| draw_toast(f, &toast, &colors)).unwrap();
        let buf = terminal.backend().buffer();
        let rows = buffer_rows(buf);

        // Bottom right, with the status bar row (19) left free.
        assert!(rows[16].contains("✓ Copied"), "{}", rows[16]);
        assert!(rows[17].contains("https://example.com/a"));
        assert!(rows[18].trim_end().ends_with('╯'), "rounded corner");
        assert!(rows[19].trim().is_empty());
        assert_eq!(buf[(78, 18)].fg, colors.success);
    }

    #[test]
    fn the_copy_menu_offers_the_text_then_links_then_media() {
        let menu = PickMenu {
            action: MenuAction::Copy,
            targets: vec![
                Target::Text("look at\nthis https://x.dev".into()),
                Target::Link("https://x.dev".into()),
                Target::File(MediaFile {
                    id: 1,
                    label: "Photo".into(),
                    photo: true,
                }),
            ],
            selected: 0,
        };
        let mut terminal = Terminal::new(TestBackend::new(70, 12)).unwrap();
        terminal
            .draw(|f| draw_menu(f, f.area(), &menu, &Theme::Mocha.colors()))
            .unwrap();
        let rows = buffer_rows(terminal.backend().buffer());
        let has = |needle: &str| rows.iter().any(|r| r.contains(needle));

        assert!(has("Copy") && has("Enter copy"));
        assert!(
            has("1 Whole message  look at this https://x.dev"),
            "on one line"
        );
        assert!(has("2 https://x.dev"));
        assert!(has("3 Photo"));
    }

    #[test]
    fn open_menu_lists_targets_with_shortcuts() {
        let menu = PickMenu {
            action: MenuAction::Open,
            targets: vec![
                Target::File(MediaFile {
                    id: 1,
                    label: "Photo".into(),
                    photo: true,
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
        assert!(has(&rows, " 1 Delete for everyone"), "first");
        assert!(
            has(&rows, "▌2 Delete for me"),
            "under the cursor, the safer choice"
        );

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
            source_text: "text".into(),
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
        assert!(text.contains("paste api_id"));
        assert!(
            buffer_rows(buf)
                .iter()
                .any(|r| r.contains("Or skip it all: cargo binstall tuigram-cli")),
            "offers the app with a key, on one line; help text isn't cut off"
        );
    }

    fn draw_login_rows(step: LoginStep, width: u16, height: u16) -> Vec<String> {
        let colors = Theme::Mocha.colors();
        let mut terminal = Terminal::new(TestBackend::new(width, height)).unwrap();
        terminal
            .draw(|f| draw_login(f, &Login::new(step), &colors))
            .unwrap();
        buffer_rows(terminal.backend().buffer())
    }

    const LOGIN_LINK: &str = "tg://login?token=AQIDBAUGBwgJCgsMDQ4PEBESExQVFhcYGRobHB0eHyA";

    #[test]
    fn the_phone_step_offers_qr_login_on_tab() {
        let rows = draw_login_rows(LoginStep::Phone, 80, 24).concat();
        assert!(
            rows.contains("scanning a QR code"),
            "help text isn't cut off"
        );
        assert!(rows.contains("Tab QR code"));
    }

    #[test]
    fn qr_login_fits_the_code_and_where_to_scan_it_in_80_by_24() {
        let colors = Theme::Mocha.colors();
        let link = LOGIN_LINK.to_string();
        let mut terminal = Terminal::new(TestBackend::new(80, 24)).unwrap();
        terminal
            .draw(|f| draw_login(f, &Login::new(LoginStep::OtherDevice { link }), &colors))
            .unwrap();
        let buf = terminal.backend().buffer();
        let rows = buffer_rows(buf);
        let row = |needle: &str| rows.iter().any(|r| r.contains(needle));
        assert!(row("Link Desktop"));
        assert!(
            row("point the camera at this code"),
            "help text isn't cut off"
        );
        assert!(row("Esc use phone number"));

        let code = qr::lines(LOGIN_LINK, &colors).unwrap();
        let drawn: Vec<u16> = (0..buf.area.height)
            .filter(|&y| rows[y as usize].contains(['█', '▀', '▄']))
            .collect();
        // The first and last rows are all margin.
        assert_eq!(drawn.len(), code.len() - 2, "no row is cut off");
        let light = (0..buf.area.width)
            .filter(|&x| buf[(x, drawn[0])].bg == colors.qr_light)
            .count();
        assert_eq!(
            light,
            code[0].width(),
            "only the code and its margin are light"
        );
    }

    #[test]
    fn a_terminal_too_small_for_the_qr_code_says_how_big_it_needs_to_be() {
        let link = LOGIN_LINK.to_string();
        let rows = draw_login_rows(LoginStep::OtherDevice { link }, 80, 20);
        let text = rows.concat();
        assert!(!text.contains('█'), "no cut-off code");
        assert!(text.contains("Make the terminal at least"));
        assert!(text.contains("QR code."), "help text isn't cut off");
    }

    /// The column where `needle` starts in a buffer row; every cell there is
    /// one character.
    fn column(row: &str, needle: &str) -> u16 {
        row[..row.find(needle).unwrap()].chars().count() as u16
    }

    #[test]
    fn commands_run_only_by_their_full_name() {
        assert_eq!(Command::parse("logout"), Some(Command::Logout));
        for typo in ["", "l", "log", "logou", "logout!", "Logout", " logout"] {
            assert_eq!(Command::parse(typo), None, "{typo:?}");
        }
    }

    #[test]
    fn the_command_list_shows_every_command_and_marks_what_is_typed() {
        let colors = Theme::Mocha.colors();
        let mut terminal = Terminal::new(TestBackend::new(80, 10)).unwrap();
        terminal
            .draw(|f| draw_commands(f, f.area(), "lo", &colors))
            .unwrap();
        let buf = terminal.backend().buffer();
        let rows = buffer_rows(buf);
        assert!(rows.iter().any(|r| r.contains("type the full name")));
        for command in Command::ALL {
            let y = rows
                .iter()
                .position(|r| r.contains(command.about()))
                .unwrap();
            assert!(
                rows[y].contains(command.name()),
                "name and about on one row"
            );
        }
        let y = rows.iter().position(|r| r.contains("logout")).unwrap() as u16;
        let x = column(&rows[y as usize], "logout");
        assert_eq!(buf[(x, y)].bg, colors.search, "typed part marked");
        assert_eq!(buf[(x + 2, y)].fg, colors.primary, "rest of the name not");

        terminal
            .draw(|f| draw_commands(f, f.area(), "x", &colors))
            .unwrap();
        let buf = terminal.backend().buffer();
        let rows = buffer_rows(buf);
        let y = rows.iter().position(|r| r.contains("logout")).unwrap() as u16;
        let x = column(&rows[y as usize], "logout");
        assert_eq!(buf[(x, y)].fg, colors.muted, "still listed, dimmed");
    }

    #[test]
    fn truncate_never_goes_over_with_emoji_sequences() {
        let hearts = "❤️".repeat(100);
        for max in [1, 2, 3, 10, 51] {
            let cut = truncate(&hearts, max);
            assert!(cut.width() <= max, "{max}: {cut:?} is {}", cut.width());
        }
        assert_eq!(truncate("short", 10), "short");
        assert_eq!(truncate("abcdef", 4), "abc…");
    }

    #[test]
    fn a_disguised_link_asks_first_and_shows_where_it_goes() {
        let colors = Theme::Mocha.colors();
        let confirm = Confirm {
            title: "Open this link?".into(),
            lines: vec![
                "The text says: bank.com".into(),
                "It goes to:    https://evil.example/login".into(),
            ],
            action: crate::app::Confirmed::OpenLink("https://evil.example/login".into()),
        };
        let mut terminal = Terminal::new(TestBackend::new(80, 12)).unwrap();
        terminal
            .draw(|f| draw_confirm(f, f.area(), &confirm, &colors))
            .unwrap();
        let buf = terminal.backend().buffer();
        let rows = buffer_rows(buf);
        let row = |needle: &str| rows.iter().position(|r| r.contains(needle));
        assert!(row("Open this link?").is_some());
        assert!(row("The text says: bank.com").is_some());
        assert!(row("It goes to:    https://evil.example/login").is_some());
        let keys = row("y open · Esc cancel").expect("says how to answer");
        let x = column(&rows[keys], "y open");
        assert_eq!(
            buf[(x - 2, keys as u16)].fg,
            colors.warning,
            "warning border"
        );
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
        let mut menu = SettingsMenu {
            tab: HelpTab::Settings,
            scroll: 0,
            selected: 0,
            saved: Theme::Mocha,
            saved_notifications: Notifications::Auto,
        };
        let colors = Theme::Latte.colors();
        let mut terminal = Terminal::new(TestBackend::new(60, 12)).unwrap();
        terminal
            .draw(|f| draw_settings(f, &mut menu, Notifications::Auto, &colors))
            .unwrap();
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

    #[test]
    fn notifications_show_as_a_checkbox_under_the_themes() {
        let colors = Theme::Mocha.colors();
        let mut menu = SettingsMenu {
            tab: HelpTab::Settings,
            scroll: 0,
            selected: SettingsMenu::NOTIFICATIONS,
            saved: Theme::Mocha,
            saved_notifications: Notifications::Auto,
        };
        let mut terminal = Terminal::new(TestBackend::new(70, 14)).unwrap();
        let mut draw = |notifications| {
            terminal
                .draw(|f| draw_settings(f, &mut menu, notifications, &colors))
                .unwrap();
            buffer_rows(terminal.backend().buffer())
        };
        let row = |rows: &[String], needle: &str| {
            rows.iter().find(|r| r.contains(needle)).unwrap().clone()
        };

        let rows = draw(Notifications::Auto);
        assert!(row(&rows, "Notifications").contains("Notifications"));
        assert!(row(&rows, "New messages").contains("▌ [✓] New messages"));
        assert!(
            row(&rows, "Space on/off").contains("Esc cancel"),
            "key hints"
        );

        let rows = draw(Notifications::Off);
        assert!(row(&rows, "New messages").contains("▌ [ ] New messages"));
    }
}

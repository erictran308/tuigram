use ratatui::Frame;
use ratatui::layout::{Alignment, Constraint, Flex, Layout, Rect};
use ratatui::style::{Style, Stylize};
use ratatui::text::{Line, Span};
use ratatui::widgets::{
    Block, BorderType, Clear, List, ListItem, ListState, Padding, Paragraph, Scrollbar,
    ScrollbarOrientation, ScrollbarState, Wrap,
};
use unicode_width::UnicodeWidthStr;

use ratatui_textarea::TextArea;

use crate::app::{
    App, Command, Confirm, Confirmed, DeleteMenu, Focus, HelpTab, Login, LoginStep, MenuAction,
    PickMenu, PromptKind, Screen, SettingsMenu, Target, Toast,
};
use crate::attach::{self, Attachment, Kind};
use crate::chats::{Badge, Chat};
use crate::complete::Completion;
use crate::config;
use crate::images::Images;
use crate::messages::{Editing, OpenChat, Replied, Sender};
use crate::notify::Notifications;
use crate::picker::{ChatPicker, Choice, Purpose};
use crate::poll::VoteMenu;
use crate::reactions::{self, ReactMenu};
use crate::search;
use crate::settings::{Settings, Side};
use crate::text;
use crate::theme::{Colors, Themes};

/// The composer grows with its text up to this many rows, then scrolls.
const MAX_COMPOSER_ROWS: usize = 6;
/// The bar at the top of the composer while replying or editing: what it's
/// about, then a line of the message.
const BAR_ROWS: u16 = 2;
/// Files waiting to be sent get a row each in the composer, up to this
/// many; the last row then counts the rest.
const MAX_ATTACHMENT_ROWS: usize = 3;

mod chat_list;
mod help;
mod messages;
mod qr;
mod stickers;

pub fn draw(frame: &mut Frame, app: &mut App) {
    let colors = app.colors;
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

/// Key hints with the keys in backticks, like "`Enter` send · `Esc` cancel":
/// the keys stand out in the accent color, and what they do is in `text`.
fn hint_spans(hints: &str, text: Style, colors: &Colors) -> Vec<Span<'static>> {
    let key = text.fg(colors.accent).bold();
    hints
        .split('`')
        .enumerate()
        .filter(|(_, part)| !part.is_empty())
        .map(|(i, part)| Span::styled(part.to_string(), if i % 2 == 1 { key } else { text }))
        .collect()
}

/// A popup's own background, and the theme's text color: `Clear` resets
/// both to the terminal's, which a light theme on a dark terminal (or the
/// other way round) can barely be read on.
fn popup_style(colors: &Colors) -> Style {
    Style::new().fg(colors.fg).bg(colors.popup_bg)
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
        LoginStep::Phone => "`Enter` submit · `Tab` QR code · `Ctrl-c` quit",
        LoginStep::OtherDevice { .. } => QR_KEYS,
        _ if login.takes_input() => "`Enter` submit · `Ctrl-c` quit",
        _ => "`Ctrl-c` quit",
    };
    let muted = Style::new().fg(colors.muted);
    let hint = Line::from(hint_spans(hint, muted, colors)).right_aligned();
    frame.render_widget(hint, footer);
}

const QR_KEYS: &str = "`Esc` use phone number · `Ctrl-c` quit";

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
        None => {
            Line::from(hint_spans(QR_KEYS, Style::new().fg(colors.muted), colors)).right_aligned()
        }
    };
    frame.render_widget(footer_line, footer);
}

fn draw_main(frame: &mut Frame, app: &mut App, colors: &Colors) {
    let [body, status] =
        Layout::vertical([Constraint::Fill(1), Constraint::Length(1)]).areas(frame.area());
    let list_width = Constraint::Percentage(app.settings.list_width());
    let [list_area, chat_area] = match app.settings.chat_list_side {
        Side::Left => Layout::horizontal([list_width, Constraint::Fill(1)]).areas(body),
        Side::Right => {
            let [chat, list] = Layout::horizontal([Constraint::Fill(1), list_width]).areas(body);
            [list, chat]
        }
    };

    let list = chat_list::ChatList {
        chats: &app.chats,
        users: &app.users,
        selected: app.selected,
        loading: app.chats_loading,
        focused: app.focus == Focus::Chats,
        // The settings popup, the command list and toasts can reach over it;
        // a toast only when the list is on the right, where toasts go.
        covered: app.settings_menu.is_some()
            || (app.toast.is_some() && app.settings.chat_list_side == Side::Right)
            || app
                .prompt
                .as_ref()
                .is_some_and(|p| p.kind == PromptKind::Command || !p.completions.is_empty()),
        gaps: app.settings.chat_gaps,
    };
    chat_list::draw(frame, list_area, &list, &mut app.images, colors);
    // Popups drawn over the messages, below. Not the toast, which only says
    // what just happened: holding photos back under it would blank them all
    // on every copy.
    let suggesting =
        app.focus == Focus::Input && app.completion.as_ref().is_some_and(|c| !c.items.is_empty());
    let popup_over_chat = suggesting
        || app.menu.is_some()
        || app.delete_menu.is_some()
        || app.react_menu.is_some()
        || app.vote_menu.is_some()
        || app.picker.is_some()
        || app.confirm.is_some()
        || app.settings_menu.is_some()
        || app
            .prompt
            .as_ref()
            .is_some_and(|p| p.kind == PromptKind::Command || !p.completions.is_empty());
    // Where the composer is, for the suggestions over it.
    let mut suggestions_at = None;
    match app.open.as_mut() {
        Some(open) => {
            let names = messages::Names {
                users: &app.users,
                chats: &app.chats,
            };
            let mut rows = app.composer.lines().len().clamp(1, MAX_COMPOSER_ROWS) as u16;
            if ComposerBar::of(open).is_some() {
                rows += BAR_ROWS;
            }
            rows += preview_rows(&open.attachments, &app.images);
            rows += attachment_rows(&open.attachments);
            let panel = app.stickers.as_mut().filter(|_| app.focus == Focus::Input);
            let panel_rows = match panel {
                Some(_) => stickers::height(chat_area.height.saturating_sub(rows + 2)),
                None => 0,
            };
            let [history, panel_area, composer] = Layout::vertical([
                Constraint::Fill(1),
                Constraint::Length(panel_rows),
                Constraint::Length(rows + 2),
            ])
            .areas(chat_area);
            messages::draw(
                frame,
                history,
                open,
                &names,
                &mut app.images,
                app.focus == Focus::Messages,
                popup_over_chat,
                colors,
                app.settings.block_gaps,
            );
            if let Some(panel) = panel {
                stickers::draw(frame, panel_area, panel, &mut app.images, colors);
            }
            // While the sticker panel is open, keys go there, not to the text.
            suggestions_at = Some(composer);
            draw_composer(
                frame,
                &mut app.composer,
                ComposerBar::of(open),
                &open.attachments,
                open.as_files,
                &names,
                composer,
                app.focus == Focus::Input && panel_rows == 0,
                &mut app.images,
                popup_over_chat,
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
    if suggesting && let (Some(composer), Some(completion)) = (suggestions_at, &app.completion) {
        draw_suggestions(frame, composer, completion, colors);
    }
    draw_status(frame, app, status, colors);
    if let Some(prompt) = app
        .prompt
        .as_ref()
        .filter(|p| p.kind == PromptKind::Command)
    {
        draw_commands(frame, body, &prompt.query(), colors);
    }
    if let Some(prompt) = app.prompt.as_ref().filter(|p| !p.completions.is_empty()) {
        draw_completions(frame, body, &prompt.completions, colors);
    }
    if let Some(menu) = &app.menu {
        draw_menu(frame, chat_area, menu, colors);
    }
    if let Some(menu) = &app.delete_menu {
        draw_delete(frame, chat_area, menu, colors);
    }
    if let Some(menu) = &app.vote_menu {
        draw_vote(frame, chat_area, menu, colors);
    }
    if let Some(menu) = &mut app.react_menu {
        let yours: Vec<String> = app
            .open
            .iter()
            .flat_map(|o| o.your_emoji(menu.message_id))
            .map(|(_, emoji)| emoji)
            .collect();
        let yours: Vec<&str> = yours.iter().map(String::as_str).collect();
        draw_react(frame, chat_area, menu, &yours, colors);
    }
    if let Some(picker) = &mut app.picker {
        let names = messages::Names {
            users: &app.users,
            chats: &app.chats,
        };
        draw_picker(frame, chat_area, picker, &names, colors);
    }
    if let Some(confirm) = &app.confirm {
        draw_confirm(frame, chat_area, confirm, colors);
    }
    if let Some(menu) = &mut app.settings_menu {
        draw_settings(frame, menu, &app.settings, &app.themes, colors);
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
        .style(popup_style(colors));
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
    let block = popup_block(
        " Delete message ",
        " `Enter` delete · `Esc` cancel ",
        colors,
    );
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

/// Suggestions for the `@name` or `:emoji` being typed, just above the
/// composer, with the one Tab takes marked.
fn draw_suggestions(frame: &mut Frame, composer: Rect, completion: &Completion, colors: &Colors) {
    let longest = completion
        .items
        .iter()
        .map(|s| s.label.width() + 2 + s.detail.width())
        .max()
        .unwrap_or(0);
    let width = (longest as u16 + 4)
        .clamp(24, 50)
        .min(composer.width.saturating_sub(2));
    let height = (completion.items.len() as u16 + 2).min(composer.y);
    if height < 3 {
        return;
    }
    let rect = Rect {
        x: composer.x + 1,
        y: composer.y - height,
        width,
        height,
    };
    let block = Block::bordered()
        .border_type(BorderType::Rounded)
        .title_bottom(
            Line::from(hint_spans(
                " `Tab` insert ",
                Style::new().fg(colors.muted),
                colors,
            ))
            .right_aligned(),
        )
        .border_style(Style::new().fg(colors.accent))
        .style(popup_style(colors));
    let room = usize::from(block.inner(rect).width).saturating_sub(1);
    let lines: Vec<Line> = completion
        .items
        .iter()
        .enumerate()
        .map(|(i, suggestion)| {
            let selected = i == completion.selected;
            let label = truncate(&suggestion.label, room);
            let detail_room = room.saturating_sub(label.width() + 2);
            let mut spans = vec![
                if selected {
                    Span::from("▌").fg(colors.accent)
                } else {
                    Span::from(" ")
                },
                Span::from(label),
            ];
            if detail_room > 0 && !suggestion.detail.is_empty() {
                spans.push(Span::from("  "));
                spans.push(Span::from(truncate(&suggestion.detail, detail_room)).fg(colors.muted));
            }
            let line = Line::from(spans);
            if selected {
                line.bg(colors.selection)
            } else {
                line
            }
        })
        .collect();
    frame.render_widget(Clear, rect);
    frame.render_widget(Paragraph::new(lines).block(block), rect);
}

/// The popup Enter opens on a poll: the question, then its answers, ticked
/// ones marked in a poll that takes several, and "Take back my vote" once
/// you voted.
fn draw_vote(frame: &mut Frame, area: Rect, menu: &VoteMenu, colors: &Colors) {
    let rows = menu.rows().max(1) as u16;
    let longest = menu
        .answers
        .iter()
        .map(|a| a.width() + 10)
        .chain([menu.question.width() + 4])
        .max()
        .unwrap_or(0);
    let width = (longest.clamp(40, 60) as u16).min(area.width);
    // Borders, the question and a gap, then the answers.
    let popup = center(area, width, (rows + 4).min(area.height));
    let keys = if menu.several {
        " `Space` tick · `Enter` vote · `Esc` close "
    } else {
        " `Enter` vote · `Esc` close "
    };
    let block = popup_block(" Vote ", keys, colors);
    let inner = block.inner(popup);
    frame.render_widget(Clear, popup);
    frame.render_widget(block, popup);

    let [question, _, list] = Layout::vertical([
        Constraint::Length(1),
        Constraint::Length(1),
        Constraint::Fill(1),
    ])
    .areas(inner);
    let text_width = (inner.width as usize).saturating_sub(3);
    frame.render_widget(
        Line::from(vec![
            Span::from(" ▎ ").fg(colors.accent),
            Span::from(truncate(&menu.question, text_width)),
        ]),
        question,
    );
    let answer_width = (inner.width as usize).saturating_sub(8);
    let mut items: Vec<ListItem> = menu
        .answers
        .iter()
        .enumerate()
        .map(|(i, answer)| {
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
            let mut line = vec![bar, Span::from(shortcut).fg(colors.muted)];
            if menu.several {
                line.push(Span::from(if menu.ticked[i] { "[x] " } else { "[ ] " }));
            }
            line.push(Span::from(truncate(answer, answer_width)));
            ListItem::new(Line::from(line))
        })
        .collect();
    if menu.can_retract {
        let bar = if menu.selected == menu.answers.len() {
            Span::from("▌").fg(colors.accent)
        } else {
            Span::from(" ")
        };
        items.push(ListItem::new(Line::from(vec![
            bar,
            Span::from("  Take back my vote").fg(colors.error),
        ])));
    }
    frame.render_stateful_widget(
        List::new(items).highlight_style(Style::new().bg(colors.selection)),
        list,
        &mut ListState::default().with_selected(Some(menu.selected)),
    );
}

/// Chats the `f` and `s` popup lists at once; more scroll.
const PICKER_ROWS: usize = 10;
/// Width of the `f` and `s` popup.
const PICKER_WIDTH: u16 = 56;

/// The `f` (forward) and `s` (find a chat) popup over the message pane: what's
/// forwarded, the search, and the chats it finds.
fn draw_picker(
    frame: &mut Frame,
    area: Rect,
    picker: &mut ChatPicker,
    names: &messages::Names,
    colors: &Colors,
) {
    let chats = names.chats;
    let choices = picker.choices(chats);
    let forwarding = picker.forwarding();
    // Borders, what's forwarded, the search and a gap, then the list.
    let fixed = 4 + usize::from(forwarding);
    let visible = choices
        .len()
        .clamp(1, PICKER_ROWS)
        .min(usize::from(area.height).saturating_sub(fixed).max(1));
    let popup = center(area, PICKER_WIDTH.min(area.width), (visible + fixed) as u16);
    let (title, keys, placeholder) = match &picker.purpose {
        Purpose::Forward { .. } => (
            " Forward to ",
            " `Enter` forward · `Esc` cancel ",
            "type to find a chat",
        ),
        Purpose::Open => (
            " Find a chat or person ",
            " `Enter` open · `Esc` cancel ",
            "a name, @username or t.me link",
        ),
    };
    let mut title = vec![Span::from(title)];
    if picker.searching {
        title.push(Span::from("· searching… ").fg(colors.muted));
    }
    let block = popup_block(Line::from(title), keys, colors);
    let inner = block.inner(popup);
    frame.render_widget(Clear, popup);
    frame.render_widget(block, popup);

    let [about, search, _, list] = Layout::vertical([
        Constraint::Length(u16::from(forwarding)),
        Constraint::Length(1),
        Constraint::Length(1),
        Constraint::Fill(1),
    ])
    .areas(inner);
    let text_width = (inner.width as usize).saturating_sub(4);
    if let Purpose::Forward { snippet, .. } = &picker.purpose {
        let line = Line::from(vec![
            Span::from(" ▎ ").fg(colors.accent),
            Span::from(truncate(snippet, text_width)),
        ]);
        frame.render_widget(line, about);
    }
    let mut query = vec![Span::from(" / ").fg(colors.search).bold()];
    if picker.query.is_empty() {
        query.push(Span::from(" ").reversed());
        query.push(Span::from(placeholder).fg(colors.muted));
    } else {
        query.push(Span::from(truncate_start(&picker.query, text_width)));
        query.push(Span::from(" ").reversed());
    }
    frame.render_widget(Line::from(query), search);

    if choices.is_empty() {
        let empty = match () {
            _ if picker.searching => " Searching Telegram…",
            _ if picker.query.trim().is_empty() => " No chats yet",
            _ => " Nothing found",
        };
        frame.render_widget(Line::from(empty).fg(colors.muted), list);
        return;
    }
    let cursor = picker.selected(&choices);
    // Scroll just far enough to keep the cursor in view.
    let max_scroll = choices.len().saturating_sub(visible);
    picker.scroll = picker
        .scroll
        .clamp(cursor.saturating_sub(visible - 1), cursor)
        .min(max_scroll);
    let width = usize::from(list.width).saturating_sub(2);
    let lines: Vec<Line> = choices
        .iter()
        .enumerate()
        .skip(picker.scroll)
        .take(visible)
        .map(|(i, choice)| {
            let selected = i == cursor;
            let (title, style, badge, detail) = match choice {
                Choice::Chat(id) => {
                    let detail = match chats.username(*id) {
                        Some(name) => format!("@{name}"),
                        None if !chats.listed(*id) => "public".into(),
                        None => String::new(),
                    };
                    (
                        chats.title(*id).unwrap_or("Unknown").to_string(),
                        title_style(chats, *id, colors),
                        chats.badge(*id),
                        detail,
                    )
                }
                Choice::User(id) => (
                    names.get(Sender::User(*id)),
                    Style::new(),
                    None,
                    chats
                        .user_username(*id)
                        .map_or("contact".into(), |name| format!("@{name}")),
                ),
                Choice::Username(name) => (
                    format!("Open @{name}"),
                    Style::new().fg(colors.accent),
                    None,
                    String::new(),
                ),
                Choice::Link(link) => (
                    format!("Open {link}"),
                    Style::new().fg(colors.accent),
                    None,
                    String::new(),
                ),
            };
            let badge = badge.map(|b| badge_span(b, colors));
            let badge_w = badge.as_ref().map_or(0, |b| b.content.width());
            // The detail takes up to half the row, the title what's left.
            let room = width.saturating_sub(badge_w);
            let detail = truncate(&detail, room / 2);
            let detail_w = if detail.is_empty() {
                0
            } else {
                detail.width() + 2
            };
            let title = truncate(&title, room.saturating_sub(detail_w));
            let used = title.width() + badge_w;
            let mut spans = vec![
                if selected {
                    Span::from("▌").fg(colors.accent)
                } else {
                    Span::from(" ")
                },
                Span::styled(title, style),
            ];
            spans.extend(badge);
            if !detail.is_empty() {
                let gap = width.saturating_sub(used + detail.width()).max(2);
                spans.push(Span::from(" ".repeat(gap)));
                spans.push(Span::from(detail).fg(colors.muted));
            }
            let line = Line::from(spans);
            if selected {
                line.bg(colors.selection)
            } else {
                line
            }
        })
        .collect();
    frame.render_widget(Paragraph::new(lines), list);
    if max_scroll > 0 {
        let mut state = ScrollbarState::new(max_scroll).position(picker.scroll);
        frame.render_stateful_widget(
            Scrollbar::new(ScrollbarOrientation::VerticalRight)
                .begin_symbol(None)
                .end_symbol(None)
                .thumb_style(colors.accent)
                .track_style(colors.border),
            list,
            &mut state,
        );
    }
}

/// Rows of emoji the `R` popup shows at once; more scroll.
const REACT_ROWS: usize = 6;
/// Width of the `R` popup, with room for its key hints and most emoji names.
const REACT_WIDTH: u16 = 44;

/// The `R` popup over the message pane: the message (or the search), a grid
/// of emoji with yours filled in, and the name of the one under the cursor.
fn draw_react(
    frame: &mut Frame,
    area: Rect,
    menu: &mut ReactMenu,
    yours: &[&str],
    colors: &Colors,
) {
    // Owned, since drawing moves `menu.scroll`.
    let shown: Vec<String> = menu.shown().into_iter().map(String::from).collect();
    let current = shown.get(menu.selected).map(String::as_str);
    let columns = reactions::COLUMNS;
    let rows = shown.len().div_ceil(columns).max(1);
    // With reactions of yours, and not searching (where X is typed), a row
    // says X takes them back, so it's known outside the popup too.
    let hint = !yours.is_empty() && menu.query.is_none();
    // Borders, the message or search and a gap, the grid, a gap and the name.
    let fixed = 6 + usize::from(hint);
    let visible = rows
        .min(REACT_ROWS)
        .min(usize::from(area.height).saturating_sub(fixed).max(1));
    let popup = center(area, REACT_WIDTH.min(area.width), (visible + fixed) as u16);
    let enter = match current {
        Some(e) if yours.contains(&e) => "`Enter` take back",
        _ => "`Enter` react",
    };
    let keys = match menu.query {
        Some(_) => format!(" {enter} · `Esc` back "),
        None => format!(" `/` search · {enter} · `Esc` close "),
    };
    let block = popup_block(" React ", &keys, colors);
    let inner = block.inner(popup);
    frame.render_widget(Clear, popup);
    frame.render_widget(block, popup);

    let [top, _, grid, _, name, hint_row] = Layout::vertical([
        Constraint::Length(1),
        Constraint::Length(1),
        Constraint::Length(visible as u16),
        Constraint::Length(1),
        Constraint::Length(1),
        Constraint::Length(u16::from(hint)),
    ])
    .areas(inner);
    if hint {
        let line = Line::from(vec![
            Span::from(" X").fg(colors.accent).bold(),
            Span::from(" removes all yours, also in the chat").fg(colors.muted),
        ]);
        frame.render_widget(line, hint_row);
    }
    let text_width = (inner.width as usize).saturating_sub(4);
    let top_line = match &menu.query {
        Some(query) => Line::from(vec![
            Span::from(" / ").fg(colors.search).bold(),
            Span::from(truncate(query, text_width)),
            Span::from(" ").reversed(),
        ]),
        None => Line::from(vec![
            Span::from(" ▎ ").fg(colors.accent),
            Span::from(truncate(&menu.snippet, text_width)),
        ]),
    };
    frame.render_widget(top_line, top);

    if menu.choices.is_none() {
        frame.render_widget(Line::from(" Loading…").fg(colors.muted), grid);
        return;
    }
    let Some(current) = current else {
        frame.render_widget(Line::from(" No emoji by that name").fg(colors.muted), grid);
        return;
    };
    // Each emoji takes four columns: itself and the cursor's brackets. One
    // more on the right for the scroll bar.
    let [grid] = Layout::horizontal([Constraint::Length((columns * 4 + 1) as u16)])
        .flex(Flex::Center)
        .areas(grid);
    let row = menu.selected / columns;
    // Scroll just far enough to keep the cursor's row in view.
    let max_scroll = rows.saturating_sub(visible);
    menu.scroll = menu
        .scroll
        .clamp(row.saturating_sub(visible - 1), row)
        .min(max_scroll);
    let lines: Vec<Line> = shown
        .chunks(columns)
        .enumerate()
        .skip(menu.scroll)
        .take(visible)
        .map(|(r, emoji)| {
            let mut spans = Vec::new();
            for (c, e) in emoji.iter().enumerate() {
                let at = r * columns + c;
                let mut style = Style::new();
                if yours.contains(&e.as_str()) {
                    style = style.bg(colors.your_reaction);
                } else if at == menu.selected {
                    style = style.bg(colors.selection);
                }
                let (open, close) = if at == menu.selected {
                    ("[", "]")
                } else {
                    (" ", " ")
                };
                let bracket = style.fg(colors.accent).bold();
                let shown = reactions::shown(e);
                let pad = " ".repeat(2usize.saturating_sub(shown.width()));
                spans.push(Span::styled(open, bracket));
                spans.push(Span::styled(shown + &pad, style));
                spans.push(Span::styled(close, bracket));
            }
            Line::from(spans)
        })
        .collect();
    frame.render_widget(Paragraph::new(lines), grid);
    if max_scroll > 0 {
        let mut state = ScrollbarState::new(max_scroll).position(menu.scroll);
        frame.render_stateful_widget(
            Scrollbar::new(ScrollbarOrientation::VerticalRight)
                .begin_symbol(None)
                .end_symbol(None)
                .thumb_style(colors.accent)
                .track_style(colors.border),
            grid,
            &mut state,
        );
    }

    // What the emoji under the cursor is called, to search for it next time.
    if let Some(found) = reactions::about(current) {
        let code = found
            .shortcode()
            .map_or(String::new(), |c| format!("  :{c}:"));
        let width = usize::from(name.width).saturating_sub(1);
        let line = Line::from(vec![
            Span::from(" "),
            Span::from(truncate(found.name(), width)),
            Span::from(code).fg(colors.muted),
        ]);
        frame.render_widget(line, name);
    }
}

/// The `?` popup, centered on the screen: a tab with every shortcut, and one
/// with the settings.
fn draw_settings(
    frame: &mut Frame,
    menu: &mut SettingsMenu,
    settings: &Settings,
    themes: &Themes,
    colors: &Colors,
) {
    let area = frame.area();
    // Both tabs get the same size, so the tabs don't move when switching.
    let width = (help::width() as u16 + 2).min(area.width.saturating_sub(2));
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
        HelpTab::Shortcuts => " `j/k` scroll · `Tab` settings · `Esc` close ",
        HelpTab::Settings if menu.selected >= SettingsMenu::THEMES => {
            " `Enter` or `Space` use · `Tab` shortcuts · `Esc` close "
        }
        HelpTab::Settings => " `Enter` or `Space` on/off · `Tab` shortcuts · `Esc` close ",
    };
    let block = popup_block(tabs, keys, colors);
    let inner = block.inner(popup);
    frame.render_widget(Clear, popup);
    frame.render_widget(block, popup);
    if menu.tab == HelpTab::Shortcuts {
        help::draw(frame, inner, &mut menu.scroll, colors);
        return;
    }

    let row = |i: usize, mark: &'static str, label: Span<'static>| {
        let selected = i == menu.selected;
        let bar = if selected {
            Span::from("▌").fg(colors.accent)
        } else {
            Span::from(" ")
        };
        let line = Line::from(vec![bar, Span::from(mark).fg(colors.accent), label]);
        if selected {
            line.style(Style::new().bg(colors.selection))
        } else {
            line
        }
    };
    let heading = |text: &'static str| Line::from(text).fg(colors.muted).bold();
    let check = |on: bool| if on { " [✓] " } else { " [ ] " };
    let last_row = SettingsMenu::THEMES + themes.list.len() - 1;
    // The line the cursor is on, and the last one to show with it.
    let mut lines = Vec::new();
    let (mut cursor, mut cursor_end) = (0, 0);
    let mut add = |lines: &mut Vec<Line<'static>>, i: usize, mark, label: &str| {
        if i == menu.selected {
            cursor = lines.len();
            cursor_end = cursor;
        }
        lines.push(row(i, mark, Span::from(label.to_string())));
    };
    lines.push(heading(" Notifications"));
    add(
        &mut lines,
        SettingsMenu::NOTIFICATIONS,
        check(settings.notifications != Notifications::Off),
        "New messages, while tuigram is in the background",
    );
    lines.push(Line::default());
    lines.push(heading(" Chat list"));
    add(
        &mut lines,
        SettingsMenu::CHAT_GAPS,
        check(settings.chat_gaps),
        "A gap between chats",
    );
    add(
        &mut lines,
        SettingsMenu::LIST_RIGHT,
        check(settings.chat_list_side == Side::Right),
        "On the right side of the window",
    );
    lines.push(Line::default());
    lines.push(heading(" Messages"));
    add(
        &mut lines,
        SettingsMenu::BLOCK_GAPS,
        check(settings.block_gaps),
        "A gap between messages in a row from one person",
    );
    lines.push(Line::default());
    lines.push(heading(" Composer"));
    add(
        &mut lines,
        SettingsMenu::AFTER_SEND,
        check(settings.normal_after_send),
        "Back to Normal mode after sending a message",
    );
    lines.push(Line::default());
    lines.push(heading(" Theme"));
    for (i, theme) in themes.list.iter().enumerate() {
        // The dot marks the theme in use.
        let mark = if theme.id == settings.theme {
            " ● "
        } else {
            " ○ "
        };
        add(&mut lines, SettingsMenu::THEMES + i, mark, &theme.label);
        // One that can't be used is dimmed; picking it says why.
        if theme.colors.is_err()
            && let Some(line) = lines.last_mut()
            && let Some(label) = line.spans.last_mut()
        {
            label.style = label.style.fg(colors.muted);
        }
    }
    if let Some(dir) = &themes.dir {
        lines.push(Line::default());
        lines.push(Line::from(" Your own themes go in").fg(colors.muted));
        lines.push(Line::from(format!(" {}", short_path(dir))).fg(colors.muted));
    }
    // On the last theme, the lines after it show too.
    if menu.selected == last_row {
        cursor_end = lines.len() - 1;
    }

    // Scrolled as little as keeps the cursor in view, with the line above
    // it: the heading, for the first row of a group.
    let rows = usize::from(inner.height);
    let scroll = &mut menu.settings_scroll;
    *scroll = (*scroll).min(cursor.saturating_sub(1));
    if cursor_end >= *scroll + rows {
        *scroll = cursor_end + 1 - rows;
    }
    let max = lines.len().saturating_sub(rows);
    *scroll = (*scroll).min(max);
    frame.render_widget(Paragraph::new(lines).scroll((*scroll as u16, 0)), inner);
    scrollbar(frame, inner, max, *scroll, colors);
}

/// A scrollbar down the right of `area`, for a list scrolled `position` rows
/// of the most it can be, `max`. None if it all fits.
fn scrollbar(frame: &mut Frame, area: Rect, max: usize, position: usize, colors: &Colors) {
    if max == 0 {
        return;
    }
    let mut state = ScrollbarState::new(max).position(position);
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

/// `path` with the home folder written `~`, to keep it short.
fn short_path(path: &std::path::Path) -> String {
    let path = match dirs::home_dir().and_then(|home| path.strip_prefix(home).ok()) {
        Some(rest) => std::path::Path::new("~").join(rest),
        None => path.to_path_buf(),
    };
    text::clean(&path.display().to_string())
}

/// A popup's frame: accent border and its own background, so it stands out
/// from what's underneath. Callers draw `Clear` first.
/// The "are you sure" popup over the message pane, for a file that could
/// run code or a link that hides where it goes.
fn draw_confirm(frame: &mut Frame, area: Rect, confirm: &Confirm, colors: &Colors) {
    const SITE: &str = "It goes to:    ";
    let title = format!(" {} ", confirm.title);
    let verdict = confirm.badge.map(|badge| match badge {
        Badge::Scam => "Telegram marks it as a SCAM",
        Badge::Fake => "Telegram marks it as FAKE: it poses as someone else",
        Badge::Official => "Verified by Telegram ✓",
    });
    let site_width = confirm
        .site
        .as_ref()
        .map_or(0, |s| SITE.width() + s.width());
    let longest = confirm
        .lines
        .iter()
        .map(|l| l.width())
        .chain(verdict.map(|v| v.width()))
        .max()
        .unwrap_or(0);
    let width = (longest.max(title.width()).max(site_width) as u16 + 4)
        .min(area.width)
        .max(40.min(area.width));
    let height =
        confirm.lines.len() + usize::from(confirm.site.is_some()) + usize::from(verdict.is_some());
    let popup = center(area, width, height as u16 + 2);
    let keys = format!(" `y` {} · `Esc` cancel ", confirm.action.verb());
    let border = match confirm.badge {
        Some(Badge::Scam | Badge::Fake) => colors.error,
        _ => colors.warning,
    };
    let block = popup_block(title, &keys, colors).border_style(Style::new().fg(border));
    let room = (block.inner(popup).width as usize).saturating_sub(2);
    let mut lines: Vec<Line> = confirm
        .lines
        .iter()
        .map(|line| Line::from(format!(" {}", truncate(line, room))))
        .collect();
    // The site goes right after what the text says, and keeps its end: a
    // long host can only hide the part a sender made up.
    if let Some(site) = &confirm.site {
        let host = truncate_start(site, room.saturating_sub(SITE.width()));
        lines.insert(lines.len().min(1), Line::from(format!(" {SITE}{host}")));
    }
    // What Telegram says about the chat goes first, on a line of its own,
    // so a long name can't push it out of sight or pose as it.
    if let (Some(verdict), Some(badge)) = (verdict, confirm.badge) {
        let style = match badge {
            Badge::Scam | Badge::Fake => Style::new().fg(colors.error).bold(),
            Badge::Official => Style::new().fg(colors.accent),
        };
        lines.insert(
            0,
            Line::styled(format!(" {}", truncate(verdict, room)), style),
        );
    }
    frame.render_widget(Clear, popup);
    frame.render_widget(Paragraph::new(lines).block(block), popup);
}

fn popup_block<'a>(title: impl Into<Line<'a>>, keys: &'a str, colors: &Colors) -> Block<'a> {
    Block::bordered()
        .title(title)
        .title_bottom(
            Line::from(hint_spans(keys, Style::new().fg(colors.muted), colors)).right_aligned(),
        )
        .border_style(Style::new().fg(colors.accent))
        .style(popup_style(colors))
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
        MenuAction::Open => popup_block(
            " Open ",
            " `Enter` open · `1-9` pick · `Esc` close ",
            colors,
        ),
        MenuAction::Copy => popup_block(
            " Copy ",
            " `Enter` copy · `1-9` pick · `Esc` close ",
            colors,
        ),
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
/// which message the reply answers, and files to send are listed under it,
/// after a picture of each photo. `covered`: a popup may be over it.
#[allow(clippy::too_many_arguments)]
fn draw_composer(
    frame: &mut Frame,
    composer: &mut TextArea<'static>,
    bar: Option<ComposerBar>,
    attachments: &[Attachment],
    as_files: bool,
    names: &messages::Names,
    area: Rect,
    insert: bool,
    images: &mut Images,
    covered: bool,
    colors: &Colors,
) {
    // Text starts a column in, in line with the message bubbles above.
    let block = Block::bordered()
        .border_style(border(insert, colors))
        .padding(Padding::horizontal(1));
    let mut text = block.inner(area);
    frame.render_widget(block, area);
    if let Some(bar) = &bar {
        let [top, rest] =
            Layout::vertical([Constraint::Length(BAR_ROWS), Constraint::Fill(1)]).areas(text);
        draw_bar(frame, bar, names, top, colors);
        text = rest;
    }
    if !attachments.is_empty() {
        let [photos, files, rest] = Layout::vertical([
            Constraint::Length(preview_rows(attachments, images)),
            Constraint::Length(attachment_rows(attachments)),
            Constraint::Fill(1),
        ])
        .areas(text);
        draw_previews(frame, attachments, photos, images, covered, colors);
        let lines = attachment_lines(attachments, as_files, files.width as usize, colors);
        frame.render_widget(Paragraph::new(lines), files);
        text = rest;
    }
    // The cursor only shows in Insert mode, so it's obvious where keys go.
    composer.set_cursor_style(if insert {
        Style::new().reversed()
    } else {
        Style::new()
    });
    frame.render_widget(&*composer, text);
    if !composer.is_empty() {
        return;
    }
    let caption = !attachments.is_empty();
    let placeholder = match (insert, &bar) {
        (true, Some(ComposerBar::Edit(_))) => "Write the new text…",
        (true, _) if caption => "Add a caption…",
        // Where it's seen, for those who don't read the status bar.
        (true, Some(ComposerBar::Reply(_))) => "Write a reply… · `Tab` for stickers",
        (true, None) => "Write a message… · `Tab` for stickers",
        (false, Some(ComposerBar::Edit(_))) => "Press `i` to edit",
        (false, _) if caption => "Press `i` to add a caption",
        (false, Some(ComposerBar::Reply(_))) => "Press `i` to write your reply",
        (false, None) => "Press `i` to write a message",
    };
    // Drawn over the empty text area rather than as its own placeholder,
    // which can't make the keys stand out. The cursor's look stays.
    let muted = Style::new().fg(colors.muted);
    frame.render_widget(Line::from(hint_spans(placeholder, muted, colors)), text);
}

fn attachment_rows(attachments: &[Attachment]) -> u16 {
    attachments.len().min(MAX_ATTACHMENT_ROWS) as u16
}

/// Rows for the pictures of the photos waiting to be sent: as many as the
/// tallest takes, or none where the terminal only has half blocks, which
/// are too coarse for a picture this small.
fn preview_rows(attachments: &[Attachment], images: &Images) -> u16 {
    if !images.draws_photos() {
        return 0;
    }
    let font = images.font_size();
    attachments
        .iter()
        .filter_map(Attachment::preview)
        .map(|photo| messages::thumbnail_cells(&photo, font).1)
        .max()
        .unwrap_or(0)
}

/// The photos waiting to be sent, side by side, each as big as a link
/// preview's picture, so a wrong one is seen before it goes. What doesn't
/// fit is counted after the last one shown.
fn draw_previews(
    frame: &mut Frame,
    attachments: &[Attachment],
    area: Rect,
    images: &mut Images,
    covered: bool,
    colors: &Colors,
) {
    if area.is_empty() {
        return;
    }
    let font = images.font_size();
    let photos: Vec<&Attachment> = attachments.iter().filter(|a| a.kind.is_photo()).collect();
    let mut slots = Vec::new();
    let mut x = 0;
    for photo in photos.iter().filter_map(|a| a.preview()) {
        let (cols, rows) = messages::thumbnail_cells(&photo, font);
        if x + cols > area.width {
            break;
        }
        slots.push(messages::PhotoSlot {
            line: 0,
            x,
            cols,
            rows,
            photo,
        });
        x += cols + 1;
    }
    // The count needs room too, which the last pictures give up.
    let more = |shown: usize| format!("+{}", photos.len() - shown);
    while slots.len() < photos.len()
        && usize::from(area.width.saturating_sub(x)) < more(slots.len()).width()
        && let Some(slot) = slots.pop()
    {
        x = slot.x;
    }
    for (slot, attachment) in slots.iter().zip(&photos) {
        images.local_file(slot.photo.file_id, &attachment.path.to_string_lossy());
    }
    messages::draw_photos(frame, area, &slots, 0, images, covered, colors);
    if slots.len() < photos.len() {
        let label = more(slots.len());
        let row = Rect {
            x: area.x + x,
            y: area.y + area.height / 2,
            width: label.width() as u16,
            height: 1,
        };
        frame.render_widget(Span::from(label).fg(colors.subtle), row.intersection(area));
    }
}

/// A row per file waiting to be sent: its name, how it goes, and its size.
fn attachment_lines(
    attachments: &[Attachment],
    as_files: bool,
    width: usize,
    colors: &Colors,
) -> Vec<Line<'static>> {
    let shown = if attachments.len() > MAX_ATTACHMENT_ROWS {
        MAX_ATTACHMENT_ROWS - 1
    } else {
        attachments.len()
    };
    let line = |name: &str, details: String| {
        let clip = "📎 ";
        let name = truncate(name, width.saturating_sub(clip.width() + details.width()));
        Line::from(vec![
            Span::from(clip).fg(colors.attach),
            Span::from(name).fg(colors.attach).bold(),
            Span::from(details).fg(colors.subtle),
        ])
    };
    let mut lines: Vec<Line> = attachments[..shown]
        .iter()
        .map(|a| {
            let kind = match a.sent_as(as_files) {
                Kind::Photo { .. } => "Photo",
                Kind::File if a.kind.is_photo() => "File, uncompressed",
                Kind::File => "File",
            };
            line(
                &a.name,
                format!(" · {kind} · {}", attach::size_label(a.size)),
            )
        })
        .collect();
    // The rest are named too, as far as the row goes: nothing should go out
    // that the composer never showed.
    let rest = &attachments[shown..];
    if !rest.is_empty() {
        let size = rest.iter().map(|a| a.size).sum();
        let names: Vec<&str> = rest.iter().map(|a| a.name.as_str()).collect();
        let more = format!("{} more: {}", rest.len(), names.join(", "));
        lines.push(line(&more, format!(" · {}", attach::size_label(size))));
    }
    lines
}

/// What sits over the composer's text, with a bar down the side like a
/// quote: who is being answered, or that a message is being edited, over a
/// line of that message.
enum ComposerBar<'a> {
    Reply(&'a Replied),
    Edit(&'a Editing),
}

impl<'a> ComposerBar<'a> {
    /// An edit hides the reply, which comes back when it's done.
    fn of(open: &'a OpenChat) -> Option<Self> {
        open.editing
            .as_ref()
            .map(Self::Edit)
            .or(open.reply.as_ref().map(Self::Reply))
    }
}

fn draw_bar(
    frame: &mut Frame,
    bar: &ComposerBar,
    names: &messages::Names,
    area: Rect,
    colors: &Colors,
) {
    let width = (area.width as usize).saturating_sub(2);
    let (color, title, snippet) = match bar {
        ComposerBar::Reply(reply) => {
            let label = "↩ Reply to ";
            let name = names.author(reply.sender, reply.outgoing);
            let name = truncate(&name, width.saturating_sub(label.width()));
            let title = vec![Span::from(label), Span::from(name).bold()];
            (colors.reply, title, &reply.snippet)
        }
        ComposerBar::Edit(editing) => (
            colors.edit,
            vec![Span::from("✎ Edit message").bold()],
            &editing.snippet,
        ),
    };
    let side = || Span::from("▎ ").fg(color);
    let mut first = vec![side()];
    first.extend(title.into_iter().map(|span| span.fg(color)));
    let lines = vec![
        Line::from(first),
        Line::from(vec![
            side(),
            Span::from(truncate(snippet, width)).fg(colors.subtle),
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
            "  `Enter` done · `Esc` cancel ",
        ),
        PromptKind::Messages => (
            " SEARCH ",
            colors.search,
            " /",
            "  `Enter` search · `Esc` cancel ",
        ),
        PromptKind::Command => (
            " COMMAND ",
            colors.command,
            " :",
            "  `Enter` run · `Esc` cancel ",
        ),
        PromptKind::Attach => (
            " ATTACH ",
            colors.attach,
            " ",
            "  `Tab` complete · `Enter` attach · `Esc` cancel ",
        ),
    };
    let hints = Line::from(hint_spans(hints, Style::new().fg(colors.muted), colors));
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
    frame.render_widget(hints.right_aligned(), keys);
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
        .style(popup_style(colors));
    // Clear first: the popup must cover text and photos underneath.
    frame.render_widget(Clear, popup);
    frame.render_widget(Paragraph::new(lines).block(block), popup);
}

/// Names in the folder that the attach prompt's last Tab matched, over the
/// bottom left corner like the command list.
fn draw_completions(frame: &mut Frame, area: Rect, names: &[String], colors: &Colors) {
    const MAX_SHOWN: usize = 12;
    let max = (area.width as usize).saturating_sub(4);
    let mut lines: Vec<Line> = names
        .iter()
        .take(MAX_SHOWN)
        .map(|name| {
            let style = if name.ends_with('/') {
                Style::new().fg(colors.primary)
            } else {
                Style::new()
            };
            Line::from(format!(" {}", truncate(&text::clean(name), max))).style(style)
        })
        .collect();
    if names.len() > MAX_SHOWN {
        lines.push(Line::from(format!(" … {} more", names.len() - MAX_SHOWN)).fg(colors.muted));
    }
    let title = " Matches · type more, then Tab ";
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
        .style(popup_style(colors));
    // Clear first: the popup must cover text and photos underneath.
    frame.render_widget(Clear, popup);
    frame.render_widget(Paragraph::new(lines).block(block), popup);
}

/// Ctrl-t's hint while photos are attached: what it switches them to.
fn as_files_hint(open: &OpenChat) -> Option<&'static str> {
    let photos = open.attachments.iter().any(|a| a.kind.is_photo());
    photos.then_some(if open.as_files {
        "`Ctrl-t` send as photos"
    } else {
        "`Ctrl-t` send as files"
    })
}

/// Hints for keys that depend on the cursor: Enter on hidden spoilers, `gd`
/// on a reply, and Ctrl-o after a `gd`.
fn jump_hints(open: &OpenChat) -> Vec<&'static str> {
    let mut hints = Vec::new();
    if open.cursor_id().is_some_and(|id| open.hides_spoilers(id)) {
        hints.push("`Enter` show spoiler");
    }
    let on_reply = open
        .cursor_id()
        .and_then(|id| open.messages.get(&id))
        .is_some_and(|m| m.reply_to.is_some());
    if on_reply {
        hints.push("`gd` go to replied");
    }
    if !open.jumps.is_empty() {
        hints.push("`Ctrl-o` back to reply");
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
    let editing = app.open.as_ref().is_some_and(|o| o.editing.is_some());
    let attaching = app.open.as_ref().is_some_and(|o| !o.attachments.is_empty());
    let dropped = app.open.as_ref().is_some_and(|o| o.dropped.is_some());
    let insert = Span::from(" INSERT ").fg(colors.bg).bg(colors.insert);
    let sticker = Span::from(" STICKERS ").fg(colors.bg).bg(colors.insert);
    let picking = app.focus == Focus::Input && app.stickers.is_some();
    let (mode, hints) = match app.focus {
        _ if app.resizing.is_some() => (
            Span::from(" RESIZE ").fg(colors.bg).bg(colors.accent),
            match app.settings.chat_list_side {
                Side::Left => {
                    "  `h` narrower · `l` wider · `=` as at first · `Enter` keep · `Esc` cancel"
                }
                Side::Right => {
                    "  `h` wider · `l` narrower · `=` as at first · `Enter` keep · `Esc` cancel"
                }
            },
        ),
        _ if app.confirm.is_some() => (
            normal,
            match app.confirm.as_ref().map(|c| &c.action) {
                Some(Confirmed::Edit { .. }) => "  `y` edit · `n` or `Esc` cancel",
                Some(Confirmed::Join(_) | Confirmed::JoinLink { .. }) => {
                    "  `y` join · `n` or `Esc` cancel"
                }
                Some(Confirmed::Leave(_)) => "  `y` leave · `n` or `Esc` cancel",
                _ => "  `y` open · `n` or `Esc` cancel",
            },
        ),
        _ if app.settings_menu.as_ref().map(|m| m.tab) == Some(HelpTab::Shortcuts) => {
            (normal, "  `j/k` scroll · `Tab` settings · `Esc` close")
        }
        _ if app.settings_menu.is_some() => (
            normal,
            "  `j/k` move · `Enter` or `Space` change, saved at once · `Tab` shortcuts · `Esc` close",
        ),
        _ if app.delete_menu.is_some() => {
            (normal, "  `j/k` choose · `Enter` delete · `Esc` cancel")
        }
        _ if app.react_menu.as_ref().is_some_and(|m| m.query.is_some()) => (
            normal,
            "  type a name, like heart or +1 · `arrows` choose · `Enter` react · `Esc` back",
        ),
        _ if app.react_menu.is_some() => (
            normal,
            "  `h/j/k/l` choose · `Enter` react, or take yours back · `X` take all yours back · `/` search · `Esc` close",
        ),
        _ if app.vote_menu.as_ref().is_some_and(|m| m.several) => (
            normal,
            "  `j/k` choose · `Space` tick · `Enter` vote for the ticked · `Esc` close",
        ),
        _ if app.vote_menu.is_some() => (normal, "  `j/k` choose · `Enter` vote · `Esc` close"),
        _ if app.picker.as_ref().is_some_and(ChatPicker::forwarding) => (
            normal,
            "  type a chat's name · `arrows` choose · `Enter` forward · `Esc` cancel",
        ),
        _ if app.picker.is_some() => (
            normal,
            "  type a name, @username or t.me link · `arrows` choose · `Enter` open · `Esc` cancel",
        ),
        Focus::Chats if !app.chats.filter().is_empty() => (
            normal,
            "  `j/k` move · `Enter` open · `Esc` clear search · `/` search again · `i` write · `q` quit",
        ),
        Focus::Chats => (
            normal,
            "  `j/k` move · `Enter` open · `i` write · `/` search · `s` find anyone · `p` pin · `m` mute · `H` highlight · `gg/G` top/bottom · `Ctrl-d/u` half page · `Ctrl-r` resize · `:` commands · `?` help · `q` quit",
        ),
        Focus::Messages if searching => (
            normal,
            "  `n/N` older/newer match · `Esc` end search · `/` search again · `j/k` newer/older · `Enter` open media · `r` reply · `i` write · `h` back",
        ),
        Focus::Messages if editing => (
            normal,
            "  `i` edit · `Esc` cancel edit · `e` edit selected instead · `j/k` newer/older · `h` back",
        ),
        Focus::Messages if attaching => (
            normal,
            "  `i` add caption · `Esc` remove files · `a` attach more · `p` paste more · `r` reply · `j/k` newer/older · `h` back",
        ),
        Focus::Messages if replying => (
            normal,
            "  `i` write reply · `Esc` cancel reply · `r` reply to selected instead · `j/k` newer/older · `Enter` open media · `h` back",
        ),
        Focus::Messages => (
            normal,
            "  `j/k` newer/older · `y` copy · `r` reply · `f` forward · `R` react · `X` unreact · `e` edit · `d` delete · `Enter` open media · `i` write · `a` attach · `p` paste · `/` search · `s` find anyone · `gg/G` oldest/newest · `h` back · `Ctrl-r` resize · `:` commands · `?` help · `q` quit",
        ),
        _ if picking && app.stickers.as_ref().is_some_and(|p| p.query.is_some()) => (
            sticker,
            "  type an emoji or a word, like cat · `arrows` choose · `Enter` send · `Esc` back",
        ),
        _ if picking => (
            sticker,
            "  `h/j/k/l` choose · `H/L` previous/next tab · `Enter` send · `/` search · `Tab` or `Esc` back to writing",
        ),
        Focus::Input if app.completion.as_ref().is_some_and(|c| !c.items.is_empty()) => (
            insert,
            "  `Tab` insert · `arrows` choose · keep typing to narrow it down · `Enter` send · `Esc` normal mode",
        ),
        Focus::Input if editing => (
            insert,
            "  `Enter` save · `Alt-Enter` or `Ctrl-j` new line · `Esc` normal mode · `Esc Esc` cancel edit",
        ),
        Focus::Input if dropped => (
            insert,
            "  `Enter` send · `Ctrl-z` paste as text instead · `Alt-Enter` or `Ctrl-j` new line · `Esc` normal mode · `Esc Esc` remove files",
        ),
        Focus::Input if attaching => (
            insert,
            "  `Enter` send · `Ctrl-v` paste more · `Alt-Enter` or `Ctrl-j` new line · `Esc` normal mode · `Esc Esc` remove files",
        ),
        Focus::Input if replying => (
            insert,
            "  `Enter` send reply · `Alt-Enter` or `Ctrl-j` new line · `Tab` stickers · `Esc` normal mode · `Esc Esc` cancel reply",
        ),
        Focus::Input => (
            insert,
            "  `Enter` send · `Alt-Enter` or `Ctrl-j` new line · `Tab` stickers · `Ctrl-v` paste photo or file · `Esc` normal mode",
        ),
    };
    // `l` goes back to a chat list on the right.
    let hints = match app.settings.chat_list_side {
        Side::Left => hints.to_string(),
        Side::Right => hints.replace("`h` back", "`l` back"),
    };
    let popup = app.settings_menu.is_some()
        || app.resizing.is_some()
        || app.delete_menu.is_some()
        || app.react_menu.is_some()
        || app.vote_menu.is_some()
        || app.picker.is_some()
        || app.confirm.is_some();
    let mut context = match &app.open {
        Some(open) if app.focus == Focus::Messages && !popup => jump_hints(open),
        _ => Vec::new(),
    };
    if let Some(open) = &app.open
        && app.focus != Focus::Chats
        && !popup
        && let Some(hint) = as_files_hint(open)
    {
        context.insert(0, hint);
    }
    let mut spans = vec![mode.bold()];
    let muted = Style::new().fg(colors.muted);
    if context.is_empty() {
        spans.extend(hint_spans(&hints, muted, colors));
    } else {
        // Keys that only work right here go first, a bit brighter.
        let here = format!("  {} · ", context.join(" · "));
        spans.extend(hint_spans(&here, Style::new().fg(colors.fg), colors));
        spans.extend(hint_spans(hints.trim_start(), muted, colors));
    }
    if app.resizing.is_some() {
        let width = format!("  chat list {}%", app.settings.list_width());
        spans.push(Span::from(width).fg(colors.fg));
    }
    if app.quit_deadline.is_some() {
        spans.push(Span::from("  Closing… (q again to force)").fg(colors.warning));
    } else if !app.opening.is_empty() {
        spans.push(Span::from("  Downloading… opens when done").fg(colors.warning));
    } else if !app.copying.is_empty() {
        spans.push(Span::from("  Downloading… copies when done").fg(colors.warning));
    } else if app.pasting {
        spans.push(Span::from("  Reading the clipboard…").fg(colors.warning));
    } else if let Some(finding) = &app.finding {
        let what = truncate(&finding.request, 60);
        let looking = format!("  Looking for {what}… (Esc stops)");
        spans.push(Span::from(looking).fg(colors.warning));
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

/// Telegram's mark after a name: SCAM and FAKE as warnings, ✓ for verified
/// and official accounts.
pub(crate) fn badge_span(badge: Badge, colors: &Colors) -> Span<'static> {
    match badge {
        Badge::Scam | Badge::Fake => Span::from(badge.mark()).fg(colors.error).bold(),
        Badge::Official => Span::from(badge.mark()).fg(colors.accent),
    }
}

/// Like [`truncate`], but keeps the end and cuts the start.
fn truncate_start(text: &str, max: usize) -> String {
    if text.width() <= max {
        return text.to_string();
    }
    let mut kept: Vec<char> = Vec::new();
    let mut used = 0;
    for c in text.chars().rev() {
        let w = unicode_width::UnicodeWidthChar::width(c).unwrap_or(0);
        if used + w + 1 > max {
            break;
        }
        kept.push(c);
        used += w;
    }
    let mut out = String::from("…");
    out.extend(kept.into_iter().rev());
    while out.width() > max && out.len() > '…'.len_utf8() {
        out.remove('…'.len_utf8());
    }
    out
}

/// Cuts `text` to at most `max` terminal columns, ending with `…` if cut.
pub(crate) fn truncate(text: &str, max: usize) -> String {
    if text.width() <= max {
        return text.to_string();
    }
    let mut out = String::new();
    let mut used = 0;
    // Zero-width characters don't add to `used`, so a run of them would all
    // be kept, and the check below measures the whole string once per one it
    // takes off. A few per column is more than any real text needs.
    for c in text.chars().take(max * 8 + 16) {
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
    use crate::messages::{Editable, MediaFile};

    /// Every row of the buffer as a string.
    /// Half blocks: no pictures of photos to send, just their names.
    fn no_images() -> Images {
        Images::new(
            ratatui_image::picker::Picker::halfblocks(),
            tokio::sync::mpsc::unbounded_channel().0,
        )
    }

    fn buffer_rows(buf: &ratatui::buffer::Buffer) -> Vec<String> {
        (0..buf.area.height)
            .map(|y| (0..buf.area.width).map(|x| buf[(x, y)].symbol()).collect())
            .collect()
    }

    #[test]
    fn help_opens_on_the_shortcuts_and_scrolls_to_the_end() {
        let colors = Colors::default();
        let mut menu = SettingsMenu {
            selected: 3,
            ..SettingsMenu::new(HelpTab::Shortcuts, &Settings::default())
        };
        // Too short for the whole list.
        let mut terminal = Terminal::new(TestBackend::new(80, 20)).unwrap();
        let mut draw = |menu: &mut SettingsMenu| {
            terminal
                .draw(|f| {
                    draw_settings(f, menu, &Settings::default(), &Themes::built_in(), &colors)
                })
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
        assert!(has(&rows, "New messages") && !has(&rows, "Everywhere"));
    }

    #[test]
    fn a_toast_says_what_was_copied_in_the_corner() {
        let colors = Colors::default();
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
            .draw(|f| draw_menu(f, f.area(), &menu, &Colors::default()))
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
            .draw(|f| draw_menu(f, f.area(), &menu, &Colors::default()))
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
    fn the_composer_says_a_message_is_being_edited() {
        let colors = Colors::default();
        let users = std::collections::HashMap::new();
        let chats = crate::chats::Chats::default();
        let names = messages::Names {
            users: &users,
            chats: &chats,
        };
        let editing = Editing {
            id: 7,
            snippet: "see you at 7".into(),
            original: "see you at 7".into(),
            editable: Editable::Text,
            draft: String::new(),
            reply: None,
            attachments: Vec::new(),
        };
        let mut composer = TextArea::default();
        let mut terminal = Terminal::new(TestBackend::new(40, 5)).unwrap();
        terminal
            .draw(|f| {
                let area = f.area();
                let bar = Some(ComposerBar::Edit(&editing));
                draw_composer(
                    f,
                    &mut composer,
                    bar,
                    &[],
                    false,
                    &names,
                    area,
                    true,
                    &mut no_images(),
                    false,
                    &colors,
                );
            })
            .unwrap();
        let rows = buffer_rows(terminal.backend().buffer());

        assert!(rows[1].contains("▎ ✎ Edit message"), "{}", rows[1]);
        assert!(rows[2].contains("▎ see you at 7"), "{}", rows[2]);
        assert!(rows[3].contains("Write the new text…"));
        let buf = terminal.backend().buffer();
        let bar = (0..buf.area.width)
            .find(|&x| buf[(x, 1)].symbol() == "▎")
            .unwrap();
        assert_eq!(buf[(bar, 1)].fg, colors.edit);
    }

    #[test]
    fn the_composer_says_which_message_a_reply_answers() {
        let colors = Colors::default();
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
                draw_composer(
                    f,
                    &mut composer,
                    Some(ComposerBar::Reply(&reply)),
                    &[],
                    false,
                    &names,
                    area,
                    true,
                    &mut no_images(),
                    false,
                    &colors,
                );
            })
            .unwrap();
        let buf = terminal.backend().buffer();
        let rows: Vec<String> = (0..buf.area.height)
            .map(|y| (0..buf.area.width).map(|x| buf[(x, y)].symbol()).collect())
            .collect();

        assert!(rows[1].contains("▎ ↩ Reply to Chardy"), "{}", rows[1]);
        assert!(rows[2].contains("▎ are we still on"), "{}", rows[2]);
        assert!(rows[2].contains('…'), "a long message is cut to fit");
        assert!(
            rows[3].contains("Write a reply… · Tab for stickers"),
            "typing goes below it: {}",
            rows[3]
        );
        let tab = (0..buf.area.width)
            .find(|&x| buf[(x, 3)].symbol() == "T")
            .unwrap();
        assert_eq!(buf[(tab, 3)].fg, colors.accent, "the key stands out");
        assert_eq!(buf[(tab + 4, 3)].fg, colors.muted, "what it does doesn't");
        let bar = (0..buf.area.width)
            .find(|&x| buf[(x, 1)].symbol() == "▎")
            .unwrap();
        assert_eq!(buf[(bar, 1)].fg, colors.reply);
    }

    #[test]
    fn files_to_send_are_listed_under_the_reply_and_the_text_is_their_caption() {
        let colors = Colors::default();
        let users = std::collections::HashMap::new();
        let chats = crate::chats::Chats::default();
        let names = messages::Names {
            users: &users,
            chats: &chats,
        };
        let reply = Replied {
            id: 7,
            sender: crate::messages::Sender::User(2),
            outgoing: true,
            snippet: "the trail map".into(),
        };
        let file = |name: &str, size, kind| Attachment {
            path: std::path::PathBuf::new(),
            name: name.into(),
            size,
            kind,
            identity: Default::default(),
            image_id: 0,
        };
        let photo = Kind::Photo {
            width: 4,
            height: 3,
        };
        let mut attachments = vec![
            file("sunrise.jpg", 2_200_000, photo),
            file("route.gpx", 340 * 1024, Kind::File),
        ];
        let mut images = no_images();
        let mut draw = |attachments: &[Attachment], as_files| {
            let mut composer = TextArea::default();
            let rows = 2 + BAR_ROWS + attachment_rows(attachments) + 1;
            let mut terminal = Terminal::new(TestBackend::new(48, rows)).unwrap();
            terminal
                .draw(|f| {
                    let bar = Some(ComposerBar::Reply(&reply));
                    let area = f.area();
                    draw_composer(
                        f,
                        &mut composer,
                        bar,
                        attachments,
                        as_files,
                        &names,
                        area,
                        true,
                        &mut images,
                        false,
                        &colors,
                    );
                })
                .unwrap();
            let buf = terminal.backend().buffer().clone();
            (buffer_rows(&buf), buf)
        };

        let (rows, buf) = draw(&attachments, false);
        assert!(rows[1].contains("↩ Reply to"), "{}", rows[1]);
        // The clip is two columns wide, so its second cell reads as a space.
        assert!(
            rows[3].contains("📎  sunrise.jpg · Photo · 2.1 MB"),
            "{}",
            rows[3]
        );
        assert!(
            rows[4].contains("📎  route.gpx · File · 340 KB"),
            "{}",
            rows[4]
        );
        assert!(rows[5].contains("Add a caption…"), "{}", rows[5]);
        let clip = column(&rows[3], "📎");
        assert_eq!(buf[(clip, 3)].fg, colors.attach);

        // Ctrl-t: photos go as files.
        let (rows, _) = draw(&attachments, true);
        assert!(
            rows[3].contains("sunrise.jpg · File, uncompressed"),
            "{}",
            rows[3]
        );
        assert!(rows[4].contains("route.gpx · File · 340 KB"), "{}", rows[4]);

        attachments.extend((0..3).map(|i| file(&format!("{i}.png"), 1024, photo)));
        let (rows, _) = draw(&attachments, false);
        assert!(rows[4].contains("route.gpx"), "{}", rows[4]);
        assert!(
            rows[5].contains("📎  3 more: 0.png, 1.png, 2.png · 3.0 KB"),
            "the rest by name: {}",
            rows[5]
        );
    }

    #[test]
    fn photos_to_send_show_their_picture_over_their_names() {
        use crate::images::Key;
        use ratatui_image::picker::{Picker, ProtocolType};
        /// What the kitty protocol puts in every cell of an image.
        const KITTY_CELL: char = '\u{10EEEE}';

        let colors = Colors::default();
        let users = std::collections::HashMap::new();
        let chats = crate::chats::Chats::default();
        let names = messages::Names {
            users: &users,
            chats: &chats,
        };
        let photo = |name: &str, width, height, image_id| Attachment {
            path: format!("/photos/{name}").into(),
            name: name.into(),
            size: 1024,
            kind: Kind::Photo { width, height },
            identity: Default::default(),
            image_id,
        };
        let notes = Attachment {
            kind: Kind::File,
            image_id: -3,
            ..photo("notes.txt", 1, 1, -3)
        };
        let attachments = [
            photo("wide.jpg", 800, 600, -1),
            notes,
            photo("tall.jpg", 600, 800, -2),
        ];
        let mut picker = Picker::halfblocks();
        picker.set_protocol_type(ProtocolType::Kitty);
        let mut images = Images::new(picker, tokio::sync::mpsc::unbounded_channel().0);
        // At 10x20 px cells, four rows tall: 11 columns and 6.
        for (file_id, cols) in [(-1, 11), (-2, 6)] {
            let key = Key {
                file_id,
                cols,
                rows: 4,
                thumbnail: false,
                avatar: false,
            };
            let red = image::RgbImage::from_pixel(cols.into(), 8, image::Rgb([255, 0, 0]));
            images.insert_ready(key, red.into());
        }
        assert_eq!(preview_rows(&attachments, &images), 4);
        let mut draw = |width| {
            let mut composer = TextArea::default();
            let rows = 2 + 4 + attachment_rows(&attachments) + 1;
            let mut terminal = Terminal::new(TestBackend::new(width, rows)).unwrap();
            terminal
                .draw(|f| {
                    let area = f.area();
                    draw_composer(
                        f,
                        &mut composer,
                        None,
                        &attachments,
                        false,
                        &names,
                        area,
                        true,
                        &mut images,
                        false,
                        &colors,
                    );
                })
                .unwrap();
            terminal.backend().buffer().clone()
        };
        let picture = |buf: &ratatui::buffer::Buffer, y| -> Vec<u16> {
            (0..buf.area.width)
                .filter(|&x| buf[(x, y)].symbol().contains(KITTY_CELL))
                .collect()
        };
        // By cell: a picture's cells hold more than one character each.
        let plus = |buf: &ratatui::buffer::Buffer, y| {
            (0..buf.area.width).find(|&x| buf[(x, y)].symbol() == "+")
        };

        // Inside the border and padding, side by side, a column apart. The
        // file has none.
        let buf = draw(48);
        let rows = buffer_rows(&buf);
        let wide: Vec<u16> = (2..13).collect();
        let tall: Vec<u16> = (14..20).collect();
        for y in 1..5 {
            assert_eq!(picture(&buf, y), [wide.clone(), tall.clone()].concat());
        }
        assert!(rows[5].contains("wide.jpg · Photo"), "{}", rows[5]);
        assert!(rows[6].contains("notes.txt · File"), "{}", rows[6]);
        assert!(rows[8].contains("Add a caption…"), "{}", rows[8]);

        // Too narrow for both: the one that doesn't fit is counted.
        let buf = draw(4 + 15);
        let rows = buffer_rows(&buf);
        assert_eq!(picture(&buf, 1), wide);
        assert!(rows[3].contains("+1"), "{}", rows[3]);
        assert_eq!(plus(&buf, 3), Some(14));

        // Too narrow for the count beside the first: it gives way.
        let buf = draw(4 + 12);
        let rows = buffer_rows(&buf);
        assert!(picture(&buf, 1).is_empty());
        assert!(rows[3].contains("+2"), "{}", rows[3]);
        assert_eq!(plus(&buf, 3), Some(2));
    }

    #[test]
    fn tab_in_the_attach_prompt_lists_what_matched() {
        let colors = Colors::default();
        let names: Vec<String> = (0..14).map(|i| format!("photo-{i:02}.jpg")).collect();
        let mut names = names;
        names.insert(0, "photos/".into());
        let mut terminal = Terminal::new(TestBackend::new(60, 20)).unwrap();
        terminal
            .draw(|f| draw_completions(f, f.area(), &names, &colors))
            .unwrap();
        let rows = buffer_rows(terminal.backend().buffer());
        // Twelve names and a count of the rest, at the bottom.
        assert!(rows[5].contains("Matches"), "{}", rows[5]);
        assert!(rows[6].contains("photos/"), "{}", rows[6]);
        assert!(rows[17].contains("photo-10.jpg"), "{}", rows[17]);
        assert!(rows[18].contains("… 3 more"), "{}", rows[18]);
        let buf = terminal.backend().buffer();
        assert_eq!(
            buf[(column(&rows[6], "photos/"), 6)].fg,
            colors.primary,
            "folders stand out"
        );
    }

    #[test]
    fn deleting_shows_the_message_and_only_the_choices_telegram_allows() {
        use crate::tg::Deletable;
        let colors = Colors::default();
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
    fn the_reaction_popup_marks_the_cursor_and_yours_and_searches_by_name() {
        let colors = Colors::default();
        let mut menu = ReactMenu::new(5, "see you at 7".into());
        let render = |menu: &mut ReactMenu| {
            let mut terminal = Terminal::new(TestBackend::new(60, 16)).unwrap();
            terminal
                .draw(|f| draw_react(f, f.area(), menu, &["❤"], &colors))
                .unwrap();
            terminal.backend().buffer().clone()
        };
        let has = |rows: &[String], needle: &str| rows.iter().any(|r| r.contains(needle));

        let rows = buffer_rows(&render(&mut menu));
        assert!(has(&rows, " React "));
        assert!(has(&rows, "▎ see you at 7"), "says which message");
        assert!(has(&rows, "Loading…"), "until TDLib answers");
        assert!(
            has(&rows, " X removes all yours, also in the chat"),
            "you have ❤"
        );

        let emoji = ["👍", "👎", "❤", "🔥", "🥰", "👏", "😁", "🤔", "💔", "😍"];
        menu.set_choices(emoji.map(String::from).to_vec(), &[]);
        menu.move_by(3);
        let buf = render(&mut menu);
        let rows = buffer_rows(&buf);
        assert!(has(&rows, "[🔥"), "the cursor is in brackets");
        assert!(has(&rows, " fire  :fire:"), "and named under the grid");
        let (x, y) = (0..buf.area.height)
            .find_map(|y| {
                let x = (0..buf.area.width).find(|&x| buf[(x, y)].symbol() == "❤\u{FE0F}")?;
                Some((x, y))
            })
            .expect("❤ drawn two columns wide");
        assert_eq!(buf[(x, y)].bg, colors.your_reaction, "yours is filled in");

        menu.edit_query(|q| q.push_str("heart"));
        let rows = buffer_rows(&render(&mut menu));
        assert!(has(&rows, " / heart"));
        assert!(!has(&rows, " X removes"), "X is typed while searching");
        assert!(has(&rows, " red heart  :heart:"));
        assert!(has(&rows, "Enter take back · Esc back"), "it's yours");
        assert!(!has(&rows, "👍"), "{rows:#?}");

        menu.edit_query(|q| q.push('z'));
        assert!(has(
            &buffer_rows(&render(&mut menu)),
            "No emoji by that name"
        ));
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
            styles: Vec::new(),
            revealed: false,
            forwarded: None,
            poll: None,
            card: None,
            state: SendState::Sent,
            reply_to,
            editable: Editable::Text,
            formatted: false,
            edited: false,
            album: 0,
            reactions: Vec::new(),
        };
        let mut open = OpenChat::new(1);
        open.messages.insert(1, msg(None));
        let to_first = ReplyTo {
            message_id: Some(1),
            quote: None,
        };
        open.messages.insert(2, msg(Some(to_first)));

        assert_eq!(
            jump_hints(&open),
            ["`gd` go to replied"],
            "newest is a reply"
        );
        open.selected = Some(1);
        assert!(jump_hints(&open).is_empty());
        open.jumps.push(2);
        assert_eq!(jump_hints(&open), ["`Ctrl-o` back to reply"]);
    }

    #[test]
    fn first_run_asks_for_api_credentials_and_says_where_to_get_them() {
        let colors = Colors::default();
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
        let colors = Colors::default();
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
        let colors = Colors::default();
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
        let colors = Colors::default();
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
    fn popups_write_in_the_themes_text_color_whatever_the_terminals_is() {
        for theme in Themes::built_in().list {
            let colors = theme.colors.unwrap();
            let mut menu = SettingsMenu {
                selected: 0,
                ..SettingsMenu::new(HelpTab::Settings, &Settings::default())
            };
            let settings = Settings {
                theme: theme.id.clone(),
                ..Settings::default()
            };
            let mut terminal = Terminal::new(TestBackend::new(70, 30)).unwrap();
            terminal
                .draw(|f| draw_settings(f, &mut menu, &settings, &Themes::built_in(), &colors))
                .unwrap();
            let buf = terminal.backend().buffer();
            let rows = buffer_rows(buf);
            let y = rows
                .iter()
                .position(|r| r.contains("A gap between chats"))
                .unwrap();
            let x = (0..buf.area.width)
                .find(|&x| buf[(x, y as u16)].symbol() == "A")
                .unwrap();
            assert_eq!(buf[(x, y as u16)].fg, colors.fg, "{}", theme.id);
        }
    }

    #[test]
    fn keys_in_hints_stand_out_from_what_they_do() {
        let colors = Colors::default();
        let muted = Style::new().fg(colors.muted);
        let spans = hint_spans(
            "  `Alt-Enter` or `Ctrl-j` new line · `/` search",
            muted,
            &colors,
        );
        let parts: Vec<(&str, bool)> = spans
            .iter()
            .map(|s| (s.content.as_ref(), s.style.fg == Some(colors.accent)))
            .collect();
        assert_eq!(
            parts,
            [
                ("  ", false),
                ("Alt-Enter", true),
                (" or ", false),
                ("Ctrl-j", true),
                (" new line · ", false),
                ("/", true),
                (" search", false),
            ]
        );
        let plain = hint_spans("Sending…", muted, &colors);
        assert_eq!(plain.len(), 1);
        assert_eq!(plain[0].style, muted);
    }

    #[test]
    fn truncate_never_goes_over_with_emoji_sequences() {
        let hearts = "❤️".repeat(100);
        for max in [1, 2, 3, 10, 51] {
            let cut = truncate(&hearts, max);
            assert!(cut.width() <= max, "{max}: {cut:?} is {}", cut.width());
        }
        assert_eq!(truncate("short", 10), "short");
        // Hearts, then thousands of zero-width selectors: cut quickly.
        let crafted = "❤️".repeat(50) + &"\u{fe0f}".repeat(4000);
        let started = std::time::Instant::now();
        let cut = truncate(&crafted, 71);
        assert!(cut.width() <= 71 && cut.ends_with('…'));
        // Quadratic work took seconds; a loaded machine still does this in far
        // less than one.
        assert!(started.elapsed() < std::time::Duration::from_secs(1));
        assert_eq!(truncate("abcdef", 4), "abc…");
    }

    #[test]
    fn a_long_host_in_a_link_warning_keeps_its_end() {
        let colors = Colors::default();
        let host = "bank.com.secure-session-verification-portal-login-account.evil.example";
        let url = format!("https://{host}/");
        let mut confirm = Confirm::new(
            "Open this link?",
            vec![
                "The text says: bank.com".into(),
                format!("Full address:  {url}"),
            ],
            crate::app::Confirmed::OpenLink(url),
        );
        confirm.site = Some(host.into());
        for width in [60, 78, 80] {
            let mut terminal = Terminal::new(TestBackend::new(width, 12)).unwrap();
            terminal
                .draw(|f| draw_confirm(f, f.area(), &confirm, &colors))
                .unwrap();
            let rows = buffer_rows(terminal.backend().buffer());
            let site = rows.iter().find(|r| r.contains("It goes to:")).unwrap();
            assert!(site.contains("evil.example"), "{width}: {site}");
        }
        assert_eq!(truncate_start("abcdef", 4), "…def");
        assert_eq!(truncate_start("abc", 4), "abc");
    }

    #[test]
    fn a_disguised_link_asks_first_and_shows_where_it_goes() {
        let colors = Colors::default();
        let mut confirm = Confirm::new(
            "Open this link?",
            vec![
                "The text says: bank.com".into(),
                "Full address:  https://evil.example/login".into(),
            ],
            crate::app::Confirmed::OpenLink("https://evil.example/login".into()),
        );
        confirm.site = Some("evil.example".into());
        let mut terminal = Terminal::new(TestBackend::new(80, 12)).unwrap();
        terminal
            .draw(|f| draw_confirm(f, f.area(), &confirm, &colors))
            .unwrap();
        let buf = terminal.backend().buffer();
        let rows = buffer_rows(buf);
        let row = |needle: &str| rows.iter().position(|r| r.contains(needle));
        assert!(row("Open this link?").is_some());
        assert!(row("The text says: bank.com").is_some());
        assert!(row("It goes to:    evil.example").is_some());
        assert!(row("Full address:  https://evil.example/login").is_some());
        let keys = row("y open · Esc cancel").expect("says how to answer");
        let x = column(&rows[keys], "y open");
        assert_eq!(
            buf[(x - 2, keys as u16)].fg,
            colors.warning,
            "warning border"
        );
    }

    #[test]
    fn telegrams_verdict_on_a_chat_to_join_cant_be_pushed_out_by_a_long_name() {
        let colors = Colors::default();
        let padded = format!("Official Support{} ✓", "\u{2800}".repeat(120));
        let mut confirm = Confirm::new(
            "Join this group?",
            vec![padded, "A group with 5000 members.".into()],
            crate::app::Confirmed::JoinLink {
                link: "https://t.me/+x".into(),
                request: "https://t.me/+x".into(),
            },
        );
        confirm.badge = Some(Badge::Scam);
        let mut terminal = Terminal::new(TestBackend::new(60, 10)).unwrap();
        terminal
            .draw(|f| draw_confirm(f, f.area(), &confirm, &colors))
            .unwrap();
        let buf = terminal.backend().buffer();
        let rows = buffer_rows(buf);
        let verdict = rows
            .iter()
            .position(|r| r.contains("Telegram marks it as a SCAM"))
            .expect("its own line");
        let name = rows
            .iter()
            .position(|r| r.contains("Official Support"))
            .unwrap();
        assert!(verdict < name, "before the name: {rows:#?}");
        let x = column(&rows[verdict], "SCAM");
        assert_eq!(buf[(x, verdict as u16)].fg, colors.error);
        let keys = rows.iter().position(|r| r.contains("y join")).unwrap();
        let x = column(&rows[keys], "y join");
        assert_eq!(buf[(x - 2, keys as u16)].fg, colors.error, "red border");
    }

    #[test]
    fn highlight_marks_every_match() {
        let colors = Colors::default();
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
        let colors = Colors::default();
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
    fn settings_list_every_theme_last_and_mark_the_saved_one() {
        // On Latte while Mocha is saved.
        let mut menu = SettingsMenu {
            selected: SettingsMenu::THEMES,
            ..SettingsMenu::new(HelpTab::Settings, &Settings::default())
        };
        let themes = Themes::built_in();
        let colors = themes.colors("latte").unwrap();
        let mut terminal = Terminal::new(TestBackend::new(60, 40)).unwrap();
        terminal
            .draw(|f| draw_settings(f, &mut menu, &Settings::default(), &themes, &colors))
            .unwrap();
        let buf = terminal.backend().buffer();
        let rows = buffer_rows(buf);
        let at = |needle: &str| rows.iter().position(|r| r.contains(needle)).unwrap();
        let row = |needle: &str| &rows[at(needle)];

        assert!(row("Settings").contains("Settings"));
        assert!(
            at("Theme") > at("Back to Normal mode"),
            "after the checkboxes"
        );
        assert!(row("Catppuccin Latte").contains("▌ ○"), "cursor on Latte");
        assert!(row("Catppuccin Frappé").contains("○"));
        assert!(row("Catppuccin Macchiato").contains("○"));
        assert!(row("Catppuccin Mocha").contains("●"), "Mocha is saved");
        for theme in [
            "Tokyo Night",
            "Dracula",
            "Gruvbox Dark",
            "Nord",
            "Rosé Pine",
        ] {
            assert!(row(theme).contains("○"), "{theme}");
        }

        // The popup paints its own background, so it covers what's below.
        let y = rows.iter().position(|r| r.contains("Frappé")).unwrap() as u16;
        let x = (0..buf.area.width)
            .find(|&x| buf[(x, y)].symbol() == "C")
            .unwrap();
        assert_eq!(buf[(x, y)].bg, colors.popup_bg);
    }

    #[test]
    fn the_settings_scroll_to_the_cursor_and_dim_a_theme_that_cant_be_used() {
        let dir = std::env::temp_dir().join(format!("tuigram-popup-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("zz-broken.toml"), "inherits = 3").unwrap();
        let themes = Themes::load(&dir);
        std::fs::remove_dir_all(&dir).unwrap();
        let colors = Colors::default();
        let mut menu = SettingsMenu::new(HelpTab::Settings, &Settings::default());
        // Too short for every row.
        let mut terminal = Terminal::new(TestBackend::new(70, 16)).unwrap();
        let mut draw = |menu: &mut SettingsMenu| {
            terminal
                .draw(|f| draw_settings(f, menu, &Settings::default(), &themes, &colors))
                .unwrap();
            terminal.backend().buffer().clone()
        };
        let has = |rows: &[String], needle: &str| rows.iter().any(|r| r.contains(needle));

        let rows = buffer_rows(&draw(&mut menu));
        assert!(has(&rows, "Notifications") && !has(&rows, "Rosé Pine"));

        // On the last theme, the lines after it show too.
        menu.selected = SettingsMenu::THEMES + themes.list.len() - 1;
        let buf = draw(&mut menu);
        let rows = buffer_rows(&buf);
        assert!(
            has(&rows, "▌ ○ zz-broken"),
            "the user's after the built-in ones"
        );
        assert!(has(&rows, "Your own themes go in"));
        assert!(!has(&rows, "Notifications"), "scrolled past");
        let y = rows.iter().position(|r| r.contains("zz-broken")).unwrap();
        let x = column(&rows[y], "zz-broken");
        assert_eq!(buf[(x, y as u16)].fg, colors.muted, "dimmed");

        // Back up on the first theme, its heading shows above it.
        menu.selected = SettingsMenu::THEMES;
        let rows = buffer_rows(&draw(&mut menu));
        let at = |needle: &str| rows.iter().position(|r| r.contains(needle)).unwrap();
        assert_eq!(at("Catppuccin Latte"), at("Theme") + 1);
        assert!(!has(&rows, "Notifications"));
    }

    #[test]
    fn notifications_show_as_a_checkbox() {
        let colors = Colors::default();
        let mut menu = SettingsMenu {
            selected: SettingsMenu::NOTIFICATIONS,
            ..SettingsMenu::new(HelpTab::Settings, &Settings::default())
        };
        let mut terminal = Terminal::new(TestBackend::new(70, 14)).unwrap();
        let mut draw = |notifications| {
            let settings = Settings {
                notifications,
                ..Settings::default()
            };
            terminal
                .draw(|f| draw_settings(f, &mut menu, &settings, &Themes::built_in(), &colors))
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
            row(&rows, "Space on/off").contains("Esc close"),
            "key hints"
        );

        let rows = draw(Notifications::Off);
        assert!(row(&rows, "New messages").contains("▌ [ ] New messages"));
    }

    #[test]
    fn gaps_and_normal_mode_after_sending_are_checkboxes() {
        let colors = Colors::default();
        let mut menu = SettingsMenu {
            selected: SettingsMenu::AFTER_SEND,
            ..SettingsMenu::new(HelpTab::Settings, &Settings::default())
        };
        let mut terminal = Terminal::new(TestBackend::new(70, 22)).unwrap();
        let mut draw = |gaps, normal_after_send| {
            let settings = Settings {
                chat_gaps: gaps,
                block_gaps: gaps,
                normal_after_send,
                ..Settings::default()
            };
            terminal
                .draw(|f| draw_settings(f, &mut menu, &settings, &Themes::built_in(), &colors))
                .unwrap();
            buffer_rows(terminal.backend().buffer())
        };
        let row = |rows: &[String], needle: &str| {
            rows.iter().find(|r| r.contains(needle)).unwrap().clone()
        };

        let rows = draw(true, false);
        assert!(row(&rows, "Chat list").contains("Chat list"));
        assert!(row(&rows, "A gap between chats").contains("  [✓] A gap between chats"));
        assert!(row(&rows, "Messages").contains("Messages"));
        assert!(row(&rows, "A gap between messages").contains("  [✓] A gap between messages"));
        assert!(row(&rows, "Composer").contains("Composer"));
        assert!(row(&rows, "Back to Normal").contains("▌ [ ] Back to Normal mode after sending"));
        assert!(row(&rows, "Space on/off").contains("Esc close"));

        let rows = draw(false, true);
        assert!(row(&rows, "A gap between chats").contains("[ ] A gap between chats"));
        assert!(row(&rows, "A gap between messages").contains("[ ] A gap between"));
        assert!(row(&rows, "Back to Normal").contains("▌ [✓] Back to Normal mode"));
    }
}

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::process::{self, Stdio};
use std::time::Duration;

use anyhow::Result;
use crossterm::event::{Event, EventStream, KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
use futures::StreamExt;
use ratatui::DefaultTerminal;
use ratatui::style::Style;
use ratatui::widgets::Block;
use ratatui_textarea::TextArea;
use tdlib_rs::enums::{
    AuthenticationCodeType, AuthorizationState, ChatList, MessageSender, NotificationType,
    OptionValue, Update,
};
use tdlib_rs::types::{Message, UpdateNotificationGroup};
use tokio::sync::mpsc::UnboundedReceiver;
use tokio::time::{Instant, sleep_until};

use crate::attach::{self, Attachment, Dropped};
use crate::chats::Chats;
use crate::clipboard::{Clipboard, ClipboardEvent, Copied, Decoded, Paste, Pasted};
use crate::config::{self, ApiKeys};
use crate::images::{ImageEvent, Images};
use crate::messages::{Editable, Editing, Link, MediaFile, OpenChat, Replied, SendState};
use crate::notify::{self, Note, Notifications, Notifier};
use crate::reactions::{self, ReactMenu, ReactionKind};
use crate::search::MessageSearch;
use crate::settings::{self, Settings, Side};
use crate::stickers::{self, Source, StickerPanel};
use crate::text;
use crate::tg::{Deletable, Found, Page, Tagged, Tg, TgEvent};
use crate::theme::{Colors, Themes};
use crate::ui;

/// Chats requested per `loadChats` call.
const CHAT_PAGE: i32 = 50;
/// Messages requested per `getChatHistory` call.
const HISTORY_PAGE: i32 = 50;
/// Search results requested per `searchChatMessages` call.
const SEARCH_PAGE: i32 = 50;
/// Keep requesting history until at least this many messages are loaded.
/// TDLib often answers the first request with only what it has cached.
const MIN_LOADED: usize = 30;
/// Start loading the next page when the cursor gets this close to the end.
const LOAD_AHEAD: usize = 10;
/// Rows moved by Ctrl-d / Ctrl-u.
const HALF_PAGE: isize = 10;
/// How long to wait for TDLib to flush its database on quit.
const CLOSE_TIMEOUT: Duration = Duration::from_secs(3);
/// How long a toast stays up.
const TOAST_TIME: Duration = Duration::from_secs(2);
/// Without a key press for this long, you're no longer shown as online.
const IDLE_AFTER: Duration = Duration::from_secs(60);
/// While typing, the chat is told again this often: others' apps stop
/// showing it after 5.5 s without a repeat.
const TYPING_EVERY: Duration = Duration::from_secs(5);

/// Where the API key TDLib was given came from.
#[derive(Clone, Copy, PartialEq, Eq)]
enum KeySource {
    Env,
    Saved,
    BuiltIn,
}

pub enum Screen {
    Login(Box<Login>),
    Main,
}

pub enum LoginStep {
    Connecting,
    LoggingOut,
    /// First run without API credentials: ask for them, ID then hash.
    ApiId,
    ApiHash {
        id: i32,
    },
    Phone,
    Code {
        sent_via: &'static str,
    },
    Password {
        hint: String,
    },
    Email,
    EmailCode,
    OtherDevice {
        link: String,
    },
    /// A state this client can't finish; the text says what to do instead.
    Unsupported(&'static str),
}

pub struct Login {
    pub step: LoginStep,
    pub input: TextArea<'static>,
    pub error: Option<String>,
    /// A request is in flight; Enter is ignored until TDLib answers.
    pub busy: bool,
}

impl Login {
    pub fn new(step: LoginStep) -> Self {
        let mut input = TextArea::default();
        input.set_block(Block::bordered());
        input.set_cursor_line_style(Style::default());
        if let LoginStep::Password { .. } = step {
            input.set_mask_char('•');
        }
        Self {
            step,
            input,
            error: None,
            busy: false,
        }
    }

    pub fn takes_input(&self) -> bool {
        !matches!(
            self.step,
            LoginStep::Connecting
                | LoginStep::LoggingOut
                | LoginStep::OtherDevice { .. }
                | LoginStep::Unsupported(_)
        )
    }
}

/// Something in a message that Enter opens or `y` copies.
pub enum Target {
    File(MediaFile),
    Link(Link),
    /// The whole text or caption; only copied.
    Text(String),
}

impl Target {
    pub fn label(&self) -> &str {
        match self {
            Target::File(file) => &file.label,
            Target::Link(link) => &link.url,
            Target::Text(_) => "Whole message",
        }
    }
}

/// Asks before opening something that could hurt: a file that may run code,
/// or a link whose words say something other than where it goes.
pub struct Confirm {
    pub title: String,
    /// Why it asks, and what exactly would open.
    pub lines: Vec<String>,
    pub action: Confirmed,
}

/// What `y` does in a [`Confirm`].
pub enum Confirmed {
    OpenFile(String),
    OpenLink(String),
    /// Edit this message, losing its formatting.
    Edit(i64),
}

impl Confirmed {
    /// What `y` does, for the key hints.
    pub fn verb(&self) -> &'static str {
        match self {
            Confirmed::OpenFile(_) | Confirmed::OpenLink(_) => "open",
            Confirmed::Edit(_) => "edit",
        }
    }
}

/// File types that open in a viewer or player, never as a program. Anything
/// else asks first: `.exe`, `.bat`, `.command`, `.jar`, `.terminal`, `.html`
/// and many more can run code when opened.
const SAFE_TO_OPEN: &[&str] = &[
    "jpg", "jpeg", "png", "gif", "webp", "heic", "heif", "avif", "bmp", "tif", "tiff", "mp4",
    "m4v", "mov", "mkv", "webm", "avi", "3gp", "mpg", "mpeg", "mp3", "m4a", "aac", "ogg", "oga",
    "opus", "wav", "flac", "pdf", "txt", "md", "epub", "docx", "xlsx", "pptx", "odt", "ods", "odp",
    "zip", "rar", "7z", "tar", "gz", "tgz",
];

/// Whether a file opens in a viewer or player, never as a program; see
/// [`SAFE_TO_OPEN`].
fn safe_to_open(path: &str) -> bool {
    Path::new(path)
        .extension()
        .and_then(|e| e.to_str())
        .is_some_and(|e| SAFE_TO_OPEN.contains(&e.to_ascii_lowercase().as_str()))
}

/// What picking from a [`PickMenu`] does.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum MenuAction {
    Open,
    Copy,
}

/// Menu for picking what to open or copy when a message holds several things.
pub struct PickMenu {
    pub action: MenuAction,
    pub targets: Vec<Target>,
    pub selected: usize,
}

/// A note in the corner that something worked, gone after [`TOAST_TIME`].
pub struct Toast {
    pub title: String,
    /// What it was about, e.g. what got copied.
    pub detail: String,
    pub until: Instant,
}

/// One way to delete a message.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DeleteChoice {
    Everyone,
    OnlyMe,
}

impl DeleteChoice {
    pub fn label(self) -> &'static str {
        match self {
            DeleteChoice::Everyone => "Delete for everyone",
            DeleteChoice::OnlyMe => "Delete for me",
        }
    }
}

/// The `d` popup, confirming which message goes and for whom.
pub struct DeleteMenu {
    pub message_id: i64,
    /// The message on one line, so it's clear which one is deleted.
    pub snippet: String,
    /// What TDLib allows, for everyone first as Telegram lists it. Empty
    /// until TDLib answers.
    pub choices: Vec<DeleteChoice>,
    pub selected: usize,
}

impl DeleteMenu {
    pub fn set_allowed(&mut self, deletable: Deletable) {
        self.choices = [
            (deletable.for_everyone, DeleteChoice::Everyone),
            (deletable.for_me, DeleteChoice::OnlyMe),
        ]
        .into_iter()
        .filter_map(|(allowed, choice)| allowed.then_some(choice))
        .collect();
        // The cursor starts on the choice that's easy to live with, as in
        // Telegram, and also lands there if keys typed while TDLib was still
        // answering went nowhere.
        self.selected = self
            .choices
            .iter()
            .position(|&c| c == DeleteChoice::OnlyMe)
            .unwrap_or(0);
    }
}

/// The tabs of the `?` popup.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum HelpTab {
    Shortcuts,
    Settings,
}

/// The `?` popup: every keyboard shortcut, and the settings. On the settings
/// tab, Enter or Space ticks a checkbox (notifications, gaps in the chat list
/// and in message blocks, Normal mode after sending) or picks a theme, and
/// saves it at once.
pub struct SettingsMenu {
    pub tab: HelpTab,
    /// First row shown on the shortcuts tab. Drawing keeps it in range.
    pub scroll: usize,
    /// Row on the settings tab: one of the checkboxes, then the themes from
    /// [`SettingsMenu::THEMES`] on.
    pub selected: usize,
    /// First line shown on the settings tab. Drawing moves it to show the
    /// selected row.
    pub settings_scroll: usize,
    /// How notifications went out when the popup opened, so turning them
    /// off and on again keeps the way, e.g. "bell".
    pub saved_notifications: Notifications,
}

impl SettingsMenu {
    /// The notifications row, the first one.
    pub const NOTIFICATIONS: usize = 0;
    /// The "gap between chats" row.
    pub const CHAT_GAPS: usize = Self::NOTIFICATIONS + 1;
    /// The "chat list on the right" row.
    pub const LIST_RIGHT: usize = Self::CHAT_GAPS + 1;
    /// The "gap between messages" row.
    pub const BLOCK_GAPS: usize = Self::LIST_RIGHT + 1;
    /// The "Normal mode after sending" row.
    pub const AFTER_SEND: usize = Self::BLOCK_GAPS + 1;
    /// The first theme's row. The themes come last, since the user's own
    /// can make a long list.
    pub const THEMES: usize = Self::AFTER_SEND + 1;

    /// Opens on `tab` with the cursor on the first setting.
    pub fn new(tab: HelpTab, settings: &Settings) -> Self {
        Self {
            tab,
            scroll: 0,
            selected: 0,
            settings_scroll: 0,
            saved_notifications: settings.notifications,
        }
    }
}

/// What the status bar prompt is for: a `/` search through chat titles or the
/// open chat's messages, a `:` command, or the path of a file to attach (`a`).
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum PromptKind {
    Chats,
    Messages,
    Command,
    Attach,
}

/// The prompt in the status bar. Searching chats filters the list as you
/// type; searching messages asks TDLib on Enter, and so does a command.
pub struct Prompt {
    pub kind: PromptKind,
    pub input: TextArea<'static>,
    /// What the last Tab found in the attach prompt, when it was more than
    /// one name. Typing clears it.
    pub completions: Vec<String>,
    /// The chat filter and cursor from before, which Esc puts back.
    previous_filter: String,
    previous_selected: Option<i64>,
}

impl Prompt {
    pub fn query(&self) -> String {
        self.input.lines().concat()
    }
}

/// What `:` runs. There are no abbreviations: only the full name runs, so a
/// typo can't log you out.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Command {
    Logout,
}

impl Command {
    pub const ALL: [Command; 1] = [Command::Logout];

    pub fn name(self) -> &'static str {
        match self {
            Command::Logout => "logout",
        }
    }

    pub fn about(self) -> &'static str {
        match self {
            Command::Logout => "Log out of Telegram on this computer",
        }
    }

    pub fn parse(text: &str) -> Option<Command> {
        Command::ALL.into_iter().find(|c| c.name() == text)
    }
}

/// Where keys go. `Input` is Insert mode; the others are Normal mode.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Focus {
    Chats,
    Messages,
    Input,
}

pub struct App {
    tg: Tg,
    pub screen: Screen,
    pub focus: Focus,
    pub chats: Chats,
    /// Display names by user id, for message senders in groups.
    pub users: HashMap<i64, String>,
    /// Selected chat id, not index, so the cursor stays put when chats reorder.
    pub selected: Option<i64>,
    pub open: Option<OpenChat>,
    /// The message being written. Cleared when switching chats.
    pub composer: TextArea<'static>,
    pub images: Images,
    /// Files being downloaded to open in their default app when done.
    pub opening: HashSet<i32>,
    clipboard: Clipboard,
    /// Files being downloaded to copy when done, by file id.
    pub copying: HashMap<i32, MediaFile>,
    /// The clipboard is being read for `p`.
    pub pasting: bool,
    pub toast: Option<Toast>,
    /// Shown over everything when Enter or `y` finds several things.
    pub menu: Option<PickMenu>,
    pub delete_menu: Option<DeleteMenu>,
    pub react_menu: Option<ReactMenu>,
    /// Opened with Tab while writing; only open in Insert mode.
    pub stickers: Option<StickerPanel>,
    /// In resize mode (Ctrl-r): the chat list's width before, which Esc
    /// puts back.
    pub resizing: Option<u16>,
    pub confirm: Option<Confirm>,
    pub settings: Settings,
    settings_path: PathBuf,
    /// Every theme, read again whenever `?` opens, so changes to a file show.
    pub themes: Themes,
    /// The colors of the theme in use.
    pub colors: Colors,
    /// You have Telegram Premium, so Premium stickers can be sent.
    premium: bool,
    /// API credentials from the environment, which win over saved ones.
    env_keys: Option<ApiKeys>,
    /// The key TDLib has; `None` until it gets one.
    keys_source: Option<KeySource>,
    /// Telegram refused the built-in key, so it isn't offered to TDLib again.
    built_in_rejected: bool,
    pub settings_menu: Option<SettingsMenu>,
    /// Shown in the status bar while typing a `/` search.
    pub prompt: Option<Prompt>,
    pub chats_loading: bool,
    all_chats_loaded: bool,
    /// Last error, shown in the status bar until the next key press.
    pub status: Option<String>,
    /// First `g` of `gg` was pressed.
    pending_g: bool,
    /// Set once quitting started; we exit at this time even if TDLib never answers.
    pub quit_deadline: Option<Instant>,
    /// The terminal window has focus. Terminals that don't report focus
    /// changes leave this on.
    terminal_focused: bool,
    /// The terminal has reported a focus change, so `terminal_focused` can
    /// be trusted.
    focus_reported: bool,
    /// The last key press or paste, or the window getting focus.
    last_input: Instant,
    /// What TDLib was last told: shown as online to others.
    online: bool,
    /// The chat last told you're typing, and when. `None` once it was told
    /// you stopped.
    typing: Option<(i64, Instant)>,
    /// Tells the user about new messages while they're away from tuigram.
    notifier: Notifier,
    /// How notifications reach this terminal (never `Auto`).
    notify_with: Notifications,
    /// Inside tmux, which passes codes on only when they're wrapped.
    in_tmux: bool,
    /// Counts notifications sent, to tell them apart.
    notifications_sent: u64,
    /// Messages from before this (unix time) aren't announced: they came in
    /// while tuigram wasn't running.
    notify_since: i32,
    /// Unmuted chats with unread messages, shown in the window title.
    unread_chats: i32,
    /// TDLib is logging out (`:logout`, the session ended elsewhere, or
    /// leaving a QR login), and a new client takes over once it has closed.
    relogin: bool,
    exit: bool,
}

impl App {
    pub fn new(
        tg: Tg,
        images: Images,
        clipboard: Clipboard,
        settings: Settings,
        settings_path: PathBuf,
        env_keys: Option<ApiKeys>,
    ) -> Self {
        let mut chats = Chats::default();
        chats.set_highlighted(&settings.highlighted_chats);
        let notify_with = settings
            .notifications
            .resolve(|name| std::env::var(name).ok());
        let mut app = Self {
            tg,
            screen: login_screen(LoginStep::Connecting),
            focus: Focus::Chats,
            chats,
            users: HashMap::new(),
            selected: None,
            open: None,
            composer: new_composer(),
            images,
            opening: HashSet::new(),
            clipboard,
            copying: HashMap::new(),
            pasting: false,
            toast: None,
            menu: None,
            delete_menu: None,
            react_menu: None,
            stickers: None,
            resizing: None,
            confirm: None,
            settings,
            themes: Themes::load(&settings_path.with_file_name("themes")),
            settings_path,
            colors: Colors::default(),
            premium: false,
            env_keys,
            keys_source: None,
            built_in_rejected: false,
            settings_menu: None,
            prompt: None,
            chats_loading: false,
            all_chats_loaded: false,
            status: None,
            pending_g: false,
            quit_deadline: None,
            terminal_focused: true,
            focus_reported: false,
            last_input: Instant::now(),
            online: false,
            typing: None,
            notifier: Notifier::default(),
            notify_with,
            in_tmux: std::env::var_os("TMUX").is_some(),
            notifications_sent: 0,
            notify_since: i32::MAX,
            unread_chats: 0,
            relogin: false,
            exit: false,
        };
        app.use_saved_theme();
        app
    }

    /// Uses the theme the settings name, or the default one, saying why, if
    /// it can't be used: a broken file never leaves the app unreadable. The
    /// setting stays, so fixing the file is enough.
    fn use_saved_theme(&mut self) {
        match self.themes.colors(&self.settings.theme) {
            Ok(colors) => self.colors = colors,
            Err(e) => {
                self.colors = Colors::default();
                self.status = Some(format!("Theme not used: {e}"));
            }
        }
    }

    pub async fn run(
        mut self,
        terminal: &mut DefaultTerminal,
        mut events: UnboundedReceiver<Tagged>,
        mut image_events: UnboundedReceiver<ImageEvent>,
        mut clipboard: UnboundedReceiver<ClipboardEvent>,
    ) -> Result<()> {
        let mut keys = EventStream::new();
        let mut signals = quit_signals();
        // What broke the terminal, if it went away: the app then closes as
        // `q` closes it, without drawing, and returns this.
        let mut failed: Option<anyhow::Error> = None;
        while !self.exit {
            if self
                .toast
                .as_ref()
                .is_some_and(|t| t.until <= Instant::now())
            {
                self.toast = None;
            }
            self.chats.refresh();
            if self
                .selected
                .is_none_or(|id| !self.chats.ids().contains(&id))
            {
                self.selected = self.chats.ids().first().copied();
            }
            self.mark_seen();
            self.update_online();
            self.send_notification();
            if failed.is_none()
                && let Err(e) = terminal.draw(|frame| ui::draw(frame, &mut self))
            {
                failed = Some(e.into());
                self.hang_up();
            }
            // Start downloads/encodes for photos the frame showed but didn't have.
            self.images.fetch(&self.tg);
            if let Some(open) = self.open.as_mut() {
                for id in open.missing_replied() {
                    self.tg.get_replied_message(open.chat_id, id);
                }
            }

            let deadline = self.quit_deadline;
            // Wakes up to take the toast down, to go offline when idle, and
            // to send notifications that had to wait.
            let wake = [
                self.toast.as_ref().map(|t| t.until),
                self.online.then_some(self.last_input + IDLE_AFTER),
                self.notifier.next_at(),
            ]
            .into_iter()
            .flatten()
            .min();
            tokio::select! {
                Some(event) = events.recv() => {
                    self.on_tagged(event);
                    // Drain the backlog so a burst of updates costs one redraw.
                    while let Ok(event) = events.try_recv() {
                        self.on_tagged(event);
                    }
                }
                Some(event) = image_events.recv() => self.images.on_built(event),
                Some(event) = clipboard.recv() => self.on_clipboard(event),
                Some(event) = keys.next(), if failed.is_none() => match event {
                    Ok(event) => self.on_terminal_event(event),
                    Err(e) => {
                        failed = Some(e.into());
                        self.hang_up();
                    }
                },
                Some(()) = signals.recv() => self.hang_up(),
                _ = sleep_until(deadline.unwrap_or_else(Instant::now)), if deadline.is_some() => break,
                _ = sleep_until(wake.unwrap_or_else(Instant::now)), if wake.is_some() => {}
                else => break,
            }
        }
        match failed {
            Some(e) => Err(e),
            None => Ok(()),
        }
    }

    /// The terminal is gone (a closed window, a dropped SSH connection), or
    /// tuigram was told to stop: nobody is reading any more, so nothing more
    /// is marked read, and it quits as `q` does, going offline first.
    fn hang_up(&mut self) {
        self.terminal_focused = false;
        self.focus_reported = true;
        if self.quit_deadline.is_none() {
            self.quit();
        }
    }

    fn on_terminal_event(&mut self, event: Event) {
        if matches!(event, Event::Key(_) | Event::Paste(_) | Event::FocusGained) {
            self.last_input = Instant::now();
        }
        match event {
            Event::Key(key) => self.on_key(key),
            Event::FocusGained => {
                self.terminal_focused = true;
                self.focus_reported = true;
                // Back at tuigram: what was waiting is on screen.
                self.notifier.clear();
            }
            Event::FocusLost => {
                self.terminal_focused = false;
                self.focus_reported = true;
            }
            Event::Paste(text) if matches!(self.screen, Screen::Login(_)) => {
                if let Screen::Login(login) = &mut self.screen
                    && login.takes_input()
                {
                    login.input.insert_str(text.trim());
                }
            }
            Event::Paste(text) if self.prompt.is_some() => {
                if let Some(prompt) = self.prompt.as_mut() {
                    prompt.input.insert_str(text.replace(['\r', '\n'], " "));
                }
                self.on_prompt_edit();
            }
            Event::Paste(text) if matches!(self.focus, Focus::Input | Focus::Messages) => {
                self.on_paste(text)
            }
            _ => {}
        }
    }

    /// Late events from a client replaced after logging out are dropped: a
    /// download or error from the old session would land in the new one.
    fn on_tagged(&mut self, (client_id, event): Tagged) {
        if client_id == self.tg.client_id() {
            self.on_tg(event);
        }
    }

    fn on_tg(&mut self, event: TgEvent) {
        match event {
            TgEvent::Update(update) => self.on_update(*update),
            TgEvent::Error(message) if message.contains("API_ID") => self.reject_api_keys(),
            TgEvent::Error(message) => match &mut self.screen {
                Screen::Login(login) => {
                    login.busy = false;
                    login.error = Some(message);
                }
                Screen::Main => self.status = Some(message),
            },
            TgEvent::ChatsLoaded { all, failed } => {
                self.chats_loading = false;
                self.all_chats_loaded |= all;
                // Unread chats go first wherever Telegram has them, so every
                // chat has to be loaded. After an error, scrolling to the end
                // of the list tries again.
                if !failed {
                    self.load_more_chats();
                }
            }
            TgEvent::History {
                chat_id,
                page,
                messages,
            } => self.on_history(chat_id, page, messages),
            TgEvent::Found {
                chat_id,
                query,
                found,
            } => self.on_found(chat_id, &query, found),
            TgEvent::Replied {
                chat_id,
                message_id,
                replied,
            } => {
                if let Some(open) = self.open.as_mut().filter(|o| o.chat_id == chat_id) {
                    open.set_replied(message_id, replied.map(|m| *m));
                }
            }
            TgEvent::Deletable {
                chat_id,
                message_id,
                deletable,
            } => self.on_deletable(chat_id, message_id, deletable),
            TgEvent::Editable {
                chat_id,
                message_id,
                editable,
            } => self.on_editable(chat_id, message_id, editable),
            TgEvent::Reactions {
                chat_id,
                message_id,
                available,
            } => self.on_available_reactions(chat_id, message_id, available),
            TgEvent::Stickers { source, stickers } => self.on_stickers(source, stickers),
            TgEvent::StickerSets(sets) => {
                if let Some(panel) = self.stickers.as_mut() {
                    panel.set_sets(sets.unwrap_or_default());
                    self.load_sticker_set();
                }
            }
            TgEvent::StickersFound { query, stickers } => {
                let premium = self.premium;
                if let Some(panel) = self.stickers.as_mut() {
                    // After an error, TDLib's message is in the status bar.
                    let found = stickers::convert(&stickers.unwrap_or_default(), premium);
                    panel.set_found(&query, found);
                }
            }
            TgEvent::Downloaded { file_id, path } => {
                if self.opening.remove(&file_id) {
                    match &path {
                        Some(path) => self.open_downloaded(path.clone()),
                        None => self.status = Some("Download failed".into()),
                    }
                }
                if let Some(file) = self.copying.remove(&file_id) {
                    match &path {
                        Some(path) => self.copy_downloaded(file, path),
                        None => self.status = Some("Download failed".into()),
                    }
                }
                self.images.on_downloaded(file_id, path);
            }
        }
    }

    fn on_history(
        &mut self,
        chat_id: i64,
        page: Page,
        messages: Option<Vec<tdlib_rs::types::Message>>,
    ) {
        // Ignore pages for a chat that was closed, or a request that was
        // replaced (e.g. by jumping elsewhere), while it was in flight.
        let Some(open) = self
            .open
            .as_mut()
            .filter(|o| o.chat_id == chat_id && o.loading == Some(page))
        else {
            return;
        };
        open.loading = None;
        let Some(messages) = messages else {
            return;
        };
        open.add_page(
            page,
            messages.into_iter().map(|m| (m.id, m.into())).collect(),
        );
        if open.messages.len() < MIN_LOADED {
            self.load_older_messages();
        }
    }

    fn on_found(&mut self, chat_id: i64, query: &str, found: Option<Found>) {
        // Ignore results for a search that was replaced or ended meanwhile.
        let Some(open) = self.open.as_mut().filter(|o| o.chat_id == chat_id) else {
            return;
        };
        let Some(search) = open
            .search
            .as_mut()
            .filter(|s| s.query == query && s.loading)
        else {
            return;
        };
        search.loading = false;
        let wanted = search.wanted.take();
        match found {
            Some(found) => {
                search.add(found);
                if let Some(index) = wanted {
                    self.go_to_match(index);
                }
            }
            // The error is in the status bar. Without a match, end the search.
            None if search.results.is_empty() => open.search = None,
            None => {}
        }
    }

    fn on_update(&mut self, update: Update) {
        match update {
            Update::AuthorizationState(u) => self.on_auth_state(u.authorization_state),
            Update::NewChat(u) => self.chats.insert(u.chat),
            Update::ChatPosition(u) => self.chats.set_position(u.chat_id, &u.position),
            Update::ChatLastMessage(u) => {
                self.chats
                    .set_last_message(u.chat_id, u.last_message.as_ref(), &u.positions)
            }
            Update::ChatTitle(u) => self.chats.set_title(u.chat_id, u.title),
            Update::ChatAction(u) => self.chats.set_action(u.chat_id, &u.sender_id, &u.action),
            Update::NotificationGroup(u) => self.on_notifications(u),
            Update::UnreadChatCount(u) if matches!(u.chat_list, ChatList::Main) => {
                self.set_unread_chats(u.unread_unmuted_count)
            }
            Update::ChatPhoto(u) => self.chats.set_photo(u.chat_id, u.photo.as_ref()),
            Update::ChatAccentColors(u) => self.chats.set_accent(u.chat_id, u.accent_color_id),
            Update::AccentColors(u) => self.chats.set_accent_colors(&u.colors),
            Update::ChatReadInbox(u) => self.chats.set_unread(u.chat_id, u.unread_count),
            Update::ChatReadOutbox(u) => {
                self.chats
                    .set_read_outbox(u.chat_id, u.last_read_outbox_message_id);
            }
            // TDLib drops the option, rather than setting it false, when
            // Premium ends.
            Update::Option(u) if u.name == "is_premium" => {
                self.premium = matches!(u.value, OptionValue::Boolean(v) if v.value);
            }
            Update::Option(u) if u.name == "my_id" => {
                if let OptionValue::Integer(v) = u.value {
                    self.chats.set_my_id(v.value);
                }
            }
            Update::User(u) => {
                let name = format!("{} {}", u.user.first_name, u.user.last_name);
                self.users.insert(u.user.id, text::clean(name.trim()));
            }
            Update::NewMessage(u) => {
                // While older messages are shown, new ones load with the rest.
                if let Some(open) = self
                    .open
                    .as_mut()
                    .filter(|o| o.chat_id == u.message.chat_id && o.at_newest)
                {
                    open.insert(u.message);
                }
            }
            Update::MessageSendSucceeded(u) => {
                if let Some(open) = self
                    .open
                    .as_mut()
                    .filter(|o| o.chat_id == u.message.chat_id)
                {
                    open.replace(u.old_message_id, u.message);
                }
            }
            Update::MessageSendFailed(u) => {
                self.status = Some(format!("Message not sent: {}", u.error.message));
                if let Some(open) = self
                    .open
                    .as_mut()
                    .filter(|o| o.chat_id == u.message.chat_id)
                {
                    open.replace(u.old_message_id, u.message);
                }
            }
            Update::File(u) => {
                if let Some(open) = self.open.as_mut() {
                    open.set_upload(&u.file);
                }
            }
            Update::MessageContent(u) => {
                if let Some(open) = self.open.as_mut().filter(|o| o.chat_id == u.chat_id) {
                    open.set_content(u.message_id, &u.new_content);
                }
            }
            Update::MessageEdited(u) => {
                if let Some(open) = self.open.as_mut().filter(|o| o.chat_id == u.chat_id) {
                    open.set_edited(u.message_id);
                }
            }
            Update::MessageInteractionInfo(u) => {
                if let Some(open) = self.open.as_mut().filter(|o| o.chat_id == u.chat_id) {
                    open.set_reactions(u.message_id, u.interaction_info.as_ref());
                }
            }
            Update::DeleteMessages(u) if u.is_permanent => {
                if let Some(open) = self.open.as_mut().filter(|o| o.chat_id == u.chat_id) {
                    open.remove(&u.message_ids);
                }
            }
            _ => {}
        }
    }

    fn on_auth_state(&mut self, state: AuthorizationState) {
        let step = match state {
            AuthorizationState::WaitTdlibParameters => match self.api_keys() {
                Some((source, keys)) => {
                    self.keys_source = Some(source);
                    self.tg.set_tdlib_parameters(keys);
                    return;
                }
                None => LoginStep::ApiId,
            },
            AuthorizationState::WaitPhoneNumber => LoginStep::Phone,
            AuthorizationState::WaitCode(s) => LoginStep::Code {
                sent_via: code_destination(&s.code_info.r#type),
            },
            AuthorizationState::WaitPassword(s) => LoginStep::Password {
                hint: s.password_hint,
            },
            AuthorizationState::WaitEmailAddress(_) => LoginStep::Email,
            AuthorizationState::WaitEmailCode(_) => LoginStep::EmailCode,
            AuthorizationState::WaitOtherDeviceConfirmation(s) => {
                LoginStep::OtherDevice { link: s.link }
            }
            AuthorizationState::WaitRegistration(_) => LoginStep::Unsupported(
                "This number has no Telegram account. Sign up in the official app first.",
            ),
            AuthorizationState::WaitPremiumPurchase(_) => LoginStep::Unsupported(
                "Telegram requires a Premium purchase to log in with this number. Use the official app.",
            ),
            AuthorizationState::Ready => {
                self.screen = Screen::Main;
                self.load_more_chats();
                // Even when they're off: they can be turned on any time.
                self.tg.enable_notifications();
                self.notify_since = unix_now();
                return;
            }
            // Also when the session was ended from another device: back to
            // the login screen instead of quitting.
            AuthorizationState::LoggingOut => {
                self.relogin = true;
                if let Screen::Login(_) = self.screen {
                    return;
                }
                LoginStep::LoggingOut
            }
            AuthorizationState::Closing => return,
            AuthorizationState::Closed if self.relogin && self.quit_deadline.is_none() => {
                self.relogin = false;
                self.keys_source = None;
                self.forget_session();
                self.tg.reopen();
                self.screen = login_screen(LoginStep::Connecting);
                return;
            }
            AuthorizationState::Closed => {
                self.exit = true;
                return;
            }
        };
        self.screen = login_screen(step);
    }

    fn on_key(&mut self, key: KeyEvent) {
        if key.kind != KeyEventKind::Press {
            return;
        }
        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
        if ctrl
            && key.code == KeyCode::Char('c')
            && self.focus != Focus::Input
            && self.prompt.is_none()
        {
            self.quit();
            return;
        }
        self.status = None;
        match self.screen {
            Screen::Login(_) => self.on_login_key(key),
            Screen::Main if self.confirm.is_some() => self.on_confirm_key(key),
            Screen::Main if self.settings_menu.is_some() => self.on_settings_key(key, ctrl),
            Screen::Main if self.delete_menu.is_some() => self.on_delete_key(key),
            Screen::Main if self.react_menu.is_some() => self.on_react_key(key, ctrl),
            Screen::Main if self.menu.is_some() => self.on_menu_key(key),
            Screen::Main if self.resizing.is_some() => self.on_resize_key(key, ctrl),
            Screen::Main if self.prompt.is_some() => self.on_prompt_key(key, ctrl),
            Screen::Main if self.focus == Focus::Input && self.stickers.is_some() => {
                self.on_sticker_key(key, ctrl)
            }
            Screen::Main if self.focus == Focus::Input => self.on_insert_key(key, ctrl),
            Screen::Main => self.on_normal_key(key, ctrl),
        }
        // The sticker panel is part of Insert mode, and closes with it.
        if self.focus != Focus::Input {
            self.stickers = None;
        }
    }

    fn on_login_key(&mut self, key: KeyEvent) {
        let Screen::Login(login) = &mut self.screen else {
            return;
        };
        match (&login.step, key.code) {
            (LoginStep::Phone, KeyCode::Tab) if !login.busy => {
                login.busy = true;
                login.error = None;
                self.tg.request_qr_code();
                return;
            }
            (LoginStep::OtherDevice { .. }, KeyCode::Esc) => {
                self.tg.log_out();
                self.relogin = true;
                self.screen = login_screen(LoginStep::Connecting);
                return;
            }
            _ => {}
        }
        if !login.takes_input() {
            return;
        }
        if key.code != KeyCode::Enter {
            login.input.input(key);
            return;
        }
        let value = login.input.lines().concat().trim().to_string();
        if value.is_empty() || login.busy {
            return;
        }
        // API credentials are checked here; TDLib only gets them once both are in.
        match login.step {
            LoginStep::ApiId => {
                match config::parse_api_id(&value) {
                    Some(id) => self.screen = login_screen(LoginStep::ApiHash { id }),
                    None => login.error = Some("The API ID is a number, like 1234567".into()),
                }
                return;
            }
            LoginStep::ApiHash { id } => {
                if !config::is_api_hash(&value) {
                    login.error = Some("The API hash is 32 letters and digits".into());
                    return;
                }
                let keys = ApiKeys { id, hash: value };
                self.settings.api_keys = Some(keys.clone());
                if let Err(e) = self.settings.save(&self.settings_path) {
                    login.error = Some(format!("Couldn't save them: {e:#}"));
                    return;
                }
                if self.keys_source.is_some() {
                    // TDLib only takes a key at startup, so a new client
                    // starts with the saved one.
                    self.relogin = true;
                    self.tg.close();
                } else {
                    self.keys_source = Some(KeySource::Saved);
                    self.tg.set_tdlib_parameters(keys);
                }
                self.screen = login_screen(LoginStep::Connecting);
                return;
            }
            _ => {}
        }
        login.busy = true;
        login.error = None;
        match login.step {
            LoginStep::Phone => self.tg.send_phone_number(value),
            LoginStep::Code { .. } => self.tg.send_code(value),
            LoginStep::Password { .. } => self.tg.send_password(value),
            LoginStep::Email => self.tg.send_email(value),
            LoginStep::EmailCode => self.tg.send_email_code(value),
            LoginStep::Connecting
            | LoginStep::LoggingOut
            | LoginStep::ApiId
            | LoginStep::ApiHash { .. }
            | LoginStep::OtherDevice { .. }
            | LoginStep::Unsupported(_) => {}
        }
    }

    /// The API key to start TDLib with: from the environment, else saved,
    /// else the one release binaries come with.
    fn api_keys(&self) -> Option<(KeySource, ApiKeys)> {
        let built_in = config::built_in_keys().filter(|_| !self.built_in_rejected);
        let env = self.env_keys.clone().map(|k| (KeySource::Env, k));
        env.or_else(|| {
            self.settings
                .api_keys
                .clone()
                .map(|k| (KeySource::Saved, k))
        })
        .or_else(|| built_in.map(|k| (KeySource::BuiltIn, k)))
    }

    /// Telegram refused the API credentials (`API_ID_INVALID`, or
    /// `API_ID_PUBLISHED_FLOOD` for a key it blocked). On the login screen,
    /// it asks for another key, which a new client then starts with.
    fn reject_api_keys(&mut self) {
        let message = match self.keys_source {
            Some(KeySource::Env) => "Telegram rejected TG_API_ID / TG_API_HASH.".to_string(),
            Some(KeySource::BuiltIn) => {
                self.built_in_rejected = true;
                "Telegram rejected tuigram's built-in key. Use your own.".into()
            }
            Some(KeySource::Saved) | None => {
                self.settings.api_keys = None;
                match self.settings.save(&self.settings_path) {
                    Ok(()) => "Telegram rejected that API ID and hash.".into(),
                    Err(e) => format!("Telegram rejected the API key; can't forget it: {e:#}"),
                }
            }
        };
        match &mut self.screen {
            Screen::Login(_) if self.keys_source != Some(KeySource::Env) => {
                let mut login = Login::new(LoginStep::ApiId);
                login.error = Some(message);
                self.screen = Screen::Login(Box::new(login));
            }
            Screen::Login(login) => {
                login.busy = false;
                login.error = Some(message);
            }
            Screen::Main => self.status = Some(message),
        }
    }

    /// Normal mode: every key is a command, nothing is typed.
    fn on_normal_key(&mut self, key: KeyEvent, ctrl: bool) {
        let pending_g = std::mem::take(&mut self.pending_g);
        // `h` and `l` go toward the pane on that side of the screen, so they
        // swap when the chat list is on the right.
        let (to_chat, to_list) = match self.settings.chat_list_side {
            Side::Left => ('l', 'h'),
            Side::Right => ('h', 'l'),
        };
        // Motions work the same in both panes. In the message pane, down
        // (`j`, `G`) is newer and up (`k`, `gg`) is older, as on screen.
        let motion = match key.code {
            KeyCode::Char('j') | KeyCode::Down => Some(1),
            KeyCode::Char('k') | KeyCode::Up => Some(-1),
            KeyCode::Char('d') if ctrl => Some(HALF_PAGE),
            KeyCode::Char('u') if ctrl => Some(-HALF_PAGE),
            KeyCode::Char('g') if pending_g => Some(isize::MIN),
            KeyCode::Char('G') => Some(isize::MAX),
            _ => None,
        };
        if let Some(delta) = motion {
            match self.focus {
                Focus::Chats => self.move_chat_cursor(delta),
                Focus::Messages => self.move_message_cursor(delta),
                Focus::Input => {}
            }
            return;
        }
        match (self.focus, key.code) {
            // Before `r`, which replies.
            (_, KeyCode::Char('r')) if ctrl => {
                self.resizing = Some(self.settings.chat_list_width);
            }
            (_, KeyCode::Char('g')) => self.pending_g = true,
            (_, KeyCode::Char('q')) => self.quit(),
            (_, KeyCode::Char('H')) => self.toggle_highlight(),
            (_, KeyCode::Char('?')) => {
                if let Some(dir) = self.themes.dir.clone() {
                    self.themes = Themes::load(&dir);
                }
                self.use_saved_theme();
                self.settings_menu = Some(SettingsMenu::new(HelpTab::Shortcuts, &self.settings));
            }
            (_, KeyCode::Char(':')) => self.open_prompt(PromptKind::Command),
            (Focus::Chats, KeyCode::Char('/')) => self.open_prompt(PromptKind::Chats),
            (Focus::Messages, KeyCode::Char('/')) => self.open_prompt(PromptKind::Messages),
            (Focus::Messages, KeyCode::Char('n')) => self.next_match(1),
            (Focus::Messages, KeyCode::Char('N')) => self.next_match(-1),
            // Esc ends a search, then an edit, then removes the files, then
            // ends a reply, before it leaves the pane.
            (Focus::Chats, KeyCode::Esc) => self.chats.set_filter(""),
            (Focus::Messages, KeyCode::Esc)
                if self.open.as_ref().is_some_and(|o| o.search.is_some()) =>
            {
                if let Some(open) = self.open.as_mut() {
                    open.search = None;
                }
            }
            (Focus::Messages, KeyCode::Esc)
                if self.open.as_ref().is_some_and(|o| o.editing.is_some()) =>
            {
                self.end_edit();
            }
            (Focus::Messages, KeyCode::Esc)
                if self
                    .open
                    .as_ref()
                    .is_some_and(|o| !o.attachments.is_empty()) =>
            {
                if let Some(open) = self.open.as_mut() {
                    open.attachments.clear();
                    open.dropped = None;
                    open.as_files = false;
                }
            }
            (Focus::Messages, KeyCode::Esc)
                if self.open.as_ref().is_some_and(|o| o.reply.is_some()) =>
            {
                if let Some(open) = self.open.as_mut() {
                    open.reply = None;
                }
            }
            (Focus::Messages, KeyCode::Char('r')) => self.reply_to_selected(),
            (Focus::Messages, KeyCode::Char('e')) => self.edit_selected(),
            (Focus::Messages, KeyCode::Char('y')) => self.copy_selected(),
            (Focus::Messages, KeyCode::Char('a')) => self.open_prompt(PromptKind::Attach),
            (Focus::Messages, KeyCode::Char('p')) => self.paste_clipboard(),
            (Focus::Messages, KeyCode::Char('t')) if ctrl => self.toggle_as_files(),
            (Focus::Messages, KeyCode::Char('d')) if pending_g => self.go_to_replied(),
            (Focus::Messages, KeyCode::Char('d')) => self.open_delete_menu(),
            (Focus::Messages, KeyCode::Char('R')) => self.open_react_menu(),
            (Focus::Messages, KeyCode::Char('X')) => self.remove_reactions(),
            (Focus::Messages, KeyCode::Char('o')) if ctrl => self.jump_back(),
            (Focus::Chats, KeyCode::Enter) => self.open_selected_chat(),
            (Focus::Chats, KeyCode::Char(c)) if c == to_chat => self.open_selected_chat(),
            (Focus::Chats, KeyCode::Char('i')) => {
                self.open_selected_chat();
                if self.open.is_some() {
                    self.focus = Focus::Input;
                }
            }
            (Focus::Messages, KeyCode::Char('i')) => self.focus = Focus::Input,
            (Focus::Messages, KeyCode::Enter) => self.open_selected_message(),
            (Focus::Messages, KeyCode::Esc) => self.focus = Focus::Chats,
            (Focus::Messages, KeyCode::Char(c)) if c == to_list => self.focus = Focus::Chats,
            _ => {}
        }
    }

    /// Insert mode: keys type into the composer.
    fn on_insert_key(&mut self, key: KeyEvent, ctrl: bool) {
        let alt = key.modifiers.contains(KeyModifiers::ALT);
        let shift = key.modifiers.contains(KeyModifiers::SHIFT);
        match key.code {
            // Ctrl-c leaves Insert mode like in vim, instead of quitting mid-sentence.
            KeyCode::Esc => self.leave_insert(),
            KeyCode::Char('c') if ctrl => self.leave_insert(),
            // Shift-Enter only arrives on terminals with the kitty keyboard protocol;
            // Alt-Enter and Ctrl-j work everywhere.
            KeyCode::Enter if alt || shift => {
                self.composer.insert_newline();
                self.on_composer_edit();
            }
            KeyCode::Char('j') if ctrl => {
                self.composer.insert_newline();
                self.on_composer_edit();
            }
            KeyCode::Enter => self.send(),
            // Ctrl-v pages down in the text area, which a composer this
            // small doesn't need.
            KeyCode::Char('v') if ctrl => self.paste_clipboard(),
            KeyCode::Char('z') if ctrl => self.undo_drop(),
            KeyCode::Char('t') if ctrl => self.toggle_as_files(),
            KeyCode::Tab => self.open_stickers(),
            _ => {
                if self.composer.input(key) {
                    self.on_composer_edit();
                }
            }
        }
    }

    fn leave_insert(&mut self) {
        self.focus = Focus::Messages;
        self.set_typing(false);
    }

    /// You're typing while the composer has text, and stopped once it's empty.
    /// An edit to a sent message isn't typing.
    fn on_composer_edit(&mut self) {
        let editing = self.open.as_ref().is_some_and(|o| o.editing.is_some());
        let typing = !editing && self.composer.lines().iter().any(|l| !l.trim().is_empty());
        self.set_typing(typing);
    }

    /// Tells the open chat whether you're typing: again every
    /// [`TYPING_EVERY`] while you are, and once when you stop.
    fn set_typing(&mut self, typing: bool) {
        let chat_id = self.open.as_ref().map(|o| o.chat_id);
        if typing && let Some(chat_id) = chat_id {
            let told = self
                .typing
                .is_some_and(|(id, at)| id == chat_id && at.elapsed() < TYPING_EVERY);
            if !told {
                self.tg.send_typing(chat_id, true);
                self.typing = Some((chat_id, Instant::now()));
            }
        } else if let Some((chat_id, _)) = self.typing.take() {
            self.tg.send_typing(chat_id, false);
        }
    }

    fn send(&mut self) {
        if self.open.as_ref().is_some_and(|o| o.editing.is_some()) {
            self.save_edit();
            return;
        }
        let Some(open) = self.open.as_mut() else {
            return;
        };
        let text = self.composer.lines().join("\n");
        let text = text.trim().to_string();
        if text.is_empty() && open.attachments.is_empty() {
            return;
        }
        let reply_to = open.reply.take().map(|r| r.id);
        if open.attachments.is_empty() {
            self.tg.send_text(open.chat_id, text, reply_to);
        } else {
            let as_files = open.as_files;
            let groups = attach::albums(&open.attachments, as_files)
                .into_iter()
                .map(|album| album.iter().map(|a| a.upload(as_files)).collect())
                .collect();
            self.tg.send_files(open.chat_id, groups, text, reply_to);
            open.attachments.clear();
            open.dropped = None;
            open.as_files = false;
        }
        self.composer = new_composer();
        // The message arriving ends the typing status for everyone.
        self.typing = None;
        if self.settings.normal_after_send {
            self.focus = Focus::Messages;
        }
        // Jump to the bottom to watch it arrive.
        self.jump_to_newest();
    }

    fn open_prompt(&mut self, kind: PromptKind) {
        let input = prompt_input("");
        // Files go into a chat, so one has to be open.
        if kind == PromptKind::Attach && !self.can_attach() {
            return;
        }
        self.prompt = Some(Prompt {
            kind,
            input,
            completions: Vec::new(),
            previous_filter: self.chats.filter().to_string(),
            previous_selected: self.selected,
        });
    }

    /// The prompt takes all keys while it's up.
    fn on_prompt_key(&mut self, key: KeyEvent, ctrl: bool) {
        let Some(prompt) = self.prompt.as_mut() else {
            return;
        };
        match key.code {
            KeyCode::Esc => self.close_prompt(false),
            KeyCode::Char('c') if ctrl => self.close_prompt(false),
            // Ctrl-m and Ctrl-j would add a line to the one-line prompt.
            KeyCode::Enter => self.close_prompt(true),
            KeyCode::Char('m' | 'j') if ctrl => self.close_prompt(true),
            // Like vim, backspace on an empty prompt closes it.
            KeyCode::Backspace if prompt.query().is_empty() => self.close_prompt(false),
            KeyCode::Tab if prompt.kind == PromptKind::Attach => {
                let completion = attach::complete(&prompt.query());
                prompt.input = prompt_input(&completion.text);
                prompt.completions = completion.matches;
            }
            _ => {
                prompt.input.input(key);
                prompt.completions.clear();
                self.on_prompt_edit();
            }
        }
    }

    /// The chat list filters as you type, with the cursor on the top match.
    fn on_prompt_edit(&mut self) {
        let Some(prompt) = self.prompt.as_ref() else {
            return;
        };
        if prompt.kind == PromptKind::Chats {
            self.chats.set_filter(prompt.query().trim());
            self.chats.refresh();
            self.selected = self.chats.ids().first().copied();
        }
    }

    /// Enter (`submit`) runs the search; Esc puts things back as they were.
    fn close_prompt(&mut self, submit: bool) {
        let Some(prompt) = self.prompt.take() else {
            return;
        };
        let query = prompt.query().trim().to_string();
        match prompt.kind {
            PromptKind::Chats => {
                if submit && (query.is_empty() || !self.chats.ids().is_empty()) {
                    return;
                }
                if submit {
                    self.status = Some(format!("No chats match \"{query}\""));
                }
                self.chats.set_filter(&prompt.previous_filter);
                self.selected = prompt.previous_selected;
            }
            PromptKind::Messages => {
                let Some(open) = self.open.as_mut().filter(|_| submit && !query.is_empty()) else {
                    return;
                };
                open.search = Some(MessageSearch::new(query));
                self.go_to_match(0);
            }
            PromptKind::Attach if !submit || query.is_empty() => {}
            PromptKind::Attach => {
                let path = attach::expand_home(&query);
                // A file dropped on the prompt comes quoted.
                let paths = match attach::pasted_paths(&query) {
                    Some(paths) if !path.exists() => paths,
                    _ => vec![path],
                };
                self.attach(paths, None);
            }
            PromptKind::Command if !submit || query.is_empty() => {}
            PromptKind::Command => match Command::parse(&query) {
                Some(Command::Logout) => self.log_out(),
                None => self.status = Some(format!("Not a command: {query}")),
            },
        }
    }

    /// Sends a read receipt for the newest incoming message once you can
    /// see it: the chat pane and the terminal window have focus, and the view
    /// is on the newest message (see [`App::watching`]). Viewing it marks the
    /// whole chat as read.
    fn mark_seen(&mut self) {
        let watched = self.open.as_ref().map(|o| o.chat_id);
        if !watched.is_some_and(|id| self.watching(id)) {
            return;
        }
        let Some(open) = self.open.as_mut() else {
            return;
        };
        let newest = open.messages.iter().rev().find(|(_, m)| !m.outgoing);
        if let Some((&id, _)) = newest
            && id > open.seen
        {
            open.seen = id;
            self.tg.view_messages(open.chat_id, vec![id]);
        }
    }

    /// The chat is open in front of the user, on its newest message, so new
    /// ones are seen as they arrive. Where the terminal never says when its
    /// window loses focus (tmux without `focus-events`, a detached session),
    /// no key press for [`IDLE_AFTER`] counts as the user being away, so
    /// messages aren't marked read, and do notify, while nobody is there.
    fn watching(&self, chat_id: i64) -> bool {
        matches!(self.screen, Screen::Main)
            && self.terminal_focused
            && (self.focus_reported || self.last_input.elapsed() < IDLE_AFTER)
            && matches!(self.focus, Focus::Messages | Focus::Input)
            && self.settings_menu.is_none()
            && self
                .open
                .as_ref()
                .is_some_and(|o| o.chat_id == chat_id && o.at_newest && o.selected.is_none())
    }

    /// The user would see a new message in this chat without being told:
    /// tuigram's window has focus. Where the terminal never reports focus,
    /// only the chat being read counts.
    fn sees(&self, chat_id: i64) -> bool {
        if self.focus_reported {
            self.terminal_focused
        } else {
            self.watching(chat_id)
        }
    }

    /// Online on Telegram while the user is at tuigram: logged in, its window
    /// focused, and a key pressed in the last [`IDLE_AFTER`].
    fn update_online(&mut self) {
        let online = matches!(self.screen, Screen::Main)
            && self.quit_deadline.is_none()
            && self.terminal_focused
            && self.last_input.elapsed() < IDLE_AFTER;
        if online != self.online {
            self.online = online;
            self.tg.set_online(online);
        }
    }

    /// New messages from TDLib, which already left out muted chats and
    /// messages read elsewhere. Those the user sees anyway, or that came in
    /// before tuigram started, are dropped.
    fn on_notifications(&mut self, update: UpdateNotificationGroup) {
        self.notifier.remove(&update.removed_notification_ids);
        if self.notify_with == Notifications::Off {
            return;
        }
        let chat_id = update.chat_id;
        for notification in update.added_notifications {
            let NotificationType::NewMessage(new) = notification.r#type else {
                continue;
            };
            if notification.date < self.notify_since || self.sees(chat_id) {
                continue;
            }
            let text = if new.show_preview {
                self.notification_text(&new.message)
            } else {
                "New message".into()
            };
            let note = Note {
                id: notification.id,
                chat_id,
                chat: self.chats.title(chat_id).unwrap_or("Telegram").into(),
                text,
                silent: notification.is_silent,
            };
            self.notifier.add(note, Instant::now());
        }
    }

    /// "Alice: see you at 5" in groups; just the text in private chats,
    /// where the chat's name says who, and in channels.
    fn notification_text(&self, message: &Message) -> String {
        let text = crate::chats::content_text(&message.content);
        match &message.sender_id {
            MessageSender::User(sender) if sender.user_id != message.chat_id => {
                match self.users.get(&sender.user_id) {
                    Some(name) => format!("{name}: {text}"),
                    None => text,
                }
            }
            _ => text,
        }
    }

    fn send_notification(&mut self) {
        let Some(alert) = self.notifier.due(Instant::now()) else {
            return;
        };
        self.notifications_sent += 1;
        let id = self.notifications_sent;
        if let Some(code) = notify::escape(self.notify_with, &alert, id, self.in_tmux) {
            notify::send(&code);
        }
    }

    fn set_unread_chats(&mut self, count: i32) {
        if count != self.unread_chats {
            self.unread_chats = count;
            notify::send(&notify::title(count));
        }
    }

    /// `:logout`: ends the session on Telegram's side and deletes what TDLib
    /// keeps on this computer. The login screen comes back once TDLib closes.
    fn log_out(&mut self) {
        self.tg.log_out();
        self.relogin = true;
        self.screen = login_screen(LoginStep::LoggingOut);
    }

    /// Drops everything from the old session before a new client starts, so
    /// nothing from it shows up after logging in again, maybe as someone else.
    fn forget_session(&mut self) {
        // Highlights are chat ids of the old account.
        self.settings.highlighted_chats.clear();
        if let Err(e) = self.settings.save(&self.settings_path) {
            self.status = Some(format!("Couldn't save settings: {e:#}"));
        }
        self.chats = Chats::default();
        self.users.clear();
        self.selected = None;
        self.open = None;
        self.focus = Focus::Chats;
        self.composer = new_composer();
        self.images.forget_files();
        self.opening.clear();
        self.copying.clear();
        self.menu = None;
        self.delete_menu = None;
        self.react_menu = None;
        self.stickers = None;
        // The next account says if it has Premium; one without may not.
        self.premium = false;
        self.confirm = None;
        self.settings_menu = None;
        self.prompt = None;
        self.chats_loading = false;
        self.all_chats_loaded = false;
        self.pending_g = false;
        // The client that knew about being online and typing is gone.
        self.online = false;
        self.typing = None;
        self.notifier.clear();
        self.notify_since = i32::MAX;
        self.set_unread_chats(0);
    }

    /// `n` (`step` 1) goes to the next older match, `N` (-1) to the next newer.
    fn next_match(&mut self, step: isize) {
        let Some(search) = self.open.as_ref().and_then(|o| o.search.as_ref()) else {
            return;
        };
        let Some(current) = search.current else {
            return;
        };
        match current.checked_add_signed(step) {
            Some(index) => self.go_to_match(index),
            None => self.status = Some("No newer matches".into()),
        }
    }

    /// Moves the cursor to search match `index` (0 = newest), fetching more
    /// results first if it's past the ones fetched.
    fn go_to_match(&mut self, index: usize) {
        let Some(open) = self.open.as_mut() else {
            return;
        };
        let Some(search) = open.search.as_mut() else {
            return;
        };
        if let Some(&id) = search.results.get(index) {
            search.current = Some(index);
            self.jump_to_message(id);
        } else if !search.done {
            search.wanted = Some(index);
            if !search.loading {
                search.loading = true;
                let (query, from) = (search.query.clone(), search.next_from);
                self.tg
                    .search_messages(open.chat_id, query, from, SEARCH_PAGE);
            }
        } else if search.results.is_empty() {
            self.status = Some(format!("No messages match \"{}\"", search.query));
            open.search = None;
        } else {
            self.status = Some("No older matches".into());
        }
    }

    /// Puts the cursor on a message, loading the history around it if it's
    /// not loaded.
    fn jump_to_message(&mut self, id: i64) {
        let Some(open) = self.open.as_mut() else {
            return;
        };
        if open.messages.contains_key(&id) {
            open.selected = Some(id);
            // A jump still loading elsewhere would move the cursor away.
            if matches!(open.loading, Some(Page::Around(_))) {
                open.loading = None;
            }
            return;
        }
        // The page replaces the loaded messages when it arrives.
        let page = Page::Around(id);
        open.loading = Some(page);
        self.tg.load_history(open.chat_id, page, HISTORY_PAGE);
    }

    /// Back to following the newest message, reloading if an older part of
    /// the chat is shown.
    fn jump_to_newest(&mut self) {
        let Some(open) = self.open.as_mut() else {
            return;
        };
        open.selected = None;
        if open.at_newest {
            return;
        }
        open.messages.clear();
        open.scroll = None;
        open.at_newest = true;
        open.all_loaded = false;
        open.loading = None;
        self.load_older_messages();
    }

    fn open_selected_chat(&mut self) {
        let Some(chat_id) = self.selected else {
            return;
        };
        self.focus = Focus::Messages;
        if self.open.as_ref().is_some_and(|o| o.chat_id == chat_id) {
            return;
        }
        self.set_typing(false);
        // TDLib only sends some updates (e.g. for channels) while a chat is open.
        if let Some(old) = self.open.take() {
            self.tg.close_chat(old.chat_id);
            self.images.clear();
        }
        self.tg.open_chat(chat_id);
        self.chats.opened(chat_id);
        self.open = Some(OpenChat::new(chat_id));
        self.composer = new_composer();
        self.load_older_messages();
    }

    /// Enter on a message: opens its file or link right away, or shows a menu
    /// when there's more than one.
    fn open_selected_message(&mut self) {
        let Some(open) = &self.open else {
            return;
        };
        let Some(msg) = open.cursor_id().and_then(|id| open.messages.get(&id)) else {
            return;
        };
        let mut targets: Vec<Target> = msg.file.clone().map(Target::File).into_iter().collect();
        targets.extend(msg.links.iter().cloned().map(Target::Link));
        match targets.len() {
            0 => self.status = Some("Nothing to open in this message".into()),
            1 => self.open_target(targets.remove(0)),
            _ => {
                self.menu = Some(PickMenu {
                    action: MenuAction::Open,
                    targets,
                    selected: 0,
                })
            }
        }
    }

    /// `r`: answer the message under the cursor. Goes straight to Insert mode,
    /// keeping whatever was already typed.
    fn reply_to_selected(&mut self) {
        // Its draft and reply come back, and the new reply replaces that one.
        self.end_edit();
        let Some(open) = self.open.as_mut() else {
            return;
        };
        let Some((&id, msg)) = open
            .cursor_id()
            .and_then(|id| open.messages.get_key_value(&id))
        else {
            return;
        };
        if msg.state == SendState::Failed {
            self.status = Some("Can't reply to a message that wasn't sent".into());
            return;
        }
        open.reply = Some(Replied::new(id, msg));
        self.focus = Focus::Input;
    }

    /// `e`: edits the message under the cursor in the composer, once TDLib
    /// says it can be.
    fn edit_selected(&mut self) {
        let Some(open) = &self.open else {
            return;
        };
        let Some(id) = open.edit_target() else {
            return;
        };
        match open.cant_edit(id) {
            Some(why) => self.status = Some(why.into()),
            None => self.tg.check_editable(open.chat_id, id),
        }
    }

    fn on_editable(&mut self, chat_id: i64, message_id: i64, editable: Option<bool>) {
        // Only while the cursor is still on it, so a late answer can't take
        // over the composer.
        let Some(open) = self
            .open
            .as_ref()
            .filter(|o| o.chat_id == chat_id && o.edit_target() == Some(message_id))
        else {
            return;
        };
        let Some(msg) = open.messages.get(&message_id) else {
            return;
        };
        // On an error, TDLib's message is already in the status bar.
        match editable {
            None => {}
            Some(false) => self.status = Some("You can't edit this message".into()),
            Some(true) if msg.formatted => {
                self.confirm = Some(Confirm {
                    title: "Edit without formatting?".into(),
                    lines: vec![
                        "tuigram edits plain text, so this message would lose".into(),
                        "its bold, italics, links behind words and the like.".into(),
                    ],
                    action: Confirmed::Edit(message_id),
                });
            }
            Some(true) => self.start_edit(message_id),
        }
    }

    /// Puts the message's text in the composer, keeping what was there to
    /// give back when the edit is done.
    fn start_edit(&mut self, id: i64) {
        self.end_edit();
        let Some(open) = self.open.as_mut() else {
            return;
        };
        let Some(msg) = open.messages.get(&id) else {
            return;
        };
        let text = msg.source_text.clone();
        open.editing = Some(Editing {
            id,
            snippet: msg.snippet(),
            editable: msg.editable,
            draft: self.composer.lines().join("\n"),
            reply: open.reply.take(),
            attachments: std::mem::take(&mut open.attachments),
        });
        open.dropped = None;
        self.set_typing(false);
        self.composer = new_composer();
        self.composer.insert_str(text);
        self.focus = Focus::Input;
    }

    /// Ends an edit, saved or not: the draft and reply from before come back.
    fn end_edit(&mut self) {
        let Some(editing) = self.open.as_mut().and_then(|o| o.editing.take()) else {
            return;
        };
        self.composer = new_composer();
        self.composer.insert_str(editing.draft);
        if let Some(open) = self.open.as_mut() {
            open.reply = editing.reply;
            open.attachments = editing.attachments;
        }
    }

    /// Enter while editing: sends the new text, unless nothing changed.
    fn save_edit(&mut self) {
        let Some(open) = &self.open else {
            return;
        };
        let Some(editing) = &open.editing else {
            return;
        };
        let text = self.composer.lines().join("\n");
        let text = text.trim();
        if text.is_empty() && editing.editable == Editable::Text {
            self.status = Some("A message can't be empty (d deletes it)".into());
            return;
        }
        let changed = open
            .messages
            .get(&editing.id)
            .is_none_or(|m| m.source_text != text);
        if changed {
            let (chat_id, id, text) = (open.chat_id, editing.id, text.to_string());
            match editing.editable {
                Editable::Text => self.tg.edit_text(chat_id, id, text),
                Editable::Caption { above } => self.tg.edit_caption(chat_id, id, text, above),
                Editable::No => {}
            }
        }
        self.end_edit();
        if self.settings.normal_after_send {
            self.focus = Focus::Messages;
        }
    }

    /// `d`: asks how to delete the message under the cursor. The popup opens
    /// at once and fills in when TDLib says what's allowed.
    fn open_delete_menu(&mut self) {
        let Some(open) = &self.open else {
            return;
        };
        let Some((&message_id, msg)) = open
            .cursor_id()
            .and_then(|id| open.messages.get_key_value(&id))
        else {
            return;
        };
        self.delete_menu = Some(DeleteMenu {
            message_id,
            snippet: msg.snippet(),
            choices: Vec::new(),
            selected: 0,
        });
        self.tg.check_deletable(open.chat_id, message_id);
    }

    fn on_deletable(&mut self, chat_id: i64, message_id: i64, deletable: Option<Deletable>) {
        // Drop answers for a popup that closed, or a chat that changed.
        if self.open.as_ref().is_none_or(|o| o.chat_id != chat_id) {
            return;
        }
        let Some(menu) = self
            .delete_menu
            .as_mut()
            .filter(|m| m.message_id == message_id)
        else {
            return;
        };
        // On an error, TDLib's message is already in the status bar.
        let Some(deletable) = deletable else {
            self.delete_menu = None;
            return;
        };
        menu.set_allowed(deletable);
        if menu.choices.is_empty() {
            self.delete_menu = None;
            self.status = Some("You can't delete this message".into());
        }
    }

    /// The delete popup takes all keys while it's up.
    fn on_delete_key(&mut self, key: KeyEvent) {
        let Some(menu) = self.delete_menu.as_mut() else {
            return;
        };
        let last = menu.choices.len().saturating_sub(1);
        let pick = match key.code {
            KeyCode::Char('j') | KeyCode::Down => {
                menu.selected = (menu.selected + 1).min(last);
                None
            }
            KeyCode::Char('k') | KeyCode::Up => {
                menu.selected = menu.selected.saturating_sub(1);
                None
            }
            KeyCode::Enter | KeyCode::Char('l') => Some(menu.selected),
            KeyCode::Char(c @ '1'..='9') => Some(c as usize - '1' as usize),
            KeyCode::Esc | KeyCode::Char('q' | 'h') => {
                self.delete_menu = None;
                return;
            }
            _ => None,
        };
        // Nothing to pick while TDLib hasn't answered.
        let Some(choice) = pick.and_then(|i| menu.choices.get(i).copied()) else {
            return;
        };
        let message_id = menu.message_id;
        self.delete_menu = None;
        if let Some(open) = &self.open {
            let revoke = choice == DeleteChoice::Everyone;
            self.tg.delete_message(open.chat_id, message_id, revoke);
        }
    }

    /// `R`: the emoji to react to the message under the cursor with. The
    /// popup opens at once and fills in when TDLib says which the chat allows.
    fn open_react_menu(&mut self) {
        let Some(open) = &self.open else {
            return;
        };
        let Some((&message_id, msg)) = open
            .react_target()
            .and_then(|id| open.messages.get_key_value(&id))
        else {
            return;
        };
        match msg.state {
            SendState::Pending => self.status = Some("Wait until it's sent".into()),
            SendState::Failed => self.status = Some("This message wasn't sent".into()),
            SendState::Sent => {
                self.react_menu = Some(ReactMenu::new(message_id, msg.snippet()));
                self.tg.available_reactions(open.chat_id, message_id);
            }
        }
    }

    fn on_available_reactions(
        &mut self,
        chat_id: i64,
        message_id: i64,
        available: Option<reactions::Available>,
    ) {
        // Drop answers for a popup that closed, or a chat that changed.
        if self.open.as_ref().is_none_or(|o| o.chat_id != chat_id)
            || self
                .react_menu
                .as_ref()
                .is_none_or(|m| m.message_id != message_id)
        {
            return;
        }
        // On an error, TDLib's message is already in the status bar.
        let Some(available) = available else {
            self.react_menu = None;
            return;
        };
        if available.emoji.is_empty() {
            self.react_menu = None;
            let reason = available.reason.unwrap_or("Reactions are off in this chat");
            self.status = Some(reason.into());
            return;
        }
        let yours: Vec<String> = self
            .open
            .iter()
            .flat_map(|o| o.your_emoji(message_id))
            .map(|(_, emoji)| emoji)
            .collect();
        if let Some(menu) = self.react_menu.as_mut() {
            menu.set_choices(available.emoji, &yours);
        }
    }

    /// The reaction popup takes all keys while it's up. In the grid, `h/j/k/l`
    /// move; after `/`, keys type the search and the arrows move.
    fn on_react_key(&mut self, key: KeyEvent, ctrl: bool) {
        let Some(menu) = self.react_menu.as_mut() else {
            return;
        };
        let row = reactions::COLUMNS as isize;
        let searching = menu.query.is_some();
        match key.code {
            KeyCode::Enter => self.react_with_selected(),
            KeyCode::Left => menu.move_by(-1),
            KeyCode::Right | KeyCode::Tab => menu.move_by(1),
            KeyCode::BackTab => menu.move_by(-1),
            KeyCode::Up => menu.move_by(-row),
            KeyCode::Down => menu.move_by(row),
            KeyCode::Char('n') if ctrl => menu.move_by(1),
            KeyCode::Char('p') if ctrl => menu.move_by(-1),
            // Esc, or Backspace on an empty search, leaves the search first.
            KeyCode::Esc if searching => menu.leave_search(),
            KeyCode::Backspace if menu.query.as_ref().is_some_and(|q| q.is_empty()) => {
                menu.leave_search();
            }
            KeyCode::Backspace if searching => menu.edit_query(|q| {
                q.pop();
            }),
            KeyCode::Char('u' | 'w') if ctrl && searching => menu.edit_query(String::clear),
            KeyCode::Char(c) if searching && !ctrl => menu.edit_query(|q| q.push(c)),
            KeyCode::Char('h') => menu.move_by(-1),
            KeyCode::Char('l') => menu.move_by(1),
            KeyCode::Char('k') => menu.move_by(-row),
            KeyCode::Char('j') => menu.move_by(row),
            KeyCode::Char('/') => menu.edit_query(|_| {}),
            KeyCode::Char('X') => {
                self.react_menu = None;
                self.remove_reactions();
            }
            KeyCode::Esc | KeyCode::Char('q' | 'R') => self.react_menu = None,
            _ => {}
        }
    }

    /// Enter in the reaction popup: adds the emoji under the cursor, or takes
    /// it back if it's already yours, and closes the popup.
    fn react_with_selected(&mut self) {
        // Nothing to pick while TDLib hasn't answered or nothing matches.
        let Some((message_id, emoji)) = self
            .react_menu
            .as_ref()
            .and_then(|m| Some((m.message_id, m.current()?.to_string())))
        else {
            return;
        };
        self.react_menu = None;
        let Some(open) = &self.open else {
            return;
        };
        // Yours already, on this photo or another of its album: taken back
        // from where it is, rather than added a second time here.
        let yours = open
            .your_emoji(message_id)
            .into_iter()
            .find(|(_, e)| *e == emoji);
        let kind = ReactionKind::Emoji(emoji);
        match yours {
            Some((on, _)) => self.tg.react(open.chat_id, on, &kind, false),
            None => self.tg.react(open.chat_id, message_id, &kind, true),
        }
    }

    /// `X`: takes back all your reactions on the message under the cursor,
    /// without the popup.
    fn remove_reactions(&mut self) {
        let Some(open) = &self.open else {
            return;
        };
        let yours = open.your_reactions();
        if yours.is_empty() {
            self.status = Some("You haven't reacted to this message".into());
            return;
        }
        for (message_id, kind) in yours {
            self.tg.react(open.chat_id, message_id, &kind, false);
        }
    }

    /// Resize mode takes the keys: `h` and `l` move the line between the
    /// chat list and the chat as you press them, `=` puts it back where it
    /// starts out, Enter keeps it and Esc puts back where it was.
    fn on_resize_key(&mut self, key: KeyEvent, ctrl: bool) {
        let Some(before) = self.resizing else {
            return;
        };
        // `h` and `l` move the line between the panes that way.
        let left = match self.settings.chat_list_side {
            Side::Left => -1,
            Side::Right => 1,
        };
        match key.code {
            KeyCode::Char('h') | KeyCode::Left => self.settings.resize_list(left),
            KeyCode::Char('l') | KeyCode::Right => self.settings.resize_list(-left),
            KeyCode::Char('=') => self.settings.chat_list_width = settings::DEFAULT_LIST_WIDTH,
            KeyCode::Esc => {
                self.settings.chat_list_width = before;
                self.resizing = None;
            }
            KeyCode::Enter => self.end_resize(),
            KeyCode::Char('r') if ctrl => self.end_resize(),
            _ => {}
        }
    }

    /// Leaves resize mode with the panes as they are, saved for next time.
    fn end_resize(&mut self) {
        let before = self.resizing.take();
        if before != Some(self.settings.chat_list_width)
            && let Err(e) = self.settings.save(&self.settings_path)
        {
            self.status = Some(format!("Couldn't save settings: {e:#}"));
        }
    }

    /// Tab while writing: the sticker panel. It opens at once and fills in
    /// as TDLib sends your recent and favorite stickers and your sets.
    fn open_stickers(&mut self) {
        let Some(open) = &self.open else {
            return;
        };
        if open.editing.is_some() {
            self.status = Some("An edit can't change a message into a sticker".into());
            return;
        }
        self.stickers = Some(StickerPanel::new(open.chat_id));
        self.tg.stickers(Source::Recent);
        self.tg.stickers(Source::Favorites);
        self.tg.sticker_sets();
    }

    /// The sticker panel takes the keys while it's open. In the grid,
    /// `h/j/k/l` move and `H/L` switch tabs; after `/`, keys type the search
    /// and the arrows move.
    fn on_sticker_key(&mut self, key: KeyEvent, ctrl: bool) {
        let Some(panel) = self.stickers.as_mut() else {
            return;
        };
        let searching = panel.query.is_some();
        let before = panel.search_query();
        match key.code {
            KeyCode::Enter => self.send_sticker(),
            KeyCode::Char('c') if ctrl => self.leave_insert(),
            KeyCode::Left => panel.move_by(-1),
            KeyCode::Right => panel.move_by(1),
            KeyCode::Up => panel.move_rows(-1),
            KeyCode::Down => panel.move_rows(1),
            KeyCode::Char('n') if ctrl => panel.move_by(1),
            KeyCode::Char('p') if ctrl => panel.move_by(-1),
            // Esc, or Backspace on an empty search, leaves the search first.
            KeyCode::Esc if searching => panel.leave_search(),
            KeyCode::Backspace if panel.query.as_ref().is_some_and(|q| q.is_empty()) => {
                panel.leave_search();
            }
            KeyCode::Backspace if searching => panel.edit_query(|q| {
                q.pop();
            }),
            KeyCode::Char('u' | 'w') if ctrl && searching => panel.edit_query(String::clear),
            KeyCode::Char(c) if searching && !ctrl => panel.edit_query(|q| q.push(c)),
            KeyCode::Char('h') => panel.move_by(-1),
            KeyCode::Char('l') => panel.move_by(1),
            KeyCode::Char('k') => panel.move_rows(-1),
            KeyCode::Char('j') => panel.move_rows(1),
            KeyCode::Char('H') => panel.switch(-1),
            KeyCode::Char('L') => panel.switch(1),
            KeyCode::Char('/') => panel.edit_query(|_| {}),
            // Back to writing.
            KeyCode::Esc | KeyCode::Tab | KeyCode::Char('i' | 'q') => self.stickers = None,
            _ => {}
        }
        self.find_stickers(before);
        self.load_sticker_set();
    }

    /// Asks TDLib for what the panel's search finds, if it's changed from
    /// `before`.
    fn find_stickers(&mut self, before: Option<String>) {
        if let Some(panel) = &self.stickers
            && let Some(query) = panel.search_query()
            && Some(&query) != before.as_ref()
        {
            self.tg.find_stickers(panel.chat_id, query);
        }
    }

    /// Asks for the stickers of the set the panel shows, the first time.
    fn load_sticker_set(&mut self) {
        if let Some(set_id) = self.stickers.as_mut().and_then(StickerPanel::pending_set) {
            self.tg.stickers(Source::Set(set_id));
        }
    }

    fn on_stickers(&mut self, source: Source, stickers: Option<Vec<tdlib_rs::types::Sticker>>) {
        let premium = self.premium;
        let Some(panel) = self.stickers.as_mut() else {
            return;
        };
        // After an error, TDLib's message is in the status bar, and the tab
        // shows no stickers.
        let stickers = stickers::convert(&stickers.unwrap_or_default(), premium);
        panel.set_stickers(source, stickers);
        // An empty Recent going away can show a set.
        self.load_sticker_set();
    }

    /// Enter in the sticker panel: sends the sticker under the cursor, as the
    /// reply if one is being written, and closes the panel. What's written
    /// in the composer stays there.
    fn send_sticker(&mut self) {
        let Some(panel) = &self.stickers else {
            return;
        };
        // Nothing to send while it's loading or nothing was found.
        let Some(sticker) = panel.current().cloned() else {
            return;
        };
        let Some(open) = self.open.as_mut().filter(|o| o.chat_id == panel.chat_id) else {
            return;
        };
        let reply_to = open.reply.take().map(|r| r.id);
        self.tg.send_sticker(open.chat_id, &sticker, reply_to);
        self.stickers = None;
        // The message arriving ends the typing status for everyone.
        self.typing = None;
        if self.settings.normal_after_send {
            self.focus = Focus::Messages;
        }
        self.jump_to_newest();
    }

    /// `gd`: from a reply to the message it answers, loading the history
    /// around it if needed. Ctrl-o comes back.
    fn go_to_replied(&mut self) {
        let Some(open) = self.open.as_mut() else {
            return;
        };
        match open.replied_jump() {
            Ok((from, to)) => {
                open.jumps.push(from);
                self.jump_to_message(to);
            }
            Err(why) => self.status = Some(why.into()),
        }
    }

    /// Ctrl-o: back to the reply the last `gd` left.
    fn jump_back(&mut self) {
        let Some(open) = self.open.as_mut() else {
            return;
        };
        match open.jumps.pop() {
            Some(id) => self.jump_to_message(id),
            None => self.status = Some("Nothing to go back to".into()),
        }
    }

    /// Files open in their default app once downloaded; links in the browser.
    fn open_target(&mut self, target: Target) {
        match target {
            Target::File(file) => {
                // TDLib answers at once if the file is already downloaded.
                if self.opening.insert(file.id) {
                    self.tg.download(file.id);
                }
            }
            Target::Link(Link {
                url,
                disguise: Some(shown),
            }) => {
                self.confirm = Some(Confirm {
                    title: "Open this link?".into(),
                    lines: vec![
                        format!("The text says: {shown}"),
                        format!("It goes to:    {url}"),
                    ],
                    action: Confirmed::OpenLink(url),
                })
            }
            Target::Link(link) => self.open_externally(&link.url),
            Target::Text(_) => {}
        }
    }

    /// A file finished downloading for Enter: opened at once if it's a
    /// type that can't run code, else only after a `y`.
    fn open_downloaded(&mut self, path: String) {
        mark_downloaded(&path);
        if safe_to_open(&path) {
            self.open_externally(&path);
            return;
        }
        let name = Path::new(&path)
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default();
        self.confirm = Some(Confirm {
            title: format!("Open {name}?"),
            lines: vec![
                "Files like this can run programs on your computer.".into(),
                "Only open it if you trust whoever sent it.".into(),
            ],
            action: Confirmed::OpenFile(path),
        });
    }

    fn open_externally(&mut self, target: &str) {
        if let Err(e) = open_externally(target) {
            self.status = Some(format!("Couldn't open it: {e}"));
        }
    }

    /// The confirmation takes all keys while it's up. Only `y` goes ahead, so
    /// an Enter pressed out of habit can't.
    fn on_confirm_key(&mut self, key: KeyEvent) {
        match key.code {
            KeyCode::Char('y') => {
                if let Some(confirm) = self.confirm.take() {
                    match confirm.action {
                        Confirmed::OpenFile(target) | Confirmed::OpenLink(target) => {
                            self.open_externally(&target)
                        }
                        Confirmed::Edit(id) => self.start_edit(id),
                    }
                }
            }
            KeyCode::Char('n' | 'q') | KeyCode::Esc => self.confirm = None,
            _ => {}
        }
    }

    /// `y`: copies what's in the message under the cursor, like Telegram's
    /// Copy. With links or media as well as text, a menu asks which.
    fn copy_selected(&mut self) {
        let Some(open) = &self.open else {
            return;
        };
        let Some(msg) = open.cursor_id().and_then(|id| open.messages.get(&id)) else {
            return;
        };
        let mut targets = Vec::new();
        if !msg.source_text.is_empty() {
            targets.push(Target::Text(msg.source_text.clone()));
        }
        targets.extend(msg.links.iter().cloned().map(Target::Link));
        targets.extend(msg.file.clone().map(Target::File));
        match targets.len() {
            0 => self.status = Some("Nothing to copy in this message".into()),
            1 => self.copy_target(targets.remove(0)),
            _ => {
                self.menu = Some(PickMenu {
                    action: MenuAction::Copy,
                    targets,
                    selected: 0,
                })
            }
        }
    }

    /// Text and links are copied at once. Media is downloaded first (TDLib
    /// answers at once if it already is), then copied by `copy_downloaded`.
    fn copy_target(&mut self, target: Target) {
        let text = match target {
            Target::Text(text) => text,
            Target::Link(link) => link.url,
            Target::File(file) => {
                let id = file.id;
                if self.copying.insert(id, file).is_none() {
                    self.tg.download(id);
                }
                return;
            }
        };
        match self.clipboard.copy_text(&text) {
            Ok(Copied::System) => self.show_toast("Copied", &text),
            Ok(Copied::Terminal) => self.show_toast("Sent to the terminal's clipboard", &text),
            Err(e) => self.status = Some(format!("Couldn't copy: {e}")),
        }
    }

    /// Photos are copied as images, after decoding off the UI thread; other
    /// files as files, so pasting attaches them.
    fn copy_downloaded(&mut self, file: MediaFile, path: &str) {
        if file.photo {
            self.clipboard.decode_image(path.to_string(), file.label);
            return;
        }
        match self.clipboard.copy_file(Path::new(path)) {
            Ok(()) => self.show_toast("Copied", &file.label),
            Err(e) => self.status = Some(format!("Couldn't copy: {e}")),
        }
    }

    fn on_clipboard(&mut self, event: ClipboardEvent) {
        match event {
            ClipboardEvent::Decoded(decoded) => self.on_decoded(decoded),
            ClipboardEvent::Pasted(pasted) => self.on_pasted(pasted),
        }
    }

    /// Whether files can go with the next message: a chat is open, and its
    /// composer isn't editing a message, which can't take any. Says why not
    /// in the status bar.
    fn can_attach(&mut self) -> bool {
        match &self.open {
            None => false,
            Some(open) if open.editing.is_some() => {
                self.status = Some("Files can't be added to an edit".into());
                false
            }
            Some(_) => true,
        }
    }

    /// Adds files to the next message and goes to Insert mode for the
    /// caption. `pasted` is the paste they came from, for Ctrl-z. Files that
    /// can't be sent are left out, and the status bar says why. Returns how
    /// many were added.
    fn attach(&mut self, paths: Vec<PathBuf>, pasted: Option<String>) -> usize {
        if !self.can_attach() {
            return 0;
        }
        let Some(open) = self.open.as_mut() else {
            return 0;
        };
        let mut added = 0;
        for path in paths {
            match Attachment::new(&path) {
                Ok(attachment) => {
                    open.attachments.push(attachment);
                    added += 1;
                }
                Err(e) => self.status = Some(e),
            }
        }
        if added == 0 {
            return 0;
        }
        open.dropped = pasted.map(|text| Dropped { text, count: added });
        self.focus = Focus::Input;
        added
    }

    /// A paste into the chat (Cmd-V, or files dropped on the window). Paths
    /// to files are attached, which Ctrl-z undoes; other text is typed in
    /// Insert mode. While editing, it's all text.
    fn on_paste(&mut self, text: String) {
        let editing = self.open.as_ref().is_some_and(|o| o.editing.is_some());
        if !editing && let Some(paths) = attach::pasted_paths(&text) {
            self.attach(paths, Some(text));
            return;
        }
        if self.focus == Focus::Input
            && let Some(panel) = self.stickers.as_mut()
        {
            // Into the sticker search: pasting an emoji is a quick way to
            // find stickers for it.
            let before = panel.search_query();
            let line = text::clean(&text)
                .split_whitespace()
                .collect::<Vec<_>>()
                .join(" ");
            panel.edit_query(|q| q.push_str(&line));
            self.find_stickers(before);
            return;
        }
        if self.focus == Focus::Input {
            self.composer.insert_str(text.replace('\r', ""));
            self.on_composer_edit();
        }
    }

    /// `p` and Ctrl-v: what's on the system clipboard goes in the message,
    /// once it's been read off the UI thread (`on_pasted`).
    fn paste_clipboard(&mut self) {
        let Some(open) = &self.open else {
            return;
        };
        self.clipboard.paste(open.chat_id);
        self.pasting = true;
    }

    fn on_pasted(&mut self, pasted: Pasted) {
        self.pasting = false;
        // Not into another chat than the one it was meant for.
        if self
            .open
            .as_ref()
            .is_none_or(|o| o.chat_id != pasted.chat_id)
        {
            return;
        }
        match pasted.content {
            Ok(Paste::Files(paths)) => {
                // Copied files have absolute paths; anything else would be
                // looked for wherever tuigram was started.
                let (paths, relative): (Vec<_>, Vec<_>) =
                    paths.into_iter().partition(|p| p.is_absolute());
                if !relative.is_empty() {
                    self.status = Some("Copied files without a full path were left out".into());
                }
                self.attach(paths, None);
            }
            Ok(Paste::Image(path)) => {
                // Not the made-up name it was saved under.
                if self.attach(vec![path], None) == 1
                    && let Some(last) = self.open.as_mut().and_then(|o| o.attachments.last_mut())
                {
                    last.name = "Pasted image".into();
                }
            }
            Ok(Paste::Text(text)) => {
                self.focus = Focus::Input;
                self.on_paste(text);
            }
            Err(e) => self.status = Some(e),
        }
    }

    /// Ctrl-t: the attached photos go as files, uncompressed and with their
    /// names, or back to photos.
    fn toggle_as_files(&mut self) {
        let Some(open) = self.open.as_mut() else {
            return;
        };
        if open.attachments.iter().any(|a| a.kind.is_photo()) {
            open.as_files = !open.as_files;
        } else if !open.attachments.is_empty() {
            self.status = Some("These go as files already: only photos can go either way".into());
        }
    }

    /// Ctrl-z: files a paste just attached go back to being the text that
    /// was pasted, for a path that was meant to be sent as words.
    fn undo_drop(&mut self) {
        let Some(text) = self.open.as_mut().and_then(OpenChat::undo_drop) else {
            return;
        };
        self.composer.insert_str(text.replace('\r', ""));
        self.on_composer_edit();
    }

    fn on_decoded(&mut self, decoded: Decoded) {
        let result = decoded
            .image
            .and_then(|image| self.clipboard.copy_image(image).map_err(|e| e.to_string()));
        match result {
            Ok(()) => self.show_toast("Copied", &decoded.label),
            Err(e) => self.status = Some(format!("Couldn't copy: {e}")),
        }
    }

    fn show_toast(&mut self, title: &str, detail: &str) {
        self.toast = Some(Toast {
            title: title.into(),
            detail: detail.split_whitespace().collect::<Vec<_>>().join(" "),
            until: Instant::now() + TOAST_TIME,
        });
    }

    /// The open menu takes all keys while it's up.
    fn on_menu_key(&mut self, key: KeyEvent) {
        let Some(menu) = self.menu.as_mut() else {
            return;
        };
        let last = menu.targets.len() - 1;
        match key.code {
            KeyCode::Char('j') | KeyCode::Down => menu.selected = (menu.selected + 1).min(last),
            KeyCode::Char('k') | KeyCode::Up => menu.selected = menu.selected.saturating_sub(1),
            KeyCode::Enter | KeyCode::Char('l') => {
                let index = menu.selected;
                self.pick(index);
            }
            // 1-9 pick an item directly.
            KeyCode::Char(c @ '1'..='9') => {
                let index = c as usize - '1' as usize;
                if index <= last {
                    self.pick(index);
                }
            }
            KeyCode::Esc | KeyCode::Char('q' | 'h') => self.menu = None,
            _ => {}
        }
    }

    /// Opens or copies item `index` of the menu, and closes it.
    fn pick(&mut self, index: usize) {
        let Some(mut menu) = self.menu.take() else {
            return;
        };
        let target = menu.targets.swap_remove(index);
        match menu.action {
            MenuAction::Open => self.open_target(target),
            MenuAction::Copy => self.copy_target(target),
        }
    }

    /// The `?` popup takes all keys while it's up. Tab (or h/l) switches
    /// between the shortcuts and the settings. On the settings, Enter or
    /// Space changes the one under the cursor and saves it at once.
    fn on_settings_key(&mut self, key: KeyEvent, ctrl: bool) {
        let Some(menu) = self.settings_menu.as_mut() else {
            return;
        };
        match key.code {
            KeyCode::Tab | KeyCode::BackTab | KeyCode::Char('h' | 'l') => {
                menu.tab = match menu.tab {
                    HelpTab::Shortcuts => HelpTab::Settings,
                    HelpTab::Settings => HelpTab::Shortcuts,
                };
                return;
            }
            KeyCode::Enter | KeyCode::Char(' ') if menu.tab == HelpTab::Settings => {
                self.change_setting();
                return;
            }
            KeyCode::Esc | KeyCode::Char('q' | '?') => {
                self.settings_menu = None;
                return;
            }
            _ => {}
        }
        let delta = match key.code {
            KeyCode::Char('j') | KeyCode::Down => 1,
            KeyCode::Char('k') | KeyCode::Up => -1,
            KeyCode::Char('d') if ctrl => HALF_PAGE,
            KeyCode::Char('u') if ctrl => -HALF_PAGE,
            KeyCode::Char('g') => isize::MIN,
            KeyCode::Char('G') => isize::MAX,
            _ => return,
        };
        match menu.tab {
            // Drawing stops it at the end of the list.
            HelpTab::Shortcuts => menu.scroll = menu.scroll.saturating_add_signed(delta),
            HelpTab::Settings => {
                let last = SettingsMenu::THEMES + self.themes.list.len() - 1;
                menu.selected = menu.selected.saturating_add_signed(delta).min(last);
            }
        }
    }

    /// Enter or Space on the settings tab: picks the theme under the cursor,
    /// or turns the setting under it on or off. Saved at once.
    fn change_setting(&mut self) {
        let Some(menu) = self.settings_menu.as_mut() else {
            return;
        };
        let settings = &mut self.settings;
        match menu.selected {
            i if i >= SettingsMenu::THEMES => {
                let Some(theme) = self.themes.list.get(i - SettingsMenu::THEMES) else {
                    return;
                };
                match &theme.colors {
                    Ok(colors) => {
                        self.colors = *colors;
                        settings.theme = theme.id.clone();
                    }
                    Err(e) => {
                        self.status = Some(format!("Theme not used: {e}"));
                        return;
                    }
                }
            }
            SettingsMenu::NOTIFICATIONS => {
                // Back on, they go out the way they did before, e.g. "bell".
                let on = match menu.saved_notifications {
                    Notifications::Off => Notifications::Auto,
                    saved => saved,
                };
                let next = match settings.notifications {
                    Notifications::Off => on,
                    _ => Notifications::Off,
                };
                self.set_notifications(next);
            }
            SettingsMenu::CHAT_GAPS => settings.chat_gaps = !settings.chat_gaps,
            SettingsMenu::LIST_RIGHT => {
                settings.chat_list_side = match settings.chat_list_side {
                    Side::Left => Side::Right,
                    Side::Right => Side::Left,
                };
            }
            SettingsMenu::BLOCK_GAPS => settings.block_gaps = !settings.block_gaps,
            SettingsMenu::AFTER_SEND => settings.normal_after_send = !settings.normal_after_send,
            _ => return,
        }
        if let Err(e) = self.settings.save(&self.settings_path) {
            self.status = Some(format!("Settings not saved: {e:#}"));
        }
    }

    /// Takes effect at once; the settings popup saves it.
    fn set_notifications(&mut self, notifications: Notifications) {
        self.settings.notifications = notifications;
        self.notify_with = notifications.resolve(|name| std::env::var(name).ok());
        if notifications == Notifications::Off {
            self.notifier.clear();
        }
    }

    fn load_older_messages(&mut self) {
        let Some(open) = self.open.as_mut() else {
            return;
        };
        if open.loading.is_some() || open.all_loaded {
            return;
        }
        let page = open.oldest_id().map_or(Page::Latest, Page::Older);
        open.loading = Some(page);
        self.tg.load_history(open.chat_id, page, HISTORY_PAGE);
    }

    /// Only needed after jumping to an old message.
    fn load_newer_messages(&mut self) {
        let Some(open) = self.open.as_mut() else {
            return;
        };
        if open.loading.is_some() || open.at_newest {
            return;
        }
        let Some(newest) = open.newest_id() else {
            return;
        };
        let page = Page::Newer(newest);
        open.loading = Some(page);
        self.tg.load_history(open.chat_id, page, HISTORY_PAGE);
    }

    fn move_message_cursor(&mut self, delta: isize) {
        let Some(open) = self.open.as_mut() else {
            return;
        };
        // `G` goes to the real newest message, not the newest loaded one.
        if delta == isize::MAX && !open.at_newest {
            self.jump_to_newest();
            return;
        }
        let Some(index) = open.move_cursor(delta) else {
            return;
        };
        let len = open.messages.len();
        if index < LOAD_AHEAD {
            self.load_older_messages();
        }
        if index + LOAD_AHEAD >= len {
            self.load_newer_messages();
        }
    }

    /// Moves the chat list cursor by `delta` rows, clamped to the list.
    fn move_chat_cursor(&mut self, delta: isize) {
        let ids = self.chats.ids();
        if ids.is_empty() {
            return;
        }
        let last = ids.len() - 1;
        let index = match self
            .selected
            .and_then(|id| ids.iter().position(|&x| x == id))
        {
            Some(current) => current.saturating_add_signed(delta).min(last),
            None => 0,
        };
        self.selected = Some(ids[index]);

        if index + LOAD_AHEAD >= last {
            self.load_more_chats();
        }
    }

    fn load_more_chats(&mut self) {
        if self.chats_loading || self.all_chats_loaded {
            return;
        }
        self.chats_loading = true;
        self.tg.load_chats(CHAT_PAGE);
    }

    /// `H`: marks the selected chat so it's easy to find, or unmarks it.
    fn toggle_highlight(&mut self) {
        let Some(chat_id) = self.selected else {
            return;
        };
        self.settings.highlighted_chats = self.chats.toggle_highlight(chat_id);
        if let Err(e) = self.settings.save(&self.settings_path) {
            self.status = Some(format!("Highlight not saved: {e:#}"));
        }
    }

    fn quit(&mut self) {
        if self.quit_deadline.is_some() {
            // Second press: stop waiting for TDLib.
            self.exit = true;
            return;
        }
        self.tg.close();
        self.quit_deadline = Some(Instant::now() + CLOSE_TIMEOUT);
    }
}

/// SIGHUP (the terminal closed, an SSH connection dropped) and SIGTERM, one
/// message each. They'd otherwise end tuigram on the spot, leaving the user
/// online on Telegram for minutes.
fn quit_signals() -> UnboundedReceiver<()> {
    let (tx, rx) = tokio::sync::mpsc::unbounded_channel();
    #[cfg(unix)]
    tokio::spawn(async move {
        use tokio::signal::unix::{SignalKind, signal};
        let (Ok(mut hangup), Ok(mut terminate)) = (
            signal(SignalKind::hangup()),
            signal(SignalKind::terminate()),
        ) else {
            return;
        };
        loop {
            tokio::select! {
                _ = hangup.recv() => {}
                _ = terminate.recv() => {}
            }
            if tx.send(()).is_err() {
                return;
            }
        }
    });
    #[cfg(not(unix))]
    drop(tx);
    rx
}

/// Hands a file path or web link to the system: the default app for a file
/// (Preview for images on macOS, QuickTime for video…), the browser for a link.
/// No shell is involved, so a `&` or `|` in a link stays part of it: Windows
/// uses ShellExecute rather than `cmd /C start`, which would run what follows.
fn open_externally(target: &str) -> std::io::Result<()> {
    // `open` on macOS waits for LaunchServices, which can take a moment, and
    // reports nothing useful, so it gets a thread of its own.
    if cfg!(target_os = "macos") {
        let target = target.to_string();
        std::thread::spawn(move || open::that_detached(target));
        Ok(())
    } else {
        open::that_detached(target)
    }
}

/// Marks a downloaded file as coming from the internet, as browsers do, so
/// the system's own checks apply when it's opened: Gatekeeper on macOS,
/// SmartScreen and Office's Protected View on Windows. Best effort.
fn mark_downloaded(path: &str) {
    #[cfg(target_os = "macos")]
    {
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |d| d.as_secs());
        let _ = process::Command::new("/usr/bin/xattr")
            .args([
                "-w",
                "com.apple.quarantine",
                &format!("0081;{now:x};tuigram;"),
                path,
            ])
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status();
    }
    #[cfg(windows)]
    {
        let _ = std::fs::write(
            format!("{path}:Zone.Identifier"),
            "[ZoneTransfer]\r\nZoneId=3\r\n",
        );
    }
    #[cfg(not(any(target_os = "macos", windows)))]
    let _ = path;
}

/// A one-line prompt input holding `text`, with the cursor at its end.
fn prompt_input(text: &str) -> TextArea<'static> {
    let mut input = TextArea::new(vec![text.to_string()]);
    input.set_cursor_line_style(Style::default());
    input.set_cursor_style(Style::default().reversed());
    input.move_cursor(ratatui_textarea::CursorMove::End);
    input
}

fn login_screen(step: LoginStep) -> Screen {
    Screen::Login(Box::new(Login::new(step)))
}

pub fn new_composer() -> TextArea<'static> {
    let mut composer = TextArea::default();
    composer.set_cursor_line_style(Style::default());
    composer
}

fn code_destination(kind: &AuthenticationCodeType) -> &'static str {
    match kind {
        AuthenticationCodeType::TelegramMessage(_) => "to your Telegram app on another device",
        AuthenticationCodeType::Sms(_)
        | AuthenticationCodeType::SmsWord(_)
        | AuthenticationCodeType::SmsPhrase(_) => "by SMS",
        AuthenticationCodeType::Call(_) => "by phone call",
        AuthenticationCodeType::MissedCall(_) | AuthenticationCodeType::FlashCall(_) => {
            "as a missed call: enter the last digits of the calling number"
        }
        AuthenticationCodeType::Fragment(_) => "to fragment.com",
        _ => "to one of your devices",
    }
}

/// Seconds since 1970, like the dates TDLib gives.
fn unix_now() -> i32 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_secs() as i32)
}

#[cfg(test)]
mod tests {
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;
    use ratatui::style::Color;
    use ratatui_image::picker::Picker;
    use tokio::sync::mpsc::unbounded_channel;

    use super::*;

    fn press(app: &mut App, code: KeyCode, modifiers: KeyModifiers) {
        app.on_key(KeyEvent::new(code, modifiers));
    }

    /// The screen's rows as text.
    fn screen(app: &mut App) -> Vec<String> {
        screen_of(app, 100)
    }

    fn screen_of(app: &mut App, width: u16) -> Vec<String> {
        let mut terminal = Terminal::new(TestBackend::new(width, 20)).unwrap();
        terminal.draw(|f| ui::draw(f, app)).unwrap();
        let buf = terminal.backend().buffer();
        (0..buf.area.height)
            .map(|y| (0..buf.area.width).map(|x| buf[(x, y)].symbol()).collect())
            .collect()
    }

    #[test]
    fn the_chat_list_can_go_on_the_right_where_h_and_l_follow_it() {
        let dir = std::env::temp_dir().join(format!("tuigram-side-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let images = Images::new(Picker::halfblocks(), unbounded_channel().0);
        let tg = Tg::detached(unbounded_channel().0);
        let mut app = crate::demo::demo_app(tg, images, &dir);
        app.focus = Focus::Chats;
        let none = KeyModifiers::NONE;
        let saved = || Settings::load(&settings::path(&dir)).unwrap();
        // The text left and right of the first pane's top right corner.
        let halves = |app: &mut App| {
            let top = screen(app).remove(0);
            let (left, right) = top.split_once('┐').unwrap();
            (left.to_string(), right.to_string())
        };

        press(&mut app, KeyCode::Char('?'), none);
        press(&mut app, KeyCode::Tab, none);
        app.settings_menu.as_mut().unwrap().selected = SettingsMenu::LIST_RIGHT;
        press(&mut app, KeyCode::Char(' '), none);
        assert_eq!(saved().chat_list_side, Side::Right, "saved at once");
        press(&mut app, KeyCode::Enter, none);
        assert_eq!(saved().chat_list_side, Side::Left, "Enter toggles too");
        assert!(app.settings_menu.is_some(), "the popup stays open");
        press(&mut app, KeyCode::Enter, none);
        press(&mut app, KeyCode::Esc, none);
        assert!(app.settings_menu.is_none());
        assert_eq!(saved().chat_list_side, Side::Right, "Esc keeps it");
        let (left, right) = halves(&mut app);
        assert!(
            left.contains("Weekend Hike") && right.contains("Chats ("),
            "{left}┐{right}"
        );

        press(&mut app, KeyCode::Char('l'), none);
        assert!(app.focus == Focus::Chats, "l points away from the chat");
        press(&mut app, KeyCode::Char('h'), none);
        assert!(
            app.focus == Focus::Messages,
            "h goes to the chat, on the left"
        );
        // Wide enough for every hint.
        let status = screen_of(&mut app, 300).pop().unwrap();
        assert!(
            status.contains("l back") && !status.contains("h back"),
            "{status}"
        );
        press(&mut app, KeyCode::Char('h'), none);
        assert!(app.focus == Focus::Messages);
        press(&mut app, KeyCode::Char('l'), none);
        assert!(app.focus == Focus::Chats, "l goes back to the list");

        // h moves the line left, which widens a list on the right.
        press(&mut app, KeyCode::Char('r'), KeyModifiers::CONTROL);
        press(&mut app, KeyCode::Char('h'), none);
        assert_eq!(app.settings.list_width(), 40);
        press(&mut app, KeyCode::Esc, none);
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn without_focus_reports_a_minute_without_a_key_counts_as_away() {
        let images = Images::new(Picker::halfblocks(), unbounded_channel().0);
        let tg = Tg::detached(unbounded_channel().0);
        let mut app = crate::demo::demo_app(tg, images, &std::env::temp_dir());
        app.focus = Focus::Messages;
        let chat = app.open.as_ref().unwrap().chat_id;
        assert!(
            app.watching(chat) && app.sees(chat),
            "a key was just pressed"
        );

        // tmux without focus-events, or a detached session.
        app.last_input = Instant::now()
            .checked_sub(IDLE_AFTER + Duration::from_secs(1))
            .unwrap();
        assert!(!app.watching(chat), "new messages aren't marked read");
        assert!(!app.sees(chat), "and they notify");

        // A terminal that reports focus is believed instead.
        app.focus_reported = true;
        assert!(app.watching(chat) && app.sees(chat));
        app.terminal_focused = false;
        assert!(!app.watching(chat) && !app.sees(chat));
    }

    #[test]
    fn once_the_terminal_is_gone_nothing_more_is_marked_read() {
        let images = Images::new(Picker::halfblocks(), unbounded_channel().0);
        let tg = Tg::detached(unbounded_channel().0);
        let mut app = crate::demo::demo_app(tg, images, &std::env::temp_dir());
        app.focus = Focus::Messages;
        let chat = app.open.as_ref().unwrap().chat_id;
        assert!(app.watching(chat));
        // Already closing, so the detached client gets no request.
        app.quit_deadline = Some(Instant::now() + CLOSE_TIMEOUT);
        app.hang_up();
        assert!(!app.watching(chat) && !app.sees(chat));
    }

    #[test]
    fn a_theme_is_used_and_saved_as_soon_as_it_is_picked() {
        let dir = std::env::temp_dir().join(format!("tuigram-theme-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let images = Images::new(Picker::halfblocks(), unbounded_channel().0);
        let tg = Tg::detached(unbounded_channel().0);
        let mut app = crate::demo::demo_app(tg, images, &dir);
        let none = KeyModifiers::NONE;
        press(&mut app, KeyCode::Char('?'), none);
        press(&mut app, KeyCode::Tab, none);
        // The themes are the last rows.
        press(&mut app, KeyCode::Char('G'), none);
        assert_eq!(app.settings.theme, "mocha", "moving doesn't change it");
        press(&mut app, KeyCode::Char(' '), none);
        assert_eq!(app.settings.theme, "rose-pine");
        assert_eq!(app.colors, app.themes.colors("rose-pine").unwrap());
        let saved = Settings::load(&settings::path(&dir)).unwrap();
        assert_eq!(saved.theme, "rose-pine");
        press(&mut app, KeyCode::Char('q'), none);
        assert_eq!(app.settings.theme, "rose-pine", "closing keeps it");
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn themes_in_the_data_folder_are_read_again_when_the_popup_opens() {
        let dir = std::env::temp_dir().join(format!("tuigram-own-theme-{}", std::process::id()));
        std::fs::create_dir_all(dir.join("themes")).unwrap();
        let images = Images::new(Picker::halfblocks(), unbounded_channel().0);
        let tg = Tg::detached(unbounded_channel().0);
        let mut app = crate::demo::demo_app(tg, images, &dir);
        let none = KeyModifiers::NONE;
        let file = dir.join("themes/mine.toml");
        app.settings.theme = "mine".into();
        std::fs::write(&file, "inherits = \"mocha\"\n[colors]\nbg = \"#000000\"").unwrap();
        press(&mut app, KeyCode::Char('?'), none);
        assert_eq!(app.colors.bg, Color::Rgb(0, 0, 0));
        assert_eq!(app.status, None);
        press(&mut app, KeyCode::Esc, none);

        // A broken file leaves the default theme in use, saying why.
        std::fs::write(&file, "inherits = 3").unwrap();
        press(&mut app, KeyCode::Char('?'), none);
        assert_eq!(app.colors, Colors::default());
        let status = app.status.clone().unwrap();
        assert!(status.contains("mine.toml: line 1"), "{status}");
        assert_eq!(app.settings.theme, "mine", "kept, for when it's fixed");

        // Picking it says why too, and changes nothing.
        press(&mut app, KeyCode::Tab, none);
        press(&mut app, KeyCode::Char('G'), none);
        press(&mut app, KeyCode::Enter, none);
        assert!(app.status.as_ref().unwrap().contains("mine.toml"));
        assert_eq!(app.colors, Colors::default());
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn ctrl_r_resizes_the_panes_and_enter_keeps_it_for_next_time() {
        let dir = std::env::temp_dir().join(format!("tuigram-resize-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let images = Images::new(Picker::halfblocks(), unbounded_channel().0);
        let tg = Tg::detached(unbounded_channel().0);
        let mut app = crate::demo::demo_app(tg, images, &dir);
        app.focus = Focus::Messages;
        // Where the chat list's top right corner is.
        let edge = |rows: &[String]| rows[0].chars().position(|c| c == '┐').unwrap();
        let none = KeyModifiers::NONE;
        assert_eq!(edge(&screen(&mut app)), 34, "35% of 100 columns");

        press(&mut app, KeyCode::Char('r'), KeyModifiers::CONTROL);
        assert!(app.open.as_ref().unwrap().reply.is_none(), "not a reply");
        press(&mut app, KeyCode::Char('l'), none);
        press(&mut app, KeyCode::Char('l'), none);
        let rows = screen(&mut app);
        assert_eq!(edge(&rows), 44, "wider as you press");
        let status = rows.last().unwrap();
        assert!(
            status.contains(" RESIZE ") && status.contains("chat list 45%"),
            "{status}"
        );

        press(&mut app, KeyCode::Enter, none);
        assert!(app.resizing.is_none());
        let saved = Settings::load(&settings::path(&dir)).unwrap();
        assert_eq!(saved.chat_list_width, 45);

        press(&mut app, KeyCode::Char('r'), KeyModifiers::CONTROL);
        press(&mut app, KeyCode::Char('h'), none);
        assert_eq!(edge(&screen(&mut app)), 39);
        press(&mut app, KeyCode::Esc, none);
        assert_eq!(edge(&screen(&mut app)), 44, "Esc puts it back");
        assert!(app.focus == Focus::Messages, "and stays in the chat");
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn only_files_that_cant_run_code_open_without_asking() {
        for path in [
            "/d/photo.JPG",
            "/d/clip.mp4",
            "/d/report.pdf",
            "/d/notes.txt",
        ] {
            assert!(safe_to_open(path), "{path}");
        }
        for path in [
            "/d/setup.exe",
            "/d/run.bat",
            "/d/x.command",
            "/d/x.terminal",
            "/d/app.jar",
            "/d/page.html",
            "/d/invoice.pdf.exe",
            "/d/no-extension",
            "/d/data.csv",
        ] {
            assert!(!safe_to_open(path), "{path}");
        }
    }
}

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

use crate::chats::Chats;
use crate::clipboard::{Clipboard, Copied, Decoded};
use crate::config::{self, ApiKeys};
use crate::images::{ImageEvent, Images};
use crate::messages::{Link, MediaFile, OpenChat, Replied, SendState};
use crate::notify::{self, Note, Notifications, Notifier};
use crate::search::MessageSearch;
use crate::settings::Settings;
use crate::text;
use crate::tg::{Deletable, Found, Page, Tagged, Tg, TgEvent};
use crate::theme::Theme;
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
/// tab, moving the cursor previews a theme and Space ticks a checkbox
/// (notifications, Normal mode after sending); Enter saves, Esc puts the
/// saved settings back.
pub struct SettingsMenu {
    pub tab: HelpTab,
    /// First row shown on the shortcuts tab. Drawing keeps it in range.
    pub scroll: usize,
    /// Row on the settings tab: a theme, [`SettingsMenu::NOTIFICATIONS`] or
    /// [`SettingsMenu::AFTER_SEND`].
    pub selected: usize,
    pub saved: Theme,
    pub saved_notifications: Notifications,
    pub saved_normal_after_send: bool,
}

impl SettingsMenu {
    /// The notifications row, after the themes.
    pub const NOTIFICATIONS: usize = Theme::ALL.len();
    /// The "Normal mode after sending" row, the last one.
    pub const AFTER_SEND: usize = Self::NOTIFICATIONS + 1;
}

/// What the status bar prompt is for: a `/` search through chat titles or the
/// open chat's messages, or a `:` command.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum PromptKind {
    Chats,
    Messages,
    Command,
}

/// The prompt in the status bar. Searching chats filters the list as you
/// type; searching messages asks TDLib on Enter, and so does a command.
pub struct Prompt {
    pub kind: PromptKind,
    pub input: TextArea<'static>,
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
    pub toast: Option<Toast>,
    /// Shown over everything when Enter or `y` finds several things.
    pub menu: Option<PickMenu>,
    pub delete_menu: Option<DeleteMenu>,
    pub confirm: Option<Confirm>,
    pub settings: Settings,
    settings_path: PathBuf,
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
        Self {
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
            toast: None,
            menu: None,
            delete_menu: None,
            confirm: None,
            settings,
            settings_path,
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
        }
    }

    pub async fn run(
        mut self,
        terminal: &mut DefaultTerminal,
        mut events: UnboundedReceiver<Tagged>,
        mut image_events: UnboundedReceiver<ImageEvent>,
        mut decoded: UnboundedReceiver<Decoded>,
    ) -> Result<()> {
        let mut keys = EventStream::new();
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
            terminal.draw(|frame| ui::draw(frame, &mut self))?;
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
                Some(decoded) = decoded.recv() => self.on_decoded(decoded),
                Some(event) = keys.next() => self.on_terminal_event(event?),
                _ = sleep_until(deadline.unwrap_or_else(Instant::now)), if deadline.is_some() => break,
                _ = sleep_until(wake.unwrap_or_else(Instant::now)), if wake.is_some() => {}
                else => break,
            }
        }
        Ok(())
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
            Event::Paste(text) if self.focus == Focus::Input => {
                self.composer.insert_str(text.replace('\r', ""));
                self.on_composer_edit();
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
            Update::MessageContent(u) => {
                if let Some(open) = self.open.as_mut().filter(|o| o.chat_id == u.chat_id) {
                    open.set_content(u.message_id, &u.new_content);
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
            Screen::Main if self.menu.is_some() => self.on_menu_key(key),
            Screen::Main if self.prompt.is_some() => self.on_prompt_key(key, ctrl),
            Screen::Main if self.focus == Focus::Input => self.on_insert_key(key, ctrl),
            Screen::Main => self.on_normal_key(key, ctrl),
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
            (_, KeyCode::Char('g')) => self.pending_g = true,
            (_, KeyCode::Char('q')) => self.quit(),
            (_, KeyCode::Char('H')) => self.toggle_highlight(),
            (_, KeyCode::Char('?')) => {
                let saved = self.settings.theme;
                self.settings_menu = Some(SettingsMenu {
                    tab: HelpTab::Shortcuts,
                    scroll: 0,
                    selected: Theme::ALL.iter().position(|&t| t == saved).unwrap_or(0),
                    saved,
                    saved_notifications: self.settings.notifications,
                    saved_normal_after_send: self.settings.normal_after_send,
                });
            }
            (_, KeyCode::Char(':')) => self.open_prompt(PromptKind::Command),
            (Focus::Chats, KeyCode::Char('/')) => self.open_prompt(PromptKind::Chats),
            (Focus::Messages, KeyCode::Char('/')) => self.open_prompt(PromptKind::Messages),
            (Focus::Messages, KeyCode::Char('n')) => self.next_match(1),
            (Focus::Messages, KeyCode::Char('N')) => self.next_match(-1),
            // Esc ends a search, then a reply, before it leaves the pane.
            (Focus::Chats, KeyCode::Esc) => self.chats.set_filter(""),
            (Focus::Messages, KeyCode::Esc)
                if self.open.as_ref().is_some_and(|o| o.search.is_some()) =>
            {
                if let Some(open) = self.open.as_mut() {
                    open.search = None;
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
            (Focus::Messages, KeyCode::Char('y')) => self.copy_selected(),
            (Focus::Messages, KeyCode::Char('d')) if pending_g => self.go_to_replied(),
            (Focus::Messages, KeyCode::Char('d')) => self.open_delete_menu(),
            (Focus::Messages, KeyCode::Char('o')) if ctrl => self.jump_back(),
            (Focus::Chats, KeyCode::Enter | KeyCode::Char('l')) => self.open_selected_chat(),
            (Focus::Chats, KeyCode::Char('i')) => {
                self.open_selected_chat();
                if self.open.is_some() {
                    self.focus = Focus::Input;
                }
            }
            (Focus::Messages, KeyCode::Char('i')) => self.focus = Focus::Input,
            (Focus::Messages, KeyCode::Enter) => self.open_selected_message(),
            (Focus::Messages, KeyCode::Esc | KeyCode::Char('h')) => self.focus = Focus::Chats,
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
    fn on_composer_edit(&mut self) {
        let typing = self.composer.lines().iter().any(|l| !l.trim().is_empty());
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
        let Some(open) = self.open.as_mut() else {
            return;
        };
        let text = self.composer.lines().join("\n");
        let text = text.trim();
        if text.is_empty() {
            return;
        }
        let reply_to = open.reply.take().map(|r| r.id);
        self.tg.send_text(open.chat_id, text.to_string(), reply_to);
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
        let mut input = TextArea::default();
        input.set_cursor_line_style(Style::default());
        input.set_cursor_style(Style::default().reversed());
        self.prompt = Some(Prompt {
            kind,
            input,
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
            _ => {
                prompt.input.input(key);
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
            PromptKind::Command if !submit || query.is_empty() => {}
            PromptKind::Command => match Command::parse(&query) {
                Some(Command::Logout) => self.log_out(),
                None => self.status = Some(format!("Not a command: {query}")),
            },
        }
    }

    /// Sends a read receipt for the newest incoming message once you can
    /// see it: the chat pane and the terminal window have focus, and the view
    /// is on the newest message. Viewing it marks the whole chat as read.
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
    /// ones are seen as they arrive.
    fn watching(&self, chat_id: i64) -> bool {
        matches!(self.screen, Screen::Main)
            && self.terminal_focused
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
    /// between the shortcuts and the settings.
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
            KeyCode::Enter => {
                self.settings_menu = None;
                if let Err(e) = self.settings.save(&self.settings_path) {
                    self.status = Some(format!("Settings not saved: {e:#}"));
                }
                return;
            }
            KeyCode::Esc | KeyCode::Char('q' | '?') => {
                self.settings.theme = menu.saved;
                self.settings.normal_after_send = menu.saved_normal_after_send;
                let saved = menu.saved_notifications;
                self.settings_menu = None;
                self.set_notifications(saved);
                return;
            }
            KeyCode::Char(' ')
                if menu.tab == HelpTab::Settings
                    && menu.selected == SettingsMenu::NOTIFICATIONS =>
            {
                // Back on, they go out the way they did before, e.g. "bell".
                let on = match menu.saved_notifications {
                    Notifications::Off => Notifications::Auto,
                    saved => saved,
                };
                self.set_notifications(match self.settings.notifications {
                    Notifications::Off => on,
                    _ => Notifications::Off,
                });
                return;
            }
            KeyCode::Char(' ')
                if menu.tab == HelpTab::Settings && menu.selected == SettingsMenu::AFTER_SEND =>
            {
                self.settings.normal_after_send = !self.settings.normal_after_send;
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
                let last = SettingsMenu::AFTER_SEND;
                menu.selected = menu.selected.saturating_add_signed(delta).min(last);
                // Preview: the whole app redraws in the theme under the cursor.
                if let Some(&theme) = Theme::ALL.get(menu.selected) {
                    self.settings.theme = theme;
                }
            }
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
    use super::*;

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

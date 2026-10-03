use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::Duration;

use anyhow::Result;
use crossterm::event::{Event, EventStream, KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
use futures::StreamExt;
use ratatui::DefaultTerminal;
use ratatui::style::Style;
use ratatui::widgets::Block;
use ratatui_textarea::TextArea;
use tdlib_rs::enums::{AuthenticationCodeType, AuthorizationState, OptionValue, Update};
use tokio::sync::mpsc::UnboundedReceiver;
use tokio::time::{Instant, sleep_until};

use crate::chats::Chats;
use crate::clipboard::{Clipboard, Copied, Decoded};
use crate::config::{self, ApiKeys};
use crate::images::{ImageEvent, Images};
use crate::messages::{MediaFile, OpenChat, Replied, SendState};
use crate::search::MessageSearch;
use crate::settings::Settings;
use crate::tg::{Deletable, Found, Page, Tg, TgEvent};
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

pub enum Screen {
    Login(Box<Login>),
    Main,
}

pub enum LoginStep {
    Connecting,
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
            LoginStep::Connecting | LoginStep::OtherDevice { .. } | LoginStep::Unsupported(_)
        )
    }
}

/// Something in a message that Enter opens or `y` copies.
pub enum Target {
    File(MediaFile),
    Link(String),
    /// The whole text or caption; only copied.
    Text(String),
}

impl Target {
    pub fn label(&self) -> &str {
        match self {
            Target::File(file) => &file.label,
            Target::Link(url) => url,
            Target::Text(_) => "Whole message",
        }
    }
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
        self.selected = 0;
    }
}

/// The tabs of the `?` popup.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum HelpTab {
    Shortcuts,
    Settings,
}

/// The `?` popup: every keyboard shortcut, and the settings. On the settings
/// tab, moving the cursor previews a theme; Esc puts the saved one back.
pub struct SettingsMenu {
    pub tab: HelpTab,
    /// First row shown on the shortcuts tab. Drawing keeps it in range.
    pub scroll: usize,
    pub selected: usize,
    pub saved: Theme,
}

/// What a `/` search looks through: chat titles or the open chat's messages.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum SearchTarget {
    Chats,
    Messages,
}

/// The `/` prompt in the status bar. Searching chats filters the list as you
/// type; searching messages asks TDLib on Enter.
pub struct SearchPrompt {
    pub target: SearchTarget,
    pub input: TextArea<'static>,
    /// The chat filter and cursor from before, which Esc puts back.
    previous_filter: String,
    previous_selected: Option<i64>,
}

impl SearchPrompt {
    pub fn query(&self) -> String {
        self.input.lines().concat()
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
    pub settings: Settings,
    settings_path: PathBuf,
    /// API credentials from the environment, which win over saved ones.
    env_keys: Option<ApiKeys>,
    pub settings_menu: Option<SettingsMenu>,
    /// Shown in the status bar while typing a `/` search.
    pub prompt: Option<SearchPrompt>,
    pub chats_loading: bool,
    all_chats_loaded: bool,
    /// Last error, shown in the status bar until the next key press.
    pub status: Option<String>,
    /// First `g` of `gg` was pressed.
    pending_g: bool,
    /// Set once quitting started; we exit at this time even if TDLib never answers.
    pub quit_deadline: Option<Instant>,
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
            settings,
            settings_path,
            env_keys,
            settings_menu: None,
            prompt: None,
            chats_loading: false,
            all_chats_loaded: false,
            status: None,
            pending_g: false,
            quit_deadline: None,
            exit: false,
        }
    }

    pub async fn run(
        mut self,
        terminal: &mut DefaultTerminal,
        mut events: UnboundedReceiver<TgEvent>,
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
            terminal.draw(|frame| ui::draw(frame, &mut self))?;
            // Start downloads/encodes for photos the frame showed but didn't have.
            self.images.fetch(&self.tg);
            if let Some(open) = self.open.as_mut() {
                for id in open.missing_replied() {
                    self.tg.get_replied_message(open.chat_id, id);
                }
            }

            let deadline = self.quit_deadline;
            let toast_until = self.toast.as_ref().map(|t| t.until);
            tokio::select! {
                Some(event) = events.recv() => {
                    self.on_tg(event);
                    // Drain the backlog so a burst of updates costs one redraw.
                    while let Ok(event) = events.try_recv() {
                        self.on_tg(event);
                    }
                }
                Some(event) = image_events.recv() => self.images.on_built(event),
                Some(decoded) = decoded.recv() => self.on_decoded(decoded),
                Some(event) = keys.next() => match event? {
                    Event::Key(key) => self.on_key(key),
                    Event::Paste(text) if self.prompt.is_some() => {
                        if let Some(prompt) = self.prompt.as_mut() {
                            prompt.input.insert_str(text.replace(['\r', '\n'], " "));
                        }
                        self.on_prompt_edit();
                    }
                    Event::Paste(text) if self.focus == Focus::Input => {
                        self.composer.insert_str(text.replace('\r', ""));
                    }
                    _ => {}
                },
                _ = sleep_until(deadline.unwrap_or_else(Instant::now)), if deadline.is_some() => break,
                // Wakes up to take the toast down.
                _ = sleep_until(toast_until.unwrap_or_else(Instant::now)), if toast_until.is_some() => {}
                else => break,
            }
        }
        Ok(())
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
                        Some(path) => {
                            if let Err(e) = open_externally(path) {
                                self.status = Some(format!("Couldn't open file: {e}"));
                            }
                        }
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
        if matches!(page, Page::Latest | Page::Newer(_))
            && open.at_newest
            && let Some(newest) = open.newest_id()
        {
            // Viewing the newest message marks the whole chat as read.
            self.tg.view_messages(chat_id, vec![newest]);
        }
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
            Update::ChatReadInbox(u) => self.chats.set_unread(u.chat_id, u.unread_count),
            Update::Option(u) if u.name == "my_id" => {
                if let OptionValue::Integer(v) = u.value {
                    self.chats.set_my_id(v.value);
                }
            }
            Update::User(u) => {
                let name = format!("{} {}", u.user.first_name, u.user.last_name);
                self.users.insert(u.user.id, name.trim().to_string());
            }
            Update::NewMessage(u) => {
                // While older messages are shown, new ones load with the rest.
                if let Some(open) = self
                    .open
                    .as_mut()
                    .filter(|o| o.chat_id == u.message.chat_id && o.at_newest)
                {
                    let (id, incoming) = (u.message.id, !u.message.is_outgoing);
                    open.insert(u.message);
                    if incoming {
                        self.tg.view_messages(open.chat_id, vec![id]);
                    }
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
            AuthorizationState::WaitTdlibParameters => {
                match self.env_keys.clone().or(self.settings.api_keys.clone()) {
                    Some(keys) => {
                        self.tg.set_tdlib_parameters(keys);
                        return;
                    }
                    None => LoginStep::ApiId,
                }
            }
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
                return;
            }
            AuthorizationState::LoggingOut | AuthorizationState::Closing => return,
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
                self.tg.set_tdlib_parameters(keys);
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
            | LoginStep::ApiId
            | LoginStep::ApiHash { .. }
            | LoginStep::OtherDevice { .. }
            | LoginStep::Unsupported(_) => {}
        }
    }

    /// Telegram refused the API credentials (`API_ID_INVALID` and the like).
    /// TDLib can't take new ones without a restart, so saved ones are forgotten
    /// and the next run asks again.
    fn reject_api_keys(&mut self) {
        let message = if self.env_keys.is_some() {
            "Telegram rejected TG_API_ID / TG_API_HASH. Check them on my.telegram.org.".into()
        } else {
            self.settings.api_keys = None;
            match self.settings.save(&self.settings_path) {
                Ok(()) => {
                    "Telegram rejected the API ID and hash. Restart tuigram to enter them again."
                        .into()
                }
                Err(e) => format!(
                    "Telegram rejected the API ID and hash, and they couldn't be cleared: {e:#}"
                ),
            }
        };
        match &mut self.screen {
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
                });
            }
            (Focus::Chats, KeyCode::Char('/')) => self.open_prompt(SearchTarget::Chats),
            (Focus::Messages, KeyCode::Char('/')) => self.open_prompt(SearchTarget::Messages),
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
            KeyCode::Esc => self.focus = Focus::Messages,
            KeyCode::Char('c') if ctrl => self.focus = Focus::Messages,
            // Shift-Enter only arrives on terminals with the kitty keyboard protocol;
            // Alt-Enter and Ctrl-j work everywhere.
            KeyCode::Enter if alt || shift => self.composer.insert_newline(),
            KeyCode::Char('j') if ctrl => self.composer.insert_newline(),
            KeyCode::Enter => self.send(),
            _ => {
                self.composer.input(key);
            }
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
        // Jump to the bottom to watch it arrive.
        self.jump_to_newest();
    }

    fn open_prompt(&mut self, target: SearchTarget) {
        let mut input = TextArea::default();
        input.set_cursor_line_style(Style::default());
        input.set_cursor_style(Style::default().reversed());
        self.prompt = Some(SearchPrompt {
            target,
            input,
            previous_filter: self.chats.filter().to_string(),
            previous_selected: self.selected,
        });
    }

    /// The `/` prompt takes all keys while it's up.
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
        if prompt.target == SearchTarget::Chats {
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
        match prompt.target {
            SearchTarget::Chats => {
                if submit && (query.is_empty() || !self.chats.ids().is_empty()) {
                    return;
                }
                if submit {
                    self.status = Some(format!("No chats match \"{query}\""));
                }
                self.chats.set_filter(&prompt.previous_filter);
                self.selected = prompt.previous_selected;
            }
            SearchTarget::Messages => {
                let Some(open) = self.open.as_mut().filter(|_| submit && !query.is_empty()) else {
                    return;
                };
                open.search = Some(MessageSearch::new(query));
                self.go_to_match(0);
            }
        }
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
            Target::Link(url) => {
                if let Err(e) = open_externally(&url) {
                    self.status = Some(format!("Couldn't open link: {e}"));
                }
            }
            Target::Text(_) => {}
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
            Target::Link(url) => url,
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
                let last = Theme::ALL.len() - 1;
                menu.selected = menu.selected.saturating_add_signed(delta).min(last);
                // Preview: the whole app redraws in the theme under the cursor.
                self.settings.theme = Theme::ALL[menu.selected];
            }
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
/// Callers only pass TDLib file paths and http(s) links, so nothing starting
/// with `-` can be mistaken for an option.
fn open_externally(target: &str) -> std::io::Result<()> {
    #[cfg(target_os = "macos")]
    let mut command = Command::new("open");
    #[cfg(target_os = "windows")]
    let mut command = {
        let mut c = Command::new("cmd");
        c.args(["/C", "start", ""]);
        c
    };
    #[cfg(all(unix, not(target_os = "macos")))]
    let mut command = Command::new("xdg-open");

    // Keep the opener's output off the TUI, and reap it when it exits.
    let mut child = command
        .arg(target)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()?;
    std::thread::spawn(move || child.wait());
    Ok(())
}

fn login_screen(step: LoginStep) -> Screen {
    Screen::Login(Box::new(Login::new(step)))
}

fn new_composer() -> TextArea<'static> {
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

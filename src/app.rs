use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::process::{self, Stdio};
use std::time::{Duration, SystemTime};

use anyhow::Result;
use crossterm::event::{Event, EventStream, KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
use futures::StreamExt;
use ratatui::DefaultTerminal;
use ratatui::style::Style;
use ratatui::widgets::Block;
use ratatui_textarea::{DataCursor, TextArea};
use tdlib_rs::enums::{
    AuthenticationCodeType, AuthorizationState, ChatMemberStatus, MessageSender, NotificationType,
    OptionValue, Update, UserType,
};
use tdlib_rs::types::{Message, UpdateNotificationGroup};
use tokio::sync::mpsc::UnboundedReceiver;
use tokio::time::{Instant, sleep_until};

use crate::attach::{self, Attachment, Dropped};
use crate::buttons::{ButtonMenu, Press};
use crate::chats::{Badge, Chats, List, Peer, Presence};
use crate::clipboard::{Clipboard, ClipboardEvent, Copied, Decoded, Paste, Pasted};
use crate::complete::{self, Commands, Completion, Kind, Suggestion};
use crate::config::{self, ApiKeys};
use crate::images::{ImageEvent, Images};
use crate::messages::{
    Editable, Editing, Link, MediaFile, OpenChat, Replied, SendState, Sender, link_host, one_line,
    web_url,
};
use crate::notify::{self, Note, Notifications, Notifier};
use crate::picker::{self, ChatPicker, Choice, Purpose};
use crate::pins::{PinMenu, Pinned, PinnedMenu, Place};
use crate::poll::{Vote, VoteMenu};
use crate::reactions::{self, ReactMenu, ReactionKind};
use crate::search::{self, MessageSearch, Who};
use crate::secret::{KeyView, Secret, SecretState, TimerMenu};
use crate::settings::{self, Settings, Side};
use crate::stickers::{self, Source, StickerPanel};
use crate::text;
use crate::tg::{Deletable, EditText, Found, Invite, Missed, Page, Tagged, Tg, TgEvent};
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
/// A confirmation ignores `y` for this long after it comes up.
const CONFIRM_GRACE: Duration = Duration::from_millis(500);
/// Without a key press for this long, you're no longer shown as online.
const IDLE_AFTER: Duration = Duration::from_secs(60);
/// Even in a window the terminal says has focus, nothing is marked read
/// after this long without a key: the screen can be left showing while
/// nobody is at it, or the connection behind it can be gone without the
/// terminal saying so.
const AWAY_AFTER: Duration = Duration::from_secs(5 * 60);
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
    /// For a link: the site it goes to, on a line of its own. A site's name
    /// is at the end of its host, so a long one is cut from the left.
    pub site: Option<String>,
    /// For a chat to join: what Telegram says about it, on a line of its
    /// own at the top.
    pub badge: Option<Badge>,
    pub action: Confirmed,
    /// When it came up. A `y` in the first moments was typed for whatever
    /// was on screen before, so it doesn't count.
    pub shown: Instant,
}

impl Confirm {
    pub fn new(title: impl Into<String>, lines: Vec<String>, action: Confirmed) -> Self {
        Self {
            title: title.into(),
            lines,
            site: None,
            badge: None,
            action,
            shown: Instant::now(),
        }
    }
}

/// What `y` does in a [`Confirm`].
pub enum Confirmed {
    OpenFile(String),
    OpenLink(String),
    /// Edit this message, starting from this text, losing the formatting
    /// Markdown can't write.
    Edit {
        id: i64,
        text: String,
    },
    /// Join this public group or channel, to write in it.
    Join(i64),
    /// Join the chat this invite link leads to, then open it.
    JoinLink {
        link: String,
        request: String,
    },
    /// Leave this group or channel.
    Leave(i64),
    /// Log out of Telegram on this computer.
    Logout,
    /// Start a secret chat with this person, named `with`.
    StartSecret {
        user_id: i64,
        with: String,
    },
    /// End this secret chat, and delete it from this computer.
    EndSecret {
        chat_id: i64,
        secret_id: i32,
    },
}

impl Confirmed {
    /// What `y` does, for the key hints.
    pub fn verb(&self) -> &'static str {
        match self {
            Confirmed::OpenFile(_) | Confirmed::OpenLink(_) => "open",
            Confirmed::Edit { .. } => "edit",
            Confirmed::Join(_) | Confirmed::JoinLink { .. } => "join",
            Confirmed::Leave(_) => "leave",
            Confirmed::Logout => "log out",
            Confirmed::StartSecret { .. } => "start it",
            Confirmed::EndSecret { .. } => "end it",
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

/// The message holds media its sender wants seen only while it's open: its
/// timer starts once it's opened, so it isn't handed to another app, which
/// would keep it.
fn opens_once(msg: &crate::messages::Msg) -> bool {
    msg.destruct.is_some_and(|d| d.on_open)
}

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

/// A bot's alert, answering a button: shown until Enter or Esc.
pub struct Notice {
    /// The button's words.
    pub title: String,
    pub text: String,
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
/// and in message blocks, Normal mode after sending, taking secret chats) or
/// picks a theme, and saves it at once.
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
    /// The "take secret chats others start" row.
    pub const SECRET_CHATS: usize = Self::AFTER_SEND + 1;
    /// The first theme's row. The themes come last, since the user's own
    /// can make a long list.
    pub const THEMES: usize = Self::SECRET_CHATS + 1;

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

/// Places Ctrl-o and Ctrl-i go back to at most.
const MAX_JUMPS: usize = 100;

/// A place to come back to: a chat, and the message the cursor was on
/// (`None` for the newest, following new ones).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Jump {
    pub chat_id: i64,
    pub message_id: Option<i64>,
}

/// Where Ctrl-o goes back to and Ctrl-i forward to again, as vim's jump
/// list: chats you left for another, and replies `gd` left.
#[derive(Default)]
pub struct Jumps {
    back: Vec<Jump>,
    forward: Vec<Jump>,
}

impl Jumps {
    /// Leaving `from` for somewhere new: Ctrl-o comes back to it, and the
    /// places Ctrl-o had come back from are forgotten.
    pub fn leave(&mut self, from: Jump) {
        self.forward.clear();
        if self.back.last() != Some(&from) {
            self.back.push(from);
        }
        if self.back.len() > MAX_JUMPS {
            self.back.remove(0);
        }
    }

    /// Ctrl-o (`back`) or Ctrl-i: where to go from `here`, which the other
    /// one then comes back to.
    fn go(&mut self, back: bool, here: Option<Jump>) -> Option<Jump> {
        let (from, to) = if back {
            (&mut self.back, &mut self.forward)
        } else {
            (&mut self.forward, &mut self.back)
        };
        let next = from.pop()?;
        to.extend(here);
        Some(next)
    }

    /// Ctrl-o has somewhere to go.
    pub fn can_go_back(&self) -> bool {
        !self.back.is_empty()
    }

    /// Ctrl-i has somewhere to go.
    pub fn can_go_forward(&self) -> bool {
        !self.forward.is_empty()
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
    /// In the `:` prompt, Tab goes through the commands that start with
    /// what was typed: this is what was typed, and which of them Tab put
    /// in. Typing clears it.
    pub tabbed: Option<(String, usize)>,
    /// The chat filter and cursor from before, which Esc puts back.
    previous_filter: String,
    previous_selected: Option<i64>,
}

impl Prompt {
    pub fn query(&self) -> String {
        self.input.lines().concat()
    }

    /// What was typed before the Tabs that went through completions.
    fn typed(&self) -> String {
        match &self.tabbed {
            Some((typed, _)) => typed.clone(),
            None => self.query(),
        }
    }

    /// Tab (`step` 1) or Shift-Tab (-1): puts in the next or previous of
    /// `options`, the ways to finish what was typed, round the end, as vim
    /// does. Enter still runs it.
    fn tab(&mut self, step: isize, typed: String, options: Vec<String>) {
        let at = self.tabbed.take().map(|(_, at)| at);
        let count = options.len() as isize;
        if count == 0 {
            return;
        }
        let next = match at {
            Some(at) => (at as isize + step).rem_euclid(count),
            None if step > 0 => 0,
            None => count - 1,
        } as usize;
        self.input = prompt_input(&options[next]);
        self.tabbed = Some((typed, next));
    }

    /// Tab in the `:` prompt: the commands that start with what was typed.
    fn complete_command(&mut self, step: isize) {
        let typed = self.typed();
        let options = Command::ALL
            .into_iter()
            .map(Command::name)
            .filter(|name| name.starts_with(typed.trim()))
            .map(String::from)
            .collect();
        self.tab(step, typed, options);
    }
}

/// A chat being looked up to open, with `s` or from a t.me link.
pub struct Finding {
    /// What's looked up (a username, a link, a contact's name), for the
    /// status bar and to match TDLib's answer.
    pub request: String,
    /// The link in a message it came from, which a browser opens instead
    /// if tuigram can't.
    pub link: Option<Link>,
}

impl Finding {
    fn new(request: &str) -> Self {
        Self {
            request: request.to_string(),
            link: None,
        }
    }
}

/// What `:` runs. There are no abbreviations: only the full name runs, so a
/// typo can't log you out.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Command {
    Key,
    Leave,
    Logout,
    Secret,
    Timer,
}

impl Command {
    pub const ALL: [Command; 5] = [
        Command::Key,
        Command::Leave,
        Command::Logout,
        Command::Secret,
        Command::Timer,
    ];

    pub fn name(self) -> &'static str {
        match self {
            Command::Key => "key",
            Command::Leave => "leave",
            Command::Logout => "logout",
            Command::Secret => "secret",
            Command::Timer => "timer",
        }
    }

    pub fn about(self) -> &'static str {
        match self {
            Command::Key => "Show a secret chat's key, to compare with the other person's",
            Command::Leave => "Leave this group or channel, or end a secret chat (asks first)",
            Command::Logout => "Log out of Telegram on this computer (asks first)",
            Command::Secret => "Start a secret chat with this person (asks first)",
            Command::Timer => "Set how long messages in a secret chat last once seen",
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
    /// Enter on a poll: its answers to vote for.
    pub vote_menu: Option<VoteMenu>,
    /// Enter on a bot's message with buttons: them, to press.
    pub button_menu: Option<ButtonMenu>,
    /// A bot's alert, answering a button.
    pub notice: Option<Notice>,
    /// `P`: how to pin the message under the cursor.
    pub pin_menu: Option<PinMenu>,
    /// `gp`: the chat's pinned messages, to go to.
    pub pinned_menu: Option<PinnedMenu>,
    /// `:timer`: how long messages in a secret chat last.
    pub timer_menu: Option<TimerMenu>,
    /// `:key`: a secret chat's key, to compare.
    pub key_view: Option<KeyView>,
    /// `f` to forward a message, or `s` to find a chat to open.
    pub picker: Option<ChatPicker>,
    /// A chat being looked up to open. Opening another chat meanwhile
    /// drops the answer.
    pub finding: Option<Finding>,
    /// Suggestions for the `@name` or `:emoji` being typed; only in Insert
    /// mode.
    pub completion: Option<Completion>,
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
    /// Chat lists with a `loadChats` call on its way.
    loading_lists: HashSet<List>,
    /// Chat lists with every chat loaded.
    loaded_lists: HashSet<List>,
    /// Chat lists loaded in the background until every chat is in, since
    /// unread chats go first wherever Telegram has them: the main list, and
    /// the folders shown.
    wanted_lists: HashSet<List>,
    /// Where Ctrl-o and Ctrl-i go.
    pub jumps: Jumps,
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
    /// The same moment by the wall clock. `Instant` stops while the computer
    /// sleeps, so after a wake only this one shows how long you were away.
    last_input_wall: SystemTime,
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
    /// The message whose photo, shown only while open, the last frame
    /// showed.
    shown_viewing: Option<i64>,
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
            vote_menu: None,
            button_menu: None,
            notice: None,
            pin_menu: None,
            pinned_menu: None,
            timer_menu: None,
            key_view: None,
            picker: None,
            finding: None,
            completion: None,
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
            loading_lists: HashSet::new(),
            loaded_lists: HashSet::new(),
            wanted_lists: HashSet::new(),
            jumps: Jumps::default(),
            // Only development builds read `.env`; someone expecting it to
            // pick a separate session should know this one didn't.
            status: config::dotenv_ignored()
                .then(|| "./.env is ignored: only development builds read it".into()),
            pending_g: false,
            quit_deadline: None,
            terminal_focused: true,
            focus_reported: false,
            last_input: Instant::now(),
            last_input_wall: SystemTime::now(),
            online: false,
            typing: None,
            notifier: Notifier::default(),
            notify_with,
            in_tmux: std::env::var_os("TMUX").is_some(),
            notifications_sent: 0,
            notify_since: i32::MAX,
            unread_chats: 0,
            shown_viewing: None,
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
            self.cover_unseen();
            // Sixel and iTerm2 pictures stay on screen until every cell of
            // them is drawn over, which tmux may skip for blank ones.
            let viewing = self.open.as_ref().and_then(|o| o.viewing);
            if self.shown_viewing.is_some()
                && viewing != self.shown_viewing
                && self.images.paints_over()
            {
                let _ = terminal.clear();
            }
            self.shown_viewing = viewing;
            self.mark_seen();
            self.update_online();
            self.send_notification();
            if let Some(query) = self
                .picker
                .as_mut()
                .and_then(|p| p.due_search(Instant::now()))
            {
                self.tg.find_chats(query);
            }
            if let Some(open) = &self.open
                && let Some(query) = self
                    .completion
                    .as_mut()
                    .and_then(|c| c.due_search(Instant::now()))
            {
                self.tg.find_members(open.chat_id, query);
            }
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
            // Wakes up to take the toast down, to go offline when idle, to
            // send notifications that had to wait, to search Telegram once
            // typing in the `s` picker pauses, and to count down messages
            // that self-destruct.
            let countdown = self
                .open
                .as_ref()
                .and_then(|o| o.next_tick(SystemTime::now()))
                .map(|left| Instant::now() + left);
            // And to cover what's shown only while open, once you're away.
            let away = self
                .open
                .as_ref()
                .filter(|o| o.viewing.is_some())
                .map(|_| self.last_input + self.away_after());
            let wake = [
                countdown,
                away,
                self.toast.as_ref().map(|t| t.until),
                self.online.then_some(self.last_input + IDLE_AFTER),
                self.notifier.next_at(),
                self.picker.as_ref().and_then(ChatPicker::search_at),
                self.completion.as_ref().and_then(Completion::search_at),
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
            self.last_input_wall = SystemTime::now();
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
            Event::Paste(text) if self.picker.is_some() => {
                // A pasted link or name goes into the search, on one line.
                let text = text::clean(&text).replace(['\n', '\t'], " ");
                if let Some(picker) = self.picker.as_mut() {
                    picker.edit_query(|q| q.push_str(text.trim()), Instant::now());
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
            TgEvent::ChatsLoaded { list, all, failed } => {
                self.loading_lists.remove(&list);
                if all {
                    self.loaded_lists.insert(list);
                }
                // Unread chats go first wherever Telegram has them, so every
                // chat of a list shown has to be loaded. After an error,
                // scrolling to the end of the list tries again.
                if !failed && self.wanted_lists.contains(&list) {
                    self.load_more_chats(list);
                }
                // A first page of the archive says whether there is one,
                // for its tab.
                if all && list == List::Main {
                    self.load_more_chats(List::Archive);
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
            TgEvent::Pinned {
                chat_id,
                request,
                messages,
            } => self.on_pinned(chat_id, request, messages),
            TgEvent::Pinnable {
                chat_id,
                message_id,
                pinnable,
            } => self.on_pinnable(chat_id, message_id, pinnable),
            TgEvent::Editable {
                chat_id,
                message_id,
                editable,
                text,
            } => self.on_editable(chat_id, message_id, editable, text),
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
                if let Some(open) = self.open.as_mut()
                    && let Some((id, _)) = open.opening.filter(|&(_, f)| f == file_id)
                {
                    open.opening = None;
                    // Not if the cursor left it before it could be seen.
                    if path.is_some() && open.viewing == Some(id) {
                        self.tg.open_content(open.chat_id, id);
                        open.start_timer(id, SystemTime::now());
                    }
                }
                if self.opening.remove(&file_id) {
                    match &path {
                        Some(path) => self.open_downloaded(path.clone()),
                        None => self.status = Some("Download failed".into()),
                    }
                }
                if let Some(file) = self.copying.remove(&file_id) {
                    match &path {
                        // Pasted in a file manager, the copy keeps the mark,
                        // as the file would if opened with Enter.
                        Some(path) => {
                            mark_downloaded(path);
                            self.copy_downloaded(file, path)
                        }
                        None => self.status = Some("Download failed".into()),
                    }
                }
                self.images.on_downloaded(file_id, path);
            }
            TgEvent::Members {
                chat_id,
                query,
                user_ids,
            } => {
                let open = self.open.as_ref().is_some_and(|o| o.chat_id == chat_id);
                let Some(completion) = self.completion.as_mut().filter(|_| open) else {
                    return;
                };
                if completion.set_found(&query, user_ids) {
                    let (query, found) = (completion.word.query.clone(), completion.found.clone());
                    let items = self.mention_items(&query, &found);
                    if let Some(completion) = self.completion.as_mut() {
                        completion.set_items(items);
                    }
                }
            }
            TgEvent::Commands { chat_id, commands } => {
                let Some(open) = self.open.as_mut().filter(|o| o.chat_id == chat_id) else {
                    return;
                };
                open.commands = Commands::Known(commands);
                let query = self
                    .completion
                    .as_ref()
                    .filter(|c| c.word.kind == Kind::Command)
                    .map(|c| c.word.query.clone());
                if let Some(query) = query {
                    let items = self.command_items(&query);
                    if let Some(completion) = self.completion.as_mut() {
                        completion.set_items(items);
                    }
                }
            }
            TgEvent::ChatsFound {
                query,
                chat_ids,
                user_ids,
            } => {
                if let Some(picker) = self.picker.as_mut() {
                    picker.set_found(&query, chat_ids, user_ids);
                }
            }
            TgEvent::ChatFound { request, found } => {
                // Dropped if another chat was opened meanwhile.
                if !self.finding.as_ref().is_some_and(|f| f.request == request) {
                    return;
                }
                let link = self.finding.take().and_then(|f| f.link);
                // Not while the keys go somewhere (writing, a popup): what
                // was typed would land in the other chat, or a popup act
                // on it.
                if self.busy() {
                    if found.is_ok() {
                        self.status = Some(format!("Found {request}: press s to open it"));
                    }
                    return;
                }
                match (found, link) {
                    (Ok((chat_id, message_id)), _) => {
                        self.open_chat(chat_id);
                        if let Some(id) = message_id {
                            self.jump_to_message(id);
                        }
                    }
                    (Err(Missed::Quiet), _) => {}
                    (Err(Missed::Said(why)), _) => self.status = Some(why),
                    // A link in a message goes to the browser, as before.
                    (Err(Missed::Elsewhere), Some(link)) => self.open_link_outside(link),
                    (Err(Missed::Elsewhere), None) => {
                        self.status = Some("tuigram can't open this kind of link".into());
                    }
                }
            }
            TgEvent::Forwarded { chat_id } => {
                let title = self.chats.title(chat_id).unwrap_or_default().to_string();
                self.show_toast("Forwarded", &format!("to {title}"));
            }
            TgEvent::Joined { chat_id } => {
                let title = self.chats.title(chat_id).unwrap_or_default().to_string();
                self.show_toast("Joined", &title);
            }
            TgEvent::Left { chat_id } => {
                let title = self.chats.title(chat_id).unwrap_or_default().to_string();
                if self.chats.is_secret(chat_id) {
                    self.show_toast("Ended the secret chat", &format!("with {title}"));
                } else {
                    self.show_toast("Left", &title);
                }
            }
            TgEvent::BotAnswer {
                chat_id,
                label,
                text,
                alert,
                url,
            } => self.on_bot_answer(chat_id, label, &text, alert, &url),
            TgEvent::Invite {
                request,
                link,
                invite,
            } => {
                if self.finding.as_ref().is_some_and(|f| f.request == request) && !self.busy() {
                    self.confirm_invite(link, request, invite);
                }
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
        open.go_to_unread();
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
            Update::ChatNotificationSettings(u) => self
                .chats
                .set_notifications(u.chat_id, u.notification_settings),
            Update::ScopeNotificationSettings(u) => self
                .chats
                .set_default_mute(&u.scope, u.notification_settings.mute_for),
            Update::ChatAction(u) => self.chats.set_action(u.chat_id, &u.sender_id, &u.action),
            Update::NotificationGroup(u) => self.on_notifications(u),
            Update::UnreadChatCount(u) => {
                let list = List::of(&u.chat_list);
                self.chats.set_unread_in(list, u.unread_unmuted_count);
                if list == List::Main {
                    self.set_unread_chats(u.unread_unmuted_count);
                }
            }
            Update::ChatFolders(u) => self
                .chats
                .set_folders(&u.chat_folders, u.main_chat_list_position),
            Update::ChatPhoto(u) => self.chats.set_photo(u.chat_id, u.photo.as_ref()),
            Update::ChatAccentColors(u) => self.chats.set_accent(u.chat_id, u.accent_color_id),
            Update::AccentColors(u) => self.chats.set_accent_colors(&u.colors),
            Update::ChatReadInbox(u) => {
                self.chats.set_unread(u.chat_id, u.unread_count);
                self.chats
                    .set_read_inbox(u.chat_id, u.last_read_inbox_message_id);
            }
            Update::ChatReadOutbox(u) => {
                self.chats
                    .set_read_outbox(u.chat_id, u.last_read_outbox_message_id);
                // Read, your messages' self-destruct timers start.
                if let Some(open) = self.open.as_mut().filter(|o| o.chat_id == u.chat_id) {
                    open.start_timers(true, u.last_read_outbox_message_id, SystemTime::now());
                }
            }
            Update::SecretChat(u) => {
                self.chats
                    .set_secret(u.secret_chat.id, Secret::of(&u.secret_chat));
                // A key compared must be the chat's key now.
                if let Some(view) = &self.key_view
                    && self
                        .chats
                        .secret(view.chat_id)
                        .is_none_or(|s| s.key_hash != view.hash || s.state == SecretState::Closed)
                {
                    self.key_view = None;
                }
            }
            Update::ChatMessageAutoDeleteTime(u) => self
                .chats
                .set_auto_delete(u.chat_id, u.message_auto_delete_time),
            Update::MessageContentOpened(u) => {
                if let Some(open) = self.open.as_mut().filter(|o| o.chat_id == u.chat_id) {
                    open.start_timer(u.message_id, SystemTime::now());
                }
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
                let badge = Badge::of(u.user.verification_status.as_ref(), u.user.is_support);
                self.chats.set_badge(Peer::User(u.user.id), badge);
                self.chats
                    .set_username(Peer::User(u.user.id), u.user.usernames.as_ref());
                self.chats
                    .set_presence(u.user.id, Presence::of(&u.user.status));
                let bot = matches!(u.user.r#type, UserType::Bot(_));
                self.chats.set_bot(u.user.id, bot);
            }
            Update::UserStatus(u) => {
                self.chats.set_presence(u.user_id, Presence::of(&u.status));
            }
            Update::Supergroup(u) => {
                let group = &u.supergroup;
                let badge = Badge::of(group.verification_status.as_ref(), false);
                self.chats.set_badge(Peer::Supergroup(group.id), badge);
                self.chats
                    .set_username(Peer::Supergroup(group.id), group.usernames.as_ref());
                let member = !matches!(group.status, ChatMemberStatus::Left);
                self.chats.set_member(group.id, member);
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
                    open.set_keyboard(u.message_id, u.reply_markup.as_ref());
                    // Its buttons may be other ones now.
                    if self
                        .button_menu
                        .as_ref()
                        .is_some_and(|m| m.message_id == u.message_id)
                    {
                        self.button_menu = None;
                    }
                }
            }
            Update::MessageIsPinned(u) => {
                if let Some(open) = self.open.as_mut().filter(|o| o.chat_id == u.chat_id) {
                    open.set_pinned(u.message_id, u.is_pinned);
                    self.ask_pinned();
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
                    if self
                        .button_menu
                        .as_ref()
                        .is_some_and(|m| u.message_ids.contains(&m.message_id))
                    {
                        self.button_menu = None;
                    }
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
                self.wanted_lists.insert(List::Main);
                self.load_more_chats(List::Main);
                // Even when they're off: they can be turned on any time.
                self.tg.enable_notifications();
                // Unless asked to, this computer doesn't take the secret
                // chats others start, which then go to your phone.
                self.tg
                    .accept_secret_chats(self.settings.accept_secret_chats);
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
        // Not while typing: in Insert mode, the prompt, or the picker's search.
        if ctrl
            && key.code == KeyCode::Char('c')
            && self.focus != Focus::Input
            && self.prompt.is_none()
            && self.picker.is_none()
        {
            self.quit();
            return;
        }
        self.status = None;
        match self.screen {
            Screen::Login(_) => self.on_login_key(key),
            Screen::Main if self.confirm.is_some() => self.on_confirm_key(key),
            Screen::Main if self.notice.is_some() => {
                // Not any key: one typed for something else as it came up
                // would close it unread.
                if matches!(key.code, KeyCode::Enter | KeyCode::Esc | KeyCode::Char('q')) {
                    self.notice = None;
                }
            }
            Screen::Main if self.key_view.is_some() => {
                if matches!(key.code, KeyCode::Enter | KeyCode::Esc | KeyCode::Char('q')) {
                    self.key_view = None;
                }
            }
            Screen::Main if self.settings_menu.is_some() => self.on_settings_key(key, ctrl),
            Screen::Main if self.delete_menu.is_some() => self.on_delete_key(key),
            Screen::Main if self.react_menu.is_some() => self.on_react_key(key, ctrl),
            Screen::Main if self.vote_menu.is_some() => self.on_vote_key(key),
            Screen::Main if self.button_menu.is_some() => self.on_button_key(key),
            Screen::Main if self.pin_menu.is_some() => self.on_pin_key(key),
            Screen::Main if self.pinned_menu.is_some() => self.on_pinned_key(key),
            Screen::Main if self.timer_menu.is_some() => self.on_timer_key(key),
            Screen::Main if self.menu.is_some() => self.on_menu_key(key),
            Screen::Main if self.picker.is_some() => self.on_picker_key(key, ctrl),
            Screen::Main if self.resizing.is_some() => self.on_resize_key(key, ctrl),
            Screen::Main if self.prompt.is_some() => self.on_prompt_key(key, ctrl),
            Screen::Main if self.focus == Focus::Input && self.stickers.is_some() => {
                self.on_sticker_key(key, ctrl)
            }
            Screen::Main if self.focus == Focus::Input => self.on_insert_key(key, ctrl),
            Screen::Main => self.on_normal_key(key, ctrl),
        }
        // The sticker panel and suggestions are part of Insert mode, and
        // close with it.
        if self.focus != Focus::Input {
            self.stickers = None;
            self.completion = None;
        }
        // Writing or a popup means you moved on: a chat still being looked
        // up won't open over it.
        if self.busy() {
            self.finding = None;
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
            (_, KeyCode::Char('o')) if ctrl => self.jump(true),
            // Before `i`, which writes. Terminals without the kitty keyboard
            // protocol send Ctrl-i as Tab, which goes forward in the chat too.
            (_, KeyCode::Char('i')) if ctrl => self.jump(false),
            (Focus::Messages, KeyCode::Tab) => self.jump(false),
            (Focus::Chats, KeyCode::Tab) => self.switch_list(1),
            (Focus::Chats, KeyCode::BackTab) => self.switch_list(-1),
            (_, KeyCode::Char('s')) => self.picker = Some(ChatPicker::new(Purpose::Open)),
            (Focus::Chats, KeyCode::Char('/')) => self.open_prompt(PromptKind::Chats),
            (Focus::Messages, KeyCode::Char('/')) => self.open_prompt(PromptKind::Messages),
            (Focus::Messages, KeyCode::Char('n')) => self.next_match(1),
            (Focus::Messages, KeyCode::Char('N')) => self.next_match(-1),
            // Esc stops a lookup with `s`. In the chat pane it then ends a
            // search, then an edit, then removes the files, then ends a
            // reply, before it leaves the pane.
            (_, KeyCode::Esc) if self.finding.is_some() => self.finding = None,
            (Focus::Chats, KeyCode::Esc) => self.chats.set_filter(""),
            (Focus::Chats, KeyCode::Char('p')) if !ctrl => self.toggle_pin(),
            (Focus::Chats, KeyCode::Char('m')) if !ctrl => self.toggle_mute(),
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
            (Focus::Messages, KeyCode::Char('p')) if pending_g => self.open_pinned_menu(),
            (Focus::Messages, KeyCode::Char('p')) => self.paste_clipboard(),
            (Focus::Messages, KeyCode::Char('P')) => self.toggle_pin_message(),
            (Focus::Messages, KeyCode::Char('t')) if ctrl => self.toggle_as_files(),
            (Focus::Messages, KeyCode::Char('d')) if pending_g => self.go_to_replied(),
            (Focus::Messages, KeyCode::Char('d')) => self.open_delete_menu(),
            (Focus::Messages, KeyCode::Char('R')) => self.open_react_menu(),
            (Focus::Messages, KeyCode::Char('X')) => self.remove_reactions(),
            (Focus::Messages, KeyCode::Char('f')) => self.forward_selected(),
            (Focus::Chats, KeyCode::Enter) => self.open_selected_chat(),
            (Focus::Chats, KeyCode::Char(c)) if c == to_chat => self.open_selected_chat(),
            (Focus::Chats, KeyCode::Char('i')) => {
                self.open_selected_chat();
                self.start_writing();
            }
            (Focus::Messages, KeyCode::Char('i')) => self.start_writing(),
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
        // While there are suggestions, Tab takes one and the arrows move
        // through them. Enter still sends: a suggestion taken by accident
        // would change the message.
        if let Some(completion) = self.completion.as_mut().filter(|c| !c.items.is_empty()) {
            match key.code {
                KeyCode::Tab => {
                    self.accept_completion();
                    return;
                }
                KeyCode::Up | KeyCode::BackTab => {
                    completion.move_by(-1);
                    return;
                }
                KeyCode::Down => {
                    completion.move_by(1);
                    return;
                }
                KeyCode::Char('p') if ctrl => {
                    completion.move_by(-1);
                    return;
                }
                KeyCode::Char('n') if ctrl => {
                    completion.move_by(1);
                    return;
                }
                _ => {}
            }
        }
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
            // Ctrl-u deletes back to the start of the line, as in a shell,
            // instead of the text area's undo.
            KeyCode::Char('u') if ctrl => {
                if self.composer.delete_line_by_head() {
                    self.on_composer_edit();
                }
            }
            KeyCode::Tab => self.open_stickers(),
            _ => {
                if self.composer.input(key) {
                    self.on_composer_edit();
                }
            }
        }
        self.update_completion();
    }

    /// Looks at the word before the cursor after each key in Insert mode,
    /// and suggests ways to finish it: emoji for `:smi`, people in the
    /// group for `@al`.
    fn update_completion(&mut self) {
        let DataCursor(row, col) = self.composer.cursor();
        let word = self
            .composer
            .lines()
            .get(row)
            .and_then(|line| complete::word_at(line, col));
        // Nobody to mention in a chat with one person, and a command only
        // starts a message.
        let group = self
            .open
            .as_ref()
            .and_then(|o| self.chats.get(o.chat_id))
            .is_some_and(|c| !c.is_private);
        let Some(word) = word.filter(|w| match w.kind {
            Kind::Emoji => true,
            Kind::Mention => group,
            Kind::Command => row == 0 && col == w.chars,
        }) else {
            self.completion = None;
            return;
        };
        if word.kind == Kind::Command {
            self.ask_bot_commands();
        }
        if self.completion.as_ref().is_some_and(|c| c.word == word) {
            return;
        }
        let completion = self
            .completion
            .get_or_insert_with(|| Completion::new(word.clone()));
        completion.retype(word.clone(), Instant::now());
        let items = match word.kind {
            Kind::Emoji => complete::emoji(&word.query),
            Kind::Mention => {
                let found = completion.found.clone();
                self.mention_items(&word.query, &found)
            }
            Kind::Command => self.command_items(&word.query),
        };
        if let Some(completion) = self.completion.as_mut() {
            completion.set_items(items);
        }
    }

    /// Asks once per open chat for the commands of its bots: the bot of a
    /// chat with one, or the bots in a group.
    fn ask_bot_commands(&mut self) {
        let Some(open) = self
            .open
            .as_mut()
            .filter(|o| o.commands == Commands::NotAsked)
        else {
            return;
        };
        let peer = self.chats.get(open.chat_id).and_then(|c| c.peer);
        open.commands = match peer {
            // A person takes no commands.
            Some(Peer::User(id)) if !self.chats.is_bot(id) => Commands::Known(Vec::new()),
            Some(peer) => {
                self.tg.bot_commands(open.chat_id, peer);
                Commands::Asked
            }
            None => Commands::Known(Vec::new()),
        };
    }

    /// Whom Tab offers after `from:` in a search: you, then the @usernames
    /// of the people whose messages are loaded, newest first.
    fn search_people(&self) -> Vec<String> {
        let mut people = vec!["me".to_string()];
        let Some(open) = &self.open else {
            return people;
        };
        for msg in open.messages.values().rev() {
            if let Sender::User(id) = msg.sender
                && !self.chats.is_saved(id)
                && let Some(name) = self.chats.user_username(id)
            {
                let name = format!("@{name}");
                if !people.contains(&name) {
                    people.push(name);
                }
            }
        }
        people
    }

    /// Turns `from:me`, and `from:` a name or a username tuigram knows, into
    /// who sent it; TDLib looks up other usernames.
    fn resolve_sender(&self, mut ask: search::Query) -> Result<search::Query, String> {
        let sender = match &ask.from {
            Some(Who::Me) => {
                let me = self.chats.my_id().ok_or("Your account isn't known yet")?;
                Sender::User(me)
            }
            Some(Who::Username(name)) => match self.chats.user_by_username(name) {
                Some(id) => Sender::User(id),
                None => return Ok(ask),
            },
            Some(Who::Name(name)) => {
                // Names are anyone's to pick, so one that only some of
                // several match doesn't get to stand for them: a name
                // matching all of it wins, else there must be just one.
                let mut matching: Vec<(i64, &str)> = Vec::new();
                for msg in self.open.iter().flat_map(|o| o.messages.values()) {
                    let Sender::User(id) = msg.sender else {
                        continue;
                    };
                    if let Some(n) = self.users.get(&id)
                        && !search::find(n, name).is_empty()
                        && !matching.iter().any(|&(seen, _)| seen == id)
                    {
                        matching.push((id, n));
                    }
                }
                let exact: Vec<i64> = matching
                    .iter()
                    .filter(|(_, n)| n.to_lowercase() == name.to_lowercase())
                    .map(|&(id, _)| id)
                    .collect();
                match (exact.as_slice(), matching.as_slice()) {
                    ([id], _) | ([], [(id, _)]) => Sender::User(*id),
                    ([], []) => {
                        return Err(format!(
                            "Nobody called {name} wrote in the messages loaded: try from:@username"
                        ));
                    }
                    _ => {
                        let names: Vec<&str> = matching.iter().map(|&(_, n)| n).take(3).collect();
                        return Err(format!(
                            "{name} could be {}: use from:@username",
                            names.join(", ")
                        ));
                    }
                }
            }
            Some(Who::Sender(_)) | None => return Ok(ask),
        };
        ask.from = Some(Who::Sender(sender));
        Ok(ask)
    }

    /// The commands of the open chat's bots that have `query` in them.
    fn command_items(&self, query: &str) -> Vec<Suggestion> {
        let Some(Commands::Known(list)) = self.open.as_ref().map(|o| &o.commands) else {
            return Vec::new();
        };
        complete::commands(list, query, |bot| {
            self.chats.user_username(bot).map(String::from)
        })
    }

    /// People in the open group whose name or username has `query` in it:
    /// the senders of the messages loaded, newest first, then members
    /// Telegram found. Not you.
    fn mention_items(&self, query: &str, found: &[i64]) -> Vec<Suggestion> {
        let Some(open) = &self.open else {
            return Vec::new();
        };
        let senders = open.messages.values().rev().filter_map(|m| match m.sender {
            Sender::User(id) => Some(id),
            Sender::Chat(_) => None,
        });
        let mut seen = HashSet::new();
        let mut items = Vec::new();
        for user_id in senders.chain(found.iter().copied()) {
            if self.chats.is_saved(user_id) || !seen.insert(user_id) {
                continue;
            }
            let Some(name) = self.users.get(&user_id) else {
                continue;
            };
            let username = self.chats.user_username(user_id);
            let matches = query.is_empty()
                || !search::find(name, query).is_empty()
                || username.is_some_and(|u| !search::find(u, query).is_empty());
            if matches {
                items.push(complete::mention(user_id, name, username));
            }
            if items.len() == complete::MAX_SUGGESTIONS {
                break;
            }
        }
        items
    }

    /// Tab with suggestions up: puts the one under the cursor in place of
    /// the word being typed.
    fn accept_completion(&mut self) {
        let Some(completion) = self.completion.take() else {
            return;
        };
        let Some(suggestion) = completion.current() else {
            return;
        };
        for _ in 0..completion.word.chars {
            self.composer.delete_char();
        }
        self.composer.insert_str(&suggestion.insert);
        self.on_composer_edit();
    }

    /// `i`: Insert mode, in a chat you can write in. In a public group or
    /// channel you're not in, it asks to join first.
    fn start_writing(&mut self) {
        let Some(open) = &self.open else {
            return;
        };
        if let Some(why) = self.cant_send(open.chat_id) {
            self.status = Some(why);
            return;
        }
        if self.chats.joined(open.chat_id) {
            self.focus = Focus::Input;
            return;
        }
        let title = self.chats.title(open.chat_id).unwrap_or("this chat");
        // Only a channel's admins write in it; joining one is following it.
        let channel = self.chats.get(open.chat_id).is_some_and(|c| c.is_channel);
        let (ask, why) = if channel {
            (
                "Join this channel?",
                "Join it to have it in your chat list.",
            )
        } else {
            (
                "Join to write here?",
                "Join it to write, and to have it in your chat list.",
            )
        };
        let mut confirm = Confirm::new(
            ask,
            vec![format!("You're not in {title}."), why.into()],
            Confirmed::Join(open.chat_id),
        );
        confirm.badge = self.chats.badge(open.chat_id);
        self.confirm = Some(confirm);
    }

    /// Why nothing can be sent in a secret chat now: it waits for the
    /// other person, or it ended.
    fn cant_send(&self, chat_id: i64) -> Option<String> {
        let secret = self.chats.secret(chat_id)?;
        let name = self
            .users
            .get(&secret.user_id)
            .map_or("them", String::as_str);
        secret.cant_send(name)
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
        let Some(chat_id) = self.open.as_ref().map(|o| o.chat_id) else {
            return;
        };
        let cant_send = self.cant_send(chat_id);
        let secret = self.chats.is_secret(chat_id);
        let Some(open) = self.open.as_mut() else {
            return;
        };
        let text = self.composer.lines().join("\n");
        let text = text.trim().to_string();
        if text.is_empty() && open.attachments.is_empty() {
            return;
        }
        if let Some(why) = cant_send {
            self.status = Some(why);
            return;
        }
        if let Some(changed) = open.attachments.iter().find(|a| a.swapped()) {
            self.status = Some(format!(
                "{} changed since it was attached. Drop the files (Esc in Normal mode) and attach it again",
                changed.name
            ));
            return;
        }
        let reply_to = open.reply.take().map(|r| r.id);
        if open.attachments.is_empty() {
            self.tg.send_text(open.chat_id, text, reply_to, secret);
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
            tabbed: None,
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
            KeyCode::Tab if prompt.kind == PromptKind::Command => prompt.complete_command(1),
            KeyCode::BackTab if prompt.kind == PromptKind::Command => prompt.complete_command(-1),
            KeyCode::Tab | KeyCode::BackTab if prompt.kind == PromptKind::Messages => {
                let step = if key.code == KeyCode::Tab { 1 } else { -1 };
                let typed = prompt.typed();
                let today = chrono::Local::now().format("%Y-%m-%d").to_string();
                let options = search::complete(&typed, &self.search_people(), &today);
                if let Some(prompt) = self.prompt.as_mut() {
                    prompt.tab(step, typed, options);
                }
            }
            _ => {
                prompt.input.input(key);
                prompt.completions.clear();
                prompt.tabbed = None;
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
                if self.open.is_none() || !submit || query.is_empty() {
                    return;
                }
                let secret = self
                    .open
                    .as_ref()
                    .is_some_and(|o| self.chats.is_secret(o.chat_id));
                let ask = search::parse(&query)
                    .and_then(|ask| match ask.from {
                        // TDLib can't tell in a secret chat.
                        Some(_) if secret => Err("from: doesn't work in secret chats".into()),
                        _ => Ok(ask),
                    })
                    .and_then(|ask| self.resolve_sender(ask));
                match ask {
                    Ok(ask) => {
                        if let Some(open) = self.open.as_mut() {
                            open.search = Some(MessageSearch::new(query, ask));
                        }
                        self.go_to_match(0);
                    }
                    Err(why) => self.status = Some(why),
                }
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
                Some(Command::Key) => self.show_key(),
                Some(Command::Leave) => self.ask_to_leave(),
                Some(Command::Logout) => self.ask_to_log_out(),
                Some(Command::Secret) => self.ask_secret_chat(),
                Some(Command::Timer) => self.open_timer_menu(),
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
            // Read, their self-destruct timers start.
            open.start_timers(false, id, SystemTime::now());
        }
    }

    /// How long since the last key, by whichever clock says longer: the
    /// monotonic one doesn't count time the computer spent asleep.
    fn idle(&self) -> Duration {
        let wall = SystemTime::now()
            .duration_since(self.last_input_wall)
            .unwrap_or_default();
        self.last_input.elapsed().max(wall)
    }

    /// The chat is open in front of the user, on its newest message, so new
    /// ones are seen as they arrive. Where the terminal never says when its
    /// window loses focus (tmux without `focus-events`, a detached session),
    /// no key press for [`IDLE_AFTER`] counts as the user being away, so
    /// messages aren't marked read, and do notify, while nobody is there.
    /// A window the terminal says has focus gets [`AWAY_AFTER`].
    fn watching(&self, chat_id: i64) -> bool {
        matches!(self.screen, Screen::Main)
            && self.present()
            && matches!(self.focus, Focus::Messages | Focus::Input)
            && self.settings_menu.is_none()
            && self.open.as_ref().is_some_and(|o| {
                o.chat_id == chat_id
                    && o.at_newest
                    && o.selected.is_none()
                    // Going to an older message: what arrives meanwhile
                    // isn't what's about to be on screen.
                    && !matches!(o.loading, Some(Page::Around(_)))
            })
    }

    /// What's shown only while open is covered once you look away, or go
    /// away.
    fn cover_unseen(&mut self) {
        let looking = self.focus == Focus::Messages && self.present();
        if let Some(open) = self.open.as_mut() {
            open.cover_unless_viewed(looking);
        }
    }

    /// Someone is at tuigram: its window has focus, and a key was pressed
    /// recently enough (see [`App::watching`]).
    fn present(&self) -> bool {
        self.terminal_focused && self.idle() < self.away_after()
    }

    /// How long without a key counts as away: [`IDLE_AFTER`] where the
    /// terminal never says when its window loses focus, [`AWAY_AFTER`]
    /// where it does.
    fn away_after(&self) -> Duration {
        if self.focus_reported {
            AWAY_AFTER
        } else {
            IDLE_AFTER
        }
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
            && self.idle() < IDLE_AFTER;
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
        // A secret chat someone started was taken here, though the setting
        // says not to: before it reached Telegram, or because it couldn't.
        let taken = update
            .added_notifications
            .iter()
            .any(|n| matches!(n.r#type, NotificationType::NewSecretChat));
        if taken && !self.settings.accept_secret_chats {
            let who = self.chats.title(update.chat_id).unwrap_or("Someone");
            self.status = Some(format!(
                "{who} started a secret chat, which this computer took anyway: :leave ends it"
            ));
        }
        if self.notify_with == Notifications::Off {
            return;
        }
        let chat_id = update.chat_id;
        for notification in update.added_notifications {
            if notification.date < self.notify_since || self.sees(chat_id) {
                continue;
            }
            let new = match notification.r#type {
                NotificationType::NewMessage(new) => new,
                // Someone started one, and this computer took it.
                NotificationType::NewSecretChat => {
                    let note = Note {
                        id: notification.id,
                        chat_id,
                        chat: "Secret chat".into(),
                        text: "Someone started a secret chat with you".into(),
                        silent: notification.is_silent,
                    };
                    self.notifier.add(note, Instant::now());
                    continue;
                }
                _ => continue,
            };
            // Notifications stay in the system's list, so a secret chat's
            // say neither who nor what, as in Telegram's apps.
            if self.chats.is_secret(chat_id) {
                let note = Note {
                    id: notification.id,
                    chat_id,
                    chat: "Secret chat".into(),
                    text: "New message".into(),
                    silent: notification.is_silent,
                };
                self.notifier.add(note, Instant::now());
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
                chat: format!(
                    "{}{}",
                    self.chats.title(chat_id).unwrap_or("Telegram"),
                    self.chats.badge(chat_id).map_or("", Badge::mark)
                ),
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

    /// `:leave`: asks before leaving the chat the command is about, the
    /// selected one in the list or else the open one. Only groups and
    /// channels can be left.
    fn ask_to_leave(&mut self) {
        let chat_id = match self.focus {
            Focus::Chats => self.selected,
            _ => self.open.as_ref().map(|o| o.chat_id),
        };
        let Some((chat_id, chat)) = chat_id.and_then(|id| Some((id, self.chats.get(id)?))) else {
            self.status = Some("Open the group or channel to leave first".into());
            return;
        };
        if let Some(secret_id) = chat.secret_id {
            let title = self.chats.title(chat_id).unwrap_or_default();
            self.confirm = Some(Confirm::new(
                "End this secret chat?",
                vec![
                    format!("With {title}."),
                    "Nothing more can be sent in it, by either of you,".into(),
                    "and its messages are deleted from this computer.".into(),
                ],
                Confirmed::EndSecret { chat_id, secret_id },
            ));
            return;
        }
        if chat.is_private {
            self.status = Some("A chat with one person can't be left".into());
            return;
        }
        if !self.chats.joined(chat_id) {
            self.status = Some("You're not in this chat".into());
            return;
        }
        let title = self.chats.title(chat_id).unwrap_or_default();
        let (ask, why) = if chat.is_channel {
            ("Leave this channel?", "Its posts stop coming to you.")
        } else {
            ("Leave this group?", "Its messages stop coming to you.")
        };
        let mut confirm = Confirm::new(
            ask,
            vec![title.to_string(), why.into()],
            Confirmed::Leave(chat_id),
        );
        confirm.badge = self.chats.badge(chat_id);
        self.confirm = Some(confirm);
    }

    /// The chat a command is about: the selected one in the list, or else
    /// the open one.
    fn command_chat(&self) -> Option<i64> {
        match self.focus {
            Focus::Chats => self.selected,
            _ => self.open.as_ref().map(|o| o.chat_id),
        }
    }

    /// `:secret`: asks before starting a secret chat with the person the
    /// chat the command is about is with.
    fn ask_secret_chat(&mut self) {
        let Some(user_id) = self.command_chat().and_then(|id| self.chats.person(id)) else {
            self.status = Some("Open a chat with someone to start a secret chat with them".into());
            return;
        };
        if self.chats.is_bot(user_id) {
            self.status = Some("Bots can't be in secret chats".into());
            return;
        }
        let with = self
            .users
            .get(&user_id)
            .cloned()
            .unwrap_or_else(|| "them".into());
        self.confirm = Some(Confirm::new(
            "Start a secret chat?",
            vec![
                format!("With {with}. It's end-to-end encrypted, and kept only"),
                "on this computer and on the device of theirs that".into(),
                "accepts it: your other devices won't have it.".into(),
            ],
            Confirmed::StartSecret { user_id, with },
        ));
        let badge = self.command_chat().and_then(|id| self.chats.badge(id));
        if let Some(confirm) = self.confirm.as_mut() {
            confirm.badge = badge;
        }
    }

    /// The open secret chat, with its state, for `:key` and `:timer`, whose
    /// popups go over it; or why there's none, said in the status bar.
    fn open_secret_chat(&mut self) -> Option<(i64, Secret)> {
        let found = self
            .open
            .as_ref()
            .map(|o| o.chat_id)
            .filter(|&id| self.chats.is_secret(id))
            .and_then(|id| Some((id, self.chats.secret(id)?.clone())));
        if found.is_none() {
            self.status = Some("Open a secret chat first (:secret starts one)".into());
        }
        found
    }

    /// `:timer`: how long new messages in the secret chat last once seen.
    fn open_timer_menu(&mut self) {
        let Some((chat_id, _)) = self.open_secret_chat() else {
            return;
        };
        if let Some(why) = self.cant_send(chat_id) {
            self.status = Some(why);
            return;
        }
        self.timer_menu = Some(TimerMenu::new(chat_id, self.chats.auto_delete(chat_id)));
    }

    /// The timer popup takes all keys while it's up.
    fn on_timer_key(&mut self, key: KeyEvent) {
        let Some(menu) = self.timer_menu.as_mut() else {
            return;
        };
        let last = menu.choices.len() - 1;
        match key.code {
            KeyCode::Char('j') | KeyCode::Down => menu.selected = (menu.selected + 1).min(last),
            KeyCode::Char('k') | KeyCode::Up => menu.selected = menu.selected.saturating_sub(1),
            KeyCode::Char('g') => menu.selected = 0,
            KeyCode::Char('G') => menu.selected = last,
            KeyCode::Enter | KeyCode::Char('l') => {
                let (chat_id, seconds) = (menu.chat_id, menu.choices[menu.selected]);
                self.timer_menu = None;
                if let Some(why) = self.cant_send(chat_id) {
                    self.status = Some(why);
                    return;
                }
                // Only if it changes: setting it again would send the chat a
                // message saying so. Kept at once, so a second Enter before
                // Telegram answers doesn't send another.
                if seconds != self.chats.auto_delete(chat_id) {
                    self.tg.set_timer(chat_id, seconds);
                    self.chats.set_auto_delete(chat_id, seconds);
                }
            }
            KeyCode::Esc | KeyCode::Char('q' | 'h') => self.timer_menu = None,
            _ => {}
        }
    }

    /// `:key`: the secret chat's key, as a picture and in numbers, for both
    /// sides to compare.
    fn show_key(&mut self) {
        let Some((chat_id, secret)) = self.open_secret_chat() else {
            return;
        };
        let with = self.chats.title(chat_id).unwrap_or("them").to_string();
        if crate::secret::key_picture(&secret.key_hash).is_none() {
            self.status = Some(match secret.state {
                SecretState::Pending => {
                    format!("The key is made once {with} accepts the secret chat")
                }
                _ => "Telegram gave no key to compare for this chat".into(),
            });
            return;
        }
        self.key_view = Some(KeyView {
            chat_id,
            with,
            hash: secret.key_hash,
        });
    }

    /// `:logout` asks first: Tab can put it in, and logging in again takes a
    /// code, and maybe the password.
    fn ask_to_log_out(&mut self) {
        let mut lines: Vec<String> = vec![
            "This ends the session on Telegram and deletes what".into(),
            "tuigram keeps on this computer. Logging in again".into(),
            "takes a code, and your password if you have one.".into(),
        ];
        if self.chats.has_secret_chats() {
            lines.push("Your secret chats go too: they're kept only here.".into());
        }
        self.confirm = Some(Confirm::new("Log out?", lines, Confirmed::Logout));
    }

    /// `:logout`, once asked: ends the session on Telegram's side and
    /// deletes what TDLib keeps on this computer. The login screen comes
    /// back once TDLib closes.
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
        self.vote_menu = None;
        self.button_menu = None;
        self.notice = None;
        self.pin_menu = None;
        self.pinned_menu = None;
        self.timer_menu = None;
        self.key_view = None;
        self.picker = None;
        self.finding = None;
        self.completion = None;
        self.stickers = None;
        // The next account says if it has Premium; one without may not.
        self.premium = false;
        self.confirm = None;
        self.settings_menu = None;
        self.prompt = None;
        self.loading_lists.clear();
        self.loaded_lists.clear();
        self.wanted_lists.clear();
        // Chats of the old account.
        self.jumps = Jumps::default();
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
                let (query, ask) = (search.query.clone(), search.ask.clone());
                let (from, offset) = (search.next_from, search.next_offset.clone());
                let secret = self.chats.is_secret(open.chat_id);
                self.tg.search_messages(
                    open.chat_id,
                    secret,
                    query,
                    ask,
                    from,
                    offset,
                    SEARCH_PAGE,
                );
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
        open.unread_after = None;
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
        open.unread_after = None;
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
        if let Some(chat_id) = self.selected {
            self.open_chat(chat_id);
        }
    }

    /// Opens a chat, in the list or not: one found with `s` may be a
    /// public group you're not in. Ctrl-o comes back to the chat before.
    fn open_chat(&mut self, chat_id: i64) {
        if let Some(here) = self.here().filter(|h| h.chat_id != chat_id) {
            self.jumps.leave(here);
        }
        self.enter_chat(chat_id);
    }

    /// Where the cursor is: the open chat, and the message it's on.
    fn here(&self) -> Option<Jump> {
        self.open.as_ref().map(|o| Jump {
            chat_id: o.chat_id,
            message_id: o.selected,
        })
    }

    /// Ctrl-o (`back`) or Ctrl-i: to the chat or message left before, or
    /// forward again to where Ctrl-o came from.
    fn jump(&mut self, back: bool) {
        let here = self.here();
        match self.jumps.go(back, here) {
            Some(to) => {
                if self.open.as_ref().is_none_or(|o| o.chat_id != to.chat_id) {
                    self.enter_chat(to.chat_id);
                }
                self.focus = Focus::Messages;
                match to.message_id {
                    Some(id) => self.jump_to_message(id),
                    None => self.jump_to_newest(),
                }
            }
            None if back => self.status = Some("Nothing to go back to".into()),
            None => self.status = Some("Nothing to go forward to".into()),
        }
    }

    /// [`App::open_chat`], without Ctrl-o coming back to the chat before.
    fn enter_chat(&mut self, chat_id: i64) {
        // A lookup still on its way would open another chat over this one.
        self.finding = None;
        // Popups about a message of the chat before are no use in this one.
        self.menu = None;
        self.delete_menu = None;
        self.react_menu = None;
        self.vote_menu = None;
        self.button_menu = None;
        self.pin_menu = None;
        self.pinned_menu = None;
        self.timer_menu = None;
        self.key_view = None;
        if self.chats.in_list(chat_id, self.chats.shown()) && self.selected != Some(chat_id) {
            // The list's cursor goes to it, even if the filter hid it.
            if !self.chats.ids().contains(&chat_id) {
                self.chats.set_filter("");
            }
            self.selected = Some(chat_id);
        }
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
        let mut open = OpenChat::new(chat_id);
        // Reading a secret chat's messages starts their timers: it opens
        // where you stopped reading, not with all of them read at once.
        if self.chats.is_secret(chat_id) && self.chats.get(chat_id).is_some_and(|c| c.unread > 0) {
            open.unread_after = Some(self.chats.read_inbox(chat_id));
        }
        self.open = Some(open);
        self.composer = new_composer();
        self.load_older_messages();
        self.ask_pinned();
    }

    /// Asks for the open chat's pinned messages, again whenever one is
    /// pinned or unpinned.
    fn ask_pinned(&mut self) {
        // Nothing is pinned in a secret chat, and TDLib can't search one
        // that way.
        if let Some(open) = self
            .open
            .as_mut()
            .filter(|o| !self.chats.is_secret(o.chat_id))
        {
            open.pinned_asked += 1;
            self.tg.pinned_messages(open.chat_id, open.pinned_asked);
        }
    }

    fn on_pinned(&mut self, chat_id: i64, request: u32, messages: Option<Vec<Message>>) {
        // Only the last answer for the chat open: an older one may miss a
        // message pinned since.
        let Some(open) = self
            .open
            .as_mut()
            .filter(|o| o.chat_id == chat_id && o.pinned_asked == request)
        else {
            return;
        };
        // On an error, TDLib's message is already in the status bar.
        let Some(messages) = messages else {
            return;
        };
        open.pinned = messages
            .into_iter()
            .map(|m| {
                let id = m.id;
                Pinned::new(id, &m.into())
            })
            .collect();
        if open.pinned.is_empty() {
            self.pinned_menu = None;
        }
    }

    /// `P`: pins the message under the cursor, asking how, or unpins it.
    /// The popup opens at once and fills in when TDLib says it can be.
    fn toggle_pin_message(&mut self) {
        let Some(open) = &self.open else {
            return;
        };
        let Some((&id, msg)) = open
            .cursor_id()
            .and_then(|id| open.messages.get_key_value(&id))
        else {
            return;
        };
        match msg.state {
            SendState::Pending => self.status = Some("Wait until it's sent".into()),
            SendState::Failed => self.status = Some("This message wasn't sent".into()),
            SendState::Sent if msg.pinned => self.tg.unpin_message(open.chat_id, id),
            SendState::Sent => {
                let chat = self.chats.get(open.chat_id);
                let place = match chat {
                    _ if self.chats.is_saved(open.chat_id) => Place::Saved,
                    Some(c) if c.is_private => Place::Private,
                    Some(c) if c.is_channel => Place::Channel,
                    _ => Place::Group,
                };
                let with = self.chats.title(open.chat_id).unwrap_or("them").to_string();
                self.pin_menu = Some(PinMenu::new(id, msg.snippet(), place, with));
                self.tg.check_pinnable(open.chat_id, id);
            }
        }
    }

    fn on_pinnable(&mut self, chat_id: i64, message_id: i64, pinnable: Option<bool>) {
        // Drop answers for a popup that closed, or a chat that changed.
        if self.open.as_ref().is_none_or(|o| o.chat_id != chat_id) {
            return;
        }
        let Some(menu) = self
            .pin_menu
            .as_mut()
            .filter(|m| m.message_id == message_id)
        else {
            return;
        };
        match pinnable {
            // On an error, TDLib's message is already in the status bar.
            None => self.pin_menu = None,
            Some(false) => {
                self.pin_menu = None;
                self.status = Some("You can't pin messages here".into());
            }
            // A second answer (P pressed again) keeps the cursor.
            Some(true) if menu.choices.is_empty() => menu.allow(),
            Some(true) => {}
        }
    }

    /// The pin popup takes all keys while it's up.
    fn on_pin_key(&mut self, key: KeyEvent) {
        let Some(menu) = self.pin_menu.as_mut() else {
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
                self.pin_menu = None;
                return;
            }
            _ => None,
        };
        // Nothing to pick while TDLib hasn't answered.
        let Some(choice) = pick.and_then(|i| menu.choices.get(i).copied()) else {
            return;
        };
        let message_id = menu.message_id;
        self.pin_menu = None;
        if let Some(open) = &self.open {
            let (quietly, only_for_self) = choice.flags();
            self.tg
                .pin_message(open.chat_id, message_id, quietly, only_for_self);
        }
    }

    /// `gp`: the chat's pinned messages, newest first, to go to one.
    fn open_pinned_menu(&mut self) {
        let Some(open) = &self.open else {
            return;
        };
        self.pinned_menu = PinnedMenu::new(&open.pinned);
        if self.pinned_menu.is_none() {
            self.status = Some("No pinned messages in this chat".into());
        }
    }

    /// The pinned messages popup takes all keys while it's up: Enter goes to
    /// the one under the cursor, and `P` unpins it.
    fn on_pinned_key(&mut self, key: KeyEvent) {
        let (Some(open), Some(menu)) = (&self.open, self.pinned_menu.as_mut()) else {
            self.pinned_menu = None;
            return;
        };
        let current = menu.current(&open.pinned).map(|p| p.id);
        match key.code {
            KeyCode::Char('j') | KeyCode::Down => menu.move_by(&open.pinned, 1),
            KeyCode::Char('k') | KeyCode::Up => menu.move_by(&open.pinned, -1),
            KeyCode::Char('g') => menu.move_by(&open.pinned, isize::MIN),
            KeyCode::Char('G') => menu.move_by(&open.pinned, isize::MAX),
            KeyCode::Enter | KeyCode::Char('l') => {
                self.pinned_menu = None;
                if let Some(id) = current {
                    // Ctrl-o comes back.
                    if let Some(here) = self.here() {
                        self.jumps.leave(here);
                    }
                    self.jump_to_message(id);
                }
            }
            // The list follows once TDLib says it's unpinned.
            KeyCode::Char('P') => {
                if let Some(id) = current {
                    self.tg.unpin_message(open.chat_id, id);
                }
            }
            KeyCode::Esc | KeyCode::Char('q' | 'h') => self.pinned_menu = None,
            _ => {}
        }
    }

    /// Enter on a message: shows its spoilers first, as a tap does in
    /// Telegram; votes in a poll; lists a bot's buttons; else opens its file
    /// or link right away, or shows a menu when there's more than one.
    fn open_selected_message(&mut self) {
        let Some(open) = self.open.as_mut() else {
            return;
        };
        // A photo shown only while open. Once it's downloaded, and can be
        // seen, its timer starts and the sender is told it was opened; not
        // for your own, nor one still on its way.
        if let Some(id) = open.cursor_id()
            && open.uncover(id)
        {
            let photo = open
                .messages
                .get(&id)
                .filter(|m| !m.outgoing && m.state == SendState::Sent)
                .and_then(|m| m.preview.as_ref())
                .map(|p| p.file_id);
            if let Some(file_id) = photo {
                open.opening = Some((id, file_id));
                self.tg.download(file_id);
            }
            return;
        }
        if let Some(id) = open.cursor_id()
            && open.reveal_spoilers(id)
        {
            return;
        }
        if let Some((&id, msg)) = open
            .cursor_id()
            .and_then(|id| open.messages.get_key_value(&id))
            && let Some(poll) = &msg.poll
        {
            match (msg.state, poll.cant_vote()) {
                (SendState::Sent, None) => self.vote_menu = Some(VoteMenu::new(id, poll)),
                (SendState::Sent, Some(why)) => self.status = Some(why.into()),
                _ => self.status = Some("Wait until it's sent".into()),
            }
            return;
        }
        if let Some((&id, msg)) = open
            .cursor_id()
            .and_then(|id| open.messages.get_key_value(&id))
            && let Some(keyboard) = &msg.keyboard
        {
            match msg.state {
                SendState::Sent => {
                    self.button_menu = Some(ButtonMenu::new(
                        id,
                        msg.snippet(),
                        keyboard,
                        msg.file.as_ref(),
                        &msg.links,
                    ));
                }
                _ => self.status = Some("Wait until it's sent".into()),
            }
            return;
        }
        let Some(msg) = open.cursor_id().and_then(|id| open.messages.get(&id)) else {
            return;
        };
        // Media seen only while open (a voice message whose timer starts
        // once it's played) isn't handed to another app, which keeps it.
        let file = msg.file.clone().filter(|_| !opens_once(msg));
        let mut targets: Vec<Target> = file.map(Target::File).into_iter().collect();
        targets.extend(msg.links.iter().cloned().map(Target::Link));
        match targets.len() {
            0 if opens_once(msg) => {
                self.status = Some(format!(
                    "tuigram can't show this only while it's open: {}",
                    crate::messages::ON_PHONE
                ));
            }
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

    fn on_editable(
        &mut self,
        chat_id: i64,
        message_id: i64,
        editable: Option<bool>,
        text: Option<EditText>,
    ) {
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
        // Without it as Markdown, it's edited as plain text, which loses
        // any formatting.
        let (text, loses) = match text {
            Some(text) => (text.markdown, text.loses),
            None => (msg.source_text.clone(), msg.formatted),
        };
        // On an error, TDLib's message is already in the status bar.
        match editable {
            None => {}
            Some(false) => self.status = Some("You can't edit this message".into()),
            Some(true) if loses => {
                self.confirm = Some(Confirm::new(
                    "Edit and lose some formatting?",
                    vec![
                        "Some of its formatting can't be written in Markdown".into(),
                        "(underline, custom emoji…), so an edit would lose it.".into(),
                    ],
                    Confirmed::Edit {
                        id: message_id,
                        text,
                    },
                ));
            }
            Some(true) => self.start_edit(message_id, text),
        }
    }

    /// Puts the message's text, as Markdown, in the composer, keeping what
    /// was there to give back when the edit is done.
    fn start_edit(&mut self, id: i64, text: String) {
        self.end_edit();
        let Some(open) = self.open.as_mut() else {
            return;
        };
        let Some(msg) = open.messages.get(&id) else {
            return;
        };
        open.editing = Some(Editing {
            id,
            snippet: msg.snippet(),
            original: text.clone(),
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
        if text != editing.original.trim() {
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

    /// The vote popup takes all keys while it's up.
    fn on_vote_key(&mut self, key: KeyEvent) {
        let Some(menu) = self.vote_menu.as_mut() else {
            return;
        };
        match key.code {
            KeyCode::Char('j') | KeyCode::Down => menu.move_by(1),
            KeyCode::Char('k') | KeyCode::Up => menu.move_by(-1),
            KeyCode::Char(' ') => menu.tick(),
            KeyCode::Char(c @ '1'..='9') => {
                let index = c as usize - '1' as usize;
                if index < menu.answers.len() {
                    menu.selected = index;
                    if menu.several {
                        menu.tick();
                    } else {
                        self.cast_vote();
                    }
                }
            }
            KeyCode::Enter | KeyCode::Char('l') => self.cast_vote(),
            KeyCode::Esc | KeyCode::Char('q' | 'h') => self.vote_menu = None,
            _ => {}
        }
    }

    /// Enter in the vote popup: votes, or takes your vote back. TDLib then
    /// sends the poll's new counts.
    fn cast_vote(&mut self) {
        let Some(menu) = self.vote_menu.take() else {
            return;
        };
        let Some(open) = &self.open else {
            return;
        };
        let answers = match menu.vote() {
            Vote::For(answers) => answers,
            Vote::Retract => Vec::new(),
        };
        self.tg.vote(open.chat_id, menu.message_id, answers);
    }

    /// The button popup takes all keys while it's up: `h/j/k/l` move as the
    /// buttons are laid out, Tab goes through them in order.
    fn on_button_key(&mut self, key: KeyEvent) {
        let Some(menu) = self.button_menu.as_mut() else {
            return;
        };
        match key.code {
            KeyCode::Char('j') | KeyCode::Down => menu.move_rows(1),
            KeyCode::Char('k') | KeyCode::Up => menu.move_rows(-1),
            KeyCode::Char('h') | KeyCode::Left => menu.move_cols(-1),
            KeyCode::Char('l') | KeyCode::Right => menu.move_cols(1),
            KeyCode::Tab => menu.move_by(1),
            KeyCode::BackTab => menu.move_by(-1),
            KeyCode::Enter if menu.shown.elapsed() >= CONFIRM_GRACE => self.press_button(),
            KeyCode::Esc | KeyCode::Char('q') => self.button_menu = None,
            _ => {}
        }
    }

    /// Enter in the button popup: does what the button under the cursor
    /// does, and closes the popup. One tuigram can't press says why, and
    /// the popup stays.
    fn press_button(&mut self) {
        let Some(menu) = &self.button_menu else {
            return;
        };
        let Some(button) = menu.current().cloned() else {
            return;
        };
        if let Press::Unsupported(why) = button.press {
            self.status = Some(why.into());
            return;
        }
        let message_id = menu.message_id;
        self.button_menu = None;
        let Some(chat_id) = self.open.as_ref().map(|o| o.chat_id) else {
            return;
        };
        match button.press {
            Press::Callback(data) => {
                self.show_toast("Pressed", &button.label);
                self.tg
                    .press_button(chat_id, message_id, data, button.label);
            }
            Press::Open(link) => self.open_target(Target::Link(link)),
            Press::File(file) => self.open_target(Target::File(file)),
            Press::Telegram(url) => {
                self.finding = Some(Finding::new(&url));
                self.tg.find_link(url.clone(), url);
            }
            Press::User(user_id) => {
                let name = self
                    .users
                    .get(&user_id)
                    .cloned()
                    .unwrap_or_else(|| button.label.clone());
                self.finding = Some(Finding::new(&name));
                self.tg.find_private_chat(user_id, name);
            }
            Press::Copy(text) => self.copy_target(Target::Text(text)),
            Press::Send(text) => {
                // In a group it answers the bot's message, as Telegram's
                // apps do, so the bot knows whose buttons they were.
                let private = self.chats.get(chat_id).is_some_and(|c| c.is_private);
                let reply_to = (!private).then_some(message_id);
                self.tg.send_plain(chat_id, text, reply_to);
                self.jump_to_newest();
            }
            Press::Unsupported(_) => {}
        }
    }

    /// A bot answered a button: a note goes in the corner, an alert in a
    /// popup. A link it sends opens only while its chat is open and nothing
    /// else holds the keys, and asks first, since nothing said where it goes.
    fn on_bot_answer(&mut self, chat_id: i64, label: String, text: &str, alert: bool, url: &str) {
        let text = one_line(text);
        let here = self.open.as_ref().is_some_and(|o| o.chat_id == chat_id) && !self.busy();
        if !text.is_empty() {
            if alert && here {
                self.notice = Some(Notice {
                    title: label.clone(),
                    text,
                });
            } else {
                self.show_toast(&label, &text);
            }
        }
        if url.is_empty() || !here || self.notice.is_some() {
            return;
        }
        match web_url(url) {
            Some(url) => self.open_link_outside(Link {
                url,
                disguise: Some(label),
            }),
            None => self.status = Some("The bot's link isn't a web address".into()),
        }
    }

    /// `f`: forwards the message under the cursor, or its whole album, to a
    /// chat picked from your list.
    fn forward_selected(&mut self) {
        let Some(open) = &self.open else {
            return;
        };
        if self.chats.is_secret(open.chat_id) {
            self.status = Some("Messages in secret chats can't be forwarded".into());
            return;
        }
        let Some((&id, msg)) = open
            .cursor_id()
            .and_then(|id| open.messages.get_key_value(&id))
        else {
            return;
        };
        match msg.state {
            SendState::Pending => self.status = Some("Wait until it's sent".into()),
            SendState::Failed => self.status = Some("This message wasn't sent".into()),
            SendState::Sent => {
                self.picker = Some(ChatPicker::new(Purpose::Forward {
                    from: open.chat_id,
                    message_ids: open.forward_ids(id),
                    snippet: msg.snippet(),
                }));
            }
        }
    }

    /// The picker takes all keys while it's up: they type the search, and
    /// the arrows (or Ctrl-n / Ctrl-p, Tab) move.
    fn on_picker_key(&mut self, key: KeyEvent, ctrl: bool) {
        let choices = self
            .picker
            .as_ref()
            .map_or(Vec::new(), |p| p.choices(&self.chats));
        let Some(picker) = self.picker.as_mut() else {
            return;
        };
        let now = Instant::now();
        match key.code {
            KeyCode::Enter => self.pick_chat(),
            KeyCode::Esc => self.picker = None,
            KeyCode::Char('c') if ctrl => self.picker = None,
            KeyCode::Up | KeyCode::BackTab => picker.move_by(-1, &choices),
            KeyCode::Down | KeyCode::Tab => picker.move_by(1, &choices),
            KeyCode::Char('p') if ctrl => picker.move_by(-1, &choices),
            KeyCode::Char('n') if ctrl => picker.move_by(1, &choices),
            KeyCode::PageUp => picker.move_by(-HALF_PAGE, &choices),
            KeyCode::PageDown => picker.move_by(HALF_PAGE, &choices),
            KeyCode::Backspace => picker.edit_query(
                |q| {
                    q.pop();
                },
                now,
            ),
            KeyCode::Char('u' | 'w') if ctrl => picker.edit_query(String::clear, now),
            KeyCode::Char(c) if !ctrl => picker.edit_query(|q| q.push(c), now),
            _ => {}
        }
    }

    /// Enter in the picker: forwards there, or opens it, looking it up
    /// first if it's a username, a link or a contact with no chat yet.
    fn pick_chat(&mut self) {
        let Some(picker) = self.picker.as_ref() else {
            return;
        };
        let Some(choice) = picker.current(&picker.choices(&self.chats)) else {
            return;
        };
        let Some(picker) = self.picker.take() else {
            return;
        };
        match (picker.purpose, choice) {
            (
                Purpose::Forward {
                    from, message_ids, ..
                },
                Choice::Chat(to),
            ) => match self.cant_send(to) {
                Some(why) => self.status = Some(why),
                None => self.tg.forward(to, from, message_ids),
            },
            (Purpose::Forward { .. }, _) => {}
            (Purpose::Open, Choice::Chat(id)) => self.open_chat(id),
            (Purpose::Open, Choice::User(user_id)) => {
                let name = self
                    .users
                    .get(&user_id)
                    .cloned()
                    .unwrap_or_else(|| "your contact".into());
                self.finding = Some(Finding::new(&name));
                self.tg.find_private_chat(user_id, name);
            }
            (Purpose::Open, Choice::Username(name)) => {
                let request = format!("@{name}");
                self.finding = Some(Finding::new(&request));
                self.tg.find_username(name, request);
            }
            (Purpose::Open, Choice::Link(link)) => {
                self.finding = Some(Finding::new(&link));
                self.tg.find_link(link.clone(), link);
            }
        }
    }

    /// Asks before joining the chat an invite link leads to, saying what it
    /// is and what Telegram thinks of it.
    fn confirm_invite(&mut self, link: String, request: String, invite: Invite) {
        let kind = if invite.channel { "channel" } else { "group" };
        let mut lines = vec![
            invite.title,
            format!("A {kind} with {} members.", invite.members),
        ];
        if invite.by_request {
            lines.push("An admin has to let you in.".into());
        }
        let title = if invite.by_request {
            format!("Ask to join this {kind}?")
        } else {
            format!("Join this {kind}?")
        };
        let mut confirm = Confirm::new(title, lines, Confirmed::JoinLink { link, request });
        confirm.badge = invite.badge;
        self.confirm = Some(confirm);
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
        if let Some(why) = self.cant_send(panel.chat_id) {
            self.status = Some(why);
            return;
        }
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
        let Some(open) = self.open.as_ref() else {
            return;
        };
        match open.replied_jump() {
            Ok((from, to)) => {
                self.jumps.leave(Jump {
                    chat_id: open.chat_id,
                    message_id: Some(from),
                });
                self.jump_to_message(to);
            }
            Err(why) => self.status = Some(why.into()),
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
            // A chat, a message or an invite on Telegram opens here. Where
            // the link really goes decides, not its words, so it can't be
            // disguised; anything tuigram can't open goes to the browser.
            Target::Link(link) if picker::telegram_link(&link.url) => {
                self.finding = Some(Finding {
                    request: link.url.clone(),
                    link: Some(link.clone()),
                });
                self.tg.find_link(link.url.clone(), link.url);
            }
            Target::Link(link) => self.open_link_outside(link),
            Target::Text(_) => {}
        }
    }

    /// Opens a link in the browser, asking first if its words say something
    /// other than where it goes.
    fn open_link_outside(&mut self, link: Link) {
        match link {
            Link {
                url,
                disguise: Some(shown),
            } => {
                // Browsers show other scripts' letters as such, so a look-alike
                // host can pass for a familiar one.
                let site = link_host(&url).map(|host| match host.is_ascii() {
                    true => host,
                    false => format!("{host} (has non-Latin letters)"),
                });
                let mut confirm = Confirm::new(
                    "Open this link?",
                    vec![
                        format!("The text says: {shown}"),
                        format!("Full address:  {url}"),
                    ],
                    Confirmed::OpenLink(url),
                );
                confirm.site = site;
                self.confirm = Some(confirm);
            }
            link => self.open_externally(&link.url),
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
        // The file name is the sender's.
        let name = text::clean(
            &Path::new(&path)
                .file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_default(),
        );
        // A slow download can finish while you're busy with something else:
        // a warning popping up then would take keys meant for that.
        if self.busy() {
            self.status = Some(format!(
                "{name} downloaded: press Enter on it again to open it"
            ));
            return;
        }
        self.confirm = Some(Confirm::new(
            format!("Open {name}?"),
            vec![
                "Files like this can run programs on your computer.".into(),
                "Only open it if you trust whoever sent it.".into(),
            ],
            Confirmed::OpenFile(path),
        ));
    }

    /// A popup, a prompt or the composer is taking keys.
    fn busy(&self) -> bool {
        self.confirm.is_some()
            || self.settings_menu.is_some()
            || self.delete_menu.is_some()
            || self.react_menu.is_some()
            || self.vote_menu.is_some()
            || self.button_menu.is_some()
            || self.notice.is_some()
            || self.pin_menu.is_some()
            || self.pinned_menu.is_some()
            || self.timer_menu.is_some()
            || self.key_view.is_some()
            || self.menu.is_some()
            || self.picker.is_some()
            || self.resizing.is_some()
            || self.prompt.is_some()
            || self.focus == Focus::Input
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
            KeyCode::Char('y')
                if self
                    .confirm
                    .as_ref()
                    .is_some_and(|c| c.shown.elapsed() >= CONFIRM_GRACE) =>
            {
                if let Some(confirm) = self.confirm.take() {
                    match confirm.action {
                        Confirmed::OpenFile(target) | Confirmed::OpenLink(target) => {
                            self.open_externally(&target)
                        }
                        Confirmed::Edit { id, text } => self.start_edit(id, text),
                        Confirmed::Join(chat_id) => self.tg.join_chat(chat_id),
                        Confirmed::Leave(chat_id) => self.tg.leave_chat(chat_id),
                        Confirmed::Logout => self.log_out(),
                        Confirmed::StartSecret { user_id, with } => {
                            let request = format!("a secret chat with {with}");
                            self.finding = Some(Finding::new(&request));
                            self.tg.start_secret_chat(user_id, request);
                        }
                        Confirmed::EndSecret { chat_id, secret_id } => {
                            // One the other side ended can't be closed again.
                            let open = self
                                .chats
                                .secret(chat_id)
                                .is_some_and(|s| s.state != SecretState::Closed);
                            self.tg.end_secret_chat(chat_id, secret_id, open)
                        }
                        Confirmed::JoinLink { link, request } => {
                            self.finding = Some(Finding::new(&request));
                            self.tg.join_by_link(link, request);
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
        if !msg.saveable {
            self.status = Some(match msg.destruct {
                Some(_) => "Self-destructing media can't be copied".into(),
                None => "This chat doesn't allow copying its messages".into(),
            });
            return;
        }
        let mut targets = Vec::new();
        if !msg.source_text.is_empty() {
            targets.push(Target::Text(msg.source_text.clone()));
        }
        targets.extend(msg.links.iter().cloned().map(Target::Link));
        targets.extend(
            msg.file
                .clone()
                .filter(|_| !opens_once(msg))
                .map(Target::File),
        );
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
        // In a secret chat, a paste meant for the message isn't sent to
        // Telegram's servers as a sticker search: it goes in the message.
        if self.focus == Focus::Input
            && self
                .open
                .as_ref()
                .is_some_and(|o| self.chats.is_secret(o.chat_id))
        {
            self.stickers = None;
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
            SettingsMenu::SECRET_CHATS => {
                settings.accept_secret_chats = !settings.accept_secret_chats;
                self.tg.accept_secret_chats(settings.accept_secret_chats);
            }
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
        // Moved by hand: no page loading later takes the cursor away.
        open.unread_after = None;
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
            self.load_more_chats(self.chats.shown());
        }
    }

    fn load_more_chats(&mut self, list: List) {
        if self.loading_lists.contains(&list) || self.loaded_lists.contains(&list) {
            return;
        }
        self.loading_lists.insert(list);
        self.tg.load_chats(list, CHAT_PAGE);
    }

    /// The list shown is still loading.
    pub fn chats_loading(&self) -> bool {
        self.loading_lists.contains(&self.chats.shown())
    }

    /// Tab (`step` 1) / Shift-Tab (-1) in the chat list: the next or
    /// previous folder, round the end, with the cursor on its first chat.
    fn switch_list(&mut self, step: isize) {
        if self.chats.tabs().is_empty() {
            self.status = Some("No folders yet: Telegram's apps can make them".into());
            return;
        }
        let list = self.chats.next_list(step);
        self.chats.show(list);
        self.chats.refresh();
        self.selected = self.chats.ids().first().copied();
        self.wanted_lists.insert(list);
        self.load_more_chats(list);
    }

    /// `p` in the list: pins the selected chat to the top of the list
    /// shown, or unpins it, on Telegram, so your other devices show it too.
    fn toggle_pin(&mut self) {
        let Some(chat_id) = self.selected else {
            return;
        };
        let pinned = self.chats.pinned(chat_id);
        self.tg.pin_chat(self.chats.shown(), chat_id, !pinned);
    }

    /// `m` in the list: mutes the selected chat for good, or unmutes it, on
    /// Telegram.
    fn toggle_mute(&mut self) {
        let Some(chat_id) = self.selected else {
            return;
        };
        let mute = !self.chats.muted(chat_id);
        if let Some(settings) = self.chats.with_mute(chat_id, mute) {
            self.tg.set_notifications(chat_id, settings);
        }
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
        // By the wall clock: Instant can't go back past boot on Windows.
        app.last_input_wall = SystemTime::now() - (IDLE_AFTER + Duration::from_secs(1));
        assert!(!app.watching(chat), "new messages aren't marked read");
        assert!(!app.sees(chat), "and they notify");

        // A terminal that reports focus is believed instead, for a while.
        app.focus_reported = true;
        assert!(app.watching(chat) && app.sees(chat));
        app.last_input_wall = SystemTime::now() - AWAY_AFTER;
        assert!(
            !app.watching(chat),
            "not for good: the screen may be left on"
        );
        assert!(app.sees(chat), "the window has focus, so no notification");
        app.last_input_wall = SystemTime::now();
        app.terminal_focused = false;
        assert!(!app.watching(chat) && !app.sees(chat));
    }

    #[test]
    fn time_the_computer_slept_counts_as_time_away() {
        let mut app = test_app("sleep");
        app.focus = Focus::Messages;
        let chat = app.open.as_ref().unwrap().chat_id;
        // A key ten seconds before a two-hour sleep: the monotonic clock
        // didn't run while asleep, the wall clock did.
        app.last_input = Instant::now();
        app.last_input_wall = SystemTime::now() - Duration::from_secs(2 * 3600);
        assert!(!app.watching(chat), "nothing is marked read on waking");
        app.online = false;
        app.update_online();
        assert!(!app.online, "and you aren't shown online");

        // A wall clock set back doesn't make you away.
        app.last_input_wall = SystemTime::now() + Duration::from_secs(3600);
        assert!(app.watching(chat));
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

    /// The demo app, with its data in a temp folder of its own.
    fn test_app(tag: &str) -> App {
        // One folder per test, reused by later runs rather than piling up.
        let dir = std::env::temp_dir().join(format!("tuigram-test-{tag}"));
        std::fs::create_dir_all(&dir).unwrap();
        let images = Images::new(Picker::halfblocks(), unbounded_channel().0);
        crate::demo::demo_app(Tg::detached(unbounded_channel().0), images, &dir)
    }

    #[test]
    fn ctrl_o_and_ctrl_i_go_back_and_forward_like_vims_jump_list() {
        let at = |chat_id, message_id| Jump {
            chat_id,
            message_id,
        };
        let mut jumps = Jumps::default();
        jumps.leave(at(1, None));
        jumps.leave(at(2, Some(5)));
        jumps.leave(at(2, Some(5)));
        assert_eq!(jumps.go(true, Some(at(3, None))), Some(at(2, Some(5))));
        assert_eq!(jumps.go(true, Some(at(2, Some(5)))), Some(at(1, None)));
        assert_eq!(
            jumps.go(true, Some(at(1, None))),
            None,
            "the same place once"
        );
        assert_eq!(jumps.go(false, Some(at(1, None))), Some(at(2, Some(5))));
        assert!(jumps.can_go_back() && jumps.can_go_forward());

        // Going somewhere new forgets where Ctrl-i would have gone.
        jumps.leave(at(2, Some(5)));
        assert!(!jumps.can_go_forward());
        for id in 0..MAX_JUMPS as i64 * 2 {
            jumps.leave(at(id, None));
        }
        assert_eq!(jumps.back.len(), MAX_JUMPS);
    }

    #[test]
    fn gd_then_ctrl_o_and_ctrl_i_move_between_a_reply_and_what_it_answers() {
        let mut app = test_app("jumps");
        app.focus = Focus::Messages;
        let (none, ctrl) = (KeyModifiers::NONE, KeyModifiers::CONTROL);
        let reply = app
            .open
            .as_ref()
            .unwrap()
            .messages
            .iter()
            .find_map(|(&id, m)| {
                let to = m.reply_to.as_ref()?.message_id?;
                Some((id, to))
            });
        let (from, to) = reply.expect("the demo has a reply");
        let cursor = |app: &App| app.open.as_ref().unwrap().selected;
        app.open.as_mut().unwrap().selected = Some(from);

        press(&mut app, KeyCode::Char('g'), none);
        press(&mut app, KeyCode::Char('d'), none);
        assert_eq!(cursor(&app), Some(to));
        press(&mut app, KeyCode::Char('o'), ctrl);
        assert_eq!(cursor(&app), Some(from));
        // Ctrl-i, from a terminal with the kitty keyboard protocol.
        press(&mut app, KeyCode::Char('i'), ctrl);
        assert_eq!(cursor(&app), Some(to));
        assert!(app.focus == Focus::Messages, "not Insert mode");
        // And as most terminals send it.
        press(&mut app, KeyCode::Char('o'), ctrl);
        press(&mut app, KeyCode::Tab, none);
        assert_eq!(cursor(&app), Some(to));
        press(&mut app, KeyCode::Tab, none);
        assert_eq!(app.status.as_deref(), Some("Nothing to go forward to"));
    }

    #[test]
    fn tab_and_shift_tab_in_the_chat_list_go_round_the_folders() {
        let mut app = test_app("folders");
        app.focus = Focus::Chats;
        // The demo's folders, loaded already, so nothing is asked of TDLib.
        let (friends, work) = (List::Folder(1), List::Folder(2));
        app.loaded_lists.extend([List::Main, friends, work]);
        let none = KeyModifiers::NONE;
        let top = |app: &mut App| screen(app)[1].clone();
        assert!(top(&mut app).contains(" All 3 "), "{}", top(&mut app));

        press(&mut app, KeyCode::Tab, none);
        assert_eq!(app.chats.shown(), friends);
        app.chats.refresh();
        assert!(
            app.chats
                .ids()
                .iter()
                .all(|&id| app.chats.in_list(id, friends))
        );
        press(&mut app, KeyCode::Tab, none);
        assert_eq!(app.chats.shown(), work);
        press(&mut app, KeyCode::Tab, none);
        assert_eq!(app.chats.shown(), List::Main, "round the end");
        press(&mut app, KeyCode::BackTab, none);
        assert_eq!(app.chats.shown(), work);
        let first = app.chats.ids()[0];
        assert_eq!(app.selected, Some(first), "on the folder's first chat");
    }

    /// The id of a message in the demo chat that Enter does nothing else
    /// with, under the cursor.
    fn plain_message(app: &mut App) -> i64 {
        let open = app.open.as_mut().unwrap();
        let id = open
            .messages
            .iter()
            .find(|(_, m)| m.state == SendState::Sent && m.poll.is_none() && !m.hides_spoilers())
            .map(|(&id, _)| id)
            .unwrap();
        open.selected = Some(id);
        id
    }

    #[test]
    fn tab_in_a_chats_search_finishes_filters_and_a_bad_one_says_why() {
        let mut app = test_app("search-filters");
        app.focus = Focus::Messages;
        let none = KeyModifiers::NONE;
        let typed = |app: &App| app.prompt.as_ref().unwrap().query();
        press(&mut app, KeyCode::Char('/'), none);
        for c in "trail h".chars() {
            press(&mut app, KeyCode::Char(c), none);
        }
        press(&mut app, KeyCode::Tab, none);
        assert_eq!(typed(&app), "trail has:");
        for c in "ph".chars() {
            press(&mut app, KeyCode::Char(c), none);
        }
        press(&mut app, KeyCode::Tab, none);
        assert_eq!(typed(&app), "trail has:photo");
        for c in " from:".chars() {
            press(&mut app, KeyCode::Char(c), none);
        }
        press(&mut app, KeyCode::Tab, none);
        assert_eq!(typed(&app), "trail has:photo from:me");

        for c in " has:video".chars() {
            press(&mut app, KeyCode::Char(c), none);
        }
        press(&mut app, KeyCode::Enter, none);
        assert_eq!(app.status.as_deref(), Some("Only one has: at a time"));
        assert!(app.open.as_ref().unwrap().search.is_none(), "nothing asked");
    }

    #[test]
    fn from_turns_me_a_name_or_a_known_username_into_a_sender() {
        let mut app = test_app("search-from");
        let resolve = |app: &App, typed: &str| {
            let ask = search::parse(typed).unwrap();
            app.resolve_sender(ask).map(|a| a.from)
        };
        let me = app.chats.my_id().unwrap();
        assert_eq!(
            resolve(&app, "from:me"),
            Ok(Some(Who::Sender(Sender::User(me))))
        );
        // A sender of the loaded messages, by part of their name.
        let (id, name) = app
            .open
            .as_ref()
            .unwrap()
            .messages
            .values()
            .find_map(|m| match m.sender {
                Sender::User(id) if id != me => Some((id, app.users.get(&id)?.clone())),
                _ => None,
            })
            .expect("someone else wrote");
        let first = name.split_whitespace().next().unwrap().to_lowercase();
        let found = resolve(&app, &format!("from:{first}"));
        assert_eq!(found, Ok(Some(Who::Sender(Sender::User(id)))));
        assert!(
            resolve(&app, "from:Zelda")
                .unwrap_err()
                .starts_with("Nobody called Zelda")
        );

        app.chats.set_username(
            Peer::User(id),
            Some(&tdlib_rs::types::Usernames {
                active_usernames: vec!["maya".into()],
                ..Default::default()
            }),
        );
        assert_eq!(
            resolve(&app, "from:@Maya"),
            Ok(Some(Who::Sender(Sender::User(id))))
        );
        assert_eq!(
            resolve(&app, "from:@stranger"),
            Ok(Some(Who::Username("stranger".into()))),
            "TDLib looks it up"
        );
    }

    #[test]
    fn tab_completes_a_command_and_goes_on_to_the_next_that_fits() {
        let mut app = test_app("command-tab");
        let none = KeyModifiers::NONE;
        let typed = |app: &App| app.prompt.as_ref().unwrap().query();
        press(&mut app, KeyCode::Char(':'), none);
        press(&mut app, KeyCode::Char('l'), none);
        press(&mut app, KeyCode::Tab, none);
        assert_eq!(typed(&app), "leave");
        let rows = screen(&mut app).join("\n");
        assert!(rows.contains("Commands · Tab completes"), "{rows}");
        press(&mut app, KeyCode::Tab, none);
        assert_eq!(typed(&app), "logout");
        press(&mut app, KeyCode::Tab, none);
        assert_eq!(typed(&app), "leave", "round the end");
        press(&mut app, KeyCode::BackTab, none);
        assert_eq!(typed(&app), "logout");

        // Typing starts over from what's there.
        for _ in 0.."logout".len() - 2 {
            press(&mut app, KeyCode::Backspace, none);
        }
        press(&mut app, KeyCode::Tab, none);
        assert_eq!(typed(&app), "logout", "the only one with lo");
        press(&mut app, KeyCode::Tab, none);
        assert_eq!(typed(&app), "logout");

        app.prompt = None;
        press(&mut app, KeyCode::Char(':'), none);
        press(&mut app, KeyCode::Char('x'), none);
        press(&mut app, KeyCode::Tab, none);
        assert_eq!(typed(&app), "x", "nothing fits");
        assert!(app.prompt.is_some(), "and nothing ran");
    }

    #[test]
    fn gp_lists_the_pinned_messages_and_enter_goes_to_one() {
        let mut app = test_app("pinned");
        app.focus = Focus::Messages;
        let (none, ctrl) = (KeyModifiers::NONE, KeyModifiers::CONTROL);
        let pinned = app.open.as_ref().unwrap().pinned[0].id;
        let cursor = |app: &App| app.open.as_ref().unwrap().selected;
        assert_eq!(cursor(&app), None, "on the newest");

        press(&mut app, KeyCode::Char('g'), none);
        press(&mut app, KeyCode::Char('p'), none);
        assert!(app.pinned_menu.is_some(), "not a paste");
        assert!(
            screen(&mut app)
                .join("\n")
                .contains(" Pinned messages (1) ")
        );
        press(&mut app, KeyCode::Enter, none);
        assert!(app.pinned_menu.is_none());
        assert_eq!(cursor(&app), Some(pinned));
        press(&mut app, KeyCode::Char('o'), ctrl);
        assert_eq!(cursor(&app), None, "Ctrl-o comes back");

        app.open.as_mut().unwrap().pinned.clear();
        press(&mut app, KeyCode::Char('g'), none);
        press(&mut app, KeyCode::Char('p'), none);
        assert!(app.pinned_menu.is_none());
        assert_eq!(
            app.status.as_deref(),
            Some("No pinned messages in this chat")
        );
    }

    #[test]
    fn the_pin_popup_fills_in_once_tdlib_says_and_old_lists_are_dropped() {
        let mut app = test_app("pin");
        let chat_id = app.open.as_ref().unwrap().chat_id;
        let menu = |id| PinMenu::new(id, "hi".into(), Place::Group, String::new());
        let pinnable = |message_id, pinnable| TgEvent::Pinnable {
            chat_id,
            message_id,
            pinnable: Some(pinnable),
        };
        app.pin_menu = Some(menu(4));
        app.on_tg(pinnable(5, true));
        assert!(
            app.pin_menu.as_ref().unwrap().choices.is_empty(),
            "another message"
        );
        app.on_tg(pinnable(4, true));
        assert_eq!(app.pin_menu.as_ref().unwrap().choices.len(), 2);
        app.pin_menu = Some(menu(4));
        app.on_tg(pinnable(4, false));
        assert!(app.pin_menu.is_none());
        assert_eq!(app.status.as_deref(), Some("You can't pin messages here"));

        // An answer to an older request misses what was pinned since.
        app.open.as_mut().unwrap().pinned_asked = 2;
        app.on_tg(TgEvent::Pinned {
            chat_id,
            request: 1,
            messages: Some(Vec::new()),
        });
        assert_eq!(app.open.as_ref().unwrap().pinned.len(), 1);
        app.on_tg(TgEvent::Pinned {
            chat_id,
            request: 2,
            messages: Some(Vec::new()),
        });
        assert!(app.open.as_ref().unwrap().pinned.is_empty());
    }

    #[test]
    fn enter_on_a_bots_message_lists_its_buttons_then_its_links() {
        use crate::buttons::{Button, Keyboard};
        let mut app = test_app("buttons");
        app.focus = Focus::Messages;
        let id = plain_message(&mut app);
        let none = KeyModifiers::NONE;
        let msg = app.open.as_mut().unwrap().messages.get_mut(&id).unwrap();
        msg.links = vec![Link::from("https://example.com/menu")];
        msg.keyboard = Some(Keyboard {
            rows: vec![vec![
                Button {
                    label: "Play".into(),
                    press: Press::Unsupported("Games only run in Telegram's own apps"),
                },
                Button {
                    label: "Our site".into(),
                    press: Press::Open(Link {
                        url: "https://shop.example/".into(),
                        disguise: Some("Our site".into()),
                    }),
                },
            ]],
            reply: false,
        });

        press(&mut app, KeyCode::Enter, none);
        let menu = app.button_menu.as_ref().expect("the buttons");
        assert_eq!(menu.rows.len(), 2, "and the message's link");
        let rows = screen(&mut app).join("\n");
        assert!(rows.contains(" Buttons "), "{rows}");
        assert!(rows.contains("Open link: example.com"), "{rows}");
        assert!(
            rows.contains("Games only run in Telegram's own apps"),
            "what Enter does"
        );

        // A second Enter right after the first, or one held down, presses
        // nothing.
        press(&mut app, KeyCode::Enter, none);
        assert_eq!(app.status, None);
        let ago = Instant::now() - CONFIRM_GRACE;
        app.button_menu.as_mut().unwrap().shown = ago;
        press(&mut app, KeyCode::Enter, none);
        assert_eq!(
            app.status.as_deref(),
            Some("Games only run in Telegram's own apps")
        );
        assert!(app.button_menu.is_some(), "stays up");
        press(&mut app, KeyCode::Char('l'), none);
        press(&mut app, KeyCode::Enter, none);
        assert!(app.button_menu.is_none());
        let confirm = app.confirm.as_ref().expect("its words aren't its address");
        assert!(matches!(&confirm.action, Confirmed::OpenLink(u) if u == "https://shop.example/"));

        // No number presses one: none are drawn.
        app.confirm = None;
        press(&mut app, KeyCode::Enter, none);
        app.button_menu.as_mut().unwrap().shown = ago;
        press(&mut app, KeyCode::Char('2'), none);
        assert!(app.confirm.is_none() && app.button_menu.is_some());
    }

    #[test]
    fn logout_asks_first_now_that_tab_can_put_it_in() {
        let mut app = test_app("logout");
        let none = KeyModifiers::NONE;
        press(&mut app, KeyCode::Char(':'), none);
        press(&mut app, KeyCode::Char('l'), none);
        press(&mut app, KeyCode::Char('o'), none);
        press(&mut app, KeyCode::Tab, none);
        assert_eq!(app.prompt.as_ref().unwrap().query(), "logout");
        press(&mut app, KeyCode::Enter, none);
        let confirm = app.confirm.as_ref().expect("asks");
        assert!(matches!(confirm.action, Confirmed::Logout));
        assert!(matches!(app.screen, Screen::Main), "still logged in");
        press(&mut app, KeyCode::Char('n'), none);
        assert!(app.confirm.is_none() && matches!(app.screen, Screen::Main));
    }

    #[test]
    fn nothing_is_marked_read_while_a_jump_to_an_older_message_loads() {
        let mut app = test_app("jump-read");
        app.focus = Focus::Messages;
        let chat = app.open.as_ref().unwrap().chat_id;
        assert!(app.watching(chat));
        // What arrives meanwhile would be inserted at the bottom, unseen.
        app.open.as_mut().unwrap().loading = Some(Page::Around(3));
        assert!(!app.watching(chat));
    }

    #[test]
    fn a_bots_answer_is_a_note_or_an_alert_and_its_link_asks_first() {
        let mut app = test_app("bot-answer");
        app.focus = Focus::Messages;
        let chat_id = app.open.as_ref().unwrap().chat_id;
        let answer = |text: &str, alert, url: &str| TgEvent::BotAnswer {
            chat_id,
            label: "Buy".into(),
            text: text.into(),
            alert,
            url: url.into(),
        };

        app.on_tg(answer("Added\nto cart", false, ""));
        let toast = app.toast.as_ref().expect("a note in the corner");
        assert_eq!(
            (toast.title.as_str(), toast.detail.as_str()),
            ("Buy", "Added to cart")
        );

        app.on_tg(answer("Sold \u{202e}out", true, ""));
        assert_eq!(app.notice.as_ref().unwrap().text, "Sold out");
        assert!(screen(&mut app).join("\n").contains("Sold out"));
        press(&mut app, KeyCode::Char('x'), KeyModifiers::NONE);
        assert!(app.notice.is_some(), "only Enter or Esc closes it");
        press(&mut app, KeyCode::Enter, KeyModifiers::NONE);
        assert!(app.notice.is_none());

        app.on_tg(answer("", false, "https://game.example/play"));
        let confirm = app.confirm.take().expect("asks before opening");
        assert!(
            matches!(confirm.action, Confirmed::OpenLink(u) if u == "https://game.example/play")
        );

        // Writing: nothing pops up over it.
        app.focus = Focus::Input;
        app.on_tg(answer("Sold out", true, "https://game.example/play"));
        assert!(app.notice.is_none() && app.confirm.is_none());
        assert_eq!(app.toast.as_ref().unwrap().detail, "Sold out");
    }

    #[test]
    fn f_opens_a_list_of_your_chats_to_forward_to_saved_messages_first() {
        let mut app = test_app("forward");
        app.focus = Focus::Messages;
        let none = KeyModifiers::NONE;
        press(&mut app, KeyCode::Char('f'), none);
        let picker = app.picker.as_ref().expect("the picker is open");
        let Purpose::Forward {
            from, message_ids, ..
        } = &picker.purpose
        else {
            panic!("forwarding");
        };
        let open = app.open.as_ref().unwrap();
        assert_eq!(*from, open.chat_id);
        assert_eq!(message_ids, &[open.cursor_id().unwrap()]);
        assert!(
            matches!(picker.choices(&app.chats)[0], Choice::Chat(id) if app.chats.is_saved(id)),
            "Saved Messages first"
        );
        let rows = screen(&mut app);
        assert!(rows.iter().any(|r| r.contains("Forward to")), "{rows:#?}");
        assert!(rows.iter().any(|r| r.contains("Saved Messages")));

        // Letters type the search, even j and k.
        for c in "tokyo".chars() {
            press(&mut app, KeyCode::Char(c), none);
        }
        let picker = app.picker.as_ref().unwrap();
        assert_eq!(picker.query, "tokyo");
        let titles: Vec<&str> = picker
            .choices(&app.chats)
            .iter()
            .map(|c| match c {
                Choice::Chat(id) => app.chats.title(*id).unwrap(),
                _ => "",
            })
            .collect();
        assert_eq!(titles, ["Tokyo Trip"]);
        press(&mut app, KeyCode::Esc, none);
        assert!(app.picker.is_none());
    }

    #[test]
    fn s_finds_a_chat_by_username_or_link_and_pastes_go_into_its_search() {
        let mut app = test_app("find");
        let none = KeyModifiers::NONE;
        press(&mut app, KeyCode::Char('s'), none);
        assert!(app.picker.as_ref().is_some_and(|p| !p.forwarding()));
        for c in "@durov".chars() {
            press(&mut app, KeyCode::Char(c), none);
        }
        let rows = screen(&mut app);
        assert!(rows.iter().any(|r| r.contains("Open @durov")), "{rows:#?}");

        press(&mut app, KeyCode::Char('u'), KeyModifiers::CONTROL);
        app.on_terminal_event(Event::Paste("t.me/+AbCdEf\n".into()));
        let picker = app.picker.as_ref().unwrap();
        assert_eq!(picker.query, "t.me/+AbCdEf", "not attached as a file");
        assert_eq!(
            picker.choices(&app.chats)[0],
            Choice::Link("https://t.me/+AbCdEf".into())
        );
        assert!(picker.search_at().is_none(), "links aren't searched for");
        press(&mut app, KeyCode::Char('c'), KeyModifiers::CONTROL);
        assert!(app.picker.is_none(), "Ctrl-c closes it");
        assert!(app.quit_deadline.is_none(), "without quitting");
    }

    #[test]
    fn ctrl_u_deletes_back_to_the_start_of_the_line_and_then_the_line_break() {
        let mut app = test_app("ctrl-u");
        let chat_id = app.open.as_ref().unwrap().chat_id;
        app.focus = Focus::Input;
        // Typing was already told, so this test sends nothing to TDLib.
        app.typing = Some((chat_id, Instant::now()));
        app.composer.insert_str("first line");
        app.composer.insert_newline();
        app.composer.insert_str("second line");
        app.composer
            .move_cursor(ratatui_textarea::CursorMove::WordBack);
        press(&mut app, KeyCode::Char('u'), KeyModifiers::CONTROL);
        assert_eq!(app.composer.lines(), ["first line", "line"]);

        press(&mut app, KeyCode::Char('u'), KeyModifiers::CONTROL);
        assert_eq!(app.composer.lines(), ["first lineline"]);
        assert!(app.focus == Focus::Input);
    }

    #[test]
    fn a_chat_found_while_you_write_does_not_open_over_what_you_type() {
        let mut app = test_app("late-find");
        let chat_id = app.open.as_ref().unwrap().chat_id;
        app.focus = Focus::Input;
        app.composer.insert_str("see you at 5");
        app.finding = Some(Finding::new("@bob"));
        app.on_tg(TgEvent::ChatFound {
            request: "@bob".into(),
            found: Ok((999, None)),
        });
        assert_eq!(app.open.as_ref().unwrap().chat_id, chat_id);
        assert!(app.focus == Focus::Input);
        assert_eq!(app.composer.lines(), ["see you at 5"]);
        assert!(app.finding.is_none());
        assert_eq!(
            app.status.as_deref(),
            Some("Found @bob: press s to open it")
        );

        // Nor does an invite ask to join over the composer.
        app.finding = Some(Finding::new("https://t.me/+x"));
        let invite = Invite {
            title: "Group".into(),
            members: 3,
            channel: false,
            badge: None,
            by_request: false,
        };
        app.on_tg(TgEvent::Invite {
            request: "https://t.me/+x".into(),
            link: "https://t.me/+x".into(),
            invite,
        });
        assert!(app.confirm.is_none());
    }

    #[test]
    fn writing_a_popup_or_esc_stops_a_lookup() {
        let mut app = test_app("stop-find");
        app.focus = Focus::Messages;
        app.finding = Some(Finding::new("@bob"));
        press(&mut app, KeyCode::Esc, KeyModifiers::NONE);
        assert!(app.finding.is_none());
        assert!(app.focus == Focus::Messages, "only the lookup stopped");

        app.finding = Some(Finding::new("@bob"));
        press(&mut app, KeyCode::Char('i'), KeyModifiers::NONE);
        assert!(app.finding.is_none(), "writing in this chat instead");
    }

    #[test]
    fn e_edits_your_message_as_markdown_and_an_untouched_edit_sends_nothing() {
        let mut app = test_app("edit-markdown");
        app.focus = Focus::Messages;
        let open = app.open.as_ref().unwrap();
        let (chat_id, id) = (open.chat_id, open.edit_target().unwrap());
        let text = EditText {
            markdown: "On my **way**!".into(),
            loses: false,
        };
        app.on_editable(chat_id, id, Some(true), Some(text));
        assert!(app.focus == Focus::Input);
        assert_eq!(app.composer.lines(), ["On my **way**!"]);
        // Unchanged, so Enter only ends the edit: no request.
        press(&mut app, KeyCode::Enter, KeyModifiers::NONE);
        assert!(app.open.as_ref().unwrap().editing.is_none());

        // Formatting Markdown can't write asks first.
        app.focus = Focus::Messages;
        let text = EditText {
            markdown: "On my way!".into(),
            loses: true,
        };
        app.on_editable(chat_id, id, Some(true), Some(text));
        let confirm = app.confirm.as_ref().expect("asks");
        assert!(matches!(&confirm.action, Confirmed::Edit { text, .. } if text == "On my way!"));
    }

    #[test]
    fn at_and_colon_suggest_people_and_emoji_and_tab_puts_one_in() {
        let mut app = test_app("complete");
        let chat_id = app.open.as_ref().unwrap().chat_id;
        app.focus = Focus::Input;
        // Typing was already told, so this test sends nothing to TDLib.
        app.typing = Some((chat_id, Instant::now()));

        app.composer.insert_str("hi @ma");
        app.update_completion();
        let completion = app.completion.as_ref().expect("suggestions");
        let labels: Vec<&str> = completion.items.iter().map(|s| s.label.as_str()).collect();
        assert_eq!(labels, ["Maya Chen"], "senders of the loaded messages");
        assert!(
            completion.search_at().is_some(),
            "members are searched once typing pauses"
        );
        press(&mut app, KeyCode::Tab, KeyModifiers::NONE);
        assert_eq!(app.composer.lines(), ["hi [Maya Chen](tg://user?id=2) "]);
        assert!(
            app.stickers.is_none(),
            "Tab took the suggestion, not the stickers"
        );

        app.composer.insert_str(":tada");
        app.update_completion();
        assert_eq!(app.completion.as_ref().unwrap().items[0].label, "🎉");
        press(&mut app, KeyCode::Tab, KeyModifiers::NONE);
        assert_eq!(app.composer.lines(), ["hi [Maya Chen](tg://user?id=2) 🎉"]);
        assert!(app.completion.is_none());

        app.composer.insert_str(" @sam");
        app.update_completion();
        assert!(
            app.completion.as_ref().is_none_or(|c| c.items.is_empty()),
            "not yourself"
        );
    }

    #[test]
    fn slash_starting_a_message_suggests_the_bots_commands() {
        let mut app = test_app("commands");
        let chat_id = app.open.as_ref().unwrap().chat_id;
        app.focus = Focus::Input;
        app.typing = Some((chat_id, Instant::now()));
        let command = |name: &str| complete::Command {
            bot: 9,
            name: name.into(),
            description: format!("{name} the bot"),
        };
        app.open.as_mut().unwrap().commands =
            Commands::Known(vec![command("start"), command("help")]);

        app.composer.insert_str("/he");
        app.update_completion();
        let completion = app.completion.as_ref().expect("suggestions");
        assert_eq!(completion.items[0].label, "/help");
        assert_eq!(completion.items[0].detail, "help the bot");
        press(&mut app, KeyCode::Tab, KeyModifiers::NONE);
        assert_eq!(app.composer.lines(), ["/help "]);

        app.composer.insert_str("see /st");
        app.update_completion();
        assert!(app.completion.is_none(), "only at the start of a message");
    }

    #[test]
    fn a_telegram_link_tuigram_cant_open_goes_to_the_browser_with_its_warning() {
        let mut app = test_app("link-elsewhere");
        let url = "https://t.me/addstickers/cats";
        app.finding = Some(Finding {
            request: url.into(),
            link: Some(Link {
                url: url.into(),
                disguise: Some("cute cats".into()),
            }),
        });
        app.on_tg(TgEvent::ChatFound {
            request: url.into(),
            found: Err(Missed::Elsewhere),
        });
        let confirm = app.confirm.as_ref().expect("the disguised link asks first");
        assert!(matches!(&confirm.action, Confirmed::OpenLink(u) if u == url));

        // From `s`, there's no browser to fall back on.
        app.confirm = None;
        app.finding = Some(Finding::new(url));
        app.on_tg(TgEvent::ChatFound {
            request: url.into(),
            found: Err(Missed::Elsewhere),
        });
        assert!(app.confirm.is_none());
        assert_eq!(
            app.status.as_deref(),
            Some("tuigram can't open this kind of link")
        );
    }

    #[test]
    fn enter_on_a_poll_opens_its_answers_unless_it_is_closed() {
        let mut app = test_app("poll");
        app.focus = Focus::Messages;
        let open = app.open.as_mut().unwrap();
        let id = open.cursor_id().unwrap();
        let mut poll = crate::poll::Poll {
            question: "Lunch?".into(),
            answers: vec![crate::poll::Answer {
                text: "Pizza".into(),
                voters: 0,
                percent: 0,
                chosen: false,
            }],
            voters: 0,
            several: false,
            quiz: false,
            correct: None,
            anonymous: true,
            closed: false,
        };
        open.messages.get_mut(&id).unwrap().poll = Some(poll.clone());
        press(&mut app, KeyCode::Enter, KeyModifiers::NONE);
        let menu = app.vote_menu.as_ref().expect("the vote popup");
        assert_eq!(menu.answers, ["Pizza"]);
        let rows = screen(&mut app);
        assert!(rows.iter().any(|r| r.contains("Vote")), "{rows:#?}");
        press(&mut app, KeyCode::Esc, KeyModifiers::NONE);
        assert!(app.vote_menu.is_none());

        poll.closed = true;
        let open = app.open.as_mut().unwrap();
        open.messages.get_mut(&id).unwrap().poll = Some(poll);
        press(&mut app, KeyCode::Enter, KeyModifiers::NONE);
        assert!(app.vote_menu.is_none());
        assert_eq!(app.status.as_deref(), Some("This poll is closed"));
    }

    #[test]
    fn leave_asks_first_and_only_for_groups_and_channels() {
        let mut app = test_app("leave");
        let none = KeyModifiers::NONE;
        let leave = |app: &mut App| {
            press(app, KeyCode::Char(':'), none);
            for c in "leave".chars() {
                press(app, KeyCode::Char(c), none);
            }
            press(app, KeyCode::Enter, none);
        };
        app.focus = Focus::Messages;
        leave(&mut app);
        let confirm = app.confirm.as_ref().expect("asks");
        let chat_id = app.open.as_ref().unwrap().chat_id;
        assert!(matches!(confirm.action, Confirmed::Leave(id) if id == chat_id));
        assert_eq!(confirm.title, "Leave this group?");
        press(&mut app, KeyCode::Esc, none);

        // A chat with one person, selected in the list.
        app.focus = Focus::Chats;
        let mom = app
            .chats
            .ids()
            .iter()
            .copied()
            .find(|&id| app.chats.title(id) == Some("Mom"))
            .unwrap();
        app.selected = Some(mom);
        leave(&mut app);
        assert!(app.confirm.is_none());
        assert_eq!(
            app.status.as_deref(),
            Some("A chat with one person can't be left")
        );
    }

    #[test]
    fn i_in_a_public_group_you_are_not_in_asks_to_join_first() {
        let mut app = test_app("join");
        let chat_id = app.open.as_ref().unwrap().chat_id;
        app.chats.add_local(chat_id, "Weekend Hike", None).peer = Some(Peer::Supergroup(77));
        app.chats.set_member(77, false);
        app.focus = Focus::Messages;
        press(&mut app, KeyCode::Char('i'), KeyModifiers::NONE);
        assert!(app.focus == Focus::Messages, "not writing yet");
        let confirm = app.confirm.as_ref().expect("asks");
        assert!(matches!(confirm.action, Confirmed::Join(id) if id == chat_id));
        press(&mut app, KeyCode::Esc, KeyModifiers::NONE);

        app.chats.set_member(77, true);
        press(&mut app, KeyCode::Char('i'), KeyModifiers::NONE);
        assert!(app.focus == Focus::Input, "once you're in");
    }

    #[test]
    fn a_y_typed_just_before_a_warning_came_up_does_not_answer_it() {
        let mut app = test_app("grace");
        let edit = Confirmed::Edit {
            id: -1,
            text: String::new(),
        };
        app.confirm = Some(Confirm::new("Edit?", vec![], edit));
        press(&mut app, KeyCode::Char('y'), KeyModifiers::NONE);
        assert!(app.confirm.is_some(), "too soon to count");
        let confirm = app.confirm.as_mut().unwrap();
        confirm.shown = Instant::now().checked_sub(CONFIRM_GRACE).unwrap();
        press(&mut app, KeyCode::Char('y'), KeyModifiers::NONE);
        assert!(
            app.confirm.is_none(),
            "answered once it has been up a moment"
        );
    }

    #[test]
    fn a_download_finishing_while_you_type_waits_for_enter_instead_of_asking() {
        let mut app = test_app("busy");
        let dir = std::env::temp_dir().join("tuigram-no-such-folder");
        let downloaded = |app: &mut App, name: &str| {
            app.opening.insert(77);
            let path = dir.join(name).to_string_lossy().into_owned();
            app.on_tg(TgEvent::Downloaded {
                file_id: 77,
                path: Some(path),
            });
        };
        app.focus = Focus::Input;
        downloaded(&mut app, "run me.sh");
        assert!(app.confirm.is_none(), "no popup over the composer");
        let status = app.status.clone().unwrap_or_default();
        assert!(status.contains("run me.sh downloaded"), "{status}");

        app.focus = Focus::Messages;
        // Not a safe type, so nothing is opened.
        downloaded(&mut app, "evil\u{202e}txt.sh");
        let title = &app
            .confirm
            .as_ref()
            .expect("asks when nothing else is up")
            .title;
        assert!(
            !title.contains('\u{202e}'),
            "the sender's name is cleaned: {title:?}"
        );
    }

    /// Types a `:` command and runs it.
    fn command(app: &mut App, name: &str) {
        press(app, KeyCode::Char(':'), KeyModifiers::NONE);
        for c in name.chars() {
            press(app, KeyCode::Char(c), KeyModifiers::NONE);
        }
        press(app, KeyCode::Enter, KeyModifiers::NONE);
    }

    /// Adds a chat with Chardy, user 2: a plain one with id 500, or with
    /// id 600 a secret one in `state`, and selects it in the list.
    fn chat_with_chardy(app: &mut App, secret: Option<crate::secret::SecretState>) -> i64 {
        let chat_id = if secret.is_some() { 600 } else { 500 };
        let chat = app.chats.add_local(chat_id, "Chardy", None);
        (chat.is_private, chat.peer) = (true, Some(Peer::User(2)));
        if let Some(state) = secret {
            chat.secret_id = Some(9);
            let secret = Secret {
                user_id: 2,
                state,
                outbound: true,
                key_hash: Vec::new(),
            };
            app.chats.set_secret(9, secret);
        }
        app.users.insert(2, "Chardy".into());
        app.chats.refresh();
        app.focus = Focus::Chats;
        app.selected = Some(chat_id);
        chat_id
    }

    /// [`chat_with_chardy`], open: `:key` and `:timer` are about the open
    /// chat, whatever the list's cursor is on.
    fn open_chardy(app: &mut App, secret: Option<crate::secret::SecretState>) -> i64 {
        let chat_id = chat_with_chardy(app, secret);
        app.open = Some(OpenChat::new(chat_id));
        app.selected = None;
        chat_id
    }

    #[test]
    fn secret_asks_before_starting_a_secret_chat_with_a_person_only() {
        let mut app = test_app("secret-start");
        chat_with_chardy(&mut app, None);
        command(&mut app, "secret");
        let confirm = app.confirm.take().expect("asks");
        assert!(matches!(
            &confirm.action,
            Confirmed::StartSecret { user_id: 2, with } if with == "Chardy"
        ));

        app.chats.set_bot(2, true);
        command(&mut app, "secret");
        assert!(app.confirm.is_none());
        assert_eq!(app.status.as_deref(), Some("Bots can't be in secret chats"));

        // The open chat is a group.
        app.focus = Focus::Messages;
        command(&mut app, "secret");
        assert!(app.confirm.is_none());
        let status = app.status.clone().unwrap_or_default();
        assert!(status.starts_with("Open a chat with someone"), "{status}");
    }

    #[test]
    fn leave_in_a_secret_chat_asks_to_end_it() {
        let mut app = test_app("secret-leave");
        let chat_id = chat_with_chardy(&mut app, Some(crate::secret::SecretState::Ready));
        command(&mut app, "leave");
        let confirm = app.confirm.as_ref().expect("asks");
        assert_eq!(confirm.title, "End this secret chat?");
        assert!(matches!(
            confirm.action,
            Confirmed::EndSecret { chat_id: id, secret_id: 9 } if id == chat_id
        ));
    }

    #[test]
    fn timer_lists_the_timers_with_the_cursor_on_the_chats_own() {
        use crate::secret::SecretState;
        let mut app = test_app("secret-timer");
        let chat_id = open_chardy(&mut app, Some(SecretState::Pending));
        command(&mut app, "timer");
        assert!(app.timer_menu.is_none(), "not until they accept it");
        assert_eq!(
            app.status.as_deref(),
            Some("Waiting for Chardy to come online")
        );

        open_chardy(&mut app, Some(SecretState::Ready));
        app.chats.set_auto_delete(chat_id, 30);
        command(&mut app, "timer");
        let at_30 = crate::secret::TIMERS.iter().position(|&t| t == 30).unwrap();
        assert_eq!(app.timer_menu.as_ref().map(|m| m.selected), Some(at_30));
        let rows = screen(&mut app).join("\n");
        assert!(rows.contains("Self-destruct timer · Chardy"), "{rows}");
        assert!(rows.contains("● 30 seconds"), "{rows}");
        press(&mut app, KeyCode::Char('j'), KeyModifiers::NONE);
        press(&mut app, KeyCode::Char('k'), KeyModifiers::NONE);
        // The timer it has already: nothing to send.
        press(&mut app, KeyCode::Enter, KeyModifiers::NONE);
        assert!(app.timer_menu.is_none());

        // Not in a chat that isn't secret, even with the list's cursor on
        // a secret one.
        open_chardy(&mut app, None);
        app.focus = Focus::Chats;
        app.selected = Some(chat_id);
        command(&mut app, "timer");
        assert!(app.timer_menu.is_none());
    }

    #[test]
    fn key_shows_the_secret_chats_key_once_there_is_one() {
        use crate::secret::SecretState;
        let mut app = test_app("secret-key");
        open_chardy(&mut app, Some(SecretState::Pending));
        command(&mut app, "key");
        assert!(app.key_view.is_none());
        assert_eq!(
            app.status.as_deref(),
            Some("The key is made once Chardy accepts the secret chat")
        );

        let key = Secret {
            user_id: 2,
            state: SecretState::Ready,
            outbound: true,
            key_hash: (0..36).collect(),
        };
        app.chats.set_secret(9, key);
        command(&mut app, "key");
        assert!(app.key_view.is_some());
        let rows = screen(&mut app).join("\n");
        assert!(rows.contains("Encryption key · Chardy"), "{rows}");
        assert!(rows.contains("00 01 02 03  04 05 06 07"), "{rows}");
        assert!(rows.contains("If Chardy sees the same picture"), "{rows}");
        press(&mut app, KeyCode::Esc, KeyModifiers::NONE);
        assert!(app.key_view.is_none());
    }

    #[test]
    fn a_secret_chat_takes_no_message_until_accepted_and_none_forwarded_from_it() {
        use crate::secret::SecretState;
        let mut app = test_app("secret-write");
        let chat_id = chat_with_chardy(&mut app, Some(SecretState::Pending));
        let mut open = OpenChat::new(chat_id);
        let id = app
            .open
            .as_ref()
            .unwrap()
            .messages
            .keys()
            .copied()
            .next()
            .unwrap();
        let msg = app.open.as_mut().unwrap().messages.remove(&id).unwrap();
        open.messages.insert(id, msg);
        app.open = Some(open);
        app.focus = Focus::Messages;
        press(&mut app, KeyCode::Char('i'), KeyModifiers::NONE);
        assert!(app.focus == Focus::Messages, "stays in Normal mode");
        assert_eq!(
            app.status.as_deref(),
            Some("Waiting for Chardy to come online")
        );

        chat_with_chardy(&mut app, Some(SecretState::Closed));
        app.focus = Focus::Messages;
        press(&mut app, KeyCode::Char('i'), KeyModifiers::NONE);
        assert_eq!(app.status.as_deref(), Some("This secret chat has ended"));

        press(&mut app, KeyCode::Char('f'), KeyModifiers::NONE);
        assert!(app.picker.is_none());
        assert_eq!(
            app.status.as_deref(),
            Some("Messages in secret chats can't be forwarded")
        );
    }

    #[test]
    fn y_copies_nothing_telegram_says_cant_be_saved() {
        let mut app = test_app("secret-copy");
        app.focus = Focus::Messages;
        let id = plain_message(&mut app);
        let open = app.open.as_mut().unwrap();
        let msg = open.messages.get_mut(&id).unwrap();
        msg.saveable = false;
        msg.destruct = Some(crate::secret::Destruct {
            after: 10,
            on_open: true,
            ends: None,
        });
        press(&mut app, KeyCode::Char('y'), KeyModifiers::NONE);
        assert!(app.menu.is_none() && app.toast.is_none());
        assert_eq!(
            app.status.as_deref(),
            Some("Self-destructing media can't be copied")
        );
    }

    /// Adds a photo shown only while open, lasting 10 seconds once opened,
    /// as the newest message of the open chat, under the cursor.
    fn secret_photo(app: &mut App) -> i64 {
        let open = app.open.as_mut().unwrap();
        let id = open.newest_id().unwrap() + 1;
        let mut photo = crate::messages::test_secret_photo(10);
        photo.outgoing = false;
        open.messages.insert(id, photo);
        open.selected = None;
        app.focus = Focus::Messages;
        id
    }

    #[test]
    fn a_photo_shown_only_while_open_is_covered_once_nobody_is_there() {
        let mut app = test_app("secret-away");
        let id = secret_photo(&mut app);
        assert!(app.open.as_mut().unwrap().uncover(id));
        app.cover_unseen();
        assert_eq!(app.open.as_ref().unwrap().viewing, Some(id), "looked at");

        // tmux without focus-events: a minute without a key is away.
        app.last_input_wall = SystemTime::now() - (IDLE_AFTER + Duration::from_secs(1));
        app.cover_unseen();
        let open = app.open.as_ref().unwrap();
        assert_eq!(open.viewing, None);
        assert!(open.messages[&id].preview.is_none(), "covered");
    }

    #[test]
    fn the_sender_hears_a_photo_was_opened_only_if_it_was_seen() {
        let mut app = test_app("secret-opened");
        let id = secret_photo(&mut app);
        let open = app.open.as_mut().unwrap();
        open.uncover(id);
        open.opening = Some((id, 20));
        // The download failed: nothing was seen.
        app.on_tg(TgEvent::Downloaded {
            file_id: 20,
            path: None,
        });
        assert_eq!(app.open.as_ref().unwrap().opening, None);

        // The cursor left it before it came: covered, and nothing is told.
        let open = app.open.as_mut().unwrap();
        open.opening = Some((id, 20));
        open.selected = Some(id - 1);
        app.cover_unseen();
        assert_eq!(app.open.as_ref().unwrap().opening, None);
        app.on_tg(TgEvent::Downloaded {
            file_id: 20,
            path: Some("/nowhere/photo.jpg".into()),
        });
        let photo = &app.open.as_ref().unwrap().messages[&id];
        assert!(
            photo.destruct.is_some_and(|d| d.ends.is_none()),
            "its timer didn't start"
        );
    }

    #[test]
    fn media_seen_only_while_open_is_never_handed_to_another_app() {
        let mut app = test_app("secret-voice");
        app.focus = Focus::Messages;
        let id = plain_message(&mut app);
        let msg = app.open.as_mut().unwrap().messages.get_mut(&id).unwrap();
        msg.file = Some(MediaFile {
            id: 30,
            label: "Voice message".into(),
            photo: false,
        });
        msg.links.clear();
        msg.source_text.clear();
        msg.destruct = Some(crate::secret::Destruct {
            after: 30,
            on_open: true,
            ends: None,
        });
        press(&mut app, KeyCode::Enter, KeyModifiers::NONE);
        assert!(app.opening.is_empty(), "not downloaded to open");
        let status = app.status.clone().unwrap_or_default();
        assert!(status.ends_with(crate::messages::ON_PHONE), "{status}");
        // Even where Telegram would let it be saved.
        press(&mut app, KeyCode::Char('y'), KeyModifiers::NONE);
        assert!(app.copying.is_empty());
        assert_eq!(
            app.status.as_deref(),
            Some("Nothing to copy in this message")
        );
    }

    #[test]
    fn a_secret_chat_taken_although_the_setting_says_not_is_told() {
        use tdlib_rs::enums::NotificationGroupType;
        use tdlib_rs::types::Notification;
        let mut app = test_app("secret-taken");
        let chat_id = chat_with_chardy(&mut app, Some(crate::secret::SecretState::Ready));
        let update = |app: &mut App| {
            app.on_notifications(UpdateNotificationGroup {
                notification_group_id: 1,
                r#type: NotificationGroupType::SecretChat,
                chat_id,
                notification_settings_chat_id: chat_id,
                notification_sound_id: 0,
                total_count: 1,
                added_notifications: vec![Notification {
                    id: 1,
                    date: unix_now(),
                    is_silent: false,
                    r#type: NotificationType::NewSecretChat,
                }],
                removed_notification_ids: Vec::new(),
            });
        };
        update(&mut app);
        let status = app.status.take().unwrap_or_default();
        assert!(status.contains("Chardy started a secret chat"), "{status}");
        assert!(status.contains(":leave ends it"), "{status}");

        app.settings.accept_secret_chats = true;
        update(&mut app);
        assert_eq!(app.status, None, "taken as asked");
    }

    #[test]
    fn logging_out_says_secret_chats_go_too() {
        let mut app = test_app("secret-logout");
        command(&mut app, "logout");
        let lines = app.confirm.take().expect("asks").lines.join(" ");
        assert!(!lines.contains("secret chats"), "{lines}");
        chat_with_chardy(&mut app, Some(crate::secret::SecretState::Ready));
        command(&mut app, "logout");
        let lines = app.confirm.take().expect("asks").lines.join(" ");
        assert!(lines.contains("Your secret chats go too"), "{lines}");
    }
}

use std::collections::{BTreeMap, HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::process::{self, Stdio};
use std::time::{Duration, SystemTime};

use anyhow::Result;
use crossterm::event::{Event, EventStream, KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
use futures::StreamExt;
use ratatui::DefaultTerminal;
use ratatui::style::Style;
use ratatui_textarea::{DataCursor, TextArea};
use tdlib_rs::enums::{
    AuthenticationCodeType, AuthorizationState, ChatMemberStatus, ConnectionState, MessageSender,
    MessageTopic, NotificationType, OptionValue, ProxyType, Update, UserType,
};
use tdlib_rs::types::{Message, Proxy, UpdateNotificationGroup};
use tokio::sync::mpsc::UnboundedReceiver;
use tokio::time::{Instant, sleep_until};

use crate::attach::{self, Attachment, Dropped};
use crate::buttons::{ButtonMenu, Press};
use crate::chats::{Badge, Chats, List, Peer, Presence};
use crate::clipboard::{Clipboard, ClipboardEvent, Copied, Decoded, Paste, Pasted};
use crate::complete::{self, Commands, Completion, Kind, Suggestion};
use crate::config::{self, ApiKeys};
use crate::draft::{self, Draft};
use crate::images::{ImageEvent, Images};
use crate::info::ChatInfo;
use crate::messages::{
    Editable, Editing, Link, MediaFile, OpenChat, Replied, SendState, Sender, link_host, one_line,
    web_url,
};
use crate::notify::{self, Note, Notifications, Notifier};
use crate::picker::{self, ChatPicker, Choice, Purpose};
use crate::pins::{PinMenu, Pinned, PinnedMenu, Place};
use crate::poll::{Vote, VoteMenu};
use crate::proxy;
use crate::reactions::{self, ReactMenu, ReactionKind};
use crate::search::{self, MessageSearch, Who};
use crate::secret::{KeyView, Secret, SecretState, TimerMenu};
use crate::settings::{self, Settings, Side};
use crate::stickers::{self, Source, StickerPanel};
use crate::text;
use crate::tg::{Deletable, EditText, Found, Invite, Mentions, Missed, Page, Tagged, Tg, TgEvent};
use crate::theme::{Colors, Corners, Themes};
use crate::topics::{self, Forum};
use crate::ui;
use crate::viewer::{self, PhotoView};
use crate::voice::{Happened, Player, VoiceEvent};

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

/// A proxy change asked of TDLib.
enum ProxyChange {
    /// Starting with the proxy set in `TG_PROXY` or settings.toml.
    Start,
    /// From `:proxy` or a link: the link to save once TDLib takes it, or
    /// none for connecting directly.
    Set(Option<String>),
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
    /// The proxy set (`from`: `TG_PROXY` or settings.toml) can't be used,
    /// for this reason. TDLib isn't started, so nothing connects without
    /// it, and only quitting is left.
    BadProxy {
        from: &'static str,
        why: String,
    },
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
                | LoginStep::BadProxy { .. }
        )
    }
}

/// Something in a message that Enter opens or `y` copies.
pub enum Target {
    File(MediaFile),
    /// Message `message_id`'s voice, which tuigram plays itself; only ever
    /// opened. It names its message: the cursor may have moved since.
    Voice {
        message_id: i64,
        file: MediaFile,
    },
    /// Message `message_id`'s photo, which opens in the viewer.
    Photo {
        message_id: i64,
        file: MediaFile,
    },
    Link(Link),
    /// The whole text or caption; only copied.
    Text(String),
}

impl Target {
    pub fn label(&self) -> &str {
        match self {
            Target::File(file) | Target::Voice { file, .. } | Target::Photo { file, .. } => {
                &file.label
            }
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
    /// For a link, the site it goes to; for a proxy, its server.
    pub site: Option<Site>,
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

/// Where a [`Confirm`] would connect, on a line of its own after its first.
/// A site's name is at the end of its host, so a long one is cut from the
/// left: a long host can only hide the part a sender made up.
pub struct Site {
    /// What it is, in front: "It goes to:".
    pub label: &'static str,
    pub host: String,
}

impl Site {
    /// The site a link goes to.
    pub fn link(host: String) -> Self {
        Self {
            label: "It goes to:",
            host,
        }
    }

    /// A proxy's server, `host:port`.
    pub fn server(address: String) -> Self {
        Self {
            label: "Server:",
            host: address,
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
    /// Connect through the proxy this link gives, from now on.
    UseProxy(String),
    /// Stop using the saved proxy, and connect directly from now on.
    NoProxy,
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
            Confirmed::UseProxy(_) => "use it",
            Confirmed::NoProxy => "connect directly",
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

/// The file Enter may open from `msg`. Media seen only while open (a voice
/// message whose timer starts once it's played) isn't handed to another
/// app, which would keep it; a voice message tuigram plays itself is fine.
fn openable_file(msg: &crate::messages::Msg) -> Option<&MediaFile> {
    msg.file
        .as_ref()
        .filter(|_| !opens_once(msg) || msg.voice.is_some())
}

/// Opening message `id`'s `file`: its voice plays here, its photo shows in
/// the viewer, anything else opens in its app.
fn file_target(id: i64, msg: &crate::messages::Msg, file: MediaFile) -> Target {
    if msg.voice.as_ref().is_some_and(|v| v.file_id == file.id) {
        Target::Voice {
            message_id: id,
            file,
        }
    } else if msg.photo.as_ref().is_some_and(|p| p.file_id == file.id) {
        Target::Photo {
            message_id: id,
            file,
        }
    } else {
        Target::File(file)
    }
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
    /// The "round corners" row.
    pub const CORNERS: usize = Self::BLOCK_GAPS + 1;
    /// The "round pills, with a Nerd Font" row.
    pub const PILLS: usize = Self::CORNERS + 1;
    /// The "Normal mode after sending" row.
    pub const AFTER_SEND: usize = Self::PILLS + 1;
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

/// A place to come back to: a chat (and a forum's topic), and the message
/// the cursor was on (`None` for the newest, following new ones).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Jump {
    pub chat_id: i64,
    pub topic: Option<i32>,
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
    /// `:proxy`: the link of the proxy to connect through.
    Proxy,
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
    Info,
    Key,
    Leave,
    Logout,
    Proxy,
    Secret,
    Timer,
}

impl Command {
    pub const ALL: [Command; 7] = [
        Command::Info,
        Command::Key,
        Command::Leave,
        Command::Logout,
        Command::Proxy,
        Command::Secret,
        Command::Timer,
    ];

    pub fn name(self) -> &'static str {
        match self {
            Command::Info => "info",
            Command::Key => "key",
            Command::Leave => "leave",
            Command::Logout => "logout",
            Command::Proxy => "proxy",
            Command::Secret => "secret",
            Command::Timer => "timer",
        }
    }

    pub fn about(self) -> &'static str {
        match self {
            Command::Info => "Show what this chat is, and who's in it (also I)",
            Command::Key => "Show a secret chat's key, to compare with the other person's",
            Command::Leave => "Leave this group or channel, or end a secret chat (asks first)",
            Command::Logout => "Log out of Telegram on this computer (asks first)",
            Command::Proxy => "Connect through a proxy where Telegram is blocked, or directly",
            Command::Secret => "Start a secret chat with this person (asks first)",
            Command::Timer => "Set how long messages in a secret chat last once seen",
        }
    }

    pub fn parse(text: &str) -> Option<Command> {
        Command::ALL.into_iter().find(|c| c.name() == text)
    }
}

/// Ctrl-r's resize mode: the pane whose edge `h` and `l` move, with its
/// width before, which Esc puts back.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Resizing {
    /// The chat list, in percent of the window.
    List(u16),
    /// A forum's topics pane, in columns.
    Topics(u16),
}

/// Where keys go. `Input` is Insert mode; the others are Normal mode.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Focus {
    Chats,
    /// A forum's topics, in the pane between the chats and the messages.
    Topics,
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
    /// The forum open, whose topics show in a pane of their own; `open`
    /// is then one of its topics, if any.
    pub forum: Option<Forum>,
    /// The message being written. Cleared when switching chats.
    pub composer: TextArea<'static>,
    pub images: Images,
    /// Plays voice messages.
    pub player: Player,
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
    /// Enter on a photo: it, as big as the window allows.
    pub photo_view: Option<PhotoView>,
    /// `I`: what a chat is, and who's in it.
    pub chat_info: Option<ChatInfo>,
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
    /// In resize mode (Ctrl-r): which pane `h` and `l` resize, and its
    /// width before, which Esc puts back.
    pub resizing: Option<Resizing>,
    pub confirm: Option<Confirm>,
    pub settings: Settings,
    settings_path: PathBuf,
    /// Every theme, read again whenever `?` opens, so changes to a file show.
    pub themes: Themes,
    /// The colors of the theme in use.
    pub colors: Colors,
    /// Corners are drawn round (`Settings.corners`, `Auto` decided).
    pub rounded: bool,
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
    /// Pages of forum topics asked for, counted, so an answer from an
    /// earlier visit to a forum can't pass for the visit now.
    topics_asked: u32,
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
    /// What TDLib's connection to Telegram is doing, while it isn't
    /// working, e.g. "Connecting…"; shown in the status bar.
    pub connection: Option<&'static str>,
    /// Proxy changes TDLib hasn't answered yet, by their number.
    proxy_changes: BTreeMap<u64, ProxyChange>,
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
    /// The chat last told you're typing, and the topic in a forum, and
    /// when. `None` once it was told you stopped.
    typing: Option<((i64, Option<i32>), Instant)>,
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
    /// The photo the last frame showed in the viewer, by file id, and the
    /// size it was zoomed to.
    shown_in_viewer: Option<(i32, Option<usize>)>,
    /// TDLib is logging out (`:logout`, the session ended elsewhere, or
    /// leaving a QR login), and a new client takes over once it has closed.
    relogin: bool,
    exit: bool,
}

impl App {
    pub fn new(
        tg: Tg,
        images: Images,
        player: Player,
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
        let rounded = settings.corners.rounded(|name| std::env::var(name).ok());
        let mut app = Self {
            tg,
            screen: login_screen(LoginStep::Connecting),
            focus: Focus::Chats,
            chats,
            users: HashMap::new(),
            selected: None,
            open: None,
            forum: None,
            composer: new_composer(),
            images,
            player,
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
            photo_view: None,
            chat_info: None,
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
            rounded,
            premium: false,
            env_keys,
            keys_source: None,
            built_in_rejected: false,
            settings_menu: None,
            prompt: None,
            topics_asked: 0,
            loading_lists: HashSet::new(),
            loaded_lists: HashSet::new(),
            wanted_lists: HashSet::new(),
            jumps: Jumps::default(),
            // Only development builds read `.env`; someone expecting it to
            // pick a separate session should know this one didn't.
            status: config::dotenv_ignored()
                .then(|| "./.env is ignored: only development builds read it".into()),
            connection: None,
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
            shown_in_viewer: None,
            relogin: false,
            proxy_changes: BTreeMap::new(),
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
        mut voice_events: UnboundedReceiver<VoiceEvent>,
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
            // them is drawn over, which tmux may skip for blank ones: the
            // viewer's photo once it closes, changes size or makes way for
            // another, and the bubbles' under its blank edges once it opens.
            let viewing = self.open.as_ref().and_then(|o| o.viewing);
            let in_viewer = self.photo_view.as_ref().map(|v| (v.photo.file_id, v.zoom));
            if ((self.shown_viewing.is_some() && viewing != self.shown_viewing)
                || in_viewer != self.shown_in_viewer)
                && self.images.paints_over()
            {
                let _ = terminal.clear();
            }
            self.shown_viewing = viewing;
            self.shown_in_viewer = in_viewer;
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
            // typing in the `s` picker pauses, to count down messages that
            // self-destruct, and to follow a voice message playing.
            let countdown = self
                .open
                .as_ref()
                .and_then(|o| o.next_tick(SystemTime::now()))
                .map(|left| Instant::now() + left);
            let playing = self.player.next_tick().map(|soon| Instant::now() + soon);
            // And to cover what's shown only while open, once you're away.
            let away = self
                .open
                .as_ref()
                .filter(|o| o.viewing.is_some())
                .map(|_| self.last_input + self.away_after());
            let wake = [
                countdown,
                playing,
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
                Some(event) = voice_events.recv() => self.on_voice(event),
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
            // Files attached and Insert mode under the photo, out of sight,
            // would go out with the next Enter or two.
            Event::Paste(_) if self.photo_view.is_some() => {
                self.status = Some("Close the photo to paste".into());
            }
            // And under the info, where Esc would leave Insert mode, ready
            // to send them.
            Event::Paste(_) if self.chat_info.is_some() => {
                self.status = Some("Close the info to paste".into());
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
            TgEvent::ProxyApplied { number, result } => self.on_proxy_applied(number, result),
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
                topic,
                page,
                messages,
            } => self.on_history((chat_id, topic), page, messages),
            TgEvent::Found {
                chat_id,
                topic,
                query,
                found,
            } => self.on_found((chat_id, topic), &query, found),
            TgEvent::Topics {
                chat_id,
                request,
                page,
            } => {
                if let Some(forum) = self.forum.as_mut().filter(|f| f.chat_id == chat_id) {
                    forum.add_page(request, page.as_ref());
                    // A forum's topics start with its first page; the cursor
                    // may already be near the end of a short one.
                    self.ask_topics();
                    self.topic_known();
                }
            }
            TgEvent::Topic {
                chat_id,
                topic_id,
                topic,
            } => {
                if let Some(forum) = self.forum.as_mut().filter(|f| f.chat_id == chat_id) {
                    match topic {
                        Some(topic) => forum.upsert(&topic),
                        None => forum.not_found(topic_id),
                    }
                    self.topic_known();
                }
            }
            TgEvent::Replied {
                chat_id,
                message_id,
                replied,
            } => {
                if let Some(open) = self.open.as_mut().filter(|o| o.chat_id == chat_id) {
                    open.set_replied(message_id, replied.map(|m| *m));
                }
            }
            TgEvent::DraftReply {
                chat_id,
                topic,
                message_id,
                message,
            } => self.on_draft_reply((chat_id, topic), message_id, message.map(|m| *m)),
            TgEvent::Deletable {
                chat_id,
                message_id,
                deletable,
            } => self.on_deletable(chat_id, message_id, deletable),
            TgEvent::Pinned {
                chat_id,
                topic,
                request,
                messages,
            } => self.on_pinned((chat_id, topic), request, messages),
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
                if self.player.waits_for(file_id) {
                    match &path {
                        // Not to an empty room: playing it tells the sender,
                        // and one that plays once would be gone unheard.
                        Some(_) if !self.present() => {
                            self.player.stop();
                            self.status = Some("Voice message downloaded: Enter plays it".into());
                        }
                        Some(path) => self.player.start(PathBuf::from(path)),
                        None => {
                            self.player.stop();
                            self.status = Some("Download failed".into());
                        }
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
                    (Ok(spot), _) => {
                        self.open_chat(spot.chat_id);
                        if let Some(topic) = spot.topic
                            && self
                                .forum
                                .as_ref()
                                .is_some_and(|f| f.chat_id == spot.chat_id)
                        {
                            self.open_topic(topic);
                        }
                        if let Some(id) = spot.message_id {
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
                // Forwards into a forum go to its General topic.
                let to = match self.chats.is_forum(chat_id) {
                    true => format!("to {title} › General"),
                    false => format!("to {title}"),
                };
                self.show_toast("Forwarded", &to);
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
                topic,
                label,
                text,
                alert,
                url,
            } => self.on_bot_answer((chat_id, topic), label, &text, alert, &url),
            TgEvent::Invite {
                request,
                link,
                invite,
            } => {
                if self.finding.as_ref().is_some_and(|f| f.request == request) && !self.busy() {
                    self.confirm_invite(link, request, invite);
                }
            }
            TgEvent::Mentions {
                chat_id,
                topic,
                ask,
                ids,
            } => self.on_mentions((chat_id, topic), ask, ids),
            TgEvent::ChatInfo { chat_id, about } => {
                if let Some(info) = self
                    .chat_info
                    .as_mut()
                    .filter(|i| i.chat_id == chat_id && i.about.is_none())
                {
                    match about {
                        Some(about) => info.set_about(*about),
                        // Why is in the status bar.
                        None => info.failed = true,
                    }
                    self.ask_members();
                }
            }
            TgEvent::ChatMembers {
                chat_id,
                offset,
                members,
            } => {
                if let Some(info) = self.chat_info.as_mut().filter(|i| i.chat_id == chat_id) {
                    info.add_page(offset, members);
                    self.ask_members();
                }
            }
        }
    }

    fn on_history(
        &mut self,
        place: (i64, Option<i32>),
        page: Page,
        messages: Option<Vec<tdlib_rs::types::Message>>,
    ) {
        // Ignore pages for a chat or topic that was closed, or a request
        // that was replaced (e.g. by jumping elsewhere), while it was in
        // flight.
        let Some(open) = self
            .open
            .as_mut()
            .filter(|o| o.place() == place && o.loading == Some(page))
        else {
            return;
        };
        open.loading = None;
        let Some(messages) = messages else {
            return;
        };
        // A topic's history has only its own messages: going to one of
        // another topic (`gd` to what a reply answers) would land on a
        // message near it instead, so the view stays.
        if let Page::Around(target) = page
            && open.topic.is_some()
            && open.unread_after.is_none()
            && !messages.iter().any(|m| m.id == target)
        {
            self.status = Some("That message isn't in this topic".into());
            return;
        }
        open.add_page(
            page,
            messages.into_iter().map(|m| (m.id, m.into())).collect(),
        );
        let newest = match open.topic {
            Some(topic) => self
                .forum
                .as_ref()
                .and_then(|f| f.get(topic))
                .map_or(0, |t| t.newest()),
            None => self.chats.last_message(open.chat_id),
        };
        open.reached(newest);
        open.go_to_unread();
        if open.messages.len() < MIN_LOADED {
            self.load_older_messages();
        }
    }

    fn on_found(&mut self, place: (i64, Option<i32>), query: &str, found: Option<Found>) {
        // Ignore results for a search that was replaced or ended meanwhile.
        let Some(open) = self.open.as_mut().filter(|o| o.place() == place) else {
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
            Update::ConnectionState(u) => self.connection = connection_words(&u.state),
            Update::ChatDraftMessage(u) => {
                self.chats
                    .set_draft(u.chat_id, u.draft_message.as_ref(), &u.positions)
            }
            Update::ChatIsMarkedAsUnread(u) => self
                .chats
                .set_marked_unread(u.chat_id, u.is_marked_as_unread),
            Update::ChatNotificationSettings(u) => self
                .chats
                .set_notifications(u.chat_id, u.notification_settings),
            Update::ScopeNotificationSettings(u) => self
                .chats
                .set_default_mute(&u.scope, u.notification_settings.mute_for),
            Update::ChatAction(u) => {
                let topic = match u.topic_id {
                    Some(MessageTopic::Forum(t)) => Some(t.forum_topic_id),
                    _ => None,
                };
                self.chats
                    .set_action(u.chat_id, topic, &u.sender_id, &u.action);
            }
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
            Update::ChatUnreadMentionCount(u) => {
                self.chats.set_mentions(u.chat_id, u.unread_mention_count);
                // Read all at once, elsewhere: TDLib may not say so of each.
                if u.unread_mention_count == 0
                    && let Some(open) = self.open.as_mut().filter(|o| o.chat_id == u.chat_id)
                {
                    open.messages.values_mut().for_each(|m| m.mention = false);
                }
            }
            Update::MessageMentionRead(u) => {
                self.chats.set_mentions(u.chat_id, u.unread_mention_count);
                if let Some(open) = self.open.as_mut().filter(|o| o.chat_id == u.chat_id) {
                    open.set_mention_read(u.message_id);
                }
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
                    open.set_opened(u.message_id);
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
                self.chats.set_forum(group.id, group.is_forum);
            }
            Update::NewMessage(u) => {
                // In a forum, its topic moves up, and the message shows
                // only in it.
                let mut topic = None;
                let mut ask = None;
                if let Some(forum) = self
                    .forum
                    .as_mut()
                    .filter(|f| f.chat_id == u.message.chat_id)
                {
                    let arrived = forum.arrived(&u.message);
                    topic = Some(arrived.topic);
                    ask = forum.add_message(&arrived);
                }
                if let Some(id) = ask {
                    self.ask_topic(id);
                }
                // While older messages are shown, new ones load with the rest.
                if let Some(open) = self.open.as_mut().filter(|o| {
                    o.chat_id == u.message.chat_id
                        && o.at_newest
                        && (o.topic.is_none() || o.topic == topic)
                }) {
                    open.insert(u.message);
                }
            }
            Update::ForumTopicInfo(u) => {
                if let Some(forum) = self.forum.as_mut().filter(|f| f.chat_id == u.info.chat_id) {
                    forum.set_info(&u.info);
                }
            }
            Update::ForumTopic(u) => {
                let ask = self
                    .forum
                    .as_mut()
                    .filter(|f| f.chat_id == u.chat_id)
                    .and_then(|f| f.set_state(&u));
                if let Some(id) = ask {
                    self.ask_topic(id);
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
                    // Edited to another photo, or to no photo at all.
                    let msg = open.messages.get(&u.message_id);
                    if self
                        .photo_view
                        .as_ref()
                        .is_some_and(|v| v.lost(u.chat_id, u.message_id, msg))
                    {
                        self.photo_view = None;
                    }
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
                if u.message_ids
                    .iter()
                    .any(|&id| self.player.is_on(u.chat_id, id))
                {
                    self.player.stop();
                }
                // A topic whose newest message went shows the one before.
                let stale = self
                    .forum
                    .as_ref()
                    .filter(|f| f.chat_id == u.chat_id)
                    .map(|f| f.deleted(&u.message_ids))
                    .unwrap_or_default();
                for id in stale {
                    self.ask_topic(id);
                }
                if self
                    .photo_view
                    .as_ref()
                    .is_some_and(|v| u.message_ids.iter().any(|&id| v.lost(u.chat_id, id, None)))
                {
                    self.photo_view = None;
                }
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
        // TDLib got no parameters past a proxy that can't be used, so
        // nothing should come but its closing; if anything does, the
        // screen still says why tuigram doesn't connect.
        if let Screen::Login(login) = &self.screen
            && matches!(login.step, LoginStep::BadProxy { .. })
            && !matches!(
                state,
                AuthorizationState::Closing | AuthorizationState::Closed
            )
        {
            return;
        }
        let step = match state {
            AuthorizationState::WaitTdlibParameters => match self.api_keys() {
                Some((source, keys)) => {
                    self.keys_source = Some(source);
                    self.start_tdlib(keys);
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
            Screen::Main if self.photo_view.is_some() => self.on_viewer_key(key),
            Screen::Main if self.settings_menu.is_some() => self.on_settings_key(key, ctrl),
            Screen::Main if self.delete_menu.is_some() => self.on_delete_key(key),
            Screen::Main if self.react_menu.is_some() => self.on_react_key(key, ctrl),
            Screen::Main if self.vote_menu.is_some() => self.on_vote_key(key),
            Screen::Main if self.button_menu.is_some() => self.on_button_key(key),
            Screen::Main if self.pin_menu.is_some() => self.on_pin_key(key),
            Screen::Main if self.pinned_menu.is_some() => self.on_pinned_key(key),
            Screen::Main if self.timer_menu.is_some() => self.on_timer_key(key),
            Screen::Main if self.chat_info.is_some() => self.on_info_key(key, ctrl),
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
                    self.tg.close(None);
                    self.screen = login_screen(LoginStep::Connecting);
                } else {
                    self.keys_source = Some(KeySource::Saved);
                    self.screen = login_screen(LoginStep::Connecting);
                    self.start_tdlib(keys);
                }
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
            | LoginStep::Unsupported(_)
            | LoginStep::BadProxy { .. } => {}
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
                Focus::Topics => self.move_topic_cursor(delta),
                Focus::Messages => self.move_message_cursor(delta),
                Focus::Input => {}
            }
            return;
        }
        match (self.focus, key.code) {
            // Before `r`, which replies.
            // In a forum's topics, the topics pane; else the chat list.
            (Focus::Topics, KeyCode::Char('r')) if ctrl && self.forum.is_some() => {
                self.resizing = Some(Resizing::Topics(self.settings.topics_width));
            }
            (_, KeyCode::Char('r')) if ctrl => {
                self.resizing = Some(Resizing::List(self.settings.chat_list_width));
            }
            (_, KeyCode::Char('g')) => self.pending_g = true,
            // From the chats or a forum's topics: opens the one under the
            // cursor first, rather than mute it (`m`).
            (Focus::Chats | Focus::Topics, KeyCode::Char('m')) if pending_g => {
                match self.focus {
                    Focus::Chats => self.open_selected_chat(),
                    _ => self.open_selected_topic(),
                }
                if self.focus == Focus::Messages {
                    self.go_to_mention(true);
                }
            }
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
            (Focus::Chats, KeyCode::Char('a')) if !ctrl => self.toggle_archive(),
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
                    open.draft_reply = None;
                }
            }
            (Focus::Messages, KeyCode::Char('r')) => self.reply_to_selected(),
            (Focus::Messages, KeyCode::Char('e')) => self.edit_selected(),
            (Focus::Messages, KeyCode::Char('y')) => self.copy_selected(),
            (Focus::Messages, KeyCode::Char('a')) => self.open_prompt(PromptKind::Attach),
            (Focus::Messages, KeyCode::Char('p')) if pending_g => self.open_pinned_menu(),
            (Focus::Messages, KeyCode::Char('u')) if pending_g => self.go_to_unread(),
            (Focus::Messages, KeyCode::Char('m')) if pending_g => self.go_to_mention(true),
            (Focus::Messages, KeyCode::Char('M')) if pending_g => self.go_to_mention(false),
            (_, KeyCode::Char('I')) => self.open_chat_info(),
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
                // A forum: the topic under the cursor there, as `i` in the
                // topics, not whichever was open.
                if self.focus == Focus::Topics {
                    self.open_selected_topic();
                }
                self.start_writing();
            }
            (Focus::Messages, KeyCode::Char('i')) => self.start_writing(),
            (Focus::Topics, KeyCode::Enter) => self.open_selected_topic(),
            (Focus::Topics, KeyCode::Char(c)) if c == to_chat => self.open_selected_topic(),
            (Focus::Topics, KeyCode::Char('i')) => {
                self.open_selected_topic();
                self.start_writing();
            }
            (Focus::Topics, KeyCode::Esc) => self.focus = Focus::Chats,
            (Focus::Topics, KeyCode::Char(c)) if c == to_list => self.focus = Focus::Chats,
            (Focus::Messages, KeyCode::Enter) => self.open_selected_message(),
            (Focus::Messages, KeyCode::Esc) => self.focus = self.left_of_messages(),
            (Focus::Messages, KeyCode::Char(c)) if c == to_list => {
                self.focus = self.left_of_messages();
            }
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
        // Your other devices have it too, should you go on there.
        self.keep_draft();
    }

    /// What's written in the open chat: the composer's text, or during an
    /// edit what it held before, and the message it answers. Only spaces
    /// is nothing.
    fn written(&self) -> Option<(String, Option<i64>)> {
        let open = self.open.as_ref()?;
        let (text, reply) = match &open.editing {
            Some(editing) => (editing.draft.clone(), editing.reply.as_ref().map(|r| r.id)),
            None => (
                self.composer.lines().join("\n"),
                open.reply.as_ref().map(|r| r.id).or(open.draft_reply),
            ),
        };
        let text = if text.trim().is_empty() {
            String::new()
        } else {
            text
        };
        Some((text, reply))
    }

    /// Keeps what's written in the open chat as its draft, here at once,
    /// if it changed since the composer was filled or last kept. Returns
    /// what to tell Telegram.
    fn draft_to_keep(&mut self) -> Option<draft::Keep> {
        let written = self.written()?;
        let open = self.open.as_mut()?;
        if open.draft == written {
            return None;
        }
        let draft = Draft::new(&written.0, written.1);
        // A reply with nothing written yet isn't a draft, and there was none.
        let had_none = open.draft.0.is_empty();
        open.draft = written;
        if draft.is_none() && had_none {
            return None;
        }
        let (chat_id, topic) = open.place();
        match topic {
            Some(topic) => {
                if let Some(forum) = self.forum.as_mut().filter(|f| f.chat_id == chat_id) {
                    forum.keep_draft(topic, draft.clone());
                }
            }
            None => self.chats.keep_draft(chat_id, draft.clone()),
        }
        Some(draft::Keep {
            chat_id,
            topic,
            draft,
        })
    }

    /// A secret chat that's ending keeps no draft: not in the list, and not
    /// in the composer, which leaving would keep again.
    fn forget_draft(&mut self, chat_id: i64) {
        self.chats.keep_draft(chat_id, None);
        if let Some(open) = self.open.as_mut().filter(|o| o.chat_id == chat_id) {
            open.editing = None;
            open.reply = None;
            open.draft_reply = None;
            open.draft = Default::default();
            self.composer = new_composer();
        }
    }

    /// Keeps what's written in the open chat as its draft on Telegram.
    fn keep_draft(&mut self) {
        if let Some(keep) = self.draft_to_keep() {
            self.tg.set_draft(keep);
        }
    }

    /// The message the draft answers, fetched to show over the composer;
    /// `None` if it's gone, and the draft then answers nothing.
    fn on_draft_reply(
        &mut self,
        place: (i64, Option<i32>),
        message_id: i64,
        message: Option<Message>,
    ) {
        let Some(open) = self
            .open
            .as_mut()
            .filter(|o| o.place() == place && o.draft_reply == Some(message_id))
        else {
            return;
        };
        open.draft_reply = None;
        if let Some(message) = message
            && open.reply.is_none()
            && open.editing.is_none()
        {
            open.reply = Some(Replied::new(
                message_id,
                &crate::messages::Msg::from(message),
            ));
        }
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
        let place = self.open.as_ref().map(|o| (o.chat_id, o.topic));
        if typing && let Some(place) = place {
            let told = self
                .typing
                .is_some_and(|(at, when)| at == place && when.elapsed() < TYPING_EVERY);
            if !told {
                self.tg.send_typing(place.0, place.1, true);
                self.typing = Some((place, Instant::now()));
            }
        } else if let Some(((chat_id, topic), _)) = self.typing.take() {
            self.tg.send_typing(chat_id, topic, false);
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
        let reply_to = open.take_reply();
        if open.attachments.is_empty() {
            self.tg
                .send_text(open.chat_id, open.topic, text, reply_to, secret);
            // Sending a text clears the draft.
            open.draft = (String::new(), None);
        } else {
            let as_files = open.as_files;
            let groups = attach::albums(&open.attachments, as_files)
                .into_iter()
                .map(|album| album.iter().map(|a| a.upload(as_files)).collect())
                .collect();
            self.tg
                .send_files(open.chat_id, open.topic, groups, text, reply_to);
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
            PromptKind::Proxy if !submit => {}
            PromptKind::Proxy => self.ask_to_change_proxy(&query),
            PromptKind::Command if !submit || query.is_empty() => {}
            PromptKind::Command => match Command::parse(&query) {
                Some(Command::Info) => self.open_chat_info(),
                Some(Command::Key) => self.show_key(),
                Some(Command::Leave) => self.ask_to_leave(),
                Some(Command::Logout) => self.ask_to_log_out(),
                Some(Command::Proxy) => self.open_proxy_prompt(),
                Some(Command::Secret) => self.ask_secret_chat(),
                Some(Command::Timer) => self.open_timer_menu(),
                None => self.status = Some(format!("Not a command: {query}")),
            },
        }
    }

    /// Sends a read receipt for the newest incoming message once you can
    /// see it: the chat pane and the terminal window have focus, and the view
    /// is on the newest message (see [`App::watching`]). Viewing it marks the
    /// whole chat as read. A message that mentions you, or answers yours,
    /// is read once the cursor is on it with nothing over the chat, as
    /// Telegram's apps read it once it's on screen, and the chat up to it;
    /// not in a secret chat, where that would start the timers of the
    /// unread messages before it, unseen.
    fn mark_seen(&mut self) {
        let Some(chat_id) = self.open.as_ref().map(|o| o.chat_id) else {
            return;
        };
        let watching = self.watching(chat_id);
        let mention = self.mention_seen();
        let Some(open) = self.open.as_mut() else {
            return;
        };
        let newest = open.messages.iter().rev().find(|(_, m)| !m.outgoing);
        if watching
            && let Some((&id, _)) = newest
            && id > open.seen
        {
            open.seen = id;
            // Viewing it reads its mention too.
            open.set_mention_read(id);
            self.tg.view_messages(open.chat_id, open.topic, vec![id]);
            // Read, their self-destruct timers start.
            open.start_timers(false, id, SystemTime::now());
        }
        if let Some(id) = mention {
            open.set_mention_read(id);
            open.seen = open.seen.max(id);
            self.tg.view_messages(open.chat_id, open.topic, vec![id]);
        }
    }

    /// The message under the cursor, when it mentions you unseen and is in
    /// front of you: the keys are in the chat, with nothing over it, and
    /// it's not a secret chat (see [`App::mark_seen`]).
    fn mention_seen(&self) -> Option<i64> {
        let open = self.open.as_ref()?;
        if !self.looking_at(open.chat_id)
            || self.busy()
            || self.focus != Focus::Messages
            || self.chats.is_secret(open.chat_id)
        {
            return None;
        }
        let id = open.cursor_id()?;
        let msg = open.messages.get(&id)?;
        (msg.mention && !msg.unplayed && !msg.outgoing && msg.state == SendState::Sent)
            .then_some(id)
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
        self.looking_at(chat_id)
            && self
                .open
                .as_ref()
                .is_some_and(|o| o.at_newest && o.selected.is_none())
    }

    /// The chat's messages are in front of the user, wherever the cursor
    /// is in them: [`App::watching`], but for being on the newest.
    fn looking_at(&self, chat_id: i64) -> bool {
        matches!(self.screen, Screen::Main)
            && self.present()
            && matches!(self.focus, Focus::Messages | Focus::Input)
            && self.settings_menu.is_none()
            && self.photo_view.is_none()
            // As tall as the pane, over the newest messages.
            && self.chat_info.is_none()
            && self.open.as_ref().is_some_and(|o| {
                o.chat_id == chat_id
                    // Going to an older message: what arrives meanwhile
                    // isn't what's about to be on screen.
                    && !matches!(o.loading, Some(Page::Around(_)))
                    // A topic opened from a link opens at its first unread
                    // message once TDLib says where that is.
                    && o.topic.is_none_or(|t| {
                        self.forum.as_ref().is_some_and(|f| f.get(t).is_some())
                    })
            })
    }

    /// What's shown only while open is covered once you look away, or go
    /// away.
    fn cover_unseen(&mut self) {
        let looking = self.focus == Focus::Messages && self.present() && self.chat_info.is_none();
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
    /// In a forum, only the topic open is being read: `topic` is the one
    /// the message is in.
    fn sees(&self, chat_id: i64, topic: Option<i32>) -> bool {
        // The photo viewer hides every chat.
        if self.photo_view.is_some() {
            return false;
        }
        if self.focus_reported {
            self.terminal_focused
        } else {
            self.watching(chat_id)
                && self
                    .open
                    .as_ref()
                    .is_some_and(|o| o.topic.is_none() || o.topic == topic)
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
            let topic = match &notification.r#type {
                NotificationType::NewMessage(new) => self
                    .forum
                    .as_ref()
                    .filter(|f| f.chat_id == chat_id)
                    .map(|f| f.topic_of(&new.message)),
                _ => None,
            };
            if notification.date < self.notify_since || self.sees(chat_id, topic) {
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
        // "Alice joined the group", whatever the chat.
        if let Some(service) = crate::service::Service::of(&message.content) {
            let sender = Sender::from(&message.sender_id);
            let name = |id| {
                self.users
                    .get(&id)
                    .cloned()
                    .unwrap_or_else(|| "Someone".into())
            };
            let actor = match sender {
                Sender::User(id) => name(id),
                Sender::Chat(id) => self.chats.title(id).unwrap_or("Someone").to_string(),
            };
            let parts = service.sentence(sender, crate::service::Part::Name(actor), name);
            return crate::service::text(&parts);
        }
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
        self.forum = None;
        self.focus = Focus::Chats;
        self.composer = new_composer();
        self.images.forget_files();
        self.player.stop();
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
        self.photo_view = None;
        self.chat_info = None;
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
                    open.topic,
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
        self.tg
            .load_history(open.chat_id, open.topic, page, HISTORY_PAGE);
    }

    /// Back to following the newest message, reloading if an older part of
    /// the chat is shown.
    fn jump_to_newest(&mut self) {
        let Some(open) = self.open.as_mut() else {
            return;
        };
        open.unread_after = None;
        open.selected = None;
        // A page around an older message on its way would take the view
        // away again.
        if open.at_newest && !matches!(open.loading, Some(Page::Around(_))) {
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
            topic: o.topic,
            message_id: o.selected,
        })
    }

    /// Ctrl-o (`back`) or Ctrl-i: to the chat or message left before, or
    /// forward again to where Ctrl-o came from.
    fn jump(&mut self, back: bool) {
        let here = self.here();
        match self.jumps.go(back, here) {
            Some(to) => {
                let entered = self
                    .open
                    .as_ref()
                    .is_none_or(|o| o.place() != (to.chat_id, to.topic));
                if entered {
                    self.enter_chat(to.chat_id);
                    if let Some(topic) = to.topic {
                        self.enter_topic(topic);
                    }
                }
                // A chat that became a forum shows its topics.
                if self
                    .open
                    .as_ref()
                    .is_none_or(|o| o.place() != (to.chat_id, to.topic))
                {
                    return;
                }
                self.focus = Focus::Messages;
                match to.message_id {
                    Some(id) => self.jump_to_message(id),
                    // Just opened, it's at its first unread message, or
                    // the newest if there's none: what came in since you
                    // left isn't read unseen.
                    None if entered => {}
                    None => self.jump_to_newest(),
                }
            }
            None if back => self.status = Some("Nothing to go back to".into()),
            None => self.status = Some("Nothing to go forward to".into()),
        }
    }

    /// [`App::open_chat`], without Ctrl-o coming back to the chat before.
    fn enter_chat(&mut self, chat_id: i64) {
        self.close_chat_popups();
        if self.chats.in_list(chat_id, self.chats.shown()) && self.selected != Some(chat_id) {
            // The list's cursor goes to it, even if the filter hid it.
            if !self.chats.ids().contains(&chat_id) {
                self.chats.set_filter("");
            }
            self.selected = Some(chat_id);
        }
        if self.chats.is_forum(chat_id) {
            self.enter_forum(chat_id);
            return;
        }
        self.focus = Focus::Messages;
        // Even the chat open, marked meanwhile, is unmarked entering it
        // again. It's opened first, so it stays put among the unread.
        let marked = self.chats.get(chat_id).is_some_and(|c| c.marked_unread);
        if self.open.as_ref().is_some_and(|o| o.chat_id == chat_id) {
            if marked {
                self.chats.opened(chat_id);
                self.unmark_unread(chat_id);
            }
            return;
        }
        self.leave_chat();
        // TDLib only sends some updates (e.g. for channels) while a chat is open.
        self.tg.open_chat(chat_id);
        self.chats.opened(chat_id);
        if marked {
            self.unmark_unread(chat_id);
        }
        let mut open = OpenChat::new(chat_id);
        // With unread messages, it opens at the first of them, as in
        // Telegram, and nothing is read until you get to the newest. (In a
        // secret chat, reading them all at once would also start their
        // timers.)
        if self.chats.get(chat_id).is_some_and(|c| c.unread > 0) {
            open.open_at_unread(self.chats.read_inbox(chat_id));
        }
        self.show_messages(open);
    }

    /// Shows a forum's topics in a pane of their own, the keys there, to
    /// pick one to read.
    fn enter_forum(&mut self, chat_id: i64) {
        self.focus = Focus::Topics;
        let marked = self.chats.get(chat_id).is_some_and(|c| c.marked_unread);
        if self.forum.as_ref().is_some_and(|f| f.chat_id == chat_id) {
            if marked {
                self.chats.opened(chat_id);
                self.unmark_unread(chat_id);
            }
            return;
        }
        self.leave_chat();
        // Topics are only kept up to date while the forum is open.
        self.tg.open_chat(chat_id);
        self.chats.opened(chat_id);
        if marked {
            self.unmark_unread(chat_id);
        }
        self.composer = new_composer();
        self.forum = Some(Forum::new(chat_id));
        self.ask_topics();
    }

    /// Opens a topic of the forum shown, as [`App::open_chat`] opens a
    /// chat: Ctrl-o comes back to where the cursor was.
    fn open_topic(&mut self, topic_id: i32) {
        let Some(chat_id) = self.forum.as_ref().map(|f| f.chat_id) else {
            return;
        };
        if let Some(here) = self
            .here()
            .filter(|h| (h.chat_id, h.topic) != (chat_id, Some(topic_id)))
        {
            self.jumps.leave(here);
        }
        self.enter_topic(topic_id);
    }

    /// [`App::open_topic`], without Ctrl-o coming back.
    fn enter_topic(&mut self, topic_id: i32) {
        let Some(forum) = self.forum.as_mut() else {
            return;
        };
        forum.selected = Some(topic_id);
        let chat_id = forum.chat_id;
        // One opened from a link or Ctrl-o may not be loaded: its name is
        // asked for, rather than leaving it unnamed.
        if forum.get(topic_id).is_none() {
            self.ask_topic(topic_id);
        }
        self.focus = Focus::Messages;
        if self
            .open
            .as_ref()
            .is_some_and(|o| o.place() == (chat_id, Some(topic_id)))
        {
            return;
        }
        self.close_chat_popups();
        // The forum stays open in TDLib: it's the same chat.
        self.leave_messages();
        let mut open = OpenChat::new(chat_id);
        open.topic = Some(topic_id);
        // At its first unread message, as a chat opens. One not loaded yet
        // does once TDLib says how it is (`App::topic_known`).
        if let Some(read) = self
            .forum
            .as_ref()
            .and_then(|f| f.get(topic_id))
            .filter(|t| t.unread > 0)
            .map(|t| t.read_inbox())
        {
            open.open_at_unread(read);
        }
        self.show_messages(open);
    }

    /// The topic open was opened before TDLib said how it is (from a link,
    /// or Ctrl-o into a forum): now that it has, it goes to its first
    /// unread message, unless the cursor went somewhere already. Nothing
    /// was read meanwhile (`App::looking_at`).
    fn topic_known(&mut self) {
        let Some(open) = self.open.as_mut().filter(|o| {
            o.unread_line.is_none()
                && o.selected.is_none()
                && o.seen == 0
                && !matches!(o.loading, Some(Page::Around(_)))
        }) else {
            return;
        };
        let Some(read) = open
            .topic
            .and_then(|id| self.forum.as_ref()?.get(id))
            .filter(|t| t.unread > 0)
            .map(|t| t.read_inbox())
        else {
            return;
        };
        open.open_at_unread(read);
        match open.first_page() {
            Page::Latest => {}
            page => self.load_page(page),
        }
    }

    /// Enter on a topic in the pane.
    fn open_selected_topic(&mut self) {
        // The one selected may not be loaded yet: one opened from a link.
        let id = self
            .forum
            .as_ref()
            .and_then(|f| f.current().map(|t| t.id).or(f.selected));
        if let Some(id) = id {
            self.open_topic(id);
        }
    }

    /// Asks for the next page of the forum's topics, when one is wanted:
    /// the first, or more as the cursor nears the end.
    fn ask_topics(&mut self) {
        let request = self.topics_asked.wrapping_add(1);
        if let Some(forum) = self.forum.as_mut()
            && let Some(from) = forum.page_to_ask(request)
        {
            self.topics_asked = request;
            self.tg.forum_topics(forum.chat_id, request, from);
        }
    }

    /// Asks TDLib how one of the forum's topics is now, unless it was asked
    /// already.
    fn ask_topic(&mut self, topic_id: i32) {
        if let Some(forum) = self.forum.as_mut()
            && forum.ask(topic_id)
        {
            self.tg.forum_topic(forum.chat_id, topic_id);
        }
    }

    fn move_topic_cursor(&mut self, delta: isize) {
        if let Some(forum) = self.forum.as_mut() {
            forum.move_cursor(delta);
        }
        self.ask_topics();
    }

    /// Where `h` and Esc go from the messages: a forum topic's goes back to
    /// the topics.
    fn left_of_messages(&self) -> Focus {
        match (&self.open, &self.forum) {
            (Some(open), Some(_)) if open.topic.is_some() => Focus::Topics,
            _ => Focus::Chats,
        }
    }

    /// Popups about a message of the chat before are no use in another.
    fn close_chat_popups(&mut self) {
        // A lookup still on its way would open another chat over this one.
        self.finding = None;
        self.menu = None;
        self.delete_menu = None;
        self.react_menu = None;
        self.vote_menu = None;
        self.button_menu = None;
        self.pin_menu = None;
        self.pinned_menu = None;
        self.timer_menu = None;
        self.key_view = None;
        self.photo_view = None;
        self.chat_info = None;
    }

    /// Leaves the chat open, or the forum shown, telling TDLib.
    fn leave_chat(&mut self) {
        self.leave_messages();
        let open = self.open.take().map(|o| o.chat_id);
        let forum = self.forum.take().map(|f| f.chat_id);
        if let Some(chat_id) = open.or(forum) {
            self.tg.close_chat(chat_id);
        }
    }

    /// Before the messages shown give way to others: you stop typing in
    /// them, and a voice message in them stops playing.
    fn leave_messages(&mut self) {
        self.keep_draft();
        self.set_typing(false);
        // A paste on its way was for these messages.
        self.pasting = false;
        // A voice message plays in its own chat, where it shows playing.
        self.player.stop();
        if self.open.is_some() {
            self.images.clear();
        }
    }

    fn show_messages(&mut self, mut open: OpenChat) {
        let page = open.first_page();
        self.composer = new_composer();
        let draft = match open.topic {
            Some(topic) => self
                .forum
                .as_ref()
                .and_then(|f| f.get(topic))
                .and_then(|t| t.draft.as_ref()),
            None => self.chats.draft(open.chat_id),
        };
        if let Some(draft) = draft.cloned() {
            self.composer.insert_str(&draft.text);
            if let Some(id) = draft.reply_to {
                open.draft_reply = Some(id);
                self.tg.draft_reply(open.chat_id, open.topic, id);
            }
            open.draft = (draft.text, draft.reply_to);
        }
        self.open = Some(open);
        match page {
            Page::Latest => self.load_older_messages(),
            page => self.load_page(page),
        }
        self.ask_pinned();
    }

    /// Asks for a page of the open chat's history, which replaces any
    /// request on its way.
    fn load_page(&mut self, page: Page) {
        if let Some(open) = self.open.as_mut() {
            open.loading = Some(page);
            self.tg
                .load_history(open.chat_id, open.topic, page, HISTORY_PAGE);
        }
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
            self.tg
                .pinned_messages(open.chat_id, open.topic, open.pinned_asked);
        }
    }

    fn on_pinned(
        &mut self,
        place: (i64, Option<i32>),
        request: u32,
        messages: Option<Vec<Message>>,
    ) {
        // Only the last answer for the chat open: an older one may miss a
        // message pinned since.
        let Some(open) = self
            .open
            .as_mut()
            .filter(|o| o.place() == place && o.pinned_asked == request)
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
            SendState::Failed { .. } => self.status = Some("This message wasn't sent".into()),
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
    /// Telegram; pauses or plays on the voice message playing; votes in a
    /// poll; lists a bot's buttons; else opens its file or link (playing a
    /// voice message) right away, or shows a menu when there's more than one.
    fn open_selected_message(&mut self) {
        if self.resend_selected() {
            return;
        }
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
        if let Some(id) = open.cursor_id()
            && self.player.is_on(open.chat_id, id)
        {
            self.player.toggle();
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
                        openable_file(msg),
                        &msg.links,
                    ));
                }
                _ => self.status = Some("Wait until it's sent".into()),
            }
            return;
        }
        let Some((&id, msg)) = open
            .cursor_id()
            .and_then(|id| open.messages.get_key_value(&id))
        else {
            return;
        };
        let file = openable_file(msg).map(|file| file_target(id, msg, file.clone()));
        let mut targets: Vec<Target> = file.into_iter().collect();
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

    /// Enter on a message Telegram didn't take sends it again, with the
    /// rest of its album. Returns whether the cursor was on one.
    fn resend_selected(&mut self) -> bool {
        let Some(open) = self.open.as_ref() else {
            return false;
        };
        let Some((id, msg)) = open
            .cursor_id()
            .and_then(|id| open.messages.get_key_value(&id))
        else {
            return false;
        };
        let SendState::Failed { can_retry } = msg.state else {
            return false;
        };
        let (chat_id, ids) = (open.chat_id, open.retry_ids(*id));
        if !can_retry {
            self.status = Some("Telegram won't take this message again: `d` deletes it".into());
        } else if let Some(why) = self.cant_send(chat_id) {
            self.status = Some(why);
        } else {
            self.tg.resend(chat_id, ids);
        }
        true
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
        if matches!(msg.state, SendState::Failed { .. }) {
            self.status = Some("Can't reply to a message that wasn't sent".into());
            return;
        }
        open.reply = Some(Replied::new(id, msg));
        // The draft's reply, still being fetched, can't come back over it.
        open.draft_reply = None;
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
        // Nor under the photo viewer, where the composer is out of sight.
        if self.photo_view.is_some() {
            self.status = Some("The edit came while a photo was open: e edits again".into());
            return;
        }
        if self.chat_info.is_some() {
            self.status = Some("The edit came while the info was open: e edits again".into());
            return;
        }
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
            SendState::Failed { .. } => self.status = Some("This message wasn't sent".into()),
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
                let topic = self.open.as_ref().and_then(|o| o.topic);
                self.tg
                    .press_button(chat_id, topic, message_id, data, button.label);
            }
            Press::Open(link) => self.open_target(Target::Link(link)),
            Press::File(file) => {
                // By the menu's message, not the cursor's, which may have
                // moved. One that's no longer loaded isn't guessed at.
                let target = self
                    .open
                    .as_ref()
                    .and_then(|o| o.messages.get(&message_id))
                    .map(|msg| file_target(message_id, msg, file));
                match target {
                    Some(target) => self.open_target(target),
                    None => self.status = Some("That message isn't loaded any more".into()),
                }
            }
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
                let topic = self.open.as_ref().and_then(|o| o.topic);
                self.tg.send_plain(chat_id, topic, text, reply_to);
                self.jump_to_newest();
            }
            Press::Unsupported(_) => {}
        }
    }

    /// A bot answered a button: a note goes in the corner, an alert in a
    /// popup. A link it sends opens only while its chat is open and nothing
    /// else holds the keys, and asks first, since nothing said where it goes.
    fn on_bot_answer(
        &mut self,
        place: (i64, Option<i32>),
        label: String,
        text: &str,
        alert: bool,
        url: &str,
    ) {
        let text = one_line(text);
        // Still in the chat, and the topic, of the button pressed.
        let here = self.open.as_ref().is_some_and(|o| o.place() == place) && !self.busy();
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
            SendState::Failed { .. } => self.status = Some("This message wasn't sent".into()),
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
        // `h` and `l` move the line between the panes that way: the chat
        // list's edge, or the topics pane's, which is beside the messages.
        let left = match self.settings.chat_list_side {
            Side::Left => -1,
            Side::Right => 1,
        };
        let settings = &mut self.settings;
        match (before, key.code) {
            (Resizing::List(_), KeyCode::Char('h') | KeyCode::Left) => settings.resize_list(left),
            (Resizing::List(_), KeyCode::Char('l') | KeyCode::Right) => settings.resize_list(-left),
            (Resizing::Topics(_), KeyCode::Char('h') | KeyCode::Left) => {
                settings.resize_topics(left)
            }
            (Resizing::Topics(_), KeyCode::Char('l') | KeyCode::Right) => {
                settings.resize_topics(-left)
            }
            (Resizing::List(_), KeyCode::Char('=')) => {
                settings.chat_list_width = settings::DEFAULT_LIST_WIDTH;
            }
            (Resizing::Topics(_), KeyCode::Char('=')) => {
                settings.topics_width = settings::DEFAULT_TOPICS_WIDTH;
            }
            (_, KeyCode::Esc) => {
                match before {
                    Resizing::List(width) => settings.chat_list_width = width,
                    Resizing::Topics(width) => settings.topics_width = width,
                }
                self.resizing = None;
            }
            (_, KeyCode::Enter) => self.end_resize(),
            (_, KeyCode::Char('r')) if ctrl => self.end_resize(),
            _ => {}
        }
    }

    /// Leaves resize mode with the panes as they are, saved for next time.
    fn end_resize(&mut self) {
        let changed = match self.resizing.take() {
            Some(Resizing::List(width)) => width != self.settings.chat_list_width,
            Some(Resizing::Topics(width)) => width != self.settings.topics_width,
            None => false,
        };
        if changed && let Err(e) = self.settings.save(&self.settings_path) {
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
        self.tg
            .send_sticker(open.chat_id, open.topic, &sticker, reply_to);
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
                    topic: open.topic,
                    message_id: Some(from),
                });
                self.jump_to_message(to);
            }
            Err(why) => self.status = Some(why.into()),
        }
    }

    /// `gu`: to the first unread message, where "Unread messages" shows,
    /// or the first unread one now if none were when the chat opened,
    /// loading the history around it if needed. Ctrl-o comes back.
    fn go_to_unread(&mut self) {
        let Some(open) = self.open.as_ref() else {
            return;
        };
        let unread_now = match open.topic {
            Some(topic) => self
                .forum
                .as_ref()
                .and_then(|f| f.get(topic))
                .filter(|t| t.unread > 0)
                .map(|t| t.read_inbox()),
            None => self
                .chats
                .get(open.chat_id)
                .filter(|c| c.unread > 0)
                .map(|_| self.chats.read_inbox(open.chat_id)),
        };
        let Some(read) = open.unread_line.or(unread_now) else {
            self.status = Some("No unread messages".into());
            return;
        };
        // Just opened, and already on its way there.
        if open.unread_after == Some(read) && matches!(open.loading, Some(Page::Around(_))) {
            return;
        }
        if let Some(here) = self.here() {
            self.jumps.leave(here);
        }
        let Some(open) = self.open.as_mut() else {
            return;
        };
        open.unread_after = Some(read);
        if open.unread_loaded(read) {
            // A jump still loading elsewhere would move the cursor away.
            if matches!(open.loading, Some(Page::Around(_))) {
                open.loading = None;
            }
            open.go_to_unread();
            open.unread_after = None;
            return;
        }
        // The page replaces the loaded messages when it arrives, and the
        // cursor goes to the first unread one in it. With nothing read yet,
        // from the first message.
        self.load_page(Page::Around(read.max(1)));
    }

    /// `gm` (`older`): to the oldest message that mentions you, or answers
    /// one of yours, that you haven't seen; seeing it there reads it (see
    /// [`App::mark_seen`]), so `gm` again goes on to the next. Once you've
    /// seen them all, to the one before the cursor, so `gm` goes back
    /// through them; `gM` to the one after it. TDLib is asked which they
    /// are.
    fn go_to_mention(&mut self, older: bool) {
        let unread = self.unread_mentions() > 0;
        let Some(open) = self.open.as_mut() else {
            return;
        };
        let from = open.cursor_id().unwrap_or(0);
        let ask = match (older, unread) {
            (true, true) => Mentions::Unread,
            (true, false) => Mentions::Before(from),
            // On the newest, following new ones: none can be newer.
            (false, _) if open.selected.is_none() && open.at_newest => {
                self.status = Some("No newer mentions".into());
                return;
            }
            (false, _) => Mentions::After(from),
        };
        open.mentions_asked = Some(ask);
        let general = self.forum.as_ref().map_or(topics::GENERAL, |f| f.general());
        self.tg.mentions(open.chat_id, open.topic, general, ask);
    }

    /// Messages in the open chat, or topic, that mention you or answer
    /// yours, unseen: what `gm` goes through.
    pub fn unread_mentions(&self) -> i32 {
        let Some(open) = &self.open else {
            return 0;
        };
        match open.topic {
            Some(topic) => self
                .forum
                .as_ref()
                .and_then(|f| f.get(topic))
                .map_or(0, |t| t.mentions),
            None => self.chats.get(open.chat_id).map_or(0, |c| c.mentions),
        }
    }

    /// TDLib's answer for `gm` or `gM`: the cursor goes to the mention it
    /// asked for, unless the keys went elsewhere meanwhile. Ctrl-o comes
    /// back.
    fn on_mentions(&mut self, place: (i64, Option<i32>), ask: Mentions, ids: Option<Vec<i64>>) {
        let Some(open) = self
            .open
            .as_mut()
            .filter(|o| o.place() == place && o.mentions_asked == Some(ask))
        else {
            return;
        };
        open.mentions_asked = None;
        // After an error, TDLib's message is in the status bar.
        let Some(ids) = ids else {
            return;
        };
        if self.busy() || self.focus != Focus::Messages {
            return;
        }
        let Some(to) = ask.pick(&ids) else {
            self.status = Some(
                match ask {
                    Mentions::Unread => "No unread mentions",
                    Mentions::Before(0) => "Nobody mentioned you here",
                    Mentions::Before(_) => "No older mentions",
                    Mentions::After(_) => "No newer mentions",
                }
                .into(),
            );
            return;
        };
        if let Some(here) = self.here() {
            self.jumps.leave(here);
        }
        self.jump_to_message(to);
    }

    /// `I` or `:info`: what the chat under the cursor is, or the one open,
    /// and who's in it.
    fn open_chat_info(&mut self) {
        let Some(chat_id) = self.info_chat() else {
            self.status = Some("No chat selected".into());
            return;
        };
        let Some(peer) = self.chats.get(chat_id).and_then(|c| c.peer) else {
            return;
        };
        self.chat_info = Some(ChatInfo::new(chat_id));
        self.tg.chat_info(chat_id, peer);
    }

    /// The chat `I` is about: the one under the cursor in the chat list,
    /// else the one open.
    fn info_chat(&self) -> Option<i64> {
        match self.focus {
            Focus::Chats => self.selected,
            Focus::Topics => self.forum.as_ref().map(|f| f.chat_id),
            Focus::Messages | Focus::Input => self.open.as_ref().map(|o| o.chat_id),
        }
    }

    /// The `I` popup takes all keys while it's up: they go through the
    /// members, and Enter writes to the one under the cursor.
    fn on_info_key(&mut self, key: KeyEvent, ctrl: bool) {
        let Some(info) = self.chat_info.as_mut() else {
            return;
        };
        match key.code {
            KeyCode::Char('j') | KeyCode::Down => info.move_cursor(1),
            KeyCode::Char('k') | KeyCode::Up => info.move_cursor(-1),
            KeyCode::Char('d') if ctrl => info.move_cursor(HALF_PAGE),
            KeyCode::Char('u') if ctrl => info.move_cursor(-HALF_PAGE),
            KeyCode::Char('g') => info.move_cursor(isize::MIN),
            KeyCode::Char('G') => info.move_cursor(isize::MAX),
            KeyCode::Enter => self.write_to_member(),
            KeyCode::Esc | KeyCode::Char('q' | 'I') => self.chat_info = None,
            _ => {}
        }
        self.ask_members();
    }

    /// Asks for the next page of a supergroup's members in the `I` popup,
    /// when one is wanted.
    fn ask_members(&mut self) {
        if let Some(info) = self.chat_info.as_mut()
            && let Some((supergroup_id, offset)) = info.page_to_ask()
        {
            self.tg.chat_members(info.chat_id, supergroup_id, offset);
        }
    }

    /// Enter on a member in the `I` popup: opens your chat with them,
    /// creating it if there's none, or the channel posting in the group.
    fn write_to_member(&mut self) {
        let Some(who) = self
            .chat_info
            .as_ref()
            .and_then(|i| i.current())
            .map(|m| m.who)
        else {
            return;
        };
        self.chat_info = None;
        match who {
            Sender::User(user_id) => {
                let name = self
                    .users
                    .get(&user_id)
                    .cloned()
                    .unwrap_or_else(|| "them".into());
                self.finding = Some(Finding::new(&name));
                self.tg.find_private_chat(user_id, name);
            }
            Sender::Chat(chat_id) => self.open_chat(chat_id),
        }
    }

    /// Files open in their default app once downloaded; links in the browser.
    fn open_target(&mut self, target: Target) {
        match target {
            Target::Voice { message_id, file } => self.play_voice(message_id, file.id),
            Target::Photo { message_id, file } => self.view_photo(message_id, file),
            Target::File(file) => {
                // TDLib answers at once if the file is already downloaded.
                if self.opening.insert(file.id) {
                    self.tg.download(file.id);
                }
            }
            // A chat, a message or an invite on Telegram opens here. Where
            // the link really goes decides, not its words, so it can't be
            // disguised; anything tuigram can't open goes to the browser.
            // A proxy, which all of tuigram's connection to Telegram would
            // go through: only once you say so, naming it.
            Target::Link(link) if proxy::is_link(&link.url) => self.ask_to_use_proxy(&link.url),
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

    /// Shows message `message_id`'s photo in the viewer; one that's no
    /// longer loaded opens in its app, as `o` in the viewer would.
    fn view_photo(&mut self, message_id: i64, file: MediaFile) {
        let Some(open) = self.open.as_mut() else {
            return;
        };
        let view = match open.messages.get(&message_id) {
            Some(msg) => PhotoView::of(open.chat_id, message_id, msg),
            None => {
                self.open_target(Target::File(file));
                return;
            }
        };
        let Some(view) = view else {
            self.status = Some("This photo can't be shown here".into());
            return;
        };
        // The cursor stays on it rather than following new messages: what
        // arrives meanwhile is hidden under the photo, so it isn't marked
        // read, and an Enter after closing it isn't on something unseen.
        if open.selected.is_none() {
            open.selected = Some(message_id);
        }
        self.photo_view = Some(view);
    }

    /// The photo viewer takes all keys while it's up.
    fn on_viewer_key(&mut self, key: KeyEvent) {
        let Some(view) = self.photo_view.as_mut() else {
            return;
        };
        // Ctrl-o, out of habit for going back, isn't `o`, which hands the
        // photo to another app.
        if key
            .modifiers
            .intersects(KeyModifiers::CONTROL | KeyModifiers::ALT)
        {
            return;
        }
        match key.code {
            KeyCode::Char('h') | KeyCode::Left => self.view_next_photo(false),
            KeyCode::Char('l') | KeyCode::Right => self.view_next_photo(true),
            KeyCode::Char('j' | '+' | '=') => view.zoom_in(),
            KeyCode::Char('k' | '-') => view.zoom_out(),
            // It closes first: the app comes up over tuigram anyway, and a
            // warning about the file mustn't come up under the photo.
            // What Telegram says can't be saved isn't handed to an app that
            // can save it: it's shown here instead.
            KeyCode::Char('o') if view.cant_copy.is_some() => {
                self.status = Some("This photo can't be saved, so it opens only here".into());
            }
            KeyCode::Char('o') => {
                let file = view.file.clone();
                self.photo_view = None;
                self.open_target(Target::File(file));
            }
            KeyCode::Char('y') => match view.cant_copy {
                Some(why) => self.status = Some(why.into()),
                None => {
                    let file = view.file.clone();
                    self.copy_target(Target::File(file));
                }
            },
            KeyCode::Enter | KeyCode::Esc | KeyCode::Char('q') => self.photo_view = None,
            _ => {}
        }
    }

    /// `h` / `l` in the viewer: the photo before or after the one shown,
    /// among the messages loaded. Past those, more are loaded, for `h` or
    /// `l` again.
    fn view_next_photo(&mut self, newer: bool) {
        let Some(view) = &self.photo_view else {
            return;
        };
        let Some(open) = self.open.as_ref().filter(|o| o.chat_id == view.chat_id) else {
            return;
        };
        let id = view.message_id;
        let photo =
            |(&id, msg): (&i64, &crate::messages::Msg)| PhotoView::of(open.chat_id, id, msg);
        let next = match newer {
            true => open.messages.range(id + 1..).find_map(photo),
            false => open.messages.range(..id).rev().find_map(photo),
        };
        if next.is_some() {
            self.photo_view = next;
            return;
        }
        match (newer, open.all_loaded, open.at_newest) {
            (false, false, _) => {
                self.status = Some("Loading older messages…".into());
                self.load_older_messages();
            }
            (true, _, false) => {
                self.status = Some("Loading newer messages…".into());
                self.load_newer_messages();
            }
            (false, true, _) => self.status = Some("No older photos in this chat".into()),
            (true, _, true) => self.status = Some("No newer photos in this chat".into()),
        }
    }

    /// Plays voice message `message_id` of the open chat, or pauses it if
    /// it's the one playing.
    fn play_voice(&mut self, message_id: i64, file_id: i32) {
        let Some(open) = self.open.as_ref() else {
            return;
        };
        if self.player.is_on(open.chat_id, message_id) {
            self.player.toggle();
            return;
        }
        // Not downloaded only to be refused.
        let size = open
            .messages
            .get(&message_id)
            .and_then(|m| m.voice.as_ref())
            .map_or(0, |v| v.size);
        if size > crate::voice::MAX_FILE {
            self.status = Some("Can't play this voice message: it's too long".into());
            return;
        }
        // Decided now, while it's surely loaded: it may not be once it plays.
        let tell = open.tells(message_id);
        self.player.play(open.chat_id, message_id, file_id, tell);
        // TDLib answers at once if it's downloaded already.
        self.tg.download(file_id);
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
                confirm.site = site.map(Site::link);
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
            || self.photo_view.is_some()
            || self.chat_info.is_some()
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
                            self.forget_draft(chat_id);
                            self.tg.end_secret_chat(chat_id, secret_id, open)
                        }
                        Confirmed::JoinLink { link, request } => {
                            self.finding = Some(Finding::new(&request));
                            self.tg.join_by_link(link, request);
                        }
                        Confirmed::UseProxy(link) => self.set_proxy(&link),
                        Confirmed::NoProxy => self.set_proxy(""),
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
        if let Some(why) = viewer::cant_copy(msg) {
            self.status = Some(why.into());
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
            Target::File(file) | Target::Photo { file, .. } => {
                let id = file.id;
                if self.copying.insert(id, file).is_none() {
                    self.tg.download(id);
                }
                return;
            }
            // Only ever played.
            Target::Voice { .. } => return,
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

    /// Word from the voice message playing.
    fn on_voice(&mut self, event: VoiceEvent) {
        let Some(news) = self.player.on_event(event) else {
            return;
        };
        match news.happened {
            // As in Telegram's apps, the sender sees it was played once it
            // is, and one that self-destructs once played starts its timer;
            // even if it's no longer loaded, so it can't be played again
            // unannounced.
            Happened::Started if news.tell => {
                self.tg.open_content(news.chat_id, news.message_id);
                if let Some(open) = self.open.as_mut().filter(|o| o.chat_id == news.chat_id) {
                    open.set_opened(news.message_id);
                    open.start_timer(news.message_id, SystemTime::now());
                }
            }
            Happened::Started | Happened::Ended => {}
            Happened::Failed(why) => {
                self.status = Some(format!("Can't play this voice message: {why}"));
            }
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
        self.clipboard.paste(open.place());
        self.pasting = true;
    }

    fn on_pasted(&mut self, pasted: Pasted) {
        self.pasting = false;
        // Not into another chat or topic than the one it was meant for.
        if self.open.as_ref().is_none_or(|o| o.place() != pasted.place) {
            return;
        }
        // Nor into Insert mode once the keys went to the chats or topics,
        // where an Enter meant to open one would send it.
        if !matches!(self.focus, Focus::Messages | Focus::Input) {
            self.status = Some("The paste came after you left the messages: p pastes again".into());
            return;
        }
        // Nor under the photo viewer, out of sight.
        if self.photo_view.is_some() {
            self.status = Some("The paste came while a photo was open: p pastes again".into());
            return;
        }
        if self.chat_info.is_some() {
            self.status = Some("The paste came while the info was open: p pastes again".into());
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
            SettingsMenu::CORNERS => {
                // Picked for good either way: a terminal the guess got
                // wrong stays as the user set it.
                self.rounded = !self.rounded;
                settings.corners = match self.rounded {
                    true => Corners::Rounded,
                    false => Corners::Square,
                };
            }
            SettingsMenu::PILLS => settings.nerd_font = !settings.nerd_font,
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
        self.tg
            .load_history(open.chat_id, open.topic, page, HISTORY_PAGE);
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
        self.tg
            .load_history(open.chat_id, open.topic, page, HISTORY_PAGE);
    }

    fn move_message_cursor(&mut self, delta: isize) {
        let Some(open) = self.open.as_mut() else {
            return;
        };
        // Moved by hand: no page loading later takes the cursor away.
        open.unread_after = None;
        // `G` goes to the real newest message, not the newest loaded one.
        if delta == isize::MAX && (!open.at_newest || matches!(open.loading, Some(Page::Around(_))))
        {
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

    /// Starts TDLib with `keys` and the proxy set. If that proxy can't be
    /// used, the login screen says why and TDLib isn't started: on a new
    /// database, or after logging out, it has no proxy of its own, and
    /// would connect directly, which may be what the proxy was there to
    /// avoid.
    fn start_tdlib(&mut self, keys: ApiKeys) {
        match self.proxy_setting() {
            Ok(proxy) => {
                let number = self.tg.set_tdlib_parameters(keys, proxy);
                self.proxy_changes.insert(number, ProxyChange::Start);
            }
            Err((from, why)) => self.screen = login_screen(LoginStep::BadProxy { from, why }),
        }
    }

    /// Where the proxy tuigram starts with is set.
    fn proxy_source(&self) -> &'static str {
        if self.tg.env_proxy().is_some() {
            "TG_PROXY"
        } else {
            "the proxy in settings.toml"
        }
    }

    /// The proxy to connect through, from `TG_PROXY` or the settings, or
    /// none; else where the one that can't be used is set, and why not.
    fn proxy_setting(&self) -> Result<Option<Proxy>, (&'static str, String)> {
        let (link, from) = match (self.tg.env_proxy(), &self.settings.proxy) {
            (Some(link), _) => (link, "TG_PROXY"),
            (None, Some(link)) => (link.as_str(), "the proxy in settings.toml"),
            (None, None) => return Ok(None),
        };
        proxy::parse(link).map(Some).map_err(|why| (from, why))
    }

    /// `:proxy`: the proxy's link in the prompt, to change, or empty for
    /// none. Its password or secret shows as `•••`, so the screen doesn't
    /// show it to whoever is looking.
    fn open_proxy_prompt(&mut self) {
        if self.tg.env_proxy().is_some() {
            self.status = Some("TG_PROXY sets the proxy: change it there".into());
            return;
        }
        self.open_prompt(PromptKind::Proxy);
        let link = self
            .settings
            .proxy
            .as_deref()
            .map(proxy::masked)
            .unwrap_or_default();
        if let Some(prompt) = self.prompt.as_mut() {
            prompt.input = prompt_input(&link);
        }
    }

    /// Enter in `:proxy`: asks before changing the proxy, as for a link in a
    /// message, since a paste can arrive as keys (always on Windows, which
    /// has no bracketed paste) and type `:proxy`, a link and Enter.
    fn ask_to_change_proxy(&mut self, link: &str) {
        let link = link.trim();
        let saved = self.settings.proxy.clone();
        // Left as it was shown, it's the saved one, password and all.
        if let Some(saved) = saved.filter(|saved| link == proxy::masked(saved)) {
            self.ask_to_use_proxy(&saved);
            return;
        }
        // Changed, it doesn't take the old password along, to another server.
        if link.contains(proxy::MASK) {
            self.status = Some(format!(
                "The link still has {} for the hidden password or secret: write it out",
                proxy::MASK
            ));
            return;
        }
        if !link.is_empty() {
            self.ask_to_use_proxy(link);
            return;
        }
        let Some(saved) = self.settings.proxy.as_deref() else {
            self.status = Some("No proxy: tuigram connects to Telegram directly".into());
            return;
        };
        let what = proxy::parse(saved)
            .map(|proxy| proxy::describe(&proxy))
            .unwrap_or_else(|_| "the proxy".into());
        self.confirm = Some(Confirm::new(
            "Connect directly?",
            vec![
                format!("tuigram would stop using {what}."),
                "Telegram would see your IP address, and your network that you use Telegram."
                    .into(),
            ],
            Confirmed::NoProxy,
        ));
    }

    /// A proxy link in a message, or typed in `:proxy`: asks before
    /// connecting through it.
    fn ask_to_use_proxy(&mut self, link: &str) {
        if self.tg.env_proxy().is_some() {
            self.status = Some("TG_PROXY sets the proxy: change it there".into());
            return;
        }
        match proxy::parse(link) {
            Ok(proxy) => {
                let mut lines = vec![
                    format!("Proxy:         {}", proxy::kind(&proxy)),
                    "tuigram would connect to Telegram through it. It sees your IP address \
                     and when you use Telegram, not your messages."
                        .into(),
                ];
                if matches!(proxy.r#type, ProxyType::Mtproto(_)) {
                    lines.push(
                        "It can also put a channel it promotes in your chat list, marked \
                         \"proxy sponsor\"."
                            .into(),
                    );
                }
                lines.push("`:proxy` changes it, or goes back to connecting directly.".into());
                let mut confirm = Confirm::new(
                    "Use this proxy?",
                    lines,
                    Confirmed::UseProxy(link.to_string()),
                );
                confirm.site = Some(Site::server(proxy::address(&proxy)));
                self.confirm = Some(confirm);
            }
            Err(why) => self.status = Some(why),
        }
    }

    /// Connects through the proxy this link gives from now on, and next
    /// time; an empty one connects directly. It's saved, and said, once
    /// TDLib takes it ([`App::on_proxy_applied`]).
    fn set_proxy(&mut self, link: &str) {
        let link = link.trim();
        let proxy = if link.is_empty() {
            None
        } else {
            match proxy::parse(link) {
                Ok(proxy) => Some(proxy),
                Err(why) => {
                    self.status = Some(why);
                    return;
                }
            }
        };
        self.status = Some(match &proxy {
            Some(proxy) => format!("Switching to {}…", proxy::describe(proxy)),
            None => "Switching to connecting directly…".into(),
        });
        let number = self.tg.use_proxy(proxy);
        let link = (!link.is_empty()).then(|| link.to_string());
        self.proxy_changes.insert(number, ProxyChange::Set(link));
    }

    /// TDLib took a proxy change, or refused it and kept the proxy it had.
    /// What it took is saved, even if a later change is still on its way,
    /// so the settings always say what TDLib does.
    fn on_proxy_applied(&mut self, number: u64, result: Result<(), String>) {
        let Some(change) = self.proxy_changes.remove(&number) else {
            return;
        };
        // Ones asked before it were skipped for it, and won't be answered.
        self.proxy_changes.retain(|&n, _| n > number);
        match (change, result) {
            (ProxyChange::Start, Ok(())) => {}
            // TDLib started with its network off, which stays off: logged
            // in or not, only quitting is left, as for a link tuigram can't
            // read. The main screen goes, and with it read receipts and
            // being online.
            (ProxyChange::Start, Err(why)) => {
                let from = self.proxy_source();
                self.screen = login_screen(LoginStep::BadProxy { from, why });
            }
            (ProxyChange::Set(link), Ok(())) => {
                let proxy = link.as_deref().and_then(|l| proxy::parse(l).ok());
                self.settings.proxy = link;
                self.status = None;
                if let Err(e) = self.settings.save(&self.settings_path) {
                    self.status = Some(format!("Proxy not saved for next time: {e:#}"));
                }
                match &proxy {
                    Some(proxy) => self.show_toast("Connecting through", &proxy::describe(proxy)),
                    None => self.show_toast("No proxy", "Connecting to Telegram directly"),
                }
            }
            (ProxyChange::Set(_), Err(why)) => {
                self.status = Some(format!("The proxy wasn't changed: {why}"));
            }
        }
    }

    /// A chat marked as unread (on another device) isn't once it's opened,
    /// as in Telegram.
    fn unmark_unread(&mut self, chat_id: i64) {
        self.chats.set_marked_unread(chat_id, false);
        self.tg.mark_unread(chat_id, false);
    }

    /// `a` in the list: moves the chat under the cursor to the archive, or
    /// out of it to the main list, on Telegram. As it leaves the list
    /// shown, the cursor goes on to the next chat.
    fn toggle_archive(&mut self) {
        let Some(chat_id) = self.selected else {
            return;
        };
        let archived = self.chats.in_list(chat_id, List::Archive);
        // Folders keep archived chats.
        if matches!(self.chats.shown(), List::Main | List::Archive) {
            let ids = self.chats.ids();
            if let Some(i) = ids.iter().position(|&id| id == chat_id) {
                let next = ids
                    .get(i + 1)
                    .or_else(|| i.checked_sub(1).and_then(|i| ids.get(i)));
                self.selected = next.copied();
            }
        }
        self.tg.archive(chat_id, !archived);
        let title = self.chats.title(chat_id).unwrap_or_default().to_string();
        let done = if archived {
            "Moved out of the archive"
        } else {
            "Archived"
        };
        self.show_toast(done, &title);
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
        self.player.stop();
        if self.quit_deadline.is_some() {
            // Second press: stop waiting for TDLib.
            self.exit = true;
            return;
        }
        let draft = self.draft_to_keep();
        self.tg.close(draft);
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

/// Trying to reach Telegram, directly: where it's blocked, that's all
/// that ever happens.
pub const CONNECTING: &str = "Connecting…";

/// What the connection to Telegram is doing, in words, while it isn't
/// working.
fn connection_words(state: &ConnectionState) -> Option<&'static str> {
    match state {
        ConnectionState::WaitingForNetwork => Some("Waiting for network…"),
        ConnectionState::ConnectingToProxy => Some("Connecting to the proxy…"),
        ConnectionState::Connecting => Some(CONNECTING),
        ConnectionState::Updating => Some("Updating…"),
        ConnectionState::Ready => None,
    }
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
    use crate::tg::Spot;

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
    fn the_look_rows_change_only_their_own_settings() {
        let dir = std::env::temp_dir().join(format!("tuigram-look-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let images = Images::new(Picker::halfblocks(), unbounded_channel().0);
        let tg = Tg::detached(unbounded_channel().0);
        let mut app = crate::demo::demo_app(tg, images, &dir);
        let none = KeyModifiers::NONE;
        let saved = || Settings::load(&settings::path(&dir)).unwrap();
        press(&mut app, KeyCode::Char('?'), none);
        press(&mut app, KeyCode::Tab, none);
        // In the order drawn, right before the secret chats' row, which
        // tells Telegram something when it changes.
        let selected = |app: &App| app.settings_menu.as_ref().unwrap().selected;
        app.settings_menu.as_mut().unwrap().selected = SettingsMenu::BLOCK_GAPS;
        press(&mut app, KeyCode::Char('j'), none);
        assert_eq!(selected(&app), SettingsMenu::CORNERS);
        let rounded = app.rounded;
        press(&mut app, KeyCode::Enter, none);
        assert_eq!(app.rounded, !rounded, "at once");
        let expected = if rounded {
            Corners::Square
        } else {
            Corners::Rounded
        };
        assert_eq!(saved().corners, expected, "kept, whatever the terminal");

        press(&mut app, KeyCode::Char('j'), none);
        assert_eq!(selected(&app), SettingsMenu::PILLS);
        press(&mut app, KeyCode::Enter, none);
        assert!(saved().nerd_font);
        let status = screen(&mut app).pop().unwrap();
        assert!(status.starts_with("\u{e0b6}NORMAL\u{e0b4}"), "{status}");

        let after = saved();
        assert!(!after.accept_secret_chats, "{after:?}");
        assert!(!after.normal_after_send, "{after:?}");
        assert!(after.block_gaps, "{after:?}");
        std::fs::remove_dir_all(&dir).unwrap();
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
        // The text left and right of the first pane's top right corner,
        // round or square as the terminal running the tests has them.
        let halves = |app: &mut App| {
            let top = screen(app).remove(0);
            let (left, right) = top.split_once(['╮', '┐']).unwrap();
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
            "{left}╮{right}"
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
            app.watching(chat) && app.sees(chat, None),
            "a key was just pressed"
        );

        // tmux without focus-events, or a detached session.
        // By the wall clock: Instant can't go back past boot on Windows.
        app.last_input_wall = SystemTime::now() - (IDLE_AFTER + Duration::from_secs(1));
        assert!(!app.watching(chat), "new messages aren't marked read");
        assert!(!app.sees(chat, None), "and they notify");

        // A terminal that reports focus is believed instead, for a while.
        app.focus_reported = true;
        assert!(app.watching(chat) && app.sees(chat, None));
        app.last_input_wall = SystemTime::now() - AWAY_AFTER;
        assert!(
            !app.watching(chat),
            "not for good: the screen may be left on"
        );
        assert!(
            app.sees(chat, None),
            "the window has focus, so no notification"
        );
        app.last_input_wall = SystemTime::now();
        app.terminal_focused = false;
        assert!(!app.watching(chat) && !app.sees(chat, None));
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
        assert!(!app.watching(chat) && !app.sees(chat, None));
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
        let edge = |rows: &[String]| {
            rows[0]
                .chars()
                .position(|c| matches!(c, '╮' | '┐'))
                .unwrap()
        };
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

    /// The chat open in the demo, and another plain chat.
    fn two_chats(app: &App) -> (i64, i64) {
        let here = app.open.as_ref().unwrap().chat_id;
        let other = app
            .chats
            .ids()
            .iter()
            .copied()
            .find(|&id| id != here && !app.chats.is_forum(id) && !app.chats.is_secret(id))
            .unwrap();
        (here, other)
    }

    fn draft_text(app: &App, chat_id: i64) -> Option<&str> {
        app.chats.draft(chat_id).map(|d| d.text.as_str())
    }

    #[test]
    fn what_is_left_written_in_a_chat_is_its_draft_and_comes_back_when_it_opens() {
        crate::tg::quiet();
        let mut app = test_app("drafts");
        let none = KeyModifiers::NONE;
        let (here, other) = two_chats(&app);
        app.focus = Focus::Messages;
        press(&mut app, KeyCode::Char('i'), none);
        for c in "on my **way**".chars() {
            press(&mut app, KeyCode::Char(c), none);
        }
        // Esc keeps it, for your other devices.
        press(&mut app, KeyCode::Esc, none);
        assert_eq!(draft_text(&app, here), Some("on my **way**"));

        app.open_chat(other);
        assert_eq!(app.composer.lines().join("\n"), "");

        // Back, it's in the composer again, as it was typed.
        app.open_chat(here);
        assert_eq!(app.composer.lines().join("\n"), "on my **way**");

        // Emptied, the draft goes once you leave.
        app.composer = new_composer();
        app.open_chat(other);
        assert_eq!(draft_text(&app, here), None);
    }

    #[test]
    fn a_draft_written_elsewhere_is_not_replaced_by_a_composer_nobody_touched() {
        crate::tg::quiet();
        let mut app = test_app("drafts-elsewhere");
        let (here, other) = two_chats(&app);
        // Written on the phone while the chat is open here.
        app.chats
            .keep_draft(here, Draft::new("from my phone", Some(7)));
        app.open_chat(other);
        assert_eq!(draft_text(&app, here), Some("from my phone"));
        // Opened again, it's in the composer, answering its message once
        // that's fetched.
        app.open_chat(here);
        assert_eq!(app.composer.lines().join("\n"), "from my phone");
        let open = app.open.as_ref().unwrap();
        assert_eq!((open.draft_reply, open.reply.is_none()), (Some(7), true));
        // Left before the answer came, it still answers it.
        app.open_chat(other);
        assert_eq!(app.chats.draft(here).and_then(|d| d.reply_to), Some(7));
    }

    #[test]
    fn a_drafts_reply_that_is_not_shown_is_not_sent() {
        crate::tg::quiet();
        let mut app = test_app("drafts-hidden-reply");
        let none = KeyModifiers::NONE;
        let (here, other) = two_chats(&app);
        app.chats
            .keep_draft(here, Draft::new("from my phone", Some(7)));
        app.open_chat(other);
        app.open_chat(here);
        app.focus = Focus::Messages;
        // `r` on another message while message 7 is still being fetched,
        // then that reply cancelled: nothing shows over the composer.
        let open = app.open.as_mut().unwrap();
        assert_eq!(open.draft_reply, Some(7));
        open.add_page(Page::Latest, crate::messages::tests::page([1, 2, 3]));
        let another = 2;
        open.selected = Some(another);
        press(&mut app, KeyCode::Char('r'), none);
        let open = app.open.as_ref().unwrap();
        assert_eq!(open.reply.as_ref().map(|r| r.id), Some(another));
        assert_eq!(open.draft_reply, None, "message 7 can't come back over it");
        press(&mut app, KeyCode::Esc, none);
        press(&mut app, KeyCode::Esc, none);
        assert!(app.open.as_ref().unwrap().reply.is_none());
        assert_eq!(app.written().unwrap().1, None, "the draft answers nothing");

        // Sent before message 7 comes, the text doesn't answer it, and the
        // next message won't once it comes.
        app.open_chat(other);
        app.chats
            .keep_draft(here, Draft::new("from my phone", Some(7)));
        app.open_chat(here);
        let open = app.open.as_mut().unwrap();
        assert_eq!(open.draft_reply, Some(7));
        assert_eq!(open.take_reply(), None);
        assert_eq!(open.draft_reply, None);
        // A reply that's shown is sent.
        open.add_page(Page::Latest, crate::messages::tests::page([1, 2, 3]));
        open.selected = Some(2);
        press(&mut app, KeyCode::Char('r'), none);
        assert_eq!(app.open.as_mut().unwrap().take_reply(), Some(2));
    }

    #[test]
    fn the_status_bar_says_while_telegram_cant_be_reached() {
        let mut app = test_app("connection");
        let state =
            |state| Update::ConnectionState(tdlib_rs::types::UpdateConnectionState { state });
        app.on_update(state(ConnectionState::WaitingForNetwork));
        let rows = screen(&mut app);
        assert!(
            rows[19].starts_with(" NORMAL   Waiting for network…"),
            "{:?}",
            rows[19]
        );
        app.on_update(state(ConnectionState::ConnectingToProxy));
        assert!(screen(&mut app)[19].contains("Connecting to the proxy…"));
        app.on_update(state(ConnectionState::Ready));
        assert!(!screen(&mut app)[19].contains('…'));
    }

    #[test]
    fn ctrl_o_and_ctrl_i_go_back_and_forward_like_vims_jump_list() {
        let at = |chat_id, message_id| Jump {
            chat_id,
            topic: None,
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
    fn h_and_l_go_from_the_chats_to_a_forums_topics_to_a_topics_messages_and_back() {
        let mut app = test_app("forum");
        crate::demo::show_forum(&mut app);
        let none = KeyModifiers::NONE;
        let topic = |app: &App| app.forum.as_ref().and_then(|f| f.current()).map(|t| t.id);
        assert!(app.focus == Focus::Topics);
        let open = topic(&app);
        assert_eq!(app.open.as_ref().and_then(|o| o.topic), open);

        press(&mut app, KeyCode::Char('j'), none);
        assert_ne!(topic(&app), open);
        press(&mut app, KeyCode::Char('k'), none);
        assert_eq!(topic(&app), open);
        // The topic open: nothing to load.
        press(&mut app, KeyCode::Char('l'), none);
        assert!(app.focus == Focus::Messages);
        press(&mut app, KeyCode::Char('h'), none);
        assert!(
            app.focus == Focus::Topics,
            "back to the topics, not the chats"
        );
        press(&mut app, KeyCode::Char('h'), none);
        assert!(app.focus == Focus::Chats);
        // The forum shown: its topics again, the one open kept.
        press(&mut app, KeyCode::Char('l'), none);
        assert!(app.focus == Focus::Topics);
        assert_eq!(app.open.as_ref().and_then(|o| o.topic), open);
        press(&mut app, KeyCode::Esc, none);
        assert!(app.focus == Focus::Chats);
        press(&mut app, KeyCode::Enter, none);
        press(&mut app, KeyCode::Enter, none);
        press(&mut app, KeyCode::Esc, none);
        assert!(
            app.focus == Focus::Topics,
            "Esc leaves a topic for the topics"
        );
    }

    #[test]
    fn whats_meant_for_one_topic_stays_out_of_another() {
        let mut app = test_app("forum-place");
        crate::demo::show_forum(&mut app);
        let (chat, topic) = app.open.as_ref().unwrap().place();
        let other = (chat, topic.map(|t| t + 1));
        app.focus = Focus::Messages;

        // A paste that comes back after another topic was opened.
        app.pasting = true;
        app.on_pasted(Pasted {
            place: other,
            content: Ok(Paste::Text("the screenshot".into())),
        });
        assert!(app.focus == Focus::Messages, "not Insert mode");
        assert!(!app.pasting);
        assert!(app.composer.is_empty());
        // Or once the keys went back to the topics.
        app.focus = Focus::Topics;
        app.on_pasted(Pasted {
            place: (chat, topic),
            content: Ok(Paste::Text("the screenshot".into())),
        });
        assert!(app.focus == Focus::Topics);
        assert!(app.composer.is_empty());
        app.focus = Focus::Messages;

        // A bot's alert, for a button pressed in another topic.
        app.on_bot_answer(other, "Buy".into(), "Sold out", true, "");
        assert!(app.notice.is_none());
        app.on_bot_answer((chat, topic), "Buy".into(), "Sold out", true, "");
        assert!(app.notice.is_some());
        app.notice = None;

        // `gd` to a message of another topic keeps the view.
        let before: Vec<i64> = app
            .open
            .as_ref()
            .unwrap()
            .messages
            .keys()
            .copied()
            .collect();
        app.open.as_mut().unwrap().loading = Some(Page::Around(1));
        app.on_history((chat, topic), Page::Around(1), Some(Vec::new()));
        let after: Vec<i64> = app
            .open
            .as_ref()
            .unwrap()
            .messages
            .keys()
            .copied()
            .collect();
        assert_eq!(before, after);
        assert_eq!(
            app.status.as_deref(),
            Some("That message isn't in this topic")
        );

        // Reading one topic, a message in another still notifies, where the
        // terminal never says whether its window has focus.
        assert!(app.sees(chat, topic));
        assert!(!app.sees(chat, other.1));
    }

    #[test]
    fn ctrl_r_in_a_forums_topics_resizes_their_pane() {
        let mut app = test_app("forum-resize");
        crate::demo::show_forum(&mut app);
        let none = KeyModifiers::NONE;
        // Where the topics pane's top right corner is: the chat list's is
        // the first.
        let edge = |rows: &[String]| {
            let top: Vec<char> = rows[0].chars().collect();
            top.iter()
                .enumerate()
                .filter(|&(_, &c)| matches!(c, '╮' | '┐'))
                .map(|(i, _)| i)
                .nth(1)
                .unwrap()
        };
        let start = edge(&screen_of(&mut app, 140));
        let list = app.settings.chat_list_width;

        press(&mut app, KeyCode::Char('r'), KeyModifiers::CONTROL);
        assert_eq!(app.resizing, Some(Resizing::Topics(30)));
        press(&mut app, KeyCode::Char('l'), none);
        press(&mut app, KeyCode::Char('l'), none);
        let rows = screen_of(&mut app, 140);
        assert_eq!(edge(&rows), start + 4, "two columns a press");
        assert!(rows.last().unwrap().contains("topics 34 columns"));
        assert_eq!(app.settings.chat_list_width, list, "the chat list stays");
        press(&mut app, KeyCode::Esc, none);
        assert_eq!(edge(&screen_of(&mut app, 140)), start, "Esc puts it back");

        press(&mut app, KeyCode::Char('r'), KeyModifiers::CONTROL);
        press(&mut app, KeyCode::Char('h'), none);
        press(&mut app, KeyCode::Enter, none);
        assert!(app.resizing.is_none());
        assert_eq!(app.settings.topics_width, 28);
        let saved = Settings::load(&app.settings_path).unwrap();
        assert_eq!(saved.topics_width, 28, "kept for next time");

        // Elsewhere, Ctrl-r still resizes the chat list.
        app.focus = Focus::Messages;
        press(&mut app, KeyCode::Char('r'), KeyModifiers::CONTROL);
        assert!(matches!(app.resizing, Some(Resizing::List(_))));
    }

    #[test]
    fn with_the_chat_list_on_the_right_the_topics_are_still_between_it_and_the_messages() {
        let mut app = test_app("forum-right");
        crate::demo::show_forum(&mut app);
        app.settings.chat_list_side = Side::Right;
        let rows = screen_of(&mut app, 140);
        let top = &rows[0];
        let messages = top.find("Rustaceans › Async").expect("the messages' title");
        let topics = top.find("Rustaceans · topics").expect("the topics' title");
        let chats = top.find("Chats (").expect("the chat list's title");
        assert!(messages < topics && topics < chats, "{top}");
        // `l` goes toward the list, now on the right.
        press(&mut app, KeyCode::Char('l'), KeyModifiers::NONE);
        assert!(app.focus == Focus::Chats);
        press(&mut app, KeyCode::Char('h'), KeyModifiers::NONE);
        assert!(app.focus == Focus::Topics);
        press(&mut app, KeyCode::Char('h'), KeyModifiers::NONE);
        assert!(app.focus == Focus::Messages);
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
    fn enter_on_a_message_that_was_not_sent_sends_it_again_if_telegram_can_take_it() {
        let mut app = test_app("resend");
        app.focus = Focus::Messages;
        let id = plain_message(&mut app);
        let set = |app: &mut App, can_retry| {
            let open = app.open.as_mut().unwrap();
            open.messages.get_mut(&id).unwrap().state = SendState::Failed { can_retry };
        };
        set(&mut app, true);
        let rows = screen(&mut app);
        assert!(rows[19].contains("Enter send again"), "{:?}", rows[19]);
        press(&mut app, KeyCode::Enter, KeyModifiers::NONE);
        assert_eq!(app.status, None);
        assert!(app.menu.is_none(), "nothing opens instead");

        set(&mut app, false);
        assert!(!screen(&mut app)[19].contains("Enter send again"));
        press(&mut app, KeyCode::Enter, KeyModifiers::NONE);
        assert_eq!(
            app.status.as_deref(),
            Some("Telegram won't take this message again: `d` deletes it")
        );
    }

    #[test]
    fn a_chat_marked_unread_elsewhere_is_unmarked_once_opened_even_the_one_open() {
        let mut app = test_app("marked-unread");
        let (here, other) = two_chats(&app);
        app.chats.set_marked_unread(other, true);
        app.open_chat(other);
        assert!(!app.chats.get(other).unwrap().marked_unread);
        // Marked on the phone while it's open here: Enter on it again.
        app.chats.set_marked_unread(other, true);
        app.focus = Focus::Chats;
        app.selected = Some(other);
        press(&mut app, KeyCode::Enter, KeyModifiers::NONE);
        assert!(!app.chats.get(other).unwrap().marked_unread);
        assert!(!app.chats.get(here).unwrap().marked_unread);
    }

    #[test]
    fn a_archives_the_chat_and_the_cursor_goes_on_to_the_next() {
        let mut app = test_app("archive");
        app.focus = Focus::Chats;
        app.chats.refresh();
        let ids = app.chats.ids().to_vec();
        app.selected = Some(ids[1]);
        // Ctrl-a isn't `a`.
        press(&mut app, KeyCode::Char('a'), KeyModifiers::CONTROL);
        assert!(app.toast.is_none() && app.selected == Some(ids[1]));
        press(&mut app, KeyCode::Char('a'), KeyModifiers::NONE);
        assert_eq!(app.selected, Some(ids[2]));
        assert_eq!(
            app.toast.as_ref().map(|t| t.title.as_str()),
            Some("Archived")
        );
        // The last one: the cursor goes up instead.
        let last = *ids.last().unwrap();
        app.selected = Some(last);
        press(&mut app, KeyCode::Char('a'), KeyModifiers::NONE);
        assert_eq!(app.selected, Some(ids[ids.len() - 2]));
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
            topic: None,
            request: 1,
            messages: Some(Vec::new()),
        });
        assert_eq!(app.open.as_ref().unwrap().pinned.len(), 1);
        app.on_tg(TgEvent::Pinned {
            chat_id,
            topic: None,
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
            topic: None,
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
        app.typing = Some(((chat_id, None), Instant::now()));
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
            found: Ok(Spot::chat(999)),
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
        app.typing = Some(((chat_id, None), Instant::now()));

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
        app.typing = Some(((chat_id, None), Instant::now()));
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
    fn proxy_asks_then_sets_the_proxy_now_and_for_next_time_and_says_what_is_wrong() {
        let dir = std::env::temp_dir().join(format!("tuigram-proxy-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let images = Images::new(Picker::halfblocks(), unbounded_channel().0);
        let tg = Tg::detached(unbounded_channel().0);
        let mut app = crate::demo::demo_app(tg, images, &dir);
        let none = KeyModifiers::NONE;
        let saved = || Settings::load(&settings::path(&dir)).unwrap().proxy;
        let type_in = |app: &mut App, text: &str| {
            app.prompt.as_mut().unwrap().input = prompt_input("");
            for c in text.chars() {
                press(app, KeyCode::Char(c), none);
            }
            press(app, KeyCode::Enter, none);
        };
        // `y` right away does nothing; `y` once it has been up a moment says yes.
        let answer = |app: &mut App| {
            press(app, KeyCode::Char('y'), none);
            let confirm = app.confirm.as_mut().expect("still asking");
            confirm.shown = Instant::now().checked_sub(CONFIRM_GRACE).unwrap();
            press(app, KeyCode::Char('y'), none);
            assert!(app.confirm.is_none());
        };
        // TDLib answers the last change asked.
        let answers = |app: &mut App, result: Result<(), String>| {
            let number = *app.proxy_changes.keys().next_back().expect("one asked");
            app.on_tg(TgEvent::ProxyApplied { number, result });
        };

        command(&mut app, "proxy");
        assert!(
            app.prompt
                .as_ref()
                .is_some_and(|p| p.kind == PromptKind::Proxy)
        );
        type_in(&mut app, "socks5://10.0.0.1:1080");
        let confirm = app.confirm.as_ref().expect("asks first");
        assert_eq!(confirm.lines[0], "Proxy:         SOCKS5");
        assert_eq!(confirm.site.as_ref().unwrap().host, "10.0.0.1:1080");
        assert!(!confirm.lines.iter().any(|l| l.contains("proxy sponsor")));
        assert_eq!(saved(), None, "not before `y`");
        answer(&mut app);
        assert_eq!(saved(), None, "not before TDLib takes it");
        assert!(app.toast.is_none());
        let status = app.status.as_deref().unwrap();
        assert_eq!(status, "Switching to SOCKS5 proxy 10.0.0.1:1080…");
        answers(&mut app, Ok(()));
        assert_eq!(saved().as_deref(), Some("socks5://10.0.0.1:1080"));
        assert!(app.status.is_none());
        let toast = app.toast.as_ref().unwrap();
        assert_eq!(
            (toast.title.as_str(), toast.detail.as_str()),
            ("Connecting through", "SOCKS5 proxy 10.0.0.1:1080")
        );

        // It's there to change, and a link that can't be used changes nothing.
        command(&mut app, "proxy");
        let prompt = app.prompt.as_ref().unwrap();
        assert_eq!(prompt.query(), "socks5://10.0.0.1:1080");
        type_in(&mut app, "socks5://10.0.0.1");
        assert!(app.confirm.is_none());
        assert!(app.status.as_deref().unwrap().contains("no port"));
        assert_eq!(saved().as_deref(), Some("socks5://10.0.0.1:1080"));

        // Empty: asks, naming the proxy it stops using, then directly.
        command(&mut app, "proxy");
        type_in(&mut app, "");
        let confirm = app.confirm.as_ref().expect("asks first");
        assert_eq!(confirm.title, "Connect directly?");
        assert!(confirm.lines[0].contains("SOCKS5 proxy 10.0.0.1:1080"));
        assert_eq!(saved().as_deref(), Some("socks5://10.0.0.1:1080"));
        answer(&mut app);
        answers(&mut app, Ok(()));
        assert_eq!(saved(), None);
        assert_eq!(app.toast.as_ref().unwrap().title, "No proxy");

        // Empty again: there's nothing to stop using.
        command(&mut app, "proxy");
        type_in(&mut app, "");
        assert!(app.confirm.is_none());
        assert!(app.status.as_deref().unwrap().contains("directly"));
    }

    #[test]
    fn a_proxy_is_saved_only_once_tdlib_takes_it_and_one_it_refuses_changes_nothing() {
        let mut app = test_app("proxy-answers");
        app.settings.proxy = None;
        let link = |port| format!("socks5://10.0.0.1:{port}");
        let answer = |app: &mut App, number, result| {
            app.on_tg(TgEvent::ProxyApplied { number, result });
        };

        // Refused: nothing is saved, and it says why.
        app.set_proxy(&link(1080));
        let refused = *app.proxy_changes.keys().next_back().unwrap();
        answer(&mut app, refused, Err("Wrong port".into()));
        assert_eq!(app.settings.proxy, None);
        assert!(app.toast.is_none());
        let status = app.status.clone().unwrap();
        assert_eq!(status, "The proxy wasn't changed: Wrong port");

        // Two quick changes: TDLib may take the first before the second, and
        // what it took is saved meanwhile.
        app.set_proxy(&link(1081));
        let first = *app.proxy_changes.keys().next_back().unwrap();
        app.set_proxy(&link(1082));
        let second = *app.proxy_changes.keys().next_back().unwrap();
        answer(&mut app, first, Ok(()));
        assert_eq!(app.settings.proxy, Some(link(1081)));
        answer(&mut app, second, Err("refused".into()));
        assert_eq!(app.settings.proxy, Some(link(1081)), "what TDLib uses");
        // Or skip the first for the second, which then answers alone.
        app.set_proxy(&link(1083));
        app.set_proxy(&link(1084));
        let last = *app.proxy_changes.keys().next_back().unwrap();
        answer(&mut app, last, Ok(()));
        assert_eq!(app.settings.proxy, Some(link(1084)));
        assert!(
            app.proxy_changes.is_empty(),
            "the skipped one isn't waited for"
        );

        // Refused at startup, on the login screen: nobody logs in without it.
        app.settings.proxy = Some(link(1084));
        let keys = ApiKeys {
            id: 1,
            hash: "0123456789abcdef0123456789abcdef".into(),
        };
        app.screen = login_screen(LoginStep::Connecting);
        app.start_tdlib(keys.clone());
        let start = *app.proxy_changes.keys().next_back().unwrap();
        answer(&mut app, start, Err("Unsupported proxy secret".into()));
        let Screen::Login(login) = &app.screen else {
            panic!("still logging in");
        };
        assert!(
            matches!(&login.step, LoginStep::BadProxy { why, .. } if why == "Unsupported proxy secret")
        );
        assert_eq!(
            app.settings.proxy,
            Some(link(1084)),
            "startup saves nothing"
        );

        // Logged in already: the main screen goes too, as TDLib's network
        // stays off.
        app.screen = Screen::Main;
        app.start_tdlib(keys);
        let start = *app.proxy_changes.keys().next_back().unwrap();
        answer(&mut app, start, Err("Unsupported proxy secret".into()));
        app.on_auth_state(AuthorizationState::Ready);
        let Screen::Login(login) = &app.screen else {
            panic!("the main screen is gone");
        };
        assert!(matches!(login.step, LoginStep::BadProxy { .. }));
        let rows = screen(&mut app).concat();
        assert!(
            rows.contains("Can't use the proxy in settings.toml"),
            "{rows}"
        );
    }

    #[test]
    fn proxy_hides_the_saved_password_and_keeps_it_only_for_the_same_server() {
        let mut app = test_app("proxy-masked");
        let saved = "socks5://me:hunter2@10.0.0.1:1080";
        app.settings.proxy = Some(saved.into());
        let type_in = |app: &mut App, text: &str| {
            app.prompt.as_mut().unwrap().input = prompt_input(text);
            press(app, KeyCode::Enter, KeyModifiers::NONE);
        };

        command(&mut app, "proxy");
        let shown = app.prompt.as_ref().unwrap().query();
        assert_eq!(shown, "socks5://me:•••@10.0.0.1:1080");
        assert!(!screen(&mut app).concat().contains("hunter2"));
        // Unchanged, it's the saved one.
        press(&mut app, KeyCode::Enter, KeyModifiers::NONE);
        let confirm = app.confirm.take().expect("asks first");
        assert!(matches!(&confirm.action, Confirmed::UseProxy(link) if link == saved));

        // Another server with the hidden password: refused.
        command(&mut app, "proxy");
        type_in(&mut app, "socks5://me:•••@203.0.113.9:1080");
        assert!(app.confirm.is_none());
        assert!(app.status.as_deref().unwrap().contains("write it out"));
    }

    #[test]
    fn a_proxy_set_that_cant_be_used_keeps_tdlib_from_starting_and_says_why() {
        let bad_proxy = |app: &App| match &app.screen {
            Screen::Login(login) => match &login.step {
                LoginStep::BadProxy { from, why } => Some((*from, why.clone())),
                _ => None,
            },
            Screen::Main => None,
        };
        let keys = ApiKeys {
            id: 1,
            hash: "0123456789abcdef0123456789abcdef".into(),
        };

        // Started with a saved key.
        let mut app = test_app("proxy-unusable");
        app.settings.api_keys = Some(keys.clone());
        app.settings.proxy = Some("socks5://10.0.0.1".into());
        app.screen = login_screen(LoginStep::Connecting);
        app.on_auth_state(AuthorizationState::WaitTdlibParameters);
        let (from, why) = bad_proxy(&app).expect("TDLib isn't started");
        assert_eq!(from, "the proxy in settings.toml");
        assert!(why.contains("no port"), "{why}");
        let said = |app: &mut App| {
            let rows = screen(app);
            let words: Vec<&str> = rows
                .iter()
                .flat_map(|r| r.split_whitespace())
                .filter(|w| w != &"│")
                .collect();
            words.join(" ")
        };
        let rows = said(&mut app);
        assert!(
            rows.contains("Can't use the proxy in settings.toml"),
            "{rows}"
        );
        assert!(rows.contains("The link has no port"), "{rows}");
        assert!(rows.contains("start tuigram again"), "{rows}");
        // Nothing takes the screen from it, and Ctrl-c still quits.
        app.on_auth_state(AuthorizationState::WaitPhoneNumber);
        assert!(bad_proxy(&app).is_some());
        assert!(said(&mut app).contains("The link has no port"));
        press(&mut app, KeyCode::Char('c'), KeyModifiers::CONTROL);
        assert!(app.quit_deadline.is_some());

        // Started with a key typed in.
        let mut app = test_app("proxy-unusable-typed-key");
        app.settings.api_keys = None;
        app.settings.proxy = Some("ftp://10.0.0.1:21".into());
        app.screen = login_screen(LoginStep::ApiHash { id: 1 });
        for c in keys.hash.chars() {
            press(&mut app, KeyCode::Char(c), KeyModifiers::NONE);
        }
        press(&mut app, KeyCode::Enter, KeyModifiers::NONE);
        let (_, why) = bad_proxy(&app).expect("TDLib isn't started");
        assert!(why.starts_with("Not a proxy link"), "{why}");

        // One that can be used starts it.
        let mut app = test_app("proxy-usable");
        app.settings.api_keys = Some(keys);
        app.settings.proxy = Some("socks5://10.0.0.1:1080".into());
        app.screen = login_screen(LoginStep::Connecting);
        app.on_auth_state(AuthorizationState::WaitTdlibParameters);
        assert!(bad_proxy(&app).is_none());
    }

    #[test]
    fn a_paste_arriving_as_keys_cannot_set_a_proxy_without_a_y() {
        let mut app = test_app("paste-proxy");
        app.focus = Focus::Messages;
        assert!(app.settings.proxy.is_none());
        let key = |app: &mut App, code: KeyCode| {
            let modifiers = match code {
                KeyCode::Char(c) if !c.is_alphanumeric() => KeyModifiers::SHIFT,
                _ => KeyModifiers::NONE,
            };
            app.on_terminal_event(Event::Key(KeyEvent::new(code, modifiers)));
        };
        // What Windows hands over for a paste of ":proxy⏎socks5://…⏎y".
        let pasted = ":proxy\nsocks5://192.0.2.1:1080\ny";
        for c in pasted.chars() {
            key(
                &mut app,
                if c == '\n' {
                    KeyCode::Enter
                } else {
                    KeyCode::Char(c)
                },
            );
        }
        let confirm = app.confirm.as_ref().expect("asks first");
        assert_eq!(confirm.site.as_ref().unwrap().host, "192.0.2.1:1080");
        assert!(app.settings.proxy.is_none(), "the pasted `y` came too soon");
        assert!(app.toast.is_none());

        // A real paste never gets that far.
        let mut app = test_app("paste-proxy-bracketed");
        app.focus = Focus::Messages;
        app.on_terminal_event(Event::Paste(":proxy\r\nsocks5://192.0.2.1:1080\r\n".into()));
        assert!(app.settings.proxy.is_none());
        assert!(app.prompt.is_none() && app.confirm.is_none());
    }

    #[test]
    fn a_proxy_link_names_the_end_of_a_long_server_and_says_what_it_sees() {
        let mut app = test_app("proxy-long-host");
        let server = "proxy.my-vpn-provider.com.cdn-relay-node-77.attacker.example";
        let url = format!(
            "https://t.me/proxy?server={server}&port=443&secret=ee0123456789abcdef0123456789abcdef6578616d706c652e636f6d"
        );
        app.open_target(Target::Link(Link {
            url,
            disguise: Some("MyVPN proxy".into()),
        }));
        assert!(app.confirm.is_some(), "asks first");
        let rows = screen_of(&mut app, 80);
        let server = rows.iter().find(|r| r.contains("Server:")).unwrap();
        assert!(server.contains("attacker.example:443"), "{rows:#?}");
        // The warning wraps rather than being cut.
        let top = rows
            .iter()
            .position(|r| r.contains("Use this proxy?"))
            .unwrap();
        let bottom = rows.iter().position(|r| r.contains("y use it ·")).unwrap();
        let said = rows[top + 1..bottom]
            .iter()
            .filter_map(|r| r.split_once("││"))
            .flat_map(|(_, inside)| inside.trim_end_matches('│').split_whitespace())
            .collect::<Vec<_>>()
            .join(" ");
        assert!(
            said.contains("It sees your IP address and when you use Telegram, not your messages."),
            "{rows:#?}"
        );
        assert!(said.contains("MTProto"), "{rows:#?}");
    }

    #[test]
    fn a_chat_an_mtproto_proxy_sponsors_is_marked_in_the_list() {
        let mut app = test_app("proxy-sponsor");
        app.focus = Focus::Chats;
        let (_, chat_id) = two_chats(&app);
        let title = app.chats.title(chat_id).unwrap().to_string();
        let position = tdlib_rs::types::ChatPosition {
            list: tdlib_rs::enums::ChatList::Main,
            order: 9_000_000,
            is_pinned: false,
            source: Some(tdlib_rs::enums::ChatSource::MtprotoProxy),
        };
        app.on_update(Update::ChatPosition(tdlib_rs::types::UpdateChatPosition {
            chat_id,
            position,
        }));
        let rows = screen(&mut app);
        let marked: Vec<&String> = rows
            .iter()
            .filter(|r| r.contains("proxy sponsor"))
            .collect();
        assert_eq!(marked.len(), 1, "{rows:#?}");
        // The name is cut to leave room for it.
        let first_word = title.split(' ').next().unwrap();
        assert!(marked[0].contains(first_word), "{rows:#?}");
    }

    #[test]
    fn a_proxy_link_in_a_message_asks_first_naming_the_proxy() {
        let mut app = test_app("proxy-link");
        let url = "https://t.me/proxy?server=1.2.3.4&port=443&secret=ee0123456789abcdef0123456789abcdef6578616d706c652e636f6d";
        app.open_target(Target::Link(Link {
            url: url.into(),
            disguise: Some("free fast proxy".into()),
        }));
        assert!(app.finding.is_none(), "not looked up as a chat");
        let confirm = app.confirm.as_ref().expect("asks first");
        assert_eq!(confirm.lines[0], "Proxy:         MTProto");
        assert_eq!(confirm.site.as_ref().unwrap().host, "1.2.3.4:443");
        assert!(
            confirm
                .lines
                .iter()
                .any(|l| l.contains("\"proxy sponsor\"")),
            "it can put a channel in the list"
        );
        assert!(matches!(&confirm.action, Confirmed::UseProxy(u) if u == url));
        let rows = screen(&mut app);
        assert!(rows[19].contains("y use it"), "{:?}", rows[19]);

        // One that can't be used says why.
        app.confirm = None;
        app.open_target(Target::Link(Link {
            url: "https://t.me/proxy?server=1.2.3.4&port=443".into(),
            disguise: None,
        }));
        assert!(app.confirm.is_none());
        assert_eq!(app.status.as_deref(), Some("The link has no secret"));
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
    fn ending_a_secret_chat_forgets_its_draft() {
        crate::tg::quiet();
        let mut app = test_app("secret-end-draft");
        let chat_id = open_chardy(&mut app, Some(crate::secret::SecretState::Ready));
        app.chats
            .keep_draft(chat_id, Draft::new("meet at noon", None));
        app.composer.insert_str("meet at noon");
        app.open.as_mut().unwrap().draft = ("meet at noon".into(), None);
        app.focus = Focus::Messages;
        command(&mut app, "leave");
        let confirm = app.confirm.as_mut().expect("asks");
        confirm.shown = Instant::now().checked_sub(CONFIRM_GRACE).unwrap();
        press(&mut app, KeyCode::Char('y'), KeyModifiers::NONE);
        assert!(app.confirm.is_none());
        assert!(app.chats.draft(chat_id).is_none());
        assert_eq!(app.composer.lines(), [""]);
        assert!(app.draft_to_keep().is_none(), "leaving keeps nothing");
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

    /// The message under the cursor, made someone else's voice message, not
    /// played yet, whose file is 30.
    fn voice_message(app: &mut App) -> (i64, i64) {
        let id = plain_message(app);
        let open = app.open.as_mut().unwrap();
        let msg = open.messages.get_mut(&id).unwrap();
        msg.outgoing = false;
        msg.links.clear();
        msg.file = Some(MediaFile {
            id: 30,
            label: "Voice message".into(),
            photo: false,
        });
        msg.voice = Some(crate::voice::Voice {
            file_id: 30,
            size: 0,
            duration: 1,
            levels: Vec::new(),
            ogg: true,
            listened: false,
        });
        (open.chat_id, id)
    }

    /// Makes the cursor's message a photo: file 40 in its bubble, 41 at its
    /// largest.
    fn photo_message(app: &mut App) -> (i64, i64) {
        let id = plain_message(app);
        make_photo(app, id, 40);
        (app.open.as_ref().unwrap().chat_id, id)
    }

    /// Makes message `id` a photo: file `file_id` in its bubble, the next
    /// one at its largest.
    fn make_photo(app: &mut App, id: i64, file_id: i32) {
        let open = app.open.as_mut().unwrap();
        let msg = open.messages.get_mut(&id).unwrap();
        msg.links.clear();
        let preview = |file_id, width, height| crate::messages::Preview {
            file_id,
            width,
            height,
            thumbnail: None,
            sticker: false,
        };
        msg.preview = Some(preview(file_id, 800, 600));
        msg.photo = Some(preview(file_id + 1, 2560, 1920));
        msg.file = Some(MediaFile {
            id: file_id + 1,
            label: "Photo".into(),
            photo: true,
        });
    }

    #[test]
    fn h_and_l_in_the_viewer_go_to_the_photo_before_and_after_it() {
        let mut app = test_app("viewer-next");
        let none = KeyModifiers::NONE;
        let open = app.open.as_mut().unwrap();
        // Everything is loaded: nothing is asked of TDLib.
        open.all_loaded = true;
        open.at_newest = true;
        let (chat_id, ids) = (
            open.chat_id,
            open.messages.keys().copied().collect::<Vec<_>>(),
        );
        // Not next to each other, and the newer one the newest.
        let (older, newer) = (ids[1], ids[ids.len() - 1]);
        assert!(ids.len() > 3);
        make_photo(&mut app, older, 50);
        make_photo(&mut app, newer, 60);
        let open = app.open.as_ref().unwrap();
        app.photo_view = PhotoView::of(chat_id, newer, &open.messages[&newer]);
        let shown = |app: &App| app.photo_view.as_ref().map(|v| v.message_id);

        press(&mut app, KeyCode::Char('k'), none);
        press(&mut app, KeyCode::Char('h'), none);
        assert_eq!(
            shown(&app),
            Some(older),
            "past the messages that aren't photos"
        );
        assert_eq!(app.photo_view.as_ref().unwrap().zoom, None, "unzoomed");
        press(&mut app, KeyCode::Char('h'), none);
        assert_eq!(shown(&app), Some(older));
        assert_eq!(app.status.as_deref(), Some("No older photos in this chat"));
        press(&mut app, KeyCode::Right, none);
        assert_eq!(shown(&app), Some(newer));
        press(&mut app, KeyCode::Char('l'), none);
        assert_eq!(shown(&app), Some(newer));
        assert_eq!(app.status.as_deref(), Some("No newer photos in this chat"));
    }

    #[test]
    fn j_and_k_in_the_viewer_zoom_out_and_back_in_never_past_filling_the_window() {
        let mut app = test_app("viewer-zoom");
        app.focus = Focus::Messages;
        let none = KeyModifiers::NONE;
        photo_message(&mut app);
        press(&mut app, KeyCode::Enter, none);
        let zoom = |app: &App| app.photo_view.as_ref().unwrap().zoom;
        // A big photo opens filling the window: there's no more to zoom in.
        screen(&mut app);
        press(&mut app, KeyCode::Char('j'), none);
        assert_eq!(zoom(&app), None);
        press(&mut app, KeyCode::Char('k'), none);
        assert_eq!(zoom(&app), Some(viewer::SIZES.len() - 2));
        for _ in 0..10 {
            press(&mut app, KeyCode::Char('-'), none);
        }
        assert_eq!(zoom(&app), Some(0));
        let rows = screen(&mut app);
        assert!(rows[0].contains("Photo · 25% · loading…"), "{rows:#?}");
        for _ in 0..10 {
            press(&mut app, KeyCode::Char('j'), none);
        }
        assert_eq!(zoom(&app), Some(viewer::SIZES.len() - 1));
        assert!(screen(&mut app)[0].contains("Photo · 100%"));
    }

    #[test]
    fn enter_on_a_photo_shows_it_in_the_viewer_not_in_another_app() {
        let mut app = test_app("viewer-enter");
        app.focus = Focus::Messages;
        let none = KeyModifiers::NONE;
        let (chat_id, id) = photo_message(&mut app);
        press(&mut app, KeyCode::Enter, none);
        let view = app.photo_view.as_ref().expect("the viewer is up");
        assert_eq!((view.chat_id, view.message_id), (chat_id, id));
        assert_eq!(view.photo.file_id, 41, "at its largest");
        assert_eq!(view.stand_in.as_ref().map(|p| p.file_id), Some(40));
        assert!(app.opening.is_empty(), "nothing waits for another app");
        assert!(app.busy(), "a download finishing won't pop up over it");

        // It takes the keys.
        press(&mut app, KeyCode::Char('k'), none);
        assert_eq!(app.open.as_ref().unwrap().selected, Some(id));
        press(&mut app, KeyCode::Char('o'), KeyModifiers::CONTROL);
        assert!(app.photo_view.is_some(), "Ctrl-o isn't o");
        assert!(app.opening.is_empty());
        for close in [KeyCode::Esc, KeyCode::Char('q'), KeyCode::Enter] {
            assert!(app.photo_view.is_some());
            press(&mut app, close, none);
            assert!(app.photo_view.is_none(), "{close:?} closes it");
            press(&mut app, KeyCode::Enter, none);
        }
    }

    #[test]
    fn what_arrives_while_a_photo_is_open_is_neither_marked_read_nor_kept_quiet() {
        let mut app = test_app("viewer-unseen");
        app.focus = Focus::Messages;
        let open = app.open.as_mut().unwrap();
        let (chat_id, newest) = (open.chat_id, open.newest_id().unwrap());
        make_photo(&mut app, newest, 40);
        // Following new messages, as when a photo just came in.
        let open = app.open.as_mut().unwrap();
        open.selected = None;
        open.at_newest = true;
        assert!(app.watching(chat_id));

        press(&mut app, KeyCode::Enter, KeyModifiers::NONE);
        assert!(app.photo_view.is_some());
        assert_eq!(
            app.open.as_ref().unwrap().selected,
            Some(newest),
            "the cursor stays on the photo"
        );
        assert!(!app.watching(chat_id));
        app.open.as_mut().unwrap().selected = None;
        assert!(!app.watching(chat_id), "the photo hides the chat");
        assert!(!app.sees(chat_id, None), "so it's notified");
        app.focus_reported = true;
        assert!(!app.sees(chat_id, None), "even in a focused window");
    }

    #[test]
    fn pastes_and_late_edits_wait_until_the_photo_is_closed() {
        let mut app = test_app("viewer-paste");
        app.focus = Focus::Messages;
        let (chat_id, id) = photo_message(&mut app);
        press(&mut app, KeyCode::Enter, KeyModifiers::NONE);
        let file = std::env::temp_dir().join("tuigram-test-viewer-paste.txt");
        std::fs::write(&file, b"notes").unwrap();
        app.on_terminal_event(Event::Paste(file.to_string_lossy().into_owned()));
        assert_eq!(
            app.status.take().as_deref(),
            Some("Close the photo to paste")
        );
        let place = app.open.as_ref().unwrap().place();
        app.on_pasted(Pasted {
            place,
            content: Ok(Paste::Text("the screenshot".into())),
        });
        assert!(app.status.take().is_some());
        app.on_editable(chat_id, id, Some(true), None);
        assert!(app.status.take().is_some());
        assert!(app.focus == Focus::Messages, "not Insert mode under it");
        assert!(app.composer.is_empty());
        assert!(app.open.as_ref().unwrap().attachments.is_empty());
        assert!(app.open.as_ref().unwrap().editing.is_none());
    }

    #[test]
    fn the_viewer_never_takes_a_photo_shown_only_while_open() {
        let mut app = test_app("viewer-once");
        app.focus = Focus::Messages;
        let (_, id) = photo_message(&mut app);
        let open = app.open.as_mut().unwrap();
        open.messages.get_mut(&id).unwrap().destruct = Some(crate::secret::Destruct {
            after: 10,
            on_open: true,
            ends: None,
        });
        let file = open.messages[&id].file.clone().unwrap();
        app.view_photo(id, file);
        assert!(app.photo_view.is_none());
        assert!(app.opening.is_empty(), "nor hands it to another app");
        assert_eq!(
            app.status.as_deref(),
            Some("This photo can't be shown here")
        );
    }

    #[test]
    fn o_in_the_viewer_closes_it_and_opens_the_photo_in_its_app_as_enter_did() {
        let mut app = test_app("viewer-o");
        app.focus = Focus::Messages;
        photo_message(&mut app);
        press(&mut app, KeyCode::Enter, KeyModifiers::NONE);
        // As if TDLib was asked for it already: a detached client can't be.
        app.opening.insert(41);
        press(&mut app, KeyCode::Char('o'), KeyModifiers::NONE);
        assert!(app.photo_view.is_none());
        assert!(app.opening.contains(&41), "opens once downloaded");
        // A file that could run code still asks first.
        app.on_tg(TgEvent::Downloaded {
            file_id: 41,
            path: Some("/nonexistent/photo.exe".into()),
        });
        assert!(matches!(
            app.confirm.as_ref().map(|c| &c.action),
            Some(Confirmed::OpenFile(_))
        ));
    }

    #[test]
    fn y_and_o_in_the_viewer_hand_on_nothing_telegram_says_cant_be_saved() {
        let mut app = test_app("viewer-y");
        app.focus = Focus::Messages;
        let (_, id) = photo_message(&mut app);
        let open = app.open.as_mut().unwrap();
        open.messages.get_mut(&id).unwrap().saveable = false;
        press(&mut app, KeyCode::Enter, KeyModifiers::NONE);
        press(&mut app, KeyCode::Char('y'), KeyModifiers::NONE);
        assert_eq!(
            app.status.as_deref(),
            Some("This chat doesn't allow copying its messages")
        );
        assert!(app.copying.is_empty());
        assert!(app.photo_view.is_some(), "still up");
        press(&mut app, KeyCode::Char('o'), KeyModifiers::NONE);
        assert_eq!(
            app.status.as_deref(),
            Some("This photo can't be saved, so it opens only here")
        );
        assert!(app.opening.is_empty(), "not handed to an app that saves it");
        assert!(app.photo_view.is_some());
    }

    #[test]
    fn the_viewer_closes_once_its_message_is_deleted_or_edited_to_another_photo() {
        let mut app = test_app("viewer-gone");
        app.focus = Focus::Messages;
        let (chat_id, id) = photo_message(&mut app);
        let deleted = |message_ids| {
            Update::DeleteMessages(tdlib_rs::types::UpdateDeleteMessages {
                chat_id,
                message_ids,
                is_permanent: true,
                from_cache: false,
            })
        };
        press(&mut app, KeyCode::Enter, KeyModifiers::NONE);
        app.on_tg(TgEvent::Update(Box::new(deleted(vec![id + 1000]))));
        assert!(app.photo_view.is_some(), "another message went");
        app.on_tg(TgEvent::Update(Box::new(deleted(vec![id]))));
        assert!(app.photo_view.is_none());

        let (_, id) = photo_message(&mut app);
        press(&mut app, KeyCode::Enter, KeyModifiers::NONE);
        let edited = tdlib_rs::types::UpdateMessageContent {
            chat_id,
            message_id: id,
            new_content: tdlib_rs::enums::MessageContent::MessageText(
                tdlib_rs::types::MessageText {
                    text: tdlib_rs::types::FormattedText {
                        text: "no photo now".into(),
                        ..Default::default()
                    },
                    link_preview: None,
                    link_preview_options: None,
                },
            ),
        };
        app.on_tg(TgEvent::Update(Box::new(Update::MessageContent(edited))));
        assert!(app.photo_view.is_none());
    }

    #[test]
    fn the_viewer_covers_the_chats_and_messages_and_says_its_keys() {
        let mut app = test_app("viewer-draw");
        app.focus = Focus::Messages;
        photo_message(&mut app);
        let chat_id = app.open.as_ref().unwrap().chat_id;
        let title = app.chats.title(chat_id).unwrap().to_string();
        assert!(screen(&mut app).iter().any(|r| r.contains(&title)));
        press(&mut app, KeyCode::Enter, KeyModifiers::NONE);
        let rows = screen(&mut app);
        assert!(rows[0].contains("Photo · loading…"), "{rows:#?}");
        assert!(!rows.iter().any(|r| r.contains(&title)), "{rows:#?}");
        assert!(rows.iter().any(|r| r.contains("Loading…")), "{rows:#?}");
        assert!(
            rows[rows.len() - 1].contains("o open in its app · y copy"),
            "{rows:#?}"
        );
    }

    #[test]
    fn enter_pauses_the_voice_message_playing_and_plays_it_on() {
        let mut app = test_app("voice-pause");
        app.focus = Focus::Messages;
        let (chat_id, id) = voice_message(&mut app);
        // As Enter starts it, without asking TDLib for the file.
        app.player.play(chat_id, id, 30, true);
        press(&mut app, KeyCode::Enter, KeyModifiers::NONE);
        assert!(app.player.playback(chat_id).unwrap().paused);
        press(&mut app, KeyCode::Enter, KeyModifiers::NONE);
        assert!(!app.player.playback(chat_id).unwrap().paused);
    }

    #[test]
    fn a_voice_message_plays_once_downloaded_says_why_it_cant_and_stops_if_deleted() {
        let mut app = test_app("voice-download");
        let (tx, mut rx) = unbounded_channel();
        app.player = Player::new(tx, crate::voice::Output::Nowhere);
        let (chat_id, id) = voice_message(&mut app);

        app.player.play(chat_id, id, 30, true);
        app.on_tg(TgEvent::Downloaded {
            file_id: 30,
            path: None,
        });
        assert_eq!(app.status.take().as_deref(), Some("Download failed"));
        assert!(!app.player.is_on(chat_id, id));

        // Downloaded once nobody is there: it waits for Enter, unplayed and
        // unannounced.
        app.player.play(chat_id, id, 30, true);
        app.terminal_focused = false;
        app.on_tg(TgEvent::Downloaded {
            file_id: 30,
            path: Some("/nonexistent/voice.ogg".into()),
        });
        assert!(!app.player.is_on(chat_id, id));
        assert_eq!(
            app.status.take().as_deref(),
            Some("Voice message downloaded: Enter plays it")
        );
        app.terminal_focused = true;

        app.player.play(chat_id, id, 30, true);
        let path = std::env::temp_dir().join("tuigram-test-voice-download.ogg");
        std::fs::write(&path, b"<html>").unwrap();
        app.on_tg(TgEvent::Downloaded {
            file_id: 30,
            path: Some(path.to_string_lossy().into_owned()),
        });
        app.on_voice(rx.blocking_recv().unwrap());
        assert_eq!(
            app.status.take().as_deref(),
            Some("Can't play this voice message: it isn't an Ogg file")
        );
        assert!(!app.player.is_on(chat_id, id));

        app.player.play(chat_id, id, 30, true);
        let deleted = tdlib_rs::types::UpdateDeleteMessages {
            chat_id,
            message_ids: vec![id],
            is_permanent: true,
            from_cache: false,
        };
        app.on_tg(TgEvent::Update(Box::new(Update::DeleteMessages(deleted))));
        assert!(!app.player.is_on(chat_id, id), "stopped");
    }

    #[test]
    fn playing_your_own_voice_message_tells_nobody() {
        let mut app = test_app("voice-own");
        let (chat_id, id) = voice_message(&mut app);
        let open = app.open.as_mut().unwrap();
        open.messages.get_mut(&id).unwrap().outgoing = true;
        let tell = open.tells(id);
        assert!(!tell);
        app.player.play(chat_id, id, 30, tell);
        // No request is made: a detached client would fail the test.
        app.on_voice(app.player.event(Happened::Started));
        assert!(app.player.is_on(chat_id, id), "still playing");
    }

    #[test]
    fn a_voice_message_picked_from_a_menu_is_the_one_enter_was_on() {
        let mut app = test_app("voice-menu");
        app.focus = Focus::Messages;
        let (_, id) = voice_message(&mut app);
        let msg = app.open.as_mut().unwrap().messages.get_mut(&id).unwrap();
        msg.links = vec![Link {
            url: "https://x.dev".into(),
            disguise: None,
        }];
        // Seen only while open, as a view-once voice message is.
        msg.destruct = Some(crate::secret::Destruct {
            after: 0,
            on_open: true,
            ends: None,
        });
        press(&mut app, KeyCode::Enter, KeyModifiers::NONE);
        let menu = app.menu.as_ref().expect("the voice message and its link");
        // Played from the menu as that message, wherever the cursor has gone
        // meanwhile; never opened as a file in another app.
        assert!(
            matches!(&menu.targets[0], Target::Voice { message_id, file } if *message_id == id && file.id == 30),
            "names its message"
        );
        assert!(app.opening.is_empty());
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
        // The demo's chats have one with Alex.
        command(&mut app, "logout");
        let lines = app.confirm.take().expect("asks").lines.join(" ");
        assert!(lines.contains("Your secret chats go too"), "{lines}");
        app.chats = Chats::default();
        command(&mut app, "logout");
        let lines = app.confirm.take().expect("asks").lines.join(" ");
        assert!(!lines.contains("secret chats"), "{lines}");
    }

    /// The open chat's message ids, oldest first.
    fn message_ids(app: &App) -> Vec<i64> {
        app.open
            .as_ref()
            .unwrap()
            .messages
            .keys()
            .copied()
            .collect()
    }

    #[test]
    fn gu_goes_to_the_first_unread_message_and_ctrl_o_comes_back() {
        let mut app = test_app("unread");
        app.focus = Focus::Messages;
        let (none, ctrl) = (KeyModifiers::NONE, KeyModifiers::CONTROL);
        let cursor = |app: &App| app.open.as_ref().unwrap().selected;

        press(&mut app, KeyCode::Char('g'), none);
        press(&mut app, KeyCode::Char('u'), none);
        assert_eq!(app.status.as_deref(), Some("No unread messages"));

        // Opened with the second message the last one read.
        let ids = message_ids(&app);
        let read = ids[1];
        let open = app.open.as_mut().unwrap();
        open.unread_line = Some(read);
        let first = open.first_unread(read).unwrap();
        press(&mut app, KeyCode::Char('g'), none);
        press(&mut app, KeyCode::Char('u'), none);
        assert_eq!(cursor(&app), Some(first));
        press(&mut app, KeyCode::Char('o'), ctrl);
        assert_eq!(cursor(&app), None, "back on the newest");
    }

    #[test]
    fn gm_goes_to_the_oldest_unread_mention_which_seeing_reads() {
        let mut app = test_app("mentions");
        app.focus = Focus::Messages;
        let none = KeyModifiers::NONE;
        let chat = app.open.as_ref().unwrap().chat_id;
        let ids = message_ids(&app);
        let (older, newer) = (ids[1], ids[3]);
        let cursor = |app: &App| app.open.as_ref().unwrap().selected;

        // On the newest, following new ones, nothing is newer.
        press(&mut app, KeyCode::Char('g'), none);
        press(&mut app, KeyCode::Char('M'), none);
        assert_eq!(app.status.as_deref(), Some("No newer mentions"));

        // `gm` asks TDLib, which tests can't: what it asked is set here.
        app.chats.set_mentions(chat, 2);
        for id in [older, newer] {
            let open = app.open.as_mut().unwrap();
            open.messages.get_mut(&id).unwrap().mention = true;
        }
        app.open.as_mut().unwrap().mentions_asked = Some(Mentions::Unread);
        // An answer for another chat, or to another question, moves nothing.
        app.on_mentions((chat + 1, None), Mentions::Unread, Some(vec![newer]));
        app.on_mentions((chat, None), Mentions::Before(0), Some(vec![newer]));
        assert_eq!(cursor(&app), None);
        app.on_mentions((chat, None), Mentions::Unread, Some(vec![newer, older]));
        assert_eq!(cursor(&app), Some(older));
        assert!(app.jumps.can_go_back(), "Ctrl-o comes back");
        // Once more, nobody asked.
        app.on_mentions((chat, None), Mentions::Unread, Some(vec![newer]));
        assert_eq!(cursor(&app), Some(older));

        // On screen, with nothing over it, it's read.
        assert_eq!(app.mention_seen(), Some(older));
        assert!(app.open.as_ref().unwrap().messages[&newer].mention);

        // Seen ones, from the cursor back, or on.
        app.open.as_mut().unwrap().mentions_asked = Some(Mentions::Before(older));
        app.on_mentions((chat, None), Mentions::Before(older), Some(vec![]));
        assert_eq!(app.status.as_deref(), Some("No older mentions"));
        app.open.as_mut().unwrap().mentions_asked = Some(Mentions::After(older));
        app.on_mentions(
            (chat, None),
            Mentions::After(older),
            Some(vec![newer, older]),
        );
        assert_eq!(cursor(&app), Some(newer));
    }

    #[test]
    fn gm_picks_the_oldest_unseen_mention_or_the_nearest_one_before_or_after() {
        // Newest first, as TDLib finds them; a page from a message has it too.
        let found = [90, 70, 50, 30];
        assert_eq!(Mentions::Unread.pick(&found), Some(30));
        assert_eq!(Mentions::Before(0).pick(&found), Some(90), "the newest");
        assert_eq!(Mentions::Before(70).pick(&found), Some(50));
        assert_eq!(Mentions::After(70).pick(&found), Some(90));
        assert_eq!(Mentions::After(90).pick(&found), None);
        assert_eq!(Mentions::Before(30).pick(&found), None);
    }

    #[test]
    fn a_mention_is_not_read_under_a_popup_while_away_or_in_a_secret_chat() {
        let mut app = test_app("mentions-unseen");
        app.focus = Focus::Messages;
        let id = message_ids(&app)[2];
        let open = app.open.as_mut().unwrap();
        open.selected = Some(id);
        open.messages.get_mut(&id).unwrap().mention = true;
        assert_eq!(app.mention_seen(), Some(id));

        app.chat_info = Some(ChatInfo::new(1));
        assert_eq!(app.mention_seen(), None, "a popup is over it");
        app.chat_info = None;
        app.terminal_focused = false;
        assert_eq!(app.mention_seen(), None, "nobody is there");
        app.terminal_focused = true;
        app.focus = Focus::Input;
        assert_eq!(app.mention_seen(), None, "writing");
        app.focus = Focus::Messages;
        app.open.as_mut().unwrap().loading = Some(Page::Around(id));
        assert_eq!(app.mention_seen(), None, "on its way elsewhere");
        app.open.as_mut().unwrap().loading = None;
        // Seeing a voice message isn't playing it.
        app.open
            .as_mut()
            .unwrap()
            .messages
            .get_mut(&id)
            .unwrap()
            .unplayed = true;
        assert_eq!(app.mention_seen(), None, "not played");

        // Read all at once on another device.
        let chat = app.open.as_ref().unwrap().chat_id;
        app.on_update(Update::ChatUnreadMentionCount(
            tdlib_rs::types::UpdateChatUnreadMentionCount {
                chat_id: chat,
                unread_mention_count: 0,
            },
        ));
        assert!(!app.open.as_ref().unwrap().messages[&id].mention);

        // In a secret chat it would start the timers of what's before it.
        let mut app = test_app("mentions-secret");
        open_chardy(&mut app, Some(SecretState::Ready));
        app.focus = Focus::Messages;
        let open = app.open.as_mut().unwrap();
        open.add_page(Page::Latest, crate::messages::tests::page([1, 2, 3]));
        open.messages.get_mut(&2).unwrap().mention = true;
        open.selected = Some(2);
        assert_eq!(app.mention_seen(), None);
    }

    #[test]
    fn nothing_is_read_under_the_info_popup_or_in_a_topic_not_known_yet() {
        let mut app = test_app("info-read");
        app.focus = Focus::Messages;
        let chat = app.open.as_ref().unwrap().chat_id;
        assert!(app.watching(chat));
        // It's as tall as the pane, over the newest messages.
        app.chat_info = Some(ChatInfo::new(chat));
        assert!(!app.watching(chat));
        app.chat_info = None;

        // A topic opened from a link opens at its first unread message once
        // TDLib says where that is; nothing is read before.
        crate::demo::show_forum(&mut app);
        app.focus = Focus::Messages;
        let (forum, topic) = app.open.as_ref().unwrap().place();
        assert!(app.watching(forum), "a topic that's loaded");
        app.open.as_mut().unwrap().topic = topic.map(|t| t + 100);
        assert!(!app.watching(forum));
    }

    #[test]
    fn i_is_about_the_chat_under_the_cursor_or_the_one_open_and_goes_down_its_members() {
        let mut app = test_app("info");
        let none = KeyModifiers::NONE;
        app.focus = Focus::Chats;
        let listed = app.selected.unwrap();
        assert_eq!(app.info_chat(), Some(listed));
        app.focus = Focus::Messages;
        let open = app.open.as_ref().map(|o| o.chat_id);
        assert_eq!(app.info_chat(), open);

        // TDLib's answer; a basic group's members come with it.
        app.chat_info = Some(ChatInfo::new(listed));
        assert!(app.busy(), "it takes the keys");
        let member = |id| crate::info::Member {
            who: Sender::User(id),
            role: crate::info::Role::Member,
            title: String::new(),
        };
        let about = crate::info::About {
            roster: crate::info::Roster::All((1..=3).map(member).collect()),
            ..Default::default()
        };
        app.on_tg(TgEvent::ChatInfo {
            chat_id: listed + 1,
            about: None,
        });
        assert!(!app.chat_info.as_ref().unwrap().failed, "another chat's");
        app.on_tg(TgEvent::ChatInfo {
            chat_id: listed,
            about: Some(Box::new(about)),
        });
        let current = |app: &App| app.chat_info.as_ref().unwrap().current().map(|m| m.who);
        press(&mut app, KeyCode::Char('j'), none);
        assert_eq!(current(&app), Some(Sender::User(2)));
        press(&mut app, KeyCode::Char('G'), none);
        assert_eq!(current(&app), Some(Sender::User(3)));
        press(&mut app, KeyCode::Char('g'), none);
        assert_eq!(current(&app), Some(Sender::User(1)));
        let rows = screen(&mut app);
        assert!(rows.iter().any(|r| r.contains("Members (3)")), "{rows:#?}");
        // Files dropped on it would leave Insert mode under it.
        let file = std::env::temp_dir().join("tuigram-test-info-drop.txt");
        std::fs::write(&file, "notes").unwrap();
        app.on_terminal_event(Event::Paste(file.display().to_string()));
        assert!(app.focus == Focus::Messages, "not Insert mode");
        assert!(app.open.as_ref().unwrap().attachments.is_empty());
        press(&mut app, KeyCode::Esc, none);
        assert!(app.chat_info.is_none());
        assert!(app.focus == Focus::Messages);

        crate::demo::show_forum(&mut app);
        app.focus = Focus::Topics;
        assert_eq!(app.info_chat(), app.forum.as_ref().map(|f| f.chat_id));
    }
}

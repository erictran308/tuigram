//! Thin wrapper around one TDLib client.
//!
//! A background thread pulls everything out of TDLib: updates are forwarded to
//! the app as [`TgEvent`]s, and responses complete the pending request futures.
//! Each request runs as its own tokio task so the UI never waits on the network.

use std::ffi::{CStr, CString, c_char};
use std::future::Future;
use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::{AtomicI32, Ordering};
use std::time::Duration;

use anyhow::{Result, anyhow, bail};
use serde_json::json;
use tdlib_rs::{enums, functions, types};
use tokio::sync::mpsc::UnboundedSender;
use tokio::sync::mpsc::error::SendError;

use crate::chats::{Badge, List, Peer};
use crate::config::{ApiKeys, Config};
use crate::reactions::{self, Available, ReactionKind};
use crate::stickers::{Source, Sticker};

pub enum TgEvent {
    Update(Box<enums::Update>),
    /// A request failed. The text is TDLib's, e.g. `PHONE_CODE_INVALID`.
    Error(String),
    /// A `loadChats` call for a list finished; `all` is true once every
    /// chat in it is loaded. `failed` if TDLib answered with an error (sent
    /// before as `Error`).
    ChatsLoaded {
        list: List,
        all: bool,
        failed: bool,
    },
    /// A page of history, newest first. `None` if the request failed.
    History {
        chat_id: i64,
        page: Page,
        messages: Option<Vec<types::Message>>,
    },
    /// A page of message search results. `None` if the request failed.
    Found {
        chat_id: i64,
        query: String,
        found: Option<Found>,
    },
    /// The message that `message_id` replies to; `None` if TDLib couldn't find it.
    Replied {
        chat_id: i64,
        message_id: i64,
        replied: Option<Box<types::Message>>,
    },
    /// A chat's pinned messages, newest first, for request number
    /// `request`; `None` if TDLib couldn't send them.
    Pinned {
        chat_id: i64,
        request: u32,
        messages: Option<Vec<types::Message>>,
    },
    /// Whether a message can be pinned; `None` if TDLib couldn't say.
    Pinnable {
        chat_id: i64,
        message_id: i64,
        pinnable: Option<bool>,
    },
    /// Who a message can be deleted for; `None` if TDLib couldn't say.
    Deletable {
        chat_id: i64,
        message_id: i64,
        deletable: Option<Deletable>,
    },
    /// Whether a message can be edited; `None` if TDLib couldn't say. If it
    /// can, its words as Markdown, unless they couldn't be had.
    Editable {
        chat_id: i64,
        message_id: i64,
        editable: Option<bool>,
        text: Option<EditText>,
    },
    /// What reactions a message can get; `None` if TDLib couldn't say.
    Reactions {
        chat_id: i64,
        message_id: i64,
        available: Option<Available>,
    },
    /// Stickers for a tab of the sticker panel; `None` if TDLib couldn't
    /// send them.
    Stickers {
        source: Source,
        stickers: Option<Vec<types::Sticker>>,
    },
    /// Your sticker sets, by id and title; `None` if TDLib couldn't say.
    StickerSets(Option<Vec<(i64, String)>>),
    /// Stickers found for a search in the sticker panel.
    StickersFound {
        query: String,
        stickers: Option<Vec<types::Sticker>>,
    },
    /// A download finished; `path` is `None` if it failed.
    Downloaded {
        file_id: i32,
        path: Option<String>,
    },
    /// Whom Telegram found for a search in the `s` picker: contacts by
    /// user id, and public chats. Failures find nothing.
    ChatsFound {
        query: String,
        chat_ids: Vec<i64>,
        user_ids: Vec<i64>,
    },
    /// A chat looked up to open, by what was looked up (a username, a link
    /// or a contact's name): the chat, and the message a link pointed at.
    /// Or what to say if it can't be opened.
    ChatFound {
        request: String,
        found: Result<(i64, Option<i64>), Missed>,
    },
    /// Members of a group whose name has `query` in it, for `@` completion.
    /// Failures find nobody.
    Members {
        chat_id: i64,
        query: String,
        user_ids: Vec<i64>,
    },
    /// The commands of a chat's bots, for `/` completion. Failures find
    /// none.
    Commands {
        chat_id: i64,
        commands: Vec<crate::complete::Command>,
    },
    /// Telegram took messages to forward to this chat.
    Forwarded {
        chat_id: i64,
    },
    /// You joined this public group or channel.
    Joined {
        chat_id: i64,
    },
    /// You left this group or channel.
    Left {
        chat_id: i64,
    },
    /// A bot's answer to a button pressed on its message: a note, or an
    /// alert to show until a key is pressed, and maybe a link to open.
    /// Empty when it only changed its message.
    BotAnswer {
        chat_id: i64,
        /// The button's words.
        label: String,
        text: String,
        alert: bool,
        url: String,
    },
    /// An invite link to a chat you're not in, to ask before joining.
    Invite {
        request: String,
        link: String,
        invite: Invite,
    },
}

/// Why a chat looked up to open wasn't opened.
#[derive(Debug, PartialEq, Eq)]
pub enum Missed {
    /// What to tell the user.
    Said(String),
    /// Nothing more to say: an invite asks to join instead.
    Quiet,
    /// A link tuigram doesn't open itself (a sticker set, a bot's start
    /// link…), which a browser can.
    Elsewhere,
}

impl From<String> for Missed {
    fn from(why: String) -> Self {
        Missed::Said(why)
    }
}

/// A chat an invite link leads to, as the link describes it.
pub struct Invite {
    pub title: String,
    pub members: i32,
    pub channel: bool,
    pub badge: Option<Badge>,
    /// You'd ask to join, and an admin lets you in.
    pub by_request: bool,
}

/// Which part of a chat's history to fetch.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Page {
    /// The newest messages.
    Latest,
    /// Messages older than this one.
    Older(i64),
    /// This message and newer ones.
    Newer(i64),
    /// This message and the ones on both sides of it.
    Around(i64),
}

/// Messages matching a search, newest first.
pub struct Found {
    pub ids: Vec<i64>,
    /// TDLib's estimate of how many match in all; -1 if unknown.
    pub total: i32,
    /// Pass as `from` to get the next page; 0 when there are no more.
    pub next_from: i64,
}

/// Who a message can be deleted for. Depends on the chat type, your rights
/// in it, and Telegram's time limits, so TDLib works it out.
#[derive(Clone, Copy)]
pub struct Deletable {
    pub for_everyone: bool,
    pub for_me: bool,
}

/// TDLib download priority, 1 (lowest) to 32. Photos on screen matter.
const DOWNLOAD_PRIORITY: i32 = 16;
/// Chat photos in the list come after photos in messages.
const QUIET_DOWNLOAD_PRIORITY: i32 = 8;
/// Chats with notifications at once. Only new ones are announced, so a few
/// is plenty; TDLib allows up to 25.
const NOTIFICATION_GROUPS: i64 = 5;

/// Pinned messages asked for at once, TDLib's most.
const PINNED_PAGE: i32 = 100;
/// Pages of pinned messages asked for at most.
const PINNED_PAGES: usize = 5;

/// Stickers a search in the sticker panel asks for.
const STICKER_SEARCH_LIMIT: i32 = 100;
/// Members an `@` completion asks for.
const MEMBER_SEARCH_LIMIT: i32 = 20;
/// Contacts a search in the `s` picker asks for.
const CONTACT_SEARCH_LIMIT: i32 = 20;

/// How long [`offline_now`] gives TDLib to send it before the process ends.
const OFFLINE_GRACE: Duration = Duration::from_millis(300);

/// The client in use, for [`offline_now`]: 0 until there is one, and in
/// `--demo`.
static CLIENT: AtomicI32 = AtomicI32::new(0);

unsafe extern "C" {
    /// TDLib's synchronous entry point, for the few requests that need no
    /// client. It's linked in with tdlib-rs, which doesn't wrap it.
    fn td_execute(request: *const c_char) -> *const c_char;
    /// Sends a request without waiting for the answer. tdlib-rs wraps it in
    /// async functions, which [`offline_now`] can't count on.
    fn td_send(client_id: i32, request: *const c_char);
    /// OpenSSL's setup, from the libcrypto linked in with TDLib.
    fn OPENSSL_init_crypto(opts: u64, settings: *const std::ffi::c_void) -> std::ffi::c_int;
}

/// `OPENSSL_INIT_NO_LOAD_CONFIG`, the same in OpenSSL 1.1 and 3.
const OPENSSL_INIT_NO_LOAD_CONFIG: u64 = 0x80;

/// Keeps the OpenSSL inside TDLib from reading a configuration file. By
/// default it reads `openssl.cnf` from a folder fixed when TDLib was built,
/// which on some systems another account can create, and that file can load
/// code into tuigram, next to your session. OpenSSL sets itself up once, so
/// this must come before TDLib uses it.
fn no_openssl_config() -> Result<()> {
    // SAFETY: a plain call into the linked libcrypto with no settings; it
    // only records what to skip when OpenSSL sets itself up.
    let done = unsafe { OPENSSL_init_crypto(OPENSSL_INIT_NO_LOAD_CONFIG, std::ptr::null()) };
    anyhow::ensure!(done == 1, "OpenSSL couldn't be set up");
    Ok(())
}

/// Tells Telegram the user is offline, for the panic hook: without it,
/// others would see them online for minutes after a crash. It goes without
/// the async runtime, which may be what panicked, and waits a moment for
/// TDLib's own threads to send it. Best effort.
pub fn offline_now() {
    let client_id = CLIENT.load(Ordering::Relaxed);
    if client_id == 0 {
        return;
    }
    let request = json!({
        "@type": "setOption",
        "name": "online",
        "value": { "@type": "optionValueBoolean", "value": false },
    });
    let Ok(request) = CString::new(request.to_string()) else {
        return;
    };
    // SAFETY: `request` is a NUL-terminated string that outlives the call,
    // and TDLib takes requests from any thread.
    unsafe { td_send(client_id, request.as_ptr()) };
    std::thread::sleep(OFFLINE_GRACE);
}

/// Sends TDLib's logs (warnings and worse) to `path`. Must run before the
/// first client is created: TDLib logs to stderr until told otherwise, which
/// would print in the terminal around the TUI.
fn log_to_file(path: &Path) -> Result<()> {
    let requests = [
        json!({
            "@type": "setLogStream",
            "log_stream": {
                "@type": "logStreamFile",
                "path": path.to_string_lossy(),
                "max_file_size": 10 * 1024 * 1024,
                "redirect_stderr": false,
            },
        }),
        json!({ "@type": "setLogVerbosityLevel", "new_verbosity_level": 2 }),
    ];
    for request in requests {
        let response = execute(&request)?;
        if !response.contains(r#""@type":"ok""#) {
            bail!("TDLib log setup failed: {response}");
        }
    }
    Ok(())
}

/// Runs one of the TDLib requests that need no client, and returns its JSON
/// answer.
fn execute(request: &serde_json::Value) -> Result<String> {
    let request = CString::new(request.to_string())?;
    // SAFETY: `request` is a NUL-terminated string that outlives the call.
    // TDLib returns a NUL-terminated answer (or null) that stays valid until
    // the next `td_execute` call on this thread, and it's copied out before
    // then.
    let response = unsafe {
        let response = td_execute(request.as_ptr());
        if response.is_null() {
            String::new()
        } else {
            CStr::from_ptr(response).to_string_lossy().into_owned()
        }
    };
    Ok(response)
}

/// A formatted text from a TDLib request that needs no client, or `None` if
/// it failed.
fn execute_text(request: &serde_json::Value) -> Option<types::FormattedText> {
    let response = execute(request).ok()?;
    match serde_json::from_str(&response) {
        Ok(enums::FormattedText::FormattedText(text)) => Some(text),
        Err(_) => None,
    }
}

/// What's written in the composer, with Telegram's Markdown turned into
/// formatting, as Telegram Desktop does: `**bold**`, `__italic__`,
/// `~~strikethrough~~`, `||spoiler||`, `` `code` ``, ` ```code block``` `
/// and `[text](https://…)`. Markup that isn't closed stays as typed, so a
/// lone `*` or `_` is just a character. It runs at once rather than as a
/// request, so messages still go out in the order they were sent.
pub fn markdown(text: String) -> types::FormattedText {
    let request = json!({
        "@type": "parseMarkdown",
        "text": { "@type": "formattedText", "text": text, "entities": [] },
    });
    let mut parsed = execute_text(&request).unwrap_or_else(|| plain(text));
    // `[Bob](tg://user?id=123)`, which `@` completion writes for someone
    // without a username, mentions them, as in the Bot API's Markdown.
    for entity in &mut parsed.entities {
        if let enums::TextEntityType::TextUrl(link) = &entity.r#type
            && let Some(user_id) = link
                .url
                .strip_prefix("tg://user?id=")
                .and_then(|id| id.parse().ok())
        {
            entity.r#type =
                enums::TextEntityType::MentionName(types::TextEntityTypeMentionName { user_id });
        }
    }
    parsed
}

/// What `e` puts in the composer: your message as Markdown, so saving it
/// keeps its formatting.
pub struct EditText {
    pub markdown: String,
    /// Some formatting has no Markdown (underline, custom emoji, a mention
    /// of someone without a username…), so an edit would lose it.
    pub loses: bool,
}

/// A message's text or caption as Markdown, for editing it.
pub fn to_markdown(text: &types::FormattedText) -> Option<EditText> {
    let request = json!({ "@type": "getMarkdownText", "text": text });
    let written = execute_text(&request)?;
    // What Markdown couldn't write is left as entities. A mention of
    // someone without a username is written as the link `markdown` makes a
    // mention of again: from the end, so the offsets before stay right.
    let mut markdown = written.text.clone();
    let mut loses = false;
    let mut left = written.entities.clone();
    left.sort_by_key(|e| std::cmp::Reverse(e.offset));
    for entity in left {
        match &entity.r#type {
            enums::TextEntityType::MentionName(mention) => {
                let start = crate::messages::byte_offset(&written.text, entity.offset);
                let end = crate::messages::byte_offset(
                    &written.text,
                    entity.offset.saturating_add(entity.length),
                );
                if start < end {
                    markdown.insert_str(end, &format!("](tg://user?id={})", mention.user_id));
                    markdown.insert(start, '[');
                }
            }
            kind if !crate::messages::found_by_telegram(kind) => loses = true,
            _ => {}
        }
    }
    Some(EditText {
        markdown: crate::text::clean(&markdown),
        loses,
    })
}

/// The words of a message that `e` can change: a text, or a caption.
fn editable_text(content: &enums::MessageContent) -> Option<&types::FormattedText> {
    use enums::MessageContent as C;
    match content {
        C::MessageText(m) => Some(&m.text),
        C::MessagePhoto(m) => Some(&m.caption),
        C::MessageVideo(m) => Some(&m.caption),
        C::MessageAnimation(m) => Some(&m.caption),
        C::MessageDocument(m) => Some(&m.caption),
        C::MessageAudio(m) => Some(&m.caption),
        C::MessageVoiceNote(m) => Some(&m.caption),
        _ => None,
    }
}

/// Events go to the app tagged with the client they came from, so it can
/// drop late ones from a client it replaced after logging out.
pub type Tagged = (i32, TgEvent);

/// Sends events from one client's requests, tagged with its id.
#[derive(Clone)]
struct Events {
    client_id: i32,
    tx: UnboundedSender<Tagged>,
}

impl Events {
    fn send(&self, event: TgEvent) -> Result<(), SendError<Tagged>> {
        self.tx.send((self.client_id, event))
    }
}

#[derive(Clone)]
pub struct Tg {
    client_id: i32,
    tx: Events,
    config: Arc<Config>,
}

impl Tg {
    pub async fn start(config: Config, tx: UnboundedSender<Tagged>) -> Result<Self> {
        no_openssl_config()?;
        log_to_file(&config.data_dir.join("tdlib.log"))?;
        let client_id = tdlib_rs::create_client();
        CLIENT.store(client_id, Ordering::Relaxed);

        // `receive` blocks for up to 2s at a time, so it gets a plain thread.
        // It stops once the app drops its end of the channel.
        std::thread::spawn({
            let tx = tx.clone();
            move || {
                loop {
                    if let Some((update, client_id)) = tdlib_rs::receive()
                        && tx
                            .send((client_id, TgEvent::Update(Box::new(update))))
                            .is_err()
                    {
                        break;
                    }
                }
            }
        });

        // A new client sends no updates until it gets its first request.
        functions::get_option("version".into(), client_id)
            .await
            .map_err(|e| anyhow!("TDLib didn't start: {}", e.message))?;

        Ok(Self {
            client_id,
            tx: Events { client_id, tx },
            config: Arc::new(config),
        })
    }

    /// A client that was never started, for `--demo`, which must make no
    /// requests: TDLib isn't set up, so one would start it with its logs on
    /// the terminal.
    pub fn detached(tx: UnboundedSender<Tagged>) -> Self {
        let client_id = 0;
        Self {
            client_id,
            tx: Events { client_id, tx },
            config: Arc::new(Config {
                api_keys: None,
                data_dir: std::env::temp_dir(),
            }),
        }
    }

    /// The client events must come from to count; see [`Tagged`].
    pub fn client_id(&self) -> i32 {
        self.client_id
    }

    pub fn set_tdlib_parameters(&self, keys: ApiKeys) {
        let config = Arc::clone(&self.config);
        let client_id = self.client_id;
        self.spawn(async move {
            let dir = |name: &str| config.data_dir.join(name).to_string_lossy().into_owned();
            functions::set_tdlib_parameters(
                false,
                dir("db"),
                dir("files"),
                String::new(), // database_encryption_key
                true,          // use_file_database
                true,          // use_chat_info_database
                true,          // use_message_database: keeps history cached locally
                false,         // use_secret_chats
                keys.id,
                keys.hash,
                "en".into(),
                "Terminal".into(), // device_model: what Settings → Devices shows
                std::env::consts::OS.into(),
                env!("CARGO_PKG_VERSION").into(),
                client_id,
            )
            .await
        });
    }

    pub fn send_phone_number(&self, phone: String) {
        self.spawn(functions::set_authentication_phone_number(
            phone,
            None,
            self.client_id,
        ));
    }

    pub fn send_code(&self, code: String) {
        self.spawn(functions::check_authentication_code(code, self.client_id));
    }

    pub fn send_password(&self, password: String) {
        self.spawn(functions::check_authentication_password(
            password,
            self.client_id,
        ));
    }

    pub fn send_email(&self, email: String) {
        self.spawn(functions::set_authentication_email_address(
            email,
            self.client_id,
        ));
    }

    /// Asks for a link to show as a QR code. TDLib answers with
    /// `authorizationStateWaitOtherDeviceConfirmation`, and again with a new
    /// link whenever the old one expires.
    pub fn request_qr_code(&self) {
        self.spawn(functions::request_qr_code_authentication(
            Vec::new(),
            self.client_id,
        ));
    }

    pub fn send_email_code(&self, code: String) {
        self.spawn(functions::check_authentication_email_code(
            enums::EmailAddressAuthentication::Code(types::EmailAddressAuthenticationCode { code }),
            self.client_id,
        ));
    }

    /// Asks TDLib for the next `limit` chats of a list. They arrive as
    /// `updateNewChat`/`updateChatPosition` updates, not in the response.
    pub fn load_chats(&self, list: List, limit: i32) {
        let tx = self.tx.clone();
        let client_id = self.client_id;
        tokio::spawn(async move {
            let (all, failed) =
                match functions::load_chats(Some(list.tdlib()), limit, client_id).await {
                    Ok(()) => (false, false),
                    // 404 is TDLib's way of saying there are no more chats.
                    Err(e) if e.code == 404 => (true, false),
                    Err(e) => {
                        let _ = tx.send(TgEvent::Error(e.message));
                        (false, true)
                    }
                };
            let _ = tx.send(TgEvent::ChatsLoaded { list, all, failed });
        });
    }

    pub fn open_chat(&self, chat_id: i64) {
        self.spawn(functions::open_chat(chat_id, self.client_id));
    }

    pub fn close_chat(&self, chat_id: i64) {
        self.spawn(functions::close_chat(chat_id, self.client_id));
    }

    /// Fetches a page of history, up to `limit` (at most 100) messages.
    /// TDLib may return fewer.
    pub fn load_history(&self, chat_id: i64, page: Page, limit: i32) {
        // A negative offset adds that many messages newer than `from`.
        let (from, offset) = match page {
            Page::Latest => (0, 0),
            Page::Older(id) => (id, 0),
            Page::Newer(id) => (id, 1 - limit),
            Page::Around(id) => (id, -limit / 2),
        };
        let tx = self.tx.clone();
        let client_id = self.client_id;
        tokio::spawn(async move {
            let result =
                functions::get_chat_history(chat_id, from, offset, limit, false, client_id).await;
            let messages = match result {
                Ok(enums::Messages::Messages(page)) => {
                    Some(page.messages.into_iter().flatten().collect())
                }
                Err(e) => {
                    let _ = tx.send(TgEvent::Error(e.message));
                    None
                }
            };
            let _ = tx.send(TgEvent::History {
                chat_id,
                page,
                messages,
            });
        });
    }

    /// Searches the whole chat for messages with `query`, newest first,
    /// starting from `from` (0 = the newest message).
    pub fn search_messages(&self, chat_id: i64, query: String, from: i64, limit: i32) {
        let tx = self.tx.clone();
        let client_id = self.client_id;
        tokio::spawn(async move {
            let result = functions::search_chat_messages(
                chat_id,
                None,
                query.clone(),
                None,
                from,
                0,
                limit,
                None,
                client_id,
            )
            .await;
            let found = match result {
                Ok(enums::FoundChatMessages::FoundChatMessages(f)) => Some(Found {
                    ids: f.messages.iter().map(|m| m.id).collect(),
                    total: f.total_count,
                    next_from: f.next_from_message_id,
                }),
                Err(e) => {
                    let _ = tx.send(TgEvent::Error(e.message));
                    None
                }
            };
            let _ = tx.send(TgEvent::Found {
                chat_id,
                query,
                found,
            });
        });
    }

    /// Fetches a chat's pinned messages, newest first, as
    /// [`TgEvent::Pinned`] numbered `request`.
    pub fn pinned_messages(&self, chat_id: i64, request: u32) {
        let tx = self.tx.clone();
        let client_id = self.client_id;
        tokio::spawn(async move {
            let mut messages = Vec::new();
            let mut from = 0;
            for _ in 0..PINNED_PAGES {
                let result = functions::search_chat_messages(
                    chat_id,
                    None,
                    String::new(),
                    None,
                    from,
                    0,
                    PINNED_PAGE,
                    Some(enums::SearchMessagesFilter::Pinned),
                    client_id,
                )
                .await;
                match result {
                    Ok(enums::FoundChatMessages::FoundChatMessages(found)) => {
                        messages.extend(found.messages);
                        from = found.next_from_message_id;
                        if from == 0 {
                            break;
                        }
                    }
                    Err(e) => {
                        let _ = tx.send(TgEvent::Error(e.message));
                        let _ = tx.send(TgEvent::Pinned {
                            chat_id,
                            request,
                            messages: None,
                        });
                        return;
                    }
                }
            }
            let _ = tx.send(TgEvent::Pinned {
                chat_id,
                request,
                messages: Some(messages),
            });
        });
    }

    /// Asks whether a message can be pinned, which depends on the chat and
    /// your rights in it.
    pub fn check_pinnable(&self, chat_id: i64, message_id: i64) {
        let tx = self.tx.clone();
        let client_id = self.client_id;
        tokio::spawn(async move {
            let result = functions::get_message_properties(chat_id, message_id, client_id).await;
            let pinnable = match result {
                Ok(enums::MessageProperties::MessageProperties(p)) => Some(p.can_be_pinned),
                Err(e) => {
                    let _ = tx.send(TgEvent::Error(e.message));
                    None
                }
            };
            let _ = tx.send(TgEvent::Pinnable {
                chat_id,
                message_id,
                pinnable,
            });
        });
    }

    /// Pins a message: `quietly` without a notification, `only_for_self`
    /// in a chat with one person. TDLib then sends `updateMessageIsPinned`.
    pub fn pin_message(&self, chat_id: i64, message_id: i64, quietly: bool, only_for_self: bool) {
        self.spawn(functions::pin_chat_message(
            chat_id,
            message_id,
            quietly,
            only_for_self,
            self.client_id,
        ));
    }

    /// Unpins a message. TDLib then sends `updateMessageIsPinned`.
    pub fn unpin_message(&self, chat_id: i64, message_id: i64) {
        self.spawn(functions::unpin_chat_message(
            chat_id,
            message_id,
            self.client_id,
        ));
    }

    /// Fetches the message that message `message_id` replies to, even from
    /// another chat. Failures aren't errors to show: the message was usually
    /// just deleted.
    pub fn get_replied_message(&self, chat_id: i64, message_id: i64) {
        let tx = self.tx.clone();
        let client_id = self.client_id;
        tokio::spawn(async move {
            let replied = functions::get_replied_message(chat_id, message_id, client_id)
                .await
                .ok()
                .map(|enums::Message::Message(m)| Box::new(m));
            let _ = tx.send(TgEvent::Replied {
                chat_id,
                message_id,
                replied,
            });
        });
    }

    /// Asks TDLib who a message can be deleted for. Answers from local data.
    pub fn check_deletable(&self, chat_id: i64, message_id: i64) {
        let tx = self.tx.clone();
        let client_id = self.client_id;
        tokio::spawn(async move {
            let result = functions::get_message_properties(chat_id, message_id, client_id).await;
            let deletable = match result {
                Ok(enums::MessageProperties::MessageProperties(p)) => Some(Deletable {
                    for_everyone: p.can_be_deleted_for_all_users,
                    for_me: p.can_be_deleted_only_for_self,
                }),
                Err(e) => {
                    let _ = tx.send(TgEvent::Error(e.message));
                    None
                }
            };
            let _ = tx.send(TgEvent::Deletable {
                chat_id,
                message_id,
                deletable,
            });
        });
    }

    /// Asks what reactions a message can get: the chat decides which, and
    /// some can't be added without Telegram Premium.
    pub fn available_reactions(&self, chat_id: i64, message_id: i64) {
        let tx = self.tx.clone();
        let client_id = self.client_id;
        tokio::spawn(async move {
            let row = reactions::COLUMNS as i32;
            let result =
                functions::get_message_available_reactions(chat_id, message_id, row, client_id)
                    .await;
            let available = match result {
                Ok(enums::AvailableReactions::AvailableReactions(r)) => Some(Available::from(r)),
                Err(e) => {
                    let _ = tx.send(TgEvent::Error(e.message));
                    None
                }
            };
            let _ = tx.send(TgEvent::Reactions {
                chat_id,
                message_id,
                available,
            });
        });
    }

    /// Adds your reaction to a message, or takes it back. TDLib shows the
    /// change at once with `updateMessageInteractionInfo`.
    pub fn react(&self, chat_id: i64, message_id: i64, kind: &ReactionKind, add: bool) {
        let Some(reaction) = kind.to_tdlib() else {
            return;
        };
        if add {
            self.spawn(functions::add_message_reaction(
                chat_id,
                message_id,
                reaction,
                false,
                true,
                self.client_id,
            ));
        } else {
            self.spawn(functions::remove_message_reaction(
                chat_id,
                message_id,
                reaction,
                self.client_id,
            ));
        }
    }

    /// Asks whether a message can be edited (TDLib knows whose it is, and
    /// how long the chat allows edits for), and for its words as Markdown.
    pub fn check_editable(&self, chat_id: i64, message_id: i64) {
        let tx = self.tx.clone();
        let client_id = self.client_id;
        tokio::spawn(async move {
            let result = functions::get_message_properties(chat_id, message_id, client_id).await;
            let editable = match result {
                Ok(enums::MessageProperties::MessageProperties(p)) => Some(p.can_be_edited),
                Err(e) => {
                    let _ = tx.send(TgEvent::Error(e.message));
                    None
                }
            };
            let mut text = None;
            if editable == Some(true)
                && let Ok(enums::Message::Message(message)) =
                    functions::get_message(chat_id, message_id, client_id).await
            {
                text = editable_text(&message.content).and_then(to_markdown);
            }
            let _ = tx.send(TgEvent::Editable {
                chat_id,
                message_id,
                editable,
                text,
            });
        });
    }

    /// Replaces a text message's text, with its Markdown made formatting.
    /// TDLib then sends `updateMessageContent` and `updateMessageEdited`.
    pub fn edit_text(&self, chat_id: i64, message_id: i64, text: String) {
        let content = enums::InputMessageContent::InputMessageText(types::InputMessageText {
            text: markdown(text),
            link_preview_options: None,
            clear_draft: false,
        });
        self.spawn(functions::edit_message_text(
            chat_id,
            message_id,
            content,
            self.client_id,
        ));
    }

    /// Replaces the caption of a photo, video or file; empty removes it.
    /// `above` keeps it over the media.
    pub fn edit_caption(&self, chat_id: i64, message_id: i64, text: String, above: bool) {
        self.spawn(functions::edit_message_caption(
            chat_id,
            message_id,
            Some(markdown(text)),
            above,
            self.client_id,
        ));
    }

    /// Deletes a message for everyone in the chat (`revoke`), or only for
    /// you. TDLib confirms with `updateDeleteMessages`.
    pub fn delete_message(&self, chat_id: i64, message_id: i64, revoke: bool) {
        self.spawn(functions::delete_messages(
            chat_id,
            vec![message_id],
            revoke,
            self.client_id,
        ));
    }

    /// Sends a message, its Markdown made formatting (see [`markdown`]), as
    /// a reply to message `reply_to` if given. TDLib first reports it with a
    /// temporary id (`updateNewMessage`), then `updateMessageSendSucceeded`
    /// or `…Failed`.
    pub fn send_text(&self, chat_id: i64, text: String, reply_to: Option<i64>) {
        self.send_formatted(chat_id, markdown(text), reply_to);
    }

    /// Sends text as it is, without making its Markdown formatting: a
    /// bot's reply button sends its words exactly.
    pub fn send_plain(&self, chat_id: i64, text: String, reply_to: Option<i64>) {
        self.send_formatted(chat_id, plain(text), reply_to);
    }

    fn send_formatted(&self, chat_id: i64, text: types::FormattedText, reply_to: Option<i64>) {
        let content = enums::InputMessageContent::InputMessageText(types::InputMessageText {
            text,
            link_preview_options: None,
            clear_draft: true,
        });
        let reply_to = reply_to.map(reply_to_message);
        self.spawn(functions::send_message(
            chat_id,
            None,
            reply_to,
            None,
            content,
            self.client_id,
        ));
    }

    /// Sends a sticker, as a reply to message `reply_to` if given.
    pub fn send_sticker(&self, chat_id: i64, sticker: &Sticker, reply_to: Option<i64>) {
        let content = enums::InputMessageContent::InputMessageSticker(types::InputMessageSticker {
            sticker: enums::InputFile::Id(types::InputFileId {
                id: sticker.file_id,
            }),
            thumbnail: None,
            width: sticker.width,
            height: sticker.height,
            emoji: sticker.emoji.clone(),
        });
        let reply_to = reply_to.map(reply_to_message);
        self.spawn(functions::send_message(
            chat_id,
            None,
            reply_to,
            None,
            content,
            self.client_id,
        ));
    }

    /// Asks for the stickers of a tab of the sticker panel.
    pub fn stickers(&self, source: Source) {
        let tx = self.tx.clone();
        let client_id = self.client_id;
        tokio::spawn(async move {
            let result = match source {
                Source::Recent => functions::get_recent_stickers(false, client_id)
                    .await
                    .map(|enums::Stickers::Stickers(s)| s.stickers),
                Source::Favorites => functions::get_favorite_stickers(client_id)
                    .await
                    .map(|enums::Stickers::Stickers(s)| s.stickers),
                Source::Set(id) => functions::get_sticker_set(id, client_id)
                    .await
                    .map(|enums::StickerSet::StickerSet(s)| s.stickers),
            };
            let stickers = match result {
                Ok(stickers) => Some(stickers),
                Err(e) => {
                    let _ = tx.send(TgEvent::Error(e.message));
                    None
                }
            };
            let _ = tx.send(TgEvent::Stickers { source, stickers });
        });
    }

    /// Asks for the sticker sets you added, for the sticker panel's tabs.
    pub fn sticker_sets(&self) {
        let tx = self.tx.clone();
        let client_id = self.client_id;
        tokio::spawn(async move {
            let result =
                functions::get_installed_sticker_sets(enums::StickerType::Regular, client_id).await;
            let sets = match result {
                Ok(enums::StickerSets::StickerSets(s)) => {
                    Some(s.sets.into_iter().map(|set| (set.id, set.title)).collect())
                }
                Err(e) => {
                    let _ = tx.send(TgEvent::Error(e.message));
                    None
                }
            };
            let _ = tx.send(TgEvent::StickerSets(sets));
        });
    }

    /// Finds stickers by emoji, or by a word their emoji or set are known
    /// by: your own first, then others Telegram suggests.
    pub fn find_stickers(&self, chat_id: i64, query: String) {
        let tx = self.tx.clone();
        let client_id = self.client_id;
        tokio::spawn(async move {
            let result = functions::get_stickers(
                enums::StickerType::Regular,
                query.clone(),
                STICKER_SEARCH_LIMIT,
                chat_id,
                client_id,
            )
            .await;
            let stickers = match result {
                Ok(enums::Stickers::Stickers(s)) => Some(s.stickers),
                Err(e) => {
                    let _ = tx.send(TgEvent::Error(e.message));
                    None
                }
            };
            let _ = tx.send(TgEvent::StickersFound { query, stickers });
        });
    }

    /// Sends files in order, one album per group (a group of one is a plain
    /// message), each request waiting for the one before so they arrive in
    /// that order. The caption goes under the first file, and the first
    /// group answers message `reply_to` if given. Uploads continue after
    /// TDLib reports the messages, like a text message's sending.
    pub fn send_files(
        &self,
        chat_id: i64,
        groups: Vec<Vec<Upload>>,
        caption: String,
        reply_to: Option<i64>,
    ) {
        let tx = self.tx.clone();
        let client_id = self.client_id;
        let mut caption = (!caption.is_empty()).then(|| markdown(caption));
        tokio::spawn(async move {
            let mut reply_to = reply_to.map(reply_to_message);
            for group in groups {
                let mut contents: Vec<_> = group
                    .into_iter()
                    .map(|upload| upload.content(caption.take()))
                    .collect();
                let reply_to = reply_to.take();
                let result = if contents.len() == 1 {
                    let content = contents.remove(0);
                    functions::send_message(chat_id, None, reply_to, None, content, client_id)
                        .await
                        .map(drop)
                } else {
                    functions::send_message_album(
                        chat_id, None, reply_to, None, contents, client_id,
                    )
                    .await
                    .map(drop)
                };
                // What failed once (no right to send media, say) fails again.
                if let Err(e) = result {
                    let _ = tx.send(TgEvent::Error(e.message));
                    break;
                }
            }
        });
    }

    /// Forwards messages of chat `from_chat_id`, in order, to chat
    /// `chat_id`. They arrive there as new messages.
    pub fn forward(&self, chat_id: i64, from_chat_id: i64, message_ids: Vec<i64>) {
        let tx = self.tx.clone();
        let client_id = self.client_id;
        tokio::spawn(async move {
            let result = functions::forward_messages(
                chat_id,
                None,
                from_chat_id,
                message_ids,
                None,
                false, // send_copy: keep "Forwarded from"
                false, // remove_caption
                client_id,
            )
            .await;
            let _ = match result {
                Ok(_) => tx.send(TgEvent::Forwarded { chat_id }),
                Err(e) => tx.send(TgEvent::Error(e.message)),
            };
        });
    }

    /// Searches your contacts, and Telegram's public chats, for the `s`
    /// picker. TDLib sends the users and chats it finds as updates first.
    pub fn find_chats(&self, query: String) {
        let tx = self.tx.clone();
        let client_id = self.client_id;
        tokio::spawn(async move {
            let user_ids =
                match functions::search_contacts(query.clone(), CONTACT_SEARCH_LIMIT, client_id)
                    .await
                {
                    Ok(enums::Users::Users(u)) => u.user_ids,
                    Err(_) => Vec::new(),
                };
            // Too short a query, or one searched too often, is an error;
            // either way nothing was found.
            let chat_ids = match functions::search_public_chats(query.clone(), client_id).await {
                Ok(enums::Chats::Chats(c)) => c.chat_ids,
                Err(_) => Vec::new(),
            };
            let _ = tx.send(TgEvent::ChatsFound {
                query,
                chat_ids,
                user_ids,
            });
        });
    }

    /// Opens a chat with one of your contacts, creating it if there's none.
    pub fn find_private_chat(&self, user_id: i64, request: String) {
        let client_id = self.client_id;
        self.find(request, async move {
            let enums::Chat::Chat(chat) = functions::create_private_chat(user_id, false, client_id)
                .await
                .map_err(|e| Missed::Said(e.message))?;
            Ok((chat.id, None))
        });
    }

    /// Finds the chat with a username: a person, bot, group or channel.
    pub fn find_username(&self, username: String, request: String) {
        let client_id = self.client_id;
        self.find(request, async move {
            let id = public_chat(&username, client_id).await?;
            Ok((id, None))
        });
    }

    /// Follows a t.me link: to a chat, to a message in one, or an invite.
    /// An invite to a chat you're not in comes back as [`TgEvent::Invite`],
    /// to ask first.
    pub fn find_link(&self, link: String, request: String) {
        let tx = self.tx.clone();
        let client_id = self.client_id;
        self.find(request.clone(), async move {
            let kind = functions::get_internal_link_type(link.clone(), client_id)
                .await
                .map_err(|_| Missed::Elsewhere)?;
            match kind {
                enums::InternalLinkType::PublicChat(p) => {
                    Ok((public_chat(&p.chat_username, client_id).await?, None))
                }
                enums::InternalLinkType::Message(m) => {
                    let enums::MessageLinkInfo::MessageLinkInfo(info) =
                        functions::get_message_link_info(m.url, client_id)
                            .await
                            .map_err(|e| Missed::Said(e.message))?;
                    if info.chat_id == 0 {
                        return Err(Missed::Said("That message can't be found".into()));
                    }
                    Ok((info.chat_id, info.message.map(|m| m.id)))
                }
                enums::InternalLinkType::ChatInvite(i) => {
                    let enums::ChatInviteLinkInfo::ChatInviteLinkInfo(info) =
                        functions::check_chat_invite_link(i.invite_link.clone(), client_id)
                            .await
                            .map_err(|e| Missed::Said(e.message))?;
                    // A chat you're in has an id and no time limit on reading it.
                    if info.chat_id != 0 && info.accessible_for == 0 {
                        return Ok((info.chat_id, None));
                    }
                    let invite = Invite {
                        title: crate::text::clean(&info.title),
                        members: info.member_count,
                        channel: matches!(info.r#type, enums::InviteLinkChatType::Channel),
                        badge: Badge::of(info.verification_status.as_ref(), false),
                        by_request: info.creates_join_request,
                    };
                    let _ = tx.send(TgEvent::Invite {
                        request,
                        link: i.invite_link,
                        invite,
                    });
                    Err(Missed::Quiet)
                }
                _ => Err(Missed::Elsewhere),
            }
        });
    }

    /// Joins a chat with an invite link, then opens it.
    pub fn join_by_link(&self, link: String, request: String) {
        let client_id = self.client_id;
        self.find(request, async move {
            let enums::Chat::Chat(chat) = functions::join_chat_by_invite_link(link, client_id)
                .await
                .map_err(|e| Missed::Said(e.message))?;
            Ok((chat.id, None))
        });
    }

    /// Joins a public group or channel. TDLib then sends `updateSupergroup`.
    pub fn join_chat(&self, chat_id: i64) {
        let tx = self.tx.clone();
        let client_id = self.client_id;
        tokio::spawn(async move {
            let _ = match functions::join_chat(chat_id, client_id).await {
                Ok(()) => tx.send(TgEvent::Joined { chat_id }),
                Err(e) => tx.send(TgEvent::Error(e.message)),
            };
        });
    }

    /// Searches a group's members by name, for `@` completion. TDLib sends
    /// the users first.
    pub fn find_members(&self, chat_id: i64, query: String) {
        let tx = self.tx.clone();
        let client_id = self.client_id;
        tokio::spawn(async move {
            let result = functions::search_chat_members(
                chat_id,
                query.clone(),
                MEMBER_SEARCH_LIMIT,
                None,
                client_id,
            )
            .await;
            let user_ids = match result {
                Ok(enums::ChatMembers::ChatMembers(m)) => m
                    .members
                    .into_iter()
                    .filter_map(|m| match m.member_id {
                        enums::MessageSender::User(u) => Some(u.user_id),
                        enums::MessageSender::Chat(_) => None,
                    })
                    .collect(),
                Err(_) => Vec::new(),
            };
            let _ = tx.send(TgEvent::Members {
                chat_id,
                query,
                user_ids,
            });
        });
    }

    /// Asks what commands the bots of a chat take: the bot a private chat is
    /// with, or the bots in a group.
    pub fn bot_commands(&self, chat_id: i64, peer: Peer) {
        let tx = self.tx.clone();
        let client_id = self.client_id;
        tokio::spawn(async move {
            let by_bot: Vec<(i64, Vec<types::BotCommand>)> = match peer {
                Peer::User(id) => match functions::get_user_full_info(id, client_id).await {
                    Ok(enums::UserFullInfo::UserFullInfo(info)) => info
                        .bot_info
                        .map(|b| vec![(id, b.commands)])
                        .unwrap_or_default(),
                    Err(_) => Vec::new(),
                },
                Peer::BasicGroup(id) => {
                    match functions::get_basic_group_full_info(id, client_id).await {
                        Ok(enums::BasicGroupFullInfo::BasicGroupFullInfo(info)) => info
                            .bot_commands
                            .into_iter()
                            .map(|b| (b.bot_user_id, b.commands))
                            .collect(),
                        Err(_) => Vec::new(),
                    }
                }
                Peer::Supergroup(id) => {
                    match functions::get_supergroup_full_info(id, client_id).await {
                        Ok(enums::SupergroupFullInfo::SupergroupFullInfo(info)) => info
                            .bot_commands
                            .into_iter()
                            .map(|b| (b.bot_user_id, b.commands))
                            .collect(),
                        Err(_) => Vec::new(),
                    }
                }
            };
            // A bot writes these, so they're cleaned like any message.
            let line = |text: &str| {
                crate::text::clean(text)
                    .split_whitespace()
                    .collect::<Vec<_>>()
                    .join(" ")
            };
            let commands = by_bot
                .into_iter()
                .flat_map(|(bot, commands)| {
                    commands.into_iter().map(move |c| crate::complete::Command {
                        bot,
                        name: line(&c.command),
                        description: line(&c.description),
                    })
                })
                .collect();
            let _ = tx.send(TgEvent::Commands { chat_id, commands });
        });
    }

    /// Pins a chat to the top of a list, or unpins it. TDLib then sends
    /// `updateChatPosition`; past Telegram's limit it's an error.
    pub fn pin_chat(&self, list: List, chat_id: i64, pinned: bool) {
        self.spawn(functions::toggle_chat_is_pinned(
            list.tdlib(),
            chat_id,
            pinned,
            self.client_id,
        ));
    }

    /// Changes how a chat notifies (`m` mutes it). TDLib then sends
    /// `updateChatNotificationSettings`.
    pub fn set_notifications(&self, chat_id: i64, settings: types::ChatNotificationSettings) {
        self.spawn(functions::set_chat_notification_settings(
            chat_id,
            settings,
            self.client_id,
        ));
    }

    /// Votes in a poll for these answers, by index; none takes your vote
    /// back. TDLib then sends the poll's new counts.
    pub fn vote(&self, chat_id: i64, message_id: i64, answers: Vec<i32>) {
        self.spawn(functions::set_poll_answer(
            chat_id,
            message_id,
            answers,
            self.client_id,
        ));
    }

    /// Presses a bot's button on its message, sending the bot the button's
    /// data. Its answer comes back as [`TgEvent::BotAnswer`]; one that
    /// doesn't come in time is an error.
    pub fn press_button(&self, chat_id: i64, message_id: i64, data: String, label: String) {
        let tx = self.tx.clone();
        let client_id = self.client_id;
        tokio::spawn(async move {
            let payload =
                enums::CallbackQueryPayload::Data(types::CallbackQueryPayloadData { data });
            let answer =
                functions::get_callback_query_answer(chat_id, message_id, payload, client_id).await;
            let _ = match answer {
                Ok(enums::CallbackQueryAnswer::CallbackQueryAnswer(a)) => {
                    tx.send(TgEvent::BotAnswer {
                        chat_id,
                        label,
                        text: a.text,
                        alert: a.show_alert,
                        url: a.url,
                    })
                }
                Err(e) => tx.send(TgEvent::Error(e.message)),
            };
        });
    }

    /// Leaves a group or channel. TDLib then takes it out of the list.
    pub fn leave_chat(&self, chat_id: i64) {
        let tx = self.tx.clone();
        let client_id = self.client_id;
        tokio::spawn(async move {
            let _ = match functions::leave_chat(chat_id, client_id).await {
                Ok(()) => tx.send(TgEvent::Left { chat_id }),
                Err(e) => tx.send(TgEvent::Error(e.message)),
            };
        });
    }

    /// Runs a lookup of a chat to open, and reports it as
    /// [`TgEvent::ChatFound`].
    fn find(
        &self,
        request: String,
        lookup: impl Future<Output = Result<(i64, Option<i64>), Missed>> + Send + 'static,
    ) {
        let tx = self.tx.clone();
        tokio::spawn(async move {
            let found = lookup.await;
            let _ = tx.send(TgEvent::ChatFound { request, found });
        });
    }

    /// Tells the chat you're typing, or that you stopped. Others see it for
    /// about 5 seconds unless it's sent again. Failing is harmless (TDLib
    /// already skips chats you can't write in), so errors aren't shown.
    pub fn send_typing(&self, chat_id: i64, typing: bool) {
        let action = typing.then_some(enums::ChatAction::Typing);
        let client_id = self.client_id;
        tokio::spawn(async move {
            let _ = functions::send_chat_action(chat_id, None, action, client_id).await;
        });
    }

    /// Downloads a file into TDLib's files directory, or returns at once if it's
    /// already there.
    pub fn download(&self, file_id: i32) {
        self.fetch_file(file_id, DOWNLOAD_PRIORITY, true);
    }

    /// Like [`download`](Self::download), for files nobody asked for (chat
    /// photos): they wait behind other downloads, and a failure isn't reported.
    pub fn download_quiet(&self, file_id: i32) {
        self.fetch_file(file_id, QUIET_DOWNLOAD_PRIORITY, false);
    }

    fn fetch_file(&self, file_id: i32, priority: i32, report_errors: bool) {
        let tx = self.tx.clone();
        let client_id = self.client_id;
        tokio::spawn(async move {
            // synchronous = true: answer only once the whole file is on disk.
            let result = functions::download_file(file_id, priority, 0, 0, true, client_id).await;
            let path = match result {
                Ok(enums::File::File(f)) if f.local.is_downloading_completed => Some(f.local.path),
                Ok(_) => None,
                Err(e) => {
                    if report_errors {
                        let _ = tx.send(TgEvent::Error(e.message));
                    }
                    None
                }
            };
            let _ = tx.send(TgEvent::Downloaded { file_id, path });
        });
    }

    /// Shows the user as online to others, or not. While online, TDLib also
    /// sends notifications without waiting to see if the phone reads the
    /// message first.
    pub fn set_online(&self, online: bool) {
        let value = enums::OptionValue::Boolean(types::OptionValueBoolean { value: online });
        self.set_option("online", value);
    }

    /// Turns on TDLib's notifications (`updateNotificationGroup`), which are
    /// off until a client says how many chats it shows at once.
    pub fn enable_notifications(&self) {
        let value = enums::OptionValue::Integer(types::OptionValueInteger {
            value: NOTIFICATION_GROUPS,
        });
        self.set_option("notification_group_count_max", value);
    }

    fn set_option(&self, name: &str, value: enums::OptionValue) {
        self.spawn(functions::set_option(
            name.into(),
            Some(value),
            self.client_id,
        ));
    }

    /// Marks messages as read, which also sends read receipts.
    pub fn view_messages(&self, chat_id: i64, message_ids: Vec<i64>) {
        self.spawn(functions::view_messages(
            chat_id,
            message_ids,
            None,
            true,
            self.client_id,
        ));
    }

    /// Flushes TDLib's database and ends with `authorizationStateClosed`.
    pub fn close(&self) {
        let client_id = self.client_id;
        self.spawn(async move {
            // Others see you go offline now, not minutes later. Before login
            // this fails, which is fine.
            let offline = enums::OptionValue::Boolean(types::OptionValueBoolean { value: false });
            let _ = functions::set_option("online".into(), Some(offline), client_id).await;
            functions::close(client_id).await
        });
    }

    /// Ends the session and deletes TDLib's database, ending with
    /// `authorizationStateClosed`. Before login finishes, it's the only way out
    /// of a QR login: TDLib takes no phone number in that state, and keeps the
    /// state across restarts.
    pub fn log_out(&self) {
        self.spawn(functions::log_out(self.client_id));
    }

    /// Replaces a closed client with a new one, which starts over at
    /// `authorizationStateWaitTdlibParameters`.
    pub fn reopen(&mut self) {
        self.client_id = tdlib_rs::create_client();
        CLIENT.store(self.client_id, Ordering::Relaxed);
        self.tx.client_id = self.client_id;
        // A new client sends no updates until it gets its first request.
        self.spawn(functions::get_option("version".into(), self.client_id));
    }

    fn spawn<T: Send + 'static>(
        &self,
        request: impl Future<Output = Result<T, types::Error>> + Send + 'static,
    ) {
        let tx = self.tx.clone();
        tokio::spawn(async move {
            if let Err(e) = request.await {
                let _ = tx.send(TgEvent::Error(e.message));
            }
        });
    }
}

/// A file to send, and how Telegram shows it.
pub enum Upload {
    /// Compressed and shown in the chat. The size is the image's, for TDLib
    /// to lay it out before the upload is done.
    Photo {
        path: String,
        width: i32,
        height: i32,
    },
    /// As it is, with its name.
    File { path: String },
}

impl Upload {
    fn content(self, caption: Option<types::FormattedText>) -> enums::InputMessageContent {
        let local = |path| enums::InputFile::Local(types::InputFileLocal { path });
        match self {
            Upload::Photo {
                path,
                width,
                height,
            } => enums::InputMessageContent::InputMessagePhoto(types::InputMessagePhoto {
                photo: local(path),
                thumbnail: None,
                added_sticker_file_ids: Vec::new(),
                width,
                height,
                caption,
                show_caption_above_media: false,
                self_destruct_type: None,
                has_spoiler: false,
            }),
            Upload::File { path } => {
                enums::InputMessageContent::InputMessageDocument(types::InputMessageDocument {
                    document: local(path),
                    thumbnail: None,
                    // Telegram may then show a song or video as one.
                    disable_content_type_detection: false,
                    caption,
                })
            }
        }
    }
}

/// The chat with a username, or why there's none.
async fn public_chat(username: &str, client_id: i32) -> Result<i64, String> {
    match functions::search_public_chat(username.to_string(), client_id).await {
        Ok(enums::Chat::Chat(chat)) => Ok(chat.id),
        Err(e) if e.code == 400 => Err(format!(
            "Nobody on Telegram is called @{}",
            crate::text::clean(username)
        )),
        Err(e) => Err(e.message),
    }
}

fn reply_to_message(message_id: i64) -> enums::InputMessageReplyTo {
    enums::InputMessageReplyTo::Message(types::InputMessageReplyToMessage {
        message_id,
        quote: None,
        checklist_task_id: 0,
    })
}

/// Text without formatting. Telegram still finds links, mentions and the
/// like in it by itself.
pub fn plain(text: String) -> types::FormattedText {
    types::FormattedText {
        text,
        entities: Vec::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Without `log_to_file`, TDLib prints every request it runs here.
    fn quiet() {
        let request = json!({ "@type": "setLogVerbosityLevel", "new_verbosity_level": 1 });
        execute(&request).unwrap();
    }

    fn entities(text: &types::FormattedText) -> Vec<(i32, i32, String)> {
        text.entities
            .iter()
            .map(|e| {
                let kind = format!("{:?}", e.r#type);
                (e.offset, e.length, kind)
            })
            .collect()
    }

    #[test]
    fn markdown_in_the_composer_is_sent_as_formatting() {
        quiet();
        let sent =
            markdown("**bold** __it__ ~~gone~~ ||secret|| `code` [site](https://x.dev)".into());
        assert_eq!(sent.text, "bold it gone secret code site");
        assert_eq!(
            entities(&sent),
            [
                (0, 4, "Bold".into()),
                (5, 2, "Italic".into()),
                (8, 4, "Strikethrough".into()),
                (13, 6, "Spoiler".into()),
                (20, 4, "Code".into()),
                (
                    25,
                    4,
                    "TextUrl(TextEntityTypeTextUrl { url: \"https://x.dev/\" })".into()
                ),
            ]
        );
    }

    #[test]
    fn a_markdown_link_to_a_user_mentions_them() {
        quiet();
        let sent = markdown("hi [Bob Smith](tg://user?id=123)".into());
        assert_eq!(sent.text, "hi Bob Smith");
        assert_eq!(
            entities(&sent),
            [(
                3,
                9,
                "MentionName(TextEntityTypeMentionName { user_id: 123 })".into()
            )]
        );
    }

    #[test]
    fn single_stars_and_underscores_stay_as_typed() {
        quiet();
        let sent = markdown("2*3*4 = 24, snake_case_name, a ** b".into());
        assert_eq!(sent.text, "2*3*4 = 24, snake_case_name, a ** b");
        assert!(sent.entities.is_empty(), "{:?}", sent.entities);
    }

    #[test]
    fn edits_start_from_the_message_as_markdown() {
        quiet();
        let entity = |offset, length, r#type| types::TextEntity {
            offset,
            length,
            r#type,
        };
        let bold = types::FormattedText {
            text: "bold and plain".into(),
            entities: vec![entity(0, 4, enums::TextEntityType::Bold)],
        };
        let edit = to_markdown(&bold).unwrap();
        assert_eq!(edit.markdown, "**bold** and plain");
        assert!(!edit.loses);
        // Saving it gives the same message back.
        assert_eq!(markdown(edit.markdown), bold);

        let underlined = types::FormattedText {
            text: "under".into(),
            entities: vec![entity(0, 5, enums::TextEntityType::Underline)],
        };
        assert!(
            to_markdown(&underlined).unwrap().loses,
            "Markdown has no underline"
        );

        let mention = types::FormattedText {
            text: "hi Bob".into(),
            entities: vec![entity(
                3,
                3,
                enums::TextEntityType::MentionName(types::TextEntityTypeMentionName {
                    user_id: 123,
                }),
            )],
        };
        let edit = to_markdown(&mention).unwrap();
        assert_eq!(edit.markdown, "hi [Bob](tg://user?id=123)");
        assert!(!edit.loses);
        assert_eq!(markdown(edit.markdown), mention);
    }

    #[test]
    fn openssl_is_told_not_to_read_a_config_file() {
        super::no_openssl_config().unwrap();
        // Again, as after a logout's new client: still fine.
        super::no_openssl_config().unwrap();
    }
}

//! Thin wrapper around one TDLib client.
//!
//! A background thread pulls everything out of TDLib: updates are forwarded to
//! the app as [`TgEvent`]s, and responses complete the pending request futures.
//! Each request runs as its own tokio task so the UI never waits on the network.

use std::ffi::{CStr, CString, c_char};
use std::future::Future;
use std::path::Path;
use std::sync::Arc;

use anyhow::{Result, anyhow, bail};
use serde_json::json;
use tdlib_rs::{enums, functions, types};
use tokio::sync::mpsc::UnboundedSender;
use tokio::sync::mpsc::error::SendError;

use crate::config::{ApiKeys, Config};
use crate::reactions::{self, Available, ReactionKind};
use crate::stickers::{Source, Sticker};

pub enum TgEvent {
    Update(Box<enums::Update>),
    /// A request failed. The text is TDLib's, e.g. `PHONE_CODE_INVALID`.
    Error(String),
    /// A `loadChats` call finished; `all` is true once every chat is loaded.
    /// `failed` if TDLib answered with an error (sent before as `Error`).
    ChatsLoaded {
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
    /// Who a message can be deleted for; `None` if TDLib couldn't say.
    Deletable {
        chat_id: i64,
        message_id: i64,
        deletable: Option<Deletable>,
    },
    /// Whether a message can be edited; `None` if TDLib couldn't say.
    Editable {
        chat_id: i64,
        message_id: i64,
        editable: Option<bool>,
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

/// Stickers a search in the sticker panel asks for.
const STICKER_SEARCH_LIMIT: i32 = 100;

unsafe extern "C" {
    /// TDLib's synchronous entry point, for the few requests that need no
    /// client. It's linked in with tdlib-rs, which doesn't wrap it.
    fn td_execute(request: *const c_char) -> *const c_char;
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
        let request = CString::new(request.to_string())?;
        // SAFETY: `request` is a NUL-terminated string that outlives the call.
        // TDLib returns a NUL-terminated answer (or null) that stays valid
        // until the next `td_execute` call, and it's copied out before then.
        let response = unsafe {
            let response = td_execute(request.as_ptr());
            if response.is_null() {
                String::new()
            } else {
                CStr::from_ptr(response).to_string_lossy().into_owned()
            }
        };
        if !response.contains(r#""@type":"ok""#) {
            bail!("TDLib log setup failed: {response}");
        }
    }
    Ok(())
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
        log_to_file(&config.data_dir.join("tdlib.log"))?;
        let client_id = tdlib_rs::create_client();

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

    /// Asks TDLib for the next `limit` chats of the main list. They arrive as
    /// `updateNewChat`/`updateChatPosition` updates, not in the response.
    pub fn load_chats(&self, limit: i32) {
        let tx = self.tx.clone();
        let client_id = self.client_id;
        tokio::spawn(async move {
            let (all, failed) =
                match functions::load_chats(Some(enums::ChatList::Main), limit, client_id).await {
                    Ok(()) => (false, false),
                    // 404 is TDLib's way of saying there are no more chats.
                    Err(e) if e.code == 404 => (true, false),
                    Err(e) => {
                        let _ = tx.send(TgEvent::Error(e.message));
                        (false, true)
                    }
                };
            let _ = tx.send(TgEvent::ChatsLoaded { all, failed });
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

    /// Asks whether a message can be edited: TDLib knows whose it is, and
    /// how long the chat allows edits for.
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
            let _ = tx.send(TgEvent::Editable {
                chat_id,
                message_id,
                editable,
            });
        });
    }

    /// Replaces a text message's text. TDLib then sends
    /// `updateMessageContent` and `updateMessageEdited`.
    pub fn edit_text(&self, chat_id: i64, message_id: i64, text: String) {
        let content = enums::InputMessageContent::InputMessageText(types::InputMessageText {
            text: plain(text),
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
            Some(plain(text)),
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

    /// Sends a plain-text message, as a reply to message `reply_to` if given.
    /// TDLib first reports it with a temporary id (`updateNewMessage`), then
    /// `updateMessageSendSucceeded` or `…Failed`.
    pub fn send_text(&self, chat_id: i64, text: String, reply_to: Option<i64>) {
        let content = enums::InputMessageContent::InputMessageText(types::InputMessageText {
            text: plain(text),
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
        tokio::spawn(async move {
            let mut caption = (!caption.is_empty()).then(|| plain(caption));
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

fn reply_to_message(message_id: i64) -> enums::InputMessageReplyTo {
    enums::InputMessageReplyTo::Message(types::InputMessageReplyToMessage {
        message_id,
        quote: None,
        checklist_task_id: 0,
    })
}

/// Text without formatting. Telegram still finds links, mentions and the
/// like in it by itself.
fn plain(text: String) -> types::FormattedText {
    types::FormattedText {
        text,
        entities: Vec::new(),
    }
}

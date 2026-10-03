//! Thin wrapper around one TDLib client.
//!
//! A background thread pulls everything out of TDLib: updates are forwarded to
//! the app as [`TgEvent`]s, and responses complete the pending request futures.
//! Each request runs as its own tokio task so the UI never waits on the network.

use std::future::Future;
use std::sync::Arc;

use anyhow::{Result, anyhow};
use tdlib_rs::{enums, functions, types};
use tokio::sync::mpsc::UnboundedSender;

use crate::config::Config;

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

/// TDLib download priority, 1 (lowest) to 32. Photos on screen matter.
const DOWNLOAD_PRIORITY: i32 = 16;

#[derive(Clone)]
pub struct Tg {
    client_id: i32,
    tx: UnboundedSender<TgEvent>,
    config: Arc<Config>,
}

impl Tg {
    pub async fn start(config: Config, tx: UnboundedSender<TgEvent>) -> Result<Self> {
        let client_id = tdlib_rs::create_client();

        // `receive` blocks for up to 2s at a time, so it gets a plain thread.
        // It stops once the app drops its end of the channel.
        std::thread::spawn({
            let tx = tx.clone();
            move || {
                loop {
                    if let Some((update, _)) = tdlib_rs::receive()
                        && tx.send(TgEvent::Update(Box::new(update))).is_err()
                    {
                        break;
                    }
                }
            }
        });

        // TDLib logs to stderr by default, which would draw over the TUI. This
        // first request also wakes the client up: it sends no updates before one.
        let log_path = config.data_dir.join("tdlib.log");
        functions::set_log_stream(
            enums::LogStream::File(types::LogStreamFile {
                path: log_path.to_string_lossy().into_owned(),
                max_file_size: 10 * 1024 * 1024,
                redirect_stderr: false,
            }),
            client_id,
        )
        .await
        .map_err(|e| anyhow!("TDLib log setup failed: {}", e.message))?;
        functions::set_log_verbosity_level(2, client_id)
            .await
            .map_err(|e| anyhow!("TDLib log setup failed: {}", e.message))?;

        Ok(Self {
            client_id,
            tx,
            config: Arc::new(config),
        })
    }

    pub fn set_tdlib_parameters(&self) {
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
                config.api_id,
                config.api_hash.clone(),
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

    /// Sends a plain-text message. TDLib first reports it with a temporary id
    /// (`updateNewMessage`), then `updateMessageSendSucceeded` or `…Failed`.
    pub fn send_text(&self, chat_id: i64, text: String) {
        let content = enums::InputMessageContent::InputMessageText(types::InputMessageText {
            text: types::FormattedText {
                text,
                entities: Vec::new(),
            },
            link_preview_options: None,
            clear_draft: true,
        });
        self.spawn(functions::send_message(
            chat_id,
            None,
            None,
            None,
            content,
            self.client_id,
        ));
    }

    /// Downloads a file into TDLib's files directory, or returns at once if it's
    /// already there.
    pub fn download(&self, file_id: i32) {
        let tx = self.tx.clone();
        let client_id = self.client_id;
        tokio::spawn(async move {
            // synchronous = true: answer only once the whole file is on disk.
            let result =
                functions::download_file(file_id, DOWNLOAD_PRIORITY, 0, 0, true, client_id).await;
            let path = match result {
                Ok(enums::File::File(f)) if f.local.is_downloading_completed => Some(f.local.path),
                Ok(_) => None,
                Err(e) => {
                    let _ = tx.send(TgEvent::Error(e.message));
                    None
                }
            };
            let _ = tx.send(TgEvent::Downloaded { file_id, path });
        });
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
        self.spawn(functions::close(self.client_id));
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

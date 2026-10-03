# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

tuigram: a Telegram *user* client (not a bot) for the terminal, built on TDLib (`tdlib-rs`) and ratatui, with vim-style modes. Published on crates.io as `tuigram-cli` (the name `tuigram` was taken); the binary is `tuigram`.

## Never run the app against the real session

The data directory holds the user's logged-in Telegram session: the platform app-data dir (`tuigram --help` prints it), or `TG_DATA_DIR` if set (e.g. `./.tdlib`). Running the binary reads their chats and sends read receipts (`view_messages`) to real people. Do not `cargo run` to check a change; verify with tests that render into ratatui's `TestBackend` (see existing tests in `src/ui/`). Ask the user to run the app when a change needs to be seen live.

## Commands

```sh
cargo build
cargo test                       # all tests
cargo test search_matches        # tests whose name contains the string
cargo clippy --all-targets       # keep it warning-free
cargo fmt
cargo install --path .           # puts `tuigram` on PATH (~/.cargo/bin)
cargo package --list             # check what would be published
cargo publish --dry-run
```

The first build downloads a prebuilt TDLib and links it statically (`download-tdlib` + `static` features, `build.rs`). Static linking is deliberate: dynamic linking crashed the build script under clippy/rust-analyzer.

Config (`config.rs`): no API credentials ship with the app, and none may ever be committed (the repo is public, and anything in the published crate is public). They come from `TG_API_ID` / `TG_API_HASH`, else from `settings.toml`, else the login screen asks for them on first run (`LoginStep::ApiId` / `ApiHash`) before TDLib gets its parameters. `TG_DATA_DIR` overrides the data directory, which holds the TDLib database, downloaded files, `tdlib.log` and `settings.toml`. `main` loads `.env` from the working directory first, so dev overrides can live there (see `.env.example`).

## Architecture

**Event loop (`app.rs`, `App::run`).** One `tokio::select!` over: TDLib events, finished image encodes, terminal key/paste events, and the quit deadline. Each wake drains the TDLib backlog, then the loop redraws. All state lives in `App`; nothing else mutates it.

**TDLib wrapper (`tg.rs`).** A plain thread blocks on `tdlib_rs::receive()` and forwards updates as `TgEvent::Update`. Every request is spawned as its own tokio task and never awaited by the UI; results come back as `TgEvent`s (`History`, `Found`, `ChatsLoaded`, `Downloaded`, `Error`). Responses carry what was asked for (chat id, `Page`, query) so the app can drop ones that went stale while in flight. TDLib logs go to `tdlib.log`, set up with its synchronous `td_execute` (declared by hand, since tdlib-rs doesn't wrap it) before the first client exists; otherwise TDLib prints to stderr over the terminal.

**Chat list (`chats.rs`).** Rebuilt purely from TDLib updates (`updateNewChat`, `updateChatPosition`, …). Order is unread-first, then TDLib's per-list `order`; `refresh()` re-sorts once per frame when dirty and applies the `/` title filter. Selection is stored by chat id, not index, so it survives reordering. All chats get loaded in the background (loop on `ChatsLoaded`) because unread-first sorting needs them all.

**Open chat (`messages.rs`).** `OpenChat.messages` is a `BTreeMap` keyed by message id (chronological) and is always one unbroken stretch of history. `Page` (`Latest`/`Older`/`Newer`/`Around`) says what to fetch; `Latest` and `Around` replace the stretch, since they may not join up. `loading: Option<Page>` is the one request in flight, and pages for any other request are ignored. `at_newest` is false after jumping to an old message (search): new messages aren't inserted then, and moving down loads `Newer` pages. `selected: None` means "on the newest message, follow new arrivals". Scroll position is a `ScrollAnchor` (message id + line offset) so it survives older pages loading above.

Replies (`Msg.reply_to`) show the message they answer above their text: the loaded one if it's there, else one fetched with `getRepliedMessage` after the frame (`OpenChat::missing_replied`), cached in `OpenChat.replied` by the reply's id. `r` sets `OpenChat.reply`, which the next sent message answers. `gd` jumps from a reply to what it answers (`OpenChat::replied_jump`), pushing the reply onto `OpenChat.jumps` for Ctrl-o to return to.

TDLib `Message`s are converted to `Msg` once on arrival (`body()`): display text, inline `Preview`, the file Enter opens, and links. Only http(s) links are kept (so a crafted link can't open local files/apps), and TDLib entity offsets are UTF-16 code units, converted to byte ranges.

**Modes and keys.** `Focus::Chats` / `Focus::Messages` are Normal mode; `Focus::Input` is Insert mode (the composer). `on_key` routes to the first modal layer that's open, in this order: settings popup (`?`), delete popup (`d`), open-file menu, `/` search prompt, Insert mode, Normal mode. The status bar shows the mode and the key hints for the current state, so update the hints when adding keys.

**Search (`search.rs`).** `/` in the chat list filters titles live (local). `/` in a chat runs TDLib `searchChatMessages` over the whole history; `n`/`N` go to older/newer matches, and a match that isn't loaded is reached with an `Around` page. `search::find` gives case-insensitive byte ranges, used for both filtering and highlighting.

**Rendering (`ui/`).** `ui/messages.rs` lays out every loaded message into lines each frame (bubbles, wrapping, date separators, sender names except in channels), then picks the scroll window. Photos are not text: layout reserves blank rows (`PhotoSlot`) and `draw_photos` paints images over them afterwards.

**Images (`images.rs`).** Drawing calls `want()` for photos on screen; after the frame, `fetch()` starts TDLib downloads and decode/encode on blocking threads, which report back as `ImageEvent`s. The message's embedded blurry thumbnail is shown until the real image is ready. The terminal's image protocol and cell size are queried once at startup (`main.rs`), before key reading starts.

**Theme (`theme.rs`).** Drawing code uses `Colors` roles (`accent`, `own_bubble`, `search`, …), never Catppuccin palette names directly. Add a role when a new kind of thing needs a color.

## Conventions

- Rust 2024 edition; `let` chains are used freely.
- Comments explain why, in plain sentences; doc comments describe behavior as the user sees it.
- Test names are sentences (`own_messages_sit_on_the_right_and_others_on_the_left`). UI tests render into a `TestBackend` buffer and assert on rows/cells.
- tdlib-rs `Message` has no `Default`, so logic that takes messages is written to accept already-converted `Msg`s where it needs testing (see `OpenChat::add_page`).

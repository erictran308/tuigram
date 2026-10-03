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

Config (`config.rs`): API credentials may never be committed or end up in the published crate (both are public). Release binaries get tuigram's own key at compile time instead: `.github/workflows/release.yml` (run by pushing a `v*` tag) passes the `TUIGRAM_API_ID` / `TUIGRAM_API_HASH` repository secrets, and `build.rs` writes them XORed with a per-build random mask to `$OUT_DIR/built_in_keys.rs` (so the hash is never plain text in the binary), which `config::built_in_keys` unmasks; `cargo install` builds have none. `cargo binstall tuigram-cli` installs the release binary instead (`[package.metadata.binstall]` in `Cargo.toml`, with its from-source and QuickInstall fallbacks off, since those would have no key), so release asset names and the `v<version>` tag must stay as `pkg-url` expects, and a release must be up before `cargo publish`. At runtime the key comes from `TG_API_ID` / `TG_API_HASH`, else `settings.toml`, else the built-in one, else the login screen asks for one (`LoginStep::ApiId` / `ApiHash`) before TDLib gets its parameters. If Telegram rejects the key in use, the login screen asks for another and a new TDLib client starts with it (`App::reject_api_keys`). `TG_DATA_DIR` overrides the data directory, which holds the TDLib database, downloaded files, `tdlib.log` and `settings.toml`; it's set to 0700 on every start. `TG_*` settings can also come from `.env` in the working directory, for development (see `.env.example`): `config::load_dotenv` reads only that directory and only `TG_*` keys, into a map `config::var` falls back to, and never sets process environment variables (so a `.env` in an untrusted folder can't set `LD_PRELOAD` for the programs tuigram starts).

## Releasing

Release binaries carry tuigram's API key, so they're built only by `.github/workflows/release.yml`, from the `TUIGRAM_API_ID` / `TUIGRAM_API_HASH` repository secrets (Settings → Secrets and variables → Actions). Order matters: `cargo binstall` looks for the GitHub release of the version it finds on crates.io, so the release must exist before `cargo publish`.

1. Bump `version` in `Cargo.toml`, run `cargo build` (updates `Cargo.lock`), then tests, clippy, `cargo fmt --check` and `actionlint`.
2. Commit and push `main`.
3. `git tag -a vX.Y.Z -m vX.Y.Z && git push origin vX.Y.Z`. The workflow builds six targets (Linux/macOS/Windows × x86_64/ARM64) and, only if all succeed, creates the GitHub release with the archives, `SHA256SUMS` and a provenance attestation. About 15–20 minutes.
4. Once `releases/download/vX.Y.Z/SHA256SUMS` exists: `cargo publish`.

When `tdlib-rs` changes in `Cargo.lock`, update `TDLIB_RS_VERSION`, `TDLIB_VERSION` and every target's `tdlib_sha256` in the workflow (the zips are at `github.com/FedericoBruzzone/tdlib-rs/releases`); the build stops until they match. To build a binary with a key baked in locally, set the same variables: `TUIGRAM_API_ID=… TUIGRAM_API_HASH=… cargo build --release` (both or neither, or build.rs fails).

## Security

Everything another Telegram user sends is untrusted: text, names, chat titles, file names, links, files, images.
- Text that is shown or copied goes through `text::clean` (control characters other than `\n`/`\t`, bidi overrides). `Msg` text gets it in `normalize`, which keeps link ranges in step.
- Links and files open through `open_externally` (the `open` crate, ShellExecute on Windows), never a shell: `cmd /C start` would run what follows a `&` in a link. Downloaded files get the OS "from the internet" mark (`mark_downloaded`), and only `SAFE_TO_OPEN` types open without a `Confirm`; so do links whose text differs from their URL (`Link.disguise`).
- Images decode within `images::limits`. Byte slicing of message text uses `.get()` or ranges known to be on char boundaries.
- Read receipts go out only from `App::mark_seen`, when the chat pane and the terminal window have focus and the view is on the newest message. Don't call `view_messages` anywhere else.
- A panic restores the terminal and exits (hook in `main`), printing the message through `text::clean`.

## Architecture

**Event loop (`app.rs`, `App::run`).** One `tokio::select!` over: TDLib events, finished image encodes, terminal key/paste/focus events, and the quit deadline. Each wake drains the TDLib backlog, then the loop redraws. All state lives in `App`; nothing else mutates it.

**TDLib wrapper (`tg.rs`).** A plain thread blocks on `tdlib_rs::receive()` and forwards updates as `TgEvent::Update`. Every event is tagged with its client id (`tg::Tagged`), and `App::on_tagged` drops events from a client that has been replaced. Every request is spawned as its own tokio task and never awaited by the UI; results come back as `TgEvent`s (`History`, `Found`, `ChatsLoaded`, `Downloaded`, `Error`). Responses carry what was asked for (chat id, `Page`, query) so the app can drop ones that went stale while in flight. TDLib takes its parameters only at startup, so logging out (`:logout`, the session ending elsewhere, Esc out of a QR login) or switching API keys closes the client, and on `authorizationStateClosed` `Tg::reopen` starts a new one after `App::forget_session` drops the old session's state. TDLib logs go to `tdlib.log`, set up with its synchronous `td_execute` (declared by hand, since tdlib-rs doesn't wrap it) before the first client exists; otherwise TDLib prints to stderr over the terminal.

**Chat list (`chats.rs`).** Rebuilt purely from TDLib updates (`updateNewChat`, `updateChatPosition`, …). Order is unread-first, then TDLib's per-list `order`; `refresh()` re-sorts once per frame when dirty and applies the `/` title filter. Selection is stored by chat id, not index, so it survives reordering. All chats get loaded in the background (loop on `ChatsLoaded`) because unread-first sorting needs them all.

**Open chat (`messages.rs`).** `OpenChat.messages` is a `BTreeMap` keyed by message id (chronological) and is always one unbroken stretch of history. `Page` (`Latest`/`Older`/`Newer`/`Around`) says what to fetch; `Latest` and `Around` replace the stretch, since they may not join up. `loading: Option<Page>` is the one request in flight, and pages for any other request are ignored. `at_newest` is false after jumping to an old message (search): new messages aren't inserted then, and moving down loads `Newer` pages. `selected: None` means "on the newest message, follow new arrivals"; while it is, only the newest `MAX_FOLLOWED` messages stay loaded. Scroll position is a `ScrollAnchor` (message id + line offset) so it survives older pages loading above.

Replies (`Msg.reply_to`) show the message they answer above their text: the loaded one if it's there, else one fetched with `getRepliedMessage` after the frame (`OpenChat::missing_replied`), cached in `OpenChat.replied` by the reply's id. `r` sets `OpenChat.reply`, which the next sent message answers. `gd` jumps from a reply to what it answers (`OpenChat::replied_jump`), pushing the reply onto `OpenChat.jumps` for Ctrl-o to return to.

TDLib `Message`s are converted to `Msg` once on arrival (`body()`): display text, inline `Preview`, the file Enter opens, and links. Only http(s) links are kept (so a crafted link can't open local files/apps), and TDLib entity offsets are UTF-16 code units, converted to byte ranges.

**Modes and keys.** `Focus::Chats` / `Focus::Messages` are Normal mode; `Focus::Input` is Insert mode (the composer). `on_key` routes to the first modal layer that's open, in this order: the confirmation popup (`Confirm`, only `y` goes ahead), settings popup (`?`), delete popup (`d`), open-file menu, the status bar prompt (`/` search or `:` command, `PromptKind`), Insert mode, Normal mode. `:` commands (`app::Command`) only run by their full name, so a typo can't log anyone out. The status bar shows the mode and the key hints for the current state, and `?` lists every shortcut (`SHORTCUTS` in `ui/help.rs`), so update both when adding keys.

**Search (`search.rs`).** `/` in the chat list filters titles live (local). `/` in a chat runs TDLib `searchChatMessages` over the whole history; `n`/`N` go to older/newer matches, and a match that isn't loaded is reached with an `Around` page. `search::find` gives case-insensitive byte ranges, used for both filtering and highlighting.

**Rendering (`ui/`).** `ui/messages.rs` lays out every loaded message into lines each frame (bubbles, wrapping, date separators, sender names except in channels), then picks the scroll window. Photos are not text: layout reserves blank rows (`PhotoSlot`) and `draw_photos` paints images over them afterwards.

**Images (`images.rs`).** Drawing calls `want()` for photos on screen; after the frame, `fetch()` starts TDLib downloads and decode/encode on blocking threads, which report back as `ImageEvent`s. The message's embedded blurry thumbnail is shown until the real image is ready. The terminal's image protocol and cell size are queried once at startup (`main.rs`), before key reading starts.

**Clipboard (`clipboard.rs`).** `y` copies with arboard. Text falls back to an OSC 52 escape sequence when there's no system clipboard (e.g. over SSH). Media is downloaded through TDLib first (`App.copying`); photos are decoded on a blocking thread and come back as `Decoded` to be copied as images, other files are copied as file references. Tests never touch the real clipboard: it would overwrite whatever the user had copied.

**Theme (`theme.rs`).** Drawing code uses `Colors` roles (`accent`, `own_bubble`, `search`, …), never Catppuccin palette names directly. Add a role when a new kind of thing needs a color.

## Conventions

- Rust 2024 edition; `let` chains are used freely.
- Comments explain why, in plain sentences; doc comments describe behavior as the user sees it.
- Test names are sentences (`own_messages_sit_on_the_right_and_others_on_the_left`). UI tests render into a `TestBackend` buffer and assert on rows/cells.
- tdlib-rs `Message` has no `Default`, so logic that takes messages is written to accept already-converted `Msg`s where it needs testing (see `OpenChat::add_page`).

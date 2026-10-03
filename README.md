<div align="center">

# tuigram

**Telegram at the speed of your keyboard.**

A Telegram client for the terminal with vim keys, inline photos and search across your whole history.<br>
Written in Rust on [TDLib](https://github.com/tdlib/td), the library behind Telegram's own apps.

[![crates.io](https://img.shields.io/crates/v/tuigram-cli.svg)](https://crates.io/crates/tuigram-cli)
[![License: MIT](https://img.shields.io/badge/license-MIT-blue.svg)](LICENSE)

```sh
cargo install tuigram-cli
```

<!-- TODO: add a screenshot or GIF here, e.g. ![tuigram](docs/screenshot.png) -->

</div>

---

## Why tuigram

- **Your real account, not a bot.** Log in with your phone number, the code Telegram sends, and your two-step password, just like the official apps.
- **Vim all the way.** Normal mode to move around (`j`/`k`, `gg`/`G`, `Ctrl-d`/`Ctrl-u`), `i` to write, `Esc` to stop. Your hands never leave the keyboard.
- **Photos and stickers, inline.** Real images in kitty, Ghostty, WezTerm and iTerm2, and block-character previews in any other terminal.
- **Search everything.** `/` in the chat list filters chats as you type. `/` inside a chat searches its entire history, and `n`/`N` jump between matches, highlighted where they appear.
- **Feels like Telegram.** Message bubbles with yours on the right, sender names in color, date separators, and unread chats on top.
- **Open anything.** Press `Enter` on a photo, video, file or link to open it in your default app.
- **Make it yours.** Four Catppuccin themes with live preview, and highlights that make your important chats stand out.
- **Private by design.** tuigram talks only to Telegram. No telemetry, no accounts and no servers in between. Your session stays on your machine.

## Get started

**1. Install.** You need [Rust](https://rustup.rs). The first build downloads TDLib and takes a few minutes.

```sh
cargo install tuigram-cli
```

Works on Linux, macOS and Windows, on x86_64 and ARM64.

**2. Get your API keys (once).** Telegram requires every client app to have its own API ID and hash. Sign in at [my.telegram.org](https://my.telegram.org), open **API development tools**, and create an app with any name.

**3. Run it from anywhere.**

```sh
tuigram
```

Paste your `api_id` and `api_hash` when asked, then log in. That's it: next time, `tuigram` takes you straight to your chats.

> **Why your own keys?** tuigram doesn't ship shared credentials. A key published in public source code can be abused and then blocked by Telegram for everyone who uses it. Your own key means your access never depends on anyone else's.

## Keys

The status bar always shows the keys for where you are. The essentials:

| Key | Action |
| --- | --- |
| `j` / `k` | Move down / up |
| `gg` / `G` | Jump to top / bottom |
| `Ctrl-d` / `Ctrl-u` | Half a page down / up |
| `Enter` / `l` | Open a chat, or the file or link in a message |
| `h` / `Esc` | Back to the chat list |
| `i` | Write a message: `Enter` sends, `Alt-Enter` or `Ctrl-j` starts a new line |
| `/` | Search chat names, or messages in the open chat |
| `n` / `N` | Next older / newer match |
| `H` | Highlight a chat |
| `?` | Settings and themes |
| `q` | Quit |

## Your data

Everything lives in one folder on your machine (`tuigram --help` prints its path):

| OS | Location |
| --- | --- |
| macOS | `~/Library/Application Support/tuigram` |
| Linux | `~/.local/share/tuigram` |
| Windows | `%LOCALAPPDATA%\tuigram` |

It holds your login session, API keys, downloaded files and settings. Deleting it removes your session from this computer. To end the session completely, go to **Settings → Devices** in another Telegram app.

Environment variables (also read from a `.env` file in the current directory):

| Variable | Use |
| --- | --- |
| `TG_DATA_DIR` | Keep the data folder somewhere else |
| `TG_API_ID`, `TG_API_HASH` | Use these API keys instead of the saved ones |

## Development

```sh
git clone https://github.com/erictran308/tuigram && cd tuigram
cargo run       # uses the same data folder as the installed app, or TG_DATA_DIR
cargo test
```

Copy `.env.example` to `.env` to keep a separate development session (for example `TG_DATA_DIR=./.tdlib`).

## Built with

- [TDLib](https://github.com/tdlib/td) through [tdlib-rs](https://github.com/FedericoBruzzone/tdlib-rs)
- [ratatui](https://ratatui.rs) and [ratatui-image](https://github.com/benjajaja/ratatui-image)
- [Catppuccin](https://catppuccin.com) colors

## License

[MIT](LICENSE)

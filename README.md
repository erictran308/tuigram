<div align="center">

# tuigram

**Telegram at the speed of your keyboard.**

A Telegram client for the terminal with vim keys, inline photos and search across your whole history.<br>
Written in Rust on [TDLib](https://github.com/tdlib/td), the library behind Telegram's own apps.

[![Release](https://img.shields.io/github/v/release/erictran308/tuigram)](https://github.com/erictran308/tuigram/releases/latest)
[![crates.io](https://img.shields.io/crates/v/tuigram-cli.svg)](https://crates.io/crates/tuigram-cli)
[![License: MIT](https://img.shields.io/badge/license-MIT-blue.svg)](LICENSE)

**[Download for macOS, Linux or Windows](#get-started)** and log in. Nothing to set up.<br>
With Rust: `cargo binstall tuigram-cli` gets the same ready-made app.

<!-- TODO: add a screenshot or GIF here, e.g. ![tuigram](docs/screenshot.png) -->

</div>

---

## Why tuigram

- **Your real account, not a bot.** Log in by scanning a QR code with Telegram on your phone, or with your phone number and the code Telegram sends, plus your two-step password if you have one, just like the official apps.
- **Vim all the way.** Normal mode to move around (`j`/`k`, `gg`/`G`, `Ctrl-d`/`Ctrl-u`), `i` to write, `Esc` to stop. Your hands never leave the keyboard.
- **Photos and stickers, inline.** Real images in kitty, Ghostty, WezTerm and iTerm2, and block-character previews in any other terminal.
- **Search everything.** `/` in the chat list filters chats as you type. `/` inside a chat searches its entire history, and `n`/`N` jump between matches, highlighted where they appear.
- **Feels like Telegram.** Message bubbles with yours on the right, sender names in color, date separators, "typing…" while someone writes to you, and unread chats on top.
- **Open anything.** Press `Enter` on a photo, video, file or link to open it in your default app.
- **Notifications.** New messages pop up as system notifications while you're in another window, and the window title counts your unread chats. Telegram's mute settings apply.
- **Make it yours.** Four Catppuccin themes with live preview, and highlights that make your important chats stand out.
- **Private by design.** tuigram talks only to Telegram. No telemetry, no accounts and no servers in between. Your session stays on your machine, and read receipts go out only for messages you've actually had in front of you.
- **Careful with what others send.** A file that could run a program, or a link whose text hides where it really goes, asks before opening.

## Get started

There are two ways to install tuigram:

- **Download it** (or `cargo binstall tuigram-cli`). Ready-made apps come with tuigram's own Telegram API key, so you just log in.
- **Build it from source** with `cargo install`. You bring your own API key, which takes two minutes on Telegram's site.

### Download (recommended)

**1. Install.** If you have Rust and [cargo-binstall](https://github.com/cargo-bins/cargo-binstall), that's one command, on any system:

```sh
cargo binstall tuigram-cli
```

Otherwise, on macOS or Linux, paste this into a terminal. It puts `tuigram` in `~/.local/bin`:

```sh
mkdir -p ~/.local/bin
curl -fsSL https://github.com/erictran308/tuigram/releases/latest/download/tuigram-aarch64-apple-darwin.tar.gz | tar xz -C ~/.local/bin tuigram
```

Swap the file name for your computer's:

| Computer | File |
| --- | --- |
| Mac with Apple silicon (M1 or later) | [`tuigram-aarch64-apple-darwin.tar.gz`](https://github.com/erictran308/tuigram/releases/latest/download/tuigram-aarch64-apple-darwin.tar.gz) |
| Mac with Intel | [`tuigram-x86_64-apple-darwin.tar.gz`](https://github.com/erictran308/tuigram/releases/latest/download/tuigram-x86_64-apple-darwin.tar.gz) |
| Linux, x86_64 | [`tuigram-x86_64-unknown-linux-gnu.tar.gz`](https://github.com/erictran308/tuigram/releases/latest/download/tuigram-x86_64-unknown-linux-gnu.tar.gz) |
| Linux, ARM64 | [`tuigram-aarch64-unknown-linux-gnu.tar.gz`](https://github.com/erictran308/tuigram/releases/latest/download/tuigram-aarch64-unknown-linux-gnu.tar.gz) |
| Windows, x86_64 | [`tuigram-x86_64-pc-windows-msvc.zip`](https://github.com/erictran308/tuigram/releases/latest/download/tuigram-x86_64-pc-windows-msvc.zip) |
| Windows, ARM64 | [`tuigram-aarch64-pc-windows-msvc.zip`](https://github.com/erictran308/tuigram/releases/latest/download/tuigram-aarch64-pc-windows-msvc.zip) |

- **Windows:** download the `.zip`, unzip it, and run `tuigram.exe` from Windows Terminal.
- **Linux:** needs Ubuntu 24.04, Debian 13, Fedora 40 or newer, plus libc++: `sudo apt install libc++1` (Fedora: `sudo dnf install libcxx`).
- **macOS:** if you downloaded the file in a browser instead, macOS blocks the app. Run `xattr -d com.apple.quarantine tuigram` once to allow it.

If `tuigram` isn't found afterwards, add `~/.local/bin` to your `PATH`. Each file comes with a signed record of the commit it was built from; to check one, run `gh attestation verify <file> --repo erictran308/tuigram`.

**2. Run it.**

```sh
tuigram
```

**3. Log in.** Type your phone number, or press Tab and scan the QR code with Telegram on your phone. That's it: next time, `tuigram` takes you straight to your chats.

### Build from source

**1. Install.** You need [Rust](https://rustup.rs). `cargo install` always builds from source (unlike `cargo binstall` above). The first build downloads TDLib and takes a few minutes. On Linux, install libc++ first (`sudo apt install libc++-dev libc++abi-dev`).

```sh
cargo install tuigram-cli
```

Works on Linux, macOS and Windows, on x86_64 and ARM64.

**2. Get your API key (once).** Builds from source have no API key: their code is public, and a key published there would get blocked by Telegram for everyone. Sign in at [my.telegram.org](https://my.telegram.org), open **API development tools**, and create an app with any name.

**3. Run it.** Type `tuigram`, paste your `api_id` and `api_hash` when asked, then log in with your phone number or a QR code. tuigram saves the key, so you only do this once.

### Using your own API key

The downloaded app can use your own key too, so your access never depends on tuigram's. Set `TG_API_ID` and `TG_API_HASH` in your environment, or add the key to `settings.toml` in [your data folder](#your-data):

```toml
[api_keys]
id = 1234567
hash = "0123456789abcdef0123456789abcdef"
```

tuigram uses the first key it finds, in this order: the environment, `settings.toml`, then the built-in key. If Telegram ever stops accepting the built-in key, tuigram asks for your own instead.

## Keys

The status bar always shows the keys for where you are. The essentials:

| Key | Action |
| --- | --- |
| `j` / `k` | Move down / up |
| `gg` / `G` | Jump to top / bottom |
| `Ctrl-d` / `Ctrl-u` | Half a page down / up |
| `Enter` / `l` | Open a chat, or the file or link in a message (files that could run code, and links that hide their address, ask first: `y` opens) |
| `h` / `Esc` | Back to the chat list |
| `i` | Write a message: `Enter` sends, `Alt-Enter` or `Ctrl-j` starts a new line |
| `y` | Copy the selected message: its text, a link, or the photo or file |
| `r` | Reply to the selected message (`Esc` twice cancels the reply) |
| `gd` / `Ctrl-o` | Go to the message a reply answers / back to the reply |
| `d` | Delete the selected message, for everyone or just you (asks first) |
| `/` | Search chat names, or messages in the open chat |
| `n` / `N` | Next older / newer match |
| `H` | Highlight a chat |
| `:` | Run a command, typed in full: `:logout` logs out of Telegram on this computer |
| `?` | Every shortcut, plus settings and themes |
| `q` | Quit |

## Notifications

While tuigram's window is in the background, new messages show as notifications from your terminal: messages that arrive together become one, and a busy chat stays quiet for half a minute after each. Chats you muted in Telegram stay silent.

They work in Ghostty, kitty, WezTerm, iTerm2, foot and Konsole, also over SSH. In Windows Terminal, turn on `compatibility.allowOSC777` in its settings. Other terminals ring the bell instead. In tmux, add `set -g allow-passthrough on` and `set -g focus-events on` to `~/.tmux.conf`.

To turn them off or on, press `?`, go to **Settings**, and press `Space` on **Notifications**, then `Enter` to save. To pick how they're sent, set `notifications` in `settings.toml` (in [your data folder](#your-data)) to `"bell"`, `"osc9"`, `"osc777"` or `"osc99"`; the default `"auto"` picks for your terminal.

Like Telegram Desktop, tuigram shows you as online while its window is focused and you've pressed a key in the last minute, so notifications arrive right away instead of waiting to see if you read them on your phone.

## Your data

Everything lives in one folder on your machine (`tuigram --help` prints its path):

| OS | Location |
| --- | --- |
| macOS | `~/Library/Application Support/tuigram` |
| Linux | `~/.local/share/tuigram` |
| Windows | `%LOCALAPPDATA%\tuigram` |

It holds your login session, API keys, downloaded files and settings. Deleting it removes your session from this computer. To end the session completely, go to **Settings → Devices** in another Telegram app.

Environment variables (they can also go in a `.env` file in the current directory):

| Variable | Use |
| --- | --- |
| `TG_DATA_DIR` | Keep the data folder somewhere else |
| `TG_API_ID`, `TG_API_HASH` | Use your own API key instead of the saved or built-in one |

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

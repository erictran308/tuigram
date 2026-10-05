mod app;
mod chats;
mod clipboard;
mod config;
mod demo;
mod images;
mod messages;
mod notify;
mod search;
mod settings;
mod text;
mod tg;
mod theme;
mod ui;

use std::io::{Write, stdout};

use anyhow::Result;
use crossterm::event::{
    DisableBracketedPaste, DisableFocusChange, EnableBracketedPaste, EnableFocusChange,
    KeyboardEnhancementFlags, PopKeyboardEnhancementFlags, PushKeyboardEnhancementFlags,
};
use crossterm::execute;
use ratatui_image::picker::Picker;

#[tokio::main]
async fn main() -> Result<()> {
    // A missing .env is fine: the variables can also come from the shell.
    config::load_dotenv();
    match std::env::args().nth(1).as_deref() {
        None => {}
        Some("-h" | "--help") => return print(&help()?),
        Some("-V" | "--version") => {
            return print(&format!("tuigram {}\n", env!("CARGO_PKG_VERSION")));
        }
        Some("--demo") => return demo::run().await,
        Some(other) => anyhow::bail!("unknown argument {other:?}; see tuigram --help"),
    }

    // Load config and start TDLib before taking over the terminal, so setup
    // errors print as normal text.
    let config = config::Config::load()?;
    let settings_path = settings::path(&config.data_dir);
    let settings = settings::Settings::load(&settings_path)?;
    let (tx, rx) = tokio::sync::mpsc::unbounded_channel();
    let env_keys = config.api_keys.clone();
    let tg = tg::Tg::start(config, tx).await?;

    let mut terminal = ratatui::init();
    // The title shows unread chats while tuigram runs, then goes back.
    notify::send(notify::SAVE_TITLE);
    notify::send(&notify::title(0));
    // A panic, on any thread, ends the app: the terminal is put back, then
    // the message is printed without control characters, since it can quote
    // text from a message. Carrying on after a background thread died would
    // leave the screen restored under a running app.
    std::panic::set_hook(Box::new(|info| {
        let _ = execute!(
            stdout(),
            PopKeyboardEnhancementFlags,
            DisableBracketedPaste,
            DisableFocusChange
        );
        notify::send(notify::RESTORE_TITLE);
        ratatui::restore();
        eprintln!("tuigram crashed: {}", text::clean(&info.to_string()));
        std::process::exit(101);
    }));
    // Ask the terminal which image protocol it speaks (Kitty on Ghostty) and its
    // cell size in pixels. Must happen before key reading starts.
    let picker = Picker::from_query_stdio().unwrap_or_else(|_| Picker::halfblocks());
    // Pasted text arrives as one event instead of keystrokes, so a pasted line
    // break can't send a half-written message.
    execute!(stdout(), EnableBracketedPaste)?;
    // Read receipts wait while the terminal window is in the background, on
    // terminals that report focus changes.
    execute!(stdout(), EnableFocusChange)?;
    // Terminals with the kitty keyboard protocol (kitty, Ghostty, WezTerm…)
    // can then report Shift-Enter separately from Enter.
    let enhanced = crossterm::terminal::supports_keyboard_enhancement().unwrap_or(false);
    if enhanced {
        execute!(
            stdout(),
            PushKeyboardEnhancementFlags(KeyboardEnhancementFlags::DISAMBIGUATE_ESCAPE_CODES)
        )?;
    }

    let (image_tx, image_rx) = tokio::sync::mpsc::unbounded_channel();
    let images = images::Images::new(picker, image_tx);
    let (decoded_tx, decoded_rx) = tokio::sync::mpsc::unbounded_channel();
    let clipboard = clipboard::Clipboard::new(decoded_tx);
    let result = app::App::new(tg, images, clipboard, settings, settings_path, env_keys)
        .run(&mut terminal, rx, image_rx, decoded_rx)
        .await;

    if enhanced {
        let _ = execute!(stdout(), PopKeyboardEnhancementFlags);
    }
    let _ = execute!(stdout(), DisableBracketedPaste, DisableFocusChange);
    notify::send(notify::RESTORE_TITLE);
    ratatui::restore();
    result
}

/// Writes to stdout. A reader that stops early (`tuigram --help | head -1`,
/// `grep -q`) is fine, where `println!` would panic on the closed pipe.
fn print(text: &str) -> Result<()> {
    match stdout().lock().write_all(text.as_bytes()) {
        Err(e) if e.kind() == std::io::ErrorKind::BrokenPipe => Ok(()),
        result => Ok(result?),
    }
}

fn help() -> Result<String> {
    // CI checks for this wording to make sure release binaries have the key.
    let keys = if config::built_in_keys().is_some() {
        "This build comes with tuigram's own API key, so you only need to log in.
To use your own key instead, set TG_API_ID and TG_API_HASH."
    } else {
        "This build has no API key: on first run, tuigram asks for your Telegram
API ID and hash, which you get once at https://my.telegram.org (API
development tools). Or install the ready-made app, which has one:
  cargo binstall tuigram-cli"
    };
    Ok(format!(
        "tuigram {version}
Telegram in your terminal, with vim-style keys.

Usage: tuigram [-h | --help] [-V | --version] [--demo]

Inside the app, the status bar lists the keys for wherever you are;
press ? for settings and q to quit.

--demo shows made-up chats without logging in or touching your session:
1-5 or Tab switch scenes, t changes the theme, q quits.

{keys}

Your login session, API credentials, downloaded files and settings are
kept in:
  {data}

Environment:
  TG_DATA_DIR    keep them somewhere else
  TG_API_ID      your own API key, used before saved or built-in ones
  TG_API_HASH
",
        version = env!("CARGO_PKG_VERSION"),
        data = config::data_dir()?.display(),
    ))
}

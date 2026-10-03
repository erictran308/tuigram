mod app;
mod chats;
mod clipboard;
mod config;
mod images;
mod messages;
mod search;
mod settings;
mod tg;
mod theme;
mod ui;

use std::io::stdout;

use anyhow::Result;
use crossterm::event::{
    DisableBracketedPaste, EnableBracketedPaste, KeyboardEnhancementFlags,
    PopKeyboardEnhancementFlags, PushKeyboardEnhancementFlags,
};
use crossterm::execute;
use ratatui_image::picker::Picker;

#[tokio::main]
async fn main() -> Result<()> {
    // A missing .env is fine: the variables can also come from the shell.
    let _ = dotenvy::dotenv();
    match std::env::args().nth(1).as_deref() {
        None => {}
        Some("-h" | "--help") => {
            print_help()?;
            return Ok(());
        }
        Some("-V" | "--version") => {
            println!("tuigram {}", env!("CARGO_PKG_VERSION"));
            return Ok(());
        }
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
    // Ask the terminal which image protocol it speaks (Kitty on Ghostty) and its
    // cell size in pixels. Must happen before key reading starts.
    let picker = Picker::from_query_stdio().unwrap_or_else(|_| Picker::halfblocks());
    // Pasted text arrives as one event instead of keystrokes, so a pasted line
    // break can't send a half-written message.
    execute!(stdout(), EnableBracketedPaste)?;
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
    let _ = execute!(stdout(), DisableBracketedPaste);
    ratatui::restore();
    result
}

fn print_help() -> Result<()> {
    println!(
        "tuigram {version}
Telegram in your terminal, with vim-style keys.

Usage: tuigram [-h | --help] [-V | --version]

Inside the app, the status bar lists the keys for wherever you are;
press ? for settings and q to quit.

On first run, tuigram asks for your Telegram API ID and hash, which you
get once at https://my.telegram.org (API development tools).

Your login session, API credentials, downloaded files and settings are
kept in:
  {data}

Environment:
  TG_DATA_DIR    keep them somewhere else
  TG_API_ID      API credentials to use instead of the saved ones
  TG_API_HASH",
        version = env!("CARGO_PKG_VERSION"),
        data = config::data_dir()?.display(),
    );
    Ok(())
}

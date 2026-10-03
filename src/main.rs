mod app;
mod chats;
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
    // Load config and start TDLib before taking over the terminal, so setup
    // errors print as normal text.
    let config = config::Config::load()?;
    let settings_path = settings::path(&config.data_dir);
    let settings = settings::Settings::load(&settings_path)?;
    let (tx, rx) = tokio::sync::mpsc::unbounded_channel();
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
    let result = app::App::new(tg, images, settings, settings_path)
        .run(&mut terminal, rx, image_rx)
        .await;

    if enhanced {
        let _ = execute!(stdout(), PopKeyboardEnhancementFlags);
    }
    let _ = execute!(stdout(), DisableBracketedPaste);
    ratatui::restore();
    result
}

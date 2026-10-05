//! User settings, changed in the app (`?`) and kept in `settings.toml` in the
//! data directory.

use std::io::Write;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

use crate::config::ApiKeys;
use crate::notify::Notifications;
use crate::theme;

#[derive(Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Settings {
    /// The theme in use, by file name without `.toml`: a built-in one or one
    /// in the themes folder.
    pub theme: String,
    /// Chats highlighted with `H`, by id.
    pub highlighted_chats: Vec<i64>,
    /// How new messages are announced: "auto" picks what the terminal
    /// supports; also "off", "bell", "osc9", "osc777" or "osc99".
    pub notifications: Notifications,
    /// After sending a message, go back to Normal mode instead of staying in
    /// Insert mode to write the next one.
    pub normal_after_send: bool,
    /// Messages in a row from one person have a row of their bubble's
    /// background between them, so each stands apart within the block.
    pub block_gaps: bool,
    /// A blank row between chats in the chat list.
    pub chat_gaps: bool,
    /// Which side of the window the chat list is on.
    pub chat_list_side: Side,
    /// The chat list's share of the window's width, in percent. Ctrl-r
    /// changes it; see [`Settings::list_width`].
    pub chat_list_width: u16,
    /// Telegram API credentials entered on the login screen.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub api_keys: Option<ApiKeys>,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            theme: theme::DEFAULT.into(),
            highlighted_chats: Vec::new(),
            notifications: Notifications::default(),
            normal_after_send: false,
            block_gaps: true,
            chat_gaps: true,
            chat_list_side: Side::Left,
            chat_list_width: DEFAULT_LIST_WIDTH,
            api_keys: None,
        }
    }
}

/// A side of the window, for the chat list.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Side {
    #[default]
    Left,
    Right,
}

/// The chat list's width, in percent of the window, at first and after `=`.
pub const DEFAULT_LIST_WIDTH: u16 = 35;
/// How narrow and how wide the chat list can be, in percent, so neither
/// pane disappears.
const LIST_WIDTHS: std::ops::RangeInclusive<u16> = 15..=70;
/// Percent a press of `h` or `l` moves the line between the panes.
const LIST_STEP: u16 = 5;

impl Settings {
    /// Defaults if the file doesn't exist yet; an error if it can't be read.
    pub fn load(path: &Path) -> Result<Self> {
        let text = match std::fs::read_to_string(path) {
            Ok(text) => text,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Self::default()),
            Err(e) => return Err(e).with_context(|| format!("cannot read {}", path.display())),
        };
        toml::from_str(&text).with_context(|| format!("{} is invalid", path.display()))
    }

    /// The chat list's width as drawn, in percent: within [`LIST_WIDTHS`],
    /// whatever the file says.
    pub fn list_width(&self) -> u16 {
        self.chat_list_width
            .clamp(*LIST_WIDTHS.start(), *LIST_WIDTHS.end())
    }

    /// Makes the chat list `steps` steps wider, or narrower if negative.
    pub fn resize_list(&mut self, steps: i16) {
        let width = self.list_width() as i16 + steps * LIST_STEP as i16;
        self.chat_list_width =
            (width.max(0) as u16).clamp(*LIST_WIDTHS.start(), *LIST_WIDTHS.end());
    }

    /// Writes the settings, which can hold API credentials: readable only by
    /// the user, and all at once, as a new file renamed over the old one, so
    /// a crash halfway leaves the old settings rather than half of the new.
    pub fn save(&self, path: &Path) -> Result<()> {
        let text = toml::to_string(self)?;
        let new = path.with_extension("toml.new");
        let write = || -> std::io::Result<()> {
            // One left by a crash, maybe with other permissions.
            let _ = std::fs::remove_file(&new);
            let mut options = std::fs::OpenOptions::new();
            options.write(true).create_new(true);
            #[cfg(unix)]
            std::os::unix::fs::OpenOptionsExt::mode(&mut options, 0o600);
            let mut file = options.open(&new)?;
            file.write_all(text.as_bytes())?;
            file.sync_all()?;
            std::fs::rename(&new, path)
        };
        write().with_context(|| format!("cannot write {}", path.display()))
    }
}

pub fn path(data_dir: &Path) -> PathBuf {
    data_dir.join("settings.toml")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn missing_file_means_defaults_and_saving_round_trips() {
        let dir = std::env::temp_dir().join(format!("tuigram-settings-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let file = path(&dir);
        let _ = std::fs::remove_file(&file);

        assert_eq!(Settings::load(&file).unwrap().theme, "mocha");
        let mut settings = Settings {
            theme: "latte".into(),
            highlighted_chats: vec![-1001234567890, 42],
            notifications: Notifications::Off,
            normal_after_send: true,
            block_gaps: false,
            chat_gaps: false,
            chat_list_side: Side::Right,
            chat_list_width: 40,
            api_keys: None,
        };
        settings.save(&file).unwrap();
        assert_eq!(
            std::fs::read_to_string(&file).unwrap().trim(),
            "theme = \"latte\"\nhighlighted_chats = [-1001234567890, 42]\nnotifications = \"off\"\nnormal_after_send = true\nblock_gaps = false\nchat_gaps = false\nchat_list_side = \"right\"\nchat_list_width = 40"
        );
        assert_eq!(Settings::load(&file).unwrap(), settings);

        settings.api_keys = Some(ApiKeys {
            id: 1234567,
            hash: "0123456789abcdef0123456789abcdef".into(),
        });
        settings.save(&file).unwrap();
        assert_eq!(Settings::load(&file).unwrap(), settings);
        assert!(
            !file.with_extension("toml.new").exists(),
            "renamed into place"
        );
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = std::fs::metadata(&file).unwrap().permissions().mode();
            assert_eq!(mode & 0o777, 0o600, "it can hold API keys");
        }

        // Files from before a setting existed get its default.
        std::fs::write(&file, r#"theme = "latte""#).unwrap();
        let old = Settings::load(&file).unwrap();
        assert!(old.block_gaps && old.chat_gaps);
        assert_eq!(old.chat_list_width, DEFAULT_LIST_WIDTH);
        assert_eq!(old.chat_list_side, Side::Left);

        // A theme that's gone is the app's to deal with, not a broken file.
        std::fs::write(&file, r#"theme = "deleted""#).unwrap();
        assert_eq!(Settings::load(&file).unwrap().theme, "deleted");
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn the_chat_list_resizes_in_steps_and_keeps_both_panes() {
        let mut settings = Settings::default();
        settings.resize_list(1);
        assert_eq!(settings.list_width(), 40);
        settings.resize_list(-3);
        assert_eq!(settings.list_width(), 25);
        settings.resize_list(-10);
        assert_eq!(settings.list_width(), 15, "the chat list stays");
        settings.resize_list(20);
        assert_eq!(settings.list_width(), 70, "the chat stays");

        // Hand-edited files can't hide a pane either.
        settings.chat_list_width = 200;
        assert_eq!(settings.list_width(), 70);
        settings.resize_list(-1);
        assert_eq!(settings.list_width(), 65);
    }
}

//! User settings, changed in the app (`?`) and kept in `settings.toml` in the
//! data directory.

use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

use crate::config::ApiKeys;
use crate::notify::Notifications;
use crate::theme::Theme;

#[derive(Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Settings {
    pub theme: Theme,
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
    /// Telegram API credentials entered on the login screen.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub api_keys: Option<ApiKeys>,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            theme: Theme::default(),
            highlighted_chats: Vec::new(),
            notifications: Notifications::default(),
            normal_after_send: false,
            block_gaps: true,
            chat_gaps: true,
            api_keys: None,
        }
    }
}

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

    pub fn save(&self, path: &Path) -> Result<()> {
        std::fs::write(path, toml::to_string(self)?)
            .with_context(|| format!("cannot write {}", path.display()))
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

        assert_eq!(Settings::load(&file).unwrap().theme, Theme::Mocha);
        let mut settings = Settings {
            theme: Theme::Latte,
            highlighted_chats: vec![-1001234567890, 42],
            notifications: Notifications::Off,
            normal_after_send: true,
            block_gaps: false,
            chat_gaps: false,
            api_keys: None,
        };
        settings.save(&file).unwrap();
        assert_eq!(
            std::fs::read_to_string(&file).unwrap().trim(),
            "theme = \"latte\"\nhighlighted_chats = [-1001234567890, 42]\nnotifications = \"off\"\nnormal_after_send = true\nblock_gaps = false\nchat_gaps = false"
        );
        assert_eq!(Settings::load(&file).unwrap(), settings);

        settings.api_keys = Some(ApiKeys {
            id: 1234567,
            hash: "0123456789abcdef0123456789abcdef".into(),
        });
        settings.save(&file).unwrap();
        assert_eq!(Settings::load(&file).unwrap(), settings);

        // Files from before a setting existed get its default.
        std::fs::write(&file, r#"theme = "latte""#).unwrap();
        let old = Settings::load(&file).unwrap();
        assert!(old.block_gaps && old.chat_gaps);

        std::fs::write(&file, r#"theme = "dracula""#).unwrap();
        let error = format!("{:#}", Settings::load(&file).unwrap_err());
        assert!(error.contains("mocha"), "lists the valid themes: {error}");
        std::fs::remove_dir_all(&dir).unwrap();
    }
}

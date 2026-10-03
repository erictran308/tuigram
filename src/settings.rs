//! User settings, changed in the app (`?`) and kept in `settings.toml` in the
//! data directory.

use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

use crate::theme::Theme;

#[derive(Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Settings {
    pub theme: Theme,
    /// Chats highlighted with `H`, by id.
    pub highlighted_chats: Vec<i64>,
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
        let settings = Settings {
            theme: Theme::Latte,
            highlighted_chats: vec![-1001234567890, 42],
        };
        settings.save(&file).unwrap();
        assert_eq!(
            std::fs::read_to_string(&file).unwrap().trim(),
            "theme = \"latte\"\nhighlighted_chats = [-1001234567890, 42]"
        );
        assert_eq!(Settings::load(&file).unwrap(), settings);

        std::fs::write(&file, r#"theme = "dracula""#).unwrap();
        let error = format!("{:#}", Settings::load(&file).unwrap_err());
        assert!(error.contains("mocha"), "lists the valid themes: {error}");
        std::fs::remove_dir_all(&dir).unwrap();
    }
}

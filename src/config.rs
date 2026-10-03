use std::path::PathBuf;

use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};

/// Telegram API credentials from https://my.telegram.org. Everyone brings
/// their own: none ship with the app, since anything in a public crate is
/// public. The login screen asks once and saves them in the settings file.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ApiKeys {
    pub id: i32,
    pub hash: String,
}

/// Settings read from the environment (which `main` fills from `.env` in the
/// working directory, if there is one).
pub struct Config {
    /// From `TG_API_ID` / `TG_API_HASH`. These win over saved ones.
    pub api_keys: Option<ApiKeys>,
    /// Where TDLib keeps its database (your login session), downloaded files and
    /// log, next to the app's own `settings.toml`.
    pub data_dir: PathBuf,
}

impl Config {
    pub fn load() -> Result<Self> {
        let api_keys = match (var("TG_API_ID"), var("TG_API_HASH")) {
            (Some(id), Some(hash)) => Some(ApiKeys {
                id: parse_api_id(&id).context("TG_API_ID must be a number")?,
                hash,
            }),
            (None, None) => None,
            _ => bail!("set both TG_API_ID and TG_API_HASH, or neither"),
        };
        let data_dir = data_dir()?;
        std::fs::create_dir_all(&data_dir)
            .with_context(|| format!("cannot create {}", data_dir.display()))?;

        Ok(Self { api_keys, data_dir })
    }
}

/// `TG_DATA_DIR`, else the platform's place for app data: `~/Library/Application
/// Support/tuigram` on macOS, `~/.local/share/tuigram` on Linux, `%LOCALAPPDATA%\tuigram`
/// on Windows. The same wherever the command is run from.
pub fn data_dir() -> Result<PathBuf> {
    if let Some(dir) = var("TG_DATA_DIR") {
        return Ok(PathBuf::from(dir));
    }
    let base = dirs::data_local_dir().context("no home directory found; set TG_DATA_DIR")?;
    Ok(base.join("tuigram"))
}

/// What my.telegram.org calls `api_id`: a positive number.
pub fn parse_api_id(text: &str) -> Option<i32> {
    text.trim().parse().ok().filter(|&id| id > 0)
}

/// What my.telegram.org calls `api_hash`: 32 hex digits.
pub fn is_api_hash(text: &str) -> bool {
    text.len() == 32 && text.bytes().all(|b| b.is_ascii_hexdigit())
}

/// A variable's trimmed value, or `None` if it's unset or blank.
fn var(name: &str) -> Option<String> {
    let value = std::env::var(name).ok()?;
    let value = value.trim();
    (!value.is_empty()).then(|| value.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn api_credentials_are_checked_before_telegram_sees_them() {
        assert_eq!(parse_api_id(" 1234567 "), Some(1234567));
        assert_eq!(parse_api_id("0"), None);
        assert_eq!(parse_api_id("abc"), None);
        assert!(is_api_hash("0123456789abcdef0123456789ABCDEF"));
        assert!(!is_api_hash("0123456789abcdef"), "too short");
        assert!(!is_api_hash("0123456789abcdef0123456789abcdeg"), "not hex");
    }
}

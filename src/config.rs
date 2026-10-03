use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::OnceLock;

use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};

/// Telegram API credentials from https://my.telegram.org. Release binaries
/// come with tuigram's own ([`built_in_keys`]); other builds have none, and
/// the login screen asks once and saves them in the settings file.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ApiKeys {
    pub id: i32,
    pub hash: String,
}

/// Settings read from the environment, or from `.env` in the working
/// directory ([`load_dotenv`]).
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
        // It holds the login session and message cache: only for this user,
        // whatever the umask or the folder it's in allow.
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&data_dir, std::fs::Permissions::from_mode(0o700))
                .with_context(|| format!("cannot protect {}", data_dir.display()))?;
        }

        Ok(Self { api_keys, data_dir })
    }
}

// `BUILT_IN_KEYS`: `id:hash` XORed with a mask, then the mask; see build.rs.
include!(concat!(env!("OUT_DIR"), "/built_in_keys.rs"));

/// The API key release binaries come with, so people can log in without
/// registering an app. CI passes it in from GitHub secrets at compile time
/// (`TUIGRAM_API_ID` / `TUIGRAM_API_HASH`). It's never in the repository or
/// the published crate, which are public, so `cargo install` builds have none.
pub fn built_in_keys() -> Option<ApiKeys> {
    let (masked, mask) = BUILT_IN_KEYS?;
    unmask(masked, mask)
}

fn unmask(masked: &[u8], mask: &[u8]) -> Option<ApiKeys> {
    // black_box keeps the compiler from undoing the mask at build time, which
    // would put the plain key back in the binary.
    let masked = std::hint::black_box(masked);
    let plain: Vec<u8> = masked.iter().zip(mask).map(|(b, m)| b ^ m).collect();
    let (id, hash) = std::str::from_utf8(&plain).ok()?.split_once(':')?;
    let id = parse_api_id(id)?;
    is_api_hash(hash).then(|| ApiKeys {
        id,
        hash: hash.to_string(),
    })
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

/// `TG_*` settings from `.env` in the current directory, for development.
static DOTENV: OnceLock<HashMap<String, String>> = OnceLock::new();

/// Reads `.env` from the current directory, not its parents, and keeps only
/// its `TG_*` keys, without touching the process environment. So a `.env` in
/// some untrusted folder can't set `LD_PRELOAD` or `BROWSER` for the
/// programs tuigram starts.
pub fn load_dotenv() {
    let vars = dotenvy::from_path_iter(".env")
        .into_iter()
        .flatten()
        .filter_map(Result::ok)
        .filter(|(key, _)| key.starts_with("TG_"))
        .collect();
    let _ = DOTENV.set(vars);
}

/// A variable's trimmed value from the environment, else from `.env`, or
/// `None` if it's unset or blank.
fn var(name: &str) -> Option<String> {
    let value = std::env::var(name)
        .ok()
        .or_else(|| DOTENV.get()?.get(name).cloned())?;
    let value = value.trim();
    (!value.is_empty()).then(|| value.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_masked_built_in_key_unmasks_to_the_key() {
        let plain = b"1234567:0123456789abcdef0123456789abcdef";
        let mask: Vec<u8> = (0..plain.len() as u8)
            .map(|i| i.wrapping_mul(37) | 1)
            .collect();
        let masked: Vec<u8> = plain.iter().zip(&mask).map(|(b, m)| b ^ m).collect();
        assert_eq!(
            unmask(&masked, &mask),
            Some(ApiKeys {
                id: 1234567,
                hash: "0123456789abcdef0123456789abcdef".into(),
            })
        );
        assert_eq!(unmask(&masked, &mask[1..]), None, "wrong mask");
    }

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

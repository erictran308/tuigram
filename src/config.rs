use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};

use crate::{attach, text};

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
    /// From `TG_PROXY`: a proxy link, which wins over the saved one.
    pub proxy: Option<String>,
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
        let data_dir = prepare(&data_dir()?)?;

        Ok(Self {
            api_keys,
            data_dir,
            proxy: var("TG_PROXY"),
        })
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
    data_dir_from(var("TG_DATA_DIR"))
}

fn data_dir_from(set: Option<String>) -> Result<PathBuf> {
    if let Some(dir) = set {
        let dir = PathBuf::from(dir);
        // On Windows even creating a folder there hands that server your
        // login hash, and the session would live on it. A single leading
        // slash can also name one there (`\??\UNC\…`); `C:\` does the rest.
        let elsewhere = attach::on_another_machine(&dir)
            || (cfg!(windows) && matches!(dir.as_os_str().as_encoded_bytes(), [b'/' | b'\\', ..]));
        if elsewhere {
            bail!("TG_DATA_DIR must be a folder on this computer, with a drive letter on Windows");
        }
        return Ok(dir);
    }
    let base = dirs::data_local_dir().context("no home directory found; set TG_DATA_DIR")?;
    Ok(base.join("tuigram"))
}

/// A path as it can be printed: it may come from the environment.
pub fn shown(path: &Path) -> String {
    text::clean(&path.display().to_string())
}

/// Makes the data folder if it's missing and keeps it for this user alone
/// on every start, as Telegram Desktop does with its own: it holds the
/// login session and message cache, whatever the umask or the folder it's
/// in allow. The folders above it are the user's to pick, as there. TDLib
/// gets its full path, whichever folder tuigram was started in.
fn prepare(dir: &Path) -> Result<PathBuf> {
    let shown = shown(dir);
    std::fs::create_dir_all(dir).with_context(|| format!("cannot create {shown}"))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(dir, std::fs::Permissions::from_mode(0o700))
            .with_context(|| format!("cannot protect {shown}"))?;
    }
    std::path::absolute(dir).with_context(|| format!("cannot find {shown}"))
}

/// A file the user put in the data folder (a theme, a sound), whole. Links
/// are followed, but only to a plain file of at most `max` bytes: a link to
/// `/dev/zero` or a pipe would otherwise hang tuigram, or fill its memory.
pub fn read_small(path: &Path, max: u64) -> std::result::Result<Vec<u8>, String> {
    use std::io::Read;
    let meta = std::fs::metadata(path).map_err(|e| e.to_string())?;
    if !meta.is_file() {
        return Err("not a file".into());
    }
    if meta.len() > max {
        return Err(match max >> 20 {
            0 => format!("bigger than {} KB", max >> 10),
            mb => format!("bigger than {mb} MB"),
        });
    }
    let mut bytes = Vec::new();
    std::fs::File::open(path)
        .and_then(|file| file.take(max).read_to_end(&mut bytes))
        .map_err(|e| e.to_string())?;
    Ok(bytes)
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
/// programs tuigram starts. Only development builds read it: in an installed
/// tuigram, a `.env` in a cloned repository could pick the folder your
/// session is kept in, or hand you one prepared by whoever wrote it.
pub fn load_dotenv() {
    if !cfg!(debug_assertions) {
        let _ = DOTENV.set(HashMap::new());
        return;
    }
    let vars = dotenvy::from_path_iter(".env")
        .into_iter()
        .flatten()
        .filter_map(Result::ok)
        .filter(|(key, _)| key.starts_with("TG_"))
        .collect();
    let _ = DOTENV.set(vars);
}

/// There's a `.env` here that this build doesn't read: worth saying, since a
/// developer may expect it to pick a separate session.
pub fn dotenv_ignored() -> bool {
    !cfg!(debug_assertions) && Path::new(".env").is_file()
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
    fn a_data_folder_on_another_machine_is_refused() {
        for dir in ["//evil.example/s/tg", "\\\\evil.example\\s\\tg"] {
            assert!(data_dir_from(Some(dir.into())).is_err(), "{dir}");
        }
        let local = data_dir_from(Some("./.tdlib".into())).unwrap();
        assert_eq!(local, PathBuf::from("./.tdlib"));
    }

    #[cfg(unix)]
    #[test]
    fn the_data_folder_is_kept_for_you_alone_wherever_it_is() {
        use std::os::unix::fs::PermissionsExt;
        let parent = std::env::temp_dir().join(format!("tuigram-place-{}", std::process::id()));
        std::fs::create_dir_all(&parent).unwrap();
        // As with Telegram Desktop, a folder others can write to above it
        // is yours to pick.
        std::fs::set_permissions(&parent, std::fs::Permissions::from_mode(0o777)).unwrap();
        let dir = prepare(&parent.join("tg")).unwrap();
        let mode = std::fs::metadata(&dir).unwrap().permissions().mode();
        assert_eq!(mode & 0o777, 0o700);
        std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o755)).unwrap();
        prepare(&dir).unwrap();
        let mode = std::fs::metadata(&dir).unwrap().permissions().mode();
        assert_eq!(mode & 0o777, 0o700, "again on every start");
        std::fs::remove_dir_all(&parent).unwrap();
    }

    #[test]
    fn paths_are_printed_without_control_characters() {
        assert_eq!(shown(Path::new("/tmp/a\u{1b}]0;x\u{7}b")), "/tmp/a]0;xb");
    }

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

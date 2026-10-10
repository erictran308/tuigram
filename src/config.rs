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
        let data_dir = data_dir()?;
        let shown = shown(&data_dir);
        std::fs::create_dir_all(&data_dir).with_context(|| format!("cannot create {shown}"))?;
        // It holds the login session and message cache: only for this user,
        // whatever the umask or the folder it's in allow.
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&data_dir, std::fs::Permissions::from_mode(0o700))
                .with_context(|| format!("cannot protect {shown}"))?;
        }
        // The default folder is in your own home; one set elsewhere must be
        // somewhere nobody else can swap it out. TDLib then gets the path
        // that was checked, with any links in it resolved.
        #[cfg(unix)]
        let data_dir = if var("TG_DATA_DIR").is_some() {
            private_place(&data_dir)?;
            std::fs::canonicalize(&data_dir).with_context(|| format!("cannot read {shown}"))?
        } else {
            data_dir
        };

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

/// Checks that only you or the system can change the folders above `dir`:
/// otherwise another account could rename the data folder away and put its
/// own in its place, after this check and before TDLib writes the session
/// into it. A link as the folder itself must be yours too. Folders anyone
/// may write to, like `/tmp`, are fine when only an entry's owner can move
/// it (the sticky bit).
#[cfg(unix)]
fn private_place(dir: &Path) -> Result<()> {
    use std::os::unix::fs::MetadataExt;
    let uid = std::fs::metadata(dir)?.uid();
    // SAFETY: getgid can't fail and touches no memory.
    let gid = unsafe { libc::getgid() };
    let unsafe_place = |what: &Path| {
        anyhow::anyhow!(
            "{} can be changed by other users, so it's no place for your session; \
             set TG_DATA_DIR somewhere in your home folder",
            shown(what)
        )
    };
    if std::fs::symlink_metadata(dir)?.uid() != uid {
        return Err(unsafe_place(dir));
    }
    for folder in std::fs::canonicalize(dir)?.ancestors().skip(1) {
        let meta = std::fs::metadata(folder)?;
        let mode = meta.mode();
        // Your own group is only yours on Linux (user private groups); on
        // macOS every account is in `staff`.
        let group_is_others = cfg!(target_os = "macos") || meta.gid() != gid;
        let others_write = mode & 0o002 != 0 || (mode & 0o020 != 0 && group_is_others);
        let sticky = mode & 0o1000 != 0;
        let owner_ok = meta.uid() == uid || meta.uid() == 0;
        if !owner_ok || (others_write && !(sticky && meta.uid() == 0)) {
            return Err(unsafe_place(folder));
        }
    }
    Ok(())
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
    fn a_data_folder_others_could_swap_out_is_refused() {
        use std::os::unix::fs::PermissionsExt;
        // As root, a folder you make is the system's, which is fine.
        // SAFETY: geteuid can't fail and touches no memory.
        if unsafe { libc::geteuid() } == 0 {
            return;
        }
        let parent = std::env::temp_dir().join(format!("tuigram-place-{}", std::process::id()));
        let dir = parent.join("tg");
        std::fs::create_dir_all(&dir).unwrap();
        let mode = |path: &Path, mode| {
            std::fs::set_permissions(path, std::fs::Permissions::from_mode(mode)).unwrap()
        };
        mode(&parent, 0o777);
        assert!(private_place(&dir).is_err(), "anyone could rename it");
        mode(&parent, 0o1777);
        assert!(private_place(&dir).is_err(), "sticky, but not the system's");
        // Every macOS account is in the same group.
        #[cfg(target_os = "macos")]
        {
            mode(&parent, 0o775);
            assert!(private_place(&dir).is_err(), "the group is everyone");
        }
        mode(&parent, 0o755);
        private_place(&dir).unwrap();
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

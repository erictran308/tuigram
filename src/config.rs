use std::path::PathBuf;

use anyhow::{Context, Result, bail};

/// Settings read from the environment, or from `.env` in the working directory.
pub struct Config {
    pub api_id: i32,
    pub api_hash: String,
    /// Where TDLib keeps its database (your login session), downloaded files and
    /// log, next to the app's own `settings.toml`.
    pub data_dir: PathBuf,
}

impl Config {
    pub fn load() -> Result<Self> {
        // A missing .env is fine: the variables can also come from the shell.
        let _ = dotenvy::dotenv();

        let api_id = required("TG_API_ID")?
            .parse()
            .context("TG_API_ID must be a number")?;
        let api_hash = required("TG_API_HASH")?;
        let data_dir = match std::env::var("TG_DATA_DIR") {
            Ok(dir) if !dir.trim().is_empty() => PathBuf::from(dir.trim()),
            _ => std::env::current_dir()?.join(".tdlib"),
        };
        std::fs::create_dir_all(&data_dir)
            .with_context(|| format!("cannot create {}", data_dir.display()))?;

        Ok(Self {
            api_id,
            api_hash,
            data_dir,
        })
    }
}

fn required(name: &str) -> Result<String> {
    let value = std::env::var(name).unwrap_or_default();
    let value = value.trim();
    if value.is_empty() {
        bail!("{name} is not set. Fill it in .env (values come from https://my.telegram.org)");
    }
    Ok(value.to_string())
}

//! The one tmux setting tuigram's image library changes, put back on the way
//! out. ratatui-image switches the pane's `allow-passthrough` on whenever it
//! runs inside tmux, so images can reach the outer terminal. Left on, it
//! would let anything printed in that pane later (a file shown with `cat`,
//! an SSH session) send escape codes past tmux too.

use std::process::{Command, Stdio};
use std::sync::OnceLock;

/// The pane tuigram runs in, and the value it had of its own before (`None`:
/// it took the window's or the server's).
struct Saved {
    pane: String,
    value: Option<String>,
}

static SAVED: OnceLock<Saved> = OnceLock::new();

/// The pane whose setting ratatui-image is about to change: it does when
/// `TERM` or `TERM_PROGRAM` says tmux. Only a pane tmux names for us
/// (`TMUX` and `TMUX_PANE`) can be put back afterwards.
fn pane(env: impl Fn(&str) -> Option<String>) -> Option<String> {
    let set = |name: &str| env(name).filter(|v| !v.is_empty());
    let in_tmux = set("TERM").is_some_and(|t| t.starts_with("tmux"))
        || set("TERM_PROGRAM").is_some_and(|p| p == "tmux");
    let pane = set("TMUX_PANE")?;
    (in_tmux && set("TMUX").is_some()).then_some(pane)
}

/// Remembers the pane's own value. Call before the image picker is made.
pub fn save() {
    let Some(pane) = pane(|name| std::env::var(name).ok()) else {
        return;
    };
    let Ok(out) = Command::new("tmux")
        .args(["show-options", "-pqv", "-t", &pane, "allow-passthrough"])
        .stdin(Stdio::null())
        .stderr(Stdio::null())
        .output()
    else {
        return;
    };
    if out.status.success() {
        let value = String::from_utf8_lossy(&out.stdout).trim().to_string();
        let value = (!value.is_empty()).then_some(value);
        let _ = SAVED.set(Saved { pane, value });
    }
}

/// Puts the pane's value back. Harmless to call more than once.
pub fn restore() {
    if let Some(saved) = SAVED.get() {
        let _ = Command::new("tmux")
            .args(restore_args(&saved.pane, saved.value.as_deref()))
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status();
    }
}

fn restore_args(pane: &str, value: Option<&str>) -> Vec<String> {
    let mut args = vec!["set-option".to_string(), "-p".into()];
    if value.is_none() {
        args.push("-u".into());
    }
    args.extend(["-t".into(), pane.into(), "allow-passthrough".into()]);
    args.extend(value.map(String::from));
    args
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use super::*;

    fn pane_with(vars: &[(&str, &str)]) -> Option<String> {
        let vars: HashMap<String, String> = vars
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect();
        pane(|name| vars.get(name).cloned())
    }

    #[test]
    fn only_a_pane_tmux_names_is_put_back() {
        let tmux = ("TMUX", "/tmp/tmux-501/default,1,0");
        assert_eq!(
            pane_with(&[("TERM_PROGRAM", "tmux"), tmux, ("TMUX_PANE", "%3")]).as_deref(),
            Some("%3")
        );
        assert_eq!(
            pane_with(&[("TERM", "tmux-256color"), tmux, ("TMUX_PANE", "%3")]).as_deref(),
            Some("%3")
        );
        assert_eq!(pane_with(&[("TERM", "tmux-256color")]), None, "over SSH");
        assert_eq!(
            pane_with(&[("TERM", "xterm-256color"), tmux, ("TMUX_PANE", "%3")]),
            None,
            "the image library leaves this one alone"
        );
    }

    #[test]
    fn the_pane_gets_its_own_value_back_or_none() {
        assert_eq!(
            restore_args("%3", None),
            ["set-option", "-p", "-u", "-t", "%3", "allow-passthrough"]
        );
        assert_eq!(
            restore_args("%3", Some("off")),
            ["set-option", "-p", "-t", "%3", "allow-passthrough", "off"]
        );
    }
}

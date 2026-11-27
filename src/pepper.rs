//! Pepper: a Ghostty build that shows video inside the terminal, straight
//! from the graphics chip. There, video messages play through Pepper's own
//! player, pepper-play (its path in `PEPPER_PLAY`: libmpv drawing into
//! surfaces Pepper shows over a box of cells), not as kitty frames tuigram
//! sends. pepper-play takes mpv's options and JSON IPC, and plays the sound.
//!
//! The two meet by a token, fresh for each playback: tuigram gives it to
//! pepper-play over the socket (never on a command line), and places the box
//! with it by an escape sequence, `ESC ] 777 ; pepper ; place ; TOKEN ; COL ;
//! ROW ; COLS ; ROWS ESC \`, which only a program writing to this tab can
//! send. Pepper shows pepper-play's frames only where that token was placed.
//! The same as tuitube's (`../tuitube/src/pepper.rs`), less its holes: here
//! the box is taken away while a popup is over it, as sixel pictures wait.

use std::ffi::OsString;
use std::path::{Path, PathBuf};

#[cfg(unix)]
use ratatui::layout::Rect;

/// What a terminal multiplexer running tuigram sets: tmux, GNU screen,
/// zellij, herdr. Each keeps Pepper's escape sequences from it, and keeps
/// the `PEPPER_PORT` of the Pepper it was started in after that one quits.
const MULTIPLEXERS: [&str; 4] = ["TMUX", "STY", "ZELLIJ", "HERDR_PANE_ID"];

/// pepper-play, when tuigram runs in a Pepper that's still running, not
/// through a multiplexer: `None` elsewhere, and video messages play as kitty
/// frames.
pub fn player() -> Option<PathBuf> {
    player_from(|name| std::env::var_os(name), running)
}

fn player_from(
    var: impl Fn(&str) -> Option<OsString>,
    running: impl Fn(u32) -> bool,
) -> Option<PathBuf> {
    if MULTIPLEXERS.iter().any(|name| var(name).is_some()) {
        return None;
    }
    // `io.github.erictran308.pepper.<pid>`: a shell that outlived its Pepper
    // still has it, and pepper-play would then play the sound to no picture.
    let port = var("PEPPER_PORT")?.into_string().ok()?;
    let pid = port.rsplit('.').next()?.parse().ok()?;
    if !running(pid) {
        return None;
    }
    var("PEPPER_PLAY")
        .map(PathBuf::from)
        .filter(|path| runnable(path))
}

/// Process `pid` is there.
fn running(pid: u32) -> bool {
    #[cfg(unix)]
    {
        let Ok(pid) = i32::try_from(pid) else {
            return false;
        };
        // SAFETY: signal 0 checks the process exists and sends nothing.
        pid > 0
            && (unsafe { libc::kill(pid, 0) } == 0
                || std::io::Error::last_os_error().raw_os_error() == Some(libc::EPERM))
    }
    #[cfg(not(unix))]
    {
        let _ = pid;
        false
    }
}

/// An absolute path to a program this user can run: not one looked up in
/// whatever folder tuigram was started in.
fn runnable(path: &Path) -> bool {
    if !path.is_absolute() {
        return false;
    }
    let Ok(meta) = std::fs::metadata(path) else {
        return false;
    };
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        meta.is_file() && meta.permissions().mode() & 0o111 != 0
    }
    #[cfg(not(unix))]
    meta.is_file()
}

/// A token for one playback: 32 random hex digits, as Pepper takes them
/// (1–64 of `[A-Za-z0-9_-]`). `None` if the system's randomness can't be
/// read: then the video plays as kitty frames.
#[cfg(unix)]
pub fn new_token() -> Option<String> {
    use std::io::Read;
    let mut bytes = [0u8; 16];
    std::fs::File::open("/dev/urandom")
        .and_then(|mut f| f.read_exact(&mut bytes))
        .ok()?;
    Some(bytes.iter().map(|b| format!("{b:02x}")).collect())
}

/// Puts the picture of `token`'s playback over `area`.
#[cfg(unix)]
pub fn place(token: &str, area: Rect) -> String {
    format!(
        "\x1b]777;pepper;place;{token};{};{};{};{}\x1b\\",
        area.x, area.y, area.width, area.height
    )
}

/// Takes the picture of `token`'s playback away.
#[cfg(unix)]
pub fn remove(token: &str) -> String {
    format!("\x1b]777;pepper;remove;{token}\x1b\\")
}

#[cfg(test)]
mod tests {
    use super::*;

    /// An environment of just `vars`.
    fn env(vars: &[(&'static str, &'static str)]) -> impl Fn(&str) -> Option<OsString> {
        let vars = vars.to_vec();
        move |name| {
            vars.iter()
                .find(|(k, _)| *k == name)
                .map(|(_, v)| OsString::from(v))
        }
    }

    #[cfg(unix)]
    #[test]
    fn only_in_a_running_pepper_outside_a_multiplexer() {
        // Any program anyone can run stands in for pepper-play.
        let play = "/bin/sh";
        let pepper = [
            ("PEPPER_PORT", "io.github.erictran308.pepper.42"),
            ("PEPPER_PLAY", play),
        ];
        let alive = |pid| pid == 42;
        assert_eq!(player_from(env(&pepper), alive), Some(PathBuf::from(play)));
        assert_eq!(
            player_from(env(&pepper), |_| false),
            None,
            "that Pepper quit"
        );
        assert_eq!(player_from(env(&pepper[1..]), alive), None, "not Pepper");
        for (name, value) in [
            ("TMUX", "/tmp/tmux-501/default,1,0"),
            ("STY", "123.ttys001.mac"),
            ("ZELLIJ", "0"),
            ("HERDR_PANE_ID", "3"),
        ] {
            let vars = [pepper[0], pepper[1], (name, value)];
            assert_eq!(player_from(env(&vars), alive), None, "{name}");
        }
        let relative = [pepper[0], ("PEPPER_PLAY", "pepper-play")];
        assert_eq!(
            player_from(env(&relative), alive),
            None,
            "looked up in PATH"
        );
        assert!(running(std::process::id()));
    }

    #[cfg(unix)]
    #[test]
    fn tokens_are_fresh_hex() {
        let (a, b) = (new_token().unwrap(), new_token().unwrap());
        assert_eq!(a.len(), 32);
        assert!(a.bytes().all(|c| c.is_ascii_hexdigit()));
        assert_ne!(a, b);
    }

    #[cfg(unix)]
    #[test]
    fn sequences() {
        let area = Rect::new(2, 3, 40, 11);
        assert_eq!(
            place("ab", area),
            "\x1b]777;pepper;place;ab;2;3;40;11\x1b\\"
        );
        assert_eq!(remove("ab"), "\x1b]777;pepper;remove;ab\x1b\\");
    }
}

//! Notifications for new messages, sent to the terminal as escape codes: the
//! terminal shows them as the system's own notifications, which also works
//! over SSH. Which code depends on the terminal ([`detect`]).
//!
//! TDLib decides what deserves a notification (mutes, mentions, messages
//! already read elsewhere); [`Notifier`] batches what arrives together and
//! keeps a busy chat from sending one every few seconds.

use std::collections::HashMap;
use std::io::Write;
use std::time::Duration;

use base64::Engine;
use serde::{Deserialize, Serialize};
use tokio::time::Instant;

use crate::text;

/// How notifications reach the terminal, set in `settings.toml`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Notifications {
    /// Picked from the terminal tuigram runs in.
    #[default]
    Auto,
    Off,
    /// The bell: most terminals bounce the dock icon or mark the tab.
    Bell,
    /// Text only: iTerm2, Ghostty, kitty, WezTerm, foot.
    Osc9,
    /// A title and text: Ghostty, WezTerm, Konsole, foot, kitty, Windows
    /// Terminal (once `compatibility.allowOSC777` is on).
    Osc777,
    /// A title and text, shown only while the window is in the background:
    /// kitty, foot, Konsole.
    Osc99,
}

impl Notifications {
    /// `Auto` turned into what the terminal understands.
    pub fn resolve(self, env: impl Fn(&str) -> Option<String>) -> Notifications {
        match self {
            Notifications::Auto => detect(env),
            other => other,
        }
    }
}

/// The best notification code for the terminal, from what it says about
/// itself in the environment. Inside tmux, `TERM_PROGRAM` is tmux, but the
/// other variables come from the terminal tmux was started in.
pub fn detect(env: impl Fn(&str) -> Option<String>) -> Notifications {
    let is = |name: &str, value: &str| env(name).is_some_and(|v| v == value);
    let set = |name: &str| env(name).is_some_and(|v| !v.is_empty());
    let term = env("TERM").unwrap_or_default();
    if set("KITTY_WINDOW_ID")
        || term == "xterm-kitty"
        || term.starts_with("foot")
        || set("KONSOLE_VERSION")
    {
        Notifications::Osc99
    } else if is("TERM_PROGRAM", "ghostty")
        || set("GHOSTTY_RESOURCES_DIR")
        || is("TERM_PROGRAM", "WezTerm")
        || set("WEZTERM_PANE")
        || set("WT_SESSION")
    {
        Notifications::Osc777
    } else if is("TERM_PROGRAM", "iTerm.app") || is("LC_TERMINAL", "iTerm2") {
        Notifications::Osc9
    } else {
        Notifications::Bell
    }
}

/// One notification, ready to send.
#[derive(Debug, PartialEq)]
pub struct Alert {
    pub title: String,
    pub body: String,
    /// Telegram says to make no sound: the chat's sound is off.
    pub silent: bool,
}

/// The escape code that shows `alert`, or `None` if nothing should go out.
/// `id` tells notifications apart, for terminals that track them. Inside
/// tmux, codes other than the bell must be passed through to the terminal
/// (which needs `set -g allow-passthrough on`).
pub fn escape(method: Notifications, alert: &Alert, id: u64, tmux: bool) -> Option<String> {
    // The codes end at a control character, and a `;` separates fields.
    let title = plain(&alert.title).replace(';', ",");
    let body = plain(&alert.body).replace(';', ",");
    let code = match method {
        Notifications::Auto | Notifications::Off => return None,
        Notifications::Bell if alert.silent => return None,
        // tmux rings the terminal's bell itself.
        Notifications::Bell => return Some("\x07".into()),
        Notifications::Osc9 => format!("\x1b]9;{title}: {body}\x07"),
        Notifications::Osc777 => format!("\x1b]777;notify;{title};{body}\x07"),
        Notifications::Osc99 => {
            let base64 = |text: &str| base64::engine::general_purpose::STANDARD.encode(text);
            let sound = if alert.silent { ":s=silent" } else { "" };
            format!(
                "\x1b]99;i={id}:d=0:o=unfocused{sound}:e=1;{}\x1b\\\
                 \x1b]99;i={id}:d=1:p=body:e=1;{}\x1b\\",
                base64(&title),
                base64(&body)
            )
        }
    };
    Some(if tmux {
        format!("\x1bPtmux;{}\x1b\\", code.replace('\x1b', "\x1b\x1b"))
    } else {
        code
    })
}

/// Sets the window title: "(3) tuigram" with three unmuted chats unread.
pub fn title(unread_chats: i32) -> String {
    if unread_chats > 0 {
        format!("\x1b]2;({unread_chats}) tuigram\x07")
    } else {
        "\x1b]2;tuigram\x07".into()
    }
}

/// Writes a code to the terminal, between frames.
pub fn send(code: &str) {
    let mut out = std::io::stdout();
    let _ = out.write_all(code.as_bytes());
    let _ = out.flush();
}

/// Saves the window title, for [`RESTORE_TITLE`] at exit.
pub const SAVE_TITLE: &str = "\x1b[22;0t";
/// Puts back the title [`SAVE_TITLE`] saved. Terminals that can't clear it
/// instead, so it doesn't stay "tuigram".
pub const RESTORE_TITLE: &str = "\x1b]2;\x07\x1b[23;0t";

/// `text` on one line, without control characters.
fn plain(text: &str) -> String {
    text::clean(text)
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

/// At most `max` characters of `text`, ending with `…` if cut.
fn cut(text: &str, max: usize) -> String {
    if text.chars().count() <= max {
        return text.to_string();
    }
    let mut out: String = text.chars().take(max - 1).collect();
    out.push('…');
    out
}

/// Notifications go out at most this often; what arrives meanwhile waits
/// and goes out together.
const GAP: Duration = Duration::from_secs(3);
/// After a notification for a chat, its messages for this long send none:
/// you already know it's talking.
const QUIET_CHAT: Duration = Duration::from_secs(30);
const MAX_TITLE: usize = 60;
const MAX_BODY: usize = 150;

/// A new message to tell the user about.
#[derive(Clone, Debug)]
pub struct Note {
    /// TDLib's notification id, for when it takes the notification back.
    pub id: i32,
    pub chat_id: i64,
    pub chat: String,
    /// "Alice: see you at 5" in groups, "see you at 5" in private chats.
    pub text: String,
    pub silent: bool,
}

#[derive(Default)]
pub struct Notifier {
    pending: Vec<Note>,
    last_sent: Option<Instant>,
    /// When each chat last had a notification, while it's quiet.
    quiet: HashMap<i64, Instant>,
}

impl Notifier {
    pub fn add(&mut self, note: Note, now: Instant) {
        let quiet = self
            .quiet
            .get(&note.chat_id)
            .is_some_and(|&sent| now < sent + QUIET_CHAT);
        if !quiet {
            self.pending.push(note);
        }
    }

    /// Drops notifications TDLib took back, e.g. because the message was
    /// read on another device.
    pub fn remove(&mut self, ids: &[i32]) {
        self.pending.retain(|n| !ids.contains(&n.id));
    }

    /// Drops what's waiting, e.g. when the user comes back to tuigram.
    pub fn clear(&mut self) {
        self.pending.clear();
    }

    /// What to send now, if anything: everything waiting, as one.
    pub fn due(&mut self, now: Instant) -> Option<Alert> {
        if self.pending.is_empty() || self.last_sent.is_some_and(|sent| now < sent + GAP) {
            return None;
        }
        let notes = std::mem::take(&mut self.pending);
        self.last_sent = Some(now);
        self.quiet.retain(|_, &mut sent| now < sent + QUIET_CHAT);
        for note in &notes {
            self.quiet.insert(note.chat_id, now);
        }
        Some(alert(&notes))
    }

    /// When notifications held back by [`GAP`] can go out.
    pub fn next_at(&self) -> Option<Instant> {
        let at = self.last_sent? + GAP;
        (!self.pending.is_empty()).then_some(at)
    }
}

/// One notification for all of `notes`, which isn't empty.
fn alert(notes: &[Note]) -> Alert {
    let mut chats: Vec<&str> = Vec::new();
    for note in notes {
        if !chats.contains(&note.chat.as_str()) {
            chats.push(&note.chat);
        }
    }
    let (title, body) = match (notes, chats.as_slice()) {
        ([note], _) => (note.chat.clone(), note.text.clone()),
        (_, [chat]) => (chat.to_string(), format!("{} new messages", notes.len())),
        _ => (format!("{} new messages", notes.len()), chats.join(", ")),
    };
    Alert {
        title: cut(&plain(&title), MAX_TITLE),
        body: cut(&plain(&body), MAX_BODY),
        silent: notes.iter().all(|n| n.silent),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn note(id: i32, chat_id: i64, chat: &str, text: &str) -> Note {
        Note {
            id,
            chat_id,
            chat: chat.into(),
            text: text.into(),
            silent: false,
        }
    }

    fn alert(title: &str, body: &str) -> Alert {
        Alert {
            title: title.into(),
            body: body.into(),
            silent: false,
        }
    }

    #[test]
    fn the_terminal_is_recognized_from_its_environment() {
        let detect_with = |vars: &[(&str, &str)]| {
            let vars: HashMap<String, String> = vars
                .iter()
                .map(|(k, v)| (k.to_string(), v.to_string()))
                .collect();
            detect(|name| vars.get(name).cloned())
        };
        assert_eq!(
            detect_with(&[("TERM_PROGRAM", "ghostty")]),
            Notifications::Osc777
        );
        assert_eq!(
            detect_with(&[("TERM_PROGRAM", "tmux"), ("GHOSTTY_RESOURCES_DIR", "/x")]),
            Notifications::Osc777,
            "inside tmux"
        );
        assert_eq!(
            detect_with(&[("TERM", "xterm-kitty")]),
            Notifications::Osc99
        );
        assert_eq!(detect_with(&[("TERM", "foot")]), Notifications::Osc99);
        assert_eq!(
            detect_with(&[("LC_TERMINAL", "iTerm2")]),
            Notifications::Osc9
        );
        assert_eq!(
            detect_with(&[("TERM_PROGRAM", "Apple_Terminal")]),
            Notifications::Bell
        );
        assert_eq!(
            Notifications::Off.resolve(|_| Some("ghostty".into())),
            Notifications::Off,
            "a setting wins"
        );
    }

    #[test]
    fn each_code_carries_the_title_and_text() {
        let hi = alert("Alice", "see you at 5");
        assert_eq!(
            escape(Notifications::Osc777, &hi, 1, false).unwrap(),
            "\x1b]777;notify;Alice;see you at 5\x07"
        );
        assert_eq!(
            escape(Notifications::Osc9, &hi, 1, false).unwrap(),
            "\x1b]9;Alice: see you at 5\x07"
        );
        assert_eq!(
            escape(Notifications::Osc99, &hi, 7, false).unwrap(),
            "\x1b]99;i=7:d=0:o=unfocused:e=1;QWxpY2U=\x1b\\\
             \x1b]99;i=7:d=1:p=body:e=1;c2VlIHlvdSBhdCA1\x1b\\"
        );
        assert_eq!(escape(Notifications::Bell, &hi, 1, false).unwrap(), "\x07");
        assert_eq!(escape(Notifications::Off, &hi, 1, false), None);
    }

    #[test]
    fn silent_chats_ring_no_bell() {
        let quiet = Alert {
            silent: true,
            ..alert("Alice", "hi")
        };
        assert_eq!(escape(Notifications::Bell, &quiet, 1, false), None);
        assert!(
            escape(Notifications::Osc99, &quiet, 1, false)
                .unwrap()
                .contains(":s=silent:")
        );
    }

    #[test]
    fn text_from_others_cannot_end_the_code_early() {
        let sneaky = alert("Eve;x", "a\x07b\x1b]0;pwned\x1b\\c\nd;e");
        assert_eq!(
            escape(Notifications::Osc777, &sneaky, 1, false).unwrap(),
            "\x1b]777;notify;Eve,x;ab]0,pwned\\c d,e\x07"
        );
    }

    #[test]
    fn inside_tmux_codes_are_passed_through() {
        let hi = alert("Alice", "hi");
        assert_eq!(
            escape(Notifications::Osc777, &hi, 1, true).unwrap(),
            "\x1bPtmux;\x1b\x1b]777;notify;Alice;hi\x07\x1b\\"
        );
        assert_eq!(escape(Notifications::Bell, &hi, 1, true).unwrap(), "\x07");
    }

    #[test]
    fn messages_arriving_together_make_one_notification() {
        let mut notifier = Notifier::default();
        let now = Instant::now();
        notifier.add(note(1, 10, "Alice", "hi"), now);
        assert_eq!(notifier.due(now).unwrap(), alert("Alice", "hi"));
        assert_eq!(notifier.due(now), None, "nothing left");

        let later = now + QUIET_CHAT;
        notifier.add(note(2, 10, "Alice", "one"), later);
        notifier.add(note(3, 10, "Alice", "two"), later);
        assert_eq!(
            notifier.due(later).unwrap(),
            alert("Alice", "2 new messages")
        );

        let later = later + QUIET_CHAT;
        notifier.add(note(4, 20, "Bob", "yo"), later);
        notifier.add(note(5, 30, "Dev team", "ship it"), later);
        notifier.add(note(6, 20, "Bob", "?"), later);
        assert_eq!(
            notifier.due(later).unwrap(),
            alert("3 new messages", "Bob, Dev team")
        );
    }

    #[test]
    fn notifications_are_spaced_out_and_busy_chats_stay_quiet() {
        let mut notifier = Notifier::default();
        let now = Instant::now();
        notifier.add(note(1, 10, "Alice", "hi"), now);
        assert!(notifier.due(now).is_some());

        // Bob, a second later, waits for the gap to pass.
        let soon = now + Duration::from_secs(1);
        notifier.add(note(2, 20, "Bob", "yo"), soon);
        assert_eq!(notifier.due(soon), None);
        assert_eq!(notifier.next_at(), Some(now + GAP));
        assert_eq!(notifier.due(now + GAP).unwrap(), alert("Bob", "yo"));

        // Alice again within half a minute: you already know.
        notifier.add(
            note(3, 10, "Alice", "you there?"),
            now + Duration::from_secs(10),
        );
        assert_eq!(notifier.next_at(), None);
        notifier.add(note(4, 10, "Alice", "hello?"), now + QUIET_CHAT);
        assert!(notifier.due(now + QUIET_CHAT).is_some());
    }

    #[test]
    fn notifications_taken_back_are_not_sent() {
        let mut notifier = Notifier::default();
        let now = Instant::now();
        notifier.add(note(1, 10, "Alice", "hi"), now);
        assert!(notifier.due(now).is_some());
        notifier.add(note(2, 20, "Bob", "yo"), now);
        notifier.remove(&[2]);
        assert_eq!(notifier.due(now + GAP), None);
    }

    #[test]
    fn long_text_is_cut_to_one_line() {
        let long = Note {
            text: format!("first line\nsecond {}", "x".repeat(300)),
            ..note(1, 10, "Alice", "")
        };
        let alert = super::alert(&[long]);
        assert!(alert.body.starts_with("first line second xx"));
        assert_eq!(alert.body.chars().count(), MAX_BODY);
        assert!(alert.body.ends_with('…'));
    }
}

//! Secret chats: end-to-end encrypted chats with one person, which live only
//! on the device that started or accepted them. `:secret` starts one, `:key`
//! shows the picture of its key to compare with the other person's, `:timer`
//! sets how long messages last once seen, and `:leave` ends it. Also the
//! self-destruct timers of messages, which view-once photos in other chats
//! have too.

use std::time::{Duration, SystemTime};

use base64::Engine;
use tdlib_rs::enums::{MessageContent, MessageSelfDestructType, SecretChatState};
use tdlib_rs::types::{self, Message};

/// A secret chat, from `updateSecretChat`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Secret {
    /// The person it's with.
    pub user_id: i64,
    pub state: SecretState,
    /// You started it.
    pub outbound: bool,
    /// What both sides' apps show to compare, to tell that nobody is in
    /// between: 36 bytes once the chat is ready.
    pub key_hash: Vec<u8>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SecretState {
    /// Waiting for the other person's app to come online and accept it.
    Pending,
    Ready,
    /// Ended, by either side: nothing more can be sent.
    Closed,
}

impl Secret {
    pub fn of(chat: &types::SecretChat) -> Self {
        let state = match chat.state {
            SecretChatState::Pending => SecretState::Pending,
            SecretChatState::Ready => SecretState::Ready,
            SecretChatState::Closed => SecretState::Closed,
        };
        Self {
            user_id: chat.user_id,
            state,
            outbound: chat.is_outbound,
            // TDLib's JSON sends bytes as base64.
            key_hash: base64::engine::general_purpose::STANDARD
                .decode(&chat.key_hash)
                .unwrap_or_default(),
        }
    }

    /// Why nothing can be sent yet, or any more; `None` once it's ready.
    /// `name` is the other person's.
    pub fn cant_send(&self, name: &str) -> Option<String> {
        match self.state {
            SecretState::Ready => None,
            SecretState::Pending => Some(format!("Waiting for {name} to come online")),
            SecretState::Closed => Some("This secret chat has ended".into()),
        }
    }
}

/// The four colors of the key's picture, as Telegram's apps draw it.
pub const KEY_COLORS: [(u8, u8, u8); 4] = [
    (0xff, 0xff, 0xff),
    (0xd5, 0xe6, 0xf3),
    (0x2d, 0x57, 0x75),
    (0x2f, 0x99, 0xc9),
];
/// Pixels across, and down, the key's picture.
pub const KEY_SIDE: usize = 12;

/// The picture Telegram's apps draw from a secret chat's key, row by row,
/// each pixel an index into [`KEY_COLORS`]: two bits of the key each, from
/// the lowest bits of each byte up. `None` without a whole key.
pub fn key_picture(hash: &[u8]) -> Option<[[u8; KEY_SIDE]; KEY_SIDE]> {
    if hash.len() * 8 < KEY_SIDE * KEY_SIDE * 2 {
        return None;
    }
    let mut rows = [[0; KEY_SIDE]; KEY_SIDE];
    for (i, pixel) in rows.iter_mut().flatten().enumerate() {
        let bit = i * 2;
        *pixel = (hash[bit / 8] >> (bit % 8)) & 0b11;
    }
    Some(rows)
}

/// The key's first 32 bytes in hex, eight to a line with a wider gap after
/// four, as Telegram's apps print them under the picture.
pub fn key_hex(hash: &[u8]) -> Vec<String> {
    hash[..hash.len().min(32)]
        .chunks(8)
        .map(|line| {
            let half = |bytes: &[u8]| {
                bytes
                    .iter()
                    .map(|b| format!("{b:02x}"))
                    .collect::<Vec<_>>()
                    .join(" ")
            };
            let (left, right) = line.split_at(line.len().min(4));
            if right.is_empty() {
                half(left)
            } else {
                format!("{}  {}", half(left), half(right))
            }
        })
        .collect()
}

/// `:key`: the secret chat's key, to compare with the other person's.
pub struct KeyView {
    pub chat_id: i64,
    /// Whom the chat is with.
    pub with: String,
    pub hash: Vec<u8>,
}

const MINUTE: i32 = 60;
const HOUR: i32 = 60 * MINUTE;
const DAY: i32 = 24 * HOUR;
const WEEK: i32 = 7 * DAY;

/// The self-destruct timers `:timer` offers, in seconds, as Telegram's apps
/// do; 0 turns it off.
pub const TIMERS: [i32; 13] = [0, 1, 2, 3, 4, 5, 10, 15, 30, MINUTE, HOUR, DAY, WEEK];

/// A timer in words, for the `:timer` popup: "Off", "1 second", "1 week".
pub fn timer_words(secs: i32) -> String {
    let (count, unit) = match secs {
        ..=0 => return "Off".into(),
        s if s % WEEK == 0 => (s / WEEK, "week"),
        s if s % DAY == 0 => (s / DAY, "day"),
        s if s % HOUR == 0 => (s / HOUR, "hour"),
        s if s % MINUTE == 0 => (s / MINUTE, "minute"),
        s => (s, "second"),
    };
    if count == 1 {
        format!("1 {unit}")
    } else {
        format!("{count} {unit}s")
    }
}

/// A timer in short, for a chat's title and a bubble: "30s", "1m", "1w".
pub fn timer_label(secs: i32) -> String {
    match secs {
        ..=0 => "off".into(),
        s if s % WEEK == 0 => format!("{}w", s / WEEK),
        s if s % DAY == 0 => format!("{}d", s / DAY),
        s if s % HOUR == 0 => format!("{}h", s / HOUR),
        s if s % MINUTE == 0 => format!("{}m", s / MINUTE),
        s => format!("{s}s"),
    }
}

/// `:timer`: how long new messages in a secret chat last once seen.
pub struct TimerMenu {
    pub chat_id: i64,
    /// What it offers: [`TIMERS`], and the chat's own if the other
    /// person's app set another, in order.
    pub choices: Vec<i32>,
    /// Index into `choices`.
    pub selected: usize,
}

impl TimerMenu {
    /// Opens with the cursor on the timer the chat has, never on "Off"
    /// unless it's off: Enter out of habit then changes nothing.
    pub fn new(chat_id: i64, current: i32) -> Self {
        let current = current.max(0);
        let mut choices = TIMERS.to_vec();
        if !choices.contains(&current) {
            choices.push(current);
            choices.sort_unstable();
        }
        let selected = choices.iter().position(|&t| t == current).unwrap_or(0);
        Self {
            chat_id,
            choices,
            selected,
        }
    }
}

/// A message's self-destruct timer: in a secret chat that has one, or a
/// view-once photo in any chat with one person.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Destruct {
    /// Seconds it lasts once its timer starts; 0 for view once, gone as
    /// soon as it's been opened.
    pub after: i32,
    /// Its timer starts when it's opened (a photo shown only while open, a
    /// voice message played), not when it's read.
    pub on_open: bool,
    /// When it's gone, once its timer started.
    pub ends: Option<SystemTime>,
}

impl Destruct {
    /// The message's timer, if it has one. `now` is when TDLib sent it,
    /// for how long it had left then.
    pub fn of(message: &Message, now: SystemTime) -> Option<Self> {
        let after = match message.self_destruct_type.as_ref()? {
            MessageSelfDestructType::Timer(t) => t.self_destruct_time.max(1),
            MessageSelfDestructType::Immediately => 0,
        };
        let ends = Some(message.self_destruct_in)
            .filter(|left| *left > 0.0)
            .and_then(|left| Duration::try_from_secs_f64(left).ok())
            .and_then(|left| now.checked_add(left));
        // As TDLib decides: short timers on media start once it's opened.
        let media = matches!(
            message.content,
            MessageContent::MessageAnimation(_)
                | MessageContent::MessageAudio(_)
                | MessageContent::MessagePhoto(_)
                | MessageContent::MessageVideo(_)
                | MessageContent::MessageVideoNote(_)
                | MessageContent::MessageVoiceNote(_)
        );
        Some(Self {
            after,
            on_open: after == 0 || (after <= MINUTE && media),
            ends,
        })
    }

    /// Its timer starts now, unless it already did: TDLib started it when
    /// the message was read or opened, and deletes the message once it's up.
    pub fn start(&mut self, now: SystemTime) {
        if self.ends.is_none() && self.after > 0 {
            self.ends = now.checked_add(Duration::from_secs(self.after as u64));
        }
    }

    /// What the bubble says beside its time: "🔥 30s" before the timer
    /// starts, then how long is left, "🔥 25s"; "🔥 once" for view once.
    pub fn label(&self, now: SystemTime) -> String {
        match self.ends {
            Some(ends) => match left(ends, now) {
                Some(left) => format!("🔥 {}", countdown(left).0),
                None => "🔥".into(),
            },
            None if self.after == 0 => "🔥 once".into(),
            None => format!("🔥 {}", timer_label(self.after)),
        }
    }

    /// How long until [`Destruct::label`] says something else, while its
    /// timer runs.
    pub fn changes_in(&self, now: SystemTime) -> Option<Duration> {
        let left = left(self.ends?, now)?;
        Duration::try_from_secs_f64(countdown(left).1).ok()
    }
}

/// Seconds left until `ends`, if any are.
fn left(ends: SystemTime, now: SystemTime) -> Option<f64> {
    let left = ends.duration_since(now).ok()?.as_secs_f64();
    (left > 0.0).then_some(left)
}

/// Time left, as a countdown: "25s", "4:05", "5h 03m", "6d 23h", counting
/// whole seconds up; and in how many seconds that changes.
fn countdown(left: f64) -> (String, f64) {
    let s = left.ceil().max(1.0) as u64;
    let (label, unit) = match s {
        0..60 => (format!("{s}s"), 1),
        60..3600 => (format!("{}:{:02}", s / 60, s % 60), 1),
        3600..86400 => (format!("{}h {:02}m", s / 3600, s % 3600 / 60), 60),
        _ => (format!("{}d {}h", s / 86400, s % 86400 / 3600), 3600),
    };
    // It says the same until there's a whole unit less.
    let next = left - ((s / unit * unit) as f64 - 1.0);
    (label, next.max(0.001))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_key_picture_reads_two_bits_a_pixel_from_the_low_bits_up() {
        // 0b11_10_01_00: the pixels 0, 1, 2, 3, from the lowest bits.
        let mut hash = vec![0b1110_0100; 36];
        hash[35] = 0b0000_0011;
        let picture = key_picture(&hash).unwrap();
        assert_eq!(picture[0][..4], [0, 1, 2, 3]);
        assert_eq!(
            picture[11][8..],
            [3, 0, 0, 0],
            "the last byte fills the end"
        );
        assert_eq!(key_picture(&hash[..16]), None, "a short key has no picture");
    }

    #[test]
    fn the_key_prints_as_four_lines_of_eight_bytes() {
        let hash: Vec<u8> = (0..36).collect();
        let lines = key_hex(&hash);
        assert_eq!(lines.len(), 4, "only the first 32 bytes");
        assert_eq!(lines[0], "00 01 02 03  04 05 06 07");
        assert_eq!(lines[3], "18 19 1a 1b  1c 1d 1e 1f");
    }

    #[test]
    fn timers_read_in_their_biggest_whole_unit() {
        let labels: Vec<String> = TIMERS.iter().map(|&t| timer_label(t)).collect();
        assert_eq!(
            labels,
            [
                "off", "1s", "2s", "3s", "4s", "5s", "10s", "15s", "30s", "1m", "1h", "1d", "1w"
            ]
        );
        assert_eq!(timer_words(1), "1 second");
        assert_eq!(timer_words(30), "30 seconds");
        assert_eq!(timer_words(WEEK), "1 week");
        assert_eq!(timer_words(0), "Off");
        assert_eq!(timer_label(90), "90s");
    }

    #[test]
    fn the_timer_popup_offers_the_chats_own_timer_even_if_it_is_unusual() {
        let menu = TimerMenu::new(1, 20);
        assert_eq!(menu.choices[menu.selected], 20, "the cursor on it");
        assert_eq!(menu.choices.len(), TIMERS.len() + 1);
        assert!(menu.choices.is_sorted());
        let menu = TimerMenu::new(1, 30);
        assert_eq!(menu.choices, TIMERS, "nothing added");
        assert_eq!(menu.choices[menu.selected], 30);
    }

    #[test]
    fn a_countdown_counts_whole_seconds_up_and_says_when_it_changes() {
        let (label, next) = countdown(25.3);
        assert_eq!(label, "26s");
        assert!((next - 0.3).abs() < 1e-9, "26s until 25 are left: {next}");
        assert_eq!(countdown(25.0), ("25s".into(), 1.0));
        assert_eq!(countdown(245.0).0, "4:05");
        assert_eq!(countdown(5.0 * 3600.0 + 190.0).0, "5h 03m");
        let (label, next) = countdown(6.0 * 86400.0 + 23.0 * 3600.0 + 10.0);
        assert_eq!(label, "6d 23h");
        assert_eq!(next, 11.0, "an hour less in 11 seconds");
        // An hour is shown in minutes until it's under one.
        assert_eq!(countdown(3600.0).0, "1h 00m");
        assert_eq!(countdown(3599.5).0, "1h 00m");
        assert_eq!(countdown(3599.0).0, "59:59");
    }

    #[test]
    fn a_timer_says_how_long_until_it_starts_then_how_long_is_left() {
        let now = SystemTime::UNIX_EPOCH + Duration::from_secs(1_800_000_000);
        let mut destruct = Destruct {
            after: 30,
            on_open: false,
            ends: None,
        };
        assert_eq!(destruct.label(now), "🔥 30s");
        assert_eq!(destruct.changes_in(now), None, "nothing to count down yet");
        destruct.start(now);
        assert_eq!(destruct.label(now), "🔥 30s");
        let later = now + Duration::from_millis(5_500);
        assert_eq!(destruct.label(later), "🔥 25s");
        assert_eq!(destruct.changes_in(later), Some(Duration::from_millis(500)));
        // Started once: reading it again doesn't start it over.
        destruct.start(later);
        assert_eq!(destruct.label(later), "🔥 25s");
        assert_eq!(destruct.label(now + Duration::from_secs(31)), "🔥");

        let once = Destruct {
            after: 0,
            on_open: true,
            ends: None,
        };
        assert_eq!(once.label(now), "🔥 once");
    }
}

//! Pinned messages: `P` pins the message under the cursor, or unpins it; a
//! bar over the chat shows the newest pinned one, and `gp` lists them all to
//! go to.

use crate::messages::{Msg, Sender};

/// A pinned message, as the bar and the `gp` list show it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Pinned {
    pub id: i64,
    pub sender: Sender,
    pub outgoing: bool,
    /// Unix timestamp.
    pub date: i32,
    /// The message on one line, spoilers hidden.
    pub snippet: String,
}

impl Pinned {
    pub fn new(id: i64, msg: &Msg) -> Self {
        Self {
            id,
            sender: msg.sender,
            outgoing: msg.outgoing,
            date: msg.date,
            snippet: msg.snippet(),
        }
    }
}

/// How a message is pinned: in a chat with one person, for both of you or
/// just you; in a group or channel, telling everyone or not.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PinChoice {
    /// In Saved Messages, where nobody else is.
    Pin,
    ForBoth,
    ForMe,
    Notify,
    Quietly,
}

impl PinChoice {
    /// What the popup says; `with` is whom a one-on-one chat is with.
    pub fn label(self, with: &str, channel: bool) -> String {
        match self {
            PinChoice::Pin => "Pin".into(),
            PinChoice::ForBoth => format!("Pin for me and {with}"),
            PinChoice::ForMe => "Pin just for me".into(),
            PinChoice::Notify if channel => "Pin and notify subscribers".into(),
            PinChoice::Notify => "Pin and notify members".into(),
            PinChoice::Quietly => "Pin without notifying".into(),
        }
    }

    /// TDLib's `disable_notification` and `only_for_self`.
    pub fn flags(self) -> (bool, bool) {
        match self {
            PinChoice::Pin | PinChoice::ForBoth | PinChoice::Notify => (false, false),
            PinChoice::ForMe => (false, true),
            PinChoice::Quietly => (true, false),
        }
    }
}

/// The chat a message is pinned in, for which [`PinChoice`]s it offers.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Place {
    Saved,
    /// A chat with one person.
    Private,
    Group,
    Channel,
}

/// The `P` popup: how to pin the message under the cursor.
pub struct PinMenu {
    pub message_id: i64,
    /// The message on one line, so it's clear which one is pinned.
    pub snippet: String,
    pub place: Place,
    /// Whom a one-on-one chat is with.
    pub with: String,
    /// Empty until TDLib says the message can be pinned.
    pub choices: Vec<PinChoice>,
    pub selected: usize,
}

impl PinMenu {
    pub fn new(message_id: i64, snippet: String, place: Place, with: String) -> Self {
        Self {
            message_id,
            snippet,
            place,
            with,
            choices: Vec::new(),
            selected: 0,
        }
    }

    /// TDLib said it can be pinned. The cursor starts on what's easy to
    /// live with: pinned for both, as Telegram does, in a chat with one
    /// person; without a notification to everyone in a group or channel.
    pub fn allow(&mut self) {
        let (choices, selected) = match self.place {
            Place::Saved => (vec![PinChoice::Pin], 0),
            Place::Private => (vec![PinChoice::ForBoth, PinChoice::ForMe], 0),
            Place::Group | Place::Channel => (vec![PinChoice::Notify, PinChoice::Quietly], 1),
        };
        self.choices = choices;
        self.selected = selected;
    }

    pub fn label(&self, choice: PinChoice) -> String {
        choice.label(&self.with, self.place == Place::Channel)
    }
}

/// The `gp` popup: the chat's pinned messages, newest first, to go to.
/// The cursor is kept on a message, not a row: pins come and go while it's
/// open (anyone can pin), and `P` unpins for everyone.
pub struct PinnedMenu {
    /// The message under the cursor.
    message_id: i64,
    /// Its row when last moved to, for `j` / `k` to go on from once it's
    /// no longer pinned.
    row: usize,
}

impl PinnedMenu {
    /// On the newest pinned message; `None` without any.
    pub fn new(pinned: &[Pinned]) -> Option<Self> {
        let first = pinned.first()?;
        Some(Self {
            message_id: first.id,
            row: 0,
        })
    }

    /// The row of the message under the cursor; `None` once it's no longer
    /// pinned, so Enter and `P` can't act on another one in its place.
    pub fn row(&self, pinned: &[Pinned]) -> Option<usize> {
        pinned.iter().position(|p| p.id == self.message_id)
    }

    /// The message under the cursor, while it's pinned.
    pub fn current<'a>(&self, pinned: &'a [Pinned]) -> Option<&'a Pinned> {
        pinned.get(self.row(pinned)?)
    }

    /// Down (`delta` > 0) or up the list, from where the cursor is or was.
    pub fn move_by(&mut self, pinned: &[Pinned], delta: isize) {
        let from = self.row(pinned).unwrap_or(self.row);
        let row = from
            .saturating_add_signed(delta)
            .min(pinned.len().saturating_sub(1));
        if let Some(p) = pinned.get(row) {
            (self.message_id, self.row) = (p.id, row);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pinned(ids: &[i64]) -> Vec<Pinned> {
        ids.iter()
            .map(|&id| Pinned {
                id,
                sender: Sender::User(1),
                outgoing: false,
                date: 0,
                snippet: format!("message {id}"),
            })
            .collect()
    }

    #[test]
    fn the_list_cursor_stays_on_its_message_as_pins_come_and_go() {
        let list = pinned(&[9, 7, 5]);
        let mut menu = PinnedMenu::new(&list).unwrap();
        menu.move_by(&list, 1);
        assert_eq!(menu.current(&list).unwrap().id, 7);

        // Someone pins another message, which goes on top.
        let list = pinned(&[12, 9, 7, 5]);
        assert_eq!(menu.current(&list).unwrap().id, 7, "not 9, now in its row");
        // And someone unpins it: nothing under the cursor to unpin.
        let list = pinned(&[12, 9, 5]);
        assert_eq!(menu.current(&list), None);
        assert_eq!(menu.row(&list), None);
        menu.move_by(&list, 1);
        assert_eq!(
            menu.current(&list).unwrap().id,
            5,
            "down from the row it was in"
        );
        assert!(PinnedMenu::new(&[]).is_none());
    }

    #[test]
    fn the_choices_fit_the_chat_and_start_on_the_gentle_one() {
        let mut menu = PinMenu::new(1, "hi".into(), Place::Private, "Maya".into());
        assert!(menu.choices.is_empty(), "until TDLib says it can be pinned");
        menu.allow();
        let labels: Vec<String> = menu.choices.iter().map(|&c| menu.label(c)).collect();
        assert_eq!(labels, ["Pin for me and Maya", "Pin just for me"]);
        assert_eq!(menu.choices[menu.selected], PinChoice::ForBoth);
        assert_eq!(PinChoice::ForMe.flags(), (false, true));

        let mut menu = PinMenu::new(1, "hi".into(), Place::Channel, String::new());
        menu.allow();
        assert_eq!(menu.label(menu.choices[0]), "Pin and notify subscribers");
        assert_eq!(menu.choices[menu.selected], PinChoice::Quietly);
        assert_eq!(PinChoice::Quietly.flags(), (true, false));
    }
}

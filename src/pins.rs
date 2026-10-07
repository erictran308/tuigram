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
#[derive(Default)]
pub struct PinnedMenu {
    pub selected: usize,
}

#[cfg(test)]
mod tests {
    use super::*;

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

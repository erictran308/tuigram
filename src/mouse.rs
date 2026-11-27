//! What the mouse points at: the panes' cells as they were last drawn, and
//! which chat, topic or message each of their rows is. The ui fills the map
//! while drawing a frame; a click or a wheel turn then reads it. Everything
//! the app.rs mouse handler needs to know about the screen lives here, the
//! `?` popup's tabs and rows too ([`Popup`]).

use ratatui::layout::Rect;

/// The panes as drawn last frame, with each row's contents: rows are
/// (first row, past the last, what they show).
#[derive(Default)]
pub struct Map {
    /// The chat list.
    pub chats: Rect,
    pub chat_rows: Vec<(u16, u16, i64)>,
    /// A forum's topics, when the pane shows.
    pub topics: Option<Rect>,
    pub topic_rows: Vec<(u16, u16, i32)>,
    /// The message history.
    pub history: Rect,
    pub message_rows: Vec<(u16, u16, i64)>,
    /// The composer.
    pub composer: Rect,
}

impl Map {
    /// Empties the map for a new frame: the panes and rows are written back
    /// as they're drawn, so a pane not drawn (no chat open) stays empty.
    pub fn reset(&mut self) {
        *self = Map::default();
    }

    /// The chat whose rows hold (x, y), if the list is there.
    pub fn chat_at(&self, x: u16, y: u16) -> Option<i64> {
        if !self.chats.contains((x, y).into()) {
            return None;
        }
        self.chat_rows
            .iter()
            .find(|(from, to, _)| (*from..*to).contains(&y))
            .map(|(_, _, id)| *id)
    }

    /// The topic whose rows hold (x, y), if the pane shows.
    pub fn topic_at(&self, x: u16, y: u16) -> Option<i32> {
        if !self.topics.is_some_and(|r| r.contains((x, y).into())) {
            return None;
        }
        self.topic_rows
            .iter()
            .find(|(from, to, _)| (*from..*to).contains(&y))
            .map(|(_, _, id)| *id)
    }

    /// The message whose rows hold (x, y), if the history is there.
    pub fn message_at(&self, x: u16, y: u16) -> Option<i64> {
        if !self.history.contains((x, y).into()) {
            return None;
        }
        self.message_rows
            .iter()
            .find(|(from, to, _)| (*from..*to).contains(&y))
            .map(|(_, _, id)| *id)
    }

    /// The wheel over the chat list, the topics or the history: which pane
    /// it scrolls.
    pub fn pane_at(&self, x: u16, y: u16) -> Option<Pane> {
        if self.chats.contains((x, y).into()) {
            Some(Pane::Chats)
        } else if self.topics.is_some_and(|r| r.contains((x, y).into())) {
            Some(Pane::Topics)
        } else if self.history.contains((x, y).into()) {
            Some(Pane::Messages)
        } else {
            None
        }
    }

    /// (x, y) is over the composer.
    pub fn over_composer(&self, x: u16, y: u16) -> bool {
        self.composer.contains((x, y).into())
    }
}

/// A pane the mouse can be over.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Pane {
    Chats,
    Topics,
    Messages,
}

/// The `?` popup as drawn last frame: where its tabs and its rows are.
#[derive(Clone, Debug, Default)]
pub struct Popup {
    /// The popup, borders included: the wheel turns over all of it.
    pub area: Rect,
    /// The columns each tab's label takes on the top border, in the order
    /// drawn: the shortcuts, then the settings.
    pub tabs: [(u16, u16); 2],
    /// Where the rows are drawn, and which setting each row of it shows
    /// from its top: `None` for headings, notes and gaps.
    pub list: Rect,
    pub rows: Vec<Option<usize>>,
}

impl Popup {
    /// (x, y) is over the popup.
    pub fn contains(&self, x: u16, y: u16) -> bool {
        self.area.contains((x, y).into())
    }

    /// The tab whose label is at (x, y): 0 the shortcuts, 1 the settings.
    pub fn tab_at(&self, x: u16, y: u16) -> Option<usize> {
        if y != self.area.y {
            return None;
        }
        self.tabs
            .iter()
            .position(|&(from, to)| (from..to).contains(&x))
    }

    /// The setting whose row is at (x, y).
    pub fn setting_at(&self, x: u16, y: u16) -> Option<usize> {
        if !self.list.contains((x, y).into()) {
            return None;
        }
        self.rows
            .get(usize::from(y - self.list.y))
            .copied()
            .flatten()
    }
}

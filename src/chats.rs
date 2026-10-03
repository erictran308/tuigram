//! The main chat list, rebuilt from TDLib updates.
//!
//! TDLib gives every chat an `order` per chat list. The list is the chats with a
//! non-zero order, sorted by (order, chat id) descending. On top of that, chats
//! with unread messages come first. A `/` search narrows the list to chats
//! whose title matches.

use std::collections::{HashMap, HashSet};

use tdlib_rs::enums::{ChatList, ChatType, MessageContent};
use tdlib_rs::types::{self, ChatPosition, Message};

use crate::search;
use crate::text;

pub struct Chat {
    pub title: String,
    /// Channel posts all come from the channel, so they show no sender name.
    pub is_channel: bool,
    pub unread: i32,
    /// One-line summary of the last message, e.g. "You: see you at 5".
    pub preview: String,
    /// Position in the main list; 0 means the chat isn't in it (e.g. archived).
    order: i64,
}

/// What Telegram calls your chat with yourself.
const SAVED_MESSAGES: &str = "Saved Messages";

#[derive(Default)]
pub struct Chats {
    by_id: HashMap<i64, Chat>,
    sorted: Vec<i64>,
    dirty: bool,
    /// Your own user id. Your chat with yourself has the same id.
    my_id: Option<i64>,
    /// An unread chat that was opened. It stays with the unread chats after
    /// it's read, until another chat is opened, so it doesn't jump away while
    /// you read it.
    held: Option<i64>,
    /// Chats you highlighted with `H`. Saved in the settings file.
    highlighted: HashSet<i64>,
    /// Only chats whose title contains this, in any case, are listed.
    filter: String,
    /// Chats in the main list, before the filter.
    total: usize,
}

impl Chats {
    pub fn insert(&mut self, chat: types::Chat) {
        let is_channel = matches!(&chat.r#type, ChatType::Supergroup(s) if s.is_channel);
        let entry = Chat {
            title: text::clean(&chat.title),
            is_channel,
            unread: chat.unread_count,
            preview: chat.last_message.as_ref().map(preview).unwrap_or_default(),
            order: main_order(&chat.positions).unwrap_or(0),
        };
        self.by_id.insert(chat.id, entry);
        self.dirty = true;
    }

    pub fn set_position(&mut self, chat_id: i64, position: &ChatPosition) {
        if let ChatList::Main = position.list
            && let Some(chat) = self.by_id.get_mut(&chat_id)
        {
            chat.order = position.order;
            self.dirty = true;
        }
    }

    pub fn set_last_message(
        &mut self,
        chat_id: i64,
        message: Option<&Message>,
        positions: &[ChatPosition],
    ) {
        if let Some(chat) = self.by_id.get_mut(&chat_id) {
            chat.preview = message.map(preview).unwrap_or_default();
            if let Some(order) = main_order(positions) {
                chat.order = order;
                self.dirty = true;
            }
        }
    }

    pub fn set_title(&mut self, chat_id: i64, title: String) {
        if let Some(chat) = self.by_id.get_mut(&chat_id) {
            chat.title = text::clean(&title);
            // It may match the filter now, or no longer.
            self.dirty = true;
        }
    }

    pub fn set_unread(&mut self, chat_id: i64, unread: i32) {
        if let Some(chat) = self.by_id.get_mut(&chat_id) {
            chat.unread = unread;
            self.dirty = true;
        }
    }

    /// Call when a chat is opened, before it's marked as read.
    pub fn opened(&mut self, chat_id: i64) {
        self.held = Some(chat_id).filter(|id| self.by_id.get(id).is_some_and(|c| c.unread > 0));
        self.dirty = true;
    }

    /// Re-sorts after updates. Call once per batch, before reading `ids`.
    pub fn refresh(&mut self) {
        if !self.dirty {
            return;
        }
        self.sorted = self
            .by_id
            .iter()
            .filter(|(_, chat)| chat.order != 0)
            .map(|(&id, _)| id)
            .collect();
        self.total = self.sorted.len();
        if !self.filter.is_empty() {
            let mut sorted = std::mem::take(&mut self.sorted);
            sorted.retain(|&id| {
                let title = self.title(id).unwrap_or_default();
                !search::find(title, &self.filter).is_empty()
            });
            self.sorted = sorted;
        }
        let (by_id, held) = (&self.by_id, self.held);
        self.sorted.sort_unstable_by_key(|&id| {
            let unread = by_id[&id].unread > 0 || held == Some(id);
            std::cmp::Reverse((unread, by_id[&id].order, id))
        });
        self.dirty = false;
    }

    /// Lists only chats whose title contains `query`; empty lists them all.
    pub fn set_filter(&mut self, query: &str) {
        if self.filter != query {
            self.filter = query.to_string();
            self.dirty = true;
        }
    }

    pub fn filter(&self) -> &str {
        &self.filter
    }

    /// How many chats are in the main list, filtered out or not.
    pub fn total(&self) -> usize {
        self.total
    }

    pub fn set_my_id(&mut self, id: i64) {
        self.my_id = Some(id);
    }

    /// True for the chat with yourself, which Telegram shows as Saved Messages.
    pub fn is_saved(&self, chat_id: i64) -> bool {
        self.my_id == Some(chat_id)
    }

    pub fn is_highlighted(&self, chat_id: i64) -> bool {
        self.highlighted.contains(&chat_id)
    }

    /// Highlights the chat, or removes its highlight. Returns all highlighted
    /// chats, sorted, for saving.
    pub fn toggle_highlight(&mut self, chat_id: i64) -> Vec<i64> {
        if !self.highlighted.remove(&chat_id) {
            self.highlighted.insert(chat_id);
        }
        let mut ids: Vec<i64> = self.highlighted.iter().copied().collect();
        ids.sort_unstable();
        ids
    }

    pub fn set_highlighted(&mut self, chat_ids: &[i64]) {
        self.highlighted = chat_ids.iter().copied().collect();
    }

    /// The title to show: "Saved Messages" for your own chat, else the chat's title.
    pub fn title(&self, chat_id: i64) -> Option<&str> {
        if self.is_saved(chat_id) {
            return Some(SAVED_MESSAGES);
        }
        self.by_id.get(&chat_id).map(|c| c.title.as_str())
    }

    /// Chat ids in display order.
    pub fn ids(&self) -> &[i64] {
        &self.sorted
    }

    pub fn get(&self, chat_id: i64) -> Option<&Chat> {
        self.by_id.get(&chat_id)
    }
}

fn main_order(positions: &[ChatPosition]) -> Option<i64> {
    positions
        .iter()
        .find(|p| matches!(p.list, ChatList::Main))
        .map(|p| p.order)
}

fn preview(message: &Message) -> String {
    let text = text::clean(&content_text(&message.content)).replace(['\n', '\t'], " ");
    if message.is_outgoing {
        format!("You: {text}")
    } else {
        text
    }
}

/// Plain-text rendering of a message body; media becomes a `[Label]`.
pub fn content_text(content: &MessageContent) -> String {
    let labeled = |label: &str, caption: &str| {
        if caption.is_empty() {
            format!("[{label}]")
        } else {
            format!("[{label}] {caption}")
        }
    };
    match content {
        MessageContent::MessageText(m) => m.text.text.clone(),
        MessageContent::MessagePhoto(m) => labeled("Photo", &m.caption.text),
        MessageContent::MessageVideo(m) => labeled("Video", &m.caption.text),
        MessageContent::MessageAnimation(m) => labeled("GIF", &m.caption.text),
        MessageContent::MessageDocument(m) => {
            labeled(&format!("File: {}", m.document.file_name), &m.caption.text)
        }
        MessageContent::MessageAudio(m) => labeled("Audio", &m.caption.text),
        MessageContent::MessageVoiceNote(m) => labeled("Voice message", &m.caption.text),
        MessageContent::MessageVideoNote(_) => "[Video message]".into(),
        MessageContent::MessageSticker(m) => format!("[Sticker {}]", m.sticker.emoji),
        MessageContent::MessagePoll(_) => "[Poll]".into(),
        MessageContent::MessageLocation(_) => "[Location]".into(),
        MessageContent::MessageContact(_) => "[Contact]".into(),
        _ => "[Message]".into(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Chats with the given (id, order, unread), as TDLib would report them.
    fn chats(list: &[(i64, i64, i32)]) -> Chats {
        let mut chats = Chats::default();
        for &(id, order, unread) in list {
            chats.by_id.insert(
                id,
                Chat {
                    title: format!("chat {id}"),
                    is_channel: false,
                    unread,
                    preview: String::new(),
                    order,
                },
            );
        }
        chats.dirty = true;
        chats.refresh();
        chats
    }

    #[test]
    fn unread_chats_come_first_in_telegram_order() {
        let mut list = chats(&[(1, 50, 0), (2, 40, 3), (3, 30, 0), (4, 20, 1), (5, 0, 9)]);
        // 5 has order 0: not in the main list (e.g. archived).
        assert_eq!(list.ids(), [2, 4, 1, 3]);

        list.set_unread(3, 2);
        list.refresh();
        assert_eq!(list.ids(), [2, 3, 4, 1], "a new message moves it up");
    }

    #[test]
    fn an_opened_chat_keeps_its_place_until_another_is_opened() {
        let mut list = chats(&[(1, 50, 0), (2, 40, 3), (3, 30, 1)]);
        list.opened(3);
        list.set_unread(3, 0); // read while open
        list.refresh();
        assert_eq!(list.ids(), [2, 3, 1], "stays with the unread chats");

        list.opened(2);
        list.refresh();
        assert_eq!(list.ids(), [2, 1, 3], "drops to its place once you move on");
    }

    #[test]
    fn the_filter_keeps_matching_titles_in_order() {
        let mut list = chats(&[(1, 50, 0), (2, 40, 3), (3, 30, 0)]);
        list.set_title(1, "Alice".into());
        list.set_title(2, "Bob".into());
        list.set_title(3, "alina".into());
        list.set_my_id(4);
        list.by_id.insert(
            4,
            Chat {
                title: "Eric".into(),
                is_channel: false,
                unread: 0,
                preview: String::new(),
                order: 10,
            },
        );

        list.set_filter("ALI");
        list.refresh();
        assert_eq!(list.ids(), [1, 3]);
        assert_eq!(list.total(), 4);

        list.set_filter("saved");
        list.refresh();
        assert_eq!(list.ids(), [4], "Saved Messages matches by its shown name");

        list.set_filter("");
        list.refresh();
        assert_eq!(list.ids(), [2, 1, 3, 4]);
    }

    #[test]
    fn highlights_toggle() {
        let mut list = chats(&[(1, 50, 0), (2, 40, 0)]);
        assert_eq!(list.toggle_highlight(2), [2]);
        assert_eq!(list.toggle_highlight(1), [1, 2]);
        assert!(list.is_highlighted(2));
        assert_eq!(list.toggle_highlight(2), [1]);
        assert!(!list.is_highlighted(2));
    }
}

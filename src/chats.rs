//! The main chat list, rebuilt from TDLib updates.
//!
//! TDLib gives every chat an `order` per chat list. The list is the chats with a
//! non-zero order, sorted by (order, chat id) descending. On top of that, chats
//! with unread messages come first. A `/` search narrows the list to chats
//! whose title matches.

use std::collections::{HashMap, HashSet};

use tdlib_rs::enums::{ChatAction, ChatList, ChatType, MessageContent, MessageSender, UserStatus};
use tdlib_rs::types::{self, AccentColor, ChatPhotoInfo, ChatPosition, FormattedText, Message};

use crate::images::Thumbnail;
use crate::messages::{Sender, decode_minithumbnail, without_spoilers};
use crate::search;
use crate::text;

pub struct Chat {
    pub title: String,
    /// Channel posts all come from the channel, so they show no sender name.
    pub is_channel: bool,
    /// A one-on-one chat, where only the other person can be typing.
    pub is_private: bool,
    pub unread: i32,
    /// Your messages up to this id have been read: by the other person, or
    /// by anyone in a group.
    pub read_outbox: i64,
    /// One-line summary of the last message, e.g. "You: see you at 5".
    pub preview: String,
    /// Position in the main list; 0 means the chat isn't in it (e.g. archived).
    order: i64,
    pub photo: Option<ChatPhoto>,
    /// Telegram's accent color id, which colors the chat's badge when it has
    /// no photo.
    accent: i32,
    /// Who is typing (or recording, sending a photo, …) right now, in the
    /// order they started, with what they're doing, e.g. "typing".
    pub activity: Vec<(Sender, &'static str)>,
    /// The person or group the chat is with, for its [`Badge`].
    pub peer: Option<Peer>,
}

/// Whom a chat is with: a person, or a group or channel.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Peer {
    User(i64),
    Supergroup(i64),
}

/// What Telegram itself says about an account or group, shown after its
/// name, as Telegram's apps do. A private chat's name and photo are the other
/// person's choice, so without these an account named "Telegram" would look
/// like the one login codes come from.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Badge {
    Scam,
    Fake,
    /// Verified, or Telegram's own (support and service notifications).
    Official,
}

impl Badge {
    pub fn of(status: Option<&types::VerificationStatus>, official: bool) -> Option<Self> {
        match status {
            Some(s) if s.is_scam => Some(Badge::Scam),
            Some(s) if s.is_fake => Some(Badge::Fake),
            Some(s) if s.is_verified => Some(Badge::Official),
            _ => official.then_some(Badge::Official),
        }
    }

    /// The mark after the name, with its leading space.
    pub fn mark(self) -> &'static str {
        match self {
            Badge::Scam => " SCAM",
            Badge::Fake => " FAKE",
            Badge::Official => " ✓",
        }
    }
}

/// A chat's photo, for its avatar in the list.
#[derive(Clone)]
pub struct ChatPhoto {
    /// The small (160 px) version.
    pub file_id: i32,
    /// Where TDLib already has it on disk, if it does.
    pub path: Option<String>,
    /// The blurry version embedded in the chat, shown until the photo is ready.
    pub thumbnail: Option<Thumbnail>,
}

impl ChatPhoto {
    fn new(info: &ChatPhotoInfo) -> Self {
        let local = &info.small.local;
        Self {
            file_id: info.small.id,
            path: local.is_downloading_completed.then(|| local.path.clone()),
            thumbnail: decode_minithumbnail(info.minithumbnail.as_ref()),
        }
    }
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
    /// Accent color ids past the seven built-in ones, mapped to the built-in
    /// one they look like.
    accent_colors: HashMap<i32, i32>,
    /// What Telegram says about the people and groups chats are with.
    badges: HashMap<Peer, Badge>,
    /// The @username of people and groups that have one.
    usernames: HashMap<Peer, String>,
    /// Groups and channels you're not in: public ones opened with `s`.
    left: HashSet<i64>,
    /// When people were last on Telegram, by user id.
    presence: HashMap<i64, Presence>,
    /// User ids of bots, which have no last seen.
    bots: HashSet<i64>,
}

/// When someone was last on Telegram, as far as their privacy settings
/// tell. Times are unix timestamps.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Presence {
    /// Online, until this time unless Telegram says it again.
    Online(i32),
    Offline(i32),
    Recently,
    LastWeek,
    LastMonth,
    /// They hide it, or haven't been on in a long time.
    LongAgo,
}

impl Presence {
    pub fn of(status: &UserStatus) -> Self {
        match status {
            UserStatus::Online(s) => Presence::Online(s.expires),
            UserStatus::Offline(s) => Presence::Offline(s.was_online),
            UserStatus::Recently(_) => Presence::Recently,
            UserStatus::LastWeek(_) => Presence::LastWeek,
            UserStatus::LastMonth(_) => Presence::LastMonth,
            UserStatus::Empty => Presence::LongAgo,
        }
    }
}

/// What the title of a chat with one person says about them: online, when
/// they were last seen, or that it's a bot.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Seen {
    Bot,
    Person(Presence),
}

impl Chats {
    pub fn insert(&mut self, chat: types::Chat) {
        let is_channel = matches!(&chat.r#type, ChatType::Supergroup(s) if s.is_channel);
        let is_private = matches!(chat.r#type, ChatType::Private(_) | ChatType::Secret(_));
        let peer = match &chat.r#type {
            ChatType::Private(p) => Some(Peer::User(p.user_id)),
            ChatType::Secret(s) => Some(Peer::User(s.user_id)),
            ChatType::Supergroup(s) => Some(Peer::Supergroup(s.supergroup_id)),
            ChatType::BasicGroup(_) => None,
        };
        let entry = Chat {
            title: text::clean(&chat.title),
            is_channel,
            is_private,
            unread: chat.unread_count,
            read_outbox: chat.last_read_outbox_message_id,
            preview: chat.last_message.as_ref().map(preview).unwrap_or_default(),
            order: main_order(&chat.positions).unwrap_or(0),
            photo: chat.photo.as_ref().map(ChatPhoto::new),
            accent: chat.accent_color_id,
            activity: Vec::new(),
            peer,
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

    /// Someone started or stopped typing (or recording, …). TDLib sends the
    /// stop itself when the message arrives, or when the action isn't
    /// repeated within about 5 seconds.
    pub fn set_action(&mut self, chat_id: i64, sender: &MessageSender, action: &ChatAction) {
        let Some(chat) = self.by_id.get_mut(&chat_id) else {
            return;
        };
        let sender = Sender::from(sender);
        let at = chat.activity.iter().position(|(s, _)| *s == sender);
        match (at, activity(action)) {
            (Some(i), Some(doing)) => chat.activity[i].1 = doing,
            (Some(i), None) => {
                chat.activity.remove(i);
            }
            (None, Some(doing)) => chat.activity.push((sender, doing)),
            (None, None) => {}
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

    pub fn set_read_outbox(&mut self, chat_id: i64, message_id: i64) {
        if let Some(chat) = self.by_id.get_mut(&chat_id) {
            chat.read_outbox = message_id;
        }
    }

    pub fn set_photo(&mut self, chat_id: i64, photo: Option<&ChatPhotoInfo>) {
        if let Some(chat) = self.by_id.get_mut(&chat_id) {
            chat.photo = photo.map(ChatPhoto::new);
        }
    }

    pub fn set_accent(&mut self, chat_id: i64, accent_color_id: i32) {
        if let Some(chat) = self.by_id.get_mut(&chat_id) {
            chat.accent = accent_color_id;
        }
    }

    /// Takes TDLib's list of the accent colors past the built-in ones.
    pub fn set_accent_colors(&mut self, colors: &[AccentColor]) {
        self.accent_colors = colors
            .iter()
            .map(|c| (c.id, c.built_in_accent_color_id))
            .collect();
    }

    /// Which of Telegram's seven colors (red, orange, violet, green, cyan,
    /// blue, pink) the chat's badge has.
    pub fn accent(&self, chat_id: i64) -> usize {
        let id = self.by_id.get(&chat_id).map_or(0, |c| c.accent);
        let built_in = match id {
            0..7 => id,
            _ => self.accent_colors.get(&id).copied().unwrap_or(id),
        };
        built_in.rem_euclid(7) as usize
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
    /// Telegram's word on someone a chat is with, from `updateUser` or
    /// `updateSupergroup`.
    pub fn set_badge(&mut self, peer: Peer, badge: Option<Badge>) {
        match badge {
            Some(badge) => self.badges.insert(peer, badge),
            None => self.badges.remove(&peer),
        };
    }

    /// The badge after a chat's title, if Telegram gave one.
    pub fn badge(&self, chat_id: i64) -> Option<Badge> {
        let peer = self.by_id.get(&chat_id)?.peer?;
        self.badges.get(&peer).copied()
    }

    /// A person's or group's first active username, from `updateUser` or
    /// `updateSupergroup`.
    pub fn set_username(&mut self, peer: Peer, usernames: Option<&types::Usernames>) {
        let first = usernames.and_then(|u| u.active_usernames.first());
        match first {
            Some(name) => self.usernames.insert(peer, text::clean(name)),
            None => self.usernames.remove(&peer),
        };
    }

    /// The @username of a chat, without the @.
    pub fn username(&self, chat_id: i64) -> Option<&str> {
        let peer = self.by_id.get(&chat_id)?.peer?;
        self.usernames.get(&peer).map(String::as_str)
    }

    /// The @username of a person, without the @, whether or not you have a
    /// chat with them.
    pub fn user_username(&self, user_id: i64) -> Option<&str> {
        self.usernames.get(&Peer::User(user_id)).map(String::as_str)
    }

    /// When someone was last on Telegram, from `updateUser` and
    /// `updateUserStatus`.
    pub fn set_presence(&mut self, user_id: i64, presence: Presence) {
        self.presence.insert(user_id, presence);
    }

    pub fn set_bot(&mut self, user_id: i64, bot: bool) {
        if bot {
            self.bots.insert(user_id);
        } else {
            self.bots.remove(&user_id);
        }
    }

    /// What to say about the person a one-on-one chat is with; `None` for
    /// groups, channels and Saved Messages.
    pub fn seen(&self, chat_id: i64) -> Option<Seen> {
        if self.is_saved(chat_id) {
            return None;
        }
        let Some(Peer::User(user_id)) = self.by_id.get(&chat_id)?.peer else {
            return None;
        };
        if self.bots.contains(&user_id) {
            return Some(Seen::Bot);
        }
        self.presence.get(&user_id).copied().map(Seen::Person)
    }

    /// Whether you're in a group or channel, from `updateSupergroup`.
    pub fn set_member(&mut self, supergroup_id: i64, member: bool) {
        if member {
            self.left.remove(&supergroup_id);
        } else {
            self.left.insert(supergroup_id);
        }
    }

    /// You're in the chat: always for private chats and basic groups, which
    /// can't be read from outside.
    pub fn joined(&self, chat_id: i64) -> bool {
        match self.by_id.get(&chat_id).and_then(|c| c.peer) {
            Some(Peer::Supergroup(id)) => !self.left.contains(&id),
            _ => true,
        }
    }

    /// The chat is in the main list, not e.g. a public channel found with `s`.
    pub fn listed(&self, chat_id: i64) -> bool {
        self.by_id.get(&chat_id).is_some_and(|c| c.order != 0)
    }

    /// Chats in the main list whose name or username contains `query`, in
    /// Telegram's order (pinned, then by the last message), for the chat
    /// picker. Unread chats don't go first, unlike in the list. An empty
    /// query matches them all.
    pub fn matching(&self, query: &str) -> Vec<i64> {
        let query = query.trim();
        let username = query.strip_prefix('@').unwrap_or(query);
        let mut ids: Vec<i64> = self
            .by_id
            .iter()
            .filter(|(_, chat)| chat.order != 0)
            .map(|(&id, _)| id)
            .filter(|&id| {
                username.is_empty()
                    || !search::find(self.title(id).unwrap_or_default(), query).is_empty()
                    || self
                        .username(id)
                        .is_some_and(|name| !search::find(name, username).is_empty())
            })
            .collect();
        ids.sort_unstable_by_key(|&id| std::cmp::Reverse((self.by_id[&id].order, id)));
        ids
    }

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

impl Chats {
    /// Adds a chat that didn't come from TDLib, below the others, for
    /// `--demo` and tests: TDLib's `Chat` is too big to build by hand.
    pub fn add_local(&mut self, id: i64, title: &str, photo: Option<ChatPhoto>) -> &mut Chat {
        let order = 1000 - self.by_id.len() as i64;
        self.dirty = true;
        self.by_id
            .entry(id)
            .insert_entry(Chat {
                title: title.into(),
                is_channel: false,
                is_private: false,
                unread: 0,
                read_outbox: 0,
                preview: String::new(),
                order,
                photo,
                accent: 0,
                activity: Vec::new(),
                peer: None,
            })
            .into_mut()
    }
}

/// What a chat action shows as, in Telegram's words: "Alice is
/// *typing*". `None` to stop showing one. Watching an emoji's animation shows
/// nothing, as in Telegram, which plays it instead.
fn activity(action: &ChatAction) -> Option<&'static str> {
    Some(match action {
        ChatAction::Typing => "typing",
        ChatAction::RecordingVideo => "recording a video",
        ChatAction::UploadingVideo(_) => "sending a video",
        ChatAction::RecordingVoiceNote => "recording a voice message",
        ChatAction::UploadingVoiceNote(_) => "sending a voice message",
        ChatAction::UploadingPhoto(_) => "sending a photo",
        ChatAction::UploadingDocument(_) => "sending a file",
        ChatAction::ChoosingSticker => "choosing a sticker",
        ChatAction::ChoosingLocation => "choosing a location",
        ChatAction::ChoosingContact => "choosing a contact",
        ChatAction::StartPlayingGame => "playing a game",
        ChatAction::RecordingVideoNote => "recording a video message",
        ChatAction::UploadingVideoNote(_) => "sending a video message",
        ChatAction::WatchingAnimations(_) | ChatAction::Cancel => return None,
    })
}

fn main_order(positions: &[ChatPosition]) -> Option<i64> {
    positions
        .iter()
        .find(|p| matches!(p.list, ChatList::Main))
        .map(|p| p.order)
}

/// How much of the last message a chat list row keeps.
const PREVIEW_CHARS: usize = 300;

fn preview(message: &Message) -> String {
    let text = text::clean(&content_text(&message.content));
    // Only the start fits in a chat list row, and it's drawn every frame.
    let text = text::first_chars(&text, PREVIEW_CHARS).replace(['\n', '\t'], " ");
    if message.is_outgoing {
        format!("You: {text}")
    } else {
        text
    }
}

/// Plain-text rendering of a message body; media becomes a `[Label]`.
/// Spoilers are blotted out, as in Telegram's chat list.
pub fn content_text(content: &MessageContent) -> String {
    labeled_text(content, without_spoilers)
}

/// [`content_text`] with spoilers as they were sent, for the message's own
/// bubble, which hides them itself.
pub fn content_text_as_sent(content: &MessageContent) -> String {
    labeled_text(content, |text| text.text.clone())
}

fn labeled_text(content: &MessageContent, text: impl Fn(&FormattedText) -> String) -> String {
    let labeled = |label: &str, caption: &FormattedText| {
        let caption = text(caption);
        if caption.is_empty() {
            format!("[{label}]")
        } else {
            format!("[{label}] {caption}")
        }
    };
    match content {
        MessageContent::MessageText(m) => text(&m.text),
        MessageContent::MessagePhoto(m) => labeled("Photo", &m.caption),
        MessageContent::MessageVideo(m) => labeled("Video", &m.caption),
        MessageContent::MessageAnimation(m) => labeled("GIF", &m.caption),
        MessageContent::MessageDocument(m) => {
            labeled(&format!("File: {}", m.document.file_name), &m.caption)
        }
        MessageContent::MessageAudio(m) => labeled("Audio", &m.caption),
        MessageContent::MessageVoiceNote(m) => labeled("Voice message", &m.caption),
        MessageContent::MessageVideoNote(_) => "[Video message]".into(),
        MessageContent::MessageSticker(m) => format!("[Sticker {}]", m.sticker.emoji),
        MessageContent::MessagePoll(m) => labeled("Poll", &m.poll.question),
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
                    is_private: false,
                    unread,
                    read_outbox: 0,
                    preview: String::new(),
                    order,
                    photo: None,
                    accent: 0,
                    activity: Vec::new(),
                    peer: None,
                },
            );
        }
        chats.dirty = true;
        chats.refresh();
        chats
    }

    fn user(user_id: i64) -> MessageSender {
        MessageSender::User(types::MessageSenderUser { user_id })
    }

    #[test]
    fn activity_lasts_until_tdlib_says_it_stopped() {
        let mut list = chats(&[(1, 50, 0)]);
        let activity = |list: &Chats| list.get(1).unwrap().activity.clone();
        list.set_action(1, &user(7), &ChatAction::Typing);
        list.set_action(1, &user(8), &ChatAction::ChoosingSticker);
        assert_eq!(
            activity(&list),
            [
                (Sender::User(7), "typing"),
                (Sender::User(8), "choosing a sticker")
            ]
        );

        // Someone doing something else keeps their place.
        let photo = types::ChatActionUploadingPhoto { progress: 10 };
        list.set_action(1, &user(7), &ChatAction::UploadingPhoto(photo));
        assert_eq!(activity(&list)[0], (Sender::User(7), "sending a photo"));

        list.set_action(1, &user(7), &ChatAction::Cancel);
        assert_eq!(activity(&list), [(Sender::User(8), "choosing a sticker")]);
        // An emoji animation being watched shows nothing.
        let watching = types::ChatActionWatchingAnimations {
            emoji: "🎉".into()
        };
        list.set_action(1, &user(9), &ChatAction::WatchingAnimations(watching));
        assert_eq!(activity(&list).len(), 1);
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
                is_private: true,
                unread: 0,
                read_outbox: 0,
                preview: String::new(),
                order: 10,
                photo: None,
                accent: 0,
                activity: Vec::new(),
                peer: None,
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
    fn accent_colors_past_the_built_in_seven_map_to_one_of_them() {
        let mut list = chats(&[(1, 50, 0), (2, 40, 0), (3, 30, 0)]);
        list.set_accent(1, 3);
        list.set_accent(2, 9);
        list.set_accent(3, 12);
        list.set_accent_colors(&[AccentColor {
            id: 9,
            built_in_accent_color_id: 5,
            light_theme_colors: Vec::new(),
            dark_theme_colors: Vec::new(),
            min_channel_chat_boost_level: 0,
        }]);
        assert_eq!(list.accent(1), 3);
        assert_eq!(list.accent(2), 5);
        assert_eq!(list.accent(3), 5, "an unknown id still gets a color");
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

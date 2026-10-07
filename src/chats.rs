//! The chat list, rebuilt from TDLib updates.
//!
//! TDLib gives every chat an `order` per chat list: the main one, the archive
//! and each of your folders. The list shown is the chats with a non-zero
//! order in it, sorted by (order, chat id) descending. On top of that, chats
//! with unread messages come first. A `/` search narrows the list to chats
//! whose title matches.

use std::collections::{HashMap, HashSet};

use tdlib_rs::enums::{
    ChatAction, ChatList, ChatType, MessageContent, MessageSender, NotificationSettingsScope,
    UserStatus,
};
use tdlib_rs::types::{
    self, AccentColor, ChatFolderInfo, ChatNotificationSettings, ChatPhotoInfo, ChatPosition,
    FormattedText, Message,
};

use crate::images::Thumbnail;
use crate::messages::{Sender, decode_minithumbnail, without_spoilers};
use crate::search;
use crate::secret::Secret;
use crate::service::Service;
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
    /// You read the messages up to this id.
    read_inbox: i64,
    /// One-line summary of the last message, e.g. "You: see you at 5".
    pub preview: String,
    /// Where the chat is in each list it's in: the main list or the archive,
    /// and any of your folders.
    positions: HashMap<List, Position>,
    pub photo: Option<ChatPhoto>,
    /// Telegram's accent color id, which colors the chat's badge when it has
    /// no photo.
    accent: i32,
    /// Who is typing (or recording, sending a photo, …) right now, in the
    /// order they started, with what they're doing, e.g. "typing", and in a
    /// forum the topic they're doing it in.
    pub activity: Vec<(Sender, &'static str, Option<i32>)>,
    /// The person or group the chat is with, for its [`Badge`].
    pub peer: Option<Peer>,
    /// How the chat notifies, which `m` changes the mute of.
    notifications: ChatNotificationSettings,
    /// TDLib's id for the secret chat this is; see [`Chats::secret`].
    pub secret_id: Option<i32>,
    /// Seconds messages last: once seen in a secret chat, once sent in
    /// others; 0 for as long as anyone keeps them.
    auto_delete: i32,
}

/// A list of chats: the main one, the archive, or one of your folders.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum List {
    #[default]
    Main,
    Archive,
    Folder(i32),
}

impl List {
    pub fn of(list: &ChatList) -> Self {
        match list {
            ChatList::Main => List::Main,
            ChatList::Archive => List::Archive,
            ChatList::Folder(f) => List::Folder(f.chat_folder_id),
        }
    }

    /// The list as TDLib takes it.
    pub fn tdlib(self) -> ChatList {
        match self {
            List::Main => ChatList::Main,
            List::Archive => ChatList::Archive,
            List::Folder(id) => ChatList::Folder(types::ChatListFolder { chat_folder_id: id }),
        }
    }
}

/// Where a chat is in a list.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Position {
    order: i64,
    /// Pinned to the top of the list.
    pinned: bool,
}

/// A tab over the chat list: a folder, all chats, or the archive.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Tab {
    pub list: List,
    pub name: String,
    /// Unread chats in it that aren't muted, as Telegram counts them.
    pub unread: i32,
}

/// What the tab of the main list is called, as in Telegram.
const ALL_CHATS: &str = "All";
/// And the archive's.
const ARCHIVE: &str = "Archive";

impl Chat {
    /// TDLib's order of the chat in a list; 0 if it isn't in it.
    fn order(&self, list: List) -> i64 {
        self.positions.get(&list).map_or(0, |p| p.order)
    }

    /// Pinned to the top of a list.
    pub fn pinned(&self, list: List) -> bool {
        self.positions.get(&list).is_some_and(|p| p.pinned)
    }

    fn set_position(&mut self, position: &ChatPosition) {
        let list = List::of(&position.list);
        if position.order == 0 {
            self.positions.remove(&list);
        } else {
            let place = Position {
                order: position.order,
                pinned: position.is_pinned,
            };
            self.positions.insert(list, place);
        }
    }

    /// Pins the chat in a list it's in, or unpins it, for tests.
    #[cfg(test)]
    pub fn set_pinned(&mut self, list: List, pinned: bool) {
        if let Some(place) = self.positions.get_mut(&list) {
            place.pinned = pinned;
        }
    }
}

/// Whom a chat is with: a person, or a group or channel.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Peer {
    User(i64),
    BasicGroup(i64),
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
    /// The list shown: the main one, the archive or a folder.
    shown: List,
    /// Your folders, by id and name, in your order.
    folders: Vec<(i32, String)>,
    /// Where the main list's tab goes among the folders.
    main_at: usize,
    /// Unread unmuted chats in each list, from `updateUnreadChatCount`.
    unread_in: HashMap<List, i32>,
    /// What Telegram says about the people and groups chats are with.
    badges: HashMap<Peer, Badge>,
    /// The @username of people and groups that have one.
    usernames: HashMap<Peer, String>,
    /// Groups and channels you're not in: public ones opened with `s`.
    left: HashSet<i64>,
    /// Groups split into topics, by supergroup id.
    forums: HashSet<i64>,
    /// How long chats of each kind are muted for unless they say
    /// otherwise: private chats, groups, channels.
    default_mute: [i32; 3],
    /// When people were last on Telegram, by user id.
    presence: HashMap<i64, Presence>,
    /// User ids of bots, which have no last seen.
    bots: HashSet<i64>,
    /// Secret chats, by TDLib's secret chat id.
    secrets: HashMap<i32, Secret>,
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
            ChatType::BasicGroup(b) => Some(Peer::BasicGroup(b.basic_group_id)),
        };
        let secret_id = match &chat.r#type {
            ChatType::Secret(s) => Some(s.secret_chat_id),
            _ => None,
        };
        let entry = Chat {
            title: text::clean(&chat.title),
            is_channel,
            is_private,
            unread: chat.unread_count,
            read_outbox: chat.last_read_outbox_message_id,
            read_inbox: chat.last_read_inbox_message_id,
            preview: chat.last_message.as_ref().map(preview).unwrap_or_default(),
            positions: HashMap::new(),
            photo: chat.photo.as_ref().map(ChatPhoto::new),
            accent: chat.accent_color_id,
            activity: Vec::new(),
            peer,
            notifications: chat.notification_settings,
            secret_id,
            auto_delete: chat.message_auto_delete_time,
        };
        let entry = self.by_id.entry(chat.id).insert_entry(entry).into_mut();
        for position in &chat.positions {
            entry.set_position(position);
        }
        self.dirty = true;
    }

    pub fn set_position(&mut self, chat_id: i64, position: &ChatPosition) {
        if let Some(chat) = self.by_id.get_mut(&chat_id) {
            chat.set_position(position);
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
            for position in positions {
                chat.set_position(position);
                self.dirty = true;
            }
        }
    }

    /// Someone started or stopped typing (or recording, …), in a forum's
    /// `topic` or the whole chat. TDLib sends the stop itself when the
    /// message arrives, or when the action isn't repeated within about 5
    /// seconds.
    pub fn set_action(
        &mut self,
        chat_id: i64,
        topic: Option<i32>,
        sender: &MessageSender,
        action: &ChatAction,
    ) {
        let Some(chat) = self.by_id.get_mut(&chat_id) else {
            return;
        };
        let sender = Sender::from(sender);
        let at = chat.activity.iter().position(|(s, _, _)| *s == sender);
        match (at, activity(action)) {
            (Some(i), Some(doing)) => chat.activity[i] = (sender, doing, topic),
            (Some(i), None) => {
                chat.activity.remove(i);
            }
            (None, Some(doing)) => chat.activity.push((sender, doing, topic)),
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

    pub fn set_read_inbox(&mut self, chat_id: i64, message_id: i64) {
        if let Some(chat) = self.by_id.get_mut(&chat_id) {
            chat.read_inbox = message_id;
        }
    }

    /// The last message you read in a chat.
    pub fn read_inbox(&self, chat_id: i64) -> i64 {
        self.by_id.get(&chat_id).map_or(0, |c| c.read_inbox)
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
        let list = self.shown;
        self.sorted = self
            .by_id
            .iter()
            .filter(|(_, chat)| chat.order(list) != 0)
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
        // Pinned chats stay on top, as you put them; unread ones come next.
        let (by_id, held) = (&self.by_id, self.held);
        self.sorted.sort_unstable_by_key(|&id| {
            let chat = &by_id[&id];
            let unread = chat.unread > 0 || held == Some(id);
            std::cmp::Reverse((chat.pinned(list), unread, chat.order(list), id))
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

    /// How many chats are in the list shown, filtered out or not.
    pub fn total(&self) -> usize {
        self.total
    }

    /// The list shown: the main one, the archive or a folder.
    pub fn shown(&self) -> List {
        self.shown
    }

    /// Shows another list. The `/` filter stays.
    pub fn show(&mut self, list: List) {
        if self.shown != list {
            self.shown = list;
            self.dirty = true;
        }
    }

    /// The chat is pinned to the top of the list shown.
    pub fn pinned(&self, chat_id: i64) -> bool {
        self.by_id
            .get(&chat_id)
            .is_some_and(|c| c.pinned(self.shown))
    }

    /// Your folders, from `updateChatFolders`, and where the main list goes
    /// among them. A folder that's gone can't stay shown.
    pub fn set_folders(&mut self, folders: &[ChatFolderInfo], main_at: i32) {
        self.folders = folders
            .iter()
            .map(|f| {
                let name = text::clean(&f.name.text.text);
                (f.id, name.split_whitespace().collect::<Vec<_>>().join(" "))
            })
            .collect();
        self.main_at = usize::try_from(main_at).unwrap_or(0);
        if !self.tabs().iter().any(|t| t.list == self.shown) {
            self.show(List::Main);
        }
    }

    /// Unread unmuted chats in a list, for its tab.
    pub fn set_unread_in(&mut self, list: List, count: i32) {
        self.unread_in.insert(list, count);
    }

    /// The tabs over the list, in Telegram's order: your folders with all
    /// chats among them where you put it, then the archive once it has
    /// chats. With neither folders nor an archive there are none.
    pub fn tabs(&self) -> Vec<Tab> {
        let tab = |list, name: &str| Tab {
            list,
            name: name.to_string(),
            unread: self.unread_in.get(&list).copied().unwrap_or(0),
        };
        let mut tabs: Vec<Tab> = self
            .folders
            .iter()
            .map(|(id, name)| tab(List::Folder(*id), name))
            .collect();
        tabs.insert(self.main_at.min(tabs.len()), tab(List::Main, ALL_CHATS));
        let archived = self
            .by_id
            .values()
            .any(|c| c.positions.contains_key(&List::Archive));
        if archived || self.shown == List::Archive {
            tabs.push(tab(List::Archive, ARCHIVE));
        }
        if tabs.len() == 1 { Vec::new() } else { tabs }
    }

    /// The list `step` tabs away from the one shown, round the end.
    pub fn next_list(&self, step: isize) -> List {
        let tabs = self.tabs();
        let Some(at) = tabs.iter().position(|t| t.list == self.shown) else {
            return List::Main;
        };
        let at = (at as isize + step).rem_euclid(tabs.len() as isize);
        tabs[at as usize].list
    }

    /// The chat is in a list.
    pub fn in_list(&self, chat_id: i64, list: List) -> bool {
        self.by_id.get(&chat_id).is_some_and(|c| c.order(list) != 0)
    }

    pub fn set_my_id(&mut self, id: i64) {
        self.my_id = Some(id);
    }

    /// Your own user id, once TDLib said.
    pub fn my_id(&self) -> Option<i64> {
        self.my_id
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

    /// The person with this @username (without the @), ignoring case, if
    /// TDLib told about them. `None` if two people had it (one gave it up,
    /// and TDLib hasn't said so yet), so TDLib is asked who has it now.
    pub fn user_by_username(&self, username: &str) -> Option<i64> {
        let mut found = self.usernames.iter().filter_map(|(peer, name)| match peer {
            Peer::User(id) if name.eq_ignore_ascii_case(username) => Some(*id),
            _ => None,
        });
        let first = found.next()?;
        found.next().is_none().then_some(first)
    }

    /// The @username of a person, without the @, whether or not you have a
    /// chat with them.
    pub fn user_username(&self, user_id: i64) -> Option<&str> {
        self.usernames.get(&Peer::User(user_id)).map(String::as_str)
    }

    pub fn set_notifications(&mut self, chat_id: i64, settings: ChatNotificationSettings) {
        if let Some(chat) = self.by_id.get_mut(&chat_id) {
            chat.notifications = settings;
        }
    }

    /// How long chats of a kind are muted for by default, from
    /// `updateScopeNotificationSettings`.
    pub fn set_default_mute(&mut self, scope: &NotificationSettingsScope, mute_for: i32) {
        self.default_mute[scope_index(scope)] = mute_for;
    }

    /// The chat's notifications are off, by its own setting or the default
    /// for its kind.
    pub fn muted(&self, chat_id: i64) -> bool {
        let Some(chat) = self.by_id.get(&chat_id) else {
            return false;
        };
        let mute_for = if chat.notifications.use_default_mute_for {
            let scope = match () {
                _ if chat.is_private => NotificationSettingsScope::PrivateChats,
                _ if chat.is_channel => NotificationSettingsScope::ChannelChats,
                _ => NotificationSettingsScope::GroupChats,
            };
            self.default_mute[scope_index(&scope)]
        } else {
            chat.notifications.mute_for
        };
        mute_for > 0
    }

    /// The chat's notification settings with it muted for good, or not at
    /// all, everything else as it was.
    pub fn with_mute(&self, chat_id: i64, mute: bool) -> Option<ChatNotificationSettings> {
        let chat = self.by_id.get(&chat_id)?;
        Some(ChatNotificationSettings {
            use_default_mute_for: false,
            // TDLib takes anything past a year as forever.
            mute_for: if mute { i32::MAX } else { 0 },
            ..chat.notifications.clone()
        })
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

    pub fn is_bot(&self, user_id: i64) -> bool {
        self.bots.contains(&user_id)
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

    /// Whether a group is split into topics, from `updateSupergroup`.
    pub fn set_forum(&mut self, supergroup_id: i64, forum: bool) {
        if forum {
            self.forums.insert(supergroup_id);
        } else {
            self.forums.remove(&supergroup_id);
        }
    }

    /// The chat is a forum: a group whose messages are in topics, which
    /// open one at a time.
    pub fn is_forum(&self, chat_id: i64) -> bool {
        match self.by_id.get(&chat_id).and_then(|c| c.peer) {
            Some(Peer::Supergroup(id)) => self.forums.contains(&id),
            _ => false,
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

    /// A secret chat's state, from `updateSecretChat`, which TDLib sends
    /// before the chat itself.
    pub fn set_secret(&mut self, secret_id: i32, secret: Secret) {
        self.secrets.insert(secret_id, secret);
    }

    /// The secret chat this is, once TDLib said how it is.
    pub fn secret(&self, chat_id: i64) -> Option<&Secret> {
        let id = self.by_id.get(&chat_id)?.secret_id?;
        self.secrets.get(&id)
    }

    /// Any secret chats are known, which logging out loses.
    pub fn has_secret_chats(&self) -> bool {
        !self.secrets.is_empty()
    }

    /// The chat is a secret chat, kept on this computer only.
    pub fn is_secret(&self, chat_id: i64) -> bool {
        self.by_id
            .get(&chat_id)
            .is_some_and(|c| c.secret_id.is_some())
    }

    /// How long messages last, from `updateChatMessageAutoDeleteTime`.
    pub fn set_auto_delete(&mut self, chat_id: i64, seconds: i32) {
        if let Some(chat) = self.by_id.get_mut(&chat_id) {
            chat.auto_delete = seconds;
        }
    }

    /// Seconds new messages in the chat last; 0 if they stay.
    pub fn auto_delete(&self, chat_id: i64) -> i32 {
        self.by_id.get(&chat_id).map_or(0, |c| c.auto_delete)
    }

    /// The person a chat with one person is with, secret or not: not
    /// yourself, in Saved Messages.
    pub fn person(&self, chat_id: i64) -> Option<i64> {
        match self.by_id.get(&chat_id)?.peer? {
            Peer::User(id) if !self.is_saved(chat_id) => Some(id),
            _ => None,
        }
    }

    /// The chat is in the main list, not e.g. a public channel found with `s`.
    pub fn listed(&self, chat_id: i64) -> bool {
        self.in_list(chat_id, List::Main)
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
            .filter(|(_, chat)| chat.order(List::Main) != 0)
            .map(|(&id, _)| id)
            .filter(|&id| {
                username.is_empty()
                    || !search::find(self.title(id).unwrap_or_default(), query).is_empty()
                    || self
                        .username(id)
                        .is_some_and(|name| !search::find(name, username).is_empty())
            })
            .collect();
        ids.sort_unstable_by_key(|&id| std::cmp::Reverse((self.by_id[&id].order(List::Main), id)));
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
        let mut chat = Chat::local(title, &[(List::Main, order)]);
        chat.photo = photo;
        self.by_id.entry(id).insert_entry(chat).into_mut()
    }

    /// Puts a chat added with [`Chats::add_local`] in another list too,
    /// where it keeps its place.
    pub fn add_local_to(&mut self, list: List, chat_id: i64) {
        if let Some(chat) = self.by_id.get_mut(&chat_id) {
            let order = chat.order(List::Main);
            chat.positions.insert(
                list,
                Position {
                    order,
                    pinned: false,
                },
            );
            self.dirty = true;
        }
    }
}

impl Chat {
    /// A chat with nothing in it yet, in these lists at these orders.
    fn local(title: &str, lists: &[(List, i64)]) -> Self {
        let positions = lists
            .iter()
            .map(|&(list, order)| {
                let place = Position {
                    order,
                    pinned: false,
                };
                (list, place)
            })
            .collect();
        Chat {
            title: title.into(),
            is_channel: false,
            is_private: false,
            unread: 0,
            read_outbox: 0,
            read_inbox: 0,
            preview: String::new(),
            positions,
            photo: None,
            accent: 0,
            activity: Vec::new(),
            peer: None,
            notifications: ChatNotificationSettings::default(),
            secret_id: None,
            auto_delete: 0,
        }
    }
}

fn scope_index(scope: &NotificationSettingsScope) -> usize {
    match scope {
        NotificationSettingsScope::PrivateChats => 0,
        NotificationSettingsScope::GroupChats => 1,
        NotificationSettingsScope::ChannelChats => 2,
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

/// How much of the last message a chat list row keeps.
const PREVIEW_CHARS: usize = 300;

/// One line of a message for the chat list: "You: see you at 5".
fn preview(message: &Message) -> String {
    let text = snippet(message);
    if message.is_outgoing {
        format!("You: {text}")
    } else {
        text
    }
}

/// One line of a message for a list, without who sent it.
pub fn snippet(message: &Message) -> String {
    let text = match Service::of(&message.content) {
        Some(service) => format!("[{}]", service.label(Sender::from(&message.sender_id))),
        None => text::clean(&content_text(&message.content)),
    };
    // Only the start fits in a list row, and it's drawn every frame.
    text::first_chars(&text, PREVIEW_CHARS).replace(['\n', '\t'], " ")
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
        MessageContent::MessageExpiredPhoto => "[Photo expired]".into(),
        MessageContent::MessageExpiredVideo => "[Video expired]".into(),
        MessageContent::MessageExpiredVideoNote => "[Video message expired]".into(),
        MessageContent::MessageExpiredVoiceNote => "[Voice message expired]".into(),
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
            let lists: &[(List, i64)] = if order == 0 {
                &[]
            } else {
                &[(List::Main, order)]
            };
            let mut chat = Chat::local(&format!("chat {id}"), lists);
            chat.unread = unread;
            chats.by_id.insert(id, chat);
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
        list.set_action(1, None, &user(7), &ChatAction::Typing);
        list.set_action(1, None, &user(8), &ChatAction::ChoosingSticker);
        assert_eq!(
            activity(&list),
            [
                (Sender::User(7), "typing", None),
                (Sender::User(8), "choosing a sticker", None)
            ]
        );

        // Someone doing something else keeps their place.
        let photo = types::ChatActionUploadingPhoto { progress: 10 };
        list.set_action(1, None, &user(7), &ChatAction::UploadingPhoto(photo));
        assert_eq!(
            activity(&list)[0],
            (Sender::User(7), "sending a photo", None)
        );

        list.set_action(1, None, &user(7), &ChatAction::Cancel);
        assert_eq!(
            activity(&list),
            [(Sender::User(8), "choosing a sticker", None)]
        );
        // An emoji animation being watched shows nothing.
        let watching = types::ChatActionWatchingAnimations {
            emoji: "🎉".into()
        };
        list.set_action(1, None, &user(9), &ChatAction::WatchingAnimations(watching));
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
    fn pinned_chats_stay_on_top_then_unread_ones() {
        let mut list = chats(&[(1, 50, 0), (2, 40, 3), (3, 30, 0)]);
        list.by_id.get_mut(&3).unwrap().set_pinned(List::Main, true);
        list.dirty = true;
        list.refresh();
        assert_eq!(list.ids(), [3, 2, 1]);
    }

    #[test]
    fn a_chat_is_muted_by_its_own_setting_or_its_kinds_default() {
        let mut list = chats(&[(1, 50, 0), (2, 40, 0)]);
        let own = |mute_for| ChatNotificationSettings {
            mute_for,
            ..ChatNotificationSettings::default()
        };
        let default = || ChatNotificationSettings {
            use_default_mute_for: true,
            ..ChatNotificationSettings::default()
        };
        list.set_notifications(1, own(3600));
        list.set_notifications(2, default());
        assert!(list.muted(1));
        assert!(!list.muted(2));
        list.set_default_mute(&NotificationSettingsScope::GroupChats, i32::MAX);
        assert!(list.muted(2), "groups are muted by default now");

        let unmute = list.with_mute(1, false).unwrap();
        assert!(!unmute.use_default_mute_for && unmute.mute_for == 0);
        assert_eq!(list.with_mute(2, true).unwrap().mute_for, i32::MAX);
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
        let mut me = Chat::local("Eric", &[(List::Main, 10)]);
        me.is_private = true;
        list.by_id.insert(4, me);

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

    fn folder(id: i32, name: &str) -> ChatFolderInfo {
        ChatFolderInfo {
            id,
            name: types::ChatFolderName {
                text: FormattedText {
                    text: name.into(),
                    entities: Vec::new(),
                },
                animate_custom_emoji: false,
            },
            ..ChatFolderInfo::default()
        }
    }

    fn position(list: List, order: i64, pinned: bool) -> ChatPosition {
        ChatPosition {
            list: list.tdlib(),
            order,
            is_pinned: pinned,
            source: None,
        }
    }

    #[test]
    fn a_folder_lists_its_own_chats_pinned_then_unread_first() {
        let mut list = chats(&[(1, 50, 0), (2, 40, 3), (3, 30, 0), (4, 20, 1)]);
        let work = List::Folder(7);
        list.set_position(1, &position(work, 10, false));
        list.set_position(3, &position(work, 30, true));
        list.set_position(4, &position(work, 20, false));
        list.show(work);
        list.refresh();
        assert_eq!(list.ids(), [3, 4, 1], "its own order and pins");
        assert_eq!(list.total(), 3);
        assert!(list.pinned(3) && !list.pinned(1));

        // Taken out of the folder, as TDLib says with an order of 0.
        list.set_position(4, &position(work, 0, false));
        list.refresh();
        assert_eq!(list.ids(), [3, 1]);

        list.show(List::Main);
        list.refresh();
        assert_eq!(list.ids(), [2, 4, 1, 3], "the main list as it was");
        assert!(!list.pinned(3), "pinned only in the folder");
    }

    #[test]
    fn tabs_put_all_chats_where_telegram_says_and_the_archive_last() {
        let mut list = chats(&[(1, 50, 0), (2, 40, 0)]);
        assert!(list.tabs().is_empty(), "no folders, no archive: no tabs");

        list.set_folders(&[folder(7, "Work"), folder(8, "Fam\u{202e}ily  ")], 1);
        list.set_unread_in(List::Folder(8), 2);
        let tabs = list.tabs();
        let names: Vec<&str> = tabs.iter().map(|t| t.name.as_str()).collect();
        assert_eq!(names, ["Work", "All", "Family"], "names are cleaned");
        assert_eq!(tabs[2].unread, 2);

        list.set_position(2, &position(List::Archive, 5, false));
        let tabs = list.tabs();
        assert_eq!(tabs.last().unwrap().list, List::Archive);

        assert_eq!(list.next_list(1), List::Folder(8));
        assert_eq!(list.next_list(-1), List::Folder(7));
        list.show(List::Archive);
        assert_eq!(list.next_list(1), List::Folder(7), "round the end");

        // A folder deleted while shown leaves the main list shown.
        list.show(List::Folder(8));
        list.set_folders(&[folder(7, "Work")], 0);
        assert_eq!(list.shown(), List::Main);
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

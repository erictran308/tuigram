//! Forum topics. A forum is a group whose messages are split into topics,
//! each a conversation of its own. Opening one shows its topics in a pane
//! between the chat list and the messages, and Enter on a topic opens its
//! messages there.
//!
//! `getForumTopics` gives the topics a page at a time, pinned ones first,
//! then the most recently active. Only some changes come as updates
//! (`updateForumTopicInfo` for the name and icon, `updateForumTopic` for
//! pins and reading), so a new message moves its topic up here, and an
//! unread count only TDLib can work out is asked for again
//! (`getForumTopic`).

use std::collections::HashSet;

use tdlib_rs::enums::MessageTopic;
use tdlib_rs::types::{self, Message};

use crate::chats;
use crate::draft::Draft;
use crate::messages::{Sender, one_line};

/// TDLib's id for the General topic, which every forum has. Messages sent
/// before the group had topics are in it.
pub const GENERAL: i32 = 1;

/// Topics asked for at once, TDLib's most.
pub const PAGE: i32 = 100;

/// More topics are asked for once the cursor is this close to the last one
/// loaded.
const LOAD_AHEAD: usize = 20;

/// Telegram's blue, for an icon that has no color.
const DEFAULT_COLOR: u32 = 0x6F_B9_F0;

pub struct Topic {
    pub id: i32,
    pub name: String,
    pub general: bool,
    /// Only admins and whoever started it can write in it.
    pub closed: bool,
    /// The icon's color, as 0xRRGGBB.
    pub color: u32,
    pub pinned: bool,
    /// TDLib's order: topics are listed by it, highest first.
    order: i64,
    pub unread: i32,
    /// Unread messages that mention you or answer yours.
    pub mentions: i32,
    /// Who sent the newest message, unless it was you, and one line of it.
    pub from: Option<Sender>,
    /// You sent the newest message.
    pub yours: bool,
    pub preview: String,
    /// What was left written in it.
    pub draft: Option<Draft>,
    /// The newest message that was sent (not one still on its way), to
    /// tell when reading reached it.
    newest: i64,
    read_inbox: i64,
}

impl Topic {
    pub fn of(topic: &types::ForumTopic) -> Self {
        let mut new = Self {
            id: topic.info.forum_topic_id,
            name: String::new(),
            general: false,
            closed: false,
            color: DEFAULT_COLOR,
            pinned: topic.is_pinned,
            order: topic.order,
            unread: topic.unread_count,
            mentions: topic.unread_mention_count,
            from: None,
            yours: false,
            preview: String::new(),
            draft: Draft::of(topic.draft_message.as_ref(), None),
            newest: 0,
            read_inbox: topic.last_read_inbox_message_id,
        };
        new.set_info(&topic.info);
        if let Some(message) = &topic.last_message {
            new.set_last(&Arrived::of(message, new.id));
        }
        new
    }

    /// The id of its newest message that was sent; 0 if not known.
    pub fn newest(&self) -> i64 {
        self.newest
    }

    /// The last message read in it, before the unread ones.
    pub fn read_inbox(&self) -> i64 {
        self.read_inbox
    }

    /// A topic made up for the demo and tests.
    pub fn local(id: i32, name: &str, color: u32, unread: i32, preview: &str) -> Self {
        Self {
            id,
            name: name.into(),
            general: id == GENERAL,
            closed: false,
            color,
            pinned: false,
            order: 0,
            unread,
            mentions: 0,
            from: None,
            yours: false,
            preview: preview.into(),
            draft: None,
            newest: 0,
            read_inbox: 0,
        }
    }

    /// Which of the theme's seven name colors (red, orange, violet, green,
    /// cyan, blue, pink) the icon is drawn in: the one nearest the six
    /// Telegram picks topic colors from.
    pub fn accent(&self) -> usize {
        match self.color {
            0xFB_6F_5F => 0,
            0xFF_D6_7E => 1,
            0xCB_86_DB => 2,
            0x8E_EE_98 => 3,
            0xFF_93_B2 => 6,
            _ => 5,
        }
    }

    fn set_info(&mut self, info: &types::ForumTopicInfo) {
        self.name = one_line(&info.name);
        self.general = info.is_general;
        self.closed = info.is_closed;
        if info.icon.color != 0 {
            self.color = info.icon.color as u32 & 0xFF_FF_FF;
        }
    }

    fn set_last(&mut self, message: &Arrived) {
        self.from = (!message.outgoing).then_some(message.sender);
        self.yours = message.outgoing;
        self.preview = message.preview.clone();
        if message.sent {
            self.newest = self.newest.max(message.id);
        }
    }
}

/// What a topic's row needs of a message: the newest in it.
pub struct Arrived {
    pub id: i64,
    pub topic: i32,
    pub outgoing: bool,
    /// Sent, not still on its way, whose id is for now a stand-in.
    pub sent: bool,
    pub sender: Sender,
    /// One line of it, without who sent it.
    pub preview: String,
}

impl Arrived {
    fn of(message: &Message, topic: i32) -> Self {
        Self {
            id: message.id,
            topic,
            outgoing: message.is_outgoing,
            sent: message.sending_state.is_none(),
            sender: Sender::from(&message.sender_id),
            preview: chats::snippet(message),
        }
    }
}

/// Where the next page of topics starts, as TDLib gave it.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Offset {
    pub date: i32,
    pub message_id: i64,
    pub topic_id: i32,
}

/// The topics of the forum open, for the pane.
pub struct Forum {
    pub chat_id: i64,
    /// Pinned first, then by TDLib's order.
    topics: Vec<Topic>,
    /// The topic under the cursor, by id, so it stays put as topics move.
    pub selected: Option<i32>,
    /// The page asked for: its request's number, counted across every forum
    /// opened so an answer from an earlier visit can't pass for one of this
    /// visit's, and where it starts. Answers to anything else are dropped.
    pub loading: Option<(u32, Offset)>,
    /// Where the next page starts; `None` once every topic is in.
    next: Option<Offset>,
    /// Topics asked for one by one, not to ask again before the answer.
    asking: HashSet<i32>,
}

impl Forum {
    pub fn new(chat_id: i64) -> Self {
        Self {
            chat_id,
            topics: Vec::new(),
            selected: None,
            loading: None,
            next: Some(Offset::default()),
            asking: HashSet::new(),
        }
    }

    pub fn topics(&self) -> &[Topic] {
        &self.topics
    }

    pub fn get(&self, id: i32) -> Option<&Topic> {
        self.topics.iter().find(|t| t.id == id)
    }

    /// Keeps what's left written in a topic as its draft at once, rather
    /// than when TDLib says it's saved.
    pub fn keep_draft(&mut self, id: i32, draft: Option<Draft>) {
        if let Some(topic) = self.topics.iter_mut().find(|t| t.id == id) {
            topic.draft = draft;
        }
    }

    /// The topic under the cursor: the one selected, else the first. One
    /// selected that isn't loaded yet (opened from a link) is none, not
    /// another topic.
    pub fn current(&self) -> Option<&Topic> {
        match self.selected {
            Some(id) => self.get(id),
            None => self.topics.first(),
        }
    }

    /// The topic a message is in. Messages TDLib gives no topic are in
    /// General.
    pub fn topic_of(&self, message: &Message) -> i32 {
        match &message.topic_id {
            Some(MessageTopic::Forum(t)) => t.forum_topic_id,
            _ => self.general(),
        }
    }

    /// What the topics pane needs of a new message in the forum.
    pub fn arrived(&self, message: &Message) -> Arrived {
        Arrived::of(message, self.topic_of(message))
    }

    /// The id of the General topic.
    pub fn general(&self) -> i32 {
        self.topics
            .iter()
            .find(|t| t.general)
            .map_or(GENERAL, |t| t.id)
    }

    /// Every topic is loaded.
    pub fn all_loaded(&self) -> bool {
        self.next.is_none()
    }

    /// The next page to ask for, if one is wanted: the first, or more once
    /// the cursor nears the end. Marks it as asked, as request `request`.
    pub fn page_to_ask(&mut self, request: u32) -> Option<Offset> {
        let next = self.next.filter(|_| self.loading.is_none())?;
        let at = self
            .selected
            .and_then(|id| self.topics.iter().position(|t| t.id == id))
            .unwrap_or(0);
        if !self.topics.is_empty() && at + LOAD_AHEAD < self.topics.len() {
            return None;
        }
        self.loading = Some((request, next));
        Some(next)
    }

    /// A page of topics TDLib sent for request `request`; `None` if it
    /// couldn't, which stops asking for more.
    pub fn add_page(&mut self, request: u32, page: Option<&types::ForumTopics>) {
        let Some((_, from)) = self.loading.filter(|&(asked, _)| asked == request) else {
            return;
        };
        self.loading = None;
        let Some(page) = page else {
            self.next = None;
            return;
        };
        let before = self.topics.len();
        for topic in &page.topics {
            self.upsert(topic);
        }
        let next = Offset {
            date: page.next_offset_date,
            message_id: page.next_offset_message_id,
            topic_id: page.next_offset_forum_topic_id,
        };
        // A page that adds nothing new ends it too, so paging that goes in
        // circles can't ask forever.
        let done = self.topics.len() == before
            || next == Offset::default()
            || next == from
            || self.topics.len() >= page.total_count.max(0) as usize;
        self.next = (!done).then_some(next);
    }

    /// Puts in a topic as TDLib has it now, new or not.
    pub fn upsert(&mut self, topic: &types::ForumTopic) {
        self.asking.remove(&topic.info.forum_topic_id);
        let mut new = Topic::of(topic);
        match self.topics.iter_mut().find(|t| t.id == new.id) {
            Some(old) => {
                // A draft saved from here keeps its text as typed.
                new.draft = Draft::of(topic.draft_message.as_ref(), old.draft.as_ref());
                *old = new;
            }
            None => self.topics.push(new),
        }
        self.sort();
    }

    /// TDLib couldn't say how a topic is: it's not asked again until
    /// something else changes in it.
    pub fn not_found(&mut self, id: i32) {
        self.asking.remove(&id);
    }

    /// Whether to ask TDLib for topic `id` itself (`getForumTopic`): not
    /// while an answer is on its way.
    pub fn ask(&mut self, id: i32) -> bool {
        self.asking.insert(id)
    }

    /// `updateForumTopicInfo`: a topic was renamed, closed or reopened.
    /// One not loaded is left for its page, or for its first message to
    /// bring: TDLib announces every topic a page brings before the page,
    /// and asking for each would be a request per topic.
    pub fn set_info(&mut self, info: &types::ForumTopicInfo) {
        if let Some(topic) = self.topics.iter_mut().find(|t| t.id == info.forum_topic_id) {
            topic.set_info(info);
        }
    }

    /// `updateForumTopic`: a topic was pinned or unpinned, or read. Returns
    /// the id of a topic to ask TDLib for, when reading went partway and
    /// only TDLib knows how many are left. Not for anything else it
    /// carries, which others change at will (reactions, mentions).
    pub fn set_state(&mut self, update: &types::UpdateForumTopic) -> Option<i32> {
        let topic = self
            .topics
            .iter_mut()
            .find(|t| t.id == update.forum_topic_id)?;
        topic.mentions = update.unread_mention_count;
        topic.draft = Draft::of(update.draft_message.as_ref(), topic.draft.as_ref());
        let read_before = topic.read_inbox;
        topic.read_inbox = update.last_read_inbox_message_id;
        let pinned = topic.pinned != update.is_pinned;
        topic.pinned = update.is_pinned;
        let ask = if topic.read_inbox >= topic.newest {
            topic.unread = 0;
            None
        } else {
            (topic.read_inbox > read_before).then_some(topic.id)
        };
        if pinned {
            self.sort();
        }
        ask
    }

    /// A new message in the forum: its topic moves up, with one more
    /// unread if someone else sent it. Returns the id of a topic to ask
    /// TDLib for, one not loaded yet.
    pub fn add_message(&mut self, message: &Arrived) -> Option<i32> {
        let top = self.topics.iter().map(|t| t.order).max().unwrap_or(0);
        let Some(topic) = self.topics.iter_mut().find(|t| t.id == message.topic) else {
            return Some(message.topic);
        };
        if !message.outgoing && message.id > topic.read_inbox && message.id > topic.newest {
            topic.unread += 1;
        }
        topic.set_last(message);
        topic.order = top.saturating_add(1);
        self.sort();
        None
    }

    /// Topics with their newest message gone ask TDLib how they are now.
    pub fn deleted(&self, message_ids: &[i64]) -> Vec<i32> {
        self.topics
            .iter()
            .filter(|t| message_ids.contains(&t.newest))
            .map(|t| t.id)
            .collect()
    }

    /// `j`/`k` and the like: moves the cursor `delta` topics, stopping at
    /// either end.
    pub fn move_cursor(&mut self, delta: isize) {
        if self.topics.is_empty() {
            return;
        }
        let last = self.topics.len() - 1;
        // Nothing selected is the first topic.
        let at = self
            .selected
            .and_then(|id| self.topics.iter().position(|t| t.id == id))
            .unwrap_or(0)
            .saturating_add_signed(delta)
            .min(last);
        self.selected = Some(self.topics[at].id);
    }

    /// Adds a topic made up for the demo and tests, after the ones there.
    pub fn add_local(&mut self, mut topic: Topic) {
        topic.order = -(self.topics.len() as i64);
        self.topics.push(topic);
        self.next = None;
    }

    fn sort(&mut self) {
        self.topics
            .sort_by_key(|t| std::cmp::Reverse((t.pinned, t.order, t.id)));
    }
}

#[cfg(test)]
mod tests {
    use tdlib_rs::enums::MessageSender;
    use tdlib_rs::types::{
        ChatNotificationSettings, ForumTopicIcon, ForumTopicInfo, MessageSenderUser,
    };

    use super::*;

    fn info(id: i32, name: &str) -> ForumTopicInfo {
        ForumTopicInfo {
            chat_id: -100,
            forum_topic_id: id,
            name: name.into(),
            icon: ForumTopicIcon {
                color: 0xFF_93_B2,
                custom_emoji_id: 0,
            },
            creation_date: 0,
            creator_id: MessageSender::User(MessageSenderUser { user_id: 2 }),
            is_general: id == GENERAL,
            is_outgoing: false,
            is_closed: false,
            is_hidden: false,
            is_name_implicit: false,
        }
    }

    fn topic(id: i32, name: &str, order: i64, unread: i32) -> types::ForumTopic {
        types::ForumTopic {
            info: info(id, name),
            last_message: None,
            order,
            is_pinned: false,
            unread_count: unread,
            last_read_inbox_message_id: 0,
            last_read_outbox_message_id: 0,
            unread_mention_count: 0,
            unread_reaction_count: 0,
            notification_settings: ChatNotificationSettings::default(),
            draft_message: None,
        }
    }

    fn page(topics: Vec<types::ForumTopic>, next: Offset, total: i32) -> types::ForumTopics {
        types::ForumTopics {
            total_count: total,
            topics,
            next_offset_date: next.date,
            next_offset_message_id: next.message_id,
            next_offset_forum_topic_id: next.topic_id,
        }
    }

    fn message(id: i64, topic: i32, outgoing: bool) -> Arrived {
        Arrived {
            id,
            topic,
            outgoing,
            sent: true,
            sender: Sender::User(2),
            preview: "hello".into(),
        }
    }

    fn names(forum: &Forum) -> Vec<&str> {
        forum.topics().iter().map(|t| t.name.as_str()).collect()
    }

    #[test]
    fn topics_are_listed_pinned_first_then_by_tdlibs_order() {
        let mut forum = Forum::new(-100);
        assert!(forum.page_to_ask(1).is_some(), "the first page");
        let mut pinned = topic(3, "Rules", 1, 0);
        pinned.is_pinned = true;
        let topics = vec![topic(2, "Help", 50, 0), pinned, topic(1, "General", 90, 0)];
        forum.add_page(1, Some(&page(topics, Offset::default(), 3)));
        assert_eq!(names(&forum), ["Rules", "General", "Help"]);
        assert!(forum.all_loaded());
    }

    #[test]
    fn more_topics_load_as_the_cursor_nears_the_end() {
        let mut forum = Forum::new(-100);
        assert!(forum.page_to_ask(1).is_some());
        assert_eq!(forum.page_to_ask(2), None, "one page at a time");
        let topics: Vec<_> = (1..=30)
            .map(|i| topic(i, "t", 100 - i64::from(i), 0))
            .collect();
        let next = Offset {
            date: 5,
            message_id: 6,
            topic_id: 30,
        };
        forum.add_page(1, Some(&page(topics, next, 200)));
        assert_eq!(forum.page_to_ask(2), None, "the cursor is far from the end");
        forum.move_cursor(isize::MAX);
        assert_eq!(forum.page_to_ask(3), Some(next));
        // An answer to an earlier request is dropped, even one for the
        // same page (from an earlier visit to the forum).
        forum.add_page(1, Some(&page(vec![topic(99, "late", 0, 0)], next, 200)));
        assert!(forum.get(99).is_none());
        // A page that brings nothing new ends the paging, whatever its
        // offset says.
        let again: Vec<_> = (1..=5).map(|i| topic(i, "t", 0, 0)).collect();
        let elsewhere = Offset { date: 9, ..next };
        forum.add_page(3, Some(&page(again, elsewhere, 200)));
        assert!(forum.all_loaded());
        assert_eq!(forum.page_to_ask(4), None);
    }

    #[test]
    fn a_new_message_moves_its_topic_up_and_counts_as_unread_unless_yours() {
        let mut forum = Forum::new(-100);
        forum.page_to_ask(1);
        let topics = vec![topic(1, "General", 90, 0), topic(2, "Help", 50, 0)];
        forum.add_page(1, Some(&page(topics, Offset::default(), 2)));

        assert_eq!(forum.add_message(&message(10, 2, false)), None);
        assert_eq!(names(&forum), ["Help", "General"]);
        let help = forum.get(2).unwrap();
        assert_eq!((help.unread, help.preview.as_str()), (1, "hello"));
        assert_eq!(help.from, Some(Sender::User(2)));

        forum.add_message(&message(11, 1, true));
        assert_eq!(names(&forum), ["General", "Help"]);
        let general = forum.get(1).unwrap();
        assert_eq!(general.unread, 0, "your own");
        assert!(general.yours && general.from.is_none());

        assert_eq!(
            forum.add_message(&message(12, 7, false)),
            Some(7),
            "not loaded"
        );
    }

    #[test]
    fn reading_up_to_the_newest_message_clears_the_count_else_tdlib_is_asked() {
        let mut forum = Forum::new(-100);
        forum.page_to_ask(1);
        forum.add_page(
            1,
            Some(&page(vec![topic(2, "Help", 50, 0)], Offset::default(), 1)),
        );
        forum.add_message(&message(10, 2, false));
        forum.add_message(&message(11, 2, false));
        let read = |up_to, reactions| types::UpdateForumTopic {
            chat_id: -100,
            forum_topic_id: 2,
            is_pinned: false,
            last_read_inbox_message_id: up_to,
            last_read_outbox_message_id: 0,
            unread_mention_count: 0,
            unread_reaction_count: reactions,
            notification_settings: ChatNotificationSettings::default(),
            draft_message: None,
        };
        assert_eq!(forum.set_state(&read(10, 0)), Some(2));
        // Someone reacting changes nothing that needs asking.
        assert_eq!(forum.set_state(&read(10, 1)), None);
        assert_eq!(forum.set_state(&read(10, 0)), None);
        assert_eq!(forum.set_state(&read(11, 0)), None);
        assert_eq!(forum.get(2).unwrap().unread, 0);
        // Nor does a topic that isn't loaded.
        let other = types::UpdateForumTopic {
            forum_topic_id: 9,
            ..read(11, 0)
        };
        assert_eq!(forum.set_state(&other), None);
    }

    #[test]
    fn names_are_cleaned_onto_one_line() {
        let mut forum = Forum::new(-100);
        // Not known yet: left for its page or its first message.
        forum.set_info(&info(4, "New"));
        assert!(forum.get(4).is_none());
        forum.upsert(&topic(4, "Bugs\u{202E}\nand more", 1, 0));
        assert!(forum.ask(4));
        assert!(!forum.ask(4), "already asked");
        assert_eq!(forum.get(4).unwrap().name, "Bugs and more");
    }

    #[test]
    fn the_cursor_stays_on_its_topic_as_topics_move() {
        let mut forum = Forum::new(-100);
        forum.add_local(Topic::local(1, "General", 0, 0, ""));
        forum.add_local(Topic::local(2, "Help", 0, 0, ""));
        forum.move_cursor(1);
        assert_eq!(forum.current().map(|t| t.id), Some(2));
        forum.add_message(&message(10, 2, false));
        assert_eq!(names(&forum), ["Help", "General"]);
        assert_eq!(forum.current().map(|t| t.id), Some(2));
        // A topic opened from a link, not loaded yet, isn't stood in for by
        // another.
        forum.selected = Some(7);
        assert!(forum.current().is_none());
        forum.move_cursor(1);
        assert_eq!(forum.current().map(|t| t.id), Some(1));
    }
}

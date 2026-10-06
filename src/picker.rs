//! The chat picker: `f` forwards the message under the cursor to the chat
//! picked, and `s` finds a chat, or anyone on Telegram, to open.

use std::time::Duration;

use tokio::time::Instant;

use crate::chats::Chats;

/// How long typing has to pause before Telegram is searched, so a name typed
/// quickly is one search, not one per letter.
pub const SEARCH_AFTER: Duration = Duration::from_millis(400);
/// Telegram is only searched for at least this many characters: fewer find
/// nothing useful.
const MIN_SEARCH_CHARS: usize = 2;

/// What picking a chat does.
pub enum Purpose {
    /// `f`: forward these messages of chat `from` there.
    Forward {
        from: i64,
        message_ids: Vec<i64>,
        /// The message on one line, so it's clear what's forwarded.
        snippet: String,
    },
    /// `s`: open it, or start a chat with them.
    Open,
}

/// One row of the picker.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Choice {
    /// A chat TDLib knows: one of yours, or a public one it found.
    Chat(i64),
    /// One of your contacts, with no chat yet.
    User(i64),
    /// A username typed with its @, looked up on Enter.
    Username(String),
    /// A t.me link to a chat, an invite or a message, looked up on Enter.
    Link(String),
}

pub struct ChatPicker {
    pub purpose: Purpose,
    pub query: String,
    /// The row under the cursor, by what it is rather than where: a chat
    /// moving up when a message arrives mustn't put another under the
    /// cursor just as Enter is pressed. `None` for the top row.
    current: Option<Choice>,
    /// First row shown. Drawing keeps the cursor in view.
    pub scroll: usize,
    /// When to search Telegram for the query, once typing pauses. Only to
    /// open a chat: forwarding goes to chats you have.
    search_at: Option<Instant>,
    /// What the last search was for, and what it found: your contacts, then
    /// public chats.
    searched: String,
    found: Vec<Choice>,
    /// A search is on its way, or waiting for typing to pause.
    pub searching: bool,
}

impl ChatPicker {
    pub fn new(purpose: Purpose) -> Self {
        Self {
            purpose,
            query: String::new(),
            current: None,
            scroll: 0,
            search_at: None,
            searched: String::new(),
            found: Vec::new(),
            searching: false,
        }
    }

    pub fn forwarding(&self) -> bool {
        matches!(self.purpose, Purpose::Forward { .. })
    }

    /// Changes the query, and starts again from the top. Opening a chat
    /// searches Telegram once typing pauses.
    pub fn edit_query(&mut self, edit: impl FnOnce(&mut String), now: Instant) {
        edit(&mut self.query);
        self.current = None;
        let wanted = search_text(&self.query);
        if self.forwarding()
            || wanted.chars().count() < MIN_SEARCH_CHARS
            || typed(&self.query).is_some_and(|c| matches!(c, Choice::Link(_)))
        {
            self.search_at = None;
            self.searching = false;
        } else if wanted != self.searched {
            self.search_at = Some(now + SEARCH_AFTER);
            self.searching = true;
        }
    }

    /// When to wake up to search Telegram.
    pub fn search_at(&self) -> Option<Instant> {
        self.search_at
    }

    /// What to search Telegram for, once typing has paused long enough.
    pub fn due_search(&mut self, now: Instant) -> Option<String> {
        self.search_at.filter(|&at| at <= now)?;
        self.search_at = None;
        self.searched = search_text(&self.query).to_string();
        Some(self.searched.clone())
    }

    /// What Telegram found for `query`, unless the query changed since.
    pub fn set_found(&mut self, query: &str, chat_ids: Vec<i64>, user_ids: Vec<i64>) {
        if query != self.searched {
            return;
        }
        self.found = user_ids
            .into_iter()
            .map(Choice::User)
            .chain(chat_ids.into_iter().map(Choice::Chat))
            .collect();
        self.searching = self.search_at.is_some();
    }

    /// The rows: a typed @username or link first, then your chats matching
    /// the query, then whom Telegram found that you have no chat with.
    /// Forwarding lists only your chats, Saved Messages first, as in
    /// Telegram.
    pub fn choices(&self, chats: &Chats) -> Vec<Choice> {
        let mut out = Vec::new();
        let mine = chats.matching(&self.query);
        if self.forwarding() {
            let (saved, rest): (Vec<i64>, Vec<i64>) =
                mine.into_iter().partition(|&id| chats.is_saved(id));
            out.extend(saved.into_iter().chain(rest).map(Choice::Chat));
            return out;
        }
        out.extend(typed(&self.query));
        out.extend(mine.iter().copied().map(Choice::Chat));
        // The query may have changed since the search; what it found only
        // shows while it still fits.
        if self.searched == search_text(&self.query) {
            for choice in &self.found {
                let known = match choice {
                    Choice::Chat(id) => mine.contains(id),
                    // A private chat's id is the other person's user id.
                    Choice::User(id) => mine.contains(id),
                    _ => false,
                };
                if !known && !out.contains(choice) {
                    out.push(choice.clone());
                }
            }
        }
        out
    }

    /// Where the cursor is in `choices`: on the row it was put on, or the
    /// top one if that's gone.
    pub fn selected(&self, choices: &[Choice]) -> usize {
        self.current
            .as_ref()
            .and_then(|current| choices.iter().position(|c| c == current))
            .unwrap_or(0)
    }

    /// The row under the cursor, what Enter picks.
    pub fn current(&self, choices: &[Choice]) -> Option<Choice> {
        choices.get(self.selected(choices)).cloned()
    }

    /// Moves the cursor `delta` rows through `choices`, stopping at the ends.
    pub fn move_by(&mut self, delta: isize, choices: &[Choice]) {
        let at = self
            .selected(choices)
            .saturating_add_signed(delta)
            .min(choices.len().saturating_sub(1));
        self.current = choices.get(at).cloned();
    }
}

/// The query as searched: without spaces around it or the @ of a username.
fn search_text(query: &str) -> &str {
    let query = query.trim();
    query.strip_prefix('@').unwrap_or(query)
}

/// What a query names by itself: a username after an @, or a link to
/// t.me (also written `telegram.me`, `telegram.dog` or `tg:`).
pub fn typed(query: &str) -> Option<Choice> {
    let query = query.trim();
    if query.contains(char::is_whitespace) {
        return None;
    }
    if let Some(name) = query.strip_prefix('@') {
        let valid = (1..=32).contains(&name.len())
            && name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_');
        return valid.then(|| Choice::Username(name.to_string()));
    }
    let lower = query.to_ascii_lowercase();
    if lower.starts_with("tg:") {
        return Some(Choice::Link(query.to_string()));
    }
    let scheme = ["https://", "http://"]
        .into_iter()
        .find(|s| lower.starts_with(s))
        .map_or(0, str::len);
    let bare = &lower[scheme..];
    let telegram = ["t.me/", "telegram.me/", "telegram.dog/"]
        .into_iter()
        .any(|host| bare.starts_with(host) && bare.len() > host.len());
    telegram.then(|| Choice::Link(format!("https://{}", &query[scheme..])))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn usernames_and_telegram_links_are_recognized_as_typed() {
        assert_eq!(typed("@durov"), Some(Choice::Username("durov".into())));
        assert_eq!(typed(" @a_b1 "), Some(Choice::Username("a_b1".into())));
        assert_eq!(typed("@"), None);
        assert_eq!(typed("@not valid"), None);
        assert_eq!(typed("@ünï"), None);
        assert_eq!(
            typed("t.me/durov"),
            Some(Choice::Link("https://t.me/durov".into()))
        );
        assert_eq!(
            typed("HTTPS://T.me/+AbC"),
            Some(Choice::Link("https://T.me/+AbC".into())),
            "the invite's case is kept"
        );
        assert_eq!(
            typed("tg://resolve?domain=durov"),
            Some(Choice::Link("tg://resolve?domain=durov".into()))
        );
        assert_eq!(typed("https://t.me/"), None);
        assert_eq!(typed("https://example.com/t.me/x"), None);
        assert_eq!(typed("durov"), None);
    }

    fn picker_with_chats() -> (ChatPicker, Chats) {
        let mut chats = Chats::default();
        chats.add_local(1, "Alice", None);
        chats.add_local(2, "Bob", None);
        chats.add_local(3, "Alina's group", None);
        chats.refresh();
        (ChatPicker::new(Purpose::Open), chats)
    }

    #[test]
    fn telegram_is_searched_once_typing_pauses_and_stale_results_are_dropped() {
        let (mut picker, chats) = picker_with_chats();
        let start = Instant::now();
        picker.edit_query(|q| q.push('a'), start);
        assert_eq!(picker.search_at(), None, "one letter finds nothing useful");
        picker.edit_query(|q| q.push('l'), start);
        assert_eq!(picker.due_search(start), None, "still typing");
        assert!(picker.searching);
        let later = start + SEARCH_AFTER;
        assert_eq!(picker.due_search(later).as_deref(), Some("al"));
        assert_eq!(picker.due_search(later), None, "once");

        // Contacts first, then public chats; ones you have a chat with are
        // already listed.
        picker.set_found("al", vec![50, 1], vec![1, 60]);
        assert!(!picker.searching);
        assert_eq!(
            picker.choices(&chats),
            [
                Choice::Chat(1),
                Choice::Chat(3),
                Choice::User(60),
                Choice::Chat(50)
            ]
        );

        // An answer for an older query is dropped.
        picker.edit_query(|q| q.push('i'), later);
        picker.set_found("al", vec![70], Vec::new());
        assert_eq!(picker.choices(&chats), [Choice::Chat(1), Choice::Chat(3)]);
    }

    #[test]
    fn a_typed_username_comes_first_and_links_arent_searched_for() {
        let (mut picker, chats) = picker_with_chats();
        let now = Instant::now();
        picker.edit_query(|q| q.push_str("@ali"), now);
        assert_eq!(picker.choices(&chats)[0], Choice::Username("ali".into()));
        assert!(
            picker.search_at().is_some(),
            "Telegram is searched for \"ali\""
        );

        picker.edit_query(|q| *q = "t.me/alice".into(), now);
        assert_eq!(picker.search_at(), None);
        assert_eq!(
            picker.choices(&chats)[0],
            Choice::Link("https://t.me/alice".into())
        );
    }

    #[test]
    fn the_cursor_stays_on_its_chat_when_the_list_reorders() {
        use tdlib_rs::enums::ChatList;
        use tdlib_rs::types::ChatPosition;
        let (mut picker, mut chats) = picker_with_chats();
        let choices = picker.choices(&chats);
        assert_eq!(choices, [Choice::Chat(1), Choice::Chat(2), Choice::Chat(3)]);
        picker.move_by(1, &choices);
        assert_eq!(picker.current(&choices), Some(Choice::Chat(2)));

        // A message in chat 3 moves it to the top.
        let position = ChatPosition {
            list: ChatList::Main,
            order: 5000,
            is_pinned: false,
            source: None,
        };
        chats.set_position(3, &position);
        chats.refresh();
        let choices = picker.choices(&chats);
        assert_eq!(choices[1], Choice::Chat(1));
        assert_eq!(picker.selected(&choices), 2);
        assert_eq!(picker.current(&choices), Some(Choice::Chat(2)), "still Bob");

        picker.edit_query(|q| q.push('a'), Instant::now());
        let choices = picker.choices(&chats);
        assert_eq!(
            picker.selected(&choices),
            0,
            "a new search starts at the top"
        );
    }

    #[test]
    fn forwarding_lists_your_chats_with_saved_messages_first() {
        let (_, mut chats) = picker_with_chats();
        chats.set_my_id(3);
        chats.refresh();
        let mut picker = ChatPicker::new(Purpose::Forward {
            from: 1,
            message_ids: vec![9],
            snippet: "hi".into(),
        });
        assert_eq!(
            picker.choices(&chats),
            [Choice::Chat(3), Choice::Chat(1), Choice::Chat(2)]
        );
        picker.edit_query(|q| q.push_str("@bob"), Instant::now());
        assert_eq!(picker.search_at(), None, "only your chats");
        assert!(picker.choices(&chats).is_empty(), "Bob has no username");
        picker.edit_query(|q| *q = "b".into(), Instant::now());
        assert_eq!(picker.choices(&chats), [Choice::Chat(2)]);
    }
}

//! Service messages: what Telegram says happened in a chat (someone joined,
//! was added or left, the group was renamed, a message pinned), drawn in
//! the middle of the chat like a date rather than in a bubble.

use tdlib_rs::enums::MessageContent;

use crate::messages::{Sender, duration};
use crate::text;

/// What a service message says, with the people it names kept as ids, to
/// be named when it's drawn.
#[derive(Clone, Debug, PartialEq)]
pub enum Service {
    /// The sender did it; the words go after their name: "pinned a message".
    Did(String),
    /// The same, naming what someone called something: "created the topic",
    /// then the topic's name, in quotes.
    Quoting(String, String),
    /// The sender added these people. Themselves alone: they joined.
    Added(Vec<i64>),
    /// The sender removed this person. Themselves: they left.
    Removed(i64),
    /// Words of their own, after nobody's name.
    Note(String),
}

/// Names a service message lists before saying how many more there are.
const MAX_NAMES: usize = 3;

/// A piece of a service message's sentence: tuigram's own words, or what
/// someone picked (their name, a title). They're drawn apart, so a name
/// like "Thu 8 Oct" or "Bob removed Alice." can't pass for tuigram's words.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Part {
    Words(String),
    /// Someone's name.
    Name(String),
    /// You, who did it.
    You,
    /// A title someone picked, in quotes.
    Quote(String),
}

impl Part {
    pub fn text(&self) -> &str {
        match self {
            Part::Words(text) | Part::Name(text) | Part::Quote(text) => text,
            Part::You => "You",
        }
    }
}

/// The sentence as one string, for a notification.
pub fn text(parts: &[Part]) -> String {
    parts.iter().map(Part::text).collect()
}

impl Service {
    /// The service message `content` is, if it's one tuigram knows.
    pub fn of(content: &MessageContent) -> Option<Self> {
        use MessageContent as C;
        let did = |words: &str| Some(Service::Did(words.into()));
        // Names and titles are other people's words.
        let quoting = |words: &str, title: &str| {
            let title = text::clean(title).replace(['\n', '\t'], " ");
            let title = format!("\"{}\"", text::first_chars(&title, 128));
            Some(Service::Quoting(words.into(), title))
        };
        match content {
            C::MessageChatAddMembers(m) => Some(Service::Added(m.member_user_ids.clone())),
            C::MessageChatDeleteMember(m) => Some(Service::Removed(m.user_id)),
            C::MessageChatJoinByLink => did("joined via an invite link"),
            C::MessageChatJoinByRequest => did("joined, approved by an admin"),
            C::MessageBasicGroupChatCreate(m) => quoting("created the group", &m.title),
            C::MessageSupergroupChatCreate(m) => quoting("created", &m.title),
            C::MessageChatChangeTitle(m) => quoting("changed the name to", &m.title),
            C::MessageChatChangePhoto(_) => did("changed the photo"),
            C::MessageChatDeletePhoto => did("removed the photo"),
            C::MessageChatUpgradeTo(_) | C::MessageChatUpgradeFrom(_) => {
                did("upgraded the group to a supergroup")
            }
            C::MessagePinMessage(_) => did("pinned a message"),
            C::MessageScreenshotTaken => did("took a screenshot"),
            C::MessageChatSetMessageAutoDeleteTime(m) => match m.message_auto_delete_time {
                0 => did("turned the timer off"),
                secs => did(&format!(
                    "set messages to disappear after {}",
                    crate::secret::timer_words(secs)
                )),
            },
            C::MessageVideoChatScheduled(_) => did("scheduled a video chat"),
            C::MessageVideoChatStarted(_) => did("started a video chat"),
            C::MessageVideoChatEnded(m) => {
                did(&format!("ended the video chat ({})", duration(m.duration)))
            }
            C::MessageInviteVideoChatParticipants(_) => did("invited people to the video chat"),
            C::MessageChatSetTheme(_) => did("changed the chat theme"),
            C::MessageChatSetBackground(_) => did("changed the chat background"),
            C::MessageChatBoost(m) if m.boost_count > 1 => {
                did(&format!("boosted the group {} times", m.boost_count))
            }
            C::MessageChatBoost(_) => did("boosted the group"),
            C::MessageForumTopicCreated(m) => quoting("created the topic", &m.name),
            C::MessageForumTopicEdited(_) => did("edited a topic"),
            C::MessageForumTopicIsClosedToggled(m) if m.is_closed => did("closed a topic"),
            C::MessageForumTopicIsClosedToggled(_) => did("reopened a topic"),
            C::MessageForumTopicIsHiddenToggled(m) if m.is_hidden => did("hid the General topic"),
            C::MessageForumTopicIsHiddenToggled(_) => did("showed the General topic"),
            C::MessageGameScore(m) => did(&format!("scored {} in a game", m.score)),
            C::MessageContactRegistered => did("joined Telegram"),
            C::MessageCustomServiceAction(m) => {
                let words = text::clean(&m.text).replace(['\n', '\t'], " ");
                Some(Service::Note(text::first_chars(&words, 300).to_string()))
            }
            _ => None,
        }
    }

    /// What happened, without names, for the chat list: "Joined the group".
    pub fn label(&self, sender: Sender) -> String {
        let words = match self {
            Service::Added(ids) if is_only(ids, sender) => "joined the group".to_string(),
            Service::Added(ids) if ids.len() == 1 => "added a member".to_string(),
            Service::Added(ids) => format!("added {} members", ids.len()),
            Service::Removed(id) if is_only(&[*id], sender) => "left the group".to_string(),
            Service::Removed(_) => "removed a member".to_string(),
            Service::Did(words) | Service::Note(words) => words.clone(),
            Service::Quoting(words, title) => format!("{words} {title}"),
        };
        let mut chars = words.chars();
        chars.next().map_or(String::new(), |first| {
            first.to_uppercase().chain(chars).collect()
        })
    }

    /// What happened, as a sentence: "Alice added Bob and Carol". `actor`
    /// is the sender (a name, or you), and `name` names the people it
    /// mentions.
    pub fn sentence(&self, sender: Sender, actor: Part, name: impl Fn(i64) -> String) -> Vec<Part> {
        let words = |text: &str| Part::Words(text.into());
        match self {
            Service::Added(ids) if is_only(ids, sender) => vec![actor, words(" joined the group")],
            Service::Added(ids) => {
                let mut parts = vec![actor, words(" added ")];
                parts.extend(names(ids, name));
                parts
            }
            Service::Removed(id) if is_only(&[*id], sender) => {
                vec![actor, words(" left the group")]
            }
            Service::Removed(id) => vec![actor, words(" removed "), Part::Name(name(*id))],
            Service::Did(did) => vec![actor, words(&format!(" {did}"))],
            Service::Quoting(did, title) => vec![
                actor,
                words(&format!(" {did} ")),
                Part::Quote(title.clone()),
            ],
            Service::Note(note) => vec![words(note)],
        }
    }
}

/// `ids` is just the sender.
fn is_only(ids: &[i64], sender: Sender) -> bool {
    matches!((ids, sender), ([id], Sender::User(user)) if *id == user)
}

/// "Bob", "Bob and Carol", "Bob, Carol, Dan and 2 others".
fn names(ids: &[i64], name: impl Fn(i64) -> String) -> Vec<Part> {
    let shown: Vec<Part> = ids
        .iter()
        .take(MAX_NAMES)
        .map(|&id| Part::Name(name(id)))
        .collect();
    let more = ids.len().saturating_sub(MAX_NAMES);
    let others = match more {
        0 => None,
        1 => Some("1 other".to_string()),
        n => Some(format!("{n} others")),
    };
    let count = shown.len() + usize::from(others.is_some());
    if count == 0 {
        return vec![Part::Words("nobody".into())];
    }
    let mut parts = Vec::new();
    let items = shown.into_iter().chain(others.map(Part::Words)).enumerate();
    for (i, item) in items {
        if i > 0 {
            let joint = if i + 1 == count { " and " } else { ", " };
            parts.push(Part::Words(joint.into()));
        }
        parts.push(item);
    }
    parts
}

#[cfg(test)]
mod tests {
    use tdlib_rs::types;

    use super::*;

    fn name(id: i64) -> String {
        ["", "Alice", "Bob", "Carol", "Dan", "Eve"][id as usize].into()
    }

    fn sentence(content: MessageContent, sender: i64) -> String {
        let service = Service::of(&content).expect("a service message");
        let sender = Sender::User(sender);
        text(&service.sentence(sender, Part::Name(name(1)), name))
    }

    fn added(ids: &[i64]) -> MessageContent {
        MessageContent::MessageChatAddMembers(types::MessageChatAddMembers {
            member_user_ids: ids.to_vec(),
        })
    }

    #[test]
    fn people_joining_being_added_and_leaving_read_as_sentences() {
        assert_eq!(sentence(added(&[1]), 1), "Alice joined the group");
        assert_eq!(sentence(added(&[2]), 1), "Alice added Bob");
        assert_eq!(sentence(added(&[2, 3]), 1), "Alice added Bob and Carol");
        assert_eq!(
            sentence(added(&[2, 3, 4, 5]), 1),
            "Alice added Bob, Carol, Dan and 1 other"
        );
        let removed = |user_id| {
            MessageContent::MessageChatDeleteMember(types::MessageChatDeleteMember { user_id })
        };
        assert_eq!(sentence(removed(1), 1), "Alice left the group");
        assert_eq!(sentence(removed(2), 1), "Alice removed Bob");
        assert_eq!(
            sentence(MessageContent::MessageChatJoinByLink, 1),
            "Alice joined via an invite link"
        );
    }

    #[test]
    fn the_chat_list_says_what_happened_without_names() {
        let label = |content, sender| Service::of(&content).unwrap().label(Sender::User(sender));
        assert_eq!(label(added(&[1]), 1), "Joined the group");
        assert_eq!(label(added(&[2, 3]), 1), "Added 2 members");
        assert_eq!(
            label(MessageContent::MessageScreenshotTaken, 1),
            "Took a screenshot"
        );
    }

    #[test]
    fn a_new_name_is_cleaned_and_kept_on_one_line() {
        let title = MessageContent::MessageChatChangeTitle(types::MessageChatChangeTitle {
            title: "Trip\u{202E}\nplans".into(),
        });
        assert_eq!(
            sentence(title, 1),
            "Alice changed the name to \"Trip plans\""
        );
    }

    #[test]
    fn names_and_titles_are_parts_of_their_own_apart_from_the_words() {
        let service = Service::of(&added(&[2, 3])).unwrap();
        let parts = service.sentence(Sender::User(1), Part::You, name);
        assert_eq!(
            parts,
            [
                Part::You,
                Part::Words(" added ".into()),
                Part::Name("Bob".into()),
                Part::Words(" and ".into()),
                Part::Name("Carol".into()),
            ]
        );
        let topic = MessageContent::MessageForumTopicCreated(types::MessageForumTopicCreated {
            name: "Thu 8 Oct".into(),
            ..Default::default()
        });
        let parts =
            Service::of(&topic)
                .unwrap()
                .sentence(Sender::User(2), Part::Name("Bob".into()), name);
        assert_eq!(parts.last(), Some(&Part::Quote("\"Thu 8 Oct\"".into())));
        assert_eq!(text(&parts), "Bob created the topic \"Thu 8 Oct\"");
    }

    #[test]
    fn messages_that_arent_about_the_chat_are_not_service_messages() {
        assert_eq!(
            Service::of(&MessageContent::MessageContact(Default::default())),
            None
        );
        assert_eq!(Service::of(&MessageContent::MessageExpiredPhoto), None);
    }
}

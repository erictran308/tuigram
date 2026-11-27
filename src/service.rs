//! Service messages: what Telegram says happened in a chat (someone joined,
//! was added or left, the group was renamed, a message pinned), drawn in
//! the middle of the chat like a date rather than in a bubble.

use tdlib_rs::enums::MessageContent;

use crate::lang;
use crate::messages::{Sender, duration};
use crate::text;

/// What a service message says, with the people it names kept as ids, to
/// be named when it's drawn.
#[derive(Clone, Debug, PartialEq)]
pub enum Service {
    /// The sender did it; the words go after their name: "pinned a message".
    Did(Deed),
    /// The same, naming what someone called something: "created the topic",
    /// then the topic's name, in quotes.
    Quoting(Deed, String),
    /// The sender added these people. Themselves alone: they joined.
    Added(Vec<i64>),
    /// The sender removed this person. Themselves: they left.
    Removed(i64),
    /// Words of their own, after nobody's name.
    Note(String),
}

/// What someone did, in the words that go after their name. Russian says
/// it one way after a name, «закрепил(а)», and another after «Вы»
/// (`lang::tc("you", …)`).
#[derive(Clone, Debug, PartialEq)]
pub struct Deed {
    them: String,
    you: String,
}

impl Deed {
    /// `them` after a name, `you` after "you".
    fn said(them: &str, you: &str) -> Self {
        Self {
            them: them.into(),
            you: you.into(),
        }
    }

    /// The same with `{hole}` filled in both by `value`, which isn't read
    /// for more, as `lang::f` fills it.
    fn filled(them: &str, you: &str, hole: &str, value: &str) -> Self {
        let fill = |text| lang::fill(text, &[(hole, &value)]);
        Self::said(&fill(them), &fill(you))
    }

    fn after(&self, you: bool) -> &str {
        if you { &self.you } else { &self.them }
    }
}

/// The same words after anyone's name.
impl From<&str> for Deed {
    fn from(words: &str) -> Self {
        Self {
            them: words.into(),
            you: words.into(),
        }
    }
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
            Part::You => lang::t("You"),
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
        let did = |them: &str, you: &str| Some(Service::Did(Deed::said(them, you)));
        // Names and titles are other people's words.
        let quoting = |deed: Deed, title: &str| {
            let title = text::clean(title).replace(['\n', '\t'], " ");
            let title = text::first_chars(&title, 128);
            let title = lang::f("\"{title}\"", &[("title", &title)]);
            Some(Service::Quoting(deed, title))
        };
        match content {
            C::MessageChatAddMembers(m) => Some(Service::Added(m.member_user_ids.clone())),
            C::MessageChatDeleteMember(m) => Some(Service::Removed(m.user_id)),
            C::MessageChatJoinByLink => did(
                lang::t("joined via an invite link"),
                lang::tc("you", "joined via an invite link"),
            ),
            C::MessageChatJoinByRequest => did(
                lang::t("joined, approved by an admin"),
                lang::tc("you", "joined, approved by an admin"),
            ),
            C::MessageBasicGroupChatCreate(m) => quoting(
                Deed::said(
                    lang::t("created the group"),
                    lang::tc("you", "created the group"),
                ),
                &m.title,
            ),
            C::MessageSupergroupChatCreate(m) => quoting(
                Deed::said(lang::t("created"), lang::tc("you", "created")),
                &m.title,
            ),
            C::MessageChatChangeTitle(m) => quoting(
                Deed::said(
                    lang::t("changed the name to"),
                    lang::tc("you", "changed the name to"),
                ),
                &m.title,
            ),
            C::MessageChatChangePhoto(_) => did(
                lang::t("changed the photo"),
                lang::tc("you", "changed the photo"),
            ),
            C::MessageChatDeletePhoto => did(
                lang::t("removed the photo"),
                lang::tc("you", "removed the photo"),
            ),
            C::MessageChatUpgradeTo(_) | C::MessageChatUpgradeFrom(_) => did(
                lang::t("upgraded the group to a supergroup"),
                lang::tc("you", "upgraded the group to a supergroup"),
            ),
            C::MessagePinMessage(_) => did(
                lang::t("pinned a message"),
                lang::tc("you", "pinned a message"),
            ),
            C::MessageScreenshotTaken => did(
                lang::t("took a screenshot"),
                lang::tc("you", "took a screenshot"),
            ),
            C::MessageChatSetMessageAutoDeleteTime(m) => match m.message_auto_delete_time {
                0 => did(
                    lang::t("turned the timer off"),
                    lang::tc("you", "turned the timer off"),
                ),
                secs => Some(Service::Did(Deed::filled(
                    lang::t("set messages to disappear after {time}"),
                    lang::tc("you", "set messages to disappear after {time}"),
                    "time",
                    &crate::secret::timer_words(secs),
                ))),
            },
            C::MessageVideoChatScheduled(_) => did(
                lang::t("scheduled a video chat"),
                lang::tc("you", "scheduled a video chat"),
            ),
            C::MessageVideoChatStarted(_) => did(
                lang::t("started a video chat"),
                lang::tc("you", "started a video chat"),
            ),
            C::MessageVideoChatEnded(m) => Some(Service::Did(Deed::filled(
                lang::t("ended the video chat ({length})"),
                lang::tc("you", "ended the video chat ({length})"),
                "length",
                &duration(m.duration),
            ))),
            C::MessageInviteVideoChatParticipants(_) => did(
                lang::t("invited people to the video chat"),
                lang::tc("you", "invited people to the video chat"),
            ),
            C::MessageChatSetTheme(_) => did(
                lang::t("changed the chat theme"),
                lang::tc("you", "changed the chat theme"),
            ),
            C::MessageChatSetBackground(_) => did(
                lang::t("changed the chat background"),
                lang::tc("you", "changed the chat background"),
            ),
            C::MessageChatBoost(m) if m.boost_count > 1 => {
                let times = lang::n(m.boost_count.into(), "{n} time", "{n} times", &[]);
                Some(Service::Did(Deed::filled(
                    lang::t("boosted the group {times}"),
                    lang::tc("you", "boosted the group {times}"),
                    "times",
                    &times,
                )))
            }
            C::MessageChatBoost(_) => did(
                lang::t("boosted the group"),
                lang::tc("you", "boosted the group"),
            ),
            C::MessageForumTopicCreated(m) => quoting(
                Deed::said(
                    lang::t("created the topic"),
                    lang::tc("you", "created the topic"),
                ),
                &m.name,
            ),
            C::MessageForumTopicEdited(_) => {
                did(lang::t("edited a topic"), lang::tc("you", "edited a topic"))
            }
            C::MessageForumTopicIsClosedToggled(m) if m.is_closed => {
                did(lang::t("closed a topic"), lang::tc("you", "closed a topic"))
            }
            C::MessageForumTopicIsClosedToggled(_) => did(
                lang::t("reopened a topic"),
                lang::tc("you", "reopened a topic"),
            ),
            C::MessageForumTopicIsHiddenToggled(m) if m.is_hidden => did(
                lang::t("hid the General topic"),
                lang::tc("you", "hid the General topic"),
            ),
            C::MessageForumTopicIsHiddenToggled(_) => did(
                lang::t("showed the General topic"),
                lang::tc("you", "showed the General topic"),
            ),
            C::MessageGameScore(m) => {
                // The count's words name what's counted where a language does.
                let points = lang::n(m.score.into(), "{n} in a game", "{n} in a game", &[]);
                Some(Service::Did(Deed::filled(
                    lang::t("scored {points}"),
                    lang::tc("you", "scored {points}"),
                    "points",
                    &points,
                )))
            }
            C::MessageContactRegistered => did(
                lang::t("joined Telegram"),
                lang::tc("you", "joined Telegram"),
            ),
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
            Service::Added(ids) if is_only(ids, sender) => lang::t("joined the group").to_string(),
            Service::Added(ids) if ids.len() == 1 => lang::t("added a member").to_string(),
            Service::Added(ids) => lang::n(
                ids.len() as i64,
                "added {n} member",
                "added {n} members",
                &[],
            ),
            Service::Removed(id) if is_only(&[*id], sender) => {
                lang::t("left the group").to_string()
            }
            Service::Removed(_) => lang::t("removed a member").into(),
            Service::Did(deed) => deed.them.clone(),
            Service::Note(words) => words.clone(),
            Service::Quoting(deed, title) => format!("{} {title}", deed.them),
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
        let you = actor == Part::You;
        let words = |text: &str| Part::Words(text.into());
        // Russian's verb is another after «Вы» than after a name.
        let verb = |them: &str, yours: &str| words(if you { yours } else { them });
        match self {
            Service::Added(ids) if is_only(ids, sender) => vec![
                actor,
                verb(
                    lang::t(" joined the group"),
                    lang::tc("you", " joined the group"),
                ),
            ],
            Service::Added(ids) => {
                let mut parts = vec![actor, verb(lang::t(" added "), lang::tc("you", " added "))];
                parts.extend(names(ids, name));
                parts
            }
            Service::Removed(id) if is_only(&[*id], sender) => vec![
                actor,
                verb(
                    lang::t(" left the group"),
                    lang::tc("you", " left the group"),
                ),
            ],
            Service::Removed(id) => vec![
                actor,
                verb(lang::t(" removed "), lang::tc("you", " removed ")),
                Part::Name(name(*id)),
            ],
            Service::Did(did) => vec![actor, words(&format!(" {}", did.after(you)))],
            Service::Quoting(did, title) => vec![
                actor,
                words(&format!(" {} ", did.after(you))),
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
    let others = (more > 0).then(|| lang::n(more as i64, "{n} other", "{n} others", &[]));
    let count = shown.len() + usize::from(others.is_some());
    if count == 0 {
        return vec![Part::Words(lang::t("nobody").into())];
    }
    let mut parts = Vec::new();
    let items = shown.into_iter().chain(others.map(Part::Words)).enumerate();
    for (i, item) in items {
        if i > 0 {
            let joint = if i + 1 == count {
                lang::t(" and ")
            } else {
                ", "
            };
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
    fn in_russian_the_verb_says_whether_you_did_it() {
        crate::lang::set(crate::lang::Lang::Ru);
        assert_eq!(sentence(added(&[2, 3]), 1), "Alice добавил(а) Bob и Carol");
        assert_eq!(
            sentence(added(&[2, 3, 4, 5]), 1),
            "Alice добавил(а) Bob, Carol, Dan и ещё 1"
        );
        let pinned = MessageContent::MessagePinMessage(types::MessagePinMessage { message_id: 1 });
        let service = Service::of(&pinned).unwrap();
        let parts = service.sentence(Sender::User(1), Part::You, name);
        assert_eq!(
            parts,
            [Part::You, Part::Words(" закрепили сообщение".into())]
        );
        assert_eq!(text(&parts), "Вы закрепили сообщение");
        assert_eq!(sentence(pinned, 1), "Alice закрепил(а) сообщение");
        assert_eq!(service.label(Sender::User(1)), "Закрепил(а) сообщение");
        let title = MessageContent::MessageChatChangeTitle(types::MessageChatChangeTitle {
            title: "Trip plans".into(),
        });
        let parts =
            Service::of(&title)
                .unwrap()
                .sentence(Sender::User(2), Part::Name("Bob".into()), name);
        assert_eq!(parts.last(), Some(&Part::Quote("«Trip plans»".into())));
        assert_eq!(text(&parts), "Bob изменил(а) название на «Trip plans»");
        crate::lang::set(crate::lang::Lang::En);
    }

    #[test]
    fn in_russian_counts_keep_the_verb_for_who_did_it() {
        crate::lang::set(crate::lang::Lang::Ru);
        let boost =
            |boost_count| MessageContent::MessageChatBoost(types::MessageChatBoost { boost_count });
        assert_eq!(sentence(boost(2), 1), "Alice забустил(а) группу 2 раза");
        let you = |content| {
            let service = Service::of(&content).unwrap();
            text(&service.sentence(Sender::User(1), Part::You, name))
        };
        assert_eq!(you(boost(5)), "Вы забустили группу 5 раз");
        let score = MessageContent::MessageGameScore(types::MessageGameScore {
            score: 21,
            ..Default::default()
        });
        assert_eq!(sentence(score.clone(), 1), "Alice набрал(а) 21 очко в игре");
        assert_eq!(you(score), "Вы набрали 21 очко в игре");
        let label = Service::of(&added(&[2, 3])).unwrap().label(Sender::User(1));
        assert_eq!(label, "Добавил(а) участников: 2");
        crate::lang::set(crate::lang::Lang::En);
        assert_eq!(sentence(boost(2), 1), "Alice boosted the group 2 times");
    }

    #[test]
    fn in_vietnamese_the_verb_is_the_same_whoever_did_it() {
        crate::lang::set(crate::lang::Lang::Vi);
        assert_eq!(sentence(added(&[2, 3]), 1), "Alice đã thêm Bob và Carol");
        assert_eq!(
            sentence(added(&[2, 3, 4, 5, 1]), 1),
            "Alice đã thêm Bob, Carol, Dan và 2 người khác"
        );
        let pinned = MessageContent::MessagePinMessage(types::MessagePinMessage { message_id: 1 });
        let service = Service::of(&pinned).unwrap();
        let parts = service.sentence(Sender::User(1), Part::You, name);
        assert_eq!(text(&parts), "Bạn đã ghim một tin nhắn");
        assert_eq!(sentence(pinned, 1), "Alice đã ghim một tin nhắn");
        assert_eq!(service.label(Sender::User(1)), "Đã ghim một tin nhắn");
        let title = MessageContent::MessageChatChangeTitle(types::MessageChatChangeTitle {
            title: "Trip plans".into(),
        });
        let parts =
            Service::of(&title)
                .unwrap()
                .sentence(Sender::User(2), Part::Name("Bob".into()), name);
        assert_eq!(parts.last(), Some(&Part::Quote("\"Trip plans\"".into())));
        assert_eq!(text(&parts), "Bob đã đổi tên thành \"Trip plans\"");
        crate::lang::set(crate::lang::Lang::En);
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

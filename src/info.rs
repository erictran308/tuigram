//! The chat info popup (`I`): who or what a chat is, and who's in it.

use tdlib_rs::enums::{ChatMemberStatus, MessageSender};
use tdlib_rs::types;

use crate::messages::{Sender, one_line};
use crate::text;

/// Members asked for at once, TDLib's most.
pub const MEMBERS_PAGE: i32 = 200;
/// More members are asked for once the cursor is this close to the last one
/// loaded.
const LOAD_AHEAD: usize = 20;
/// Members listed at most: Telegram gives no more than 10,000 of a group's
/// recent ones anyway.
const MAX_MEMBERS: usize = 10_000;

/// What TDLib says about a chat beyond what the chat list knows. Everything
/// in it was written by the person or the group's admins, and is cleaned.
#[derive(Default)]
pub struct About {
    /// A person's phone number, if they share it with you.
    pub phone: String,
    /// A person's bio, a bot's description, or a group's or channel's.
    pub bio: String,
    /// Groups you're both in, with a person.
    pub common_groups: i32,
    /// Members in all, of a group or channel; 0 if unknown.
    pub member_count: i32,
    pub roster: Roster,
}

impl About {
    /// What a person's profile says: their phone number and bio, or what a
    /// bot says it does.
    pub fn of_user(user: &types::User, full: Option<&types::UserFullInfo>) -> Self {
        let bio = full.map_or(String::new(), |full| match &full.bot_info {
            Some(bot) if !bot.short_description.is_empty() => bot.short_description.clone(),
            Some(bot) => bot.description.clone(),
            None => full.bio.as_ref().map_or(String::new(), |b| b.text.clone()),
        });
        Self {
            phone: text::clean(&user.phone_number),
            bio: text::clean(bio.trim()),
            common_groups: full.map_or(0, |f| f.group_in_common_count),
            member_count: 0,
            roster: Roster::None,
        }
    }

    /// A basic group's description and all its members, who come with it.
    pub fn of_basic_group(
        group: &types::BasicGroup,
        full: Option<&types::BasicGroupFullInfo>,
    ) -> Self {
        let members = full.map_or(Vec::new(), |f| {
            let mut members: Vec<Member> = f.members.iter().filter_map(Member::of).collect();
            // The owner and admins first, as Telegram lists them.
            members.sort_by_key(|m| m.role);
            members
        });
        Self {
            phone: String::new(),
            bio: full.map_or(String::new(), |f| text::clean(f.description.trim())),
            common_groups: 0,
            member_count: group.member_count,
            roster: Roster::All(members),
        }
    }

    /// A supergroup's or channel's description, and whether you can see
    /// its members, which come a page at a time.
    pub fn of_supergroup(
        group: &types::Supergroup,
        full: Option<&types::SupergroupFullInfo>,
    ) -> Self {
        let roster = match full {
            None => Roster::Hidden("Couldn't get its members"),
            Some(f) if f.has_hidden_members => Roster::Hidden("Its admins hid who's in it"),
            Some(f) if !f.can_get_members => Roster::Hidden("Only its admins can see who's in it"),
            Some(_) => Roster::Paged {
                supergroup_id: group.id,
            },
        };
        let member_count = full
            .map(|f| f.member_count)
            .filter(|&n| n > 0)
            .unwrap_or(group.member_count);
        Self {
            phone: String::new(),
            bio: full.map_or(String::new(), |f| text::clean(f.description.trim())),
            common_groups: 0,
            member_count,
            roster,
        }
    }
}

/// A chat's members, as far as you can see them.
#[derive(Default)]
pub enum Roster {
    /// A chat with one person has none to list.
    #[default]
    None,
    /// All of them: a basic group's come with its info.
    All(Vec<Member>),
    /// A supergroup's or channel's, asked for a page at a time.
    Paged { supergroup_id: i64 },
    /// You can't see them, and why.
    Hidden(&'static str),
}

/// What someone is in a group, in the order Telegram lists them.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Role {
    Owner,
    Admin,
    Member,
}

/// Someone in a group: a person, or a channel posting in it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Member {
    pub who: Sender,
    pub role: Role,
    /// The title the group gave an admin, on one line; empty if none.
    pub title: String,
}

impl Member {
    /// One of TDLib's members; `None` for someone who left or was banned.
    pub fn of(member: &types::ChatMember) -> Option<Self> {
        let (role, title) = match &member.status {
            ChatMemberStatus::Creator(c) if c.is_member => (Role::Owner, c.custom_title.as_str()),
            ChatMemberStatus::Administrator(a) => (Role::Admin, a.custom_title.as_str()),
            ChatMemberStatus::Member(_) => (Role::Member, ""),
            ChatMemberStatus::Restricted(r) if r.is_member => (Role::Member, ""),
            _ => return None,
        };
        let who = match &member.member_id {
            MessageSender::User(u) => Sender::User(u.user_id),
            MessageSender::Chat(c) => Sender::Chat(c.chat_id),
        };
        Some(Self {
            who,
            role,
            title: one_line(title),
        })
    }
}

/// The `I` popup: about a chat, and its members to go through, `Enter`
/// writing to one.
pub struct ChatInfo {
    pub chat_id: i64,
    /// `None` until TDLib answers.
    pub about: Option<About>,
    /// TDLib couldn't say; why is in the status bar.
    pub failed: bool,
    /// Members loaded so far, in TDLib's order.
    pub members: Vec<Member>,
    /// Where the next page of members starts; `None` once there are no
    /// more to ask for.
    next: Option<i32>,
    /// A page of members was asked for and hasn't come.
    asking: bool,
    /// The member under the cursor, by row: members only ever join the end.
    pub selected: usize,
}

impl ChatInfo {
    pub fn new(chat_id: i64) -> Self {
        Self {
            chat_id,
            about: None,
            failed: false,
            members: Vec::new(),
            next: None,
            asking: false,
            selected: 0,
        }
    }

    /// TDLib's answer about the chat: a basic group's members come with
    /// it; a supergroup's are asked for next.
    pub fn set_about(&mut self, mut about: About) {
        match &mut about.roster {
            Roster::All(members) => self.members = std::mem::take(members),
            Roster::Paged { .. } => self.next = Some(0),
            Roster::None | Roster::Hidden(_) => {}
        }
        self.about = Some(about);
    }

    /// The supergroup to ask, and where from, when a page of members is
    /// wanted: the first, or the next once the cursor nears the end. It's
    /// counted as asked for.
    pub fn page_to_ask(&mut self) -> Option<(i64, i32)> {
        let Some(Roster::Paged { supergroup_id }) = self.about.as_ref().map(|a| &a.roster) else {
            return None;
        };
        let offset = self.next?;
        let near_end = self.selected + LOAD_AHEAD >= self.members.len();
        if self.asking || !near_end {
            return None;
        }
        self.asking = true;
        Some((*supergroup_id, offset))
    }

    /// A page of members from `offset` on; `None` if TDLib couldn't send
    /// it. A page for another offset is from before, and dropped.
    pub fn add_page(&mut self, offset: i32, page: Option<Vec<Member>>) {
        if self.next != Some(offset) || !self.asking {
            return;
        }
        self.asking = false;
        let Some(page) = page else {
            self.next = None;
            return;
        };
        let count = page.len();
        let before = self.members.len();
        for member in page {
            // Members who came in meanwhile push others into the next page.
            if !self.members.iter().any(|m| m.who == member.who) {
                self.members.push(member);
            }
        }
        // A page that adds nobody is the end: TDLib's own limit, or a group
        // whose members keep moving.
        let more = self.members.len() > before && self.members.len() < MAX_MEMBERS;
        self.members.truncate(MAX_MEMBERS);
        self.next = more.then(|| offset + count as i32);
    }

    /// Members are still to come: being asked for, or not yet.
    pub fn loading_members(&self) -> bool {
        (self.about.is_none() && !self.failed) || self.asking
    }

    pub fn move_cursor(&mut self, delta: isize) {
        let last = self.members.len().saturating_sub(1);
        self.selected = self.selected.saturating_add_signed(delta).min(last);
    }

    pub fn current(&self) -> Option<&Member> {
        self.members.get(self.selected)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn member(id: i64, role: Role) -> Member {
        Member {
            who: Sender::User(id),
            role,
            title: String::new(),
        }
    }

    fn paged() -> ChatInfo {
        let mut info = ChatInfo::new(-100);
        info.set_about(About {
            member_count: 500,
            roster: Roster::Paged { supergroup_id: 7 },
            ..About::default()
        });
        info
    }

    #[test]
    fn a_supergroups_members_come_a_page_at_a_time_as_the_cursor_nears_the_end() {
        let mut info = paged();
        assert_eq!(info.page_to_ask(), Some((7, 0)));
        assert_eq!(info.page_to_ask(), None, "once until it comes");
        info.add_page(
            0,
            Some((1..=50).map(|id| member(id, Role::Member)).collect()),
        );
        assert_eq!(info.members.len(), 50);
        assert_eq!(info.page_to_ask(), None, "the cursor is far from the end");
        info.move_cursor(40);
        assert_eq!(info.page_to_ask(), Some((7, 50)));

        // Someone in both pages shows once, and a page adding nobody ends it.
        info.add_page(
            50,
            Some(vec![member(50, Role::Member), member(51, Role::Admin)]),
        );
        assert_eq!(info.members.len(), 51);
        assert_eq!(info.page_to_ask(), Some((7, 52)));
        info.add_page(52, Some(vec![member(51, Role::Admin)]));
        assert_eq!(info.page_to_ask(), None);
        assert!(!info.loading_members());
    }

    #[test]
    fn a_page_from_before_or_a_failed_one_adds_nobody() {
        let mut info = paged();
        assert_eq!(info.page_to_ask(), Some((7, 0)));
        info.add_page(200, Some(vec![member(1, Role::Member)]));
        assert!(info.members.is_empty(), "not the page asked for");
        info.add_page(0, None);
        assert!(info.members.is_empty());
        assert_eq!(info.page_to_ask(), None, "not again after an error");
    }

    #[test]
    fn a_basic_groups_members_come_with_it_the_owner_and_admins_first() {
        let mut info = ChatInfo::new(-5);
        info.set_about(About {
            roster: Roster::All(vec![
                member(1, Role::Member),
                member(2, Role::Admin),
                member(3, Role::Owner),
            ]),
            ..About::default()
        });
        // Sorted by `of_basic_group`; here they're taken as they come.
        assert_eq!(info.members.len(), 3);
        assert_eq!(info.page_to_ask(), None);
        info.move_cursor(isize::MAX);
        assert_eq!(info.current().map(|m| m.who), Some(Sender::User(3)));
        info.move_cursor(isize::MIN);
        assert_eq!(info.selected, 0);
    }

    #[test]
    fn people_who_left_or_were_banned_are_not_members() {
        let member = |status| types::ChatMember {
            member_id: MessageSender::User(types::MessageSenderUser { user_id: 1 }),
            inviter_user_id: 0,
            joined_chat_date: 0,
            status,
        };
        assert!(Member::of(&member(ChatMemberStatus::Left)).is_none());
        let admin = ChatMemberStatus::Administrator(types::ChatMemberStatusAdministrator {
            custom_title: "Chief\n\u{202E}cook".into(),
            can_be_edited: false,
            rights: types::ChatAdministratorRights::default(),
        });
        let admin = Member::of(&member(admin)).unwrap();
        assert_eq!(admin.role, Role::Admin);
        assert_eq!(admin.title, "Chief cook", "cleaned, on one line");
    }
}

//! Reactions on messages: what's shown under a bubble, and the `R` popup
//! that adds or takes back yours.
//!
//! Only plain emoji can be picked. Telegram's custom emoji are pictures, not
//! text, so reactions with them show as a stand-in mark, with their count.

use tdlib_rs::enums::{ReactionType, ReactionUnavailabilityReason};
use tdlib_rs::types::{
    AvailableReactions, MessageInteractionInfo, ReactionTypeCustomEmoji, ReactionTypeEmoji,
};
use unicode_width::UnicodeWidthStr;

use crate::text;

/// Emoji per row in the `R` popup. TDLib is asked for the same row size, which
/// decides how many go in its "top" list.
pub const COLUMNS: usize = 8;

/// Joins emoji like ❤‍🔥 or 👨‍💻 into one. Terminals that can't draw the
/// joined emoji draw each part, two columns each, where the layout has room
/// for one, which shifts the rest of the row.
const JOINER: char = '\u{200D}';

/// Asks for an emoji's colored, two-column look.
const EMOJI_STYLE: char = '\u{FE0F}';

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ReactionKind {
    Emoji(String),
    /// A Telegram Premium custom emoji, by its id.
    Custom(i64),
    /// Telegram Stars paid in a channel.
    Paid,
}

impl ReactionKind {
    /// How the reaction looks under a bubble.
    pub fn label(&self) -> String {
        match self {
            ReactionKind::Emoji(emoji) => shown(emoji),
            ReactionKind::Custom(_) => "✦".into(),
            ReactionKind::Paid => "⭐".into(),
        }
    }

    /// The reaction as TDLib takes it; `None` for the paid one, which
    /// can't be added or taken back like the others.
    pub fn to_tdlib(&self) -> Option<ReactionType> {
        match self {
            ReactionKind::Emoji(emoji) => Some(ReactionType::Emoji(ReactionTypeEmoji {
                emoji: emoji.clone(),
            })),
            ReactionKind::Custom(id) => Some(ReactionType::CustomEmoji(ReactionTypeCustomEmoji {
                custom_emoji_id: *id,
            })),
            ReactionKind::Paid => None,
        }
    }
}

/// One kind of reaction on a message, and how many people added it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Reaction {
    pub kind: ReactionKind,
    pub count: i32,
    /// You added it.
    pub chosen: bool,
}

/// The reactions on a message, in Telegram's order (most added first).
pub fn from_info(info: Option<&MessageInteractionInfo>) -> Vec<Reaction> {
    let Some(reactions) = info.and_then(|i| i.reactions.as_ref()) else {
        return Vec::new();
    };
    reactions
        .reactions
        .iter()
        .filter(|r| r.total_count > 0)
        .filter_map(|r| {
            let kind = match &r.r#type {
                ReactionType::Emoji(e) => ReactionKind::Emoji(clean(&e.emoji)?),
                ReactionType::CustomEmoji(e) => ReactionKind::Custom(e.custom_emoji_id),
                ReactionType::Paid => ReactionKind::Paid,
            };
            Some(Reaction {
                kind,
                count: r.total_count,
                chosen: r.is_chosen,
            })
        })
        .collect()
}

/// An emoji from Telegram without anything that could upset the layout:
/// [`text::is_hidden`] characters and white space. `None` if nothing is left.
fn clean(emoji: &str) -> Option<String> {
    let clean: String = emoji
        .chars()
        .filter(|&c| !text::is_hidden(c) && !c.is_whitespace())
        .collect();
    (!clean.is_empty()).then_some(clean)
}

/// The reactions on all the photos of an album, which is drawn as one bubble:
/// counts of the same reaction add up, in the order they first appear.
pub fn merge<'a>(lists: impl IntoIterator<Item = &'a [Reaction]>) -> Vec<Reaction> {
    let mut out: Vec<Reaction> = Vec::new();
    for reaction in lists.into_iter().flatten() {
        match out.iter_mut().find(|r| r.kind == reaction.kind) {
            Some(r) => {
                r.count += reaction.count;
                r.chosen |= reaction.chosen;
            }
            None => out.push(reaction.clone()),
        }
    }
    out
}

/// An emoji as it's drawn. Telegram sends some, like ❤ and ✍, without the
/// mark that asks for their emoji look, so many terminals draw them as narrow
/// symbols and others as two-column emoji; with the mark, all agree on two.
/// Telegram is always sent the emoji as it gave it.
pub fn shown(emoji: &str) -> String {
    let mut chars = emoji.chars();
    match (chars.next(), chars.next()) {
        (Some(c), None) if emoji.width() == 1 => format!("{c}{EMOJI_STYLE}"),
        _ => emoji.to_string(),
    }
}

/// A count as Telegram shortens it: 999, 1K, 1.5K, 12K, 1.2M.
pub fn count_label(n: i32) -> String {
    let (unit, size) = match n {
        ..1000 => return n.to_string(),
        1000..1_000_000 => ("K", 1000),
        _ => ("M", 1_000_000),
    };
    let tenths = n / (size / 10);
    if tenths < 100 && tenths % 10 != 0 {
        format!("{}.{}{unit}", tenths / 10, tenths % 10)
    } else {
        format!("{}{unit}", n / size)
    }
}

/// The emoji's name and GitHub shortcodes, for the search and the popup.
pub fn about(emoji: &str) -> Option<&'static emojis::Emoji> {
    emojis::get(emoji).or_else(|| emojis::get(&format!("{emoji}{EMOJI_STYLE}")))
}

/// How well `emoji` matches a search, best first; `None` if it doesn't.
/// `query` is lowercase, without colons around it.
fn rank(emoji: &str, query: &str) -> Option<u8> {
    if emoji == query {
        return Some(0);
    }
    let found = about(emoji)?;
    let name = found.name().to_lowercase();
    let mut names = std::iter::once(name.as_str()).chain(found.shortcodes());
    if names.clone().any(|n| n == query) {
        Some(0)
    } else if names.clone().any(|n| {
        n.split(|c: char| !c.is_alphanumeric())
            .any(|word| word.starts_with(query))
    }) {
        Some(1)
    } else if names.any(|n| n.contains(query)) {
        Some(2)
    } else {
        None
    }
}

/// The emoji of `choices` whose name or shortcode has `query` in it: exact
/// names first, then ones with a word starting with it, then the rest, each
/// in the order Telegram gave. All of them for an empty query.
pub fn search<'a>(choices: &'a [String], query: &str) -> Vec<&'a str> {
    let query = query.trim().trim_matches(':').to_lowercase();
    if query.is_empty() {
        return choices.iter().map(String::as_str).collect();
    }
    let mut found: Vec<(u8, &str)> = choices
        .iter()
        .filter_map(|e| rank(e, &query).map(|r| (r, e.as_str())))
        .collect();
    found.sort_by_key(|&(r, _)| r);
    found.into_iter().map(|(_, e)| e).collect()
}

/// What `R` can offer, from TDLib's `getMessageAvailableReactions`.
pub struct Available {
    /// Plain emoji you can add, in Telegram's order: top, then recently
    /// used, then popular.
    pub emoji: Vec<String>,
    /// Why you can't react here although others can.
    pub reason: Option<&'static str>,
}

impl From<AvailableReactions> for Available {
    fn from(r: AvailableReactions) -> Self {
        let mut emoji: Vec<String> = Vec::new();
        let lists = [r.top_reactions, r.recent_reactions, r.popular_reactions];
        for reaction in lists.into_iter().flatten() {
            // Custom emoji can't be drawn, and the paid one costs Stars.
            if let ReactionType::Emoji(e) = reaction.r#type
                && !reaction.needs_premium
                && let Some(e) = clean(&e.emoji)
                && !emoji.contains(&e)
            {
                emoji.push(e);
            }
        }
        let reason = r.unavailability_reason.map(|reason| match reason {
            ReactionUnavailabilityReason::AnonymousAdministrator => {
                "You post anonymously here, so you can't react"
            }
            ReactionUnavailabilityReason::Guest => "Join the chat to react",
        });
        Self { emoji, reason }
    }
}

/// The `R` popup: a grid of emoji for the message under the cursor. Enter
/// adds the one under the cursor, or takes it back if you already had.
pub struct ReactMenu {
    pub message_id: i64,
    /// The message on one line, so it's clear which one gets the reaction.
    pub snippet: String,
    /// What can be picked; `None` until TDLib answers.
    pub choices: Option<Vec<String>>,
    /// What's typed after `/`; `None` when not searching.
    pub query: Option<String>,
    /// Index into [`ReactMenu::shown`].
    pub selected: usize,
    /// Where the cursor starts in the grid: on your reaction, so `R` then
    /// Enter takes it back.
    start: usize,
    /// First row of the grid shown. Drawing keeps the cursor's row in view.
    pub scroll: usize,
}

impl ReactMenu {
    pub fn new(message_id: i64, snippet: String) -> Self {
        Self {
            message_id,
            snippet,
            choices: None,
            query: None,
            selected: 0,
            start: 0,
            scroll: 0,
        }
    }

    /// Fills in what TDLib offers, given the emoji you already put on the
    /// message, and puts the cursor on the first of yours. Joined emoji are
    /// left out (see [`JOINER`]), unless they're yours, so they can still be
    /// taken back.
    pub fn set_choices(&mut self, emoji: Vec<String>, yours: &[String]) {
        let choices: Vec<String> = emoji
            .into_iter()
            .filter(|e| !e.contains(JOINER) || yours.contains(e))
            .collect();
        self.start = choices.iter().position(|e| yours.contains(e)).unwrap_or(0);
        self.selected = self.start;
        self.choices = Some(choices);
    }

    /// Back from the search to the whole grid, with the cursor where it
    /// started.
    pub fn leave_search(&mut self) {
        self.query = None;
        self.selected = self.start;
    }

    /// The emoji in the grid: all of them, or the ones the search finds.
    pub fn shown(&self) -> Vec<&str> {
        let choices = self.choices.as_deref().unwrap_or_default();
        search(choices, self.query.as_deref().unwrap_or_default())
    }

    /// The emoji under the cursor.
    pub fn current(&self) -> Option<&str> {
        self.shown().get(self.selected).copied()
    }

    /// Moves the cursor `delta` places through the grid, stopping at its ends.
    pub fn move_by(&mut self, delta: isize) {
        let last = self.shown().len().saturating_sub(1);
        self.selected = self.selected.saturating_add_signed(delta).min(last);
    }

    /// Changes the search, and starts again from the best match.
    pub fn edit_query(&mut self, edit: impl FnOnce(&mut String)) {
        edit(self.query.get_or_insert_default());
        self.selected = 0;
    }
}

#[cfg(test)]
mod tests {
    use tdlib_rs::types::{AvailableReaction, MessageReaction, MessageReactions};

    use super::*;

    fn emoji(e: &str) -> ReactionType {
        ReactionType::Emoji(ReactionTypeEmoji { emoji: e.into() })
    }

    fn reaction(kind: ReactionType, count: i32, chosen: bool) -> MessageReaction {
        MessageReaction {
            r#type: kind,
            total_count: count,
            is_chosen: chosen,
            used_sender_id: None,
            recent_sender_ids: Vec::new(),
        }
    }

    fn strings(list: &[&str]) -> Vec<String> {
        list.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn reactions_keep_telegram_order_and_stand_ins_for_what_isnt_text() {
        let info = MessageInteractionInfo {
            view_count: 0,
            forward_count: 0,
            reply_info: None,
            reactions: Some(MessageReactions {
                reactions: vec![
                    reaction(emoji("👍"), 3, true),
                    reaction(
                        ReactionType::CustomEmoji(ReactionTypeCustomEmoji { custom_emoji_id: 7 }),
                        2,
                        false,
                    ),
                    reaction(ReactionType::Paid, 1, false),
                    reaction(emoji("\u{202E}❤\n"), 1, false),
                    reaction(emoji("🔥"), 0, false),
                ],
                are_tags: false,
                paid_reactors: Vec::new(),
                can_get_added_reactions: false,
            }),
        };
        let reactions = from_info(Some(&info));
        let labels: Vec<_> = reactions
            .iter()
            .map(|r| (r.kind.label(), r.count, r.chosen))
            .collect();
        assert_eq!(
            labels,
            [
                ("👍".to_string(), 3, true),
                ("✦".into(), 2, false),
                ("⭐".into(), 1, false),
                ("❤\u{FE0F}".into(), 1, false),
            ]
        );
        assert_eq!(reactions[3].kind, ReactionKind::Emoji("❤".into()));
        assert!(from_info(None).is_empty());
    }

    #[test]
    fn narrow_emoji_are_drawn_two_columns_wide_like_the_rest() {
        for e in ["❤", "✍", "☃", "🕊"] {
            assert_eq!(shown(e).width(), 2, "{e}");
        }
        assert_eq!(shown("👍"), "👍");
        assert_eq!(shown("❤️"), "❤️");
        assert_eq!(shown("👨‍💻"), "👨‍💻");
    }

    #[test]
    fn counts_shorten_like_telegram() {
        let labels: Vec<_> = [7, 999, 1000, 1500, 9999, 12_345, 999_999, 1_200_000]
            .map(count_label)
            .into();
        assert_eq!(
            labels,
            ["7", "999", "1K", "1.5K", "9.9K", "12K", "999K", "1.2M"]
        );
    }

    #[test]
    fn an_album_adds_up_the_reactions_of_its_photos() {
        let heart = ReactionKind::Emoji("❤".into());
        let fire = ReactionKind::Emoji("🔥".into());
        let first = [Reaction {
            kind: heart.clone(),
            count: 2,
            chosen: false,
        }];
        let second = [
            Reaction {
                kind: fire.clone(),
                count: 1,
                chosen: false,
            },
            Reaction {
                kind: heart.clone(),
                count: 1,
                chosen: true,
            },
        ];
        let merged = merge([first.as_slice(), second.as_slice()]);
        assert_eq!(
            merged,
            [
                Reaction {
                    kind: heart,
                    count: 3,
                    chosen: true
                },
                Reaction {
                    kind: fire,
                    count: 1,
                    chosen: false
                },
            ]
        );
    }

    #[test]
    fn search_finds_emoji_by_name_or_shortcode_best_match_first() {
        let choices = strings(&["👍", "❤", "🔥", "💔", "😍", "❤‍🔥", "💯", "🆒"]);
        assert_eq!(search(&choices, "fire"), ["🔥", "❤‍🔥"]);
        assert_eq!(search(&choices, "heart"), ["❤", "💔", "😍", "❤‍🔥"]);
        assert_eq!(search(&choices, ":+1:"), ["👍"]);
        assert_eq!(search(&choices, "Thumbs"), ["👍"]);
        assert_eq!(search(&choices, "100"), ["💯"]);
        assert_eq!(search(&choices, "cool"), ["🆒"]);
        assert!(search(&choices, "zebra").is_empty());
        assert_eq!(search(&choices, " ").len(), choices.len());
    }

    #[test]
    fn the_popup_offers_plain_emoji_once_each_and_says_why_it_cant() {
        let available = |kind, needs_premium| AvailableReaction {
            r#type: kind,
            needs_premium,
        };
        let r = AvailableReactions {
            top_reactions: vec![available(emoji("👍"), false), available(emoji("❤"), false)],
            recent_reactions: vec![
                available(emoji("👍"), false),
                available(
                    ReactionType::CustomEmoji(ReactionTypeCustomEmoji { custom_emoji_id: 1 }),
                    false,
                ),
            ],
            popular_reactions: vec![
                available(emoji("🔥"), false),
                available(emoji("🐳"), true),
                available(ReactionType::Paid, false),
            ],
            allow_custom_emoji: false,
            are_tags: false,
            unavailability_reason: Some(ReactionUnavailabilityReason::Guest),
        };
        let available = Available::from(r);
        assert_eq!(available.emoji, ["👍", "❤", "🔥"]);
        assert_eq!(available.reason, Some("Join the chat to react"));
    }

    #[test]
    fn joined_emoji_are_left_out_of_the_popup_unless_theyre_yours() {
        let mut menu = ReactMenu::new(1, "hi".into());
        menu.set_choices(strings(&["👍", "❤‍🔥", "👨‍💻"]), &strings(&["👨‍💻"]));
        assert_eq!(menu.shown(), ["👍", "👨‍💻"]);
    }

    #[test]
    fn the_cursor_starts_on_your_reaction_and_goes_back_there_after_a_search() {
        let mut menu = ReactMenu::new(1, "hi".into());
        menu.set_choices(strings(&["👍", "❤", "🔥"]), &strings(&["🔥", "❤"]));
        assert_eq!(menu.current(), Some("❤"), "the first of yours in the grid");
        menu.edit_query(|q| q.push_str("thumbs"));
        assert_eq!(menu.current(), Some("👍"));
        menu.leave_search();
        assert_eq!(menu.current(), Some("❤"));

        menu.set_choices(strings(&["👍", "❤"]), &[]);
        assert_eq!(menu.current(), Some("👍"), "none of yours: the top");
    }

    #[test]
    fn every_reaction_but_the_paid_one_can_be_taken_back() {
        let emoji = ReactionKind::Emoji("👍".into()).to_tdlib();
        assert!(matches!(emoji, Some(ReactionType::Emoji(e)) if e.emoji == "👍"));
        let custom = ReactionKind::Custom(7).to_tdlib();
        assert!(matches!(custom, Some(ReactionType::CustomEmoji(e)) if e.custom_emoji_id == 7));
        assert!(ReactionKind::Paid.to_tdlib().is_none());
    }

    #[test]
    fn the_cursor_stays_in_the_grid_and_a_new_search_starts_at_the_top() {
        let mut menu = ReactMenu::new(1, "hi".into());
        assert_eq!(menu.current(), None);
        menu.set_choices(strings(&["👍", "❤", "🔥", "💔"]), &[]);
        menu.move_by(COLUMNS as isize);
        assert_eq!(menu.current(), Some("💔"));
        menu.move_by(-(COLUMNS as isize));
        assert_eq!(menu.current(), Some("👍"));
        menu.move_by(2);
        menu.edit_query(|q| q.push_str("hea"));
        assert_eq!(menu.shown(), ["❤", "💔"]);
        assert_eq!(menu.current(), Some("❤"));
        menu.edit_query(|q| q.push('z'));
        assert_eq!(menu.current(), None);
    }
}

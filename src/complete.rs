//! Completing the word being typed in the composer: `@` and a name mentions
//! someone in the group, `:` and a few letters puts in an emoji by its
//! shortcode. Tab takes the suggestion under the cursor.

use std::time::Duration;

use tokio::time::Instant;

/// How long typing has to pause before the group's members are searched.
pub const SEARCH_AFTER: Duration = Duration::from_millis(300);
/// Suggestions listed at once.
pub const MAX_SUGGESTIONS: usize = 6;
/// Letters after `:` before emoji are suggested: one finds too many.
const MIN_EMOJI_CHARS: usize = 2;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    /// `@name`.
    Mention,
    /// `:shortcode`.
    Emoji,
}

/// The word before the cursor that can be completed.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Word {
    pub kind: Kind,
    /// What's typed after the `@` or `:`.
    pub query: String,
    /// Characters it takes, with its `@` or `:`, which Tab replaces.
    pub chars: usize,
}

/// The word that ends at character `col` of `line`, if it's one to
/// complete: `@` with a name or nothing yet, or `:` with at least two
/// letters of a shortcode. It has to start the line or follow a space, so
/// `user@mail` and `10:30` are left alone, and the cursor has to be at its
/// end.
pub fn word_at(line: &str, col: usize) -> Option<Word> {
    let chars: Vec<char> = line.chars().collect();
    if col > chars.len() || chars.get(col).is_some_and(|c| !c.is_whitespace()) {
        return None;
    }
    let start = chars[..col]
        .iter()
        .rposition(|c| c.is_whitespace())
        .map_or(0, |i| i + 1);
    let word: String = chars[start..col].iter().collect();
    let chars = col - start;
    if let Some(query) = word.strip_prefix('@')
        && query.chars().all(|c| c.is_alphanumeric() || c == '_')
    {
        return Some(Word {
            kind: Kind::Mention,
            query: query.to_string(),
            chars,
        });
    }
    let query = word.strip_prefix(':')?;
    let shortcode = query.chars().count() >= MIN_EMOJI_CHARS
        && query.starts_with(|c: char| c.is_ascii_alphabetic() || c == '+' || c == '-')
        && query
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '+' | '-'));
    shortcode.then(|| Word {
        kind: Kind::Emoji,
        query: query.to_ascii_lowercase(),
        chars,
    })
}

/// One row of suggestions.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Suggestion {
    pub label: String,
    /// A quieter note after it: a username, a shortcode.
    pub detail: String,
    /// What Tab puts in place of the word.
    pub insert: String,
}

/// Emoji whose shortcode has the query in it: ones it starts first, then
/// ones with a word starting with it, then the rest, shortest first.
/// Joined emoji (👨‍💻) are left out, since terminals disagree on their width.
pub fn emoji(query: &str) -> Vec<Suggestion> {
    let mut found: Vec<(u8, usize, &str, &str)> = Vec::new();
    for emoji in emojis::iter() {
        if emoji.as_str().contains('\u{200D}') {
            continue;
        }
        let best = emoji
            .shortcodes()
            .filter_map(|code| {
                let rank = if code.starts_with(query) {
                    0
                } else if code.split('_').any(|word| word.starts_with(query)) {
                    1
                } else if code.contains(query) {
                    2
                } else {
                    return None;
                };
                Some((rank, code.len(), code))
            })
            .min();
        if let Some((rank, len, code)) = best {
            found.push((rank, len, code, emoji.as_str()));
        }
    }
    found.sort();
    found
        .into_iter()
        .take(MAX_SUGGESTIONS)
        .map(|(_, _, code, emoji)| Suggestion {
            label: emoji.to_string(),
            detail: format!(":{code}:"),
            insert: emoji.to_string(),
        })
        .collect()
}

/// Someone to mention: by username if they have one, else by name with a
/// link to their account, which is sent as a mention.
pub fn mention(user_id: i64, name: &str, username: Option<&str>) -> Suggestion {
    match username {
        Some(username) => Suggestion {
            label: name.to_string(),
            detail: format!("@{username}"),
            insert: format!("@{username} "),
        },
        None => Suggestion {
            label: name.to_string(),
            detail: String::new(),
            // Brackets in the name would end the link early.
            insert: format!(
                "[{}](tg://user?id={user_id}) ",
                name.replace(['[', ']'], "")
            ),
        },
    }
}

/// Suggestions for the word being typed.
pub struct Completion {
    pub word: Word,
    pub items: Vec<Suggestion>,
    pub selected: usize,
    /// When to search the group's members, once typing pauses.
    search_at: Option<Instant>,
    /// Members Telegram found, and what for.
    pub found: Vec<i64>,
    found_for: Option<String>,
}

impl Completion {
    pub fn new(word: Word) -> Self {
        Self {
            word,
            items: Vec::new(),
            selected: 0,
            search_at: None,
            found: Vec::new(),
            found_for: None,
        }
    }

    /// A new word was typed: back to the top, with members searched once
    /// typing pauses. What was found for the word before stays until then,
    /// narrowed by the new one.
    pub fn retype(&mut self, word: Word, now: Instant) {
        if word.kind == Kind::Mention && self.found_for.as_deref() != Some(&word.query) {
            self.search_at = Some(now + SEARCH_AFTER);
        }
        self.word = word;
        self.selected = 0;
    }

    pub fn search_at(&self) -> Option<Instant> {
        self.search_at
    }

    /// The name to search the group's members for, once typing has paused.
    pub fn due_search(&mut self, now: Instant) -> Option<String> {
        self.search_at.filter(|&at| at <= now)?;
        self.search_at = None;
        self.found_for = Some(self.word.query.clone());
        Some(self.word.query.clone())
    }

    /// Members Telegram found for `query`, unless the word changed since.
    pub fn set_found(&mut self, query: &str, user_ids: Vec<i64>) -> bool {
        if self.found_for.as_deref() != Some(query) {
            return false;
        }
        self.found = user_ids;
        true
    }

    /// New suggestions, keeping the cursor among them.
    pub fn set_items(&mut self, items: Vec<Suggestion>) {
        self.selected = self.selected.min(items.len().saturating_sub(1));
        self.items = items;
    }

    pub fn move_by(&mut self, delta: isize) {
        let last = self.items.len().saturating_sub(1);
        self.selected = self.selected.saturating_add_signed(delta).min(last);
    }

    pub fn current(&self) -> Option<&Suggestion> {
        self.items.get(self.selected)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_a_word_starting_with_at_or_colon_at_the_cursor_is_completed() {
        let word = |line: &str| word_at(line, line.chars().count());
        let mention = |query: &str, chars| {
            Some(Word {
                kind: Kind::Mention,
                query: query.into(),
                chars,
            })
        };
        assert_eq!(word("hi @al"), mention("al", 3));
        assert_eq!(word("@"), mention("", 1), "everyone, before a letter");
        assert_eq!(word("hi @José"), mention("José", 5));
        assert_eq!(word("mail me@home"), None);
        assert_eq!(word("hi @al bob"), None, "the cursor is past it");
        assert_eq!(word_at("hi @al bob", 6), mention("al", 3));
        assert_eq!(word_at("hi @alice", 6), None, "the cursor is inside it");

        assert_eq!(
            word("so :Smi"),
            Some(Word {
                kind: Kind::Emoji,
                query: "smi".into(),
                chars: 4,
            })
        );
        assert!(word("ok :+1").is_some());
        assert_eq!(word("so :s"), None, "one letter finds too many");
        assert_eq!(word("at 10:30"), None);
        assert_eq!(word("so :30"), None);
        assert_eq!(word("so :)"), None);
    }

    #[test]
    fn emoji_whose_shortcode_starts_with_the_query_come_first() {
        let found = emoji("smile");
        assert_eq!(found[0].label, "😄");
        assert_eq!(found[0].detail, ":smile:");
        assert!(found.len() <= MAX_SUGGESTIONS);
        assert!(found.iter().all(|s| !s.insert.contains('\u{200D}')));
        assert_eq!(emoji("+1")[0].insert, "👍");
        assert!(emoji("zzzzqq").is_empty());
    }

    #[test]
    fn people_without_a_username_are_mentioned_by_a_link_to_them() {
        assert_eq!(mention(5, "Ann", Some("ann")).insert, "@ann ");
        assert_eq!(
            mention(7, "Bob [admin]", None).insert,
            "[Bob admin](tg://user?id=7) "
        );
    }

    #[test]
    fn members_are_searched_once_typing_pauses_and_late_answers_dropped() {
        let start = Instant::now();
        let word = |query: &str| Word {
            kind: Kind::Mention,
            query: query.into(),
            chars: query.len() + 1,
        };
        let mut completion = Completion::new(word(""));
        completion.retype(word("a"), start);
        completion.retype(word("al"), start);
        assert_eq!(completion.due_search(start), None);
        assert_eq!(
            completion.due_search(start + SEARCH_AFTER).as_deref(),
            Some("al")
        );
        completion.retype(word("ali"), start + SEARCH_AFTER);
        assert!(!completion.set_found("ali", vec![1]), "not asked yet");
        assert!(completion.set_found("al", vec![1]), "what was asked");
        assert_eq!(completion.found, [1]);
    }
}

//! `/` search: matching text against a query, and the state of a message
//! search in the open chat, with the filters it can take (`from:@alice
//! has:photo before:2025-10-01`).

use std::ops::Range;

use chrono::{Local, NaiveDate, TimeZone};
use tdlib_rs::enums::SearchMessagesFilter;

use crate::messages::Sender;
use crate::tg::Found;

/// What a `/` search in a chat asks TDLib for: its words, and the filters
/// typed with them.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Query {
    /// The words to find, without the filters; what's highlighted.
    pub words: String,
    /// `from:`: who sent it.
    pub from: Option<Who>,
    /// `has:`: what's in it.
    pub has: Option<Has>,
    /// `before:`: sent before this unix time, the start of the day typed.
    pub before: Option<i32>,
    /// `after:`: sent at this unix time or later, the start of the day
    /// typed, so that day counts.
    pub after: Option<i32>,
}

/// Who `from:` means. The app turns `Me` and `Name` into a sender, and a
/// username it knows too; TDLib looks up the rest.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Who {
    Me,
    /// `from:Alice`: someone whose name has this in it, among the senders
    /// of the messages loaded.
    Name(String),
    /// `from:@alice`, without the @.
    Username(String),
    Sender(Sender),
}

/// What `has:` looks for.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Has {
    Photo,
    Video,
    /// Photos and videos.
    Media,
    File,
    Link,
    Voice,
    Gif,
    Audio,
}

impl Has {
    /// Every kind, by the word `has:` takes.
    pub const ALL: [(&'static str, Has); 8] = [
        ("photo", Has::Photo),
        ("video", Has::Video),
        ("media", Has::Media),
        ("file", Has::File),
        ("link", Has::Link),
        ("voice", Has::Voice),
        ("gif", Has::Gif),
        ("audio", Has::Audio),
    ];

    pub fn tdlib(self) -> SearchMessagesFilter {
        match self {
            Has::Photo => SearchMessagesFilter::Photo,
            Has::Video => SearchMessagesFilter::Video,
            Has::Media => SearchMessagesFilter::PhotoAndVideo,
            Has::File => SearchMessagesFilter::Document,
            Has::Link => SearchMessagesFilter::Url,
            Has::Voice => SearchMessagesFilter::VoiceNote,
            Has::Gif => SearchMessagesFilter::Animation,
            Has::Audio => SearchMessagesFilter::Audio,
        }
    }
}

/// The filters a search takes, as typed.
pub const FILTERS: [&str; 4] = ["from:", "has:", "before:", "after:"];

/// Reads a `/` search typed in a chat: filters anywhere among the words,
/// each at most once. Says what's wrong with one that can't be read.
pub fn parse(typed: &str) -> Result<Query, String> {
    let mut query = Query::default();
    let mut words = Vec::new();
    let mut seen = Vec::new();
    for token in typed.split_whitespace() {
        let lower = token.to_lowercase();
        let Some(key) = FILTERS.into_iter().find(|f| lower.starts_with(f)) else {
            words.push(token);
            continue;
        };
        if seen.contains(&key) {
            return Err(format!("Only one {key} at a time"));
        }
        seen.push(key);
        let value = &token[key.len()..];
        match key {
            "from:" => query.from = Some(who(value)?),
            "has:" => {
                let kind = Has::ALL
                    .into_iter()
                    .find(|(name, _)| value.eq_ignore_ascii_case(name))
                    .ok_or("has: takes photo, video, media, file, link, voice, gif or audio")?;
                query.has = Some(kind.1);
            }
            "before:" => query.before = Some(day_start(value)?),
            _ => query.after = Some(day_start(value)?),
        }
    }
    query.words = words.join(" ");
    if query.words.is_empty() && query.from.is_none() && query.has.is_none() {
        return Err("Add words, from: or has: to search for".into());
    }
    if let (Some(before), Some(after)) = (query.before, query.after)
        && after >= before
    {
        return Err("after: has to be a day before before:".into());
    }
    Ok(query)
}

fn who(value: &str) -> Result<Who, String> {
    match value {
        "" | "@" => Err("from: takes me, @username or a name, like from:@alice".into()),
        _ if value.eq_ignore_ascii_case("me") => Ok(Who::Me),
        _ => Ok(match value.strip_prefix('@') {
            Some(name) => Who::Username(name.to_string()),
            None => Who::Name(value.to_string()),
        }),
    }
}

/// The start of a day typed like 2025-10-01, in local time, as a unix time.
fn day_start(value: &str) -> Result<i32, String> {
    let wrong = || format!("Dates are like 2025-10-01, not \"{value}\"");
    let day = NaiveDate::parse_from_str(value, "%Y-%m-%d").map_err(|_| wrong())?;
    let start = Local
        .from_local_datetime(&day.and_hms_opt(0, 0, 0).ok_or_else(wrong)?)
        .earliest()
        .ok_or_else(wrong)?;
    i32::try_from(start.timestamp()).map_err(|_| wrong())
}

/// What Tab can put in place of the last word of a search typed so far,
/// each as the whole search: a filter's name, what `has:` takes, one of
/// `people` (`me`, `@username`s) after `from:`, or `today` after
/// `before:` and `after:`.
pub fn complete(typed: &str, people: &[String], today: &str) -> Vec<String> {
    // After the last space, which may be wider than a byte (a no-break
    // space).
    let start = typed
        .char_indices()
        .rev()
        .find(|(_, c)| c.is_whitespace())
        .map_or(0, |(i, c)| i + c.len_utf8());
    let (head, word) = typed.split_at(start);
    let lower = word.to_lowercase();
    let with = |key: &str, values: Vec<&str>| -> Vec<String> {
        let typed = lower[key.len()..].to_string();
        values
            .into_iter()
            .filter(|v| v.to_lowercase().starts_with(&typed))
            .map(|v| format!("{head}{key}{v}"))
            .collect()
    };
    match FILTERS.into_iter().find(|f| lower.starts_with(f)) {
        Some("has:") => with("has:", Has::ALL.iter().map(|(name, _)| *name).collect()),
        Some("from:") => with("from:", people.iter().map(String::as_str).collect()),
        Some(key) if lower == key => vec![format!("{head}{key}{today}")],
        Some(_) => Vec::new(),
        None => FILTERS
            .into_iter()
            .filter(|f| f.starts_with(&lower))
            .map(|f| format!("{head}{f}"))
            .collect(),
    }
}

/// Byte ranges of `text` where `query` appears, ignoring case. In order and
/// non-overlapping. An empty query matches nothing.
pub fn find(text: &str, query: &str) -> Vec<Range<usize>> {
    let query: Vec<char> = query.chars().flat_map(char::to_lowercase).collect();
    let mut out = Vec::new();
    if query.is_empty() {
        return out;
    }
    let mut next_free = 0;
    for (start, _) in text.char_indices() {
        if start < next_free {
            continue;
        }
        if let Some(end) = match_at(text, start, &query) {
            out.push(start..end);
            next_free = end;
        }
    }
    out
}

/// Where a match of the (lowercased) query starting at byte `start` ends.
fn match_at(text: &str, start: usize, query: &[char]) -> Option<usize> {
    let mut want = query.iter();
    let mut next = want.next();
    for (i, c) in text[start..].char_indices() {
        // Some characters lowercase to several, e.g. 'İ'.
        for lower in c.to_lowercase() {
            match next {
                Some(&q) if q == lower => next = want.next(),
                Some(_) => return None,
                None => break,
            }
        }
        if next.is_none() {
            return Some(start + i + c.len_utf8());
        }
    }
    None
}

/// A `/` search in the open chat. TDLib searches the whole history and
/// returns matches newest first, a page at a time.
pub struct MessageSearch {
    /// The search as typed, filters and all, for the pane's title and to
    /// match TDLib's answers.
    pub query: String,
    /// What TDLib is asked for.
    pub ask: Query,
    /// Matching message ids, newest first, as far as fetched.
    pub results: Vec<i64>,
    /// Index into `results` of the match the cursor was sent to.
    pub current: Option<usize>,
    /// TDLib's estimate of how many messages match; -1 if it doesn't know.
    total: i32,
    /// Where the next page of results starts.
    pub next_from: i64,
    /// Or this, for words found in a secret chat.
    pub next_offset: String,
    /// Every match has been fetched.
    pub done: bool,
    /// A page of results is being fetched.
    pub loading: bool,
    /// The match to go to once the page being fetched arrives.
    pub wanted: Option<usize>,
}

impl MessageSearch {
    pub fn new(query: String, ask: Query) -> Self {
        Self {
            query,
            ask,
            results: Vec::new(),
            current: None,
            total: -1,
            next_from: 0,
            next_offset: String::new(),
            done: false,
            loading: false,
            wanted: None,
        }
    }

    pub fn add(&mut self, found: Found) {
        for id in found.ids {
            if !self.results.contains(&id) {
                self.results.push(id);
            }
        }
        self.total = found.total;
        self.next_from = found.next_from;
        self.next_offset = found.next_offset;
        // Neither means no more pages.
        self.done = self.next_from == 0 && self.next_offset.is_empty();
    }

    /// "3 of 41" for the pane title, or "searching…" before the first match.
    pub fn position(&self) -> String {
        match self.current {
            Some(i) => {
                let total = (self.total.max(0) as usize).max(self.results.len());
                format!("{} of {total}", i + 1)
            }
            None => "searching…".into(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn found<'a>(text: &'a str, query: &str) -> Vec<&'a str> {
        find(text, query).into_iter().map(|r| &text[r]).collect()
    }

    #[test]
    fn matches_ignore_case_and_keep_the_original_text() {
        assert_eq!(
            found("Hello hello HELLO", "hello"),
            ["Hello", "hello", "HELLO"]
        );
        assert_eq!(found("Café au lait", "CAFÉ"), ["Café"]);
        assert_eq!(found("aaaa", "aa"), ["aa", "aa"], "no overlaps");
        assert!(found("abc", "").is_empty());
        assert!(found("abc", "abcd").is_empty());
    }

    #[test]
    fn ranges_are_byte_ranges_around_wide_characters() {
        let text = "🎉 Đà Nẵng 🎉";
        assert_eq!(found(text, "nẵng"), ["Nẵng"]);
        // 'İ' lowercases to two characters; the match still ends on a char boundary.
        assert_eq!(found("İstanbul", "i̇st"), ["İst"]);
    }

    #[test]
    fn filters_mix_with_the_words_in_any_order() {
        let query = parse("trail FROM:@Maya has:Photo before:2025-10-01 map").unwrap();
        assert_eq!(query.words, "trail map");
        assert_eq!(query.from, Some(Who::Username("Maya".into())));
        assert_eq!(query.has, Some(Has::Photo));
        let day = |s| day_start(s).unwrap();
        assert_eq!(query.before, Some(day("2025-10-01")));
        assert_eq!(day("2025-10-02") - day("2025-10-01"), 86_400);

        assert_eq!(parse("from:me").unwrap().from, Some(Who::Me));
        assert_eq!(
            parse("from:Alice").unwrap().from,
            Some(Who::Name("Alice".into()))
        );
        let both = parse("has:media after:2025-09-01 before:2025-10-01").unwrap();
        assert_eq!(both.after, Some(day("2025-09-01")));
        assert_eq!(
            parse("12:30 http://x.com").unwrap().words,
            "12:30 http://x.com"
        );
    }

    #[test]
    fn a_filter_that_cant_be_read_says_why() {
        let why = |typed| parse(typed).unwrap_err();
        assert_eq!(why("has:photo has:video"), "Only one has: at a time");
        assert!(why("has:sticker").starts_with("has: takes photo"));
        assert!(why("from: hi").starts_with("from: takes me"));
        assert_eq!(
            why("hi before:10/01/2025"),
            "Dates are like 2025-10-01, not \"10/01/2025\""
        );
        assert_eq!(
            why("before:2025-10-01"),
            "Add words, from: or has: to search for"
        );
        assert!(why("hi after:2025-10-01 before:2025-10-01").starts_with("after: has to be"));
    }

    #[test]
    fn tab_finishes_the_last_word_with_a_filter_or_what_it_takes() {
        let people = ["me".to_string(), "@maya".to_string(), "@leo".to_string()];
        let tab = |typed| complete(typed, &people, "2026-10-07");
        assert_eq!(tab(""), ["from:", "has:", "before:", "after:"]);
        assert_eq!(tab("trail h"), ["trail has:"]);
        assert_eq!(tab("has:v"), ["has:video", "has:voice"]);
        assert_eq!(tab("from:"), ["from:me", "from:@maya", "from:@leo"]);
        assert_eq!(tab("trail from:@M"), ["trail from:@maya"]);
        assert_eq!(tab("before:"), ["before:2026-10-07"]);
        assert!(tab("before:2025").is_empty());
        assert!(tab("trail").is_empty());
        assert_eq!(
            tab("trail\u{a0}h"),
            ["trail\u{a0}has:"],
            "after a no-break space"
        );
    }

    #[test]
    fn results_pile_up_without_duplicates() {
        let mut search = MessageSearch::new("hi".into(), parse("hi").unwrap());
        assert_eq!(search.position(), "searching…");
        search.add(Found {
            ids: vec![9, 7],
            total: 3,
            next_from: 7,
            next_offset: String::new(),
        });
        assert!(!search.done);
        search.add(Found {
            ids: vec![7, 2],
            total: 3,
            next_from: 0,
            next_offset: String::new(),
        });
        assert_eq!(search.results, [9, 7, 2]);
        assert!(search.done);
        search.current = Some(1);
        assert_eq!(search.position(), "2 of 3");
    }

    #[test]
    fn a_secret_chats_search_goes_on_from_where_tdlib_says() {
        let mut search = MessageSearch::new("hi".into(), parse("hi").unwrap());
        search.add(Found {
            ids: vec![9, 7],
            total: 3,
            next_from: 0,
            next_offset: "7".into(),
        });
        assert!(!search.done, "an offset to go on from");
        assert_eq!(search.next_offset, "7");
        search.add(Found {
            ids: vec![2],
            total: 3,
            next_from: 0,
            next_offset: String::new(),
        });
        assert!(search.done);
    }
}

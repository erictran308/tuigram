//! `/` search: matching text against a query, and the state of a message
//! search in the open chat.

use std::ops::Range;

use crate::tg::Found;

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
    pub query: String,
    /// Matching message ids, newest first, as far as fetched.
    pub results: Vec<i64>,
    /// Index into `results` of the match the cursor was sent to.
    pub current: Option<usize>,
    /// TDLib's estimate of how many messages match; -1 if it doesn't know.
    total: i32,
    /// Where the next page of results starts.
    pub next_from: i64,
    /// Every match has been fetched.
    pub done: bool,
    /// A page of results is being fetched.
    pub loading: bool,
    /// The match to go to once the page being fetched arrives.
    pub wanted: Option<usize>,
}

impl MessageSearch {
    pub fn new(query: String) -> Self {
        Self {
            query,
            results: Vec::new(),
            current: None,
            total: -1,
            next_from: 0,
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
        // 0 means no more pages.
        self.done = found.next_from == 0;
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
    fn results_pile_up_without_duplicates() {
        let mut search = MessageSearch::new("hi".into());
        assert_eq!(search.position(), "searching…");
        search.add(Found {
            ids: vec![9, 7],
            total: 3,
            next_from: 7,
        });
        assert!(!search.done);
        search.add(Found {
            ids: vec![7, 2],
            total: 3,
            next_from: 0,
        });
        assert_eq!(search.results, [9, 7, 2]);
        assert!(search.done);
        search.current = Some(1);
        assert_eq!(search.position(), "2 of 3");
    }
}

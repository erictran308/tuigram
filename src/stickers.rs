//! The sticker panel: Tab while writing opens it above the composer, to send
//! a sticker from your recent ones, your favorites or a set you added, or
//! one found by emoji or word.

use tdlib_rs::enums::StickerFullType;
use tdlib_rs::types;

use crate::messages::Preview;
use crate::text;

/// One sticker you can send.
#[derive(Clone)]
pub struct Sticker {
    /// TDLib file id of the sticker itself: what gets sent.
    pub file_id: i32,
    pub width: i32,
    pub height: i32,
    /// The emoji it stands for. Sent with it, and shown where its picture
    /// can't be.
    pub emoji: String,
    /// The picture in the grid; `None` if there's none we can decode.
    pub preview: Option<Preview>,
}

impl Sticker {
    /// `None` for what can't be sent as a sticker message: a custom emoji, or
    /// a sticker only Telegram Premium can send when you don't have it.
    pub fn new(sticker: &types::Sticker, premium: bool) -> Option<Self> {
        match &sticker.full_type {
            StickerFullType::CustomEmoji(_) => return None,
            StickerFullType::Regular(r) if r.premium_animation.is_some() && !premium => {
                return None;
            }
            _ => {}
        }
        Some(Self {
            file_id: sticker.sticker.id,
            width: sticker.width,
            height: sticker.height,
            // Chosen by whoever made the set, so it's cleaned like any text
            // from others.
            emoji: text::clean(&sticker.emoji),
            preview: Preview::sticker_thumbnail(sticker),
        })
    }
}

/// The stickers of a list from TDLib that can be sent, each once.
pub fn convert(list: &[types::Sticker], premium: bool) -> Vec<Sticker> {
    let mut out: Vec<Sticker> = Vec::new();
    for sticker in list.iter().filter_map(|s| Sticker::new(s, premium)) {
        if !out.iter().any(|s| s.file_id == sticker.file_id) {
            out.push(sticker);
        }
    }
    out
}

/// Where a tab of the panel gets its stickers.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Source {
    Recent,
    Favorites,
    /// A sticker set you added, by its id.
    Set(i64),
}

/// A tab of the panel.
pub struct Section {
    pub source: Source,
    pub title: String,
    /// `None` until TDLib sends them.
    pub stickers: Option<Vec<Sticker>>,
    /// TDLib was asked for them.
    asked: bool,
}

/// The sticker panel. Tabs for your recent stickers, your favorites and each
/// set you added, a grid of the one shown, and `/` to find stickers by emoji
/// or word.
pub struct StickerPanel {
    pub chat_id: i64,
    pub sections: Vec<Section>,
    /// Index into `sections` of the tab shown.
    pub tab: usize,
    /// What's typed after `/`; `None` when not searching.
    pub query: Option<String>,
    /// What TDLib found for the search; `None` while it's looking.
    pub found: Option<Vec<Sticker>>,
    /// Index into [`StickerPanel::shown`].
    pub selected: usize,
    /// Where the cursor was in the tab before the search.
    before_search: usize,
    /// First row of the grid shown. Drawing keeps the cursor's row in view.
    pub scroll: usize,
    /// Stickers per row as last drawn, for moving a row up or down.
    pub columns: usize,
    /// TDLib sent your sticker sets.
    pub sets_loaded: bool,
}

impl StickerPanel {
    /// The panel as it opens, before TDLib has sent anything. Recent and
    /// Favorites are asked for at once, along with your sets.
    pub fn new(chat_id: i64) -> Self {
        let section = |source, title: &str| Section {
            source,
            title: title.into(),
            stickers: None,
            asked: true,
        };
        Self {
            chat_id,
            sections: vec![
                section(Source::Recent, "Recent"),
                section(Source::Favorites, "Favorites"),
            ],
            tab: 0,
            query: None,
            found: None,
            selected: 0,
            before_search: 0,
            scroll: 0,
            columns: 1,
            sets_loaded: false,
        }
    }

    /// Your sticker sets, by id and title, in Telegram's order, as tabs after
    /// Recent and Favorites. Their stickers are asked for when they're shown.
    pub fn set_sets(&mut self, sets: Vec<(i64, String)>) {
        self.sets_loaded = true;
        for (id, title) in sets {
            if self.sections.iter().any(|s| s.source == Source::Set(id)) {
                continue;
            }
            self.sections.push(Section {
                source: Source::Set(id),
                // Named by whoever made the set.
                title: text::clean(&title),
                stickers: None,
                asked: false,
            });
        }
    }

    /// The stickers of a tab, from TDLib. Recent and Favorites go away when
    /// you have none, as in Telegram; the tab shown stays the same unless it
    /// was the one that went.
    pub fn set_stickers(&mut self, source: Source, stickers: Vec<Sticker>) {
        let Some(i) = self.sections.iter().position(|s| s.source == source) else {
            return;
        };
        if !stickers.is_empty() || matches!(source, Source::Set(_)) {
            self.sections[i].stickers = Some(stickers);
            return;
        }
        self.sections.remove(i);
        if i < self.tab {
            self.tab -= 1;
        } else if i == self.tab {
            self.tab = self.tab.min(self.sections.len().saturating_sub(1));
            if self.query.is_none() {
                self.selected = 0;
                self.scroll = 0;
            }
        }
    }

    /// The set to ask TDLib for: the one shown, the first time it's shown.
    pub fn pending_set(&mut self) -> Option<i64> {
        if self.query.is_some() {
            return None;
        }
        let section = self.sections.get_mut(self.tab)?;
        match section.source {
            Source::Set(id) if !section.asked => {
                section.asked = true;
                Some(id)
            }
            _ => None,
        }
    }

    /// Nothing to show: no recent or favorite stickers, and no sets.
    pub fn is_empty(&self) -> bool {
        self.sets_loaded && self.sections.is_empty()
    }

    /// The tab shown.
    pub fn section(&self) -> Option<&Section> {
        self.sections.get(self.tab)
    }

    /// The search as TDLib is asked it; `None` when there's nothing to find.
    pub fn search_query(&self) -> Option<String> {
        let query = self.query.as_deref()?.trim();
        (!query.is_empty()).then(|| query.to_string())
    }

    /// The stickers in the grid: what the search found, else the tab shown.
    /// `None` while they're loading.
    pub fn shown(&self) -> Option<&[Sticker]> {
        if self.search_query().is_some() {
            self.found.as_deref()
        } else {
            self.section()?.stickers.as_deref()
        }
    }

    /// The sticker under the cursor.
    pub fn current(&self) -> Option<&Sticker> {
        self.shown()?.get(self.selected)
    }

    /// Moves the cursor `delta` places through the grid, stopping at its ends.
    pub fn move_by(&mut self, delta: isize) {
        let last = self.shown().map_or(0, |s| s.len().saturating_sub(1));
        self.selected = self.selected.saturating_add_signed(delta).min(last);
    }

    /// Moves the cursor `rows` rows up (negative) or down the grid.
    pub fn move_rows(&mut self, rows: isize) {
        self.move_by(rows * self.columns.max(1) as isize);
    }

    /// Shows the tab `delta` tabs over, from its first sticker.
    pub fn switch(&mut self, delta: isize) {
        let last = self.sections.len().saturating_sub(1);
        let tab = self.tab.saturating_add_signed(delta).min(last);
        if tab != self.tab {
            self.tab = tab;
            self.selected = 0;
            self.scroll = 0;
        }
    }

    /// Changes the search. When what TDLib is asked changes, the grid waits
    /// for its answer, from the top.
    pub fn edit_query(&mut self, edit: impl FnOnce(&mut String)) {
        if self.query.is_none() {
            self.before_search = self.selected;
        }
        let before = self.search_query();
        edit(self.query.get_or_insert_default());
        let now = self.search_query();
        if now != before {
            self.found = None;
            self.selected = if now.is_none() { self.before_search } else { 0 };
            self.scroll = 0;
        }
    }

    /// Back from the search to the tab, with the cursor where it was.
    pub fn leave_search(&mut self) {
        if self.search_query().is_some() {
            self.selected = self.before_search;
            self.scroll = 0;
        }
        self.query = None;
        self.found = None;
    }

    /// What TDLib found for `query`; dropped if the search changed since.
    pub fn set_found(&mut self, query: &str, stickers: Vec<Sticker>) {
        if self.search_query().as_deref() == Some(query) {
            self.found = Some(stickers);
        }
    }
}

#[cfg(test)]
mod tests {
    use tdlib_rs::enums::{StickerFormat, ThumbnailFormat};

    use super::*;

    fn tdlib_sticker(file_id: i32, emoji: &str, full_type: StickerFullType) -> types::Sticker {
        types::Sticker {
            id: i64::from(file_id),
            set_id: 2,
            width: 512,
            height: 512,
            emoji: emoji.into(),
            format: StickerFormat::Tgs,
            full_type,
            thumbnail: Some(types::Thumbnail {
                format: ThumbnailFormat::Webp,
                width: 128,
                height: 128,
                file: types::File {
                    id: file_id + 1000,
                    ..Default::default()
                },
            }),
            sticker: types::File {
                id: file_id,
                ..Default::default()
            },
        }
    }

    fn regular(premium: bool) -> StickerFullType {
        StickerFullType::Regular(types::StickerFullTypeRegular {
            premium_animation: premium.then(types::File::default),
        })
    }

    fn stickers(emoji: &[&str]) -> Vec<Sticker> {
        let list: Vec<_> = (1..)
            .zip(emoji)
            .map(|(id, e)| tdlib_sticker(id, e, regular(false)))
            .collect();
        convert(&list, false)
    }

    fn emoji(list: Option<&[Sticker]>) -> Vec<&str> {
        list.unwrap_or_default()
            .iter()
            .map(|s| s.emoji.as_str())
            .collect()
    }

    #[test]
    fn only_stickers_you_can_send_are_offered_each_once() {
        let custom = StickerFullType::CustomEmoji(types::StickerFullTypeCustomEmoji {
            custom_emoji_id: 1,
            needs_repainting: false,
        });
        let list = [
            tdlib_sticker(1, "😀\u{202E}", regular(false)),
            tdlib_sticker(2, "🐳", regular(true)),
            tdlib_sticker(3, "✨", custom),
            tdlib_sticker(1, "😀", regular(false)),
        ];
        let offered = convert(&list, false);
        assert_eq!(emoji(Some(&offered)), ["😀"], "cleaned, once, no Premium");
        let sticker = &offered[0];
        assert_eq!(sticker.file_id, 1, "the sticker itself is sent");
        let preview = sticker.preview.as_ref().expect("a still thumbnail");
        assert_eq!(preview.file_id, 1001, "the grid shows the thumbnail");

        let premium = convert(&list, true);
        assert_eq!(emoji(Some(&premium)), ["😀", "🐳"]);
    }

    #[test]
    fn empty_recent_and_favorites_go_away_and_the_tab_shown_stays() {
        let mut panel = StickerPanel::new(1);
        panel.set_sets(vec![(10, "Cats\u{202E}".into()), (11, "Dogs".into())]);
        let titles = |p: &StickerPanel| -> Vec<String> {
            p.sections.iter().map(|s| s.title.clone()).collect()
        };
        assert_eq!(titles(&panel), ["Recent", "Favorites", "Cats", "Dogs"]);

        panel.switch(2);
        panel.set_stickers(Source::Favorites, Vec::new());
        assert_eq!(titles(&panel), ["Recent", "Cats", "Dogs"]);
        assert_eq!(panel.section().map(|s| s.source), Some(Source::Set(10)));

        panel.switch(-1);
        panel.set_stickers(Source::Recent, Vec::new());
        assert_eq!(panel.section().map(|s| s.source), Some(Source::Set(10)));
        assert!(!panel.is_empty());

        // A set that turns out empty stays, so its tab doesn't vanish.
        panel.set_stickers(Source::Set(10), Vec::new());
        assert_eq!(panel.shown().map(<[_]>::len), Some(0));
    }

    #[test]
    fn with_no_stickers_at_all_the_panel_says_so_once_everything_is_in() {
        let mut panel = StickerPanel::new(1);
        panel.set_stickers(Source::Recent, Vec::new());
        panel.set_stickers(Source::Favorites, Vec::new());
        assert!(!panel.is_empty(), "the sets may still come");
        panel.set_sets(Vec::new());
        assert!(panel.is_empty());
        assert!(panel.shown().is_none() && panel.current().is_none());
        panel.switch(1);
        panel.move_rows(1);
    }

    #[test]
    fn a_set_is_asked_for_once_when_it_is_shown() {
        let mut panel = StickerPanel::new(1);
        assert_eq!(panel.pending_set(), None, "Recent is asked for on opening");
        panel.set_sets(vec![(10, "Cats".into()), (11, "Dogs".into())]);
        panel.switch(2);
        assert_eq!(panel.pending_set(), Some(10));
        assert_eq!(panel.pending_set(), None);
        panel.switch(1);
        panel.edit_query(|q| q.push_str("cat"));
        assert_eq!(panel.pending_set(), None, "not while searching");
        panel.leave_search();
        assert_eq!(panel.pending_set(), Some(11));
    }

    #[test]
    fn the_cursor_moves_through_the_grid_by_place_and_by_row() {
        let mut panel = StickerPanel::new(1);
        panel.set_stickers(Source::Recent, stickers(&["😀", "😂", "😍", "🥺", "😎"]));
        panel.columns = 2;
        panel.move_rows(1);
        assert_eq!(panel.current().map(|s| s.emoji.as_str()), Some("😍"));
        panel.move_by(1);
        panel.move_rows(1);
        assert_eq!(
            panel.current().map(|s| s.emoji.as_str()),
            Some("😎"),
            "the end"
        );
        panel.move_rows(-5);
        assert_eq!(panel.selected, 0);

        panel.set_stickers(Source::Favorites, stickers(&["⭐"]));
        panel.move_by(2);
        panel.switch(1);
        assert_eq!(
            (panel.tab, panel.selected),
            (1, 0),
            "a new tab starts at the top"
        );
        panel.switch(1);
        assert_eq!(panel.tab, 1, "no tab after the last");
    }

    #[test]
    fn a_search_shows_only_its_own_answer_and_goes_back_where_it_was() {
        let mut panel = StickerPanel::new(1);
        panel.set_stickers(Source::Recent, stickers(&["😀", "😂", "😍"]));
        panel.move_by(2);

        panel.edit_query(|_| {});
        assert_eq!(panel.search_query(), None);
        assert_eq!(
            emoji(panel.shown()),
            ["😀", "😂", "😍"],
            "the tab, until typed"
        );
        assert_eq!(panel.selected, 2);

        panel.edit_query(|q| q.push_str("ca"));
        assert!(panel.shown().is_none(), "waiting for TDLib");
        panel.edit_query(|q| q.push('t'));
        panel.set_found("ca", stickers(&["🐶"]));
        assert!(panel.shown().is_none(), "an answer to an older search");
        panel.set_found("cat", stickers(&["🐱", "😺"]));
        assert_eq!(emoji(panel.shown()), ["🐱", "😺"]);
        assert_eq!(panel.selected, 0);

        panel.move_by(1);
        panel.edit_query(|q| q.push(' '));
        assert_eq!(panel.selected, 1, "a space doesn't change the search");
        assert!(panel.shown().is_some());

        panel.leave_search();
        assert_eq!(emoji(panel.shown()), ["😀", "😂", "😍"]);
        assert_eq!(panel.selected, 2);
    }
}

//! `v`: a message's text in a popup, to pick out part of it and copy that,
//! the way vim's Visual mode does.

use crossterm::event::KeyCode;
use ratatui::style::Style;
use ratatui_textarea::{CursorMove, DataCursor, Scrolling, TextArea, WrapMode};

/// The `v` popup over the messages: the text of one message, with a cursor
/// that moves as in vim, `v` starting a selection and `y` copying it.
pub struct TextSelect {
    pub chat_id: i64,
    pub message_id: i64,
    /// The text as it came, to tell whether an edit changed it.
    pub source: String,
    /// The text, never changed here: only its cursor and selection move.
    pub text: TextArea<'static>,
    /// Where `v` was pressed, while selecting.
    anchor: Option<DataCursor>,
}

/// Where a key moves the cursor.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Motion {
    Left,
    Right,
    /// A row up or down on screen, so a long paragraph can be gone through.
    Up,
    Down,
    WordForward,
    WordBack,
    /// The last character of the word, which a selection then takes in.
    WordEnd,
    LineStart,
    LineEnd,
    /// The text's first character, or its last: past it while selecting,
    /// so the selection takes in the last one too.
    Start,
    End,
    HalfPageDown,
    HalfPageUp,
}

/// The motion a key in the popup stands for, as in vim.
pub fn motion(code: KeyCode, ctrl: bool) -> Option<Motion> {
    Some(match code {
        KeyCode::Char('d') if ctrl => Motion::HalfPageDown,
        KeyCode::Char('u') if ctrl => Motion::HalfPageUp,
        _ if ctrl => return None,
        KeyCode::Char('h') | KeyCode::Left => Motion::Left,
        KeyCode::Char('l') | KeyCode::Right => Motion::Right,
        KeyCode::Char('k') | KeyCode::Up => Motion::Up,
        KeyCode::Char('j') | KeyCode::Down => Motion::Down,
        KeyCode::Char('w') => Motion::WordForward,
        KeyCode::Char('b') => Motion::WordBack,
        KeyCode::Char('e') => Motion::WordEnd,
        KeyCode::Char('0' | '^') | KeyCode::Home => Motion::LineStart,
        KeyCode::Char('$') | KeyCode::End => Motion::LineEnd,
        // `gg` in vim: a second `g` stays there.
        KeyCode::Char('g') => Motion::Start,
        KeyCode::Char('G') => Motion::End,
        _ => return None,
    })
}

impl TextSelect {
    /// `text` is a message's `source_text`, already cleaned.
    pub fn new(chat_id: i64, message_id: i64, text: String) -> Self {
        let mut area = TextArea::new(text.split('\n').map(String::from).collect());
        area.set_wrap_mode(WrapMode::WordOrGlyph);
        area.set_cursor_line_style(Style::default());
        Self {
            chat_id,
            message_id,
            source: text,
            text: area,
            anchor: None,
        }
    }

    pub fn selecting(&self) -> bool {
        self.anchor.is_some()
    }

    /// `v`: starts a selection where the cursor is, or drops the one there
    /// is.
    pub fn toggle(&mut self) {
        self.anchor = match self.anchor {
            Some(_) => None,
            None => Some(self.text.cursor()),
        };
        self.settle();
        self.mark();
    }

    pub fn cancel(&mut self) {
        self.anchor = None;
        self.settle();
        self.mark();
    }

    pub fn go(&mut self, motion: Motion) {
        // From the cursor alone: `mark` puts the selection back after.
        self.text.cancel_selection();
        let moves: &[CursorMove] = match motion {
            Motion::Left => &[CursorMove::Back],
            Motion::Right => &[CursorMove::Forward],
            Motion::Up => &[CursorMove::Up],
            Motion::Down => &[CursorMove::Down],
            Motion::WordForward => &[CursorMove::WordForward],
            Motion::WordBack => &[CursorMove::WordBack],
            Motion::WordEnd => &[CursorMove::WordEnd],
            Motion::LineStart => &[CursorMove::Head],
            Motion::LineEnd => &[CursorMove::End],
            Motion::Start => &[CursorMove::Top, CursorMove::Head],
            Motion::End => &[CursorMove::Bottom, CursorMove::End],
            Motion::HalfPageDown => {
                self.text.scroll(Scrolling::HalfPageDown);
                &[]
            }
            Motion::HalfPageUp => {
                self.text.scroll(Scrolling::HalfPageUp);
                &[]
            }
        };
        for &m in moves {
            self.text.move_cursor(m);
        }
        self.settle();
        self.mark();
    }

    /// Out of a selection the cursor is on a character, as in vim's Normal
    /// mode, not past the end of a line where `$` and `G` leave it: `v`
    /// there then takes in that last character, not nothing. While
    /// selecting, past the end takes in the line's break, as `v$` does.
    fn settle(&mut self) {
        if self.anchor.is_some() {
            return;
        }
        let DataCursor(row, col) = self.text.cursor();
        let len = self.text.lines()[row].chars().count();
        if len > 0 && col >= len {
            self.jump(DataCursor(row, len - 1));
        }
    }

    /// What's selected, both ends included, as `y` copies it.
    pub fn selected(&self) -> Option<String> {
        let anchor = self.anchor?;
        let cursor = self.text.cursor();
        let (start, end) = if cursor < anchor {
            (cursor, anchor)
        } else {
            (anchor, cursor)
        };
        let end = self.after(end);
        // Positions count characters; slicing takes bytes.
        let byte =
            |line: &str, col: usize| line.char_indices().nth(col).map_or(line.len(), |(i, _)| i);
        let rows = &self.text.lines()[start.0..=end.0];
        let last = rows.len() - 1;
        let pieces: Vec<&str> = rows
            .iter()
            .enumerate()
            .map(|(i, line)| {
                let from = if i == 0 { byte(line, start.1) } else { 0 };
                let to = if i == last {
                    byte(line, end.1)
                } else {
                    line.len()
                };
                &line[from..to]
            })
            .collect();
        let text = pieces.join("\n");
        (!text.is_empty()).then_some(text)
    }

    /// Has the text area highlight the selection from where `v` was pressed
    /// to the cursor, both included as in vim. The text area's own leaves
    /// out where it ends, so going forward the cursor, drawn over the
    /// character it's on, ends it, and going back it starts just past
    /// where `v` was pressed.
    fn mark(&mut self) {
        self.text.cancel_selection();
        let Some(anchor) = self.anchor else {
            return;
        };
        let cursor = self.text.cursor();
        let from = if cursor < anchor {
            self.after(anchor)
        } else {
            anchor
        };
        self.jump(from);
        self.text.start_selection();
        self.jump(cursor);
    }

    /// The position just past `at`: the next character, or past the end of
    /// a line, the start of the next, taking in the line break.
    fn after(&self, at: DataCursor) -> DataCursor {
        let lines = self.text.lines();
        let DataCursor(row, col) = at;
        if col < lines[row].chars().count() {
            DataCursor(row, col + 1)
        } else if row + 1 < lines.len() {
            DataCursor(row + 1, 0)
        } else {
            at
        }
    }

    fn jump(&mut self, to: DataCursor) {
        // Telegram's messages are far shorter than this.
        let n = |v: usize| u16::try_from(v).unwrap_or(u16::MAX);
        self.text.move_cursor(CursorMove::Jump(n(to.0), n(to.1)));
    }
}

#[cfg(test)]
mod tests {
    use ratatui::buffer::Buffer;
    use ratatui::layout::Rect;
    use ratatui::widgets::Widget;

    use super::*;

    fn select(text: &str) -> TextSelect {
        TextSelect::new(1, 2, text.into())
    }

    /// Draws it `width` columns wide, which is how the text area learns
    /// where its lines wrap.
    fn draw(select: &TextSelect, width: u16) {
        let area = Rect::new(0, 0, width, 10);
        (&select.text).render(area, &mut Buffer::empty(area));
    }

    #[test]
    fn v_e_y_copies_the_whole_word_as_in_vim() {
        let mut s = select("see you tomorrow");
        s.go(Motion::WordForward);
        s.toggle();
        s.go(Motion::WordEnd);
        assert_eq!(s.selected().as_deref(), Some("you"));
    }

    #[test]
    fn v_alone_selects_the_character_under_the_cursor() {
        let mut s = select("abc");
        s.go(Motion::Right);
        assert_eq!(s.selected(), None, "nothing before v");
        s.toggle();
        assert_eq!(s.selected().as_deref(), Some("b"));
        s.toggle();
        assert_eq!(s.selected(), None, "v again drops it");
    }

    #[test]
    fn going_back_from_where_v_was_pressed_keeps_that_character() {
        let mut s = select("one two three");
        s.go(Motion::WordForward);
        s.go(Motion::WordEnd);
        s.toggle();
        s.go(Motion::WordBack);
        assert_eq!(s.selected().as_deref(), Some("two"));
        assert_eq!(
            s.text.selection_range(),
            Some(((0, 4), (0, 7))),
            "highlighted as copied"
        );
    }

    #[test]
    fn a_selection_across_lines_keeps_their_breaks() {
        let mut s = select("first line\nsecond\nthird one");
        s.go(Motion::WordForward);
        s.toggle();
        s.go(Motion::Down);
        s.go(Motion::Down);
        s.go(Motion::LineStart);
        s.go(Motion::WordEnd);
        assert_eq!(s.selected().as_deref(), Some("line\nsecond\nthird"));
    }

    #[test]
    fn the_end_of_a_line_takes_in_its_break_and_the_end_of_the_text_nothing_more() {
        let mut s = select("ab\ncd");
        s.toggle();
        s.go(Motion::LineEnd);
        assert_eq!(s.selected().as_deref(), Some("ab\n"));
        s.go(Motion::End);
        assert_eq!(s.selected().as_deref(), Some("ab\ncd"));
        s.cancel();
        s.go(Motion::Start);
        s.toggle();
        s.go(Motion::End);
        assert_eq!(s.selected().as_deref(), Some("ab\ncd"), "g to G: all of it");
    }

    #[test]
    fn dollar_or_g_then_v_y_copies_the_last_character_as_in_vim() {
        let mut s = select("ab\ncd");
        s.go(Motion::LineEnd);
        s.toggle();
        assert_eq!(s.selected().as_deref(), Some("b"));
        s.cancel();
        s.go(Motion::End);
        s.toggle();
        assert_eq!(s.selected().as_deref(), Some("d"));
        // Esc out of `v$` puts the cursor back on the last character.
        s.cancel();
        s.go(Motion::Start);
        s.toggle();
        s.go(Motion::LineEnd);
        s.cancel();
        assert_eq!(s.text.cursor(), (0, 1));
    }

    #[test]
    fn an_empty_last_line_selects_nothing_rather_than_an_empty_text() {
        let mut s = select("ab\n");
        s.go(Motion::End);
        s.toggle();
        assert_eq!(s.selected(), None, "y copies nothing");
    }

    #[test]
    fn j_and_k_go_a_row_at_a_time_through_a_long_paragraph() {
        let mut s = select("aaaa bbbb cccc dddd");
        draw(&s, 10);
        s.toggle();
        s.go(Motion::Down);
        assert_eq!(s.text.cursor(), (0, 10), "the second row, not the end");
        assert_eq!(s.selected().as_deref(), Some("aaaa bbbb c"));
        s.go(Motion::Up);
        assert_eq!(s.selected().as_deref(), Some("a"));
    }

    #[test]
    fn wide_characters_are_cut_on_their_boundaries() {
        let mut s = select("héllo 👋 wörld");
        s.go(Motion::WordForward);
        s.toggle();
        s.go(Motion::LineEnd);
        assert_eq!(s.selected().as_deref(), Some("👋 wörld"));
    }

    #[test]
    fn keys_are_vim_motions_and_ctrl_only_pages() {
        assert_eq!(motion(KeyCode::Char('e'), false), Some(Motion::WordEnd));
        assert_eq!(motion(KeyCode::Char('d'), true), Some(Motion::HalfPageDown));
        assert_eq!(motion(KeyCode::Char('j'), true), None);
        assert_eq!(motion(KeyCode::Char('y'), false), None);
    }
}

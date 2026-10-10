//! The terminal tuimeta draws on: ratatui's crossterm backend, with the blank
//! behind an emoji like ❤️ sent before the emoji instead of after it.

use std::io::{self, Stdout, Write, stdout};

use crossterm::execute;
use crossterm::terminal::{EnterAlternateScreen, enable_raw_mode};
use ratatui::backend::{Backend, ClearType, CrosstermBackend, WindowSize};
use ratatui::buffer::{Cell, CellWidth};
use ratatui::layout::{Position, Size};

/// Asks for an emoji's colored, two-column look.
const EMOJI_STYLE: char = '\u{FE0F}';

pub type Terminal = ratatui::Terminal<Crossterm<Stdout>>;

/// Takes over the terminal as `ratatui::init` does: raw mode, the alternate
/// screen, and a panic hook that puts them back before the one already set.
pub fn init() -> Terminal {
    try_init().expect("failed to initialize terminal")
}

fn try_init() -> io::Result<Terminal> {
    let hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        ratatui::restore();
        hook(info);
    }));
    enable_raw_mode()?;
    execute!(stdout(), EnterAlternateScreen)?;
    ratatui::Terminal::new(Crossterm(CrosstermBackend::new(stdout())))
}

/// ratatui's crossterm backend, drawing each frame's changes in the order
/// [`blank_first`] puts them.
pub struct Crossterm<W: Write>(CrosstermBackend<W>);

impl<W: Write> Backend for Crossterm<W> {
    type Error = io::Error;

    fn draw<'a, I>(&mut self, content: I) -> io::Result<()>
    where
        I: Iterator<Item = (u16, u16, &'a Cell)>,
    {
        self.0.draw(blank_first(content.collect()).into_iter())
    }

    fn append_lines(&mut self, n: u16) -> io::Result<()> {
        self.0.append_lines(n)
    }

    fn hide_cursor(&mut self) -> io::Result<()> {
        self.0.hide_cursor()
    }

    fn show_cursor(&mut self) -> io::Result<()> {
        self.0.show_cursor()
    }

    fn get_cursor_position(&mut self) -> io::Result<Position> {
        self.0.get_cursor_position()
    }

    fn set_cursor_position<P: Into<Position>>(&mut self, position: P) -> io::Result<()> {
        self.0.set_cursor_position(position)
    }

    fn clear(&mut self) -> io::Result<()> {
        self.0.clear()
    }

    fn clear_region(&mut self, clear_type: ClearType) -> io::Result<()> {
        self.0.clear_region(clear_type)
    }

    fn size(&self) -> io::Result<Size> {
        self.0.size()
    }

    fn window_size(&mut self) -> io::Result<WindowSize> {
        self.0.window_size()
    }

    fn flush(&mut self) -> io::Result<()> {
        Backend::flush(&mut self.0)
    }
}

/// The cells to draw, with the blanks behind each emoji that asks for its
/// two-column look (❤️, with U+FE0F) moved in front of it.
///
/// ratatui sends the column behind such an emoji again whenever it held
/// something else, for terminals that draw the emoji one column wide and
/// would leave it as it was, and sends it right after the emoji without
/// moving the cursor there. Terminals that draw the emoji two columns wide,
/// as ratatui lays it out (Ghostty, kitty, WezTerm, iTerm2), have the cursor
/// past both columns by then, so the blank landed one column to the right,
/// and every cell after it up to the next one the cursor was moved to: a
/// reaction scrolled into view showed a gap after its heart, and the pill
/// beside it lost its emoji. Sent first, the blank is where ratatui means it
/// on both kinds of terminal, the emoji covers it where it's wide, and the
/// cell after the emoji is no longer next to the last one sent, so the
/// backend moves the cursor to it.
fn blank_first(mut cells: Vec<(u16, u16, &Cell)>) -> Vec<(u16, u16, &Cell)> {
    let mut i = 0;
    while i < cells.len() {
        let (x, y, cell) = cells[i];
        let width = usize::from(cell.cell_width());
        let mut end = i + 1;
        if width > 1 && cell.symbol().contains(EMOJI_STYLE) {
            while end - i < width
                && let Some(&(next_x, next_y, _)) = cells.get(end)
                && next_y == y
                && usize::from(next_x) == usize::from(x) + (end - i)
            {
                end += 1;
            }
            cells[i..end].rotate_left(1);
        }
        i = end;
    }
    cells
}

#[cfg(test)]
mod tests {
    use ratatui::buffer::Buffer;
    use ratatui::layout::Rect;
    use ratatui::style::Style;
    use unicode_width::UnicodeWidthChar;

    use super::*;

    /// A terminal's screen, from the bytes the backend wrote: cursor moves,
    /// colors (ignored) and text. Each cell holds what's drawn in it; the
    /// second column of a wide character holds nothing.
    struct Emulator {
        cells: Vec<String>,
        x: usize,
        /// Whether U+FE0F makes the character before it two columns wide and
        /// moves the cursor past both, as Ghostty, kitty and WezTerm do.
        widens: bool,
    }

    impl Emulator {
        fn new(row: &str, widens: bool) -> Self {
            let cells = row.chars().map(String::from).collect();
            Self {
                cells,
                x: 0,
                widens,
            }
        }

        fn put(&mut self, x: usize, text: String) {
            // Past the edge: dropped, as a terminal without line wrap does.
            if x >= self.cells.len() {
                return;
            }
            // Writing over either half of a wide character clears the other.
            if self.cells[x].is_empty() {
                self.cells[x - 1] = " ".into();
            }
            if self.cells.get(x + 1).is_some_and(String::is_empty) {
                self.cells[x + 1] = " ".into();
            }
            self.cells[x] = text;
        }

        fn run(&mut self, bytes: &[u8]) {
            let text = std::str::from_utf8(bytes).unwrap();
            let mut chars = text.chars().peekable();
            while let Some(c) = chars.next() {
                if c == '\x1b' {
                    assert_eq!(chars.next(), Some('['), "only CSI sequences in {text:?}");
                    let mut params = String::new();
                    let end = loop {
                        let c = chars.next().unwrap();
                        if ('\x40'..='\x7e').contains(&c) {
                            break c;
                        }
                        params.push(c);
                    };
                    if end == 'H' {
                        let (_, col) = params.split_once(';').unwrap();
                        self.x = col.parse::<usize>().unwrap() - 1;
                    }
                    continue;
                }
                match c.width().unwrap_or(0) {
                    0 => {
                        let last = self.x - 1;
                        self.cells[last].push(c);
                        if c == EMOJI_STYLE && self.widens {
                            self.put(self.x, String::new());
                            self.x += 1;
                        }
                    }
                    width => {
                        self.put(self.x, c.into());
                        for column in 1..width {
                            self.put(self.x + column, String::new());
                        }
                        self.x += width;
                    }
                }
            }
        }
    }

    /// Draws the changes from `before` to `after` on a terminal showing
    /// `before`, and returns what each of its cells then shows.
    fn draw(before: &str, after: &str, widens: bool) -> Vec<String> {
        let area = Rect::new(0, 0, before.chars().count() as u16, 1);
        let mut prev = Buffer::empty(area);
        prev.set_string(0, 0, before, Style::new());
        let mut next = Buffer::empty(area);
        next.set_string(0, 0, after, Style::new());
        let mut out = Vec::new();
        Crossterm(CrosstermBackend::new(&mut out))
            .draw(prev.diff(&next).into_iter())
            .unwrap();
        let mut screen = Emulator::new(before, widens);
        screen.run(&out);
        screen.cells
    }

    #[test]
    fn reactions_drawn_over_other_text_land_in_their_columns_where_hearts_are_wide() {
        let cells = draw("abcdefghijkl", " ❤\u{fe0f} 1  ❤\u{fe0f} 2 ", true);
        assert_eq!(
            cells,
            [
                " ",
                "❤\u{fe0f}",
                "",
                " ",
                "1",
                " ",
                " ",
                "❤\u{fe0f}",
                "",
                " ",
                "2",
                " "
            ]
        );
    }

    #[test]
    fn reactions_drawn_over_other_text_land_in_their_columns_where_hearts_are_narrow() {
        let cells = draw("abcdefghijkl", " ❤\u{fe0f} 1  ❤\u{fe0f} 2 ", false);
        assert_eq!(
            cells,
            [
                " ",
                "❤\u{fe0f}",
                " ",
                " ",
                "1",
                " ",
                " ",
                "❤\u{fe0f}",
                " ",
                " ",
                "2",
                " "
            ]
        );
    }

    #[test]
    fn wide_characters_without_the_emoji_mark_are_drawn_as_ratatui_orders_them() {
        let prev = Buffer::with_lines(["abcdefg"]);
        let next = Buffer::with_lines(["a👍コ😀"]);
        let diff = prev.diff(&next);
        assert_eq!(blank_first(diff.clone()), diff);
    }

    #[test]
    fn only_the_columns_behind_the_emoji_go_before_it() {
        let prev = Buffer::with_lines(["abcdef"]);
        let next = Buffer::with_lines(["❤\u{fe0f}xyz "]);
        let order: Vec<_> = blank_first(prev.diff(&next))
            .into_iter()
            .map(|(x, _, cell)| (x, cell.symbol().to_string()))
            .collect();
        let expected = [
            (1, " "),
            (0, "❤\u{fe0f}"),
            (2, "x"),
            (3, "y"),
            (4, "z"),
            (5, " "),
        ];
        let expected: Vec<_> = expected.map(|(x, s)| (x, s.to_string())).to_vec();
        assert_eq!(order, expected);
    }
}

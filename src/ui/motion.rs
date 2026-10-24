//! Drawing what `crate::motion` times, over what's drawn already.
//!
//! When tuigram's window gets focus back, four lights draw the border of
//! the pane with the keys, so the eyes find where typing goes. They start a
//! quarter of the way round from each other, and each ends where the next
//! one started, so they meet as one line. Where none has been yet the
//! border is still dim, as if the pane didn't have the keys; behind them,
//! it's in the accent color, as it stays.
//!
//! Only colors change, never text, so nothing moves under the cursor or
//! the eyes, and a cell with a picture in it is left alone: changed, it
//! would send the picture to the terminal again.

use std::time::Duration;

use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::Color;

use crate::motion;
use crate::theme::{self, Colors};

/// How many lights there are, each drawing as much of the border.
const LIGHT_COUNT: usize = 4;
/// The share of a light's stretch its tail covers at first.
const TAIL: f32 = 0.6;
/// How far a new message's bubble starts towards the accent color.
const GLOW_TINT: f32 = 0.35;

/// Draws the lights over the border of `area`, `elapsed` into their run.
/// Only the line changes color, not a title on it.
pub fn lights(buf: &mut Buffer, area: Rect, elapsed: Duration, colors: &Colors) {
    let area = area.intersection(buf.area);
    let cells = perimeter(area);
    let time = elapsed.as_secs_f32() / motion::LIGHTS.as_secs_f32();
    // Done: the border is whole, as the pane draws it.
    if cells.is_empty() || time >= 1.0 {
        return;
    }
    // A title on the top border, from its first character that isn't the
    // line to its last: lines in it, like a name's "──", are its own.
    let top = area.top();
    let words: Vec<u16> = (area.left()..area.right())
        .filter(|&x| !is_line(buf[(x, top)].symbol()))
        .collect();
    let title = words.first().zip(words.last()).map(|(&a, &b)| a..=b);
    let stretch = cells.len() as f32 / LIGHT_COUNT as f32;
    // Fast at first, slowing down as they arrive.
    let eased = ease_out(time);
    let head = eased * stretch;
    // Shorter as they go, so their glow is gone once they meet.
    let tail = (stretch * TAIL).max(1.0) * (1.0 - time);
    for (i, (x, y)) in cells.into_iter().enumerate() {
        let in_title = y == top && title.as_ref().is_some_and(|t| t.contains(&x));
        let cell = &mut buf[(x, y)];
        if in_title || !is_line(cell.symbol()) {
            continue;
        }
        // How far behind the head of the light whose stretch it's in.
        let behind = head - (i as f32) % stretch;
        cell.fg = if behind < 0.0 {
            colors.border
        } else if behind >= tail {
            colors.accent
        } else {
            light_color(colors, 1.0 - behind / tail)
        };
    }
}

/// Fades in what was drawn over `under`, a popup `elapsed` into fading in:
/// each cell it changed starts in the background of what it covers, and
/// turns into its own colors.
pub fn fade_in(buf: &mut Buffer, under: &Buffer, elapsed: Duration) {
    let time = elapsed.as_secs_f32() / motion::POPUP.as_secs_f32();
    if time >= 1.0 || buf.area != under.area {
        return;
    }
    let shown = ease_out(time);
    for (cell, below) in buf.content.iter_mut().zip(&under.content) {
        if cell != below {
            cell.fg = blend(below.bg, cell.fg, shown);
            cell.bg = blend(below.bg, cell.bg, shown);
        }
    }
}

/// A new message's bubble, its rows on screen in `area`, `elapsed` into
/// glowing: its background starts tinted with the accent and goes back to
/// the bubble's own. Cells where `picture` says one is drawn are left as
/// they are.
pub fn glow(
    buf: &mut Buffer,
    area: Rect,
    picture: impl Fn(u16, u16) -> bool,
    elapsed: Duration,
    colors: &Colors,
) {
    let time = elapsed.as_secs_f32() / motion::GLOW.as_secs_f32();
    // A theme whose bubbles are the background has no bubble to glow.
    if time >= 1.0 || colors.other_bubble == colors.bg {
        return;
    }
    // Bright for a moment, then fading smoothly away.
    let strength = 1.0 - time * time * (3.0 - 2.0 * time);
    let tint = theme::mix(colors.other_bubble, colors.accent, GLOW_TINT * strength);
    let area = area.intersection(buf.area);
    for y in area.top()..area.bottom() {
        for x in area.left()..area.right() {
            let cell = &mut buf[(x, y)];
            if cell.bg == colors.other_bubble && !picture(x, y) {
                cell.bg = tint;
            }
        }
    }
}

/// Fast at first, slowing down as it arrives: `time` (0 to 1) eased out.
fn ease_out(time: f32) -> f32 {
    1.0 - (1.0 - time) * (1.0 - time)
}

/// `from` moved `amount` (0 to 1) of the way to `to`. Terminal colors,
/// which can't be mixed, switch halfway.
fn blend(from: Color, to: Color, amount: f32) -> Color {
    match (from, to) {
        (Color::Rgb(..), Color::Rgb(..)) => theme::mix(from, to, amount),
        _ if amount < 0.5 => from,
        _ => to,
    }
}

/// The cells round the edge of `area`, clockwise from its top left corner.
fn perimeter(area: Rect) -> Vec<(u16, u16)> {
    if area.width < 2 || area.height < 2 {
        return Vec::new();
    }
    let (left, right) = (area.left(), area.right() - 1);
    let (top, bottom) = (area.top(), area.bottom() - 1);
    let along_top = (left..right).map(|x| (x, top));
    let down_right = (top..bottom).map(|y| (right, y));
    let along_bottom = (left + 1..=right).rev().map(|x| (x, bottom));
    let up_left = (top + 1..=bottom).rev().map(|y| (left, y));
    along_top
        .chain(down_right)
        .chain(along_bottom)
        .chain(up_left)
        .collect()
}

/// A piece of the border's line, round corners and the joins of a rule
/// across the pane too.
fn is_line(symbol: &str) -> bool {
    symbol
        .chars()
        .next()
        .is_some_and(|c| ('\u{2500}'..='\u{257f}').contains(&c))
}

/// The light at `strength`: 1 at its head, in `colors.light`, fading to the
/// accent down its tail.
fn light_color(colors: &Colors, strength: f32) -> Color {
    blend(colors.accent, colors.light, strength)
}

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::widgets::Widget;

    /// A focused pane's border, `width` by `height`, with the lights
    /// `elapsed` into their run.
    fn lit(width: u16, height: u16, elapsed: Duration, colors: &Colors) -> Buffer {
        let area = Rect::new(0, 0, width, height);
        let mut buf = Buffer::empty(area);
        crate::ui::bordered(colors)
            .border_style(colors.accent)
            .render(area, &mut buf);
        lights(&mut buf, area, elapsed, colors);
        buf
    }

    /// Runs of lit cells round the border: the lights in sight.
    fn in_sight(buf: &Buffer, colors: &Colors) -> usize {
        let glowing: Vec<bool> = perimeter(buf.area)
            .into_iter()
            .map(|p| buf[p].fg != colors.accent && buf[p].fg != colors.border)
            .collect();
        let before = |i: usize| glowing[(i + glowing.len() - 1) % glowing.len()];
        (0..glowing.len())
            .filter(|&i| glowing[i] && !before(i))
            .count()
    }

    #[test]
    fn the_lights_go_clockwise_from_the_top_left_corner() {
        let area = Rect::new(2, 1, 4, 3);
        assert_eq!(
            perimeter(area),
            [
                (2, 1),
                (3, 1),
                (4, 1),
                (5, 1),
                (5, 2),
                (5, 3),
                (4, 3),
                (3, 3),
                (2, 3),
                (2, 2)
            ],
            "every cell of the edge once"
        );
        assert!(
            perimeter(Rect::new(0, 0, 1, 5)).is_empty(),
            "no border to go round"
        );
    }

    #[test]
    fn four_lights_each_draw_a_quarter_of_the_border() {
        let colors = Colors::default();
        // 48 cells round, so a light starts every 12 cells.
        let edge = perimeter(Rect::new(0, 0, 20, 6));
        let buf = lit(20, 6, motion::LIGHTS / 10, &colors);
        assert_eq!(in_sight(&buf, &colors), LIGHT_COUNT);
        for start in [0, 12, 24, 36] {
            assert_ne!(
                buf[edge[start]].fg, colors.border,
                "a light set off at {start}"
            );
            assert_ne!(
                buf[edge[start]].fg, colors.accent,
                "a light set off at {start}"
            );
        }
        assert_eq!(buf[edge[11]].fg, colors.border, "not reached yet");

        let buf = lit(20, 6, motion::LIGHTS / 2, &colors);
        assert_eq!(in_sight(&buf, &colors), LIGHT_COUNT, "still going");
        assert_eq!(buf[edge[0]].fg, colors.accent, "behind them");
        assert_eq!(buf[edge[11]].fg, colors.border, "not reached yet");
    }

    #[test]
    fn the_lights_meet_as_one_line() {
        let colors = Colors::default();
        let buf = lit(20, 6, motion::LIGHTS.mul_f32(0.99), &colors);
        assert!(
            perimeter(buf.area)
                .iter()
                .all(|&p| buf[p].fg != colors.border),
            "each reached where the next one started"
        );
    }

    #[test]
    fn once_they_meet_the_border_is_the_accent_all_the_way() {
        let colors = Colors::default();
        let buf = lit(20, 6, motion::LIGHTS, &colors);
        assert_eq!(in_sight(&buf, &colors), 0);
        assert!(
            perimeter(buf.area)
                .iter()
                .all(|&p| buf[p].fg == colors.accent)
        );
    }

    #[test]
    fn a_title_on_the_border_keeps_its_color() {
        let colors = Colors::default();
        let area = Rect::new(0, 0, 20, 6);
        let block = crate::ui::bordered(&colors)
            .title("Chats")
            .border_style(colors.accent);
        let mut unlit = Buffer::empty(area);
        block.render(area, &mut unlit);
        for elapsed in [Duration::ZERO, motion::LIGHTS / 10, motion::LIGHTS / 2] {
            let mut buf = unlit.clone();
            lights(&mut buf, area, elapsed, &colors);
            // "Chats" starts in the second column.
            assert_eq!(buf[(1, 0)].symbol(), "C");
            assert!((1..6).all(|x| buf[(x, 0)] == unlit[(x, 0)]), "{elapsed:?}");
        }
    }

    #[test]
    fn lines_in_a_title_are_the_title_s_own() {
        let colors = Colors::default();
        let area = Rect::new(0, 0, 30, 6);
        let block = crate::ui::bordered(&colors)
            .title(" ──Bob── ")
            .border_style(colors.accent);
        let mut unlit = Buffer::empty(area);
        block.render(area, &mut unlit);
        let mut buf = unlit.clone();
        lights(&mut buf, area, motion::LIGHTS / 10, &colors);
        assert!((1..10).all(|x| buf[(x, 0)] == unlit[(x, 0)]), "the name");
        assert_eq!(buf[(11, 0)].fg, colors.border, "the border after it");
    }

    #[test]
    fn the_light_stands_out_on_light_themes_too() {
        let themes = crate::theme::Themes::built_in();
        let (dark, light) = (themes.colors("mocha"), themes.colors("latte"));
        let (dark, light) = (dark.unwrap(), light.unwrap());
        assert_eq!(light_color(&dark, 1.0), dark.light, "its head");
        assert_eq!(light_color(&dark, 0.0), dark.accent, "its tail's end");
        let (
            Color::Rgb(r, ..),
            Color::Rgb(accent_r, ..),
            Color::Rgb(lr, ..),
            Color::Rgb(light_accent_r, ..),
        ) = (dark.light, dark.accent, light.light, light.accent)
        else {
            panic!("mixed");
        };
        assert!(r > accent_r, "brighter than Mocha's accent");
        assert!(lr < light_accent_r, "darker than Latte's accent");
    }

    #[test]
    fn a_popup_fades_in_from_what_it_covers() {
        let colors = Colors::default();
        let area = Rect::new(0, 0, 10, 3);
        let mut under = Buffer::empty(area);
        under.set_style(area, ratatui::style::Style::new().bg(colors.bg));
        let mut over = under.clone();
        let popup = Rect::new(2, 1, 5, 1);
        over.set_string(
            2,
            1,
            "Hello",
            ratatui::style::Style::new()
                .fg(colors.fg)
                .bg(colors.popup_bg),
        );

        let mut buf = over.clone();
        fade_in(&mut buf, &under, Duration::ZERO);
        assert_eq!(buf[(2, 1)].symbol(), "H", "its text is in place at once");
        assert_eq!((buf[(2, 1)].fg, buf[(2, 1)].bg), (colors.bg, colors.bg));
        assert_eq!(buf[(0, 0)], under[(0, 0)], "around it, nothing changes");

        let mut buf = over.clone();
        fade_in(&mut buf, &under, motion::POPUP / 2);
        let cell = &buf[(popup.x, popup.y)];
        assert!(cell.fg != colors.bg && cell.fg != colors.fg, "on its way");

        let mut buf = over.clone();
        fade_in(&mut buf, &under, motion::POPUP);
        assert_eq!(buf, over, "faded in");
    }

    #[test]
    fn a_new_bubble_glows_then_goes_back_to_its_color() {
        let colors = Colors::default();
        let area = Rect::new(0, 0, 8, 2);
        let mut drawn = Buffer::empty(area);
        drawn.set_style(
            Rect::new(0, 0, 6, 2),
            ratatui::style::Style::new().bg(colors.other_bubble),
        );
        let picture = |x: u16, y: u16| (x, y) == (1, 1);

        let mut buf = drawn.clone();
        glow(&mut buf, area, picture, Duration::ZERO, &colors);
        assert_ne!(buf[(0, 0)].bg, colors.other_bubble, "tinted");
        assert_eq!(buf[(1, 1)], drawn[(1, 1)], "the picture's cell");
        assert_eq!(buf[(7, 0)], drawn[(7, 0)], "beside the bubble");

        let mut buf = drawn.clone();
        glow(&mut buf, area, picture, motion::GLOW, &colors);
        assert_eq!(buf, drawn, "back to its color");
    }
}

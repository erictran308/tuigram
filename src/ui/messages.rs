//! The message pane: chat bubbles, yours on the right, newest at the bottom.

use std::collections::HashMap;
use std::ops::Range;

use chrono::{Local, TimeZone};
use ratatui::Frame;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Color, Style, Stylize};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Paragraph};
use ratatui_image::FontSize;
use ratatui_image::sliced::{SignedPosition, SlicedImage};
use unicode_width::UnicodeWidthStr;

use super::truncate;
use crate::chats::Chats;
use crate::images::Images;
use crate::messages::{Msg, OpenChat, Preview, ScrollAnchor, SendState, Sender};
use crate::search;
use crate::theme::Colors;

/// Bubbles take at most this share of the pane width.
const BUBBLE_WIDTH_PERCENT: usize = 75;
/// Largest inline photo, in terminal cells.
const MAX_PHOTO_COLS: usize = 40;
const MAX_PHOTO_ROWS: usize = 16;
/// Stickers are smaller, as in Telegram.
const MAX_STICKER_COLS: usize = 20;
const MAX_STICKER_ROWS: usize = 10;

/// Resolves message senders to display names.
pub struct Names<'a> {
    pub users: &'a HashMap<i64, String>,
    pub chats: &'a Chats,
}

impl Names<'_> {
    fn get(&self, sender: Sender) -> String {
        let name = match sender {
            Sender::User(id) => self.users.get(&id).cloned(),
            Sender::Chat(id) => self.chats.get(id).map(|c| c.title.clone()),
        };
        name.unwrap_or_else(|| "Unknown".into())
    }
}

/// Where one message landed in the laid-out lines.
struct Placed {
    id: i64,
    /// First line, including the date separator and gap above the bubble.
    start: usize,
    bubble_start: usize,
    end: usize,
}

/// Rows reserved inside a bubble where a photo gets drawn after the text.
struct PhotoSlot {
    /// First line of the reserved rows.
    line: usize,
    /// Column where the photo starts, from the left of the message area.
    x: u16,
    cols: u16,
    rows: u16,
    photo: Preview,
}

pub fn draw(
    frame: &mut Frame,
    area: Rect,
    open: &mut OpenChat,
    names: &Names,
    images: &mut Images,
    focused: bool,
    colors: &Colors,
) {
    let chat = names.chats.get(open.chat_id);
    let mut title = vec![
        Span::from(format!(
            " {} ",
            names.chats.title(open.chat_id).unwrap_or_default()
        ))
        .style(super::title_style(names.chats, open.chat_id, colors)),
    ];
    if let Some(search) = &open.search {
        title.push(Span::from(format!(
            "· /{} {} ",
            search.query,
            search.position()
        )));
    }
    if open.loading.is_some() {
        title.push(Span::from("· loading… "));
    }
    let block = Block::bordered()
        .title(Line::from(title))
        .border_style(super::border(focused, colors));
    let inner = block.inner(area);
    frame.render_widget(block, area);

    if open.messages.is_empty() {
        let text = if open.loading.is_some() {
            "Loading…"
        } else {
            "No messages yet"
        };
        frame.render_widget(Paragraph::new(text).fg(colors.muted).centered(), inner);
        return;
    }

    // One-column gutters either side hold the cursor marker.
    let [left, body, right] = Layout::horizontal([
        Constraint::Length(1),
        Constraint::Fill(1),
        Constraint::Length(1),
    ])
    .areas(inner);
    let show_names = chat.is_none_or(|c| !c.is_channel);
    let font = images.font_size();
    let (lines, placed, photos) =
        layout(open, names, show_names, body.width as usize, font, colors);

    let height = body.height as usize;
    let selected = match open.selected {
        Some(id) => placed.iter().find(|p| p.id == id),
        None => placed.last(),
    };
    let top = scroll_top(open, &placed, selected, lines.len(), height);

    let visible: Vec<Line> = lines.into_iter().skip(top).take(height).collect();
    // Short chats sit at the bottom of the pane, like in Telegram.
    let pad = (height - visible.len()) as u16;
    let shift = |r: Rect| Rect {
        y: r.y + pad,
        height: r.height - pad,
        ..r
    };
    frame.render_widget(Paragraph::new(visible), shift(body));
    draw_photos(frame, shift(body), &photos, top, images, colors);

    if focused && let Some(sel) = selected {
        let rows = (sel.bubble_start.max(top)..sel.end.min(top + height)).map(|_| "▌");
        let marker = |rows: Vec<&'static str>| {
            Paragraph::new(rows.into_iter().map(Line::from).collect::<Vec<_>>()).fg(colors.accent)
        };
        let rows: Vec<_> = rows.collect();
        let offset = sel.bubble_start.saturating_sub(top) as u16;
        let place = |r: Rect| {
            let r = shift(r);
            Rect {
                y: r.y + offset,
                height: (rows.len() as u16).min(r.height.saturating_sub(offset)),
                ..r
            }
        };
        frame.render_widget(marker(rows.clone()), place(left));
        frame.render_widget(marker(rows.iter().map(|_| "▐").collect()), place(right));
    }
}

/// Paints photos over the rows reserved for them. `SlicedImage` draws just
/// the visible rows of a photo that's partly scrolled out.
fn draw_photos(
    frame: &mut Frame,
    area: Rect,
    photos: &[PhotoSlot],
    top: usize,
    images: &mut Images,
    colors: &Colors,
) {
    for slot in photos {
        let y = slot.line as i64 - top as i64;
        if y >= i64::from(area.height) || y + i64::from(slot.rows) <= 0 {
            continue;
        }
        images.want(&slot.photo, slot.cols, slot.rows);
        if let Some(image) = images.get(&slot.photo, slot.cols, slot.rows) {
            let position = SignedPosition::from((slot.x as i16, y as i16));
            frame.render_widget(SlicedImage::new(image, position), area);
            continue;
        }
        let label = if images.is_broken(&slot.photo) {
            "Preview unavailable"
        } else {
            "Loading…"
        };
        let middle = y + i64::from(slot.rows / 2);
        if (0..i64::from(area.height)).contains(&middle) {
            let row = Rect {
                x: area.x + slot.x,
                y: area.y + middle as u16,
                width: slot.cols.min(area.width.saturating_sub(slot.x)),
                height: 1,
            };
            frame.render_widget(Line::from(label).fg(colors.subtle).centered(), row);
        }
    }
}

/// Size of a photo in cells: as wide as allowed (but not past its own pixels),
/// with rows from the aspect ratio, since cells are taller than wide.
fn photo_cells(photo: &Preview, max_cols: usize, font: FontSize) -> (u16, u16) {
    let (fw, fh) = (f64::from(font.width.max(1)), f64::from(font.height.max(1)));
    let (pw, ph) = (f64::from(photo.width), f64::from(photo.height));
    let (limit_cols, limit_rows) = if photo.sticker {
        (MAX_STICKER_COLS, MAX_STICKER_ROWS)
    } else {
        (MAX_PHOTO_COLS, MAX_PHOTO_ROWS)
    };
    let mut cols = (max_cols.min(limit_cols) as f64).min(pw / fw);
    let mut rows = cols * fw * ph / pw / fh;
    if rows > limit_rows as f64 {
        rows = limit_rows as f64;
        cols = rows * fh * pw / ph / fw;
    }
    (cols.round().max(1.0) as u16, rows.round().max(1.0) as u16)
}

/// Picks the first visible line: stick to the bottom while following new
/// messages, otherwise scroll as little as possible to keep the cursor in view.
fn scroll_top(
    open: &mut OpenChat,
    placed: &[Placed],
    selected: Option<&Placed>,
    total: usize,
    height: usize,
) -> usize {
    let max_top = total.saturating_sub(height);
    let mut top = match (open.selected, open.scroll) {
        (Some(_), Some(anchor)) => placed
            .iter()
            .find(|p| p.id == anchor.msg_id)
            .map_or(max_top, |p| p.start + anchor.offset),
        _ => max_top,
    };
    if let Some(sel) = selected {
        if sel.start < top {
            top = sel.start;
        } else if sel.end > top + height {
            // A bubble taller than the pane shows from its top.
            top = if sel.end - sel.start > height {
                sel.start
            } else {
                sel.end - height
            };
        }
    }
    let top = top.min(max_top);
    open.scroll = placed
        .iter()
        .rev()
        .find(|p| p.start <= top)
        .map(|p| ScrollAnchor {
            msg_id: p.id,
            offset: top - p.start,
        });
    top
}

fn layout(
    open: &OpenChat,
    names: &Names,
    show_names: bool,
    width: usize,
    font: FontSize,
    colors: &Colors,
) -> (Vec<Line<'static>>, Vec<Placed>, Vec<PhotoSlot>) {
    // Text width inside a bubble, after one column of padding each side.
    let max_text = (width * BUBBLE_WIDTH_PERCENT / 100)
        .max(12)
        .min(width)
        .saturating_sub(2)
        .max(1);
    let mut lines = Vec::new();
    let mut placed = Vec::new();
    let mut photos = Vec::new();
    let mut prev_day = None;
    let mut prev_sender = None;
    let query = open.search.as_ref().map_or("", |s| s.query.as_str());

    for (&id, msg) in &open.messages {
        let start = lines.len();
        let time = Local.timestamp_opt(i64::from(msg.date), 0).single();
        let day = time.map(|t| t.date_naive());
        if day != prev_day {
            if !lines.is_empty() {
                lines.push(Line::default());
            }
            let label = time.map_or(String::new(), |t| t.format(" %a %-d %b %Y ").to_string());
            lines.push(Line::from(label).fg(colors.muted).centered());
            prev_sender = None;
        }
        lines.push(Line::default());

        // Like Telegram: name only on the first of several messages in a row.
        let name = (show_names && prev_sender != Some(msg.sender)).then(|| {
            let count = colors.names.len() as i64;
            let color = colors.names[sender_key(msg.sender).rem_euclid(count) as usize];
            let mut name = names.get(msg.sender);
            if msg.outgoing {
                name.push_str(" (me)");
            }
            (name, color)
        });
        let meta = match msg.state {
            SendState::Sent => time.map_or(String::new(), |t| t.format("%H:%M").to_string()),
            SendState::Pending => "sending…".into(),
            SendState::Failed => "not sent".into(),
        };
        let bubble_start = lines.len();
        let photo_size = msg.preview.as_ref().map(|p| photo_cells(p, max_text, font));
        let has_name = name.is_some();
        let matches = search::find(&msg.text, query);
        let (rows, inner) = bubble(msg, name, photo_size, &meta, &matches, max_text, colors);
        if let (Some(photo), Some((cols, photo_rows))) = (&msg.preview, photo_size) {
            // Rows are right-aligned for own messages, so measure from the right.
            let bubble_x = if msg.outgoing { width - (inner + 2) } else { 0 };
            photos.push(PhotoSlot {
                line: bubble_start + usize::from(has_name),
                x: (bubble_x + 1) as u16,
                cols,
                rows: photo_rows,
                photo: photo.clone(),
            });
        }
        lines.extend(rows);
        placed.push(Placed {
            id,
            start,
            bubble_start,
            end: lines.len(),
        });
        prev_day = day;
        prev_sender = Some(msg.sender);
    }
    (lines, placed, photos)
}

fn sender_key(sender: Sender) -> i64 {
    match sender {
        Sender::User(id) | Sender::Chat(id) => id,
    }
}

/// One message as padded, colored lines. `meta` (the time, or the send status)
/// sits at the bottom right, on the last text line if it fits. `matches` are
/// byte ranges of the text to highlight for a search.
/// Returns the rows and the bubble's inner width. A photo gets `rows` blank
/// rows of `cols` width right under the name, for [`draw_photos`] to fill.
fn bubble(
    msg: &Msg,
    name: Option<(String, Color)>,
    photo: Option<(u16, u16)>,
    meta: &str,
    matches: &[Range<usize>],
    max_text: usize,
    colors: &Colors,
) -> (Vec<Line<'static>>, usize) {
    let (bg, meta_fg) = if msg.outgoing {
        (colors.own_bubble, colors.own_meta)
    } else {
        (colors.other_bubble, colors.other_meta)
    };
    // Stickers float on the pane, without a bubble behind them.
    let sticker = msg.preview.as_ref().is_some_and(|p| p.sticker);
    let style = if sticker {
        Style::new()
    } else {
        Style::new().fg(colors.fg).bg(bg)
    };
    let meta_color = match (msg.state, sticker) {
        (SendState::Failed, _) => colors.error,
        (_, true) => colors.muted,
        (_, false) => meta_fg,
    };
    let meta_style = style.fg(meta_color);
    let text = wrap(&msg.text, max_text);
    let meta_w = meta.width();
    let last_w = text.last().map_or(0, |(l, _)| l.width());
    let meta_inline = last_w + 1 + meta_w <= max_text;
    let found = style.patch(super::match_style(colors));
    let spans =
        |line: &str, start: usize| line_spans(line, start, &msg.link_ranges, matches, style, found);

    let mut inner = text
        .iter()
        .map(|(l, _)| l.width())
        .max()
        .unwrap_or(0)
        .max(meta_w);
    if meta_inline {
        inner = inner.max(last_w + 1 + meta_w);
    }
    let name = name.map(|(n, color)| (truncate(&n, max_text), color));
    if let Some((n, _)) = &name {
        inner = inner.max(n.width());
    }
    if let Some((cols, _)) = photo {
        inner = inner.max(usize::from(cols));
    }

    // Pads a row to the bubble width (one column of padding each side) and
    // puts own messages on the right.
    let row = |mut spans: Vec<Span<'static>>, used: usize| {
        spans.insert(0, Span::styled(" ", style));
        spans.push(Span::styled(" ".repeat(inner - used + 1), style));
        let line = Line::from(spans);
        if msg.outgoing {
            line.right_aligned()
        } else {
            line
        }
    };

    let mut out = Vec::new();
    if let Some((n, color)) = name {
        let w = n.width();
        out.push(row(vec![Span::styled(n, style.fg(color).bold())], w));
    }
    if let Some((cols, rows)) = photo {
        for _ in 0..rows {
            let blank = " ".repeat(usize::from(cols));
            out.push(row(vec![Span::styled(blank, style)], usize::from(cols)));
        }
    }
    let count = text.len();
    for (i, (line, start)) in text.into_iter().enumerate() {
        let w = line.width();
        let mut line_spans = spans(&line, start);
        if i + 1 == count && meta_inline {
            line_spans.push(Span::styled(" ".repeat(inner - w - meta_w), style));
            line_spans.push(Span::styled(meta.to_string(), meta_style));
            out.push(row(line_spans, inner));
        } else {
            out.push(row(line_spans, w));
        }
    }
    if !meta_inline {
        out.push(row(
            vec![
                Span::styled(" ".repeat(inner - meta_w), style),
                Span::styled(meta.to_string(), meta_style),
            ],
            inner,
        ));
    }
    (out, inner)
}

/// Word-wraps text to `width` columns, keeping the message's own line breaks.
/// Each line comes with the byte offset where it starts in `text`, so link
/// ranges can be matched up after wrapping.
fn wrap(text: &str, width: usize) -> Vec<(String, usize)> {
    let options = textwrap::Options::new(width).break_words(true);
    let mut out = Vec::new();
    let mut line_start = 0;
    for line in text.split('\n') {
        if line.is_empty() {
            out.push((String::new(), line_start));
        } else {
            let mut cursor = 0;
            for piece in textwrap::wrap(line, &options) {
                // Pieces are slices of `line` in order, with only the spaces
                // wrapping dropped in between.
                let at = line[cursor..]
                    .find(piece.as_ref())
                    .map_or(cursor, |i| cursor + i);
                cursor = (at + piece.len()).min(line.len());
                out.push((piece.into_owned(), line_start + at));
            }
        }
        line_start += line.len() + 1;
    }
    out
}

/// Splits one wrapped line into spans: `links` underlined, search `matches`
/// in `found`. Both are byte ranges of the whole text; the line starts at
/// byte `start`.
fn line_spans(
    line: &str,
    start: usize,
    links: &[Range<usize>],
    matches: &[Range<usize>],
    style: Style,
    found: Style,
) -> Vec<Span<'static>> {
    let end = start + line.len();
    // Every offset in the line where the style can change.
    let mut cuts = vec![0, line.len()];
    for range in links.iter().chain(matches) {
        for at in [range.start, range.end] {
            if at > start && at < end {
                cuts.push(at - start);
            }
        }
    }
    cuts.sort_unstable();
    cuts.dedup();
    let inside = |ranges: &[Range<usize>], at: usize| ranges.iter().any(|r| r.contains(&at));
    let mut spans: Vec<Span<'static>> = cuts
        .windows(2)
        .map(|w| {
            let at = start + w[0];
            let mut s = if inside(matches, at) { found } else { style };
            if inside(links, at) {
                s = s.underlined();
            }
            Span::styled(line[w[0]..w[1]].to_string(), s)
        })
        .collect();
    if spans.is_empty() {
        spans.push(Span::styled(String::new(), style));
    }
    spans
}

#[cfg(test)]
mod tests {
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;
    use ratatui_image::picker::Picker;

    use super::*;
    use crate::images::Key;
    use crate::theme::Theme;

    /// Halfblocks need no terminal query, and their cells are easy to assert on.
    fn images() -> Images {
        Images::new(
            Picker::halfblocks(),
            tokio::sync::mpsc::unbounded_channel().0,
        )
    }

    fn msg(outgoing: bool, date: i32, text: &str) -> Msg {
        Msg {
            sender: Sender::User(if outgoing { 1 } else { 2 }),
            outgoing,
            date,
            text: text.into(),
            preview: None,
            file: None,
            links: Vec::new(),
            link_ranges: Vec::new(),
            state: SendState::Sent,
        }
    }

    /// Renders a chat into a 60x24 buffer and returns its rows as strings.
    fn render(open: &mut OpenChat, focused: bool) -> Vec<String> {
        render_with(open, focused, &mut images())
    }

    fn render_with(open: &mut OpenChat, focused: bool, images: &mut Images) -> Vec<String> {
        let buf = render_buffer(open, focused, images);
        (0..buf.area.height)
            .map(|y| (0..buf.area.width).map(|x| buf[(x, y)].symbol()).collect())
            .collect()
    }

    fn render_buffer(
        open: &mut OpenChat,
        focused: bool,
        images: &mut Images,
    ) -> ratatui::buffer::Buffer {
        let users = HashMap::new();
        let chats = Chats::default();
        let names = Names {
            users: &users,
            chats: &chats,
        };
        let mut terminal = Terminal::new(TestBackend::new(60, 24)).unwrap();
        terminal
            .draw(|f| {
                let colors = Theme::default().colors();
                draw(f, f.area(), open, &names, images, focused, &colors)
            })
            .unwrap();
        terminal.backend().buffer().clone()
    }

    fn sample() -> OpenChat {
        let mut open = OpenChat::new(42);
        let day1 = 1_790_000_000;
        let day2 = day1 + 86_400;
        open.messages.insert(1, msg(false, day1, "hi there"));
        open.messages
            .insert(2, msg(true, day1 + 60, "hello from me"));
        open.messages.insert(
            3,
            msg(
                false,
                day2,
                "a much longer message that has to wrap onto more than one line inside its bubble",
            ),
        );
        open.messages.insert(4, msg(true, day2 + 60, "ok"));
        open
    }

    #[test]
    fn own_messages_sit_on_the_right_and_others_on_the_left() {
        let rows = render(&mut sample(), false);
        let row = |needle: &str| rows.iter().find(|r| r.contains(needle)).unwrap().clone();

        // Inner columns: border (1) + gutter (1) on each side.
        let mine = row("hello from me");
        let theirs = row("hi there");
        // Border, gutter, padding, then text.
        assert_eq!(
            theirs.chars().position(|c| c == 'h'),
            Some(3),
            "incoming bubble starts at the left edge"
        );
        let mine: Vec<char> = mine.chars().collect();
        let end = mine.len() - 2; // before gutter + border
        assert_eq!(mine[end - 1], ' ', "own bubble has right padding");
        assert!(mine[end - 2].is_ascii_digit(), "time is flush right");
    }

    #[test]
    fn pending_messages_show_sending() {
        let mut open = sample();
        open.messages.insert(
            5,
            Msg {
                state: SendState::Pending,
                ..msg(true, 1_790_086_500, "on its way")
            },
        );
        let rows = render(&mut open, false);
        let at = |needle: &str| rows.iter().position(|r| r.contains(needle)).unwrap();

        assert!(rows[at("on its way")].contains("sending…"));
    }

    #[test]
    fn group_names_mark_own_messages_with_me() {
        let users = HashMap::from([(1, "Eric".to_string()), (2, "Chardy".to_string())]);
        let chats = Chats::default();
        let names = Names {
            users: &users,
            chats: &chats,
        };
        let font = FontSize {
            width: 10,
            height: 20,
        };
        let colors = Theme::default().colors();
        let (lines, _, _) = layout(&sample(), &names, true, 58, font, &colors);
        let text: Vec<String> = lines
            .iter()
            .map(|l| l.spans.iter().map(|s| s.content.as_ref()).collect())
            .collect();
        assert!(text.iter().any(|l| l.contains("Eric (me)")));
        assert!(
            text.iter()
                .any(|l| l.contains("Chardy") && !l.contains("(me)"))
        );
    }

    #[test]
    fn names_show_once_per_run_and_not_in_channels() {
        let users = HashMap::from([(1, "Eric".to_string()), (2, "Chardy".to_string())]);
        let chats = Chats::default();
        let names = Names {
            users: &users,
            chats: &chats,
        };
        let font = FontSize {
            width: 10,
            height: 20,
        };
        let colors = Theme::default().colors();
        let mut open = sample();
        // A second message from Chardy right after the first.
        open.messages
            .insert(0, msg(false, 1_790_000_000 - 60, "first"));
        let text = |show_names| -> Vec<String> {
            let (lines, _, _) = layout(&open, &names, show_names, 58, font, &colors);
            lines
                .iter()
                .map(|l| l.spans.iter().map(|s| s.content.as_ref()).collect())
                .collect()
        };
        let chat = text(true);
        let count = |name: &str| chat.iter().filter(|l| l.contains(name)).count();
        // Chardy: "first" + "hi there", then after a day break. Eric: two runs.
        assert_eq!(count("Chardy"), 2, "once per run of messages");
        assert_eq!(count("Eric (me)"), 2);

        let channel = text(false);
        assert!(
            !channel
                .iter()
                .any(|l| l.contains("Chardy") || l.contains("Eric"))
        );
    }

    #[test]
    fn photos_show_a_placeholder_then_the_image() {
        let mut open = sample();
        let photo = Preview {
            file_id: 7,
            width: 800,
            height: 600,
            thumbnail: None,
            sticker: false,
        };
        open.messages.insert(
            5,
            Msg {
                preview: Some(photo.clone()),
                ..msg(true, 1_790_086_500, "look")
            },
        );
        let mut images = images();
        let rows = render_with(&mut open, false, &mut images);
        assert!(rows.iter().any(|r| r.contains("Loading…")));
        let caption = rows.iter().position(|r| r.contains("look")).unwrap();
        assert!(
            rows[caption - 1].contains("    "),
            "photo rows sit above the caption"
        );

        // 40 cols max, 800x600 at 10x20 px cells -> 40 x 15.
        let (cols, rows_) = photo_cells(&photo, 56, images.font_size());
        assert_eq!((cols, rows_), (40, 15));
        let red = image::RgbImage::from_pixel(80, 60, image::Rgb([255, 0, 0]));
        let key = Key {
            file_id: 7,
            cols,
            rows: rows_,
            thumbnail: false,
        };
        images.insert_ready(key, red.into());
        let buf = render_buffer(&mut open, false, &mut images);
        let red_cells = buf
            .content()
            .iter()
            .filter(|c| c.bg == Color::Rgb(255, 0, 0) || c.fg == Color::Rgb(255, 0, 0))
            .count();
        assert_eq!(red_cells, 40 * 15, "the whole 40x15 photo is drawn");
    }

    #[test]
    fn photos_cut_off_at_the_top_show_their_visible_rows() {
        let photo = Preview {
            file_id: 7,
            width: 800,
            height: 600,
            thumbnail: None,
            sticker: false,
        };
        let mut open = OpenChat::new(42);
        open.messages.insert(
            1,
            Msg {
                preview: Some(photo.clone()),
                ..msg(false, 1_790_000_000, "")
            },
        );
        for i in 2..6 {
            open.messages
                .insert(i, msg(i % 2 == 0, 1_790_000_000 + i as i32, "text"));
        }
        let mut images = images();
        let (cols, rows) = photo_cells(&photo, 56, images.font_size());
        let red = image::RgbImage::from_pixel(80, 60, image::Rgb([255, 0, 0]));
        let key = Key {
            file_id: 7,
            cols,
            rows,
            thumbnail: false,
        };
        images.insert_ready(key, red.into());

        // Following the newest message pushes the top of the photo off screen.
        let buf = render_buffer(&mut open, false, &mut images);
        let red_rows = (0..buf.area.height)
            .filter(|&y| (0..buf.area.width).any(|x| buf[(x, y)].bg == Color::Rgb(255, 0, 0)))
            .count();
        assert!(
            red_rows > 0 && red_rows < usize::from(rows),
            "partly visible: {red_rows} rows"
        );
    }

    #[test]
    fn wrapped_lines_know_where_they_start() {
        let text = "one two three\n\nfour";
        let lines = wrap(text, 8);
        let starts: Vec<(&str, usize)> = lines.iter().map(|(l, s)| (l.as_str(), *s)).collect();
        assert_eq!(
            starts,
            [("one two", 0), ("three", 8), ("", 14), ("four", 15)]
        );
        for (line, start) in &lines {
            assert_eq!(&text[*start..*start + line.len()], line);
        }
    }

    #[test]
    fn links_are_underlined_even_across_a_wrap() {
        let text = "see https://example.com/a/long/path ok";
        let link = 4..text.find(" ok").unwrap();
        let mut underlined = String::new();
        for (line, start) in wrap(text, 20) {
            let links = std::slice::from_ref(&link);
            for span in line_spans(&line, start, links, &[], Style::new(), Style::new()) {
                if span
                    .style
                    .add_modifier
                    .contains(ratatui::style::Modifier::UNDERLINED)
                {
                    underlined.push_str(&span.content);
                }
            }
        }
        assert_eq!(underlined, "https://example.com/a/long/path");
    }

    #[test]
    fn search_matches_are_highlighted_and_counted_in_the_title() {
        let mut open = sample();
        let mut search = crate::search::MessageSearch::new("HELLO".into());
        search.results = vec![2];
        search.current = Some(0);
        open.search = Some(search);
        let buf = render_buffer(&mut open, false, &mut images());
        let rows: Vec<String> = (0..buf.area.height)
            .map(|y| (0..buf.area.width).map(|x| buf[(x, y)].symbol()).collect())
            .collect();
        assert!(rows[0].contains("/HELLO 1 of 1"), "title: {}", rows[0]);

        let colors = Theme::default().colors();
        let lit: String = buf
            .content()
            .iter()
            .filter(|c| c.bg == colors.search)
            .map(|c| c.symbol())
            .collect();
        assert_eq!(lit, "hello", "only the match, in the message's own case");
    }

    #[test]
    fn a_match_inside_a_link_is_both_highlighted_and_underlined() {
        let text = "see https://hello.dev now";
        let link = 4..text.find(" now").unwrap();
        let found = Style::new().bg(Color::Yellow);
        let matches = crate::search::find(text, "hello");
        let spans = line_spans(text, 0, &[link], &matches, Style::new(), found);
        let parts: Vec<(&str, bool, bool)> = spans
            .iter()
            .map(|s| {
                let underlined = s
                    .style
                    .add_modifier
                    .contains(ratatui::style::Modifier::UNDERLINED);
                (
                    s.content.as_ref(),
                    s.style.bg == Some(Color::Yellow),
                    underlined,
                )
            })
            .collect();
        assert_eq!(
            parts,
            [
                ("see ", false, false),
                ("https://", false, true),
                ("hello", true, true),
                (".dev", false, true),
                (" now", false, false),
            ]
        );
    }

    #[test]
    fn stickers_have_no_bubble() {
        let mut open = OpenChat::new(42);
        let sticker = Preview {
            file_id: 3,
            width: 512,
            height: 512,
            thumbnail: None,
            sticker: true,
        };
        open.messages.insert(
            1,
            Msg {
                preview: Some(sticker.clone()),
                ..msg(true, 1_790_000_000, "")
            },
        );
        let mut images = images();
        // 512x512 at 10x20 px cells: 20 cols by 10 rows.
        assert_eq!(photo_cells(&sticker, 56, images.font_size()), (20, 10));
        let buf = render_buffer(&mut open, false, &mut images);
        let own = Theme::default().colors().own_bubble;
        assert!(
            buf.content().iter().all(|c| c.bg != own),
            "no bubble color anywhere"
        );
    }

    #[test]
    fn cursor_moves_scroll_the_view() {
        let mut open = OpenChat::new(42);
        for i in 0..40 {
            open.messages.insert(
                i,
                msg(
                    i % 2 == 0,
                    1_790_000_000 + i as i32,
                    &format!("message {i}"),
                ),
            );
        }
        let rows = render(&mut open, true);
        assert!(
            rows.iter().any(|r| r.contains("message 39")),
            "starts at the newest"
        );
        assert!(!rows.iter().any(|r| r.contains("message 0 ")));

        open.move_cursor(isize::MIN);
        let rows = render(&mut open, true);
        assert!(
            rows.iter().any(|r| r.contains("message 0 ")),
            "gg shows the oldest"
        );
        assert!(
            rows.iter()
                .any(|r| r.contains("▌") && r.contains("message 0 ")),
            "cursor marks it"
        );
    }
}

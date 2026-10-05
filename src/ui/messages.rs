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
use crate::messages::{
    Fetched, Msg, OpenChat, Preview, Replied, ReplyTo, ScrollAnchor, SendState, Sender,
};
use crate::reactions::{self, Reaction};
use crate::search;
use crate::settings::Settings;
use crate::theme::Colors;

/// Bubbles take at most this share of the pane width.
const BUBBLE_WIDTH_PERCENT: usize = 75;
/// Largest inline photo, in terminal cells.
const MAX_PHOTO_COLS: usize = 40;
const MAX_PHOTO_ROWS: usize = 16;
/// Stickers are smaller, as in Telegram.
const MAX_STICKER_COLS: usize = 20;
const MAX_STICKER_ROWS: usize = 10;
/// Least space between a message's last line and the time beside it.
const META_GAP: usize = 3;
/// Space between two reactions under a bubble.
const CHIP_GAP: usize = 1;

/// Resolves message senders to display names.
pub struct Names<'a> {
    pub users: &'a HashMap<i64, String>,
    pub chats: &'a Chats,
}

impl Names<'_> {
    pub(super) fn get(&self, sender: Sender) -> String {
        let name = match sender {
            Sender::User(id) => self.users.get(&id).cloned(),
            Sender::Chat(id) => self.chats.get(id).map(|c| c.title.clone()),
        };
        name.unwrap_or_else(|| "Unknown".into())
    }

    /// Who sent a message, with "(me)" on your own.
    pub(super) fn author(&self, sender: Sender, outgoing: bool) -> String {
        let mut name = self.get(sender);
        if outgoing {
            name.push_str(" (me)");
        }
        name
    }
}

/// The lines at the top of a reply: who it answers, then what they said.
struct Quote {
    name: String,
    /// For the bar and the name. `None` while the answered message loads or
    /// if it's gone, which draws them faded.
    color: Option<Color>,
    text: Option<String>,
}

/// What sits above a message's photo and text.
struct Header {
    name: Option<(String, Color)>,
    quote: Option<Quote>,
}

impl Header {
    fn rows(&self) -> usize {
        let quote = self
            .quote
            .as_ref()
            .map_or(0, |q| 1 + usize::from(q.text.is_some()));
        usize::from(self.name.is_some()) + quote
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
    settings: &Settings,
) {
    let colors = &settings.theme.colors();
    let chat = names.chats.get(open.chat_id);
    let mut title = vec![
        Span::from(format!(
            " {} ",
            names.chats.title(open.chat_id).unwrap_or_default()
        ))
        .style(super::title_style(names.chats, open.chat_id, colors)),
    ];
    if let Some(doing) = chat.and_then(|c| super::activity(c, names)) {
        title.push(Span::from(format!("· {doing} ")).fg(colors.activity));
    }
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
    let (lines, placed, photos) = layout(
        open,
        names,
        show_names,
        settings.block_gaps,
        body.width as usize,
        font,
        colors,
    );

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

    let gutters = [shift(left), shift(right)];
    // The message being answered or edited stays marked while writing.
    if let Some(reply) = &open.reply
        && let Some(target) = placed.iter().find(|p| p.id == reply.id)
    {
        mark(frame, gutters, target, top, colors.reply);
    }
    if let Some(editing) = &open.editing
        && let Some(target) = placed.iter().find(|p| p.id == editing.id)
    {
        mark(frame, gutters, target, top, colors.edit);
    }
    if focused && let Some(sel) = selected {
        mark(frame, gutters, sel, top, colors.accent);
    }
}

/// Bars in both gutters beside a message's bubble, for its visible rows.
fn mark(frame: &mut Frame, gutters: [Rect; 2], msg: &Placed, top: usize, color: Color) {
    let height = usize::from(gutters[0].height);
    let rows = msg.bubble_start.max(top)..msg.end.min(top + height);
    if rows.is_empty() {
        return;
    }
    let offset = (rows.start - top) as u16;
    for (gutter, bar) in gutters.into_iter().zip(["▌", "▐"]) {
        let area = Rect {
            y: gutter.y + offset,
            height: rows.len() as u16,
            ..gutter
        };
        frame.render_widget(
            Paragraph::new(vec![Line::from(bar); rows.len()]).fg(color),
            area,
        );
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

/// Lays out every loaded message. With `gaps`, messages in a block have a
/// row of their bubble's background between them.
fn layout(
    open: &OpenChat,
    names: &Names,
    show_names: bool,
    gaps: bool,
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
    let query = open.search.as_ref().map_or("", |s| s.query.as_str());

    // Measure every bubble first, so the ones in a block can share a width.
    let mut measured = Vec::with_capacity(open.messages.len());
    let mut prev_day = None;
    let mut prev_sender = None;
    let mut prev_sticker = false;
    let messages: Vec<(i64, &Msg)> = open.messages.iter().map(|(&id, m)| (id, m)).collect();
    let albums = albums(&messages);
    for ((id, msg), album) in messages.into_iter().zip(albums) {
        let time = Local.timestamp_opt(i64::from(msg.date), 0).single();
        let day = time.map(|t| t.date_naive());
        let separator = (day != prev_day).then(|| {
            prev_sender = None;
            time.map_or(String::new(), |t| t.format(" %a %-d %b %Y ").to_string())
        });

        // Like Telegram: name only on the first of several messages in a row.
        let name = (show_names && prev_sender != Some(msg.sender)).then(|| {
            (
                names.author(msg.sender, msg.outgoing),
                name_color(msg.sender, colors),
            )
        });
        let header = Header {
            name,
            // Every photo of an album answers the same message; it's said once.
            quote: msg
                .reply_to
                .as_ref()
                .filter(|_| album.is_none_or(|a| a.first))
                .map(|reply| quote(open, id, reply, names, colors)),
        };
        // An album's caption goes under its last photo, as in Telegram,
        // though it belongs to the photo that was sent with it.
        let caption = match album {
            Some(InAlbum {
                caption: Some((holder_id, holder)),
                last,
                ..
            }) => {
                if last {
                    Some(holder)
                } else {
                    (holder_id != id).then_some(msg)
                }
            }
            _ => Some(msg),
        };
        let edited = msg.edited || caption.is_some_and(|c| c.edited);
        // An album has one time, at the bottom; a photo still on its way, or
        // that didn't make it, says so under itself.
        let meta = match msg.state {
            _ if msg.state == SendState::Sent && album.is_some_and(|a| !a.last) => None,
            SendState::Sent => Some(time.map_or(String::new(), |t| {
                let time = t.format("%H:%M");
                if edited {
                    format!("edited {time}")
                } else {
                    time.to_string()
                }
            })),
            SendState::Pending => Some(match open.upload_progress(msg) {
                Some(done) => format!("sending {done}%"),
                None => "sending…".into(),
            }),
            SendState::Failed => Some("not sent".into()),
        };
        let photo = msg.preview.as_ref().map(|p| photo_cells(p, max_text, font));
        let matches = caption.map_or(Vec::new(), |c| search::find(&c.text, query));
        // An album's reactions go under it, where its time is, whichever
        // photos they're on.
        let reactions = match album {
            Some(a) if a.last => reactions::merge(
                open.messages
                    .values()
                    .filter(|m| m.album == msg.album)
                    .map(|m| m.reactions.as_slice()),
            ),
            Some(_) => Vec::new(),
            None => msg.reactions.clone(),
        };
        let chips = chip_rows(&reactions, max_text);
        let bubble = Bubble::new(msg, caption, header, photo, meta, chips, matches, max_text);
        // Messages in a row from one sender form a block, with no gap between
        // them. Not in channels, where every post has the same sender, and
        // not for stickers, which have no bubble to join up.
        let sticker = bubble.sticker();
        let joined = show_names && prev_sender == Some(msg.sender) && !sticker && !prev_sticker;
        measured.push(Measured {
            id,
            separator,
            joined,
            bubble,
        });
        prev_day = day;
        prev_sender = Some(msg.sender);
        prev_sticker = sticker;
    }

    // Every bubble in a block is as wide as the widest, so they line up.
    let mut widths = Vec::with_capacity(measured.len());
    for block in measured.chunk_by(|_, next| next.joined) {
        let widest = block.iter().map(|m| m.bubble.width).max().unwrap_or(0);
        widths.extend(std::iter::repeat_n(widest, block.len()));
    }

    let mut lines = Vec::new();
    let mut placed = Vec::new();
    let mut photos = Vec::new();
    for (m, inner) in measured.into_iter().zip(widths) {
        let start = lines.len();
        if let Some(label) = m.separator {
            if !lines.is_empty() {
                lines.push(Line::default());
            }
            lines.push(Line::from(label).fg(colors.muted).centered());
        }
        // Inside a block the gap keeps the bubble's background, so the
        // messages read as one block but still apart.
        if !m.joined {
            lines.push(Line::default());
        } else if gaps {
            lines.push(m.bubble.gap(inner, colors));
        }
        let bubble_start = lines.len();
        let msg = m.bubble.msg;
        if let (Some(photo), Some((cols, photo_rows))) = (&msg.preview, m.bubble.photo) {
            // Rows are right-aligned for own messages, so measure from the right.
            let bubble_x = if msg.outgoing {
                width.saturating_sub(inner + 2)
            } else {
                0
            };
            photos.push(PhotoSlot {
                line: bubble_start + m.bubble.header.rows(),
                x: (bubble_x + 1) as u16,
                cols,
                rows: photo_rows,
                photo: photo.clone(),
            });
        }
        lines.extend(m.bubble.rows(inner, colors));
        placed.push(Placed {
            id: m.id,
            start,
            bubble_start,
            end: lines.len(),
        });
    }
    (lines, placed, photos)
}

/// Where a message sits in an album, which is drawn as one bubble: the
/// photos in order, then the caption and the time.
#[derive(Clone, Copy)]
struct InAlbum<'a> {
    first: bool,
    last: bool,
    /// The one photo sent with a caption, and its id. `None` if none or
    /// several were, which then show under their own photos.
    caption: Option<(i64, &'a Msg)>,
}

/// For each message, where it sits in an album of photos or videos, or
/// `None`. Albums of files show each file's caption under it, like Telegram.
fn albums<'a>(messages: &[(i64, &'a Msg)]) -> Vec<Option<InAlbum<'a>>> {
    let mut out = Vec::with_capacity(messages.len());
    for run in messages.chunk_by(|(_, a), (_, b)| a.album != 0 && a.album == b.album) {
        let media = run.len() > 1
            && run
                .iter()
                .all(|(_, m)| m.preview.as_ref().is_some_and(|p| !p.sticker));
        if !media {
            out.extend(run.iter().map(|_| None));
            continue;
        }
        // The caption as sent, not a video's length.
        let mut captioned = run.iter().filter(|(_, m)| !m.source_text.is_empty());
        let caption = match (captioned.next(), captioned.next()) {
            (Some(&(id, msg)), None) => Some((id, msg)),
            _ => None,
        };
        out.extend((0..run.len()).map(|i| {
            Some(InAlbum {
                first: i == 0,
                last: i + 1 == run.len(),
                caption,
            })
        }));
    }
    out
}

/// A message measured in the first pass of [`layout`].
struct Measured<'a> {
    id: i64,
    /// The date to show above it, when it's the first message of a day.
    separator: Option<String>,
    /// Whether it continues the block above, with no gap between.
    joined: bool,
    bubble: Bubble<'a>,
}

/// Each sender keeps one of the theme's name colors.
fn name_color(sender: Sender, colors: &Colors) -> Color {
    let id = match sender {
        Sender::User(id) | Sender::Chat(id) => id,
    };
    colors.names[id.rem_euclid(colors.names.len() as i64) as usize]
}

/// What reply `id` answers: the loaded message if it's there, else what
/// TDLib sent for it. A quote the sender picked replaces the message's text.
fn quote(open: &OpenChat, id: i64, reply: &ReplyTo, names: &Names, colors: &Colors) -> Quote {
    let fetched = open.replied.get(&id);
    let replied = reply
        .message_id
        .and_then(|answered| open.messages.get_key_value(&answered))
        .map(|(&answered, msg)| Replied::new(answered, msg))
        .or_else(|| match fetched {
            Some(Fetched::Found(replied)) => Some(replied.clone()),
            _ => None,
        });
    match replied {
        Some(replied) => Quote {
            name: names.author(replied.sender, replied.outgoing),
            color: Some(name_color(replied.sender, colors)),
            text: Some(reply.quote.clone().unwrap_or(replied.snippet)),
        },
        None => Quote {
            name: match fetched {
                Some(Fetched::Missing) => "Deleted message".into(),
                _ => "Loading…".into(),
            },
            color: None,
            text: reply.quote.clone(),
        },
    }
}

/// A reaction under a bubble: the emoji and how many added it.
struct Chip {
    label: String,
    /// You added it.
    chosen: bool,
}

impl Chip {
    /// Columns it takes, with a space of padding each side.
    fn width(&self) -> usize {
        self.label.width() + 2
    }
}

/// Columns a row of reactions takes.
fn chips_width(row: &[Chip]) -> usize {
    let gaps = row.len().saturating_sub(1) * CHIP_GAP;
    row.iter().map(Chip::width).sum::<usize>() + gaps
}

/// Reactions in rows that fit in `max` columns.
fn chip_rows(reactions: &[Reaction], max: usize) -> Vec<Vec<Chip>> {
    let mut rows: Vec<Vec<Chip>> = Vec::new();
    let mut used = 0;
    for reaction in reactions {
        let chip = Chip {
            label: format!(
                "{} {}",
                reaction.kind.label(),
                reactions::count_label(reaction.count)
            ),
            chosen: reaction.chosen,
        };
        let w = chip.width();
        match rows.last_mut() {
            Some(row) if used + CHIP_GAP + w <= max => {
                used += CHIP_GAP + w;
                row.push(chip);
            }
            _ => {
                used = w;
                rows.push(vec![chip]);
            }
        }
    }
    rows
}

/// Where a bubble's time goes.
#[derive(Clone, Copy, PartialEq, Eq)]
enum MetaAt {
    /// After the last line of text.
    Text,
    /// After the last row of reactions.
    Reactions,
    /// On a row of its own.
    Own,
}

/// One message's bubble, measured but not yet drawn, so the bubbles in a
/// block can all be drawn as wide as the widest.
struct Bubble<'a> {
    msg: &'a Msg,
    /// Byte ranges of the text that are links.
    links: &'a [Range<usize>],
    header: Header,
    /// Columns and rows of the photo.
    photo: Option<(u16, u16)>,
    /// The time, or the send status. `None` inside an album, which has one
    /// time at the bottom.
    meta: Option<String>,
    /// Reactions, in rows, under the text.
    chips: Vec<Vec<Chip>>,
    /// Byte ranges of the text to highlight for a search.
    matches: Vec<Range<usize>>,
    /// Wrapped lines, each with the byte offset where it starts in the text.
    text: Vec<(String, usize)>,
    meta_at: MetaAt,
    /// Columns the contents need, inside the padding.
    width: usize,
}

impl<'a> Bubble<'a> {
    /// Wraps the text of `caption` (usually `msg` itself, but an album's
    /// caption goes with its last photo) and shortens the header to fit
    /// `max_text` columns.
    #[allow(clippy::too_many_arguments)]
    fn new(
        msg: &'a Msg,
        caption: Option<&'a Msg>,
        header: Header,
        photo: Option<(u16, u16)>,
        meta: Option<String>,
        chips: Vec<Vec<Chip>>,
        matches: Vec<Range<usize>>,
        max_text: usize,
    ) -> Self {
        let source = caption.map_or("", |c| c.text.as_str());
        // A photo inside an album has no row under it at all, and one with
        // reactions has its time beside them.
        let text = if source.is_empty() && (meta.is_none() || !chips.is_empty()) {
            Vec::new()
        } else {
            wrap(source, max_text)
        };
        let meta_w = meta.as_ref().map_or(0, |m| m.width());
        // The time goes beside the last row of reactions, else the last line
        // of text, if it fits.
        let last_w = match chips.last() {
            Some(row) => chips_width(row),
            None => text.last().map_or(0, |(l, _)| l.width()),
        };
        let meta_at = match () {
            _ if meta.is_none() || last_w + META_GAP + meta_w > max_text => MetaAt::Own,
            _ if chips.is_empty() => MetaAt::Text,
            _ => MetaAt::Reactions,
        };

        let mut width = text
            .iter()
            .map(|(l, _)| l.width())
            .chain(chips.iter().map(|row| chips_width(row)))
            .max()
            .unwrap_or(0)
            .max(meta_w);
        if meta_at != MetaAt::Own {
            width = width.max(last_w + META_GAP + meta_w);
        }
        let name = header
            .name
            .map(|(n, color)| (truncate(&n, max_text), color));
        if let Some((n, _)) = &name {
            width = width.max(n.width());
        }
        // The quote's bar takes two columns.
        let quote = header.quote.map(|q| Quote {
            name: truncate(&q.name, max_text.saturating_sub(2)),
            text: q.text.map(|t| truncate(&t, max_text.saturating_sub(2))),
            ..q
        });
        if let Some(q) = &quote {
            let text_w = q.text.as_ref().map_or(0, |t| t.width());
            width = width.max(2 + q.name.width().max(text_w));
        }
        if let Some((cols, _)) = photo {
            width = width.max(usize::from(cols));
        }
        Bubble {
            msg,
            links: caption.map_or(&[], |c| c.link_ranges.as_slice()),
            header: Header { name, quote },
            photo,
            meta,
            chips,
            matches,
            text,
            meta_at,
            width,
        }
    }

    /// Stickers float on the pane, without a bubble behind them.
    fn sticker(&self) -> bool {
        self.msg.preview.as_ref().is_some_and(|p| p.sticker)
    }

    /// A blank row in the bubble's background, `inner` columns wide inside
    /// the padding: the gap above it when it continues a block.
    fn gap(&self, inner: usize, colors: &Colors) -> Line<'static> {
        let (bg, _) = bubble_colors(self.msg.outgoing, colors);
        let row = " ".repeat(inner.max(self.width) + 2);
        let line = Line::from(Span::styled(row, Style::new().bg(bg)));
        if self.msg.outgoing {
            line.right_aligned()
        } else {
            line
        }
    }

    /// The message as padded, colored lines, `inner` columns wide inside the
    /// padding (at least its own `width`). Reactions go under the text, and
    /// `meta` at the bottom right, beside the last row if it fits. A photo
    /// gets blank rows right under the header, for [`draw_photos`] to fill.
    fn rows(self, inner: usize, colors: &Colors) -> Vec<Line<'static>> {
        let sticker = self.sticker();
        let Bubble {
            msg,
            links,
            header,
            photo,
            meta,
            chips,
            matches,
            text,
            meta_at,
            width,
        } = self;
        let inner = inner.max(width);
        let (bg, meta_fg) = bubble_colors(msg.outgoing, colors);
        let style = if sticker {
            Style::new()
        } else {
            Style::new().fg(colors.fg).bg(bg)
        };
        // Secondary text that still reads on the bubble.
        let faded = if sticker { colors.muted } else { meta_fg };
        let meta_color = match msg.state {
            SendState::Failed => colors.error,
            _ => faded,
        };
        let meta_style = style.fg(meta_color);
        let meta_w = meta.as_ref().map_or(0, |m| m.width());
        let found = style.patch(super::match_style(colors));
        let spans =
            |line: &str, start: usize| line_spans(line, start, links, &matches, style, found);

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
        if let Some((n, color)) = header.name {
            let w = n.width();
            out.push(row(vec![Span::styled(n, style.fg(color).bold())], w));
        }
        if let Some(q) = header.quote {
            let accent = style.fg(q.color.unwrap_or(faded));
            let bar = || Span::styled("▎ ", accent);
            let w = q.name.width();
            out.push(row(vec![bar(), Span::styled(q.name, accent.bold())], 2 + w));
            if let Some(text) = q.text {
                let w = text.width();
                out.push(row(vec![bar(), Span::styled(text, style.fg(faded))], 2 + w));
            }
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
            if i + 1 == count
                && meta_at == MetaAt::Text
                && let Some(meta) = &meta
            {
                line_spans.push(Span::styled(" ".repeat(inner - w - meta_w), style));
                line_spans.push(Span::styled(meta.clone(), meta_style));
                out.push(row(line_spans, inner));
            } else {
                out.push(row(line_spans, w));
            }
        }
        let chip_style = |chosen| {
            if chosen {
                Style::new().fg(colors.bg).bg(colors.your_reaction)
            } else if msg.outgoing {
                Style::new().fg(colors.fg).bg(colors.own_reaction)
            } else {
                Style::new().fg(colors.fg).bg(colors.other_reaction)
            }
        };
        let count = chips.len();
        for (i, chips) in chips.into_iter().enumerate() {
            let w = chips_width(&chips);
            let mut spans = Vec::new();
            for (j, chip) in chips.into_iter().enumerate() {
                if j > 0 {
                    spans.push(Span::styled(" ".repeat(CHIP_GAP), style));
                }
                spans.push(Span::styled(
                    format!(" {} ", chip.label),
                    chip_style(chip.chosen),
                ));
            }
            if i + 1 == count
                && meta_at == MetaAt::Reactions
                && let Some(meta) = &meta
            {
                spans.push(Span::styled(" ".repeat(inner - w - meta_w), style));
                spans.push(Span::styled(meta.clone(), meta_style));
                out.push(row(spans, inner));
            } else {
                out.push(row(spans, w));
            }
        }
        if meta_at == MetaAt::Own
            && let Some(meta) = meta
        {
            out.push(row(
                vec![
                    Span::styled(" ".repeat(inner - meta_w), style),
                    Span::styled(meta, meta_style),
                ],
                inner,
            ));
        }
        out
    }
}

/// A bubble's background and the color of its time and send status.
fn bubble_colors(outgoing: bool, colors: &Colors) -> (Color, Color) {
    if outgoing {
        (colors.own_bubble, colors.own_meta)
    } else {
        (colors.other_bubble, colors.other_meta)
    }
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
    use crate::messages::Editable;
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
            source_text: text.into(),
            preview: None,
            file: None,
            links: Vec::new(),
            link_ranges: Vec::new(),
            state: SendState::Sent,
            reply_to: None,
            editable: Editable::Text,
            formatted: false,
            edited: false,
            album: 0,
            reactions: Vec::new(),
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
        render_in(open, &Chats::default(), focused, images)
    }

    fn render_in(
        open: &mut OpenChat,
        chats: &Chats,
        focused: bool,
        images: &mut Images,
    ) -> ratatui::buffer::Buffer {
        let users = HashMap::new();
        let names = Names {
            users: &users,
            chats,
        };
        let mut terminal = Terminal::new(TestBackend::new(60, 24)).unwrap();
        terminal
            .draw(|f| {
                draw(
                    f,
                    f.area(),
                    open,
                    &names,
                    images,
                    focused,
                    &Settings::default(),
                )
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
    fn the_time_keeps_its_distance_from_the_text() {
        let rows = render(&mut sample(), false);
        let row = rows.iter().find(|r| r.contains("hi there")).unwrap();
        let after = &row[row.find("hi there").unwrap() + "hi there".len()..];
        assert!(
            after.starts_with("   ") && after[3..].starts_with(char::is_numeric),
            "{row}"
        );
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
        let (lines, _, _) = layout(&sample(), &names, true, true, 58, font, &colors);
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
            let (lines, _, _) = layout(&open, &names, show_names, true, 58, font, &colors);
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
    fn the_title_says_when_the_other_person_is_typing() {
        let mut chats = Chats::default();
        chats.add_local(42, "Chardy", None).is_private = true;
        let typing = tdlib_rs::enums::ChatAction::Typing;
        let chardy =
            tdlib_rs::enums::MessageSender::User(tdlib_rs::types::MessageSenderUser { user_id: 2 });
        chats.set_action(42, &chardy, &typing);
        let buf = render_in(&mut sample(), &chats, false, &mut images());
        let title: String = (0..buf.area.width).map(|x| buf[(x, 0)].symbol()).collect();
        assert!(title.contains(" Chardy · typing… "), "{title}");
    }

    #[test]
    fn messages_in_a_row_from_one_sender_form_one_even_block() {
        let mut open = OpenChat::new(42);
        let day = 1_790_000_000;
        open.messages.insert(1, msg(false, day, "short"));
        open.messages
            .insert(2, msg(false, day + 60, "a somewhat longer message"));
        open.messages
            .insert(3, msg(true, day + 120, "a longer one of mine"));
        open.messages.insert(4, msg(true, day + 180, "ok"));
        let colors = Theme::default().colors();
        let buf = render_buffer(&mut open, false, &mut images());
        let row = |needle: &str| {
            (0..buf.area.height)
                .find(|&y| {
                    let text: String = (0..buf.area.width).map(|x| buf[(x, y)].symbol()).collect();
                    text.contains(needle)
                })
                .unwrap()
        };
        // Columns painted with a bubble's background on row `y`.
        let painted = |y: u16, bg: Color| -> Vec<u16> {
            (0..buf.area.width)
                .filter(|&x| buf[(x, y)].bg == bg)
                .collect()
        };

        let (short, longer) = (row("short"), row("somewhat"));
        assert_eq!(longer, short + 2, "a row between them");
        let bubble = painted(longer, colors.other_bubble);
        assert_eq!(
            painted(short, colors.other_bubble),
            bubble,
            "as wide as the widest"
        );
        assert_eq!(
            painted(short + 1, colors.other_bubble),
            bubble,
            "the row between keeps the background"
        );

        let (mine, ok) = (row("one of mine"), row("ok "));
        assert!(
            painted(longer + 1, colors.other_bubble).is_empty(),
            "a blank gap between blocks"
        );
        assert_eq!(ok, mine + 2);
        let bubble = painted(ok, colors.own_bubble);
        assert_eq!(painted(mine, colors.own_bubble), bubble);
        assert_eq!(painted(mine + 1, colors.own_bubble), bubble);
    }

    #[test]
    fn the_gap_inside_a_block_can_be_turned_off() {
        let users = HashMap::new();
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
        let mut open = OpenChat::new(42);
        open.messages.insert(1, msg(false, 1_790_000_000, "one"));
        open.messages.insert(2, msg(false, 1_790_000_060, "two"));
        let rows = |gaps| layout(&open, &names, true, gaps, 58, font, &colors).0.len();
        assert_eq!(rows(true), rows(false) + 1);
    }

    #[test]
    fn an_album_is_one_bubble_with_the_caption_and_time_under_the_last_photo() {
        let users = HashMap::new();
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
        let photo = |file_id, caption: &str| Msg {
            preview: Some(Preview {
                file_id,
                width: 800,
                height: 600,
                thumbnail: None,
                sticker: false,
            }),
            album: 5,
            ..msg(true, 1_790_000_000, caption)
        };
        let mut open = OpenChat::new(42);
        open.messages.insert(1, photo(7, "Sunrise at the top"));
        open.messages.insert(2, photo(8, ""));
        open.messages.insert(3, photo(9, ""));
        open.messages
            .insert(4, msg(true, 1_790_000_000, "and a text after"));
        let (lines, placed, photos) = layout(&open, &names, true, true, 58, font, &colors);
        let text: Vec<String> = lines
            .iter()
            .map(|l| l.spans.iter().map(|s| s.content.as_ref()).collect())
            .collect();
        let time = Local
            .timestamp_opt(1_790_000_000, 0)
            .unwrap()
            .format("%H:%M")
            .to_string();

        assert_eq!(photos.len(), 3);
        let caption = text.iter().position(|l| l.contains("Sunrise")).unwrap();
        let last = &photos[2];
        assert_eq!(
            caption,
            last.line + usize::from(last.rows),
            "right under the last photo"
        );
        assert!(
            placed[2].start <= caption && caption < placed[2].end,
            "drawn with the last photo"
        );
        assert_eq!(
            placed[0].end,
            photos[0].line + usize::from(photos[0].rows),
            "nothing under the first"
        );
        assert!(
            text[caption].contains(&time),
            "the time once, after the caption"
        );
        assert_eq!(
            text.iter().filter(|l| l.contains(&time)).count(),
            2,
            "and once for the text after"
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
            avatar: false,
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
            avatar: false,
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
    fn replies_show_who_and_what_they_answer() {
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
        let reply = |message_id, quote: Option<&str>| {
            Some(ReplyTo {
                message_id,
                quote: quote.map(Into::into),
            })
        };
        let at = 1_790_086_500;
        let mut open = sample();
        // Answers "hi there", which is loaded.
        open.messages.insert(
            5,
            Msg {
                reply_to: reply(Some(1), None),
                ..msg(true, at, "yes!")
            },
        );
        // Quotes part of a message TDLib says is gone.
        open.messages.insert(
            6,
            Msg {
                reply_to: reply(Some(0), Some("the plan")),
                ..msg(false, at, "and you?")
            },
        );
        open.replied.insert(6, Fetched::Missing);
        // A photo answering a message in another chat, not fetched yet.
        open.messages.insert(
            7,
            Msg {
                reply_to: reply(None, None),
                preview: Some(Preview {
                    file_id: 7,
                    width: 800,
                    height: 600,
                    thumbnail: None,
                    sticker: false,
                }),
                ..msg(false, at, "")
            },
        );

        let (lines, _, photos) = layout(&open, &names, true, true, 58, font, &colors);
        let text: Vec<String> = lines
            .iter()
            .map(|l| l.spans.iter().map(|s| s.content.as_ref()).collect())
            .collect();
        let at = |needle: &str| text.iter().position(|l| l.contains(needle)).unwrap();

        let yes = at("yes!");
        assert!(text[yes - 2].contains("▎ Chardy"), "{}", text[yes - 2]);
        assert!(text[yes - 1].contains("▎ hi there"), "{}", text[yes - 1]);
        let bar = &lines[yes - 2].spans[1];
        assert_eq!(bar.content, "▎ ");
        assert_eq!(bar.style.fg, Some(name_color(Sender::User(2), &colors)));

        let and_you = at("and you?");
        assert!(text[and_you - 2].contains("▎ Deleted message"));
        assert!(
            text[and_you - 1].contains("▎ the plan"),
            "the quote, not the text"
        );
        let faded = &lines[and_you - 2].spans[1];
        assert_eq!(faded.style.fg, Some(colors.other_meta));

        assert_eq!(
            photos[0].line,
            at("▎ Loading…") + 1,
            "the photo goes under the quote"
        );
    }

    #[test]
    fn the_message_being_replied_to_stays_marked_while_typing() {
        let mut open = sample();
        open.reply = Some(crate::messages::Replied::new(1, &open.messages[&1]));
        // Typing the reply: the message pane isn't focused.
        let buf = render_buffer(&mut open, false, &mut images());
        let colors = Theme::default().colors();
        let y = (0..buf.area.height)
            .find(|&y| {
                let row: String = (0..buf.area.width).map(|x| buf[(x, y)].symbol()).collect();
                row.contains("hi there")
            })
            .unwrap();
        let (left, right) = (buf[(1, y)].clone(), buf[(buf.area.width - 2, y)].clone());
        assert_eq!((left.symbol(), left.fg), ("▌", colors.reply));
        assert_eq!((right.symbol(), right.fg), ("▐", colors.reply));

        open.reply = None;
        let buf = render_buffer(&mut open, false, &mut images());
        assert_eq!(buf[(1, y)].symbol(), " ", "no marker without a reply");
    }

    #[test]
    fn edited_messages_say_so_and_the_one_being_edited_is_marked() {
        let mut open = sample();
        open.set_edited(2);
        open.editing = Some(crate::messages::Editing {
            id: 2,
            snippet: "hello from me".into(),
            editable: Editable::Text,
            draft: String::new(),
            reply: None,
            attachments: Vec::new(),
        });
        let buf = render_buffer(&mut open, false, &mut images());
        let colors = Theme::default().colors();
        let (y, row) = (0..buf.area.height)
            .map(|y| {
                let row: String = (0..buf.area.width).map(|x| buf[(x, y)].symbol()).collect();
                (y, row)
            })
            .find(|(_, row)| row.contains("hello from me"))
            .unwrap();
        assert!(row.contains("edited "), "{row}");
        let left = &buf[(1, y)];
        assert_eq!((left.symbol(), left.fg), ("▌", colors.edit));
    }

    fn reaction(emoji: &str, count: i32, chosen: bool) -> Reaction {
        Reaction {
            kind: crate::reactions::ReactionKind::Emoji(emoji.into()),
            count,
            chosen,
        }
    }

    #[test]
    fn reactions_sit_under_the_text_with_the_time_beside_them_and_yours_filled_in() {
        let mut open = sample();
        open.messages.get_mut(&1).unwrap().reactions =
            vec![reaction("👍", 3, true), reaction("❤", 1, false)];
        let buf = render_buffer(&mut open, false, &mut images());
        let colors = Theme::default().colors();
        let rows: Vec<String> = (0..buf.area.height)
            .map(|y| (0..buf.area.width).map(|x| buf[(x, y)].symbol()).collect())
            .collect();
        let text = rows.iter().position(|r| r.contains("hi there")).unwrap();
        let time = Local
            .timestamp_opt(1_790_000_000, 0)
            .unwrap()
            .format("%H:%M")
            .to_string();
        let chips = &rows[text + 1];
        assert!(!rows[text].contains(&time), "{}", rows[text]);
        assert!(chips.contains('👍') && chips.contains(&time), "{chips}");

        let y = (text + 1) as u16;
        let at = |symbol: &str| (0..buf.area.width).find(|&x| buf[(x, y)].symbol() == symbol);
        let thumbs = at("👍").unwrap();
        assert_eq!(buf[(thumbs, y)].bg, colors.your_reaction);
        let heart = at("❤\u{FE0F}").expect("❤ drawn two columns wide");
        assert_eq!(buf[(heart, y)].bg, colors.other_reaction);
        assert_eq!(buf[(heart + 3, y)].symbol(), "1");
    }

    #[test]
    fn reactions_wrap_onto_more_rows_when_they_dont_fit() {
        // Each takes " 👍 12 ", seven columns, and two fit with the gap.
        let list = [
            reaction("👍", 12, false),
            reaction("🔥", 12, false),
            reaction("🎉", 12, false),
        ];
        let rows = chip_rows(&list, 15);
        assert_eq!(rows.iter().map(Vec::len).collect::<Vec<_>>(), [2, 1]);
    }

    #[test]
    fn an_album_shows_the_reactions_of_all_its_photos_once_under_the_last() {
        let mut open = OpenChat::new(42);
        for id in [1, 2] {
            let mut photo = msg(false, 1_790_000_000, "");
            photo.album = 7;
            photo.preview = Some(Preview {
                file_id: id as i32,
                width: 100,
                height: 100,
                thumbnail: None,
                sticker: false,
            });
            photo.reactions = vec![reaction("🔥", 1, false)];
            open.messages.insert(id, photo);
        }
        let rows = render(&mut open, false);
        let fire: Vec<&String> = rows.iter().filter(|r| r.contains('🔥')).collect();
        assert_eq!(fire.len(), 1, "{rows:#?}");
        let words: Vec<&str> = fire[0].split_whitespace().collect();
        assert!(words.windows(2).any(|w| w == ["🔥", "2"]), "{}", fire[0]);
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

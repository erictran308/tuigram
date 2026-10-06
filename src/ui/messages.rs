//! The message pane: chat bubbles, yours on the right, newest at the bottom.

use std::collections::HashMap;
use std::ops::Range;

use chrono::{Datelike, Local, TimeZone};
use ratatui::Frame;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Color, Style, Stylize};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Paragraph};
use ratatui_image::FontSize;
use ratatui_image::sliced::{SignedPosition, SlicedImage};
use unicode_width::UnicodeWidthStr;

use super::truncate;
use crate::chats::{Chats, Presence, Seen};
use crate::images::Images;
use crate::messages::{
    Card, Fetched, Format, Msg, OpenChat, Origin, Preview, Replied, ReplyTo, SPOILER, ScrollAnchor,
    SendState, Sender, Styled,
};
use crate::reactions::{self, Reaction};
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

    /// Who a forwarded message first came from: "Alice", or "News (Bob)"
    /// for a channel post Bob signed.
    pub(super) fn origin(&self, origin: &Origin) -> String {
        match origin {
            Origin::User(id) => self.get(Sender::User(*id)),
            Origin::Hidden(name) => name.clone(),
            Origin::Chat { chat_id, signature } => {
                let title = self.get(Sender::Chat(*chat_id));
                if signature.is_empty() {
                    title
                } else {
                    format!("{title} ({signature})")
                }
            }
        }
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
    /// "Forwarded from Alice".
    forward: Option<String>,
    quote: Option<Quote>,
}

impl Header {
    fn rows(&self) -> usize {
        let quote = self
            .quote
            .as_ref()
            .map_or(0, |q| 1 + usize::from(q.text.is_some()));
        usize::from(self.name.is_some()) + usize::from(self.forward.is_some()) + quote
    }
}

/// Where one message landed in the laid-out lines.
#[derive(Clone)]
struct Placed {
    id: i64,
    /// First line, including the date separator and gap above the bubble.
    start: usize,
    bubble_start: usize,
    end: usize,
}

/// Rows reserved inside a bubble where a photo gets drawn after the text.
pub(super) struct PhotoSlot {
    /// First line of the reserved rows.
    pub line: usize,
    /// Column where the photo starts, from the left of the message area.
    pub x: u16,
    pub cols: u16,
    pub rows: u16,
    pub photo: Preview,
}

#[allow(clippy::too_many_arguments)]
pub fn draw(
    frame: &mut Frame,
    area: Rect,
    open: &mut OpenChat,
    names: &Names,
    images: &mut Images,
    focused: bool,
    covered: bool,
    colors: &Colors,
    block_gaps: bool,
) {
    let chat = names.chats.get(open.chat_id);
    let mut title = vec![
        Span::from(format!(
            " {}",
            names.chats.title(open.chat_id).unwrap_or_default()
        ))
        .style(super::title_style(names.chats, open.chat_id, colors)),
    ];
    if let Some(badge) = names.chats.badge(open.chat_id) {
        title.push(super::badge_span(badge, colors));
    }
    title.push(Span::from(" "));
    // What they're doing says more than when they were last seen.
    if let Some(doing) = chat.and_then(|c| super::activity(c, names)) {
        title.push(Span::from(format!("· {doing} ")).fg(colors.activity));
    } else if let Some(seen) = names.chats.seen(open.chat_id) {
        let (label, online) = seen_label(seen, Local::now().timestamp());
        let color = if online {
            colors.activity
        } else {
            colors.muted
        };
        title.push(Span::from(format!("· {label} ")).fg(color));
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
    let mut laid = measure(
        open,
        names,
        show_names,
        block_gaps,
        body.width as usize,
        font,
        images.draws_photos(),
        colors,
    );

    let height = body.height as usize;
    let placed = laid.placed.clone();
    let selected = match open.selected {
        Some(id) => placed.iter().find(|p| p.id == id),
        None => placed.last(),
    };
    let (top, anchor) = scroll_top(open, &placed, selected, laid.total, height);
    let photos = std::mem::take(&mut laid.photos);
    let visible = laid.lines(top, height, block_gaps, colors);
    open.scroll = anchor;
    // Short chats sit at the bottom of the pane, like in Telegram.
    let pad = (height - visible.len()) as u16;
    let shift = |r: Rect| Rect {
        y: r.y + pad,
        height: r.height - pad,
        ..r
    };
    frame.render_widget(Paragraph::new(visible), shift(body));
    draw_photos(frame, shift(body), &photos, top, images, covered, colors);

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

/// "online", "last seen 5 minutes ago", "bot"… for a chat's title, and
/// whether they're online. `now` is a unix timestamp.
fn seen_label(seen: Seen, now: i64) -> (String, bool) {
    let presence = match seen {
        Seen::Bot => return ("bot".into(), false),
        Seen::Person(presence) => presence,
    };
    let was_online = match presence {
        Presence::Online(until) if i64::from(until) > now => return ("online".into(), true),
        // Telegram didn't say it again, so they went offline then.
        Presence::Online(at) | Presence::Offline(at) => i64::from(at),
        Presence::Recently => return ("last seen recently".into(), false),
        Presence::LastWeek => return ("last seen within a week".into(), false),
        Presence::LastMonth => return ("last seen within a month".into(), false),
        Presence::LongAgo => return ("last seen a long time ago".into(), false),
    };
    let ago = (now - was_online).max(0);
    let label = match ago {
        0..60 => "last seen just now".into(),
        60..120 => "last seen a minute ago".into(),
        120..3600 => format!("last seen {} minutes ago", ago / 60),
        _ => {
            let (Some(then), Some(today)) = (
                Local.timestamp_opt(was_online, 0).single(),
                Local.timestamp_opt(now, 0).single(),
            ) else {
                return ("last seen a long time ago".into(), false);
            };
            let days = (today.date_naive() - then.date_naive()).num_days();
            match days {
                0 => format!("last seen today at {}", then.format("%H:%M")),
                1 => format!("last seen yesterday at {}", then.format("%H:%M")),
                _ if then.year() == today.year() => format!("last seen {}", then.format("%-d %b")),
                _ => format!("last seen {}", then.format("%-d %b %Y")),
            }
        }
    };
    (label, false)
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
pub(super) fn draw_photos(
    frame: &mut Frame,
    area: Rect,
    photos: &[PhotoSlot],
    top: usize,
    images: &mut Images,
    covered: bool,
    colors: &Colors,
) {
    for slot in photos {
        let y = slot.line as i64 - top as i64;
        if y >= i64::from(area.height) || y + i64::from(slot.rows) <= 0 {
            continue;
        }
        images.want(&slot.photo, slot.cols, slot.rows);
        // Sixel and iTerm2 images are painted from one cell outside the
        // popup, and a later repaint would cover the popup's text, which
        // isn't sent again. So under a popup they wait until it closes.
        if covered && images.paints_over() {
            continue;
        }
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
        // A link preview's picture is too small to say so.
        let middle = y + i64::from(slot.rows / 2);
        if label.width() <= usize::from(slot.cols) && (0..i64::from(area.height)).contains(&middle)
        {
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
    open: &OpenChat,
    placed: &[Placed],
    selected: Option<&Placed>,
    total: usize,
    height: usize,
) -> (usize, Option<ScrollAnchor>) {
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
    let anchor = placed
        .iter()
        .rev()
        .find(|p| p.start <= top)
        .map(|p| ScrollAnchor {
            msg_id: p.id,
            offset: top - p.start,
        });
    (top, anchor)
}

/// Measures every loaded message and works out where each goes. With
/// `gaps`, messages in a block have a row of their bubble's background
/// between them. Link previews get their picture only with `thumbnails`:
/// half blocks are too coarse for one a few cells big.
#[allow(clippy::too_many_arguments)]
fn measure<'a>(
    open: &'a OpenChat,
    names: &Names,
    show_names: bool,
    gaps: bool,
    width: usize,
    font: FontSize,
    thumbnails: bool,
    colors: &Colors,
) -> Laid<'a> {
    // Text width inside a bubble, after one column of padding each side.
    let max_text = (width * BUBBLE_WIDTH_PERCENT / 100)
        .max(12)
        .min(width)
        .saturating_sub(2)
        .max(1);
    let query = open.search.as_ref().map_or("", |s| s.query.as_str());
    // How far your messages have been read, for their ticks. None in
    // channels and Saved Messages, where nobody reads them.
    let read_outbox = names
        .chats
        .get(open.chat_id)
        .filter(|c| !c.is_channel && !names.chats.is_saved(open.chat_id))
        .map(|c| c.read_outbox);

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
        // Every photo of an album was forwarded from, and answers, the same
        // place; it's said once.
        let header = Header {
            name,
            forward: msg
                .forwarded
                .as_ref()
                .filter(|_| album.is_none_or(|a| a.first))
                .map(|origin| format!("Forwarded from {}", names.origin(origin))),
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
                let mut meta = if edited {
                    format!("edited {time}")
                } else {
                    time.to_string()
                };
                // One tick once it's sent, two once it's been read.
                if msg.outgoing
                    && let Some(read) = read_outbox
                {
                    meta.push_str(if id <= read { " ✓✓" } else { " ✓" });
                }
                meta
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
        let card_photo = caption
            .and_then(|c| c.card.as_ref())
            .and_then(|card| card.image.as_ref())
            .filter(|_| thumbnails);
        let card_image = card_photo.map(|p| (p, thumbnail_cells(p, font)));
        let bubble = Bubble::new(
            msg, caption, header, photo, card_image, meta, chips, matches, max_text,
        );
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

    // Where everything goes, counted without building the rows: only the
    // messages on screen are built (see [`Laid::lines`]). A sender decides
    // how many rows a message takes, up to thousands of line breaks.
    let mut laid = Laid {
        messages: Vec::with_capacity(measured.len()),
        placed: Vec::with_capacity(measured.len()),
        photos: Vec::new(),
        total: 0,
    };
    for (m, inner) in measured.into_iter().zip(widths) {
        let start = laid.total;
        let mut bubble_start = start + usize::from(!m.joined || gaps);
        if m.separator.is_some() {
            bubble_start += 1 + usize::from(start > 0);
        }
        let msg = m.bubble.msg;
        // Rows are right-aligned for own messages, so measure from the right.
        let bubble_x = if msg.outgoing {
            width.saturating_sub(inner + 2)
        } else {
            0
        };
        let photo_rows = m.bubble.photo.map_or(0, |(_, rows)| usize::from(rows));
        if let (Some(photo), Some((cols, rows))) = (&msg.preview, m.bubble.photo) {
            laid.photos.push(PhotoSlot {
                line: bubble_start + m.bubble.header.rows(),
                x: (bubble_x + 1) as u16,
                cols,
                rows,
                photo: photo.clone(),
            });
        }
        // A link preview's picture sits after its bar, under the text.
        if let Some((photo, (cols, rows))) = m.bubble.card_image {
            laid.photos.push(PhotoSlot {
                line: bubble_start + m.bubble.header.rows() + photo_rows + m.bubble.text.len(),
                x: (bubble_x + 3) as u16,
                cols,
                rows,
                photo: photo.clone(),
            });
        }
        let end = bubble_start + m.bubble.height();
        laid.placed.push(Placed {
            id: m.id,
            start,
            bubble_start,
            end,
        });
        laid.messages.push((m, inner));
        laid.total = end;
    }
    laid
}

/// The chat measured for drawing: where each message goes, and what it takes
/// to build its rows.
struct Laid<'a> {
    messages: Vec<(Measured<'a>, usize)>,
    placed: Vec<Placed>,
    photos: Vec<PhotoSlot>,
    /// Rows in all.
    total: usize,
}

impl Laid<'_> {
    /// Rows `top..top + height`, building only the messages they show.
    fn lines(self, top: usize, height: usize, gaps: bool, colors: &Colors) -> Vec<Line<'static>> {
        let bottom = top.saturating_add(height);
        let mut out = Vec::new();
        for ((m, inner), placed) in self.messages.into_iter().zip(&self.placed) {
            if placed.end <= top || placed.start >= bottom {
                continue;
            }
            let mut lines = Vec::with_capacity(placed.end - placed.start);
            if let Some(label) = m.separator {
                if placed.start > 0 {
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
            lines.extend(m.bubble.rows(inner, colors));
            debug_assert_eq!(lines.len(), placed.end - placed.start, "measured right");
            let from = top.saturating_sub(placed.start);
            let to = bottom.min(placed.end) - placed.start;
            out.extend(lines.into_iter().take(to).skip(from));
        }
        out
    }
}

/// Every row of the chat, built: what [`draw`] would show if the pane were
/// tall enough.
#[cfg(test)]
fn layout(
    open: &OpenChat,
    names: &Names,
    show_names: bool,
    gaps: bool,
    width: usize,
    font: FontSize,
    colors: &Colors,
) -> (Vec<Line<'static>>, Vec<Placed>, Vec<PhotoSlot>) {
    let mut laid = measure(open, names, show_names, gaps, width, font, true, colors);
    let placed = laid.placed.clone();
    let photos = std::mem::take(&mut laid.photos);
    let total = laid.total;
    (laid.lines(0, total, gaps, colors), placed, photos)
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

/// Lines of a link preview's description shown under a message.
const CARD_LINES: usize = 3;

/// What a row of a link preview shows.
#[derive(Clone, Copy, PartialEq, Eq)]
enum CardRow {
    Host,
    Title,
    Description,
}

/// A link preview's picture is this many rows tall, or less if it's wide.
const THUMBNAIL_ROWS: u16 = 4;
/// And at most this many columns wide.
const THUMBNAIL_MAX_COLS: u16 = 15;

/// Cells for a link preview's picture: [`THUMBNAIL_ROWS`] tall, as wide as
/// its shape makes that, up to [`THUMBNAIL_MAX_COLS`]; a wider picture is
/// less tall instead, so it fills its cells.
pub(super) fn thumbnail_cells(photo: &Preview, font: FontSize) -> (u16, u16) {
    let (fw, fh) = (f64::from(font.width.max(1)), f64::from(font.height.max(1)));
    let (pw, ph) = (f64::from(photo.width), f64::from(photo.height));
    let rows = f64::from(THUMBNAIL_ROWS);
    let cols = rows * fh * pw / ph / fw;
    if cols <= f64::from(THUMBNAIL_MAX_COLS) {
        return (cols.round().max(1.0) as u16, THUMBNAIL_ROWS);
    }
    let cols = f64::from(THUMBNAIL_MAX_COLS);
    let rows = cols * fw * ph / pw / fh;
    (THUMBNAIL_MAX_COLS, rows.round().max(1.0) as u16)
}

/// A link preview's rows, `width` columns wide after its bar. With a
/// picture, its text goes to the right of it, and there are at least as
/// many rows as the picture is tall.
fn card_rows(card: &Card, width: usize, image: Option<(u16, u16)>) -> Vec<(String, CardRow)> {
    let width = width.saturating_sub(image.map_or(0, |(cols, _)| usize::from(cols) + 1));
    let mut rows = vec![(truncate(&card.host, width), CardRow::Host)];
    if !card.title.is_empty() {
        rows.push((truncate(&card.title, width), CardRow::Title));
    }
    let lines = wrap(&card.description, width);
    let more = lines.len() > CARD_LINES;
    for (i, (line, _)) in lines.into_iter().take(CARD_LINES).enumerate() {
        let line = if more && i + 1 == CARD_LINES {
            // `truncate` adds the "…" once the line can't take another.
            truncate(&format!("{line} …"), width)
        } else {
            line
        };
        rows.push((line, CardRow::Description));
    }
    let tall = image.map_or(0, |(_, rows)| usize::from(rows));
    while rows.len() < tall {
        rows.push((String::new(), CardRow::Description));
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
    /// The text's formatting.
    styles: &'a [Styled],
    /// Enter showed the text's spoilers.
    revealed: bool,
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
    /// The link preview under the text, cut to fit: where the link goes,
    /// the page's title, and a few lines of its description.
    card: Vec<(String, CardRow)>,
    /// The preview's picture, and the columns and rows it takes on the
    /// left of the preview's text.
    card_image: Option<(&'a Preview, (u16, u16))>,
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
        card_image: Option<(&'a Preview, (u16, u16))>,
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
        // The preview's bar takes two columns, and its picture its own and
        // one more.
        let image = card_image.map(|(_, cells)| cells);
        let card = caption
            .and_then(|c| c.card.as_ref())
            .map_or(Vec::new(), |card| {
                card_rows(card, max_text.saturating_sub(2), image)
            });
        let card_indent = 2 + image.map_or(0, |(cols, _)| usize::from(cols) + 1);
        let meta_w = meta.as_ref().map_or(0, |m| m.width());
        // The time goes beside the last row of reactions, else the last line
        // of text, if it fits. Under a link preview it has a row of its own.
        let last_w = match chips.last() {
            Some(row) => chips_width(row),
            None => text.last().map_or(0, |(l, _)| l.width()),
        };
        let meta_at = match () {
            _ if meta.is_none() || last_w + META_GAP + meta_w > max_text => MetaAt::Own,
            _ if chips.is_empty() && !card.is_empty() => MetaAt::Own,
            _ if chips.is_empty() => MetaAt::Text,
            _ => MetaAt::Reactions,
        };

        let mut width = text
            .iter()
            .map(|(l, _)| l.width())
            .chain(chips.iter().map(|row| chips_width(row)))
            .chain(card.iter().map(|(l, _)| card_indent + l.width()))
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
        let forward = header.forward.map(|f| truncate(&f, max_text));
        if let Some(f) = &forward {
            width = width.max(f.width());
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
            styles: caption.map_or(&[], |c| c.styles.as_slice()),
            revealed: caption.is_some_and(|c| c.revealed),
            header: Header {
                name,
                forward,
                quote,
            },
            photo,
            meta,
            chips,
            matches,
            text,
            card,
            card_image,
            meta_at,
            width,
        }
    }

    /// Rows [`Bubble::rows`] makes.
    fn height(&self) -> usize {
        self.header.rows()
            + self.photo.map_or(0, |(_, rows)| usize::from(rows))
            + self.text.len()
            + self.card.len()
            + self.chips.len()
            + usize::from(self.meta_at == MetaAt::Own && self.meta.is_some())
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
            styles,
            revealed,
            header,
            photo,
            meta,
            chips,
            matches,
            text,
            card,
            card_image,
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
        let paint = Paint {
            links,
            matches: &matches,
            styles,
            revealed,
            style,
            found: super::match_style(colors),
            code: colors.code,
            faded,
        };
        let spans = |line: &str, start: usize| line_spans(line, start, &paint);

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
        if let Some(forward) = header.forward {
            let w = forward.width();
            out.push(row(
                vec![Span::styled(forward, style.fg(faded).italic())],
                w,
            ));
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
        // Blank under the picture, for `draw_photos` to paint.
        let image_cols = card_image.map_or(0, |(_, (cols, _))| usize::from(cols) + 1);
        for (line, kind) in card {
            let w = line.width();
            let text_style = match kind {
                CardRow::Host => style.fg(faded),
                CardRow::Title => style.bold(),
                CardRow::Description => style,
            };
            let spans = vec![
                Span::styled("▎ ", style.fg(faded)),
                Span::styled(" ".repeat(image_cols), style),
                Span::styled(line, text_style),
            ];
            out.push(row(spans, 2 + image_cols + w));
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

/// What decides how each part of a bubble's text looks. Ranges are byte
/// ranges of the whole text.
struct Paint<'a> {
    /// Underlined.
    links: &'a [Range<usize>],
    /// Search matches, in `found`.
    matches: &'a [Range<usize>],
    /// The formatting the sender picked.
    styles: &'a [Styled],
    /// Spoilers are shown, not blotted out.
    revealed: bool,
    /// The bubble's text.
    style: Style,
    /// Laid over a search match.
    found: Style,
    /// Code.
    code: Color,
    /// What's quieter than the text on the bubble: quotes, hidden spoilers.
    faded: Color,
}

impl Paint<'_> {
    /// `style` with the sender's formatting.
    fn format(&self, style: Style, format: Format) -> Style {
        let mut style = style;
        if format.bold {
            style = style.bold();
        }
        if format.italic || format.quote {
            style = style.italic();
        }
        if format.underline {
            style = style.underlined();
        }
        if format.strike {
            style = style.crossed_out();
        }
        if format.code {
            style = style.fg(self.code);
        }
        if format.quote || (format.spoiler && !self.revealed) {
            style = style.fg(self.faded);
        }
        style
    }
}

/// Splits one wrapped line, which starts at byte `start` of the text, into
/// spans painted as `paint` says.
fn line_spans(line: &str, start: usize, paint: &Paint) -> Vec<Span<'static>> {
    let end = start + line.len();
    // Only the ranges on this line: a message can have thousands, and each
    // of its lines is drawn on every frame.
    let links = overlapping(paint.links, start, end, |r| r);
    let matches = overlapping(paint.matches, start, end, |r| r);
    let styles = overlapping(paint.styles, start, end, |s| &s.range);
    // Every offset in the line where the style can change.
    let mut cuts = vec![0, line.len()];
    let edges = links
        .iter()
        .chain(matches)
        .chain(styles.iter().map(|s| &s.range));
    for range in edges {
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
            let mut text = line[w[0]..w[1]].to_string();
            let mut s = paint.style;
            if let Some(styled) = styles.iter().find(|s| s.range.contains(&at)) {
                s = paint.format(s, styled.format);
                // As wide as the words it hides, so revealing them moves nothing.
                if styled.format.spoiler && !paint.revealed {
                    text = SPOILER.repeat(text.width());
                }
            }
            if inside(links, at) {
                s = s.underlined();
            }
            if inside(matches, at) {
                s = s.patch(paint.found);
            }
            Span::styled(text, s)
        })
        .collect();
    if spans.is_empty() {
        spans.push(Span::styled(String::new(), paint.style));
    }
    spans
}

/// The items whose ranges overlap `start..end`, out of items in order whose
/// ranges don't overlap each other (links, search matches and formatting).
fn overlapping<T>(
    items: &[T],
    start: usize,
    end: usize,
    range: impl Fn(&T) -> &Range<usize>,
) -> &[T] {
    let first = items.partition_point(|i| range(i).end <= start);
    let count = items[first..].partition_point(|i| range(i).start < end);
    &items[first..first + count]
}

#[cfg(test)]
mod tests {
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;
    use ratatui_image::picker::Picker;

    use super::*;
    use crate::images::Key;
    use crate::messages::Editable;
    use crate::settings::Settings;

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
            styles: Vec::new(),
            revealed: false,
            forwarded: None,
            poll: None,
            card: None,
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
                    false,
                    &Colors::default(),
                    Settings::default().block_gaps,
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
        let colors = Colors::default();
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
        let colors = Colors::default();
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
    fn your_messages_get_one_tick_when_sent_and_two_once_read() {
        let mut open = OpenChat::new(1);
        open.messages.insert(1, msg(true, 1_790_000_000, "seen it"));
        open.messages.insert(2, msg(false, 1_790_000_060, "answer"));
        open.messages.insert(3, msg(true, 1_790_000_120, "not yet"));
        let render = |chats: &Chats, open: &mut OpenChat| -> Vec<String> {
            let buf = render_in(open, chats, false, &mut images());
            (0..buf.area.height)
                .map(|y| (0..buf.area.width).map(|x| buf[(x, y)].symbol()).collect())
                .collect()
        };
        let row =
            |rows: &[String], text: &str| rows.iter().find(|r| r.contains(text)).cloned().unwrap();

        let mut chats = Chats::default();
        chats.add_local(1, "Chardy", None).read_outbox = 2;
        let rows = render(&chats, &mut open);
        assert!(row(&rows, "seen it").contains(" ✓✓"), "{rows:#?}");
        let unread = row(&rows, "not yet");
        assert!(unread.contains(" ✓") && !unread.contains("✓✓"), "{unread}");
        assert!(!row(&rows, "answer").contains('✓'), "not on theirs");

        chats.add_local(1, "News", None).is_channel = true;
        assert!(!render(&chats, &mut open).iter().any(|r| r.contains('✓')));
        let mut saved = Chats::default();
        saved.add_local(1, "Saved Messages", None);
        saved.set_my_id(1);
        assert!(!render(&saved, &mut open).iter().any(|r| r.contains('✓')));
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
        let colors = Colors::default();
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
        let colors = Colors::default();
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
        let colors = Colors::default();
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
    fn sixel_photos_wait_while_a_popup_is_over_them() {
        use ratatui_image::picker::ProtocolType;
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
        let mut picker = Picker::halfblocks();
        picker.set_protocol_type(ProtocolType::Sixel);
        let mut images = Images::new(picker, tokio::sync::mpsc::unbounded_channel().0);
        let (cols, rows) = photo_cells(&photo, 56, images.font_size());
        let key = Key {
            file_id: 7,
            cols,
            rows,
            thumbnail: false,
            avatar: false,
        };
        let red = image::RgbImage::from_pixel(80, 60, image::Rgb([255, 0, 0]));
        images.insert_ready(key, red.into());
        let mut painted = |covered| {
            let users = HashMap::new();
            let chats = Chats::default();
            let names = Names {
                users: &users,
                chats: &chats,
            };
            let mut terminal = Terminal::new(TestBackend::new(60, 24)).unwrap();
            terminal
                .draw(|f| {
                    let colors = Colors::default();
                    draw(
                        f,
                        f.area(),
                        &mut open,
                        &names,
                        &mut images,
                        false,
                        covered,
                        &colors,
                        true,
                    )
                })
                .unwrap();
            let buf = terminal.backend().buffer().clone();
            buf.content().iter().any(|c| c.symbol().contains('\x1b'))
        };
        assert!(painted(false), "drawn as usual");
        assert!(!painted(true), "held back under a popup");
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

    /// Plain text with these links, search matches (on yellow) and formatting.
    fn paint<'a>(
        links: &'a [Range<usize>],
        matches: &'a [Range<usize>],
        styles: &'a [Styled],
    ) -> Paint<'a> {
        Paint {
            links,
            matches,
            styles,
            revealed: false,
            style: Style::new(),
            found: Style::new().bg(Color::Yellow),
            code: Color::Green,
            faded: Color::Gray,
        }
    }

    #[test]
    fn links_are_underlined_even_across_a_wrap() {
        let text = "see https://example.com/a/long/path ok";
        let link = 4..text.find(" ok").unwrap();
        let mut underlined = String::new();
        for (line, start) in wrap(text, 20) {
            let links = std::slice::from_ref(&link);
            for span in line_spans(&line, start, &paint(links, &[], &[])) {
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
    fn a_link_preview_sits_under_the_text_with_the_links_real_host() {
        let mut open = OpenChat::new(42);
        let mut m = msg(false, 1_790_000_000, "look https://example.com/a");
        m.card = Some(Card {
            host: "example.com".into(),
            title: "Example Domain".into(),
            description: "word ".repeat(60),
            image: None,
        });
        open.messages.insert(1, m);
        let rows = render(&mut open, false);
        let at = |needle: &str| {
            rows.iter()
                .position(|r| r.contains(needle))
                .unwrap_or_else(|| panic!("{needle}: {rows:#?}"))
        };
        let (text, host, title) = (
            at("look https"),
            at("▎ example.com"),
            at("▎ Example Domain"),
        );
        assert!(text < host && host < title);
        let description = rows.iter().filter(|r| r.contains("▎ word")).count();
        assert_eq!(description, CARD_LINES, "a few lines of it");
        assert!(rows[title + CARD_LINES].contains('…'), "then it stops");
    }

    #[test]
    fn a_link_previews_picture_sits_left_of_its_text() {
        let mut open = OpenChat::new(42);
        let mut m = msg(false, 1_790_000_000, "look");
        let square = Preview {
            file_id: 9,
            width: 300,
            height: 300,
            thumbnail: None,
            sticker: false,
        };
        m.card = Some(Card {
            host: "example.com".into(),
            title: "Example Domain".into(),
            description: "Short".into(),
            image: Some(square),
        });
        open.messages.insert(1, m);
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
        let colors = Colors::default();
        let (lines, _, photos) = layout(&open, &names, false, true, 58, font, &colors);
        let rows: Vec<String> = lines.iter().map(|l| l.to_string()).collect();
        let [slot] = photos.as_slice() else {
            panic!("one picture: {}", photos.len());
        };
        // Square, so twice as many columns as rows (cells are twice as tall).
        assert_eq!((slot.cols, slot.rows), (8, 4));
        assert_eq!(slot.x, 3, "after the bubble's padding and the bar");
        assert!(
            rows[slot.line].contains("▎          example.com"),
            "{rows:#?}"
        );
        assert!(rows[slot.line + 1].contains("Example Domain"));
        assert!(
            rows[slot.line + 3].trim_end().ends_with('▎'),
            "as tall as the picture"
        );

        let wide = Preview {
            width: 1200,
            height: 300,
            ..slot.photo.clone()
        };
        assert_eq!(
            thumbnail_cells(&wide, font),
            (15, 2),
            "a wide one is less tall"
        );
        let page = Preview {
            width: 1200,
            height: 630,
            ..slot.photo.clone()
        };
        assert_eq!(
            thumbnail_cells(&page, font),
            (15, 4),
            "a web page's picture"
        );
    }

    #[test]
    fn the_title_says_when_the_other_person_was_last_seen() {
        let now = Local
            .with_ymd_and_hms(2026, 10, 6, 15, 0, 0)
            .unwrap()
            .timestamp();
        let ago = |seconds: i64| (now - seconds) as i32;
        let seen = |presence| seen_label(Seen::Person(presence), now).0;
        assert_eq!(
            seen_label(Seen::Person(Presence::Online(ago(-60))), now),
            ("online".into(), true)
        );
        assert_eq!(
            seen(Presence::Online(ago(30))),
            "last seen just now",
            "online ran out"
        );
        assert_eq!(seen(Presence::Offline(ago(90))), "last seen a minute ago");
        assert_eq!(
            seen(Presence::Offline(ago(5 * 60))),
            "last seen 5 minutes ago"
        );
        assert_eq!(
            seen(Presence::Offline(ago(3 * 3600))),
            "last seen today at 12:00"
        );
        assert_eq!(
            seen(Presence::Offline(ago(20 * 3600))),
            "last seen yesterday at 19:00"
        );
        assert_eq!(seen(Presence::Offline(ago(10 * 86400))), "last seen 26 Sep");
        assert_eq!(
            seen(Presence::Offline(ago(400 * 86400))),
            "last seen 1 Sep 2025"
        );
        assert_eq!(seen(Presence::Recently), "last seen recently");
        assert_eq!(seen_label(Seen::Bot, now), ("bot".into(), false));
    }

    #[test]
    fn formatting_shows_in_the_bubble_and_spoilers_only_once_revealed() {
        let mut open = OpenChat::new(42);
        let mut m = msg(false, 1_790_000_000, "bold code secret");
        let styled = |range, format| Styled { range, format };
        m.styles = vec![
            styled(
                0..4,
                Format {
                    bold: true,
                    ..Format::default()
                },
            ),
            styled(
                5..9,
                Format {
                    code: true,
                    ..Format::default()
                },
            ),
            styled(
                10..16,
                Format {
                    spoiler: true,
                    ..Format::default()
                },
            ),
        ];
        open.messages.insert(1, m);
        let rows = |buf: &ratatui::buffer::Buffer| -> Vec<String> {
            (0..buf.area.height)
                .map(|y| (0..buf.area.width).map(|x| buf[(x, y)].symbol()).collect())
                .collect()
        };
        let buf = render_buffer(&mut open, false, &mut images());
        assert!(
            rows(&buf).iter().any(|r| r.contains("bold code ⠿⠿⠿⠿⠿⠿")),
            "{:#?}",
            rows(&buf)
        );
        assert!(!rows(&buf).iter().any(|r| r.contains("secret")));
        let cell = |buf: &ratatui::buffer::Buffer, symbol: &str| {
            buf.content()
                .iter()
                .find(|c| c.symbol() == symbol)
                .cloned()
                .unwrap()
        };
        assert!(
            cell(&buf, "b")
                .modifier
                .contains(ratatui::style::Modifier::BOLD)
        );
        assert_eq!(cell(&buf, "c").fg, Colors::default().code);

        open.messages.get_mut(&1).unwrap().revealed = true;
        let buf = render_buffer(&mut open, false, &mut images());
        assert!(
            rows(&buf).iter().any(|r| r.contains("bold code secret")),
            "the same width, so nothing moves"
        );
    }

    #[test]
    fn forwarded_messages_say_where_from_once_per_album() {
        let mut open = OpenChat::new(42);
        let mut m = msg(true, 1_790_000_000, "look at this");
        m.forwarded = Some(Origin::Hidden("Bob".into()));
        open.messages.insert(1, m);
        let mut post = msg(false, 1_790_000_100, "news");
        post.forwarded = Some(Origin::Chat {
            chat_id: 5,
            signature: "Ann".into(),
        });
        open.messages.insert(2, post);
        let rows = render(&mut open, false);
        assert!(
            rows.iter().any(|r| r.contains("Forwarded from Bob")),
            "{rows:#?}"
        );
        assert!(
            rows.iter()
                .any(|r| r.contains("Forwarded from Unknown (Ann)")),
            "a signed post from a chat that isn't loaded"
        );

        let mut album = OpenChat::new(42);
        for id in 1..=2 {
            let mut photo = msg(false, 1_790_000_000, "");
            photo.album = 9;
            photo.forwarded = Some(Origin::Hidden("Bob".into()));
            photo.preview = Some(Preview {
                file_id: id as i32,
                width: 100,
                height: 100,
                thumbnail: None,
                sticker: false,
            });
            album.messages.insert(id, photo);
        }
        let rows = render(&mut album, false);
        let said = rows.iter().filter(|r| r.contains("Forwarded from")).count();
        assert_eq!(said, 1, "{rows:#?}");
    }

    #[test]
    fn a_flood_of_line_breaks_draws_only_what_is_on_screen() {
        let mut open = OpenChat::new(42);
        let text = format!("a{}b", "\n".repeat(4094));
        for id in 1..=200 {
            open.messages.insert(id, msg(false, 1_790_000_000, &text));
        }
        let started = std::time::Instant::now();
        let buf = render_buffer(&mut open, false, &mut images());
        let took = started.elapsed();
        let rows: Vec<String> = (0..buf.area.height)
            .map(|y| (0..buf.area.width).map(|x| buf[(x, y)].symbol()).collect())
            .collect();
        // A bubble taller than the pane shows from its top.
        assert!(
            rows.iter().any(|r| r.contains("│  a ")),
            "the newest message: {rows:#?}"
        );
        eprintln!("200 messages of 4095 rows: {took:?}");
        assert!(took < std::time::Duration::from_secs(5), "{took:?}");
    }

    #[test]
    fn thousands_of_links_in_one_message_are_drawn_quickly() {
        let text = "ab".repeat(2048);
        let links: Vec<_> = (0..2048).map(|i| 2 * i..2 * i + 1).collect();
        let started = std::time::Instant::now();
        let mut underlined = 0;
        for (line, start) in wrap(&text, 71) {
            for span in line_spans(&line, start, &paint(&links, &[], &[])) {
                if span
                    .style
                    .add_modifier
                    .contains(ratatui::style::Modifier::UNDERLINED)
                {
                    underlined += span.content.len();
                }
            }
        }
        assert_eq!(underlined, 2048, "every 'a', and nothing else");
        // Quadratic work took seconds; a loaded machine still does this in far
        // less than one.
        assert!(started.elapsed() < std::time::Duration::from_secs(1));
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

        let colors = Colors::default();
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
        let matches = crate::search::find(text, "hello");
        let spans = line_spans(text, 0, &paint(&[link], &matches, &[]));
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
        let own = Colors::default().own_bubble;
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
        let colors = Colors::default();
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
        let colors = Colors::default();
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
            original: "hello from me".into(),
            editable: Editable::Text,
            draft: String::new(),
            reply: None,
            attachments: Vec::new(),
        });
        let buf = render_buffer(&mut open, false, &mut images());
        let colors = Colors::default();
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
        let colors = Colors::default();
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

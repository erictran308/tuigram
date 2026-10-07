//! History of the open chat, sorted by message id (which is chronological).

use std::collections::{BTreeMap, HashMap};
use std::ops::Range;
use std::time::{Duration, SystemTime};

use base64::Engine;
use tdlib_rs::enums::{
    LinkPreviewType, MessageContent, MessageOrigin, MessageReplyTo, MessageSender,
    MessageSendingState, ReplyMarkup, StickerFormat, TextEntityType, ThumbnailFormat,
};
use tdlib_rs::types::{self, Message};
use unicode_width::UnicodeWidthStr;

use crate::attach::{Attachment, Dropped};
use crate::buttons::Keyboard;
use crate::chats::content_text_as_sent;
use crate::complete::Commands;
use crate::images::Thumbnail;
use crate::pins::Pinned;
use crate::poll::Poll;
use crate::reactions::{self, Reaction, ReactionKind};
use crate::search::MessageSearch;
use crate::secret::Destruct;
use crate::service::Service;
use crate::text;
use crate::tg::Page;
use crate::voice::Voice;

/// Download the smallest size at least this big (TDLib's "x", ~800px), sharp
/// enough for a bubble on a high-DPI screen without fetching the original.
const PHOTO_MIN_SIDE: i32 = 640;
/// A link preview's thumbnail is a few cells: TDLib's "m" (320 px) does.
const THUMBNAIL_MIN_SIDE: i32 = 200;

/// Messages kept loaded while following new ones; see [`OpenChat::add_new`].
const MAX_FOLLOWED: usize = 1000;
/// While reading older messages, new ones are taken until this many are
/// loaded; after that they load again on moving down to them.
const MAX_LOADED: usize = 2 * MAX_FOLLOWED;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Sender {
    User(i64),
    /// Channel posts and anonymous group admins are sent "as" a chat.
    Chat(i64),
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum SendState {
    Sent,
    /// Still on its way to the server, under a temporary id.
    Pending,
    Failed,
}

/// An image shown inline: a photo, or a video's thumbnail.
#[derive(Clone)]
pub struct Preview {
    /// TDLib file id of the image we download.
    pub file_id: i32,
    /// Size of what it depicts, for the aspect ratio.
    pub width: u32,
    pub height: u32,
    /// Tiny blurred JPEG sent inside the message, shown until the download finishes.
    pub thumbnail: Option<Thumbnail>,
    /// Stickers are drawn smaller and without a bubble, like in Telegram.
    pub sticker: bool,
}

impl Preview {
    fn from_photo(photo: &types::Photo) -> Option<Self> {
        Self::sized(photo, PHOTO_MIN_SIDE)
    }

    /// A photo's smallest size at least `min_side` pixels on its long side,
    /// else its largest.
    fn sized(photo: &types::Photo, min_side: i32) -> Option<Self> {
        let size = photo
            .sizes
            .iter()
            .filter(|s| s.width.max(s.height) >= min_side)
            .min_by_key(|s| s.width)
            .or_else(|| largest(photo))?;
        Some(Self {
            file_id: size.photo.id,
            width: size.width.max(1) as u32,
            height: size.height.max(1) as u32,
            thumbnail: decode_minithumbnail(photo.minithumbnail.as_ref()),
            sticker: false,
        })
    }

    /// Static stickers are WebP images themselves. Animated ones (TGS, WebM)
    /// show their still thumbnail.
    fn from_sticker(sticker: &types::Sticker) -> Option<Self> {
        let file_id = match sticker.format {
            StickerFormat::Webp => sticker.sticker.id,
            StickerFormat::Tgs | StickerFormat::Webm => {
                sticker
                    .thumbnail
                    .as_ref()
                    .filter(|t| decodable(&t.format))?
                    .file
                    .id
            }
        };
        Some(Self {
            file_id,
            width: sticker.width.max(1) as u32,
            height: sticker.height.max(1) as u32,
            thumbnail: None,
            sticker: true,
        })
    }

    /// A sticker small, as the sticker panel shows it: its thumbnail, a
    /// fraction of the download, else the sticker itself if it's a still
    /// image.
    pub fn sticker_thumbnail(sticker: &types::Sticker) -> Option<Self> {
        let thumbnail = sticker.thumbnail.as_ref().filter(|t| decodable(&t.format));
        let file_id = match (thumbnail, &sticker.format) {
            (Some(thumbnail), _) => thumbnail.file.id,
            (None, StickerFormat::Webp) => sticker.sticker.id,
            (None, _) => return None,
        };
        Some(Self {
            file_id,
            width: sticker.width.max(1) as u32,
            height: sticker.height.max(1) as u32,
            thumbnail: None,
            sticker: true,
        })
    }

    /// A video's still thumbnail, if it's in a format we can decode
    /// (some are tiny MPEG-4 clips).
    fn from_video(video: &types::Video) -> Option<Self> {
        let thumbnail = video.thumbnail.as_ref().filter(|t| decodable(&t.format))?;
        let (width, height) = if video.width > 0 && video.height > 0 {
            (video.width, video.height)
        } else {
            (thumbnail.width, thumbnail.height)
        };
        Some(Self {
            file_id: thumbnail.file.id,
            width: width.max(1) as u32,
            height: height.max(1) as u32,
            thumbnail: decode_minithumbnail(video.minithumbnail.as_ref()),
            sticker: false,
        })
    }
}

/// Still-image formats the `image` crate decodes (not MPEG-4, TGS or WebM clips).
fn decodable(format: &ThumbnailFormat) -> bool {
    matches!(
        format,
        ThumbnailFormat::Jpeg | ThumbnailFormat::Png | ThumbnailFormat::Webp | ThumbnailFormat::Gif
    )
}

fn largest(photo: &types::Photo) -> Option<&types::PhotoSize> {
    photo.sizes.iter().max_by_key(|s| s.width)
}

/// TDLib's JSON sends bytes as base64.
pub fn decode_minithumbnail(mini: Option<&types::Minithumbnail>) -> Option<Thumbnail> {
    let data = &mini?.data;
    base64::engine::general_purpose::STANDARD
        .decode(data)
        .ok()
        .map(Thumbnail::from)
}

pub struct Msg {
    pub sender: Sender,
    pub outgoing: bool,
    /// Unix timestamp.
    pub date: i32,
    /// Message text; with a preview, just the caption (plus a video's length).
    pub text: String,
    /// The text or caption as sent, without labels like "[File]". What `y` copies.
    pub source_text: String,
    pub preview: Option<Preview>,
    /// The file Enter opens: the full photo, the video, the document…
    pub file: Option<MediaFile>,
    /// Web links in the text or caption, in order, without duplicates.
    pub links: Vec<Link>,
    /// Byte ranges of `text` that are links, to underline.
    pub link_ranges: Vec<Range<usize>>,
    /// Formatting the sender picked (bold, code, spoilers…), by byte range
    /// of `text`, in order and not overlapping.
    pub styles: Vec<Styled>,
    /// Enter showed its spoilers.
    pub revealed: bool,
    /// Who it was forwarded from, if it was.
    pub forwarded: Option<Origin>,
    /// A poll, which Enter votes in.
    pub poll: Option<Poll>,
    /// Telegram's preview of a link in the text.
    pub card: Option<Card>,
    pub state: SendState,
    /// Set when this message is a reply.
    pub reply_to: Option<ReplyTo>,
    /// What `e` can change.
    pub editable: Editable,
    /// The text has formatting (bold, links behind words…) that an edit,
    /// which sends plain text, would lose.
    pub formatted: bool,
    /// Changed after it was sent.
    pub edited: bool,
    /// Messages sent together as an album share this id; 0 for the rest.
    pub album: i64,
    /// Reactions people added, most added first.
    pub reactions: Vec<Reaction>,
    /// A bot's buttons, which Enter lists to press.
    pub keyboard: Option<Keyboard>,
    /// Pinned in the chat.
    pub pinned: bool,
    /// Its self-destruct timer, in a secret chat with one or a view-once
    /// photo.
    pub destruct: Option<Destruct>,
    /// A photo or video shown only while it's open, until Enter opens it.
    pub hidden: Option<Hidden>,
    /// Telegram lets it be saved: not in a chat that protects its content,
    /// nor media with a self-destruct timer. `y` copies only what can be.
    pub saveable: bool,
    /// A voice message, which Enter plays.
    pub voice: Option<Voice>,
    /// What happened in the chat, when it's a service message: drawn in the
    /// middle, like a date, not in a bubble.
    pub service: Option<Service>,
}

/// A photo or video its sender wants seen only while it's open (view once,
/// or a short self-destruct timer), which Telegram blurs until it's tapped:
/// the bubble names it until Enter opens it, and it's covered again once
/// the cursor leaves it.
pub struct Hidden {
    /// The message as it shows the other way: open while it's covered, and
    /// covered while it's open.
    other: Box<Body>,
    /// Enter opened it.
    pub open: bool,
    /// It expired while open: covering it shows that it's gone.
    gone: bool,
}

/// What `e` can change in a message, which decides how TDLib is asked.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Editable {
    /// No words to edit: a sticker, a video message, a poll…
    No,
    /// A text message.
    Text,
    /// The caption of a photo, video or file, which may be left empty.
    /// `above` keeps it over the media, where the sender put it.
    Caption { above: bool },
}

/// A message being edited in the composer, set with `e`.
pub struct Editing {
    pub id: i64,
    /// What it said, for the bar over the composer.
    pub snippet: String,
    /// What the composer started with: the message as Markdown. Enter
    /// sends nothing if it's still that.
    pub original: String,
    pub editable: Editable,
    /// What the composer held before, put back once the edit is saved or
    /// cancelled.
    pub draft: String,
    pub reply: Option<Replied>,
    /// Files waiting to be sent, put back too: an edit can't add any.
    pub attachments: Vec<Attachment>,
}

impl Msg {
    /// The message on one line: its text, or else what it holds ("Photo").
    /// Spoilers stay hidden.
    pub fn snippet(&self) -> String {
        let text = one_line(&hide_spoilers(&self.text, &self.styles));
        match &self.file {
            _ if !text.is_empty() => text,
            Some(file) => file.label.clone(),
            None => "Message".into(),
        }
    }

    /// It has spoilers that Enter hasn't shown yet.
    pub fn hides_spoilers(&self) -> bool {
        !self.revealed && self.styles.iter().any(|s| s.format.spoiler)
    }

    /// Shows a photo or video that's shown only while open. Returns whether
    /// there was one, covered.
    pub fn uncover(&mut self) -> bool {
        let Some(mut hidden) = self.hidden.take() else {
            return false;
        };
        let covered = !hidden.open;
        if covered {
            self.swap_body(&mut hidden.other);
            hidden.open = true;
        }
        self.hidden = Some(hidden);
        covered
    }

    /// Covers what [`Msg::uncover`] showed; once it expired, for good.
    pub fn cover(&mut self) {
        let Some(mut hidden) = self.hidden.take() else {
            return;
        };
        if hidden.open {
            self.swap_body(&mut hidden.other);
            hidden.open = false;
        }
        if !hidden.gone {
            self.hidden = Some(hidden);
        }
    }

    /// Takes what a new body shows, keeping whatever else is known.
    fn set_body(&mut self, mut body: Body) {
        self.hidden = body.hidden.take();
        self.swap_body(&mut body);
    }

    fn swap_body(&mut self, body: &mut Body) {
        std::mem::swap(&mut self.text, &mut body.text);
        std::mem::swap(&mut self.source_text, &mut body.source_text);
        std::mem::swap(&mut self.preview, &mut body.preview);
        std::mem::swap(&mut self.file, &mut body.file);
        std::mem::swap(&mut self.links, &mut body.links);
        std::mem::swap(&mut self.link_ranges, &mut body.link_ranges);
        std::mem::swap(&mut self.styles, &mut body.styles);
        std::mem::swap(&mut self.poll, &mut body.poll);
        std::mem::swap(&mut self.card, &mut body.card);
        std::mem::swap(&mut self.editable, &mut body.editable);
        std::mem::swap(&mut self.formatted, &mut body.formatted);
        std::mem::swap(&mut self.voice, &mut body.voice);
    }
}

/// How part of a message's text looks, from the formatting its sender picked.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Format {
    pub bold: bool,
    pub italic: bool,
    pub underline: bool,
    pub strike: bool,
    /// Inline code or a code block.
    pub code: bool,
    /// Hidden until Enter shows it.
    pub spoiler: bool,
    /// A block quote.
    pub quote: bool,
}

/// A stretch of text with one [`Format`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Styled {
    pub range: Range<usize>,
    pub format: Format,
}

/// The kinds of formatting a [`Format`] has, for counting how many entities
/// of each cover a spot.
#[derive(Clone, Copy)]
enum Mark {
    Bold,
    Italic,
    Underline,
    Strike,
    Code,
    Spoiler,
    Quote,
}

const MARKS: usize = 7;

impl Mark {
    fn of(kind: &TextEntityType) -> Option<Self> {
        use TextEntityType as T;
        Some(match kind {
            T::Bold => Mark::Bold,
            T::Italic => Mark::Italic,
            T::Underline => Mark::Underline,
            T::Strikethrough => Mark::Strike,
            T::Code | T::Pre | T::PreCode(_) => Mark::Code,
            T::Spoiler => Mark::Spoiler,
            T::BlockQuote | T::ExpandableBlockQuote => Mark::Quote,
            _ => return None,
        })
    }
}

impl Format {
    /// The formatting where `open[mark]` entities of each kind are open.
    fn of(open: &[u32; MARKS]) -> Self {
        let on = |mark: Mark| open[mark as usize] > 0;
        Format {
            bold: on(Mark::Bold),
            italic: on(Mark::Italic),
            underline: on(Mark::Underline),
            strike: on(Mark::Strike),
            code: on(Mark::Code),
            spoiler: on(Mark::Spoiler),
            quote: on(Mark::Quote),
        }
    }
}

/// The formatting in a text, as byte ranges. Entities can nest (bold inside
/// italic) and overlap, so they're cut into stretches that each look one way.
fn styles(text: &types::FormattedText) -> Vec<Styled> {
    // Where each entity starts (+1) and ends (-1).
    let mut edges = Vec::new();
    for entity in &text.entities {
        let Some(mark) = Mark::of(&entity.r#type) else {
            continue;
        };
        // Entity offsets count UTF-16 code units, not bytes or chars.
        let start = byte_offset(&text.text, entity.offset);
        let end = byte_offset(&text.text, entity.offset.saturating_add(entity.length));
        if start < end {
            edges.push((start, true, mark));
            edges.push((end, false, mark));
        }
    }
    edges.sort_by_key(|&(at, _, _)| at);
    let mut open = [0u32; MARKS];
    let mut out: Vec<Styled> = Vec::new();
    let mut from = 0;
    for (at, starts, mark) in edges {
        if at > from {
            let format = Format::of(&open);
            if format != Format::default() {
                match out.last_mut() {
                    Some(last) if last.range.end == from && last.format == format => {
                        last.range.end = at;
                    }
                    _ => out.push(Styled {
                        range: from..at,
                        format,
                    }),
                }
            }
            from = at;
        }
        let count = &mut open[mark as usize];
        *count = if starts {
            count.saturating_add(1)
        } else {
            count.saturating_sub(1)
        };
    }
    out
}

/// What a spoiler shows until it's revealed: one of these per column, so the
/// text keeps its width.
pub const SPOILER: &str = "⠿";

/// `text` with its spoilers blotted out, for places that never reveal them:
/// snippets, chat previews, notifications.
fn hide_spoilers(text: &str, styles: &[Styled]) -> String {
    let mut out = String::with_capacity(text.len());
    let mut done = 0;
    for styled in styles.iter().filter(|s| s.format.spoiler) {
        let Some(hidden) = text.get(styled.range.clone()) else {
            continue;
        };
        out.push_str(&text[done..styled.range.start]);
        out.push_str(&SPOILER.repeat(hidden.width()));
        done = styled.range.end;
    }
    out.push_str(&text[done..]);
    out
}

/// A text or caption as plain text, spoilers blotted out.
pub fn without_spoilers(text: &types::FormattedText) -> String {
    hide_spoilers(&text.text, &styles(text))
}

/// What a link in a message leads to, as Telegram previews it under the
/// text: the page's title and the start of its description, beside a small
/// picture if it has one.
#[derive(Clone)]
pub struct Card {
    /// Where the link really goes, read from its address: the name a page
    /// gives itself could be anyone's. Always the host of one of the
    /// message's own links, which Enter opens.
    pub host: String,
    pub title: String,
    pub description: String,
    /// The page's picture, a video's cover, or the photo linked to.
    pub image: Option<Preview>,
}

impl Card {
    fn new(preview: &types::LinkPreview) -> Option<Self> {
        let host = link_host(&preview.url)?;
        let title = match one_line(&preview.title) {
            title if title.is_empty() => one_line(&preview.site_name),
            title => title,
        };
        let description = one_line(&without_spoilers(&preview.description));
        if title.is_empty() && description.is_empty() {
            return None;
        }
        let small = |photo: &types::Photo| Preview::sized(photo, THUMBNAIL_MIN_SIDE);
        let image = match &preview.r#type {
            LinkPreviewType::Article(a) => a.photo.as_ref().and_then(small),
            LinkPreviewType::Photo(p) => small(&p.photo),
            LinkPreviewType::Video(v) => v
                .cover
                .as_ref()
                .and_then(small)
                .or_else(|| Preview::from_video(&v.video)),
            _ => None,
        };
        Some(Card {
            host: text::clean(&host),
            title,
            description,
            image,
        })
    }
}

/// Who a forwarded message first came from.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Origin {
    User(i64),
    /// Someone whose privacy settings hide their account: only the name.
    Hidden(String),
    /// A channel, or a group's anonymous admin, with the author's name if
    /// the post was signed.
    Chat {
        chat_id: i64,
        signature: String,
    },
}

impl From<&MessageOrigin> for Origin {
    fn from(origin: &MessageOrigin) -> Self {
        match origin {
            MessageOrigin::User(o) => Origin::User(o.sender_user_id),
            MessageOrigin::HiddenUser(o) => Origin::Hidden(text::clean(&o.sender_name)),
            MessageOrigin::Chat(o) => Origin::Chat {
                chat_id: o.sender_chat_id,
                signature: text::clean(&o.author_signature),
            },
            MessageOrigin::Channel(o) => Origin::Chat {
                chat_id: o.chat_id,
                signature: text::clean(&o.author_signature),
            },
        }
    }
}

/// The text on one line, at most [`SNIPPET_CHARS`] long: a quote or a popup
/// shows only its start, and a sender's 4096 characters would be measured
/// again on every frame.
pub fn one_line(text: &str) -> String {
    let line = text::clean(text)
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ");
    text::first_chars(&line, SNIPPET_CHARS).to_string()
}

/// How much of a message a one-line snippet keeps.
const SNIPPET_CHARS: usize = 300;

/// A web link in a message.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Link {
    pub url: String,
    /// The words the link hides behind, when they aren't the URL itself.
    /// Opening it asks first and shows where it really goes, since
    /// `https://bank.com` can lead anywhere.
    pub disguise: Option<String>,
}

impl From<&str> for Link {
    fn from(url: &str) -> Self {
        Link {
            url: url.into(),
            disguise: None,
        }
    }
}

/// What a reply answers.
pub struct ReplyTo {
    /// The answered message, if it's in this chat. Replies to another chat's
    /// messages only get them from TDLib, since ids are per chat.
    pub message_id: Option<i64>,
    /// The part of the answered message the reply quotes, on one line, if
    /// the sender picked one.
    pub quote: Option<String>,
}

/// A message being replied to: who sent it and a line of what it said. For
/// the composer, it's copied when `r` is pressed, so the reply bar still
/// shows it after another part of the history loads.
#[derive(Clone)]
pub struct Replied {
    pub id: i64,
    pub sender: Sender,
    pub outgoing: bool,
    pub snippet: String,
}

impl Replied {
    pub fn new(id: i64, msg: &Msg) -> Self {
        Self {
            id,
            sender: msg.sender,
            outgoing: msg.outgoing,
            snippet: msg.snippet(),
        }
    }
}

/// A message's downloadable file, with what to call it in the open menu.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MediaFile {
    pub id: i32,
    pub label: String,
    /// A photo: copied as an image, not as a file.
    pub photo: bool,
}

struct Body {
    text: String,
    source_text: String,
    preview: Option<Preview>,
    file: Option<MediaFile>,
    links: Vec<Link>,
    link_ranges: Vec<Range<usize>>,
    styles: Vec<Styled>,
    poll: Option<Poll>,
    card: Option<Card>,
    editable: Editable,
    formatted: bool,
    /// The media shown only while it's open, when it's that.
    hidden: Option<Hidden>,
    voice: Option<Voice>,
}

/// The message as the bubble shows it. A photo or video shown only while
/// open is covered, with how it shows when open kept aside.
fn body(content: &MessageContent) -> Body {
    let mut body = body_as(content, true);
    if shown_while_open(content) {
        body.hidden = Some(Hidden {
            other: Box::new(body_as(content, false)),
            open: false,
            gone: false,
        });
    }
    body
}

/// A photo its sender wants seen only while it's open, which tuigram can
/// show that way: in the bubble. Videos would need another app, which
/// keeps them.
fn shown_while_open(content: &MessageContent) -> bool {
    matches!(content, MessageContent::MessagePhoto(m) if m.is_secret)
}

/// Where media seen only while open, which tuigram can't show that way,
/// can be.
pub const ON_PHONE: &str = "watch it in Telegram on your phone";

/// Media that expired: a view-once photo once it was opened.
fn expired(content: &MessageContent) -> bool {
    use MessageContent as C;
    matches!(
        content,
        C::MessageExpiredPhoto
            | C::MessageExpiredVideo
            | C::MessageExpiredVideoNote
            | C::MessageExpiredVoiceNote
    )
}

/// The message as the bubble shows it; media shown only while it's open is
/// named instead, if `cover`.
fn body_as(content: &MessageContent, cover: bool) -> Body {
    use MessageContent as C;
    let mut body = Body {
        text: content_text_as_sent(content),
        source_text: String::new(),
        preview: None,
        file: None,
        links: Vec::new(),
        link_ranges: Vec::new(),
        styles: Vec::new(),
        poll: None,
        card: None,
        editable: Editable::No,
        formatted: false,
        hidden: None,
        voice: None,
    };
    let file = |id: i32, label: String| {
        Some(MediaFile {
            id,
            label,
            photo: false,
        })
    };
    // The text or caption whose links count.
    let mut source = None;
    // What it is and what to do; the caption, if any, under it.
    let labeled = |label: String, caption: &types::FormattedText| {
        if caption.text.is_empty() {
            label
        } else {
            format!("{label}\n{}", caption.text)
        }
    };
    match content {
        C::MessagePhoto(m) if cover && m.is_secret => {
            body.text = labeled("[Photo · Enter to view]".into(), &m.caption);
            source = Some(&m.caption);
        }
        // Only another app could play these, and it would keep them.
        C::MessageVideo(m) if m.is_secret => {
            body.text = labeled(format!("[Video · {ON_PHONE}]"), &m.caption);
            source = Some(&m.caption);
        }
        C::MessageAnimation(m) if m.is_secret => {
            body.text = labeled(format!("[GIF · {ON_PHONE}]"), &m.caption);
            source = Some(&m.caption);
        }
        C::MessageVideoNote(m) if m.is_secret => {
            body.text = format!("[Video message · {ON_PHONE}]");
        }
        C::MessageText(m) => {
            source = Some(&m.text);
            body.editable = Editable::Text;
            body.card = m.link_preview.as_ref().and_then(Card::new);
        }
        C::MessagePhoto(m) => {
            body.editable = Editable::Caption {
                above: m.show_caption_above_media,
            };
            body.preview = Preview::from_photo(&m.photo);
            if body.preview.is_some() {
                body.text = m.caption.text.clone();
            }
            // One shown only while open is shown here, not handed to
            // another app that keeps it.
            body.file = largest(&m.photo)
                .filter(|_| !m.is_secret)
                .map(|s| MediaFile {
                    id: s.photo.id,
                    label: "Photo".into(),
                    photo: true,
                });
            source = Some(&m.caption);
        }
        C::MessageVideo(m) => {
            body.editable = Editable::Caption {
                above: m.show_caption_above_media,
            };
            body.preview = Preview::from_video(&m.video);
            if body.preview.is_some() {
                body.text = format!("▶ {}", duration(m.video.duration));
                if !m.caption.text.is_empty() {
                    body.text = format!("{}\n{}", body.text, m.caption.text);
                }
            }
            let label = format!("Video {}", duration(m.video.duration));
            body.file = file(m.video.video.id, label);
            source = Some(&m.caption);
        }
        C::MessageAnimation(m) => {
            body.editable = Editable::Caption {
                above: m.show_caption_above_media,
            };
            body.file = file(m.animation.animation.id, "GIF".into());
            source = Some(&m.caption);
        }
        C::MessageDocument(m) => {
            body.editable = Editable::Caption { above: false };
            let label = format!("File: {}", text::clean(&m.document.file_name));
            body.file = file(m.document.document.id, label);
            source = Some(&m.caption);
        }
        C::MessageAudio(m) => {
            let a = &m.audio;
            let name = if a.title.is_empty() {
                &a.file_name
            } else {
                &a.title
            };
            body.file = file(a.audio.id, format!("Audio: {}", text::clean(name)));
            body.editable = Editable::Caption { above: false };
            source = Some(&m.caption);
        }
        C::MessageVoiceNote(m) => {
            body.file = file(m.voice_note.voice.id, "Voice message".into());
            body.editable = Editable::Caption { above: false };
            source = Some(&m.caption);
            let voice = Voice::new(&m.voice_note, m.is_listened);
            // One tuigram plays shows its waveform, with the caption under it.
            if voice.ogg {
                body.text = m.caption.text.clone();
                body.voice = Some(voice);
            }
        }
        C::MessageVideoNote(m) => body.file = file(m.video_note.video.id, "Video message".into()),
        C::MessagePoll(m) => {
            let poll = Poll::new(&m.poll);
            body.text = poll.text();
            body.source_text = body.text.clone();
            body.poll = Some(poll);
        }
        C::MessageSticker(m) => {
            body.preview = Preview::from_sticker(&m.sticker);
            if body.preview.is_some() {
                body.text = String::new();
            }
            body.file = file(m.sticker.sticker.id, "Sticker".into());
        }
        _ => {}
    }
    if let Some(source) = source {
        body.source_text = text::clean(&source.text);
        body.formatted = source
            .entities
            .iter()
            .any(|e| !found_by_telegram(&e.r#type));
        let found = links(source);
        // The caption ends the shown text (after e.g. "[File] " or a video's
        // length), so its ranges shift by whatever comes before it.
        if body.text.ends_with(&source.text) {
            let shift = body.text.len() - source.text.len();
            body.link_ranges = found
                .iter()
                .map(|(_, r)| r.start + shift..r.end + shift)
                .collect();
            // Drawing looks ranges up by position.
            body.link_ranges.sort_by_key(|r| r.start);
            body.styles = styles(source);
            for styled in &mut body.styles {
                styled.range = styled.range.start + shift..styled.range.end + shift;
            }
        }
        for (link, _) in found {
            match body.links.iter_mut().find(|l| l.url == link.url) {
                // The same URL also behind other words keeps its warning,
                // whichever came first.
                Some(seen) => {
                    if seen.disguise.is_none() {
                        seen.disguise = link.disguise;
                    }
                }
                None => body.links.push(link),
            }
        }
    }
    // A sender can attach a preview of another page than the links in the
    // text, and Enter opens those links, not the preview's: a preview of
    // paypal.com under a link to a look-alike would vouch for it. So it
    // shows only when it's of one of the text's own links.
    let bare = |host: &str| host.strip_prefix("www.").unwrap_or(host).to_string();
    if let Some(card) = &body.card
        && !body
            .links
            .iter()
            .filter_map(|l| link_host(&l.url))
            .any(|host| bare(&text::clean(&host)) == bare(&card.host))
    {
        body.card = None;
    }
    let ranges = body
        .link_ranges
        .iter_mut()
        .chain(body.styles.iter_mut().map(|s| &mut s.range));
    body.text = normalize(&body.text, ranges);
    // Formatting on nothing but hidden characters is gone with them.
    body.styles.retain(|s| !s.range.is_empty());
    body
}

/// Entities Telegram finds in plain text by itself, so an edit sent as
/// plain text gets them back. Any other kind is formatting the sender chose.
pub fn found_by_telegram(kind: &TextEntityType) -> bool {
    use TextEntityType as T;
    matches!(
        kind,
        T::Mention
            | T::Hashtag
            | T::Cashtag
            | T::BotCommand
            | T::Url
            | T::EmailAddress
            | T::PhoneNumber
            | T::BankCardNumber
            | T::MediaTimestamp(_)
    )
}

/// Tabs become spaces and hidden characters ([`text::is_hidden`], `\r`
/// among them) go, so terminal widths add up. Link and formatting ranges
/// move along with the text.
fn normalize<'a>(text: &str, ranges: impl IntoIterator<Item = &'a mut Range<usize>>) -> String {
    if !text.contains(|c| c == '\t' || text::is_hidden(c)) {
        return text.to_string();
    }
    let mut out = String::with_capacity(text.len());
    // Old byte offset -> new byte offset.
    let mut map = vec![0; text.len() + 1];
    for (i, c) in text.char_indices() {
        map[i..i + c.len_utf8()].fill(out.len());
        match c {
            '\t' => out.push_str("    "),
            c if text::is_hidden(c) => {}
            c => out.push(c),
        }
    }
    map[text.len()] = out.len();
    for range in ranges {
        *range = map[range.start]..map[range.end];
    }
    out
}

/// Web links Telegram marked in a text, with the byte range they cover:
/// plain URLs, and links hidden behind words. Only http(s), so a crafted link
/// can't get the OS to open a local file or app.
fn links(text: &types::FormattedText) -> Vec<(Link, Range<usize>)> {
    let mut out = Vec::new();
    for entity in &text.entities {
        // Entity offsets count UTF-16 code units, not bytes or chars.
        let start = byte_offset(&text.text, entity.offset);
        let end = byte_offset(&text.text, entity.offset.saturating_add(entity.length));
        // TDLib checks entities, but a bad one mustn't crash the app.
        let Some(shown) = text.text.get(start..end).filter(|s| !s.is_empty()) else {
            continue;
        };
        let (url, hidden) = match &entity.r#type {
            TextEntityType::Url => (shown, false),
            TextEntityType::TextUrl(t) => (t.url.as_str(), true),
            _ => continue,
        };
        if let Some(url) = web_url(url) {
            let disguise = (hidden && !same_place(shown, &url)).then(|| one_line(shown));
            out.push((Link { url, disguise }, start..end));
        }
    }
    out
}

/// Whether link text spells out the URL it leads to, give or take the
/// scheme, `www.`, a trailing slash and case.
pub fn same_place(shown: &str, url: &str) -> bool {
    let bare = |s: &str| {
        let s = s.trim().to_lowercase();
        let s = s
            .strip_prefix("https://")
            .or(s.strip_prefix("http://"))
            .unwrap_or(&s);
        let s = s.strip_prefix("www.").unwrap_or(s);
        s.trim_end_matches('/').to_string()
    };
    bare(shown) == bare(url)
}

/// Byte offset of a UTF-16 offset, clamped to the text.
pub fn byte_offset(text: &str, utf16: i32) -> usize {
    let mut units = 0;
    for (i, c) in text.char_indices() {
        if units >= utf16.max(0) as usize {
            return i;
        }
        units += c.len_utf16();
    }
    text.len()
}

/// `example.com/x` becomes `https://example.com/x`; other schemes are dropped.
pub fn web_url(url: &str) -> Option<String> {
    let url = text::clean(url);
    let url = url.trim();
    // A link with a line break in it would be copied as several lines, and
    // no web address has spaces.
    if url.contains(char::is_whitespace) {
        return None;
    }
    let lower = url.to_ascii_lowercase();
    if lower.starts_with("https://") || lower.starts_with("http://") {
        Some(url.to_string())
    } else if !url.is_empty() && !url.contains("://") && !url.contains(':') {
        Some(format!("https://{url}"))
    } else {
        None
    }
}

/// The host a web link really goes to, read the way browsers do: past the
/// scheme and any slashes, up to the first `/`, `\\`, `?` or `#`, after the
/// last `@` and without the port. Lowercased.
pub fn link_host(url: &str) -> Option<String> {
    let lower = url.to_ascii_lowercase();
    let rest = lower
        .strip_prefix("https:")
        .or_else(|| lower.strip_prefix("http:"))?;
    let rest = rest.trim_start_matches(['/', '\\']);
    let authority = &rest[..rest.find(['/', '\\', '?', '#']).unwrap_or(rest.len())];
    let host = authority
        .rsplit_once('@')
        .map_or(authority, |(_, host)| host);
    let host = match host.find(']') {
        Some(end) if host.starts_with('[') => &host[..=end],
        _ => host.rsplit_once(':').map_or(host, |(host, _)| host),
    };
    (!host.is_empty()).then(|| host.to_string())
}

/// `1:05`, or `1:02:05` past an hour.
pub fn duration(seconds: i32) -> String {
    let (h, m, s) = (seconds / 3600, seconds / 60 % 60, seconds % 60);
    if h > 0 {
        format!("{h}:{m:02}:{s:02}")
    } else {
        format!("{m}:{s:02}")
    }
}

impl From<&MessageSender> for Sender {
    fn from(sender: &MessageSender) -> Self {
        match sender {
            MessageSender::User(u) => Sender::User(u.user_id),
            MessageSender::Chat(c) => Sender::Chat(c.chat_id),
        }
    }
}

impl From<Message> for Msg {
    fn from(message: Message) -> Self {
        let destruct = Destruct::of(&message, SystemTime::now());
        let sender = Sender::from(&message.sender_id);
        let state = match message.sending_state {
            None => SendState::Sent,
            Some(MessageSendingState::Pending(_)) => SendState::Pending,
            Some(MessageSendingState::Failed(_)) => SendState::Failed,
        };
        // Replies to stories aren't shown.
        let reply_to = match message.reply_to {
            Some(MessageReplyTo::Message(r)) => Some(ReplyTo {
                message_id: (r.chat_id == message.chat_id).then_some(r.message_id),
                quote: r.quote.map(|q| one_line(&without_spoilers(&q.text))),
            }),
            _ => None,
        };
        let body = body(&message.content);
        let service = Service::of(&message.content);
        Self {
            sender,
            outgoing: message.is_outgoing,
            date: message.date,
            text: service.as_ref().map_or(body.text, |s| s.label(sender)),
            source_text: body.source_text,
            preview: body.preview,
            file: body.file,
            links: body.links,
            link_ranges: body.link_ranges,
            styles: body.styles,
            revealed: false,
            forwarded: message.forward_info.map(|f| Origin::from(&f.origin)),
            poll: body.poll,
            card: body.card,
            state,
            reply_to,
            editable: body.editable,
            formatted: body.formatted,
            edited: message.edit_date != 0,
            album: message.media_album_id,
            reactions: reactions::from_info(message.interaction_info.as_ref()),
            keyboard: Keyboard::of(message.reply_markup.as_ref()),
            pinned: message.is_pinned,
            destruct,
            hidden: body.hidden,
            saveable: message.can_be_saved,
            voice: body.voice,
            service,
        }
    }
}

/// A replied message that isn't loaded, asked of TDLib.
pub enum Fetched {
    Loading,
    Found(Replied),
    /// TDLib couldn't find it: it was deleted, or is in a chat we can't see.
    Missing,
}

/// The top of the message view: `offset` lines into the block of `msg_id`.
/// Stored by message rather than line so it survives older messages loading above.
#[derive(Clone, Copy)]
pub struct ScrollAnchor {
    pub msg_id: i64,
    pub offset: usize,
}

/// The loaded messages are one unbroken stretch of the history. It usually
/// runs up to the newest message, but jumping to an old search match starts a
/// new stretch around it, and scrolling down then loads the newer ones.
pub struct OpenChat {
    pub chat_id: i64,
    /// In a forum, the topic open: only its messages are loaded, and what's
    /// sent goes there.
    pub topic: Option<i32>,
    /// The newest message a read receipt was sent for.
    pub seen: i64,
    pub messages: BTreeMap<i64, Msg>,
    /// Message under the cursor. `None` means "the newest one, and follow new arrivals".
    pub selected: Option<i64>,
    /// Scroll position from the last frame; the UI keeps it up to date.
    pub scroll: Option<ScrollAnchor>,
    /// The `getChatHistory` request in flight. Pages for any other request are
    /// stale and get dropped.
    pub loading: Option<Page>,
    /// There's nothing older than the oldest loaded message.
    pub all_loaded: bool,
    /// The loaded messages reach the newest one, so new arrivals join them.
    pub at_newest: bool,
    pub search: Option<MessageSearch>,
    /// Set with `r`; the next message sent answers this one.
    pub reply: Option<Replied>,
    /// Set with `e`; Enter saves the composer's text into this message.
    pub editing: Option<Editing>,
    /// Files the next message sends, with the composer's text as caption.
    pub attachments: Vec<Attachment>,
    /// The paste that added the last attachments, while Ctrl-z can turn
    /// them back into text.
    pub dropped: Option<Dropped>,
    /// Ctrl-t: the attached photos go as files, uncompressed. Ends with the
    /// message it was for.
    pub as_files: bool,
    /// How far files on their way to Telegram are, in percent, by TDLib
    /// file id.
    pub uploads: HashMap<i32, u8>,
    /// What replies answer when it isn't among the loaded messages, by the
    /// id of the reply.
    pub replied: HashMap<i64, Fetched>,
    /// The commands of the chat's bots, for `/` completion.
    pub commands: Commands,
    /// The chat's pinned messages, newest first, for the bar over it and
    /// `gp`.
    pub pinned: Vec<Pinned>,
    /// The last time the pinned messages were asked for, counted, so an
    /// older answer can't replace a newer one.
    pub pinned_asked: u32,
    /// The message whose photo, shown only while open, Enter opened. It's
    /// covered again once the cursor leaves it.
    pub viewing: Option<i64>,
    /// That message, and its photo's file, until the photo is downloaded:
    /// only then is the sender told it was opened, and its timer started.
    pub opening: Option<(i64, i32)>,
    /// In a secret chat opened with unread messages, the last one read:
    /// the cursor goes to the first after it once it's loaded, since
    /// reading them starts their self-destruct timers, and nothing is read
    /// until the cursor gets to the newest.
    pub unread_after: Option<i64>,
}

impl OpenChat {
    pub fn new(chat_id: i64) -> Self {
        Self {
            chat_id,
            topic: None,
            seen: 0,
            messages: BTreeMap::new(),
            selected: None,
            scroll: None,
            loading: None,
            all_loaded: false,
            at_newest: true,
            search: None,
            reply: None,
            editing: None,
            attachments: Vec::new(),
            dropped: None,
            as_files: false,
            uploads: HashMap::new(),
            replied: HashMap::new(),
            commands: Commands::NotAsked,
            pinned: Vec::new(),
            pinned_asked: 0,
            viewing: None,
            opening: None,
            unread_after: None,
        }
    }

    /// The chat, and in a forum the topic: what TDLib's answers are for.
    pub fn place(&self) -> (i64, Option<i32>) {
        (self.chat_id, self.topic)
    }

    /// Ctrl-z after a paste of file paths: takes back the files it
    /// attached, and gives the text that was pasted.
    pub fn undo_drop(&mut self) -> Option<String> {
        let dropped = self.dropped.take()?;
        let kept = self.attachments.len().saturating_sub(dropped.count);
        self.attachments.truncate(kept);
        Some(dropped.text)
    }

    /// Keeps up with an upload from TDLib's `updateFile`, which also comes
    /// for downloads and everything else about files.
    pub fn set_upload(&mut self, file: &types::File) {
        let total = if file.size > 0 {
            file.size
        } else {
            file.expected_size
        };
        if file.remote.is_uploading_active && total > 0 {
            // 100% only once the message is sent.
            let done = file.remote.uploaded_size.clamp(0, total) * 100 / total;
            self.uploads.insert(file.id, done.min(99) as u8);
        } else {
            self.uploads.remove(&file.id);
        }
    }

    /// How far the upload of a message's file is, while it's on its way.
    pub fn upload_progress(&self, msg: &Msg) -> Option<u8> {
        let photo = msg.preview.as_ref().map(|p| p.file_id);
        [msg.file.as_ref().map(|f| f.id), photo]
            .into_iter()
            .flatten()
            .find_map(|id| self.uploads.get(&id).copied())
    }

    /// What `e` edits: the message under the cursor, or in an album of
    /// photos, the one photo sent with the caption, since the caption shows
    /// under the last photo wherever the cursor is. Files and music in an
    /// album are drawn one by one, each with its own caption.
    pub fn edit_target(&self) -> Option<i64> {
        let id = self.cursor_id()?;
        let mut captioned = self
            .bubble(id)
            .into_iter()
            .filter(|(_, m)| !m.source_text.is_empty())
            .map(|(id, _)| id);
        match (captioned.next(), captioned.next()) {
            (Some(holder), None) => Some(holder),
            _ => Some(id),
        }
    }

    /// The message under the cursor: the selected one, else the newest.
    pub fn cursor_id(&self) -> Option<i64> {
        self.selected.or_else(|| self.newest_id())
    }

    /// Why message `id` can't be edited, as far as can be told without
    /// asking TDLib, which also knows whose it is and Telegram's time limits.
    pub fn cant_edit(&self, id: i64) -> Option<&'static str> {
        let msg = self.messages.get(&id)?;
        match msg.state {
            SendState::Pending => Some("Wait until it's sent"),
            SendState::Failed => Some("This message wasn't sent"),
            SendState::Sent if msg.editable == Editable::No => {
                Some("This message has no text to edit")
            }
            SendState::Sent => None,
        }
    }

    /// The messages drawn as one bubble with message `id`: its album of
    /// photos or videos, or just itself. Albums of files show each file as
    /// its own message.
    fn bubble(&self, id: i64) -> Vec<(i64, &Msg)> {
        let Some(msg) = self.messages.get(&id) else {
            return Vec::new();
        };
        if msg.album != 0 {
            let photos: Vec<(i64, &Msg)> = self
                .messages
                .iter()
                .filter(|(_, m)| m.album == msg.album)
                .map(|(&id, m)| (id, m))
                .collect();
            if photos
                .iter()
                .all(|(_, m)| m.preview.as_ref().is_some_and(|p| !p.sticker))
            {
                return photos;
            }
        }
        vec![(id, msg)]
    }

    /// What `f` forwards: message `id`, or its whole album, as far as it
    /// was sent.
    pub fn forward_ids(&self, id: i64) -> Vec<i64> {
        self.bubble(id)
            .into_iter()
            .filter(|(_, m)| m.state == SendState::Sent)
            .map(|(id, _)| id)
            .collect()
    }

    /// The bubble with message `id` has spoilers Enter hasn't shown.
    pub fn hides_spoilers(&self, id: i64) -> bool {
        self.bubble(id).iter().any(|(_, m)| m.hides_spoilers())
    }

    /// Shows the spoilers in the bubble with message `id`: an album's
    /// caption can be on another of its photos. Returns whether it had any.
    pub fn reveal_spoilers(&mut self, id: i64) -> bool {
        let hiding: Vec<i64> = self
            .bubble(id)
            .into_iter()
            .filter(|(_, m)| m.hides_spoilers())
            .map(|(id, _)| id)
            .collect();
        for id in &hiding {
            if let Some(msg) = self.messages.get_mut(id) {
                msg.revealed = true;
            }
        }
        !hiding.is_empty()
    }

    /// What `R` reacts to: the message under the cursor. An album of photos
    /// or videos is drawn as one bubble, so its reactions go where it
    /// already has some, else on its first photo, as in Telegram's apps.
    pub fn react_target(&self) -> Option<i64> {
        let bubble = self.bubble(self.cursor_id()?);
        bubble
            .iter()
            .find(|(_, m)| !m.reactions.is_empty())
            .or(bubble.first())
            .map(|&(id, _)| id)
    }

    /// What `X` takes back: your reactions on the message under the cursor,
    /// or on any photo of its album.
    pub fn your_reactions(&self) -> Vec<(i64, ReactionKind)> {
        self.cursor_id()
            .map_or_else(Vec::new, |id| self.your_reactions_on(id))
    }

    /// Your emoji reactions on message `id`, or on any photo of its album,
    /// as its bubble shows them, with the message each is on: what the `R`
    /// popup marks as yours, and Enter there takes back.
    pub fn your_emoji(&self, id: i64) -> Vec<(i64, String)> {
        self.your_reactions_on(id)
            .into_iter()
            .filter_map(|(id, kind)| match kind {
                ReactionKind::Emoji(emoji) => Some((id, emoji)),
                _ => None,
            })
            .collect()
    }

    /// Your reactions on message `id`, or on any photo of its album, with
    /// the message each is on. Not the paid one, which can't be taken back.
    fn your_reactions_on(&self, id: i64) -> Vec<(i64, ReactionKind)> {
        self.bubble(id)
            .into_iter()
            .flat_map(|(id, m)| {
                m.reactions
                    .iter()
                    .filter(|r| r.chosen && r.kind != ReactionKind::Paid)
                    .map(move |r| (id, r.kind.clone()))
            })
            .collect()
    }

    /// TDLib's `updateMessageInteractionInfo`: someone reacted, or took a
    /// reaction back.
    pub fn set_reactions(&mut self, message_id: i64, info: Option<&types::MessageInteractionInfo>) {
        if let Some(msg) = self.messages.get_mut(&message_id) {
            msg.reactions = reactions::from_info(info);
        }
    }

    /// TDLib says message `message_id` was changed.
    pub fn set_edited(&mut self, message_id: i64) {
        if let Some(msg) = self.messages.get_mut(&message_id) {
            msg.edited = true;
        }
    }

    /// A message was pinned or unpinned, by you or anyone. The list follows
    /// at once where it can; asking TDLib again brings it up to date.
    pub fn set_pinned(&mut self, message_id: i64, pinned: bool) {
        self.pinned.retain(|p| p.id != message_id);
        if let Some(msg) = self.messages.get_mut(&message_id) {
            msg.pinned = pinned;
            if pinned {
                let at = self.pinned.partition_point(|p| p.id > message_id);
                self.pinned.insert(at, Pinned::new(message_id, msg));
            }
        }
    }

    /// A bot changed a message's buttons, or took them away.
    pub fn set_keyboard(&mut self, message_id: i64, markup: Option<&ReplyMarkup>) {
        if let Some(msg) = self.messages.get_mut(&message_id) {
            msg.keyboard = Keyboard::of(markup);
        }
    }

    /// Where `gd` goes from the message under the cursor: (the reply, the
    /// message it answers), or why it can't go anywhere.
    pub fn replied_jump(&self) -> Result<(i64, i64), &'static str> {
        let from = self.cursor_id().ok_or("No message selected")?;
        let reply = self
            .messages
            .get(&from)
            .and_then(|m| m.reply_to.as_ref())
            .ok_or("Not a reply")?;
        let to = reply
            .message_id
            .ok_or("It answers a message in another chat")?;
        if !self.messages.contains_key(&to)
            && matches!(self.replied.get(&from), Some(Fetched::Missing))
        {
            return Err("The message it answers was deleted");
        }
        Ok((from, to))
    }

    /// Loaded replies whose answered message isn't loaded and hasn't been
    /// asked for yet. They're marked as loading; the caller asks TDLib.
    pub fn missing_replied(&mut self) -> Vec<i64> {
        let missing: Vec<i64> = self
            .messages
            .iter()
            .filter(|(id, msg)| {
                // A message still sending has a temporary id TDLib can't look up.
                msg.state == SendState::Sent
                    && !self.replied.contains_key(id)
                    && msg.reply_to.as_ref().is_some_and(|r| {
                        r.message_id
                            .is_none_or(|answered| !self.messages.contains_key(&answered))
                    })
            })
            .map(|(&id, _)| id)
            .collect();
        for &id in &missing {
            self.replied.insert(id, Fetched::Loading);
        }
        missing
    }

    /// TDLib's answer for what reply `reply_id` answers.
    pub fn set_replied(&mut self, reply_id: i64, replied: Option<Message>) {
        let fetched = match replied {
            Some(message) => Fetched::Found(Replied::new(message.id, &message.into())),
            None => Fetched::Missing,
        };
        self.replied.insert(reply_id, fetched);
    }

    pub fn insert(&mut self, message: Message) {
        self.add_new(message.id, message.into());
    }

    /// Adds a message that just arrived. While following new messages,
    /// only the newest [`MAX_FOLLOWED`] stay loaded (older ones load again on
    /// scrolling up), so a busy or spammed group can't grow memory and the
    /// layout done every frame without end.
    fn add_new(&mut self, id: i64, msg: Msg) {
        self.messages.insert(id, msg);
        // Not mid-request: a page must still join up.
        if self.loading.is_some() {
            return;
        }
        let loaded = self.messages.len();
        if self.selected.is_none() {
            while self.messages.len() > MAX_FOLLOWED {
                self.messages.pop_first();
                self.all_loaded = false;
            }
        } else if self.messages.len() > MAX_LOADED {
            // Reading older messages while new ones pour in: stop taking
            // them, as after a jump into the past. They load again on the way
            // down.
            while self.messages.len() > MAX_LOADED
                && self.messages.last_key_value().map(|(&id, _)| id) != self.selected
            {
                self.messages.pop_last();
            }
            self.at_newest = false;
        }
        if self.messages.len() < loaded {
            self.prune_replied();
        }
    }

    /// Forgets what unloaded replies answer.
    fn prune_replied(&mut self) {
        let messages = &self.messages;
        self.replied.retain(|id, _| messages.contains_key(id));
    }

    /// Adds a page of history, as (message id, message) pairs. A `Latest` or
    /// `Around` page replaces what was loaded, since it may not join up with it.
    pub fn add_page(&mut self, page: Page, messages: Vec<(i64, Msg)>) {
        let (oldest, newest) = (self.oldest_id(), self.newest_id());
        match page {
            Page::Latest => {
                // Keep anything that arrived after TDLib put the page together.
                let page_newest = messages.iter().map(|(id, _)| *id).max();
                self.messages.retain(|&id, _| Some(id) > page_newest);
                self.at_newest = true;
                self.all_loaded = messages.is_empty();
            }
            Page::Around(target) => {
                if messages.is_empty() {
                    return;
                }
                self.messages.clear();
                self.scroll = None;
                self.at_newest = false;
                self.all_loaded = false;
                self.messages.extend(messages);
                // The target, or the closest message if it's gone.
                self.selected = self
                    .messages
                    .range(..=target)
                    .next_back()
                    .or_else(|| self.messages.first_key_value())
                    .map(|(&id, _)| id);
                self.prune_replied();
                self.forget_replaced_view();
                return;
            }
            Page::Older(_) => {
                self.all_loaded = !messages
                    .iter()
                    .any(|(id, _)| oldest.is_none_or(|o| *id < o));
            }
            Page::Newer(_) => {
                self.at_newest = !messages
                    .iter()
                    .any(|(id, _)| newest.is_none_or(|n| *id > n));
            }
        }
        self.messages.extend(messages);
        self.prune_replied();
        self.forget_replaced_view();
    }

    /// Swaps a message sent under a temporary id for the server's version.
    pub fn replace(&mut self, old_id: i64, message: Message) {
        // Not loaded if an older part of the chat is shown; it loads with the rest.
        if self.messages.remove(&old_id).is_none() {
            return;
        }
        if self.selected == Some(old_id) {
            self.selected = Some(message.id);
        }
        // A reply to a message that was still sending goes to its real id.
        if let Some(reply) = self.reply.as_mut().filter(|r| r.id == old_id) {
            reply.id = message.id;
        }
        self.insert(message);
        self.forget_replaced_view();
    }

    pub fn set_content(&mut self, message_id: i64, content: &MessageContent) {
        let Some(msg) = self.messages.get_mut(&message_id) else {
            return;
        };
        let body = body(content);
        let once = msg.destruct.is_some_and(|d| d.after == 0);
        // A view-once photo expires as soon as it's opened: it stays on
        // screen until the cursor leaves it, as Telegram shows it until it's
        // let go of.
        if once
            && expired(content)
            && let Some(hidden) = msg.hidden.as_mut().filter(|h| h.open)
        {
            *hidden.other = body;
            hidden.gone = true;
            return;
        }
        msg.set_body(body);
        msg.service = Service::of(content);
        if let Some(service) = &msg.service {
            msg.text = service.label(msg.sender);
        }
        // New spoilers stay hidden until asked for again.
        msg.revealed = false;
        if self.viewing == Some(message_id) {
            self.viewing = None;
            self.opening = None;
        }
        if let Some(reply) = self.reply.as_mut().filter(|r| r.id == message_id) {
            reply.snippet = msg.snippet();
        }
    }

    /// Enter on a photo shown only while open: opens it until the cursor
    /// leaves it. Returns whether there was one, covered.
    pub fn uncover(&mut self, id: i64) -> bool {
        if self.viewing == Some(id) && self.is_open(id) {
            return false;
        }
        self.cover_viewed();
        let opened = self.messages.get_mut(&id).is_some_and(Msg::uncover);
        if opened {
            self.viewing = Some(id);
        }
        opened
    }

    /// Message `id` shows a photo that's shown only while open.
    fn is_open(&self, id: i64) -> bool {
        self.messages
            .get(&id)
            .and_then(|m| m.hidden.as_ref())
            .is_some_and(|h| h.open)
    }

    /// Covers what Enter opened once the cursor isn't on it any more, or
    /// nobody is `looking` at the chat.
    pub fn cover_unless_viewed(&mut self, looking: bool) {
        if self.viewing.is_some() && (!looking || self.viewing != self.cursor_id()) {
            self.cover_viewed();
        }
    }

    fn cover_viewed(&mut self) {
        self.opening = None;
        if let Some(msg) = self
            .viewing
            .take()
            .and_then(|id| self.messages.get_mut(&id))
        {
            msg.cover();
        }
    }

    /// Forgets what's viewed once its message was replaced by a new copy,
    /// which comes covered.
    fn forget_replaced_view(&mut self) {
        if let Some(id) = self.viewing
            && !self.is_open(id)
        {
            self.viewing = None;
            self.opening = None;
        }
    }

    /// A secret chat opened with unread messages: the cursor goes to the
    /// first unread one loaded, until it's found. Not when that's the
    /// newest, which is on screen anyway.
    pub fn go_to_unread(&mut self) {
        let Some(read) = self.unread_after else {
            return;
        };
        let Some(first) = self.messages.range(read + 1..).map(|(&id, _)| id).next() else {
            return;
        };
        let found = self.all_loaded || self.oldest_id().is_some_and(|o| o <= read);
        self.selected = match () {
            _ if found && self.at_newest && Some(first) == self.newest_id() => None,
            _ if found => Some(first),
            // Maybe older still: as far as is loaded, for now.
            _ => self.oldest_id(),
        };
        if found {
            self.unread_after = None;
        }
    }

    /// Starts the self-destruct timers TDLib starts when messages are read:
    /// of yours up to `up_to` once the other person read them (`outgoing`),
    /// or of theirs once you did. Media shown only while open starts its own
    /// when it's opened.
    pub fn start_timers(&mut self, outgoing: bool, up_to: i64, now: SystemTime) {
        for msg in self.messages.range_mut(..=up_to).map(|(_, m)| m) {
            if msg.outgoing == outgoing
                && msg.state == SendState::Sent
                && let Some(destruct) = msg.destruct.as_mut().filter(|d| !d.on_open)
            {
                destruct.start(now);
            }
        }
    }

    /// Playing voice message `id` is to tell its sender: it's someone
    /// else's, sent, and not played before.
    pub fn tells(&self, id: i64) -> bool {
        self.messages.get(&id).is_some_and(|msg| {
            !msg.outgoing
                && msg.state == SendState::Sent
                && msg.voice.as_ref().is_some_and(|v| !v.listened)
        })
    }

    /// Message `id` was opened, here or elsewhere: a voice message counts
    /// as played.
    pub fn set_opened(&mut self, id: i64) {
        if let Some(voice) = self.messages.get_mut(&id).and_then(|m| m.voice.as_mut()) {
            voice.listened = true;
        }
    }

    /// A message was opened, by you or the other person: its timer starts.
    pub fn start_timer(&mut self, id: i64, now: SystemTime) {
        if let Some(destruct) = self.messages.get_mut(&id).and_then(|m| m.destruct.as_mut()) {
            destruct.start(now);
        }
    }

    /// When the first countdown on a loaded message changes, for the screen
    /// to follow it.
    pub fn next_tick(&self, now: SystemTime) -> Option<Duration> {
        self.messages
            .values()
            .filter_map(|m| m.destruct?.changes_in(now))
            .min()
    }

    pub fn remove(&mut self, message_ids: &[i64]) {
        for id in message_ids {
            if self.viewing == Some(*id) {
                self.viewing = None;
                self.opening = None;
            }
            self.pinned.retain(|p| p.id != *id);
            self.messages.remove(id);
            if self.selected == Some(*id) {
                self.selected = None;
            }
            if self.reply.as_ref().is_some_and(|r| r.id == *id) {
                self.reply = None;
            }
        }
    }

    pub fn oldest_id(&self) -> Option<i64> {
        self.messages.keys().next().copied()
    }

    pub fn newest_id(&self) -> Option<i64> {
        self.messages.keys().next_back().copied()
    }

    /// Moves the cursor `delta` messages (negative = older), clamped to what's
    /// loaded. Returns the new position counted from the oldest message.
    pub fn move_cursor(&mut self, delta: isize) -> Option<usize> {
        let ids: Vec<i64> = self.messages.keys().copied().collect();
        let last = ids.len().checked_sub(1)?;
        let current = self
            .selected
            .and_then(|id| ids.binary_search(&id).ok())
            .unwrap_or(last);
        let index = current.saturating_add_signed(delta).min(last);
        // Landing on the newest message switches back to following.
        self.selected = (index != last || !self.at_newest).then(|| ids[index]);
        Some(index)
    }
}

/// A photo shown only while open, as TDLib sends one, for tests.
#[cfg(test)]
fn secret_photo(caption: &str) -> MessageContent {
    MessageContent::MessagePhoto(types::MessagePhoto {
        photo: types::Photo {
            sizes: vec![types::PhotoSize {
                r#type: "x".into(),
                photo: types::File {
                    id: 20,
                    ..Default::default()
                },
                width: 800,
                height: 600,
                progressive_sizes: Vec::new(),
            }],
            ..Default::default()
        },
        caption: types::FormattedText {
            text: caption.into(),
            ..Default::default()
        },
        is_secret: true,
        ..Default::default()
    })
}

/// A message that's a photo shown only while open, lasting `after`
/// seconds once opened (0 for view once), for tests.
#[cfg(test)]
pub fn test_secret_photo(after: i32) -> Msg {
    let mut msg = tests::page([1]).remove(0).1;
    msg.set_body(body(&secret_photo("for you")));
    msg.saveable = false;
    msg.destruct = Some(Destruct {
        after,
        on_open: true,
        ends: None,
    });
    msg
}

#[cfg(test)]
mod tests {
    use super::*;

    fn video(format: ThumbnailFormat, caption: &str) -> MessageContent {
        MessageContent::MessageVideo(types::MessageVideo {
            video: types::Video {
                duration: 65,
                width: 1920,
                height: 1080,
                file_name: "clip.mp4".into(),
                mime_type: "video/mp4".into(),
                has_stickers: false,
                supports_streaming: true,
                minithumbnail: None,
                thumbnail: Some(types::Thumbnail {
                    format,
                    width: 320,
                    height: 180,
                    file: types::File {
                        id: 9,
                        ..Default::default()
                    },
                }),
                video: types::File {
                    id: 10,
                    ..Default::default()
                },
            },
            alternative_videos: Vec::new(),
            storyboards: Vec::new(),
            cover: None,
            start_timestamp: 0,
            caption: types::FormattedText {
                text: caption.into(),
                ..Default::default()
            },
            show_caption_above_media: false,
            has_spoiler: false,
            is_secret: false,
        })
    }

    #[test]
    fn videos_preview_their_thumbnail_and_open_the_video() {
        let body = body(&video(ThumbnailFormat::Jpeg, "our trip"));
        let preview = body.preview.expect("jpeg thumbnails are shown");
        assert_eq!(preview.file_id, 9, "downloads the thumbnail");
        assert_eq!((preview.width, preview.height), (1920, 1080));
        assert_eq!(body.text, "▶ 1:05\nour trip");
        let file = body.file.unwrap();
        assert_eq!(
            (file.id, file.label.as_str()),
            (10, "Video 1:05"),
            "Enter opens the video itself"
        );
    }

    #[test]
    fn videos_with_clip_thumbnails_fall_back_to_a_label() {
        let body = body(&video(ThumbnailFormat::Mpeg4, ""));
        assert!(body.preview.is_none());
        assert_eq!(body.text, "[Video]");
        assert_eq!(body.file.map(|f| f.id), Some(10));
    }

    fn entity(offset: i32, length: i32, kind: TextEntityType) -> types::TextEntity {
        types::TextEntity {
            offset,
            length,
            r#type: kind,
        }
    }

    #[test]
    fn links_come_from_telegram_entities() {
        // "🎉" is 2 UTF-16 units, so the URL starts at offset 9, not 8.
        let text = "🎉 see: example.com/a and docs, then https://x.dev twice: https://x.dev";
        let at = |needle: &str| {
            let byte = text.find(needle).unwrap();
            text[..byte].encode_utf16().count() as i32
        };
        let text = types::FormattedText {
            text: text.into(),
            entities: vec![
                entity(at("example.com"), 13, TextEntityType::Url),
                entity(
                    at("docs"),
                    4,
                    TextEntityType::TextUrl(types::TextEntityTypeTextUrl {
                        url: "https://docs.rs".into(),
                    }),
                ),
                entity(at("https://x.dev"), 13, TextEntityType::Url),
                entity(
                    text.rfind("https")
                        .map(|b| text[..b].encode_utf16().count())
                        .unwrap() as i32,
                    13,
                    TextEntityType::Url,
                ),
                entity(
                    0,
                    2,
                    TextEntityType::TextUrl(types::TextEntityTypeTextUrl {
                        url: "file:///etc/passwd".into(),
                    }),
                ),
            ],
        };
        let found = links(&text);
        let urls: Vec<&str> = found.iter().map(|(l, _)| l.url.as_str()).collect();
        assert_eq!(
            urls,
            [
                "https://example.com/a",
                "https://docs.rs",
                "https://x.dev",
                "https://x.dev"
            ],
            "scheme added, hidden link kept, file:// dropped"
        );
        let shown: Vec<&str> = found.iter().map(|(_, r)| &text.text[r.clone()]).collect();
        assert_eq!(
            shown,
            ["example.com/a", "docs", "https://x.dev", "https://x.dev"]
        );
    }

    #[test]
    fn links_hidden_behind_other_words_are_marked_and_bad_entities_skipped() {
        let text_url = |offset, length, url: &str| types::TextEntity {
            offset,
            length,
            r#type: TextEntityType::TextUrl(types::TextEntityTypeTextUrl { url: url.into() }),
        };
        let text = types::FormattedText {
            text: "bank.com Example.com/ here".into(),
            entities: vec![
                text_url(0, 8, "https://evil.example"),
                text_url(9, 12, "https://www.example.com"),
                text_url(22, 4, "https://elsewhere.dev"),
                // Broken: negative or past-the-end lengths.
                text_url(5, -3, "https://a.b"),
                text_url(30, 4, "https://c.d"),
            ],
        };
        let found: Vec<Link> = links(&text).into_iter().map(|(l, _)| l).collect();
        assert_eq!(
            found,
            [
                Link {
                    url: "https://evil.example".into(),
                    disguise: Some("bank.com".into()),
                },
                Link::from("https://www.example.com"),
                Link {
                    url: "https://elsewhere.dev".into(),
                    disguise: Some("here".into()),
                },
            ]
        );
    }

    #[test]
    fn a_hidden_link_keeps_its_warning_when_its_url_is_also_shown_plainly() {
        let text = |entities| {
            body(&MessageContent::MessageText(types::MessageText {
                text: types::FormattedText {
                    text: "evil.example then bank.com".into(),
                    entities,
                },
                link_preview: None,
                link_preview_options: None,
            }))
        };
        let hidden = TextEntityType::TextUrl(types::TextEntityTypeTextUrl {
            url: "https://evil.example".into(),
        });
        let both = text(vec![
            entity(0, 12, TextEntityType::Url),
            entity(18, 8, hidden),
        ]);
        assert_eq!(
            both.links,
            [Link {
                url: "https://evil.example".into(),
                disguise: Some("bank.com".into()),
            }]
        );
    }

    #[test]
    fn a_links_host_is_read_the_way_browsers_read_it() {
        let host = |url| link_host(url);
        assert_eq!(
            host("https://bank.com.secure-login.evil.example/x").as_deref(),
            Some("bank.com.secure-login.evil.example")
        );
        assert_eq!(
            host("https://bank.com@evil.example/").as_deref(),
            Some("evil.example")
        );
        assert_eq!(
            host("https://evil.example\\@bank.com/").as_deref(),
            Some("evil.example")
        );
        assert_eq!(
            host("HTTPS:///Evil.Example:8443?x").as_deref(),
            Some("evil.example")
        );
        assert_eq!(host("http://[::1]:80/").as_deref(), Some("[::1]"));
        assert_eq!(host("https:///"), None);
        assert_eq!(host("ftp://a.example"), None);
    }

    #[test]
    fn urls_with_spaces_or_line_breaks_are_not_links() {
        assert_eq!(web_url("https://a.example/x y"), None);
        assert_eq!(web_url("a.example/\nx"), None);
        assert_eq!(
            web_url("a.example/x").as_deref(),
            Some("https://a.example/x")
        );
    }

    fn sticker(format: StickerFormat, thumbnail: Option<ThumbnailFormat>) -> MessageContent {
        MessageContent::MessageSticker(types::MessageSticker {
            sticker: types::Sticker {
                id: 1,
                set_id: 2,
                width: 512,
                height: 512,
                emoji: "😀".into(),
                format,
                full_type: tdlib_rs::enums::StickerFullType::Regular(
                    types::StickerFullTypeRegular {
                        premium_animation: None,
                    },
                ),
                thumbnail: thumbnail.map(|format| types::Thumbnail {
                    format,
                    width: 128,
                    height: 128,
                    file: types::File {
                        id: 7,
                        ..Default::default()
                    },
                }),
                sticker: types::File {
                    id: 8,
                    ..Default::default()
                },
            },
            is_premium: false,
        })
    }

    #[test]
    fn stickers_show_the_image_or_a_still_thumbnail() {
        let still = body(&sticker(StickerFormat::Webp, None));
        let preview = still.preview.expect("WebP stickers are images");
        assert_eq!((preview.file_id, preview.sticker), (8, true));
        assert_eq!(still.text, "", "no caption under a sticker");

        let animated = body(&sticker(StickerFormat::Tgs, Some(ThumbnailFormat::Webp)));
        assert_eq!(animated.preview.map(|p| p.file_id), Some(7), "thumbnail");

        let unknown = body(&sticker(StickerFormat::Tgs, Some(ThumbnailFormat::Tgs)));
        assert!(unknown.preview.is_none());
        assert_eq!(unknown.text, "[Sticker 😀]");
    }

    #[test]
    fn the_sticker_panel_shows_a_thumbnail_or_the_still_sticker_itself() {
        let shown = |format, thumbnail| {
            let MessageContent::MessageSticker(m) = sticker(format, thumbnail) else {
                unreachable!();
            };
            Preview::sticker_thumbnail(&m.sticker).map(|p| p.file_id)
        };
        let webp = Some(ThumbnailFormat::Webp);
        assert_eq!(
            shown(StickerFormat::Webp, webp),
            Some(7),
            "the smaller file"
        );
        assert_eq!(shown(StickerFormat::Webp, None), Some(8));
        assert_eq!(shown(StickerFormat::Tgs, Some(ThumbnailFormat::Tgs)), None);
    }

    #[test]
    fn messages_know_what_an_edit_can_change() {
        let text = |entities| {
            body(&MessageContent::MessageText(types::MessageText {
                text: types::FormattedText {
                    text: "see x.dev now".into(),
                    entities,
                },
                link_preview: None,
                link_preview_options: None,
            }))
        };
        let plain = text(vec![entity(4, 5, TextEntityType::Url)]);
        assert_eq!(plain.editable, Editable::Text);
        assert!(!plain.formatted, "Telegram finds the link again by itself");
        assert!(text(vec![entity(0, 3, TextEntityType::Bold)]).formatted);

        let video = body(&video(ThumbnailFormat::Jpeg, "our trip"));
        assert_eq!(video.editable, Editable::Caption { above: false });
        let sticker = body(&sticker(StickerFormat::Tgs, Some(ThumbnailFormat::Tgs)));
        assert_eq!(sticker.editable, Editable::No);
    }

    #[test]
    fn only_sent_messages_with_words_can_be_edited() {
        let mut open = OpenChat::new(1);
        let mut msgs = page([1, 2, 3, 4]);
        msgs[1].1.state = SendState::Pending;
        msgs[2].1.state = SendState::Failed;
        msgs[3].1.editable = Editable::No;
        open.messages.extend(msgs);
        assert_eq!(open.cant_edit(1), None, "TDLib decides the rest");
        assert_eq!(open.cant_edit(2), Some("Wait until it's sent"));
        assert_eq!(open.cant_edit(3), Some("This message wasn't sent"));
        assert_eq!(open.cant_edit(4), Some("This message has no text to edit"));

        open.set_edited(1);
        assert!(open.messages[&1].edited);
    }

    #[test]
    fn caption_links_line_up_after_a_label() {
        let caption = types::FormattedText {
            text: "see x.dev".into(),
            entities: vec![entity(4, 5, TextEntityType::Url)],
        };
        let content = MessageContent::MessageDocument(types::MessageDocument {
            document: types::Document {
                file_name: "a.pdf".into(),
                mime_type: "application/pdf".into(),
                minithumbnail: None,
                thumbnail: None,
                document: types::File::default(),
            },
            caption,
        });
        let body = body(&content);
        assert_eq!(body.text, "[File: a.pdf] see x.dev");
        assert_eq!(body.source_text, "see x.dev", "copies only the caption");
        let range = body.link_ranges[0].clone();
        assert_eq!(&body.text[range], "x.dev");
    }

    fn text_message(text: &str, entities: Vec<types::TextEntity>) -> MessageContent {
        MessageContent::MessageText(types::MessageText {
            text: types::FormattedText {
                text: text.into(),
                entities,
            },
            link_preview: None,
            link_preview_options: None,
        })
    }

    /// The formatted parts of a body's text, with what each looks like.
    fn styled(body: &Body) -> Vec<(&str, Format)> {
        body.styles
            .iter()
            .map(|s| (&body.text[s.range.clone()], s.format))
            .collect()
    }

    #[test]
    fn nested_and_overlapping_formatting_is_cut_into_stretches() {
        use TextEntityType as T;
        // "🎉" is 2 UTF-16 units: "bold" starts at offset 3.
        let content = text_message(
            "🎉 bold both italic code",
            vec![
                entity(3, 9, T::Bold),
                entity(8, 11, T::Italic),
                entity(20, 4, T::Code),
                entity(0, 0, T::Strikethrough),
                entity(50, 4, T::Underline),
            ],
        );
        let body = body(&content);
        let bold = Format {
            bold: true,
            ..Format::default()
        };
        let italic = Format {
            italic: true,
            ..Format::default()
        };
        let code = Format {
            code: true,
            ..Format::default()
        };
        let both = Format {
            bold: true,
            italic: true,
            ..Format::default()
        };
        assert_eq!(
            styled(&body),
            [
                ("bold ", bold),
                ("both", both),
                (" italic", italic),
                ("code", code),
            ],
            "empty and out-of-range entities are skipped"
        );
        assert!(body.formatted);
    }

    #[test]
    fn caption_formatting_lines_up_after_a_label_and_expanded_tabs() {
        let caption = types::FormattedText {
            text: "a\tb bold".into(),
            entities: vec![entity(4, 4, TextEntityType::Bold)],
        };
        let content = MessageContent::MessageDocument(types::MessageDocument {
            document: types::Document {
                file_name: "a.pdf".into(),
                mime_type: "application/pdf".into(),
                minithumbnail: None,
                thumbnail: None,
                document: types::File::default(),
            },
            caption,
        });
        let body = body(&content);
        assert_eq!(body.text, "[File: a.pdf] a    b bold");
        assert_eq!(styled(&body)[0].0, "bold");
    }

    #[test]
    fn spoilers_stay_hidden_everywhere_but_the_bubble() {
        let content = text_message(
            "the butler did it",
            vec![entity(4, 6, TextEntityType::Spoiler)],
        );
        assert_eq!(crate::chats::content_text(&content), "the ⠿⠿⠿⠿⠿⠿ did it");
        let body = body(&content);
        assert_eq!(
            body.text, "the butler did it",
            "the bubble blots it out itself"
        );

        let mut msg = page([1]).remove(0).1;
        (msg.text, msg.styles) = (body.text, body.styles);
        assert_eq!(msg.snippet(), "the ⠿⠿⠿⠿⠿⠿ did it");
        assert!(msg.hides_spoilers());
        msg.revealed = true;
        assert!(!msg.hides_spoilers());
        assert_eq!(msg.snippet(), "the ⠿⠿⠿⠿⠿⠿ did it", "snippets never reveal");

        // As wide as what it hides.
        let wide = text_message("答えは東京", vec![entity(3, 2, TextEntityType::Spoiler)]);
        assert_eq!(crate::chats::content_text(&wide), "答えは⠿⠿⠿⠿");
    }

    #[test]
    fn enter_shows_the_spoilers_of_the_whole_album() {
        let mut open = OpenChat::new(1);
        let mut msgs = page([1, 2, 3]);
        for (_, msg) in &mut msgs[..2] {
            msg.album = 7;
            msg.preview = Some(Preview {
                file_id: 1,
                width: 10,
                height: 10,
                thumbnail: None,
                sticker: false,
            });
        }
        // The caption, with its spoiler, is on the first photo.
        msgs[0].1.styles = vec![Styled {
            range: 0..7,
            format: Format {
                spoiler: true,
                ..Format::default()
            },
        }];
        open.messages.extend(msgs);
        assert!(open.hides_spoilers(2));
        assert!(!open.hides_spoilers(3));
        assert!(open.reveal_spoilers(2), "from the other photo");
        assert!(open.messages[&1].revealed);
        assert!(!open.reveal_spoilers(2), "nothing left to show");
    }

    #[test]
    fn a_link_preview_shows_only_for_a_link_in_the_message() {
        let with_preview = |text: &str, preview_url: &str| {
            let start = text.find("https://").unwrap_or(0);
            let entities = if text.contains("https://") {
                let length = (text.len() - start) as i32;
                vec![entity(start as i32, length, TextEntityType::Url)]
            } else {
                Vec::new()
            };
            body(&MessageContent::MessageText(types::MessageText {
                text: types::FormattedText {
                    text: text.into(),
                    entities,
                },
                link_preview: Some(types::LinkPreview {
                    url: preview_url.into(),
                    display_url: preview_url.into(),
                    site_name: "Site".into(),
                    title: "Log in".into(),
                    description: types::FormattedText::default(),
                    author: String::new(),
                    r#type: LinkPreviewType::Unsupported,
                    has_large_media: false,
                    show_large_media: false,
                    show_media_above_description: false,
                    skip_confirmation: false,
                    show_above_text: false,
                    instant_view_version: 0,
                }),
                link_preview_options: None,
            }))
        };
        let shown = with_preview("see https://www.example.com/a", "https://example.com/a");
        assert_eq!(shown.card.map(|c| c.host).as_deref(), Some("example.com"));
        let elsewhere = with_preview(
            "sign in: https://paypa1-login.example/x",
            "https://www.paypal.com/signin",
        );
        assert!(
            elsewhere.card.is_none(),
            "it would vouch for the other link"
        );
        assert!(
            with_preview("no link here", "https://example.com")
                .card
                .is_none()
        );
    }

    /// A page of plain messages with these ids.
    pub(super) fn page(ids: impl IntoIterator<Item = i64>) -> Vec<(i64, Msg)> {
        ids.into_iter()
            .map(|id| {
                let msg = Msg {
                    sender: Sender::User(1),
                    outgoing: false,
                    date: 0,
                    text: format!("message {id}"),
                    source_text: format!("message {id}"),
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
                    keyboard: None,
                    pinned: false,
                    destruct: None,
                    hidden: None,
                    saveable: true,
                    voice: None,
                    service: None,
                };
                (id, msg)
            })
            .collect()
    }

    fn ids(open: &OpenChat) -> Vec<i64> {
        open.messages.keys().copied().collect()
    }

    #[test]
    fn a_busy_chat_keeps_only_the_newest_messages_while_following_them() {
        let mut open = OpenChat::new(1);
        open.add_page(Page::Latest, page(0..10));
        for (id, msg) in page(10..MAX_FOLLOWED as i64 + 50) {
            open.add_new(id, msg);
        }
        assert_eq!(open.messages.len(), MAX_FOLLOWED);
        assert_eq!(open.newest_id(), Some(MAX_FOLLOWED as i64 + 49));
        assert!(
            !open.all_loaded,
            "the dropped ones load again on scrolling up"
        );

        // Reading further up, nothing goes, up to a point.
        open.selected = open.oldest_id();
        for (id, msg) in page(5000..5010) {
            open.add_new(id, msg);
        }
        assert_eq!(open.messages.len(), MAX_FOLLOWED + 10);
        for (id, msg) in page(6000..6000 + MAX_LOADED as i64) {
            open.add_new(id, msg);
        }
        assert_eq!(open.messages.len(), MAX_LOADED, "then new ones wait");
        assert!(!open.at_newest, "and load again on the way down");
        assert_eq!(open.selected, open.oldest_id(), "the cursor stays put");
    }

    #[test]
    fn what_replies_answer_is_forgotten_with_the_replies() {
        let mut open = OpenChat::new(1);
        open.add_page(Page::Latest, page(0..10));
        open.set_replied(3, None);
        open.set_replied(4, None);
        for (id, msg) in page(10..MAX_FOLLOWED as i64 + 50) {
            open.add_new(id, msg);
        }
        assert!(open.replied.is_empty(), "replies 3 and 4 were unloaded");
    }

    #[test]
    fn jumping_to_an_old_message_loads_around_it() {
        let mut open = OpenChat::new(1);
        open.add_page(Page::Latest, page(90..100));
        assert!(open.at_newest);

        open.add_page(Page::Around(20), page(15..26));
        assert_eq!(
            ids(&open),
            (15..26).collect::<Vec<_>>(),
            "replaces the rest"
        );
        assert_eq!(open.selected, Some(20));
        assert!(!open.at_newest);

        // At the end of what's loaded, the cursor stays put instead of following.
        open.move_cursor(isize::MAX);
        assert_eq!(open.selected, Some(25));

        open.add_page(Page::Newer(25), page(25..40));
        assert!(!open.at_newest, "more came, there may be more still");
        open.add_page(Page::Newer(39), page(39..40));
        assert!(open.at_newest, "nothing newer: caught up");
        open.move_cursor(isize::MAX);
        assert_eq!(open.selected, None, "follows new messages again");
    }

    #[test]
    fn a_jump_to_a_deleted_message_lands_next_to_it() {
        let mut open = OpenChat::new(1);
        open.add_page(Page::Around(20), page([17, 18, 22, 23]));
        assert_eq!(open.selected, Some(18));
        open.add_page(Page::Around(5), page([8, 9]));
        assert_eq!(open.selected, Some(8));
    }

    #[test]
    fn older_pages_tell_when_the_start_is_reached() {
        let mut open = OpenChat::new(1);
        open.add_page(Page::Latest, page(50..60));
        open.add_page(Page::Older(50), page(40..50));
        assert!(!open.all_loaded);
        // TDLib may repeat the message it started from.
        open.add_page(Page::Older(40), page([40]));
        assert!(open.all_loaded);
    }

    #[test]
    fn the_latest_page_keeps_messages_that_arrived_meanwhile() {
        let mut open = OpenChat::new(1);
        // Stale messages from an old stretch, plus one that just arrived.
        open.messages.extend(page([3, 4, 101]));
        open.add_page(Page::Latest, page(90..100));
        assert_eq!(ids(&open), (90..100).chain([101]).collect::<Vec<_>>());
    }

    #[test]
    fn snippets_put_a_message_on_one_line_or_name_what_it_holds() {
        let mut msgs = page([1]);
        let msg = &mut msgs[0].1;
        msg.text = "first line\n\nsecond\tline ".into();
        assert_eq!(msg.snippet(), "first line second line");

        msg.text = String::new();
        msg.file = Some(MediaFile {
            id: 3,
            label: "Photo".into(),
            photo: true,
        });
        assert_eq!(msg.snippet(), "Photo", "a photo without a caption");
    }

    #[test]
    fn uploads_count_up_to_99_percent_until_the_message_is_sent() {
        let mut open = OpenChat::new(1);
        let mut msg = page([1]).remove(0).1;
        msg.file = Some(MediaFile {
            id: 5,
            label: "File: map.pdf".into(),
            photo: false,
        });
        let mut file = types::File {
            id: 5,
            size: 2000,
            ..Default::default()
        };
        file.remote.is_uploading_active = true;
        file.remote.uploaded_size = 500;
        open.set_upload(&file);
        assert_eq!(open.upload_progress(&msg), Some(25));
        file.remote.uploaded_size = 2000;
        open.set_upload(&file);
        assert_eq!(open.upload_progress(&msg), Some(99));
        file.remote.is_uploading_active = false;
        file.remote.is_uploading_completed = true;
        open.set_upload(&file);
        assert_eq!(open.upload_progress(&msg), None);
    }

    #[test]
    fn e_in_an_album_edits_the_photo_with_the_caption() {
        let mut open = OpenChat::new(1);
        open.messages = page([1, 2, 3, 4]).into_iter().collect();
        for (id, caption) in [(1, "the view"), (2, ""), (3, "")] {
            let msg = open.messages.get_mut(&id).unwrap();
            msg.album = 9;
            msg.source_text = caption.into();
            msg.preview = Some(Preview {
                file_id: id as i32,
                width: 10,
                height: 10,
                thumbnail: None,
                sticker: false,
            });
        }
        open.messages.get_mut(&4).unwrap().source_text = "after".into();
        open.selected = Some(3);
        assert_eq!(open.edit_target(), Some(1));
        open.selected = Some(4);
        assert_eq!(open.edit_target(), Some(4), "not in the album");
        open.messages.get_mut(&2).unwrap().source_text = "second caption".into();
        open.selected = Some(3);
        assert_eq!(open.edit_target(), Some(3), "with two captions, its own");

        // Files in an album are each their own bubble, with their own caption.
        open.messages.get_mut(&2).unwrap().source_text = String::new();
        for id in [1, 2, 3] {
            open.messages.get_mut(&id).unwrap().preview = None;
        }
        open.selected = Some(2);
        assert_eq!(open.edit_target(), Some(2), "not file 1's caption");
    }

    #[test]
    fn r_in_an_album_reacts_where_it_already_has_reactions_else_on_its_first_photo() {
        let mut open = OpenChat::new(1);
        open.messages = page([1, 2, 3, 4]).into_iter().collect();
        for id in [1, 2, 3] {
            let msg = open.messages.get_mut(&id).unwrap();
            msg.album = 9;
            msg.preview = Some(Preview {
                file_id: id as i32,
                width: 10,
                height: 10,
                thumbnail: None,
                sticker: false,
            });
        }
        open.selected = Some(3);
        assert_eq!(open.react_target(), Some(1));
        let heart = Reaction {
            kind: ReactionKind::Emoji("❤".into()),
            count: 1,
            chosen: false,
        };
        open.messages.get_mut(&2).unwrap().reactions = vec![heart];
        assert_eq!(open.react_target(), Some(2));
        open.selected = Some(4);
        assert_eq!(open.react_target(), Some(4), "not in the album");

        // X takes back yours from any photo of the album, not others'.
        let mine = |emoji: &str| Reaction {
            kind: ReactionKind::Emoji(emoji.into()),
            count: 2,
            chosen: true,
        };
        open.messages.get_mut(&1).unwrap().reactions = vec![mine("👍")];
        open.messages.get_mut(&3).unwrap().reactions = vec![
            mine("🔥"),
            Reaction {
                kind: ReactionKind::Paid,
                count: 1,
                chosen: true,
            },
        ];
        assert!(open.your_reactions().is_empty(), "nothing of yours on 4");
        open.selected = Some(2);
        assert_eq!(
            open.your_reactions(),
            [
                (1, ReactionKind::Emoji("👍".into())),
                (3, ReactionKind::Emoji("🔥".into())),
            ]
        );
        // The R popup sees them too, wherever it reacts: picking 🔥 on photo 2
        // takes it back from photo 3 rather than adding a second one.
        assert_eq!(
            open.your_emoji(2),
            [(1, "👍".to_string()), (3, "🔥".to_string())]
        );

        let info = types::MessageInteractionInfo {
            view_count: 0,
            forward_count: 0,
            reply_info: None,
            reactions: None,
        };
        open.set_reactions(2, Some(&info));
        assert!(open.messages[&2].reactions.is_empty());
    }

    /// A chat whose newest message, 3, is a photo shown only while open,
    /// lasting `after` seconds once opened (0 for view once).
    fn voice_note(mime: &str, caption: &str) -> MessageContent {
        MessageContent::MessageVoiceNote(types::MessageVoiceNote {
            voice_note: types::VoiceNote {
                duration: 7,
                mime_type: mime.into(),
                ..Default::default()
            },
            caption: types::FormattedText {
                text: caption.into(),
                entities: Vec::new(),
            },
            is_listened: false,
        })
    }

    #[test]
    fn a_voice_message_tuigram_plays_has_a_waveform_and_others_open_elsewhere() {
        let ogg = body(&voice_note("audio/ogg", "hey"));
        assert_eq!(ogg.text, "hey", "the waveform says what it is");
        let voice = ogg.voice.expect("played here");
        assert_eq!((voice.duration, voice.listened), (7, false));
        assert!(ogg.file.is_some(), "y still copies it");

        let mp3 = body(&voice_note("audio/mpeg", "hey"));
        assert!(mp3.voice.is_none());
        assert_eq!(mp3.text, "[Voice message] hey");
        assert!(mp3.file.is_some(), "opened in another app");
    }

    #[test]
    fn only_someone_elses_voice_message_played_for_the_first_time_tells_them() {
        let mut open = OpenChat::new(1);
        open.add_page(Page::Latest, page([1, 2, 3, 4]));
        for (id, msg) in &mut open.messages {
            let mut voice = body(&voice_note("audio/ogg", "")).voice.unwrap();
            voice.listened = *id == 4;
            msg.voice = Some(voice);
        }
        open.messages.get_mut(&2).unwrap().outgoing = true;
        open.messages.get_mut(&3).unwrap().state = SendState::Pending;
        assert!(open.tells(1), "theirs, sent, not played");
        assert!(!open.tells(2), "yours");
        assert!(!open.tells(3), "not sent yet");
        assert!(!open.tells(4), "played before");
        assert!(!open.tells(9), "not loaded");
        // Played, here or on your phone: once is enough.
        open.set_opened(1);
        assert!(!open.tells(1));
    }

    fn chat_with_secret_photo(after: i32) -> OpenChat {
        let mut open = OpenChat::new(1);
        open.add_page(Page::Latest, page([1, 2]));
        open.messages.insert(3, test_secret_photo(after));
        open
    }

    #[test]
    fn a_photo_shown_only_while_open_is_covered_until_enter_and_once_the_cursor_leaves() {
        let mut open = chat_with_secret_photo(10);
        let photo = &open.messages[&3];
        assert_eq!(photo.text, "[Photo · Enter to view]\nfor you");
        assert!(photo.preview.is_none() && photo.file.is_none());
        assert_eq!(photo.snippet(), "[Photo · Enter to view] for you");

        assert!(open.uncover(3), "Enter on the newest message");
        let photo = &open.messages[&3];
        assert_eq!(photo.text, "for you");
        assert_eq!(photo.preview.as_ref().map(|p| p.file_id), Some(20));
        assert!(photo.file.is_none(), "not handed to an app that keeps it");
        assert!(!open.uncover(3), "already open");

        open.cover_unless_viewed(true);
        assert!(open.messages[&3].preview.is_some(), "still on it");
        open.selected = Some(2);
        open.cover_unless_viewed(true);
        assert!(open.messages[&3].preview.is_none(), "covered again");
        assert_eq!(open.viewing, None);
        assert!(open.uncover(3), "Enter opens it again while it lasts");
        open.cover_unless_viewed(false);
        assert!(
            open.messages[&3].preview.is_none(),
            "looking away covers it"
        );
    }

    #[test]
    fn a_view_once_photo_stays_on_screen_after_it_expires_until_the_cursor_leaves() {
        let mut open = chat_with_secret_photo(0);
        open.uncover(3);
        // TDLib expires it as soon as it's opened.
        open.set_content(3, &MessageContent::MessageExpiredPhoto);
        assert!(open.messages[&3].preview.is_some(), "still on screen");
        open.cover_unless_viewed(false);
        let photo = &open.messages[&3];
        assert_eq!(photo.text, "[Photo expired]");
        assert!(photo.preview.is_none() && photo.hidden.is_none());
        assert!(!open.uncover(3), "gone for good");

        // One with a timer goes when it runs out, open or not.
        let mut open = chat_with_secret_photo(10);
        open.uncover(3);
        open.set_content(3, &MessageContent::MessageExpiredPhoto);
        assert_eq!(open.messages[&3].text, "[Photo expired]");
        assert_eq!(open.viewing, None);
    }

    #[test]
    fn a_new_copy_of_an_open_photo_comes_covered_and_enter_opens_it_again() {
        let mut open = chat_with_secret_photo(10);
        open.uncover(3);
        open.add_page(Page::Newer(2), vec![(3, test_secret_photo(10))]);
        assert_eq!(open.viewing, None, "the copy is covered");
        assert!(open.uncover(3));
        assert!(open.messages[&3].preview.is_some(), "and stays open");
        assert_eq!(open.viewing, Some(3));
    }

    #[test]
    fn videos_shown_only_while_open_are_left_to_the_phone() {
        let mut content = video(ThumbnailFormat::Jpeg, "watch");
        if let MessageContent::MessageVideo(video) = &mut content {
            video.is_secret = true;
        }
        let body = body(&content);
        assert_eq!(body.text, format!("[Video · {ON_PHONE}]\nwatch"));
        assert!(body.preview.is_none() && body.file.is_none() && body.hidden.is_none());
    }

    #[test]
    fn a_secret_chat_opens_at_its_first_unread_message() {
        let mut open = OpenChat::new(1);
        open.unread_after = Some(5);
        open.add_page(Page::Latest, page(4..=9));
        open.go_to_unread();
        assert_eq!(open.selected, Some(6));
        assert_eq!(open.unread_after, None);

        // Only the newest is unread: it's on screen, following new ones.
        let mut open = OpenChat::new(1);
        open.unread_after = Some(8);
        open.add_page(Page::Latest, page(4..=9));
        open.go_to_unread();
        assert_eq!(open.selected, None);

        // Not loaded back that far yet: the oldest loaded, until it is.
        let mut open = OpenChat::new(1);
        open.unread_after = Some(2);
        open.add_page(Page::Latest, page(6..=9));
        open.go_to_unread();
        assert_eq!(open.selected, Some(6));
        assert_eq!(open.unread_after, Some(2));
        open.add_page(Page::Older(6), page(1..=5));
        open.go_to_unread();
        assert_eq!(open.selected, Some(3));
        assert_eq!(open.unread_after, None);
    }

    #[test]
    fn self_destruct_timers_start_once_read_and_short_ones_on_media_once_opened() {
        let now = SystemTime::UNIX_EPOCH + Duration::from_secs(1_800_000_000);
        let mut open = OpenChat::new(1);
        open.add_page(Page::Latest, page([1, 2, 3, 4]));
        for (id, outgoing, on_open) in [
            (1, false, false),
            (2, true, false),
            (3, false, true),
            (4, false, false),
        ] {
            let msg = open.messages.get_mut(&id).unwrap();
            msg.outgoing = outgoing;
            msg.destruct = Some(Destruct {
                after: 30,
                on_open,
                ends: None,
            });
        }
        let started = |open: &OpenChat| -> Vec<i64> {
            open.messages
                .iter()
                .filter(|(_, m)| m.destruct.is_some_and(|d| d.ends.is_some()))
                .map(|(&id, _)| id)
                .collect()
        };
        assert_eq!(open.next_tick(now), None, "nothing counts down yet");
        // You read up to 3.
        open.start_timers(false, 3, now);
        assert_eq!(
            started(&open),
            [1],
            "not yours, nor a voice message not played"
        );
        // They read yours.
        open.start_timers(true, 4, now);
        assert_eq!(started(&open), [1, 2]);
        // It was played.
        open.start_timer(3, now);
        assert_eq!(started(&open), [1, 2, 3]);
        assert_eq!(open.next_tick(now), Some(Duration::from_secs(1)));
    }

    #[test]
    fn ctrl_z_takes_back_only_the_files_the_paste_attached() {
        let file = |name: &str| Attachment {
            path: name.into(),
            name: name.into(),
            size: 1,
            kind: crate::attach::Kind::File,
            identity: Default::default(),
            image_id: 0,
        };
        let mut open = OpenChat::new(1);
        open.attachments = vec![file("chosen.pdf"), file("a.txt"), file("b.txt")];
        open.dropped = Some(Dropped {
            text: "/tmp/a.txt /tmp/b.txt".into(),
            count: 2,
        });
        assert_eq!(open.undo_drop().as_deref(), Some("/tmp/a.txt /tmp/b.txt"));
        let names: Vec<&str> = open.attachments.iter().map(|a| a.name.as_str()).collect();
        assert_eq!(names, ["chosen.pdf"], "the file attached before stays");
        assert_eq!(open.undo_drop(), None, "only once");
    }

    #[test]
    fn deleting_the_message_being_replied_to_ends_the_reply() {
        let mut open = OpenChat::new(1);
        open.add_page(Page::Latest, page(1..5));
        open.reply = Some(Replied::new(3, &open.messages[&3]));
        assert_eq!(open.reply.as_ref().unwrap().snippet, "message 3");

        open.remove(&[2]);
        assert!(open.reply.is_some(), "another message went");
        open.remove(&[3]);
        assert!(open.reply.is_none());
    }

    #[test]
    fn replied_messages_that_arent_loaded_are_asked_for_once() {
        let mut open = OpenChat::new(1);
        let mut messages = page(10..15);
        let mut reply = |at: usize, message_id| {
            messages[at].1.reply_to = Some(ReplyTo {
                message_id,
                quote: None,
            })
        };
        reply(1, Some(10)); // 11 answers 10, which is loaded
        reply(2, Some(3)); // 12 answers an older message
        reply(3, None); // 13 answers a message in another chat
        open.add_page(Page::Latest, messages);

        assert_eq!(open.missing_replied(), [12, 13]);
        assert!(open.missing_replied().is_empty(), "already asked");

        open.set_replied(12, None);
        assert!(matches!(open.replied[&12], Fetched::Missing));

        // Once it's gone from the loaded messages, it has to be asked for.
        open.remove(&[10]);
        assert_eq!(open.missing_replied(), [11]);
    }

    #[test]
    fn gd_goes_from_a_reply_to_the_message_it_answers() {
        let mut open = OpenChat::new(1);
        let mut messages = page(10..15);
        let mut reply = |at: usize, message_id| {
            messages[at].1.reply_to = Some(ReplyTo {
                message_id,
                quote: None,
            })
        };
        reply(1, Some(10));
        reply(2, Some(3)); // not loaded
        reply(3, None); // in another chat
        reply(4, Some(4)); // the newest, answering a deleted message
        open.add_page(Page::Latest, messages);
        open.replied.insert(14, Fetched::Missing);

        let from = |open: &mut OpenChat, id| {
            open.selected = Some(id);
            open.replied_jump()
        };
        assert_eq!(from(&mut open, 11), Ok((11, 10)));
        assert_eq!(from(&mut open, 12), Ok((12, 3)), "loads around it");
        assert_eq!(
            from(&mut open, 13),
            Err("It answers a message in another chat")
        );
        assert_eq!(from(&mut open, 10), Err("Not a reply"));
        open.selected = None;
        assert_eq!(
            open.replied_jump(),
            Err("The message it answers was deleted"),
            "no selection means the newest"
        );
    }

    #[test]
    fn pinning_keeps_the_list_newest_first_and_marks_the_message() {
        let mut open = OpenChat::new(1);
        open.add_page(Page::Latest, page(1..=6));
        let ids = |open: &OpenChat| open.pinned.iter().map(|p| p.id).collect::<Vec<_>>();
        for id in [2, 5, 3] {
            open.set_pinned(id, true);
        }
        assert_eq!(ids(&open), [5, 3, 2]);
        assert!(open.messages[&3].pinned);
        assert_eq!(open.pinned[1].snippet, "message 3");

        open.set_pinned(3, false);
        assert_eq!(ids(&open), [5, 2]);
        assert!(!open.messages[&3].pinned);
        open.remove(&[5]);
        assert_eq!(ids(&open), [2], "deleted, so no longer pinned");
        // Not loaded: asking TDLib again brings it.
        open.set_pinned(99, true);
        assert_eq!(ids(&open), [2]);
    }

    #[test]
    fn durations_read_like_a_clock() {
        assert_eq!(duration(5), "0:05");
        assert_eq!(duration(65), "1:05");
        assert_eq!(duration(3725), "1:02:05");
    }
}

//! History of the open chat, sorted by message id (which is chronological).

use std::collections::{BTreeMap, HashMap};
use std::ops::Range;

use base64::Engine;
use tdlib_rs::enums::{
    MessageContent, MessageReplyTo, MessageSender, MessageSendingState, StickerFormat,
    TextEntityType, ThumbnailFormat,
};
use tdlib_rs::types::{self, Message};

use crate::chats::content_text;
use crate::images::Thumbnail;
use crate::search::MessageSearch;
use crate::text;
use crate::tg::Page;

/// Download the smallest size at least this big (TDLib's "x", ~800px), sharp
/// enough for a bubble on a high-DPI screen without fetching the original.
const PHOTO_MIN_SIDE: i32 = 640;

/// Messages kept loaded while following new ones; see [`OpenChat::add_new`].
const MAX_FOLLOWED: usize = 1000;

#[derive(Clone, Copy, PartialEq, Eq)]
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
        let size = photo
            .sizes
            .iter()
            .filter(|s| s.width.max(s.height) >= PHOTO_MIN_SIDE)
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
fn decode_minithumbnail(mini: Option<&types::Minithumbnail>) -> Option<Thumbnail> {
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
    pub state: SendState,
    /// Set when this message is a reply.
    pub reply_to: Option<ReplyTo>,
}

impl Msg {
    /// The message on one line: its text, or else what it holds ("Photo").
    pub fn snippet(&self) -> String {
        let text = one_line(&self.text);
        match &self.file {
            _ if !text.is_empty() => text,
            Some(file) => file.label.clone(),
            None => "Message".into(),
        }
    }
}

fn one_line(text: &str) -> String {
    text::clean(text)
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

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
#[derive(Clone)]
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
}

fn body(content: &MessageContent) -> Body {
    use MessageContent as C;
    let mut body = Body {
        text: content_text(content),
        source_text: String::new(),
        preview: None,
        file: None,
        links: Vec::new(),
        link_ranges: Vec::new(),
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
    match content {
        C::MessageText(m) => source = Some(&m.text),
        C::MessagePhoto(m) => {
            body.preview = Preview::from_photo(&m.photo);
            if body.preview.is_some() {
                body.text = m.caption.text.clone();
            }
            body.file = largest(&m.photo).map(|s| MediaFile {
                id: s.photo.id,
                label: "Photo".into(),
                photo: true,
            });
            source = Some(&m.caption);
        }
        C::MessageVideo(m) => {
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
            body.file = file(m.animation.animation.id, "GIF".into());
            source = Some(&m.caption);
        }
        C::MessageDocument(m) => {
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
            source = Some(&m.caption);
        }
        C::MessageVoiceNote(m) => {
            body.file = file(m.voice_note.voice.id, "Voice message".into());
            source = Some(&m.caption);
        }
        C::MessageVideoNote(m) => body.file = file(m.video_note.video.id, "Video message".into()),
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
        let found = links(source);
        // The caption ends the shown text (after e.g. "[File] " or a video's
        // length), so its link ranges shift by whatever comes before it.
        if body.text.ends_with(&source.text) {
            let shift = body.text.len() - source.text.len();
            body.link_ranges = found
                .iter()
                .map(|(_, r)| r.start + shift..r.end + shift)
                .collect();
        }
        for (link, _) in found {
            if !body.links.iter().any(|l| l.url == link.url) {
                body.links.push(link);
            }
        }
    }
    body.text = normalize(&body.text, &mut body.link_ranges);
    body
}

/// Tabs become spaces and hidden characters ([`text::is_hidden`], `\r`
/// among them) go, so terminal widths add up. Link ranges move along with
/// the text.
fn normalize(text: &str, ranges: &mut [Range<usize>]) -> String {
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
fn same_place(shown: &str, url: &str) -> bool {
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
fn byte_offset(text: &str, utf16: i32) -> usize {
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
fn web_url(url: &str) -> Option<String> {
    let url = text::clean(url);
    let url = url.trim();
    let lower = url.to_ascii_lowercase();
    if lower.starts_with("https://") || lower.starts_with("http://") {
        Some(url.to_string())
    } else if !url.is_empty() && !url.contains("://") && !url.contains(':') {
        Some(format!("https://{url}"))
    } else {
        None
    }
}

/// `1:05`, or `1:02:05` past an hour.
fn duration(seconds: i32) -> String {
    let (h, m, s) = (seconds / 3600, seconds / 60 % 60, seconds % 60);
    if h > 0 {
        format!("{h}:{m:02}:{s:02}")
    } else {
        format!("{m}:{s:02}")
    }
}

impl From<Message> for Msg {
    fn from(message: Message) -> Self {
        let sender = match message.sender_id {
            MessageSender::User(u) => Sender::User(u.user_id),
            MessageSender::Chat(c) => Sender::Chat(c.chat_id),
        };
        let state = match message.sending_state {
            None => SendState::Sent,
            Some(MessageSendingState::Pending(_)) => SendState::Pending,
            Some(MessageSendingState::Failed(_)) => SendState::Failed,
        };
        // Replies to stories aren't shown.
        let reply_to = match message.reply_to {
            Some(MessageReplyTo::Message(r)) => Some(ReplyTo {
                message_id: (r.chat_id == message.chat_id).then_some(r.message_id),
                quote: r.quote.map(|q| one_line(&q.text.text)),
            }),
            _ => None,
        };
        let body = body(&message.content);
        Self {
            sender,
            outgoing: message.is_outgoing,
            date: message.date,
            text: body.text,
            source_text: body.source_text,
            preview: body.preview,
            file: body.file,
            links: body.links,
            link_ranges: body.link_ranges,
            state,
            reply_to,
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
    /// What replies answer when it isn't among the loaded messages, by the
    /// id of the reply.
    pub replied: HashMap<i64, Fetched>,
    /// Replies `gd` jumped away from, latest last, for Ctrl-o to go back to.
    pub jumps: Vec<i64>,
}

impl OpenChat {
    pub fn new(chat_id: i64) -> Self {
        Self {
            chat_id,
            seen: 0,
            messages: BTreeMap::new(),
            selected: None,
            scroll: None,
            loading: None,
            all_loaded: false,
            at_newest: true,
            search: None,
            reply: None,
            replied: HashMap::new(),
            jumps: Vec::new(),
        }
    }

    /// The message under the cursor: the selected one, else the newest.
    pub fn cursor_id(&self) -> Option<i64> {
        self.selected.or_else(|| self.newest_id())
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
        // Not mid-request: a page of older messages must still join up.
        if self.selected.is_none() && self.loading.is_none() {
            while self.messages.len() > MAX_FOLLOWED {
                self.messages.pop_first();
                self.all_loaded = false;
            }
        }
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
    }

    pub fn set_content(&mut self, message_id: i64, content: &MessageContent) {
        if let Some(msg) = self.messages.get_mut(&message_id) {
            let body = body(content);
            (msg.text, msg.preview, msg.file) = (body.text, body.preview, body.file);
            msg.source_text = body.source_text;
            (msg.links, msg.link_ranges) = (body.links, body.link_ranges);
            if let Some(reply) = self.reply.as_mut().filter(|r| r.id == message_id) {
                reply.snippet = msg.snippet();
            }
        }
    }

    pub fn remove(&mut self, message_ids: &[i64]) {
        for id in message_ids {
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

    /// A page of plain messages with these ids.
    fn page(ids: impl IntoIterator<Item = i64>) -> Vec<(i64, Msg)> {
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
                    state: SendState::Sent,
                    reply_to: None,
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

        // Reading further up, nothing goes.
        open.selected = open.oldest_id();
        for (id, msg) in page(5000..5010) {
            open.add_new(id, msg);
        }
        assert_eq!(open.messages.len(), MAX_FOLLOWED + 10);
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
    fn durations_read_like_a_clock() {
        assert_eq!(duration(5), "0:05");
        assert_eq!(duration(65), "1:05");
        assert_eq!(duration(3725), "1:02:05");
    }
}

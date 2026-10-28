//! Photos, shown inline in chat bubbles, and chat photos in the chat list.
//!
//! Every frame, the message view and the chat list ask for the photos they
//! have on screen. After the frame, [`Images::fetch`] starts whatever is
//! missing: a TDLib download, then decode + encode for the terminal on a
//! blocking thread. Until the real photo is ready, the blurry thumbnail that
//! came with the message (or chat) stands in.

use std::cell::Cell;
use std::collections::{HashMap, HashSet};
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::sync::{Arc, Mutex, PoisonError};

use anyhow::Result;
use image::imageops::FilterType;
use image::{DynamicImage, RgbaImage};
use ratatui::layout::Size;
use ratatui_image::picker::cap_parser::Parser;
use ratatui_image::picker::{Capability, Picker, ProtocolType};
use ratatui_image::protocol::kitty::Kitty;
use ratatui_image::sliced::SlicedProtocol;
use ratatui_image::{FontSize, Resize};
use tokio::sync::mpsc::UnboundedSender;

use crate::chats::ChatPhoto;
use crate::messages::Preview;
use crate::term::{self, Terminal};
use crate::tg::Tg;

/// Scale both ways (`Fit` never enlarges, which would leave the tiny thumbnail
/// tiny). Triangle filtering keeps the enlarged thumbnail soft, not blocky.
const RESIZE: Resize = Resize::Scale(Some(FilterType::Triangle));

/// Chat photos kept encoded; the ones drawn longest ago go first. Enough for
/// several screens of the list.
const MAX_AVATARS: usize = 300;

/// One encoded image: a photo (or its thumbnail) at one size in cells.
#[derive(Clone, Copy, PartialEq, Eq, Hash)]
pub struct Key {
    pub file_id: i32,
    pub cols: u16,
    pub rows: u16,
    /// The blurry thumbnail embedded in the message, not the downloaded photo.
    pub thumbnail: bool,
    /// A chat photo for the chat list, cut to a circle.
    pub avatar: bool,
}

/// Caps for decoding images from other people. Telegram's photos are at most
/// 2560px a side and stickers 512px, so a bigger one is only a way to run
/// memory out.
fn limits() -> image::Limits {
    sized(4096)
}

/// Caps of `side` pixels a side. Decoders allocate more than `max_alloc`
/// counts (WebP's scratch and frame buffers), so the side is what bounds it.
fn sized(side: u32) -> image::Limits {
    let mut limits = image::Limits::default();
    limits.max_image_width = Some(side);
    limits.max_image_height = Some(side);
    limits.max_alloc = Some(128 * 1024 * 1024);
    limits
}

/// Stickers are 512px a side; this leaves room to spare.
const STICKER_SIDE: u32 = 1024;

/// Like `image::open`, within [`limits`]. The format comes from the file's
/// first bytes, else its extension: a file sent as a file is named by its
/// sender, so a JPEG can be called `.png`.
pub fn open_image(path: &str) -> image::ImageResult<image::DynamicImage> {
    open_within(path, limits())
}

fn open_within(path: &str, limits: image::Limits) -> image::ImageResult<image::DynamicImage> {
    let mut reader = image::ImageReader::open(path)?.with_guessed_format()?;
    reader.limits(limits);
    reader.decode()
}

/// At most this many images decode at once; the rest wait for a later frame.
/// Each may take a few hundred MB while it decodes.
const MAX_BUILDING: usize = 4;

/// Encoded photos kept for the open chat; the least recently drawn go first.
const MAX_READY: usize = 200;

/// More cells than any photo in a bubble (40 × 16): only the photo viewer
/// asks for photos this big.
const LARGE_CELLS: u32 = 40 * 16;

/// Of those, at most this many are kept. Each can take tens of MB (a kitty
/// encode keeps the code it sent, the whole image), and the viewer shows one
/// at a time.
const MAX_LARGE: usize = 4;

fn large(key: &Key) -> bool {
    u32::from(key.cols) * u32::from(key.rows) > LARGE_CELLS
}

/// What the photos kept may take in a terminal speaking kitty's protocol, in
/// bytes of pixels. Such a terminal keeps every image it's sent until told
/// to delete it, and past its quota (320 MB in kitty and Ghostty) it drops
/// the ones sent first: the chat photos, sent once at startup and never
/// again, which then leave blanks in the list. Half the quota leaves room
/// for those (at most [`MAX_AVATARS`], tens of KB each).
const KITTY_BUDGET: usize = 160_000_000;

/// Numbers of kitty images dropped since the terminal was last told; see
/// [`Images::send_deletions`].
type Dropped = Arc<Mutex<Vec<u32>>>;

/// An image sent with kitty's protocol: the number the terminal knows it by
/// and the bytes it takes there. Dropping it, wherever that happens, adds
/// its number to those the terminal is told to delete.
struct KittyImage {
    id: u32,
    bytes: usize,
    dropped: Dropped,
}

impl Drop for KittyImage {
    fn drop(&mut self) {
        self.dropped
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .push(self.id);
    }
}

/// An image encoded for the terminal.
struct Encoded {
    image: SlicedProtocol,
    kitty: Option<KittyImage>,
}

impl Encoded {
    fn kitty_bytes(&self) -> usize {
        self.kitty.as_ref().map_or(0, |k| k.bytes)
    }
}

/// `image` encoded to fill `size` cells. With kitty's protocol it's sent as
/// the image numbered by `kitty`, picked here rather than by the picker, so
/// it can be deleted from the terminal once it's dropped.
fn encode(
    picker: &Picker,
    image: DynamicImage,
    size: Size,
    kitty: Option<(u32, Dropped)>,
) -> Result<Encoded> {
    let Some((id, dropped)) = kitty else {
        let image = SlicedProtocol::new_with_resize(picker, image, size, RESIZE)?;
        return Ok(Encoded { image, kitty: None });
    };
    // As the picker's `new_protocol` does with a `Resize::Scale`, which
    // always resizes.
    let font = picker.font_size();
    let size = RESIZE.size_for(&image, font, size);
    let image = RESIZE.resize(&image, font, size, None);
    let bytes = image.width() as usize * image.height() as usize * 4;
    let compress = picker
        .capabilities()
        .contains(&Capability::KittyCompression);
    let image = Kitty::new(image, size, id, picker.tmux_detected(), compress)?;
    Ok(Encoded {
        image: SlicedProtocol::Kitty(image),
        kitty: Some(KittyImage { id, bytes, dropped }),
    })
}

/// Where numbers for kitty's images start: at random, as the picker's own
/// are, since every program drawing in the terminal's window shares them
/// (two tuigrams in panes of one tmux), and one's deletions mustn't take
/// the other's pictures.
fn kitty_id_base() -> u32 {
    use std::hash::{BuildHasher, RandomState};
    RandomState::new().hash_one(std::process::id()) as u32
}

/// The code that has the terminal delete image number `id` of kitty's
/// protocol. `I` deletes the image itself, not only where it's placed;
/// `q=2` asks for no answer, which would come in as keys.
fn kitty_delete(id: u32, tmux: bool) -> String {
    let (start, escape, end) = Parser::tmux_start_escape_end(tmux);
    format!("{start}{escape}_Ga=d,d=I,i={id},q=2{escape}\\{end}")
}

thread_local! {
    /// Set while this thread decodes another user's image.
    static DECODING: Cell<bool> = const { Cell::new(false) };
}

/// The panic is in an image decoder, where it's caught: the image shows as
/// broken and the app carries on, rather than every chat holding that image
/// becoming one that can't be opened.
pub fn panic_is_contained() -> bool {
    DECODING.with(Cell::get)
}

/// Runs `decode`, with a panic in it caught and turned into an error. Every
/// decode of another user's image goes through this.
pub fn contained<T>(decode: impl FnOnce() -> Result<T>) -> Result<T> {
    DECODING.with(|d| d.set(true));
    let result = catch_unwind(AssertUnwindSafe(decode))
        .unwrap_or_else(|_| Err(anyhow::anyhow!("the image couldn't be decoded")));
    DECODING.with(|d| d.set(false));
    result
}

/// An image still building after this long counts as failed.
const BUILD_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(30);

/// Like `image::load_from_memory`, within [`limits`].
fn decode_bytes(data: &[u8]) -> image::ImageResult<image::DynamicImage> {
    let mut reader = image::ImageReader::new(std::io::Cursor::new(data)).with_guessed_format()?;
    reader.limits(limits());
    reader.decode()
}

/// `photo` cut to a circle in the middle of a `width`×`height` canvas, which
/// is transparent around it, so the row's background shows there. The edge
/// fades out over a pixel to look round rather than jagged.
pub fn circle(photo: &DynamicImage, width: u32, height: u32) -> RgbaImage {
    let mut canvas = RgbaImage::new(width, height);
    let size = width.min(height);
    let side = photo.width().min(photo.height());
    if size == 0 || side == 0 {
        return canvas;
    }
    let square = photo
        .crop_imm(
            (photo.width() - side) / 2,
            (photo.height() - side) / 2,
            side,
            side,
        )
        .resize_exact(size, size, FilterType::Triangle)
        .to_rgba8();
    let (left, top) = ((width - size) / 2, (height - size) / 2);
    let radius = size as f32 / 2.0;
    for (x, y, pixel) in square.enumerate_pixels() {
        let dx = x as f32 + 0.5 - radius;
        let dy = y as f32 + 0.5 - radius;
        let inside = (radius - (dx * dx + dy * dy).sqrt() + 0.5).clamp(0.0, 1.0);
        let mut pixel = *pixel;
        pixel[3] = (f32::from(pixel[3]) * inside).round() as u8;
        canvas.put_pixel(left + x, top + y, pixel);
    }
    canvas
}

/// An image finished encoding (or failed) on a background thread.
pub struct ImageEvent {
    key: Key,
    /// [`Images::generation`] when it was started.
    generation: u64,
    /// The encoded image, and the size in pixels it was decoded at.
    result: Result<(Encoded, (u32, u32))>,
}

enum FileState {
    Downloading,
    Ready(String),
    Failed,
}

/// An encoded chat photo.
struct Avatar {
    image: Encoded,
    /// When it was last drawn, in calls to [`Images::avatar`].
    used: u64,
    /// It has been drawn. Kitty's protocol sends an image to the terminal
    /// the first time it's drawn, and only then, so that time must not be
    /// under a popup that hides it.
    shown: bool,
}

/// How much a cell's measured width or height must change, as a share of
/// what it was, to count as a new cell size rather than rounding: a window
/// whose pixels include its padding measures a little more per cell the
/// fewer rows it has. A screen of another density changes it by a quarter
/// or more, a step of the font's size by about a fourteenth.
const CELL_CHANGE: u32 = 16;

/// The pixel size of a cell as the terminal's window size says: its size
/// in pixels over its columns and rows. None where it doesn't say, as on
/// Windows, so cells keep the size the terminal answered at startup.
pub fn measure_cells() -> Option<FontSize> {
    let size = crossterm::terminal::window_size().ok()?;
    if size.columns == 0 || size.rows == 0 || size.width == 0 || size.height == 0 {
        return None;
    }
    Some(FontSize::new(
        size.width / size.columns,
        size.height / size.rows,
    ))
}

/// The cells' size in pixels, for a terminal that answered `queried` at
/// startup, when its window measured `then`, and measures `now` (see
/// [`measure_cells`]): as answered, unless they measure noticeably bigger
/// or smaller, then grown or shrunk as much. The answer is kept as the
/// truth, since a window's measure may count its padding.
fn cells_now(queried: FontSize, then: FontSize, now: FontSize) -> FontSize {
    let changed =
        |then: u16, now: u16| u32::from(then.abs_diff(now)) * CELL_CHANGE > u32::from(then);
    if !changed(then.width, now.width) && !changed(then.height, now.height) {
        return queried;
    }
    let scale = |queried: u16, then: u16, now: u16| {
        let (queried, then, now) = (u32::from(queried), u32::from(then.max(1)), u32::from(now));
        ((queried * now + then / 2) / then).clamp(1, u32::from(u16::MAX)) as u16
    };
    FontSize::new(
        scale(queried.width, then.width, now.width),
        scale(queried.height, then.height, now.height),
    )
}

pub struct Images {
    picker: Picker,
    /// The cell size the terminal answered at startup.
    queried: FontSize,
    /// What its window measured its cells as then: see [`Images::measured`].
    measured: Option<FontSize>,
    tx: UnboundedSender<ImageEvent>,
    /// Encoded photos, with the frame each was last drawn in.
    ready: HashMap<Key, (Encoded, u64)>,
    /// Counts frames, for `ready`.
    frame: u64,
    /// Goes up when the files are forgotten, for a new TDLib client: an
    /// image still being built for the old one numbers its file the old way,
    /// and is dropped when it's done. Also when cells change size, which
    /// leaves one being built the wrong size.
    generation: u64,
    /// Kept apart from `ready`: they stay when another chat is opened.
    avatars: HashMap<Key, Avatar>,
    /// Counts calls to [`Images::avatar`], to find the least recently drawn.
    avatar_clock: u64,
    /// Images being built, by the [`Images::generation`] each was started
    /// in, and when. One of an older generation keeps its place until it's
    /// done: its thread still decodes, taking as much memory.
    building: HashMap<(u64, Key), std::time::Instant>,
    /// Keys that failed to decode; never retried.
    failed: HashSet<Key>,
    files: HashMap<i32, FileState>,
    /// Photos' own size in pixels, learned by decoding them: an image sent
    /// as a file comes without it.
    pixels: HashMap<i32, (u32, u32)>,
    /// Photos the last frame showed, with their size.
    wanted: Vec<(Preview, u16, u16)>,
    /// Chat photos the last frame showed, with their size.
    wanted_avatars: Vec<(ChatPhoto, u16, u16)>,
    /// The number the last image built for kitty's protocol was given.
    kitty_id: u32,
    /// Images dropped since the terminal was last told.
    dropped: Dropped,
    /// See [`KITTY_BUDGET`]; smaller in tests.
    kitty_budget: usize,
}

impl Images {
    pub fn new(picker: Picker, tx: UnboundedSender<ImageEvent>) -> Self {
        Self {
            queried: picker.font_size(),
            measured: None,
            picker,
            tx,
            ready: HashMap::new(),
            frame: 0,
            generation: 0,
            avatars: HashMap::new(),
            avatar_clock: 0,
            building: HashMap::new(),
            failed: HashSet::new(),
            files: HashMap::new(),
            pixels: HashMap::new(),
            wanted: Vec::new(),
            wanted_avatars: Vec::new(),
            kitty_id: kitty_id_base(),
            dropped: Dropped::default(),
            kitty_budget: KITTY_BUDGET,
        }
    }

    /// False when the terminal shows no real images, only colored blocks
    /// (halfblocks), which are too coarse for a chat photo in a few cells.
    pub fn draws_photos(&self) -> bool {
        self.picker.protocol_type() != ProtocolType::Halfblocks
    }

    /// The terminal paints each image (or each of its rows) from one cell,
    /// and repaints the whole of it whenever that cell is sent again, over
    /// anything drawn on top since: sixel and iTerm2's protocol.
    pub fn paints_over(&self) -> bool {
        matches!(
            self.picker.protocol_type(),
            ProtocolType::Sixel | ProtocolType::Iterm2
        )
    }

    /// Pixel size of one terminal cell, for sizing photos by aspect ratio.
    pub fn font_size(&self) -> FontSize {
        self.picker.font_size()
    }

    /// The terminal's window measures its cells `now` (see
    /// [`measure_cells`]): first right after the terminal answered at
    /// startup, then whenever its size may have changed. Cells change size
    /// in pixels when the window moves to a screen of another density, or
    /// the font is made bigger or smaller. True if they did: photos and
    /// chat photos are then encoded again at the new size, since sixel and
    /// iTerm2's protocol draw them as many pixels as they were made, too
    /// big or too small for the cells, and kitty's would be blurred or
    /// wasted. The screen must then be drawn afresh.
    pub fn measured(&mut self, now: Option<FontSize>) -> bool {
        let Some(now) = now else {
            return false;
        };
        let Some(then) = self.measured else {
            self.measured = Some(now);
            return false;
        };
        let font = cells_now(self.queried, then, now);
        let old = self.picker.font_size();
        if (font.width, font.height) == (old.width, old.height) {
            return false;
        }
        // A picker takes its cell size only when it's made, from the
        // terminal's answer, which can't be asked for again while keys are
        // read. This one keeps the protocol found then, but not kitty's
        // compression, so kitty is sent its images uncompressed.
        let protocol = self.picker.protocol_type();
        #[allow(deprecated)]
        let mut picker = Picker::from_fontsize(font);
        picker.set_protocol_type(protocol);
        self.picker = picker;
        self.generation += 1;
        self.drop_all();
        true
    }

    /// The best image ready to draw: the photo, else its blurry thumbnail.
    pub fn get(&mut self, photo: &Preview, cols: u16, rows: u16) -> Option<&SlicedProtocol> {
        let key = |thumbnail| Key {
            file_id: photo.file_id,
            cols,
            rows,
            thumbnail,
            avatar: false,
        };
        let key = [key(false), key(true)]
            .into_iter()
            .find(|k| self.ready.contains_key(k))?;
        let frame = self.frame;
        let (encoded, used) = self.ready.get_mut(&key)?;
        *used = frame;
        Some(&encoded.image)
    }

    /// Like [`get`](Self::get), but only the photo itself, not its blurry
    /// thumbnail.
    pub fn sharp(&mut self, photo: &Preview, cols: u16, rows: u16) -> Option<&SlicedProtocol> {
        let key = Key {
            file_id: photo.file_id,
            cols,
            rows,
            thumbnail: false,
            avatar: false,
        };
        let frame = self.frame;
        let (encoded, used) = self.ready.get_mut(&key)?;
        *used = frame;
        Some(&encoded.image)
    }

    /// The photo's file is downloaded.
    pub fn downloaded(&self, photo: &Preview) -> bool {
        matches!(self.files.get(&photo.file_id), Some(FileState::Ready(_)))
    }

    /// True when the photo can't be shown: its download or decode failed.
    pub fn is_broken(&self, photo: &Preview) -> bool {
        matches!(self.files.get(&photo.file_id), Some(FileState::Failed))
    }

    /// Called while drawing, for each photo on screen.
    pub fn want(&mut self, photo: &Preview, cols: u16, rows: u16) {
        self.wanted.push((photo.clone(), cols, rows));
    }

    /// The best chat photo ready to draw, as for [`get`](Self::get). While
    /// `covered` (a popup may be over it), only one that has been drawn before
    /// is returned; see [`Avatar::shown`].
    pub fn avatar(
        &mut self,
        photo: &ChatPhoto,
        cols: u16,
        rows: u16,
        covered: bool,
    ) -> Option<&SlicedProtocol> {
        let key = |thumbnail| Key {
            file_id: photo.file_id,
            cols,
            rows,
            thumbnail,
            avatar: true,
        };
        // See paints_over: a popup may be over the list.
        if covered && self.paints_over() {
            return None;
        }
        let key = [key(false), key(true)]
            .into_iter()
            .find(|k| self.avatars.get(k).is_some_and(|a| a.shown || !covered))?;
        self.avatar_clock += 1;
        let avatar = self.avatars.get_mut(&key)?;
        avatar.used = self.avatar_clock;
        avatar.shown = true;
        Some(&avatar.image.image)
    }

    /// Called while drawing, for each chat photo on screen.
    pub fn want_avatar(&mut self, photo: &ChatPhoto, cols: u16, rows: u16) {
        self.wanted_avatars.push((photo.clone(), cols, rows));
    }

    /// Starts downloads and encodes for what the last frame wanted.
    pub fn fetch(&mut self, tg: &Tg) {
        self.frame += 1;
        self.fetch_avatars(tg);
        for (photo, cols, rows) in std::mem::take(&mut self.wanted) {
            let full = Key {
                file_id: photo.file_id,
                cols,
                rows,
                thumbnail: false,
                avatar: false,
            };
            if self.ready.contains_key(&full) {
                continue;
            }
            match self.files.get(&photo.file_id) {
                Some(FileState::Ready(path)) => {
                    let path = path.clone();
                    let limits = if photo.sticker {
                        sized(STICKER_SIDE)
                    } else {
                        limits()
                    };
                    self.build(full, move || Ok(open_within(&path, limits)?));
                }
                Some(FileState::Downloading | FileState::Failed) => {}
                None => {
                    self.files.insert(photo.file_id, FileState::Downloading);
                    tg.download(photo.file_id);
                }
            }
            if let Some(data) = photo.thumbnail {
                let key = Key {
                    thumbnail: true,
                    ..full
                };
                self.build(key, move || Ok(decode_bytes(&data)?));
            }
        }
    }

    fn fetch_avatars(&mut self, tg: &Tg) {
        let font = self.picker.font_size();
        for (photo, cols, rows) in std::mem::take(&mut self.wanted_avatars) {
            let full = Key {
                file_id: photo.file_id,
                cols,
                rows,
                thumbnail: false,
                avatar: true,
            };
            if self.avatars.contains_key(&full) {
                continue;
            }
            let (width, height) = (
                u32::from(cols) * u32::from(font.width),
                u32::from(rows) * u32::from(font.height),
            );
            let file = self
                .files
                .entry(photo.file_id)
                .or_insert_with(|| match &photo.path {
                    Some(path) => FileState::Ready(path.clone()),
                    None => {
                        tg.download_quiet(photo.file_id);
                        FileState::Downloading
                    }
                });
            if let FileState::Ready(path) = file {
                let path = path.clone();
                self.build(full, move || {
                    Ok(circle(&open_image(&path)?, width, height).into())
                });
            }
            if let Some(data) = photo.thumbnail {
                let key = Key {
                    thumbnail: true,
                    ..full
                };
                self.build(key, move || {
                    Ok(circle(&decode_bytes(&data)?, width, height).into())
                });
            }
        }
    }

    pub fn on_downloaded(&mut self, file_id: i32, path: Option<String>) {
        let state = path.map_or(FileState::Failed, FileState::Ready);
        self.files.insert(file_id, state);
    }

    /// The photo numbered `file_id` is a file on this computer, such as one
    /// about to be sent: it's read from `path`, never downloaded. Called
    /// while drawing, before [`want`](Self::want); a photo that failed to
    /// decode stays broken.
    pub fn local_file(&mut self, file_id: i32, path: &str) {
        self.files
            .entry(file_id)
            .or_insert_with(|| FileState::Ready(path.into()));
    }

    pub fn on_built(&mut self, event: ImageEvent) {
        self.building.remove(&(event.generation, event.key));
        if event.generation != self.generation {
            return;
        }
        match event.result {
            Ok((image, _)) if event.key.avatar => self.add_avatar(event.key, image),
            Ok((image, pixels)) => {
                if !event.key.thumbnail {
                    self.pixels.insert(event.key.file_id, pixels);
                }
                self.add_ready(event.key, image);
            }
            Err(_) => {
                self.failed.insert(event.key);
                if !event.key.thumbnail {
                    self.files.insert(event.key.file_id, FileState::Failed);
                }
            }
        }
    }

    /// `photo` as many pixels as it was decoded at, once it has been: an
    /// image sent as a file comes only with its thumbnail's shape.
    pub fn measured_photo(&self, photo: &Preview) -> Preview {
        let mut photo = photo.clone();
        if let Some(&(width, height)) = self.pixels.get(&photo.file_id) {
            (photo.width, photo.height) = (width.max(1), height.max(1));
        }
        photo
    }

    fn add_ready(&mut self, key: Key, image: Encoded) {
        // One it replaces counts no more.
        self.ready.remove(&key);
        while large(&key)
            && self.ready.keys().filter(|k| large(k)).count() >= MAX_LARGE
            && let Some(oldest) = self.least_used(|k, _| large(k))
        {
            self.ready.remove(&oldest);
        }
        if self.ready.len() >= MAX_READY
            && let Some(oldest) = self.least_used(|_, _| true)
        {
            self.ready.remove(&oldest);
        }
        // Those the last frame drew stay, or the next would build them again
        // and drop others it shows, frame after frame. Window-sized ones go
        // first: they free the most, and the viewer shows one at a time, so
        // zooming in it doesn't take the bubbles' photos behind it.
        let shown = self.frame.saturating_sub(1);
        while self.kitty_bytes() + image.kitty_bytes() > self.kitty_budget
            && let Some(oldest) = self
                .least_used(|k, used| large(k) && used < shown)
                .or_else(|| self.least_used(|_, used| used < shown))
        {
            self.ready.remove(&oldest);
        }
        self.ready.insert(key, (image, self.frame));
    }

    /// The photo drawn longest ago of those `which` takes, by its key and
    /// the frame it was last drawn in.
    fn least_used(&self, which: impl Fn(&Key, u64) -> bool) -> Option<Key> {
        self.ready
            .iter()
            .filter(|&(key, &(_, used))| which(key, used))
            .min_by_key(|(_, (_, used))| *used)
            .map(|(&key, _)| key)
    }

    /// What the photos kept take in a terminal speaking kitty's protocol.
    fn kitty_bytes(&self) -> usize {
        self.ready.values().map(|(e, _)| e.kitty_bytes()).sum()
    }

    fn add_avatar(&mut self, key: Key, image: Encoded) {
        if self.avatars.len() >= MAX_AVATARS
            && let Some(oldest) = self
                .avatars
                .iter()
                .min_by_key(|(_, a)| a.used)
                .map(|(&k, _)| k)
        {
            self.avatars.remove(&oldest);
        }
        let avatar = Avatar {
            image,
            used: self.avatar_clock,
            shown: false,
        };
        self.avatars.insert(key, avatar);
    }

    /// Drops encoded photos, e.g. when switching chats. Chat photos and
    /// downloads stay.
    pub fn clear(&mut self) {
        self.ready.clear();
    }

    /// Drops every encoded image, chat photos too, as tuigram quits: a
    /// terminal speaking kitty's protocol would keep them after it's gone.
    pub fn drop_all(&mut self) {
        self.ready.clear();
        self.avatars.clear();
    }

    /// Has a terminal speaking kitty's protocol delete the images dropped
    /// since it was last told, freeing what they took there; see
    /// [`KITTY_BUDGET`]. Called before each frame, ahead of any image it
    /// sends.
    pub fn send_deletions(&mut self, terminal: &mut Terminal) {
        if let Some(code) = self.deletions() {
            let _ = term::send(terminal, &code);
        }
    }

    /// The code [`send_deletions`](Self::send_deletions) sends.
    fn deletions(&mut self) -> Option<String> {
        let ids = std::mem::take(&mut *self.dropped.lock().unwrap_or_else(PoisonError::into_inner));
        if ids.is_empty() {
            return None;
        }
        let tmux = self.picker.tmux_detected();
        Some(ids.into_iter().map(|id| kitty_delete(id, tmux)).collect())
    }

    /// Forgets every file, for a new TDLib client: it numbers files afresh.
    pub fn forget_files(&mut self) {
        self.generation += 1;
        self.drop_all();
        self.failed.clear();
        self.files.clear();
        self.pixels.clear();
        self.wanted.clear();
        self.wanted_avatars.clear();
    }

    /// Decodes and encodes on a blocking thread, once per key.
    fn build(
        &mut self,
        key: Key,
        decode: impl FnOnce() -> Result<image::DynamicImage> + Send + 'static,
    ) {
        let built = if key.avatar {
            self.avatars.contains_key(&key)
        } else {
            self.ready.contains_key(&key)
        };
        // One that never finishes mustn't hold its place for good: it counts
        // as failed, and the others get their turn.
        let (failed, generation) = (&mut self.failed, self.generation);
        self.building.retain(|&(started_in, key), started| {
            let alive = started.elapsed() < BUILD_TIMEOUT;
            if !alive && started_in == generation {
                failed.insert(key);
            }
            alive
        });
        // Those still decoding for an older generation count too.
        if built
            || self.building.contains_key(&(generation, key))
            || self.failed.contains(&key)
            || self.building.len() >= MAX_BUILDING
        {
            return;
        }
        self.building
            .insert((generation, key), std::time::Instant::now());
        let picker = self.picker.clone();
        let tx = self.tx.clone();
        let kitty = self.next_kitty();
        tokio::task::spawn_blocking(move || {
            let result = contained(|| {
                decode().and_then(|image| {
                    let pixels = (image.width(), image.height());
                    let size = Size::new(key.cols, key.rows);
                    let image = encode(&picker, image, size, kitty)?;
                    Ok((image, pixels))
                })
            });
            let _ = tx.send(ImageEvent {
                key,
                generation,
                result,
            });
        });
    }

    /// A number for the next image built with kitty's protocol, and where
    /// it goes once that image is dropped.
    fn next_kitty(&mut self) -> Option<(u32, Dropped)> {
        (self.picker.protocol_type() == ProtocolType::Kitty).then(|| {
            // Never 0, which kitty's protocol takes as no number.
            self.kitty_id = self.kitty_id.wrapping_add(1).max(1);
            (self.kitty_id, self.dropped.clone())
        })
    }

    #[cfg(test)]
    pub fn insert_ready(&mut self, key: Key, image: image::DynamicImage) {
        let size = Size::new(key.cols, key.rows);
        let kitty = self.next_kitty();
        let image = encode(&self.picker, image, size, kitty).unwrap();
        if key.avatar {
            self.add_avatar(key, image);
        } else {
            self.add_ready(key, image);
        }
    }
}

/// Shared thumbnail bytes, so cloning a [`Preview`] each frame is cheap.
pub type Thumbnail = Arc<[u8]>;

#[cfg(test)]
mod tests {
    use super::*;

    fn png(width: u32, height: u32) -> Vec<u8> {
        let mut data = std::io::Cursor::new(Vec::new());
        image::RgbImage::new(width, height)
            .write_to(&mut data, image::ImageFormat::Png)
            .unwrap();
        data.into_inner()
    }

    #[test]
    fn chat_photos_are_cut_to_a_circle_with_see_through_corners() {
        let photo = image::RgbaImage::from_pixel(160, 100, image::Rgba([255, 0, 0, 255])).into();
        let round = circle(&photo, 40, 44);
        let alpha = |x, y| round.get_pixel(x, y)[3];
        assert_eq!(alpha(0, 2), 0, "corner");
        assert_eq!(alpha(39, 41), 0, "corner");
        assert_eq!(alpha(20, 22), 255, "middle");
        assert_eq!(alpha(20, 0), 0, "above the circle: the canvas is taller");
        assert_eq!(alpha(1, 22), 255, "the circle reaches the sides");
        let edge = alpha(6, 7);
        assert!(edge > 0 && edge < 255, "a soft edge: {edge}");
        assert_eq!(circle(&photo, 0, 44).dimensions(), (0, 44), "no panic");
    }

    #[test]
    fn only_a_few_window_sized_photos_are_kept() {
        let tx = tokio::sync::mpsc::unbounded_channel().0;
        let mut images = Images::new(Picker::halfblocks(), tx);
        let at = |file_id, cols| Key {
            file_id,
            cols,
            rows: 50,
            thumbnail: false,
            avatar: false,
        };
        for file_id in 0..10 {
            images.insert_ready(at(file_id, 4), image::RgbImage::new(4, 4).into());
            images.insert_ready(at(file_id, 200), image::RgbImage::new(4, 4).into());
        }
        let large = images.ready.keys().filter(|k| k.cols == 200).count();
        assert_eq!(large, MAX_LARGE);
        let small = images.ready.keys().filter(|k| k.cols == 4).count();
        assert_eq!(small, 10, "bubbles' photos aren't touched");
    }

    fn key(file_id: i32) -> Key {
        Key {
            file_id,
            cols: 4,
            rows: 2,
            thumbnail: false,
            avatar: false,
        }
    }

    fn preview(file_id: i32) -> Preview {
        Preview {
            file_id,
            width: 8,
            height: 8,
            thumbnail: None,
            sticker: false,
        }
    }

    #[tokio::test]
    async fn a_decoder_panic_marks_the_photo_broken_instead_of_ending_the_app() {
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
        let mut images = Images::new(Picker::halfblocks(), tx);
        images.build(key(7), || panic!("a decoder bug"));
        images.on_built(rx.recv().await.unwrap());
        assert!(images.is_broken(&preview(7)));
        assert!(!panic_is_contained(), "only while decoding");
    }

    #[tokio::test]
    async fn an_image_built_for_a_logged_out_session_is_dropped() {
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
        let mut images = Images::new(Picker::halfblocks(), tx);
        let (go, wait) = std::sync::mpsc::channel::<()>();
        images.build(key(7), move || {
            wait.recv().ok();
            Ok(DynamicImage::new_rgba8(8, 8))
        });
        images.forget_files();
        go.send(()).unwrap();
        images.on_built(rx.recv().await.unwrap());
        assert!(
            images.get(&preview(7), 4, 2).is_none(),
            "not the new session's"
        );
        assert!(!images.is_broken(&preview(7)));
    }

    fn cells_after(queried: (u16, u16), then: (u16, u16), now: (u16, u16)) -> (u16, u16) {
        let font = |(w, h)| FontSize::new(w, h);
        let size = cells_now(font(queried), font(then), font(now));
        (size.width, size.height)
    }

    #[test]
    fn cells_keep_the_size_the_terminal_answered_unless_they_measure_another() {
        assert_eq!(cells_after((20, 40), (20, 40), (20, 40)), (20, 40));
        // A window that counts its padding measures a pixel more per cell
        // with few rows: not a new size.
        assert_eq!(cells_after((20, 40), (20, 40), (20, 41)), (20, 40));
        // Moved from a Retina screen to one of half its density.
        assert_eq!(cells_after((20, 40), (20, 40), (10, 20)), (10, 20));
        assert_eq!(cells_after((10, 20), (10, 20), (20, 40)), (20, 40));
        // Measured with its padding, the answer is scaled, not replaced.
        assert_eq!(cells_after((20, 40), (21, 42), (10, 21)), (10, 20));
        // A step of the font's size.
        assert_eq!(cells_after((14, 28), (14, 28), (15, 30)), (15, 30));
    }

    #[tokio::test]
    async fn photos_are_made_again_once_cells_change_size() {
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
        let mut images = Images::new(Picker::halfblocks(), tx);
        let start = images.font_size();
        let measure = |w, h| Some(FontSize::new(w, h));
        assert!(
            !images.measured(measure(10, 20)),
            "the first is the startup's"
        );
        assert!(!images.measured(None), "a terminal that doesn't say");
        assert!(!images.measured(measure(10, 20)));
        images.insert_ready(key(1), DynamicImage::new_rgba8(8, 8));

        // One on its way is the old size.
        let (go, wait) = std::sync::mpsc::channel::<()>();
        images.build(key(7), move || {
            wait.recv().ok();
            Ok(DynamicImage::new_rgba8(8, 8))
        });
        assert!(images.measured(measure(20, 40)));
        let font = images.font_size();
        assert_eq!(
            (font.width, font.height),
            (start.width * 2, start.height * 2)
        );
        assert!(!images.draws_photos(), "the protocol stays");
        assert!(images.get(&preview(1), 4, 2).is_none(), "made again");
        go.send(()).unwrap();
        images.on_built(rx.recv().await.unwrap());
        assert!(images.get(&preview(7), 4, 2).is_none(), "dropped");
        assert!(!images.is_broken(&preview(7)));
        assert!(!images.measured(measure(20, 40)), "only once");
        images.build(key(7), || Ok(DynamicImage::new_rgba8(8, 8)));
        images.on_built(rx.recv().await.unwrap());
        assert!(images.get(&preview(7), 4, 2).is_some(), "the new size");
    }

    #[tokio::test]
    async fn only_a_few_images_decode_at_once() {
        let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
        let mut images = Images::new(Picker::halfblocks(), tx);
        let (go, wait) = std::sync::mpsc::channel::<()>();
        let wait = std::sync::Arc::new(std::sync::Mutex::new(wait));
        for id in 0..10 {
            let wait = wait.clone();
            images.build(key(id), move || {
                let _ = wait.lock().unwrap().recv();
                Ok(DynamicImage::new_rgba8(8, 8))
            });
        }
        assert_eq!(images.building.len(), MAX_BUILDING);
        drop(go);
    }

    #[tokio::test]
    async fn decodes_left_over_from_another_cell_size_still_take_their_place() {
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
        let mut images = Images::new(Picker::halfblocks(), tx);
        images.measured(Some(FontSize::new(10, 20)));
        let (go, wait) = std::sync::mpsc::channel::<()>();
        let wait = std::sync::Arc::new(std::sync::Mutex::new(wait));
        for id in 0..MAX_BUILDING as i32 {
            let wait = wait.clone();
            images.build(key(id), move || {
                let _ = wait.lock().unwrap().recv();
                Ok(DynamicImage::new_rgba8(8, 8))
            });
        }
        // Zooming the font a few times shouldn't start four more each time.
        assert!(images.measured(Some(FontSize::new(20, 40))));
        images.build(key(99), || Ok(DynamicImage::new_rgba8(8, 8)));
        assert_eq!(images.building.len(), MAX_BUILDING, "no room yet");
        drop(go);
        images.on_built(rx.recv().await.unwrap());
        images.build(key(99), || Ok(DynamicImage::new_rgba8(8, 8)));
        assert!(
            images.building.contains_key(&(images.generation, key(99))),
            "its turn came once an old one was done"
        );
    }

    #[tokio::test]
    async fn a_decode_that_never_finishes_gives_up_its_place() {
        let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
        let mut images = Images::new(Picker::halfblocks(), tx);
        for id in 0..MAX_BUILDING as i32 {
            let long_ago = std::time::Instant::now() - BUILD_TIMEOUT;
            images.building.insert((0, key(id)), long_ago);
        }
        images.build(key(99), || Ok(DynamicImage::new_rgba8(8, 8)));
        assert!(images.building.contains_key(&(0, key(99))), "its turn came");
        assert!(
            images.failed.contains(&key(0)),
            "the stuck one counts as failed"
        );
    }

    #[test]
    fn the_photo_cache_keeps_the_ones_drawn_lately() {
        let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
        let mut images = Images::new(Picker::halfblocks(), tx);
        images.insert_ready(key(0), DynamicImage::new_rgba8(8, 8));
        for id in 1..MAX_READY as i32 + 10 {
            images.frame += 1;
            assert!(images.get(&preview(0), 4, 2).is_some(), "still drawn");
            images.insert_ready(key(id), DynamicImage::new_rgba8(8, 8));
        }
        assert_eq!(images.ready.len(), MAX_READY);
        assert!(images.get(&preview(0), 4, 2).is_some());
        assert!(images.get(&preview(1), 4, 2).is_none(), "the oldest went");
    }

    #[tokio::test]
    async fn a_photo_about_to_be_sent_is_read_from_its_file_not_downloaded() {
        let dir = std::env::temp_dir().join(format!("tuigram-local-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("cat.png");
        std::fs::write(&path, png(80, 60)).unwrap();
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
        let mut images = Images::new(Picker::halfblocks(), tx);
        let photo = preview(-1);
        images.local_file(-1, path.to_str().unwrap());
        images.want(&photo, 4, 2);
        // A detached client makes no requests, so a download would never end.
        images.fetch(&Tg::detached(tokio::sync::mpsc::unbounded_channel().0));
        images.on_built(rx.recv().await.unwrap());
        assert!(images.get(&photo, 4, 2).is_some());
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[tokio::test]
    async fn a_file_named_as_another_format_decodes_and_says_how_big_it_is() {
        let dir = std::env::temp_dir().join(format!("tuigram-misnamed-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        // A PNG its sender named .jpg.
        let path = dir.join("photo.jpg");
        std::fs::write(&path, png(80, 60)).unwrap();
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
        let mut images = Images::new(Picker::halfblocks(), tx);
        let photo = preview(3);
        let size = |p: Preview| (p.width, p.height);
        assert_eq!(
            size(images.measured_photo(&photo)),
            (8, 8),
            "as said, until decoded"
        );
        images.on_downloaded(3, Some(path.to_str().unwrap().into()));
        images.want(&photo, 4, 2);
        images.fetch(&Tg::detached(tokio::sync::mpsc::unbounded_channel().0));
        images.on_built(rx.recv().await.unwrap());
        assert!(images.get(&photo, 4, 2).is_some(), "read by its contents");
        assert_eq!(size(images.measured_photo(&photo)), (80, 60));
        std::fs::remove_dir_all(&dir).unwrap();
    }

    /// Images for a terminal speaking kitty's protocol, its numbers from 1.
    fn kitty() -> Images {
        let mut picker = Picker::halfblocks();
        picker.set_protocol_type(ProtocolType::Kitty);
        let mut images = Images::new(picker, tokio::sync::mpsc::unbounded_channel().0);
        images.kitty_id = 0;
        images
    }

    /// 4 × 2 cells of 10 × 20 px.
    fn photo() -> DynamicImage {
        DynamicImage::new_rgba8(40, 40)
    }

    fn delete(id: u32) -> String {
        format!("\x1b_Ga=d,d=I,i={id},q=2\x1b\\")
    }

    #[test]
    fn a_kitty_terminal_is_told_to_delete_the_photos_dropped_by_the_number_they_were_sent_as() {
        use ratatui::buffer::Buffer;
        use ratatui::layout::Rect;
        use ratatui::widgets::Widget;
        use ratatui_image::sliced::{SignedPosition, SlicedImage};

        let mut images = kitty();
        images.insert_ready(key(7), photo());
        let mut buf = Buffer::empty(Rect::new(0, 0, 4, 2));
        let image = images.get(&preview(7), 4, 2).unwrap();
        SlicedImage::new(image, SignedPosition::from((0, 0))).render(buf.area, &mut buf);
        assert!(buf[(0, 0)].symbol().contains("i=1,a=T"), "sent as 1");
        assert_eq!(images.deletions(), None, "nothing dropped yet");

        images.clear();
        assert_eq!(images.deletions(), Some(delete(1)));
        assert_eq!(images.deletions(), None, "told once");
    }

    #[test]
    fn chat_photos_are_deleted_too_once_dropped_and_all_of_them_on_quitting() {
        let mut images = kitty();
        let avatar = |file_id| Key {
            avatar: true,
            ..key(file_id)
        };
        for file_id in 0..=MAX_AVATARS as i32 {
            images.avatar_clock += 1;
            images.insert_ready(avatar(file_id), photo());
        }
        assert_eq!(images.avatars.len(), MAX_AVATARS);
        assert_eq!(images.deletions(), Some(delete(1)), "the oldest");
        images.insert_ready(key(1000), photo());
        images.drop_all();
        let code = images.deletions().unwrap();
        assert_eq!(code.matches("a=d").count(), MAX_AVATARS + 1);
        assert!(images.avatars.is_empty() && images.ready.is_empty());
    }

    #[test]
    fn photos_kept_for_a_kitty_terminal_stay_under_its_quota_but_not_by_dropping_those_on_screen() {
        let mut images = kitty();
        images.kitty_budget = 2 * 40 * 40 * 4;
        images.insert_ready(key(1), photo());
        images.frame = 1;
        images.insert_ready(key(2), photo());
        // A frame draws the second, then a third comes.
        images.frame = 3;
        assert!(images.get(&preview(2), 4, 2).is_some());
        images.frame = 4;
        images.insert_ready(key(3), photo());
        assert!(!images.ready.contains_key(&key(1)), "drawn longest ago");
        assert!(images.ready.contains_key(&key(2)) && images.ready.contains_key(&key(3)));
        assert_eq!(images.deletions(), Some(delete(1)));

        // Every one kept is on screen: the quota gives way.
        assert!(images.get(&preview(2), 4, 2).is_some());
        assert!(images.get(&preview(3), 4, 2).is_some());
        images.frame = 5;
        images.insert_ready(key(4), photo());
        assert_eq!(images.ready.len(), 3);
        assert_eq!(images.deletions(), None);
    }

    #[test]
    fn zooming_in_the_viewer_drops_its_other_sizes_before_the_bubbles_photos() {
        let mut images = kitty();
        let window = |file_id| Key {
            cols: 40,
            rows: 17,
            ..key(file_id)
        };
        let picture = || DynamicImage::new_rgba8(400, 340);
        images.kitty_budget = 2 * 400 * 340 * 4;
        // Drawn before the viewer covered it.
        images.insert_ready(key(1), photo());
        images.frame = 2;
        images.insert_ready(window(2), picture());
        images.frame = 4;
        images.insert_ready(window(3), picture());
        assert!(
            images.ready.contains_key(&key(1)),
            "though drawn longest ago"
        );
        assert!(!images.ready.contains_key(&window(2)));
        assert!(images.ready.contains_key(&window(3)));
    }

    #[test]
    fn inside_tmux_the_code_that_deletes_images_goes_through_it() {
        assert_eq!(
            kitty_delete(1, true),
            "\x1bPtmux;\x1b\x1b_Ga=d,d=I,i=1,q=2\x1b\x1b\\\x1b\\"
        );
    }

    #[test]
    fn terminals_that_keep_no_images_are_told_nothing() {
        let mut images = Images::new(
            Picker::halfblocks(),
            tokio::sync::mpsc::unbounded_channel().0,
        );
        images.insert_ready(key(1), photo());
        images.drop_all();
        assert_eq!(images.deletions(), None);
    }

    #[test]
    fn images_bigger_than_telegram_sends_are_refused() {
        assert!(decode_bytes(&png(512, 512)).is_ok());
        let error = decode_bytes(&png(5000, 1)).unwrap_err();
        assert!(matches!(error, image::ImageError::Limits(_)), "{error}");
    }
}

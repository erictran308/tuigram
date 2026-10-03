//! Photos, shown inline in chat bubbles, and chat photos in the chat list.
//!
//! Every frame, the message view and the chat list ask for the photos they
//! have on screen. After the frame, [`Images::fetch`] starts whatever is
//! missing: a TDLib download, then decode + encode for the terminal on a
//! blocking thread. Until the real photo is ready, the blurry thumbnail that
//! came with the message (or chat) stands in.

use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use anyhow::Result;
use image::imageops::FilterType;
use image::{DynamicImage, RgbaImage};
use ratatui::layout::Size;
use ratatui_image::picker::{Picker, ProtocolType};
use ratatui_image::sliced::SlicedProtocol;
use ratatui_image::{FontSize, Resize};
use tokio::sync::mpsc::UnboundedSender;

use crate::chats::ChatPhoto;
use crate::messages::Preview;
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
    let mut limits = image::Limits::default();
    limits.max_image_width = Some(4096);
    limits.max_image_height = Some(4096);
    limits.max_alloc = Some(128 * 1024 * 1024);
    limits
}

/// Like `image::open` (the format comes from the extension), within [`limits`].
pub fn open_image(path: &str) -> image::ImageResult<image::DynamicImage> {
    let mut reader = image::ImageReader::open(path)?;
    reader.limits(limits());
    reader.decode()
}

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
    result: Result<SlicedProtocol>,
}

enum FileState {
    Downloading,
    Ready(String),
    Failed,
}

/// An encoded chat photo.
struct Avatar {
    image: SlicedProtocol,
    /// When it was last drawn, in calls to [`Images::avatar`].
    used: u64,
    /// It has been drawn. Kitty's protocol sends an image to the terminal
    /// the first time it's drawn, and only then, so that time must not be
    /// under a popup that hides it.
    shown: bool,
}

pub struct Images {
    picker: Picker,
    tx: UnboundedSender<ImageEvent>,
    ready: HashMap<Key, SlicedProtocol>,
    /// Kept apart from `ready`: they stay when another chat is opened.
    avatars: HashMap<Key, Avatar>,
    /// Counts calls to [`Images::avatar`], to find the least recently drawn.
    avatar_clock: u64,
    building: HashSet<Key>,
    /// Keys that failed to decode; never retried.
    failed: HashSet<Key>,
    files: HashMap<i32, FileState>,
    /// Photos the last frame showed, with their size.
    wanted: Vec<(Preview, u16, u16)>,
    /// Chat photos the last frame showed, with their size.
    wanted_avatars: Vec<(ChatPhoto, u16, u16)>,
}

impl Images {
    pub fn new(picker: Picker, tx: UnboundedSender<ImageEvent>) -> Self {
        Self {
            picker,
            tx,
            ready: HashMap::new(),
            avatars: HashMap::new(),
            avatar_clock: 0,
            building: HashSet::new(),
            failed: HashSet::new(),
            files: HashMap::new(),
            wanted: Vec::new(),
            wanted_avatars: Vec::new(),
        }
    }

    /// False when the terminal shows no real images, only colored blocks
    /// (halfblocks), which are too coarse for a chat photo in a few cells.
    pub fn draws_photos(&self) -> bool {
        self.picker.protocol_type() != ProtocolType::Halfblocks
    }

    /// Pixel size of one terminal cell, for sizing photos by aspect ratio.
    pub fn font_size(&self) -> FontSize {
        self.picker.font_size()
    }

    /// The best image ready to draw: the photo, else its blurry thumbnail.
    pub fn get(&self, photo: &Preview, cols: u16, rows: u16) -> Option<&SlicedProtocol> {
        let key = |thumbnail| Key {
            file_id: photo.file_id,
            cols,
            rows,
            thumbnail,
            avatar: false,
        };
        self.ready
            .get(&key(false))
            .or_else(|| self.ready.get(&key(true)))
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
        let key = [key(false), key(true)]
            .into_iter()
            .find(|k| self.avatars.get(k).is_some_and(|a| a.shown || !covered))?;
        self.avatar_clock += 1;
        let avatar = self.avatars.get_mut(&key)?;
        avatar.used = self.avatar_clock;
        avatar.shown = true;
        Some(&avatar.image)
    }

    /// Called while drawing, for each chat photo on screen.
    pub fn want_avatar(&mut self, photo: &ChatPhoto, cols: u16, rows: u16) {
        self.wanted_avatars.push((photo.clone(), cols, rows));
    }

    /// Starts downloads and encodes for what the last frame wanted.
    pub fn fetch(&mut self, tg: &Tg) {
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
                    self.build(full, move || Ok(open_image(&path)?));
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

    pub fn on_built(&mut self, event: ImageEvent) {
        self.building.remove(&event.key);
        match event.result {
            Ok(image) if event.key.avatar => self.add_avatar(event.key, image),
            Ok(image) => {
                self.ready.insert(event.key, image);
            }
            Err(_) => {
                self.failed.insert(event.key);
                if !event.key.thumbnail {
                    self.files.insert(event.key.file_id, FileState::Failed);
                }
            }
        }
    }

    fn add_avatar(&mut self, key: Key, image: SlicedProtocol) {
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

    /// Forgets every file, for a new TDLib client: it numbers files afresh.
    pub fn forget_files(&mut self) {
        self.ready.clear();
        self.avatars.clear();
        self.building.clear();
        self.failed.clear();
        self.files.clear();
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
        if built || self.building.contains(&key) || self.failed.contains(&key) {
            return;
        }
        self.building.insert(key);
        let picker = self.picker.clone();
        let tx = self.tx.clone();
        tokio::task::spawn_blocking(move || {
            let result = decode().and_then(|image| {
                let size = Size::new(key.cols, key.rows);
                Ok(SlicedProtocol::new_with_resize(
                    &picker, image, size, RESIZE,
                )?)
            });
            let _ = tx.send(ImageEvent { key, result });
        });
    }

    #[cfg(test)]
    pub fn insert_ready(&mut self, key: Key, image: image::DynamicImage) {
        let size = Size::new(key.cols, key.rows);
        let image = SlicedProtocol::new_with_resize(&self.picker, image, size, RESIZE).unwrap();
        if key.avatar {
            self.add_avatar(key, image);
        } else {
            self.ready.insert(key, image);
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
    fn images_bigger_than_telegram_sends_are_refused() {
        assert!(decode_bytes(&png(512, 512)).is_ok());
        let error = decode_bytes(&png(5000, 1)).unwrap_err();
        assert!(matches!(error, image::ImageError::Limits(_)), "{error}");
    }
}

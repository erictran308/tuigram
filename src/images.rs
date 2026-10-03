//! Photos, shown inline in chat bubbles.
//!
//! Every frame, the message view asks for the photos it has on screen. After
//! the frame, [`Images::fetch`] starts whatever is missing: a TDLib download,
//! then decode + encode for the terminal on a blocking thread. Until the real
//! photo is ready, the blurry thumbnail that came with the message stands in.

use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use anyhow::Result;
use image::imageops::FilterType;
use ratatui::layout::Size;
use ratatui_image::picker::Picker;
use ratatui_image::sliced::SlicedProtocol;
use ratatui_image::{FontSize, Resize};
use tokio::sync::mpsc::UnboundedSender;

use crate::messages::Preview;
use crate::tg::Tg;

/// Scale both ways (`Fit` never enlarges, which would leave the tiny thumbnail
/// tiny). Triangle filtering keeps the enlarged thumbnail soft, not blocky.
const RESIZE: Resize = Resize::Scale(Some(FilterType::Triangle));

/// One encoded image: a photo (or its thumbnail) at one size in cells.
#[derive(Clone, Copy, PartialEq, Eq, Hash)]
pub struct Key {
    pub file_id: i32,
    pub cols: u16,
    pub rows: u16,
    /// The blurry thumbnail embedded in the message, not the downloaded photo.
    pub thumbnail: bool,
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

pub struct Images {
    picker: Picker,
    tx: UnboundedSender<ImageEvent>,
    ready: HashMap<Key, SlicedProtocol>,
    building: HashSet<Key>,
    /// Keys that failed to decode; never retried.
    failed: HashSet<Key>,
    files: HashMap<i32, FileState>,
    /// Photos the last frame showed, with their size.
    wanted: Vec<(Preview, u16, u16)>,
}

impl Images {
    pub fn new(picker: Picker, tx: UnboundedSender<ImageEvent>) -> Self {
        Self {
            picker,
            tx,
            ready: HashMap::new(),
            building: HashSet::new(),
            failed: HashSet::new(),
            files: HashMap::new(),
            wanted: Vec::new(),
        }
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

    /// Starts downloads and encodes for what the last frame wanted.
    pub fn fetch(&mut self, tg: &Tg) {
        for (photo, cols, rows) in std::mem::take(&mut self.wanted) {
            let full = Key {
                file_id: photo.file_id,
                cols,
                rows,
                thumbnail: false,
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

    pub fn on_downloaded(&mut self, file_id: i32, path: Option<String>) {
        let state = path.map_or(FileState::Failed, FileState::Ready);
        self.files.insert(file_id, state);
    }

    pub fn on_built(&mut self, event: ImageEvent) {
        self.building.remove(&event.key);
        match event.result {
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

    /// Drops encoded images, e.g. when switching chats. Downloads stay on disk.
    pub fn clear(&mut self) {
        self.ready.clear();
    }

    /// Forgets every file, for a new TDLib client: it numbers files afresh.
    pub fn forget_files(&mut self) {
        self.ready.clear();
        self.building.clear();
        self.failed.clear();
        self.files.clear();
        self.wanted.clear();
    }

    /// Decodes and encodes on a blocking thread, once per key.
    fn build(
        &mut self,
        key: Key,
        decode: impl FnOnce() -> Result<image::DynamicImage> + Send + 'static,
    ) {
        if self.ready.contains_key(&key)
            || self.building.contains(&key)
            || self.failed.contains(&key)
        {
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
        self.ready.insert(key, image);
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
    fn images_bigger_than_telegram_sends_are_refused() {
        assert!(decode_bytes(&png(512, 512)).is_ok());
        let error = decode_bytes(&png(5000, 1)).unwrap_err();
        assert!(matches!(error, image::ImageError::Limits(_)), "{error}");
    }
}

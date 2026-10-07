//! The system clipboard: copying text, files, and photos as images, and
//! pasting photos and files to send.

use std::borrow::Cow;
use std::io::{Write, stdout};
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use base64::Engine;
use image::codecs::jpeg::JpegEncoder;
use image::{DynamicImage, ImageFormat, RgbaImage};
use tokio::sync::mpsc::UnboundedSender;

/// Telegram takes photos up to 10 MB. A pasted image saved bigger than this
/// as PNG is saved as a JPEG instead.
const PNG_MAX_BYTES: u64 = 10 * 1024 * 1024;
/// Pasted images are kept this long: TDLib uploads from the file, and
/// carries on with an upload after a restart.
const OUTBOX_KEEP: Duration = Duration::from_secs(7 * 24 * 60 * 60);

/// How text got to the clipboard.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Copied {
    /// Straight into the system clipboard.
    System,
    /// Handed to the terminal, which puts it in the clipboard if it supports
    /// that (most modern ones do). There's no way to tell if it did.
    Terminal,
}

/// Work done off the UI thread, back for the app.
pub enum ClipboardEvent {
    Decoded(Decoded),
    Pasted(Pasted),
}

/// A photo decoded off the UI thread, back to be put on the clipboard.
pub struct Decoded {
    /// What the toast calls it.
    pub label: String,
    pub image: Result<RgbaImage, String>,
}

/// What the clipboard held when `p` asked, for the chat that was open then,
/// and in a forum the topic.
pub struct Pasted {
    pub place: (i64, Option<i32>),
    pub content: Result<Paste, String>,
}

pub enum Paste {
    /// Files copied in a file manager.
    Files(Vec<PathBuf>),
    /// An image (a screenshot, one copied from a browser…), saved to a file.
    Image(PathBuf),
    Text(String),
}

pub struct Clipboard {
    /// Kept for the whole run: on Linux, what was copied stays on the
    /// clipboard only while the program that copied it holds on to it.
    system: Option<arboard::Clipboard>,
    tx: UnboundedSender<ClipboardEvent>,
    /// Where pasted images are saved for TDLib to upload.
    outbox: PathBuf,
}

impl Clipboard {
    pub fn new(tx: UnboundedSender<ClipboardEvent>, outbox: PathBuf) -> Self {
        Self {
            system: None,
            tx,
            outbox,
        }
    }

    fn system(&mut self) -> Result<&mut arboard::Clipboard, arboard::Error> {
        if self.system.is_none() {
            self.system = Some(arboard::Clipboard::new()?);
        }
        Ok(self.system.as_mut().expect("just set"))
    }

    /// Copies `text`. Where there's no system clipboard to reach (over SSH,
    /// or a Linux box without a display), asks the terminal instead.
    pub fn copy_text(&mut self, text: &str) -> std::io::Result<Copied> {
        if let Ok(system) = self.system()
            && system.set_text(text).is_ok()
        {
            return Ok(Copied::System);
        }
        let mut out = stdout();
        out.write_all(osc52(text).as_bytes())?;
        out.flush()?;
        Ok(Copied::Terminal)
    }

    /// Copies a file the way a file manager does, so pasting attaches it.
    pub fn copy_file(&mut self, path: &Path) -> Result<(), arboard::Error> {
        self.system()?.set().file_list(&[path])
    }

    /// Decodes a downloaded photo on a blocking thread; it comes back as a
    /// [`Decoded`] for [`Self::copy_image`].
    pub fn decode_image(&self, path: String, label: String) {
        let tx = self.tx.clone();
        tokio::task::spawn_blocking(move || {
            // The photo is another user's, so a decoder panic is caught here
            // as it is for the chat's own copy.
            let image = crate::images::contained(|| Ok(crate::images::open_image(&path)?))
                .map(|i| i.to_rgba8())
                .map_err(|e| e.to_string());
            let _ = tx.send(ClipboardEvent::Decoded(Decoded { label, image }));
        });
    }

    /// Reads the clipboard on a blocking thread, since an image takes a
    /// moment to convert and save. It comes back as a [`Pasted`] for
    /// `place`: files first, as a file manager copies an icon image along
    /// with them, then an image, then text.
    pub fn paste(&self, place: (i64, Option<i32>)) {
        let tx = self.tx.clone();
        let outbox = self.outbox.clone();
        tokio::task::spawn_blocking(move || {
            let content = read(&outbox);
            let _ = tx.send(ClipboardEvent::Pasted(Pasted { place, content }));
        });
    }

    pub fn copy_image(&mut self, image: RgbaImage) -> Result<(), arboard::Error> {
        let (width, height) = image.dimensions();
        self.system()?.set_image(arboard::ImageData {
            width: width as usize,
            height: height as usize,
            bytes: Cow::Owned(image.into_raw()),
        })
    }
}

fn read(outbox: &Path) -> Result<Paste, String> {
    // A clipboard of its own, as this runs on another thread.
    let mut clipboard =
        arboard::Clipboard::new().map_err(|e| format!("Can't reach the clipboard: {e}"))?;
    if let Ok(files) = clipboard.get().file_list()
        && !files.is_empty()
    {
        return Ok(Paste::Files(files));
    }
    if let Ok(image) = clipboard.get_image() {
        let image = RgbaImage::from_raw(
            image.width as u32,
            image.height as u32,
            image.bytes.into_owned(),
        )
        .ok_or("The image on the clipboard is broken")?;
        return save_image(image, outbox)
            .map(Paste::Image)
            .map_err(|e| format!("Can't save the image: {e}"));
    }
    match clipboard.get_text() {
        Ok(text) if !text.is_empty() => Ok(Paste::Text(text)),
        _ => Err("Nothing to paste: the clipboard is empty".into()),
    }
}

/// Saves a pasted image in `outbox` as PNG, or as JPEG when the PNG would
/// be too big for Telegram to take as a photo.
fn save_image(image: RgbaImage, outbox: &Path) -> Result<PathBuf, Box<dyn std::error::Error>> {
    private_folder(outbox)?;
    let stamp = SystemTime::now().duration_since(UNIX_EPOCH)?.as_millis();
    let png = outbox.join(format!("pasted-{stamp}.png"));
    image.write_to(
        &mut std::io::BufWriter::new(new_file(&png)?),
        ImageFormat::Png,
    )?;
    if std::fs::metadata(&png)?.len() <= PNG_MAX_BYTES {
        return Ok(png);
    }
    std::fs::remove_file(&png)?;
    let jpeg = outbox.join(format!("pasted-{stamp}.jpg"));
    let file = std::io::BufWriter::new(new_file(&jpeg)?);
    // JPEG has no transparency.
    DynamicImage::ImageRgba8(image)
        .to_rgb8()
        .write_with_encoder(JpegEncoder::new_with_quality(file, 90))?;
    Ok(jpeg)
}

/// The outbox, readable only by you, like the data folder it's in, so what
/// you pasted stays yours even if that folder's own protection fails.
fn private_folder(path: &Path) -> std::io::Result<()> {
    let mut builder = std::fs::DirBuilder::new();
    builder.recursive(true);
    #[cfg(unix)]
    std::os::unix::fs::DirBuilderExt::mode(&mut builder, 0o700);
    builder.create(path)
}

/// A new file only you can read. Never one that's there already, or a link.
fn new_file(path: &Path) -> std::io::Result<std::fs::File> {
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    std::os::unix::fs::OpenOptionsExt::mode(&mut options, 0o600);
    options.open(path)
}

/// Removes pasted images older than [`OUTBOX_KEEP`]. Errors don't matter:
/// it's tried again on the next start. Only files named as tuigram names
/// them go: the data folder can be set from a `.env` file, so this folder
/// might not be tuigram's own.
pub fn clean_outbox(outbox: &Path) {
    let Ok(entries) = std::fs::read_dir(outbox) else {
        return;
    };
    for entry in entries.flatten() {
        let name = entry.file_name();
        let ours = name.to_str().is_some_and(|name| {
            name.strip_prefix("pasted-")
                .and_then(|n| n.strip_suffix(".png").or_else(|| n.strip_suffix(".jpg")))
                .is_some_and(|stamp| !stamp.is_empty() && stamp.bytes().all(|b| b.is_ascii_digit()))
        });
        // The entry itself, not what a link points to.
        let old = entry
            .metadata()
            .ok()
            .filter(std::fs::Metadata::is_file)
            .and_then(|m| m.modified().ok())
            .and_then(|modified| modified.elapsed().ok())
            .is_some_and(|age| age > OUTBOX_KEEP);
        if ours && old {
            let _ = std::fs::remove_file(entry.path());
        }
    }
}

/// The escape sequence asking a terminal to put `text` on the clipboard.
fn osc52(text: &str) -> String {
    let encoded = base64::engine::general_purpose::STANDARD.encode(text);
    format!("\x1b]52;c;{encoded}\x07")
}

#[cfg(test)]
mod tests {
    use super::*;

    // Only the escape sequence is tested: a test that copied for real would
    // overwrite whatever the person running the tests had copied.
    #[test]
    fn the_terminal_gets_the_text_in_base64() {
        assert_eq!(osc52("hi ✓"), "\x1b]52;c;aGkg4pyT\x07");
    }

    #[test]
    fn pasted_images_are_saved_as_png() {
        let outbox =
            std::env::temp_dir().join(format!("tuigram-test-outbox-{}", std::process::id()));
        let path = save_image(RgbaImage::new(30, 20), &outbox).unwrap();
        assert_eq!(path.extension().unwrap(), "png");
        assert_eq!(image::image_dimensions(&path).unwrap(), (30, 20));
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = |p: &Path| std::fs::metadata(p).unwrap().permissions().mode() & 0o777;
            assert_eq!(mode(&path), 0o600, "only you can read what you pasted");
            assert_eq!(mode(&outbox), 0o700);
        }
        clean_outbox(&outbox);
        assert!(path.exists(), "new ones are kept");
        let _ = std::fs::remove_dir_all(&outbox);
    }

    #[test]
    fn only_old_pasted_images_are_cleaned_out() {
        let outbox =
            std::env::temp_dir().join(format!("tuigram-test-clean-{}", std::process::id()));
        std::fs::create_dir_all(&outbox).unwrap();
        let old = SystemTime::now() - OUTBOX_KEEP - Duration::from_secs(60);
        let file = |name: &str| {
            let path = outbox.join(name);
            std::fs::write(&path, b"x").unwrap();
            let file = std::fs::File::options().write(true).open(&path).unwrap();
            file.set_modified(old).unwrap();
            path
        };
        let png = file("pasted-1790000000000.png");
        let jpeg = file("pasted-1790000000001.jpg");
        let other = file("report.pdf");
        let lookalike = file("pasted-notes.png");
        clean_outbox(&outbox);
        assert!(!png.exists() && !jpeg.exists());
        assert!(other.exists() && lookalike.exists(), "not tuigram's");
        std::fs::remove_dir_all(&outbox).unwrap();
    }
}

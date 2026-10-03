//! Copying to the system clipboard: text, files, and photos as images.

use std::borrow::Cow;
use std::io::{Write, stdout};
use std::path::Path;

use base64::Engine;
use image::RgbaImage;
use tokio::sync::mpsc::UnboundedSender;

/// How text got to the clipboard.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Copied {
    /// Straight into the system clipboard.
    System,
    /// Handed to the terminal, which puts it in the clipboard if it supports
    /// that (most modern ones do). There's no way to tell if it did.
    Terminal,
}

/// A photo decoded off the UI thread, back to be put on the clipboard.
pub struct Decoded {
    /// What the toast calls it.
    pub label: String,
    pub image: Result<RgbaImage, String>,
}

pub struct Clipboard {
    /// Kept for the whole run: on Linux, what was copied stays on the
    /// clipboard only while the program that copied it holds on to it.
    system: Option<arboard::Clipboard>,
    tx: UnboundedSender<Decoded>,
}

impl Clipboard {
    pub fn new(tx: UnboundedSender<Decoded>) -> Self {
        Self { system: None, tx }
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
            let image = crate::images::open_image(&path)
                .map(|i| i.to_rgba8())
                .map_err(|e| e.to_string());
            let _ = tx.send(Decoded { label, image });
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
}

//! Video messages played in the terminal itself: ffmpeg decodes frames to
//! raw RGB in a tmpfs file at the video's own rate, kitty's protocol puts
//! each where the overlay's blank area is, and mpv plays the sound.
//! Zooming restarts the decoder at the same position with a bigger frame.
//! Everything but the few calls app.rs makes lives here.

use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering::Relaxed};

use anyhow::{Context, Result};
use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::text::Span;
use ratatui::widgets::{Clear, Paragraph};
use ratatui_image::FontSize;
use ratatui_image::picker::cap_parser::Parser;
use tdlib_rs::types::VideoNote;
use tokio::sync::mpsc::UnboundedSender;

use crate::messages::{MediaFile, Msg};
use crate::term::{self, Terminal};
use crate::theme::Colors;
use crate::viewer::SIZES;

/// The longest file played in the terminal: video messages top out near a
/// minute, this leaves room for long ones sent as video files.
pub const MAX_FILE: u64 = 256 << 20;

/// A video message, as TDLib describes it.
#[derive(Clone)]
pub struct Note {
    pub file_id: i32,
    pub size: u64,
    pub duration: i32,
}

impl Note {
    pub fn new(note: &VideoNote) -> Self {
        let size = note.video.size.max(note.video.expected_size);
        Self {
            file_id: note.video.id,
            size: u64::try_from(size).unwrap_or(0),
            duration: note.duration.max(0),
        }
    }
}

/// Word from the decoder thread.
pub enum VideoEvent {
    /// A new frame waits in the tmpfs file.
    Frame,
    /// The video played out.
    Ended,
    Failed(String),
}

/// The number kitty knows a video's frames by: outside the range images
/// take, so one's deletions never touch the other's.
const KITTY_ID: u32 = 0x7000_0000;

/// Where the frame file lives: tmpfs, so kitty reads it at memory speed.
fn frame_dir() -> PathBuf {
    PathBuf::from(format!("/dev/shm/tuigram-video-{}", std::process::id()))
}

/// What plays: ffmpeg feeding frames, mpv on the sound. Both pause and
/// resume together (SIGSTOP/SIGCONT), and die with the view.
struct Player {
    ffmpeg: Child,
    audio: Option<Child>,
    /// Frames written to the file this run.
    frames: Arc<AtomicU64>,
    stop: Arc<AtomicBool>,
    paused: bool,
}

impl Player {
    fn toggle(&mut self) {
        let signal = if self.paused {
            libc::SIGCONT
        } else {
            libc::SIGSTOP
        };
        for child in std::iter::once(&self.ffmpeg).chain(&self.audio) {
            let pid: i32 = match child.id().try_into() {
                Ok(pid) if pid > 0 => pid,
                _ => continue,
            };
            unsafe { libc::kill(pid, signal) };
        }
        self.paused = !self.paused;
    }
}

impl Drop for Player {
    fn drop(&mut self) {
        self.stop.store(true, Relaxed);
        let _ = self.ffmpeg.kill();
        if let Some(audio) = &mut self.audio {
            let _ = audio.kill();
        }
    }
}

/// Reads frames from `pipe` (width × height × 3 bytes each) into the tmpfs
/// file, atomically: a half-written one never reaches the terminal. At the
/// video's own rate, since ffmpeg runs with `-re`.
fn pump_frames(
    mut pipe: impl Read,
    width: u32,
    height: u32,
    frames: Arc<AtomicU64>,
    stop: Arc<AtomicBool>,
    tx: UnboundedSender<VideoEvent>,
) {
    let size = width as usize * height as usize * 3;
    let dir = frame_dir();
    let tmp = dir.join("frame.tmp");
    let out = dir.join("frame.rgb");
    let mut buf = vec![0u8; size];
    loop {
        if stop.load(Relaxed) {
            return;
        }
        if let Err(e) = pipe.read_exact(&mut buf) {
            if !stop.load(Relaxed) {
                let event = match e.kind() {
                    std::io::ErrorKind::UnexpectedEof => VideoEvent::Ended,
                    _ => VideoEvent::Failed(format!("decoding broke: {e}")),
                };
                let _ = tx.send(event);
            }
            return;
        }
        let written = std::fs::write(&tmp, &buf).and_then(|()| std::fs::rename(&tmp, &out));
        if let Err(e) = written {
            let _ = tx.send(VideoEvent::Failed(format!("the frame file can't be written: {e}")));
            return;
        }
        frames.fetch_add(1, Relaxed);
        if tx.send(VideoEvent::Frame).is_err() {
            return;
        }
    }
}

/// Starts decoding `path` from `start` seconds, scaled into width × height
/// pixels, with the sound in mpv. No audio rather than no video when mpv
/// is missing.
fn spawn(
    path: &Path,
    start: f64,
    width: u32,
    height: u32,
    tx: UnboundedSender<VideoEvent>,
) -> Result<Player> {
    std::fs::create_dir_all(frame_dir())?;
    let frames = Arc::new(AtomicU64::new(0));
    let stop = Arc::new(AtomicBool::new(false));
    // Fit into the box, padded to it: the frame's size never changes, so
    // the reader can count bytes.
    let filter = format!(
        "scale={width}:{height}:force_original_aspect_ratio=decrease,\
         pad={width}:{height}:(ow-iw)/2:(oh-ih)/2"
    );
    let mut ffmpeg = Command::new("ffmpeg")
        .args([
            "-v",
            "error",
            "-nostdin",
            "-re",
            "-ss",
            &format!("{start:.3}"),
            "-i",
            &path.to_string_lossy(),
            "-an",
            "-vf",
            &filter,
            "-f",
            "rawvideo",
            "-pix_fmt",
            "rgb24",
            "pipe:1",
        ])
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .context("ffmpeg isn't installed")?;
    let pipe = ffmpeg.stdout.take().context("ffmpeg's frames are piped")?;
    let audio = Command::new("mpv")
        .args([
            "--no-video",
            "--really-quiet",
            &format!("--start={start:.3}"),
            &path.to_string_lossy(),
        ])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .ok();
    let (frames2, stop2) = (Arc::clone(&frames), Arc::clone(&stop));
    std::thread::spawn(move || pump_frames(pipe, width, height, frames2, stop2, tx));
    Ok(Player {
        ffmpeg,
        audio,
        frames,
        stop,
        paused: false,
    })
}

/// The video's shape and frame rate, read once the file is here: the rate
/// tells where it is by frames shown, the shape sizes the overlay.
fn probe(path: &Path) -> (f64, f64) {
    let parsed = Command::new("ffprobe")
        .args([
            "-v",
            "error",
            "-select_streams",
            "v:0",
            "-show_entries",
            "stream=width,height,r_frame_rate",
            "-of",
            "csv=p=0",
            &path.to_string_lossy(),
        ])
        .output()
        .ok()
        .and_then(|o| String::from_utf8(o.stdout).ok())
        .and_then(|s| {
            let mut parts = s.trim().split(',');
            let w: f64 = parts.next()?.parse().ok()?;
            let h: f64 = parts.next()?.parse().ok()?;
            let rate = parts.next()?;
            let mut rate = rate.split('/');
            let num: f64 = rate.next()?.parse().ok()?;
            let den: f64 = rate.next().map_or(Ok(1.0), str::parse).ok()?;
            Some((w / h.max(1.0), num / den.max(0.001)))
        });
    match parsed {
        Some((aspect, fps)) if fps >= 1.0 && fps <= 240.0 && aspect > 0.0 => (aspect, fps),
        _ => (1.0, 30.0),
    }
}

/// A video message playing in the terminal, over everything but the status
/// bar, as the photo viewer shows photos.
pub struct VideoView {
    pub chat_id: i64,
    /// The message it's in.
    pub message_id: i64,
    /// What `o` opens in another app.
    pub file: MediaFile,
    pub note: Note,
    /// The downloaded file, once TDLib brings it.
    path: Option<PathBuf>,
    /// Width over height, and frames a second.
    aspect: f64,
    fps: f64,
    player: Option<Player>,
    tx: UnboundedSender<VideoEvent>,
    /// Seconds into the video the current run started at.
    start: f64,
    /// The size `j` or `k` picked, in [`SIZES`]. `None` until then: filling
    /// the window.
    zoom: Option<usize>,
    /// The cells the picture filled last frame.
    rect: Option<Rect>,
    /// The pixel size the running player decodes at: a change (zoom, the
    /// window resized) restarts it at the same position.
    pixels: Option<(u32, u32)>,
    /// A frame waits to be sent to the terminal.
    pending: bool,
    /// Why it can't play, once: the app shows it and closes the view.
    pub error: Option<String>,
    /// Escapes go through tmux's passthrough when detected.
    tmux: bool,
}

impl VideoView {
    /// The video message of message `message_id`, `msg`, if it has one the
    /// terminal can play.
    pub fn of(
        chat_id: i64,
        message_id: i64,
        msg: &Msg,
        tx: UnboundedSender<VideoEvent>,
        tmux: bool,
    ) -> Option<Self> {
        let note = msg.video.clone()?;
        let file = msg.file.clone().filter(|f| f.id == note.file_id)?;
        Some(Self {
            chat_id,
            message_id,
            file,
            note,
            path: None,
            aspect: 1.0,
            fps: 30.0,
            player: None,
            tx,
            start: 0.0,
            zoom: None,
            rect: None,
            pixels: None,
            pending: false,
            error: None,
            tmux,
        })
    }

    /// The file Enter was on.
    pub fn waits_for(&self, file_id: i32) -> bool {
        self.path.is_none() && self.note.file_id == file_id
    }

    /// The file is downloaded: played from the next frame drawn.
    pub fn downloaded(&mut self, path: PathBuf) {
        let (aspect, fps) = probe(&path);
        self.aspect = aspect;
        self.fps = fps;
        self.path = Some(path);
    }

    /// Where it is, in seconds, by frames shown this run.
    fn position(&self) -> f64 {
        let shown = self.player.as_ref().map_or(0, |p| p.frames.load(Relaxed));
        self.start + shown as f64 / self.fps
    }

    /// Playing; paused; waiting for the file.
    pub fn status(&self) -> &'static str {
        match &self.player {
            None => "loading…",
            Some(p) if p.paused => "paused",
            Some(_) => "playing",
        }
    }

    /// Message `message_id` of chat `chat_id`, now `msg`, no longer has the
    /// video message shown.
    pub fn lost(&self, chat_id: i64, message_id: i64, msg: Option<&Msg>) -> bool {
        chat_id == self.chat_id
            && message_id == self.message_id
            && msg.and_then(|m| m.video.as_ref()).map(|v| v.file_id) != Some(self.note.file_id)
    }

    /// Play or pause.
    pub fn toggle(&mut self) {
        if let Some(player) = &mut self.player {
            player.toggle();
        }
    }

    /// Bigger, as in the photo viewer; the decoder restarts at the same
    /// position on the next frame drawn, at the new size.
    pub fn zoom_in(&mut self) {
        let now = self.share();
        if let Some(i) = SIZES.iter().position(|&s| s > now + 0.001) {
            self.zoom = Some(i);
        }
    }

    /// Smaller.
    pub fn zoom_out(&mut self) {
        let now = self.share();
        if let Some(i) = SIZES.iter().rposition(|&s| s < now - 0.001) {
            self.zoom = Some(i);
        }
    }

    fn share(&self) -> f64 {
        self.zoom.map_or(1.0, |i| SIZES[i])
    }

    /// Word from the decoder: a frame to send, the end (looped, as
    /// Telegram's apps loop video messages), or why it can't play — `Err`
    /// closes the view.
    pub fn on_event(&mut self, event: VideoEvent) -> Result<(), String> {
        match event {
            VideoEvent::Frame => {
                self.pending = true;
                Ok(())
            }
            VideoEvent::Ended => {
                self.player = None;
                self.start = 0.0;
                self.pixels = None; // restarts on the next frame drawn
                Ok(())
            }
            VideoEvent::Failed(why) => Err(why),
        }
    }

    /// The picture's cells within `inner`, centered, keeping the video's
    /// shape at the zoom picked. Font cells are about twice as tall as
    /// wide: the pixel box accounts for it.
    fn layout(&self, inner: Rect, font: FontSize) -> (Rect, (u32, u32)) {
        let share = self.share();
        let (fw, fh) = (
            f64::from(font.width).max(1.0),
            f64::from(font.height).max(1.0),
        );
        let mut cols = f64::from(inner.width) * share;
        let mut rows = f64::from(inner.height) * share;
        let wide = cols * fw / (rows * fh);
        if wide > self.aspect {
            cols = rows * fh / fw * self.aspect;
        } else {
            rows = cols * fw / fh / self.aspect;
        }
        let cols = cols.max(1.0) as u16;
        let rows = rows.max(1.0) as u16;
        let rect = Rect {
            x: inner.x + inner.width.saturating_sub(cols) / 2,
            y: inner.y + inner.height.saturating_sub(rows) / 2,
            width: cols,
            height: rows,
        };
        let pixels = (
            (f64::from(cols) * fw) as u32,
            (f64::from(rows) * fh) as u32,
        );
        (rect, pixels)
    }

    /// The overlay: a blank board the frames land on, with a title. The
    /// player (re)starts once the picture's size is known or it changed.
    pub fn draw(&mut self, frame: &mut Frame, area: Rect, font: FontSize, colors: &Colors) {
        let block = crate::ui::bordered(colors);
        let inner = block.inner(area);
        let (at, pixels) = self.layout(inner, font);
        self.rect = Some(at);
        let now = self.position().min(f64::from(self.note.duration)) as u32;
        let of = self.note.duration.max(0) as u32;
        let mut title = format!(
            " {} · {} · {}:{:02} of {}:{:02}",
            self.file.label,
            self.status(),
            now / 60,
            now % 60,
            of / 60,
            of % 60
        );
        if let Some(i) = self.zoom {
            title.push_str(&format!(" · {:.0}%", SIZES[i] * 100.0));
        }
        frame.render_widget(Clear, area);
        frame.render_widget(Paragraph::new(Span::from(title)).block(block), area);
        frame.render_widget(Clear, at);

        let needs_restart =
            self.path.is_some() && (self.player.is_none() || self.pixels != Some(pixels));
        if needs_restart && self.error.is_none() {
            let position = if self.player.is_some() {
                self.position()
            } else {
                self.start
            };
            let was_paused = self.player.as_ref().is_some_and(|p| p.paused);
            self.player = None;
            match spawn(
                &self.path.clone().unwrap(),
                position,
                pixels.0,
                pixels.1,
                self.tx.clone(),
            ) {
                Ok(mut player) => {
                    if was_paused {
                        player.toggle();
                    }
                    self.player = Some(player);
                    self.start = position;
                    self.pixels = Some(pixels);
                }
                Err(e) => self.error = Some(format!("{e:#}")),
            }
        }
    }

    /// Sends the newest frame to the terminal, if one waits and the picture
    /// has a place. Called after each frame ratatui draws, so the picture
    /// lands over the board's blank cells.
    pub fn send_frame(&mut self, terminal: &mut Terminal) {
        if !self.pending || self.player.is_none() {
            return;
        }
        let (Some(at), Some((w, h))) = (self.rect, self.pixels) else {
            return;
        };
        self.pending = false;
        let file = frame_dir().join("frame.rgb");
        use base64::Engine;
        let path =
            base64::engine::general_purpose::STANDARD.encode(file.as_os_str().as_encoded_bytes());
        let (start, escape, end) = Parser::tmux_start_escape_end(self.tmux);
        let codes = [
            // The picture goes at its cell, whatever ratatui left under the
            // cursor.
            format!("\x1b[{};{}H", at.y + 1, at.x + 1),
            format!(
                "{start}{escape}_Ga=T,t=f,f=24,s={w},v={h},i={KITTY_ID},q=2,c={},r={};{path}{escape}\\{end}",
                at.width, at.height
            ),
        ];
        let _ = term::send(terminal, codes);
    }
}

/// The video's frames go away with the view: kitty's image is deleted, the
/// decoder and the sound die with the player, the tmpfs file is removed.
impl Drop for VideoView {
    fn drop(&mut self) {
        let (start, escape, end) = Parser::tmux_start_escape_end(self.tmux);
        let code = format!("{start}{escape}_Ga=d,d=I,i={KITTY_ID},q=2{escape}\\{end}");
        use std::io::Write;
        let _ = std::io::stdout().write_all(code.as_bytes());
        let _ = std::io::stdout().flush();
        let _ = std::fs::remove_dir_all(frame_dir());
    }
}

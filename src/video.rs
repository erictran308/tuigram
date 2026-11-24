//! Video messages played in the terminal itself: ffmpeg decodes frames to
//! raw RGBA in a tmpfs file at the video's own rate, the circle outside the
//! round frame goes transparent, kitty's protocol puts each where the
//! overlay's blank area is, and mpv plays the sound.
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
    /// Diameter in pixels: video notes are square.
    pub length: u32,
}

impl Note {
    pub fn new(note: &VideoNote) -> Self {
        let size = note.video.size.max(note.video.expected_size);
        Self {
            file_id: note.video.id,
            size: u64::try_from(size).unwrap_or(0),
            duration: note.duration.max(0),
            length: note.length.max(1) as u32,
        }
    }
}

/// Word from the decoder thread, tagged with the player it serves: the
/// fullscreen view and the inline one share the channel.
pub enum VideoEvent {
    /// A new frame waits in the tmpfs file.
    Frame(u8),
    /// The video played out.
    Ended(u8),
    Failed(u8, String),
}

impl VideoEvent {
    /// The player the word is for: [`TAG_VIEW`] or [`TAG_INLINE`].
    pub fn tag(&self) -> u8 {
        match self {
            VideoEvent::Frame(tag) | VideoEvent::Ended(tag) | VideoEvent::Failed(tag, _) => *tag,
        }
    }
}

/// Tag of the fullscreen player's decoder.
pub const TAG_VIEW: u8 = 0;
/// Tag of the inline player's decoder.
pub const TAG_INLINE: u8 = 1;

/// The number kitty knows a video's frames by: outside the range images
/// take, so one's deletions never touch the other's.
const KITTY_ID: u32 = 0x7000_0000;

/// Where the frame file lives: tmpfs, so kitty reads it at memory speed;
/// the system temp dir where there is no /dev/shm (macOS, Windows).
fn frame_dir() -> PathBuf {
    let shm = PathBuf::from("/dev/shm");
    let dir = if shm.is_dir() {
        shm
    } else {
        std::env::temp_dir()
    };
    dir.join(format!("tuigram-video-{}", std::process::id()))
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
    /// Play or pause: the decoder and the sound freeze together. Unix
    /// signals do it; Windows has nothing like them, so there pause just
    /// isn't there.
    #[cfg(unix)]
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

    #[cfg(not(unix))]
    fn toggle(&mut self) {}
}

impl Drop for Player {
    fn drop(&mut self) {
        self.stop.store(true, Relaxed);
        let _ = self.ffmpeg.kill();
        // Reaped too: killed or finished, a child whose exit no one waits
        // for stays a zombie until tuigram itself exits.
        let _ = self.ffmpeg.wait();
        if let Some(audio) = &mut self.audio {
            let _ = audio.kill();
            let _ = audio.wait();
        }
    }
}

/// Reads frames from `pipe` (width × height × 4 bytes each, RGBA) into the
/// tmpfs file, atomically: a half-written one never reaches the terminal.
/// At the video's own rate, since ffmpeg runs with `-re`. The pixels
/// outside the frame's circle go transparent: video messages are round.
fn pump_frames(
    mut pipe: impl Read,
    width: u32,
    height: u32,
    frames: Arc<AtomicU64>,
    stop: Arc<AtomicBool>,
    tag: u8,
    tx: UnboundedSender<VideoEvent>,
) {
    let size = width as usize * height as usize * 4;
    let dir = frame_dir();
    let tmp = dir.join(format!("frame{tag}.tmp"));
    let out = dir.join(format!("frame{tag}.rgb"));
    let mut buf = vec![0u8; size];
    loop {
        if stop.load(Relaxed) {
            return;
        }
        if let Err(e) = pipe.read_exact(&mut buf) {
            if !stop.load(Relaxed) {
                let event = match e.kind() {
                    std::io::ErrorKind::UnexpectedEof => VideoEvent::Ended(tag),
                    _ => VideoEvent::Failed(tag, format!("decoding broke: {e}")),
                };
                let _ = tx.send(event);
            }
            return;
        }
        mask_circle(&mut buf, width, height);
        let written = std::fs::write(&tmp, &buf).and_then(|()| std::fs::rename(&tmp, &out));
        if let Err(e) = written {
            let _ = tx.send(VideoEvent::Failed(
                tag,
                format!("the frame file can't be written: {e}"),
            ));
            return;
        }
        frames.fetch_add(1, Relaxed);
        if tx.send(VideoEvent::Frame(tag)).is_err() {
            return;
        }
    }
}

/// Sets each pixel's alpha by how much of it the frame's inscribed circle
/// covers: the corners go transparent, and kitty draws what's under them —
/// the bubble. The edge is a shade, not a step, so it doesn't look jagged.
/// ffmpeg's frames are opaque, so only the alpha byte is written.
fn mask_circle(buf: &mut [u8], width: u32, height: u32) {
    let (w, h) = (f64::from(width), f64::from(height));
    let (cx, cy) = ((w - 1.0) / 2.0, (h - 1.0) / 2.0);
    let r = w.min(h) / 2.0;
    for y in 0..height {
        // Pixels of this row inside the circle have |x - cx| <= half.
        let half = (r * r - (f64::from(y) - cy).powi(2)).max(0.0).sqrt();
        let (lo, hi) = (cx - half, cx + half + 1.0);
        for x in 0..width {
            let xf = f64::from(x);
            let cover = (hi.min(xf + 1.0) - lo.max(xf)).clamp(0.0, 1.0);
            buf[(y as usize * width as usize + x as usize) * 4 + 3] = (cover * 255.0) as u8;
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
    tag: u8,
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
            "rgba",
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
    std::thread::spawn(move || pump_frames(pipe, width, height, frames2, stop2, tag, tx));
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
        Some((aspect, fps)) if (1.0..=240.0).contains(&fps) && aspect > 0.0 => (aspect, fps),
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
    /// It played out: video messages play once, then the app closes the view.
    done: bool,
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
            done: false,
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

    /// Continue from `secs` in: a video message playing inline grows to the
    /// view where it is.
    pub fn resume_at(&mut self, secs: f64) {
        self.start = secs;
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

    /// Word from the decoder: a frame to send, the end (it plays once, then
    /// the app closes the view), or why it can't play — `Err` closes the
    /// view too.
    pub fn on_event(&mut self, event: VideoEvent) -> Result<(), String> {
        match event {
            VideoEvent::Frame(_) => {
                self.pending = true;
                Ok(())
            }
            VideoEvent::Ended(_) => {
                self.player = None;
                self.done = true;
                Ok(())
            }
            VideoEvent::Failed(_, why) => Err(why),
        }
    }

    /// It played to the end.
    pub fn finished(&self) -> bool {
        self.done
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
        let pixels = ((f64::from(cols) * fw) as u32, (f64::from(rows) * fh) as u32);
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

        let needs_restart = self.path.is_some()
            && !self.done
            && (self.player.is_none() || self.pixels != Some(pixels));
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
                TAG_VIEW,
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
        let file = frame_dir().join(format!("frame{TAG_VIEW}.rgb"));
        use base64::Engine;
        let path =
            base64::engine::general_purpose::STANDARD.encode(file.as_os_str().as_encoded_bytes());
        let (start, escape, end) = Parser::tmux_start_escape_end(self.tmux);
        let codes = [
            // The picture goes at its cell, whatever ratatui left under the
            // cursor.
            format!("\x1b[{};{}H", at.y + 1, at.x + 1),
            format!(
                "{start}{escape}_Ga=T,t=f,f=32,s={w},v={h},i={KITTY_ID},q=2,c={},r={};{path}{escape}\\{end}",
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

/// The number kitty knows an inline player's frames by: beside the view's,
/// outside the range images take.
const INLINE_KITTY_ID: u32 = 0x7000_0001;

/// A video message playing where its thumbnail sits in the chat, as in
/// Telegram's apps. Space starts (and pauses) it, z blows it up to twice
/// the cells each way, pushing the messages around it apart; it stops once
/// its bubble isn't drawn — scrolled away, or the chat left — and plays
/// once, then the still is back. Enter grows it to the whole terminal
/// (see [`VideoView`]).
pub struct Inline {
    pub chat_id: i64,
    /// The message it's in.
    pub message_id: i64,
    pub note: Note,
    /// The downloaded file, once TDLib brings it.
    path: Option<PathBuf>,
    fps: f64,
    /// The square the decoder scales to, in pixels: the note's own size.
    side: u32,
    player: Option<Player>,
    tx: UnboundedSender<VideoEvent>,
    /// A frame waits to be sent to the terminal.
    pending: bool,
    /// z blew it up to twice the thumbnail's cells each way.
    medium: bool,
    /// The bubble's picture cells were drawn this frame; ui sets it, the
    /// app stops the player when a whole frame went by without it.
    pub seen: bool,
    /// The slot's size in cells, set by the ui when it draws the
    /// placeholder cells: kitty's virtual placement is made exactly that
    /// big (`c`/`r` on the transmit), or it guesses a size from the cells
    /// it has seen — a guess a partial redraw breaks, cutting the picture.
    pub placement: Option<(u16, u16)>,
    tmux: bool,
}

impl Inline {
    /// The video message of message `message_id`, `msg`, if it has one the
    /// terminal can play inline: it needs a thumbnail's cells to play in.
    pub fn of(
        chat_id: i64,
        message_id: i64,
        msg: &Msg,
        tx: UnboundedSender<VideoEvent>,
        tmux: bool,
    ) -> Option<Self> {
        let note = msg.video.clone()?;
        msg.preview.as_ref()?;
        Some(Self {
            chat_id,
            message_id,
            side: note.length,
            note,
            path: None,
            fps: 30.0,
            player: None,
            tx,
            pending: false,
            medium: false,
            seen: false,
            placement: None,
            tmux,
        })
    }

    /// It's this chat's this message's player.
    pub fn is_on(&self, chat_id: i64, message_id: i64) -> bool {
        self.chat_id == chat_id && self.message_id == message_id
    }

    /// The file Space was on.
    pub fn waits_for(&self, file_id: i32) -> bool {
        self.path.is_none() && self.note.file_id == file_id
    }

    /// The file is downloaded: played from the first frame, square as the
    /// note is.
    pub fn downloaded(&mut self, path: PathBuf) -> Result<(), String> {
        let (_, fps) = probe(&path);
        self.fps = fps;
        self.player = Some(
            spawn(
                &path,
                0.0,
                self.side,
                self.side,
                TAG_INLINE,
                self.tx.clone(),
            )
            .map_err(|e| format!("{e:#}"))?,
        );
        self.path = Some(path);
        Ok(())
    }

    /// The decoder is up: the bubble can show frames instead of the still.
    pub fn started(&self) -> bool {
        self.player.is_some()
    }

    /// Where it is, in seconds, by frames shown: where [`VideoView`] takes
    /// over on Enter.
    pub fn position(&self) -> f64 {
        self.player.as_ref().map_or(0, |p| p.frames.load(Relaxed)) as f64 / self.fps
    }

    /// Play or pause; after it played out, play it again from the start.
    pub fn toggle(&mut self) {
        match &mut self.player {
            Some(player) => player.toggle(),
            None => {
                if let Some(path) = &self.path {
                    self.player =
                        spawn(path, 0.0, self.side, self.side, TAG_INLINE, self.tx.clone()).ok();
                }
            }
        }
    }

    /// Twice as big in the chat, pushing the messages around it apart, or
    /// back to the thumbnail's size. The decoder is untouched: kitty scales
    /// the placement.
    pub fn toggle_zoom(&mut self) {
        self.medium = !self.medium;
    }

    /// It takes twice the thumbnail's cells each way.
    pub fn medium(&self) -> bool {
        self.medium
    }

    /// Message `message_id` of chat `chat_id`, now `msg`, no longer has the
    /// video message shown.
    pub fn lost(&self, chat_id: i64, message_id: i64, msg: Option<&Msg>) -> bool {
        chat_id == self.chat_id
            && message_id == self.message_id
            && msg.and_then(|m| m.video.as_ref()).map(|v| v.file_id) != Some(self.note.file_id)
    }

    /// Word from the decoder: a frame to send, or the end — it plays once,
    /// then the bubble shows the still again until Space starts it over.
    pub fn on_event(&mut self, event: VideoEvent) -> Result<(), String> {
        match event {
            VideoEvent::Frame(_) => {
                self.pending = true;
                Ok(())
            }
            VideoEvent::Ended(_) => {
                self.player = None;
                Ok(())
            }
            VideoEvent::Failed(_, why) => Err(why),
        }
    }

    /// Sends the newest frame to the terminal as a virtual placement sized
    /// to the slot's cells (`c`/`r`): kitty fits the frame into it, keeping
    /// the aspect. The bubble's placeholder cells (see [`InlineImage`]) say
    /// where it shows, so it follows the text as it scrolls. Called after
    /// each frame ratatui draws.
    pub fn send_frame(&mut self, terminal: &mut Terminal) {
        if !self.pending || self.player.is_none() {
            return;
        }
        let Some((c, r)) = self.placement else {
            return;
        };
        self.pending = false;
        let file = frame_dir().join(format!("frame{TAG_INLINE}.rgb"));
        use base64::Engine;
        let path =
            base64::engine::general_purpose::STANDARD.encode(file.as_os_str().as_encoded_bytes());
        let (start, escape, end) = Parser::tmux_start_escape_end(self.tmux);
        let (s, v) = (self.side, self.side);
        let code = format!(
            "{start}{escape}_Gq=2,i={INLINE_KITTY_ID},a=T,U=1,f=32,t=f,s={s},v={v},c={c},r={r};{path}{escape}\\{end}"
        );
        let _ = term::send(terminal, [code]);
    }
}

/// The video's frames go away with the player: kitty's image is deleted,
/// the decoder and the sound die with it.
impl Drop for Inline {
    fn drop(&mut self) {
        let (start, escape, end) = Parser::tmux_start_escape_end(self.tmux);
        let code = format!("{start}{escape}_Ga=d,d=I,i={INLINE_KITTY_ID},q=2{escape}\\{end}");
        use std::io::Write;
        let _ = std::io::stdout().write_all(code.as_bytes());
        let _ = std::io::stdout().flush();
    }
}

/// The placeholder cells kitty's protocol draws an inline player's frames
/// at: U+10EEEE with the image's number as the cell's foreground color, and
/// the row, column and the number's high byte as diacritics on each row's
/// first cell. Mirrored from ratatui-image's kitty backend, which can't be
/// used here: it transmits the image's bytes itself, while a playing
/// video's frames go as files ([`Inline::send_frame`]).
pub struct InlineImage {
    /// Rows scrolled off the top: the first visible row's number.
    pub skip: u16,
}

impl ratatui::widgets::Widget for InlineImage {
    fn render(self, area: Rect, buf: &mut ratatui::buffer::Buffer) {
        use ratatui::buffer::CellDiffOption;
        use std::num::NonZeroU16;
        let [id_extra, r, g, b] = INLINE_KITTY_ID.to_be_bytes();
        let fg = ratatui::style::Color::Rgb(r, g, b);
        let max = DIACRITICS.len() as u16;
        // Cells the diacritic table can't number are left alone rather than
        // shown at the wrong offset.
        let mut symbol = String::with_capacity(8);
        for y in 0..area.height.min(max) {
            let row = y + self.skip;
            if row >= max {
                break;
            }
            for x in 0..area.width.min(max) {
                let Some(cell) = buf.cell_mut((area.x + x, area.y + y)) else {
                    continue;
                };
                symbol.clear();
                symbol.push(PLACEHOLDER);
                if x == 0 {
                    symbol.push(diacritic(row));
                    symbol.push(diacritic(0));
                    symbol.push(diacritic(u16::from(id_extra)));
                }
                cell.set_symbol(&symbol)
                    .set_fg(fg)
                    .set_diff_option(CellDiffOption::ForcedWidth(NonZeroU16::new(1).unwrap()));
            }
        }
    }
}

/// The character kitty reserves for image placeholders.
const PLACEHOLDER: char = '\u{10EEEE}';

/// Row/column diacritics for kitty's unicode placeholders.
/// From https://sw.kovidgoyal.net/kitty/_downloads/1792bad15b12979994cd6ecc54c967a6/rowcolumn-diacritics.txt
/// See https://sw.kovidgoyal.net/kitty/graphics-protocol/#unicode-placeholders for further explanation.
static DIACRITICS: [char; 297] = [
    '\u{305}',
    '\u{30D}',
    '\u{30E}',
    '\u{310}',
    '\u{312}',
    '\u{33D}',
    '\u{33E}',
    '\u{33F}',
    '\u{346}',
    '\u{34A}',
    '\u{34B}',
    '\u{34C}',
    '\u{350}',
    '\u{351}',
    '\u{352}',
    '\u{357}',
    '\u{35B}',
    '\u{363}',
    '\u{364}',
    '\u{365}',
    '\u{366}',
    '\u{367}',
    '\u{368}',
    '\u{369}',
    '\u{36A}',
    '\u{36B}',
    '\u{36C}',
    '\u{36D}',
    '\u{36E}',
    '\u{36F}',
    '\u{483}',
    '\u{484}',
    '\u{485}',
    '\u{486}',
    '\u{487}',
    '\u{592}',
    '\u{593}',
    '\u{594}',
    '\u{595}',
    '\u{597}',
    '\u{598}',
    '\u{599}',
    '\u{59C}',
    '\u{59D}',
    '\u{59E}',
    '\u{59F}',
    '\u{5A0}',
    '\u{5A1}',
    '\u{5A8}',
    '\u{5A9}',
    '\u{5AB}',
    '\u{5AC}',
    '\u{5AF}',
    '\u{5C4}',
    '\u{610}',
    '\u{611}',
    '\u{612}',
    '\u{613}',
    '\u{614}',
    '\u{615}',
    '\u{616}',
    '\u{617}',
    '\u{657}',
    '\u{658}',
    '\u{659}',
    '\u{65A}',
    '\u{65B}',
    '\u{65D}',
    '\u{65E}',
    '\u{6D6}',
    '\u{6D7}',
    '\u{6D8}',
    '\u{6D9}',
    '\u{6DA}',
    '\u{6DB}',
    '\u{6DC}',
    '\u{6DF}',
    '\u{6E0}',
    '\u{6E1}',
    '\u{6E2}',
    '\u{6E4}',
    '\u{6E7}',
    '\u{6E8}',
    '\u{6EB}',
    '\u{6EC}',
    '\u{730}',
    '\u{732}',
    '\u{733}',
    '\u{735}',
    '\u{736}',
    '\u{73A}',
    '\u{73D}',
    '\u{73F}',
    '\u{740}',
    '\u{741}',
    '\u{743}',
    '\u{745}',
    '\u{747}',
    '\u{749}',
    '\u{74A}',
    '\u{7EB}',
    '\u{7EC}',
    '\u{7ED}',
    '\u{7EE}',
    '\u{7EF}',
    '\u{7F0}',
    '\u{7F1}',
    '\u{7F3}',
    '\u{816}',
    '\u{817}',
    '\u{818}',
    '\u{819}',
    '\u{81B}',
    '\u{81C}',
    '\u{81D}',
    '\u{81E}',
    '\u{81F}',
    '\u{820}',
    '\u{821}',
    '\u{822}',
    '\u{823}',
    '\u{825}',
    '\u{826}',
    '\u{827}',
    '\u{829}',
    '\u{82A}',
    '\u{82B}',
    '\u{82C}',
    '\u{82D}',
    '\u{951}',
    '\u{953}',
    '\u{954}',
    '\u{F82}',
    '\u{F83}',
    '\u{F86}',
    '\u{F87}',
    '\u{135D}',
    '\u{135E}',
    '\u{135F}',
    '\u{17DD}',
    '\u{193A}',
    '\u{1A17}',
    '\u{1A75}',
    '\u{1A76}',
    '\u{1A77}',
    '\u{1A78}',
    '\u{1A79}',
    '\u{1A7A}',
    '\u{1A7B}',
    '\u{1A7C}',
    '\u{1B6B}',
    '\u{1B6D}',
    '\u{1B6E}',
    '\u{1B6F}',
    '\u{1B70}',
    '\u{1B71}',
    '\u{1B72}',
    '\u{1B73}',
    '\u{1CD0}',
    '\u{1CD1}',
    '\u{1CD2}',
    '\u{1CDA}',
    '\u{1CDB}',
    '\u{1CE0}',
    '\u{1DC0}',
    '\u{1DC1}',
    '\u{1DC3}',
    '\u{1DC4}',
    '\u{1DC5}',
    '\u{1DC6}',
    '\u{1DC7}',
    '\u{1DC8}',
    '\u{1DC9}',
    '\u{1DCB}',
    '\u{1DCC}',
    '\u{1DD1}',
    '\u{1DD2}',
    '\u{1DD3}',
    '\u{1DD4}',
    '\u{1DD5}',
    '\u{1DD6}',
    '\u{1DD7}',
    '\u{1DD8}',
    '\u{1DD9}',
    '\u{1DDA}',
    '\u{1DDB}',
    '\u{1DDC}',
    '\u{1DDD}',
    '\u{1DDE}',
    '\u{1DDF}',
    '\u{1DE0}',
    '\u{1DE1}',
    '\u{1DE2}',
    '\u{1DE3}',
    '\u{1DE4}',
    '\u{1DE5}',
    '\u{1DE6}',
    '\u{1DFE}',
    '\u{20D0}',
    '\u{20D1}',
    '\u{20D4}',
    '\u{20D5}',
    '\u{20D6}',
    '\u{20D7}',
    '\u{20DB}',
    '\u{20DC}',
    '\u{20E1}',
    '\u{20E7}',
    '\u{20E9}',
    '\u{20F0}',
    '\u{2CEF}',
    '\u{2CF0}',
    '\u{2CF1}',
    '\u{2DE0}',
    '\u{2DE1}',
    '\u{2DE2}',
    '\u{2DE3}',
    '\u{2DE4}',
    '\u{2DE5}',
    '\u{2DE6}',
    '\u{2DE7}',
    '\u{2DE8}',
    '\u{2DE9}',
    '\u{2DEA}',
    '\u{2DEB}',
    '\u{2DEC}',
    '\u{2DED}',
    '\u{2DEE}',
    '\u{2DEF}',
    '\u{2DF0}',
    '\u{2DF1}',
    '\u{2DF2}',
    '\u{2DF3}',
    '\u{2DF4}',
    '\u{2DF5}',
    '\u{2DF6}',
    '\u{2DF7}',
    '\u{2DF8}',
    '\u{2DF9}',
    '\u{2DFA}',
    '\u{2DFB}',
    '\u{2DFC}',
    '\u{2DFD}',
    '\u{2DFE}',
    '\u{2DFF}',
    '\u{A66F}',
    '\u{A67C}',
    '\u{A67D}',
    '\u{A6F0}',
    '\u{A6F1}',
    '\u{A8E0}',
    '\u{A8E1}',
    '\u{A8E2}',
    '\u{A8E3}',
    '\u{A8E4}',
    '\u{A8E5}',
    '\u{A8E6}',
    '\u{A8E7}',
    '\u{A8E8}',
    '\u{A8E9}',
    '\u{A8EA}',
    '\u{A8EB}',
    '\u{A8EC}',
    '\u{A8ED}',
    '\u{A8EE}',
    '\u{A8EF}',
    '\u{A8F0}',
    '\u{A8F1}',
    '\u{AAB0}',
    '\u{AAB2}',
    '\u{AAB3}',
    '\u{AAB7}',
    '\u{AAB8}',
    '\u{AABE}',
    '\u{AABF}',
    '\u{AAC1}',
    '\u{FE20}',
    '\u{FE21}',
    '\u{FE22}',
    '\u{FE23}',
    '\u{FE24}',
    '\u{FE25}',
    '\u{FE26}',
    '\u{10A0F}',
    '\u{10A38}',
    '\u{1D185}',
    '\u{1D186}',
    '\u{1D187}',
    '\u{1D188}',
    '\u{1D189}',
    '\u{1D1AA}',
    '\u{1D1AB}',
    '\u{1D1AC}',
    '\u{1D1AD}',
    '\u{1D242}',
    '\u{1D243}',
    '\u{1D244}',
];

#[inline]
fn diacritic(y: u16) -> char {
    *DIACRITICS
        .get(usize::from(y))
        .unwrap_or_else(|| &DIACRITICS[0])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_mask_leaves_the_circle_and_hides_the_corners() {
        let (w, h) = (16, 16);
        let mut buf = vec![255u8; w * h * 4];
        mask_circle(&mut buf, w as u32, h as u32);
        let alpha = |x: usize, y: usize| buf[(y * w + x) * 4 + 3];
        assert_eq!(alpha(0, 0), 0, "the corner");
        assert_eq!(alpha(15, 15), 0, "the other corner");
        assert_eq!(alpha(8, 8), 255, "the center");
        assert_eq!(alpha(8, 0), 255, "the circle's top");
        // The edge is a shade, not a step.
        let edge = (1..15).map(|x| alpha(x, 1)).find(|&a| a > 0 && a < 255);
        assert!(edge.is_some(), "a half-covered pixel on the edge");
    }
}

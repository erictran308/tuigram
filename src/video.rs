//! Video messages played in the terminal itself. In Pepper, Pepper's own
//! player draws the video straight into a box of cells and plays the sound
//! (`crate::pepper`). Elsewhere ffmpeg decodes frames to raw RGBA in a folder
//! of the player's own, at the video's own pace, the circle outside the round
//! frame goes transparent, kitty's protocol puts each where the overlay's
//! blank area is, and mpv plays the sound; and where pepper-play can't play
//! one, it plays that way too. Zooming restarts the decoder at the same
//! position with a bigger frame. Everything but the few calls app.rs makes
//! lives here.

use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering::Relaxed};
use std::thread::JoinHandle;

use anyhow::{Context, Result};
use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::text::Span;
use ratatui::widgets::Paragraph;
use ratatui_image::FontSize;
use ratatui_image::picker::cap_parser::Parser;
use tdlib_rs::types::VideoNote;
use tokio::sync::mpsc::UnboundedSender;

use crate::messages::{MediaFile, Msg};
use crate::term::{self, Terminal};
use crate::theme::Colors;
use crate::viewer::SIZES;

/// The biggest video message played in the terminal: they top out near a
/// minute, a few MB, so this leaves room to spare.
pub const MAX_FILE: u64 = 256 << 20;

/// Frames a second the decoder puts out, whatever the video's own rate, so
/// the frames shown tell where it is.
const FPS: f64 = 30.0;

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

/// Word from a decoder thread: the player it serves ([`TAG_VIEW`] or
/// [`TAG_INLINE`], which share the channel) and which run of it, since one
/// restarted at another size can leave the last run's word on the way.
pub struct VideoEvent {
    pub tag: u8,
    run: u64,
    pub news: News,
}

pub enum News {
    /// A new frame waits in the player's frame file.
    Frame,
    /// The video played out.
    Ended,
    Failed(String),
}

/// Tag of the fullscreen player's decoder.
pub const TAG_VIEW: u8 = 0;
/// Tag of the inline player's decoder.
pub const TAG_INLINE: u8 = 1;

/// Numbers every decoder run: see [`VideoEvent`].
static RUNS: AtomicU64 = AtomicU64::new(0);

/// The number kitty knows a video's frames by: outside the range images
/// take, so one's deletions never touch the other's.
const KITTY_ID: u32 = 0x7000_0000;

/// Where players make their folders: tmpfs, so kitty reads frames at memory
/// speed; the system temp dir where there is no /dev/shm (macOS, Windows).
fn frames_parent() -> PathBuf {
    let shm = PathBuf::from("/dev/shm");
    if shm.is_dir() {
        shm
    } else {
        std::env::temp_dir()
    }
}

/// One run of a video: as kitty frames this program sends, or through
/// Pepper's own player.
enum Run {
    Frames(Player),
    #[cfg(unix)]
    Pepper(PepperPlayer),
}

impl Run {
    fn run(&self) -> u64 {
        match self {
            Run::Frames(p) => p.run,
            #[cfg(unix)]
            Run::Pepper(p) => p.run,
        }
    }

    fn paused(&self) -> bool {
        match self {
            Run::Frames(p) => p.paused,
            #[cfg(unix)]
            Run::Pepper(p) => p.paused,
        }
    }

    fn toggle(&mut self) {
        match self {
            Run::Frames(p) => p.toggle(),
            #[cfg(unix)]
            Run::Pepper(p) => p.toggle(),
        }
    }

    /// Where it is, in seconds, for a run that started `start` in.
    fn position(&self, start: f64) -> f64 {
        match self {
            Run::Frames(p) => start + p.frames.load(Relaxed) as f64 / FPS,
            #[cfg(unix)]
            Run::Pepper(p) => f64::from_bits(p.seconds.load(Relaxed)),
        }
    }

    fn frames(&self) -> Option<&Player> {
        match self {
            Run::Frames(p) => Some(p),
            #[cfg(unix)]
            Run::Pepper(_) => None,
        }
    }
}

/// Pepper's player, pepper-play, on one video, driven over a socket pair
/// only it and tuigram hold (its fd 3). It quits once the video played or
/// the socket closes.
#[cfg(unix)]
struct PepperPlayer {
    child: Child,
    control: std::os::unix::net::UnixStream,
    /// The thread reading what pepper-play says.
    reader: Option<JoinHandle<()>>,
    /// The box's token: see [`crate::pepper`].
    token: String,
    /// Seconds into the video, as pepper-play last said, as `f64` bits.
    seconds: Arc<AtomicU64>,
    /// Where the box was last placed; `None` while it's taken away.
    placed: Option<Rect>,
    run: u64,
    paused: bool,
}

#[cfg(unix)]
impl PepperPlayer {
    /// Sends mpv's JSON IPC `command`.
    fn send(&mut self, command: serde_json::Value) {
        use std::io::Write;
        let line = serde_json::json!({ "command": command }).to_string();
        let _ = writeln!(self.control, "{line}");
    }

    fn toggle(&mut self) {
        self.paused = !self.paused;
        let pause = self.paused;
        self.send(serde_json::json!(["set_property", "pause", pause]));
    }

    /// Puts the picture over `area`, or takes it away for `None`: only when
    /// that changed, as each place is drawn again.
    fn place(&mut self, terminal: &mut Terminal, area: Option<Rect>) {
        if self.placed == area {
            return;
        }
        let code = match area {
            Some(area) => crate::pepper::place(&self.token, area),
            None => crate::pepper::remove(&self.token),
        };
        let _ = term::send(terminal, [code]);
        self.placed = area;
    }
}

#[cfg(unix)]
impl Drop for PepperPlayer {
    fn drop(&mut self) {
        // Its end of the socket closing ends pepper-play; killed too, in case
        // it hangs, and reaped.
        let _ = self.control.shutdown(std::net::Shutdown::Both);
        let _ = self.child.kill();
        let _ = self.child.wait();
        if let Some(reader) = self.reader.take() {
            let _ = reader.join();
        }
        if self.placed.is_some() {
            use std::io::Write;
            let _ = std::io::stdout().write_all(crate::pepper::remove(&self.token).as_bytes());
            let _ = std::io::stdout().flush();
        }
    }
}

/// Kitty frames: ffmpeg feeding frames into a folder of the player's own,
/// mpv on the sound. Both pause and resume together (SIGSTOP/SIGCONT), and
/// die with the player, the folder with them.
struct Player {
    ffmpeg: Child,
    audio: Option<Child>,
    /// The thread copying ffmpeg's frames into `dir`.
    pump: Option<JoinHandle<()>>,
    /// Made new for this run and only the user's
    /// ([`crate::config::new_private_dir`]): someone else's frames folder in
    /// a shared /dev/shm could read the frames or plant links to write through.
    dir: PathBuf,
    run: u64,
    /// Frames written to the file this run.
    frames: Arc<AtomicU64>,
    stop: Arc<AtomicBool>,
    paused: bool,
}

impl Player {
    /// Where the newest frame waits for the terminal to read it.
    fn frame_file(&self) -> PathBuf {
        self.dir.join("frame.rgb")
    }

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
        // The pump ends with ffmpeg's pipe; after it nothing writes there.
        if let Some(pump) = self.pump.take() {
            let _ = pump.join();
        }
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

/// Reads frames from `pipe` (width × height × 4 bytes each, RGBA) into
/// `dir`'s frame file, atomically: a half-written one never reaches the
/// terminal. At the video's own pace, since ffmpeg runs with `-re`. The
/// pixels outside the frame's circle go transparent: video messages are round.
#[allow(clippy::too_many_arguments)]
fn pump_frames(
    mut pipe: impl Read,
    width: u32,
    height: u32,
    dir: &Path,
    frames: &AtomicU64,
    stop: &AtomicBool,
    (tag, run): (u8, u64),
    tx: &UnboundedSender<VideoEvent>,
) {
    let send = |news| tx.send(VideoEvent { tag, run, news }).is_ok();
    let size = width as usize * height as usize * 4;
    let tmp = dir.join("frame.tmp");
    let out = dir.join("frame.rgb");
    let mut buf = vec![0u8; size];
    loop {
        if stop.load(Relaxed) {
            return;
        }
        if let Err(e) = pipe.read_exact(&mut buf) {
            if !stop.load(Relaxed) {
                send(match e.kind() {
                    std::io::ErrorKind::UnexpectedEof => News::Ended,
                    _ => News::Failed(format!("decoding broke: {e}")),
                });
            }
            return;
        }
        mask_circle(&mut buf, width, height);
        let written = std::fs::write(&tmp, &buf).and_then(|()| std::fs::rename(&tmp, &out));
        if let Err(e) = written {
            send(News::Failed(format!(
                "the frame file can't be written: {e}"
            )));
            return;
        }
        frames.fetch_add(1, Relaxed);
        if !send(News::Frame) {
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

/// ffmpeg's arguments to decode `path` from `start` seconds through
/// `filter`. The file is a sender's: it's read only as the MP4 a video
/// message is, from the file alone, since one that's really a playlist
/// would have older ffmpegs fetch what it lists.
fn ffmpeg_args(path: &Path, start: &str, filter: &str) -> Vec<String> {
    let input = format!("file:{}", path.to_string_lossy());
    [
        "-v",
        "error",
        "-nostdin",
        "-re",
        "-protocol_whitelist",
        "file",
        "-f",
        "mov",
        "-ss",
        start,
        "-i",
        &input,
        "-an",
        "-vf",
        filter,
        "-f",
        "rawvideo",
        "-pix_fmt",
        "rgba",
        "pipe:1",
    ]
    .map(String::from)
    .to_vec()
}

/// mpv's options for a sender's file, in mpv or pepper-play: read only as
/// the MP4 a video message is, from the file alone (mpv takes a playlist for
/// one by its contents and plays what it lists), with no config, scripts,
/// yt-dlp or output on the terminal.
const STRICT: [&str; 7] = [
    "--no-config",
    "--load-scripts=no",
    "--terminal=no",
    "--ytdl=no",
    "--demuxer=lavf",
    "--demuxer-lavf-format=mov",
    "--demuxer-lavf-o=protocol_whitelist=file",
];

/// mpv's arguments to play `path`'s sound from `start` seconds.
fn mpv_args(path: &Path, start: &str) -> Vec<String> {
    let mut args: Vec<String> = STRICT.map(String::from).into();
    args.extend(["--no-video".into(), format!("--start={start}")]);
    args.extend(["--".into(), path.to_string_lossy().into_owned()]);
    args
}

/// pepper-play's arguments to play `path` from `start` seconds, driven over
/// the socket it gets as fd 3.
#[cfg(unix)]
fn pepper_args(path: &Path, start: &str) -> Vec<String> {
    let mut args: Vec<String> = STRICT.map(String::from).into();
    args.extend([
        "--keep-open=no".into(),
        "--input-ipc-client=fd://3".into(),
        format!("--start={start}"),
    ]);
    args.extend(["--".into(), path.to_string_lossy().into_owned()]);
    args
}

/// Starts playing `path` from `start` seconds: through Pepper's player
/// `pepper` where there's one and it starts, else as kitty frames scaled
/// into width × height pixels.
fn spawn(
    path: &Path,
    start: f64,
    (width, height): (u32, u32),
    tag: u8,
    tx: UnboundedSender<VideoEvent>,
    pepper: Option<&Path>,
) -> Result<Run> {
    #[cfg(unix)]
    if let Some(play) = pepper
        && let Ok(player) = spawn_pepper(play, path, start, tag, tx.clone())
    {
        return Ok(Run::Pepper(player));
    }
    #[cfg(not(unix))]
    let _ = pepper;
    spawn_frames(path, start, width, height, tag, tx).map(Run::Frames)
}

/// Starts pepper-play on `path` from `start` seconds. Its words come back
/// as decoder words ([`read_pepper`]), so the app can't tell the two apart.
#[cfg(unix)]
fn spawn_pepper(
    play: &Path,
    path: &Path,
    start: f64,
    tag: u8,
    tx: UnboundedSender<VideoEvent>,
) -> Result<PepperPlayer> {
    use std::os::fd::AsRawFd;
    use std::os::unix::process::CommandExt;
    let token = crate::pepper::new_token().context("no randomness for Pepper's token")?;
    let (ours, theirs) = std::os::unix::net::UnixStream::pair()?;
    let reading = ours.try_clone()?;
    let fd = theirs.as_raw_fd();
    let mut command = Command::new(play);
    command
        .args(pepper_args(path, &format!("{start:.3}")))
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    // SAFETY: only dup2 or fcntl runs between fork and exec, both
    // async-signal-safe. dup2 also clears close-on-exec on fd 3.
    unsafe {
        command.pre_exec(move || {
            let ok = if fd == 3 {
                libc::fcntl(3, libc::F_SETFD, 0) != -1
            } else {
                libc::dup2(fd, 3) != -1
            };
            if ok {
                Ok(())
            } else {
                Err(std::io::Error::last_os_error())
            }
        });
    }
    let child = command.spawn().context("pepper-play can't start")?;
    drop(theirs);
    let run = RUNS.fetch_add(1, Relaxed);
    let seconds = Arc::new(AtomicU64::new(start.to_bits()));
    let reader = {
        let seconds = Arc::clone(&seconds);
        std::thread::spawn(move || read_pepper(reading, &seconds, (tag, run), &tx))
    };
    let mut player = PepperPlayer {
        child,
        control: ours,
        reader: Some(reader),
        token,
        seconds,
        placed: None,
        run,
        paused: false,
    };
    let token = player.token.clone();
    player.send(serde_json::json!([
        "script-message-to",
        "main",
        "pepper-token",
        token
    ]));
    player.send(serde_json::json!(["observe_property", 1, "time-pos"]));
    Ok(player)
}

/// What pepper-play said, of what's heard.
#[cfg(unix)]
#[derive(Debug, PartialEq)]
enum Said {
    /// The picture started (mpv's `playback-restart`).
    Started,
    /// Seconds into the video.
    At(f64),
    Ended,
    Failed(String),
}

/// One line of mpv's JSON IPC from pepper-play, read as [`Said`].
#[cfg(unix)]
fn said(line: &str) -> Option<Said> {
    let said: serde_json::Value = serde_json::from_str(line).ok()?;
    match said.get("event")?.as_str()? {
        "playback-restart" => Some(Said::Started),
        "property-change" if said.get("name")? == "time-pos" => {
            said.get("data")?.as_f64().map(Said::At)
        }
        "end-file" if said.get("reason")? == "error" => {
            let why = said.get("file_error").and_then(|e| e.as_str());
            Some(Said::Failed(crate::text::clean(
                why.unwrap_or("it can't be played"),
            )))
        }
        "end-file" => Some(Said::Ended),
        _ => None,
    }
}

/// Reads what pepper-play says on `stream` until it closes, keeping where it
/// is in `seconds` and sending the rest on as decoder words: a frame once
/// the picture started, the end, or why it couldn't play.
#[cfg(unix)]
fn read_pepper(
    stream: impl Read,
    seconds: &AtomicU64,
    (tag, run): (u8, u64),
    tx: &UnboundedSender<VideoEvent>,
) {
    use std::io::BufRead;
    let send = |news| tx.send(VideoEvent { tag, run, news }).is_ok();
    let mut started = false;
    for line in std::io::BufReader::new(stream).lines() {
        let Ok(line) = line else {
            break;
        };
        match said(&line) {
            Some(Said::Started) if !started => {
                started = true;
                if !send(News::Frame) {
                    return;
                }
            }
            Some(Said::At(at)) => seconds.store(at.to_bits(), Relaxed),
            Some(Said::Ended) => {
                send(News::Ended);
                return;
            }
            Some(Said::Failed(why)) => {
                send(News::Failed(why));
                return;
            }
            _ => {}
        }
    }
    // The socket closed with no end said: pepper-play quit, or was stopped.
    send(if started {
        News::Ended
    } else {
        News::Failed("pepper-play stopped".into())
    });
}

/// Starts decoding `path` from `start` seconds, scaled into width × height
/// pixels, with the sound in mpv. No audio rather than no video when mpv
/// is missing.
fn spawn_frames(
    path: &Path,
    start: f64,
    width: u32,
    height: u32,
    tag: u8,
    tx: UnboundedSender<VideoEvent>,
) -> Result<Player> {
    let dir = crate::config::new_private_dir(&frames_parent(), "tuigram-video")?;
    // Fit into the box, padded to it: the frame's size never changes, so
    // the reader can count bytes.
    let filter = format!(
        "fps={FPS},scale={width}:{height}:force_original_aspect_ratio=decrease,\
         pad={width}:{height}:(ow-iw)/2:(oh-ih)/2"
    );
    let start = format!("{start:.3}");
    let ffmpeg = Command::new("ffmpeg")
        .args(ffmpeg_args(path, &start, &filter))
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn();
    let ffmpeg = match ffmpeg {
        Ok(ffmpeg) => ffmpeg,
        Err(e) => {
            let _ = std::fs::remove_dir_all(&dir);
            return Err(e).context("ffmpeg isn't installed");
        }
    };
    // From here on, dropping the player cleans up after it.
    let mut player = Player {
        ffmpeg,
        audio: None,
        pump: None,
        dir,
        run: RUNS.fetch_add(1, Relaxed),
        frames: Arc::new(AtomicU64::new(0)),
        stop: Arc::new(AtomicBool::new(false)),
        paused: false,
    };
    let pipe = player
        .ffmpeg
        .stdout
        .take()
        .context("ffmpeg's frames are piped")?;
    player.audio = Command::new("mpv")
        .args(mpv_args(path, &start))
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .ok();
    let (frames, stop) = (Arc::clone(&player.frames), Arc::clone(&player.stop));
    let (dir, run) = (player.dir.clone(), player.run);
    player.pump = Some(std::thread::spawn(move || {
        pump_frames(pipe, width, height, &dir, &frames, &stop, (tag, run), &tx);
    }));
    Ok(player)
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
    /// Why `o` doesn't hand the file to another app: Telegram says it
    /// can't be saved.
    pub cant_copy: Option<&'static str>,
    /// Playing it is to tell its sender, once a frame of it shows: see
    /// [`VideoView::tell_now`].
    pub tell: bool,
    /// The downloaded file, once TDLib brings it.
    path: Option<PathBuf>,
    player: Option<Run>,
    /// Pepper's player, while it's the one to try: see [`crate::pepper`].
    pepper: Option<PathBuf>,
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
    /// A frame of it came from the decoder.
    framed: bool,
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
        pepper: Option<PathBuf>,
    ) -> Option<Self> {
        let note = msg.video.clone()?;
        let file = msg.file.clone().filter(|f| f.id == note.file_id)?;
        Some(Self {
            chat_id,
            message_id,
            file,
            note,
            cant_copy: crate::viewer::cant_copy(msg),
            tell: false,
            path: None,
            player: None,
            pepper,
            tx,
            start: 0.0,
            zoom: None,
            rect: None,
            pixels: None,
            pending: false,
            framed: false,
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
        self.path = Some(path);
    }

    /// True once, when a frame of it has come and playing it is to tell
    /// its sender: the app tells them then, as Telegram's apps do.
    pub fn tell_now(&mut self) -> bool {
        let now = self.tell && self.framed;
        if now {
            self.tell = false;
        }
        now
    }

    /// Where it is, in seconds.
    fn position(&self) -> f64 {
        self.player
            .as_ref()
            .map_or(self.start, |p| p.position(self.start))
    }

    /// Playing; paused; waiting for the file.
    pub fn status(&self) -> &'static str {
        match &self.player {
            None => "loading…",
            Some(p) if p.paused() => "paused",
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
    /// view too. A run stopped for another, at a new size, is no longer heard.
    pub fn on_event(&mut self, event: VideoEvent) -> Result<(), String> {
        if self.player.as_ref().is_none_or(|p| p.run() != event.run) {
            return Ok(());
        }
        match event.news {
            News::Frame => {
                self.pending = true;
                self.framed = true;
                Ok(())
            }
            News::Ended => {
                self.player = None;
                self.done = true;
                Ok(())
            }
            // pepper-play couldn't play it at all: kitty frames play it
            // instead, from the next frame drawn.
            News::Failed(_)
                if !self.framed && self.player.as_ref().is_some_and(|p| p.frames().is_none()) =>
            {
                self.pepper = None;
                self.player = None;
                Ok(())
            }
            News::Failed(why) => Err(why),
        }
    }

    /// It played to the end.
    pub fn finished(&self) -> bool {
        self.done
    }

    /// The picture's cells within `inner`, centered and square, as video
    /// messages are, at the zoom picked. Font cells are about twice as tall
    /// as wide: the pixel box accounts for it.
    fn layout(&self, inner: Rect, font: FontSize) -> (Rect, (u32, u32)) {
        let share = self.share();
        let (fw, fh) = (
            f64::from(font.width).max(1.0),
            f64::from(font.height).max(1.0),
        );
        let mut cols = f64::from(inner.width) * share;
        let mut rows = f64::from(inner.height) * share;
        if cols * fw > rows * fh {
            cols = rows * fh / fw;
        } else {
            rows = cols * fw / fh;
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
        frame.render_widget(crate::ui::Cover, area);
        frame.render_widget(Paragraph::new(Span::from(title)).block(block), area);
        frame.render_widget(crate::ui::Cover, at);

        // Pepper scales the picture to its box itself: only kitty frames are
        // made again at a new size.
        let needs_restart = self.path.is_some()
            && !self.done
            && self
                .player
                .as_ref()
                .is_none_or(|p| p.frames().is_some() && self.pixels != Some(pixels));
        if needs_restart && self.error.is_none() {
            let position = if self.player.is_some() {
                self.position()
            } else {
                self.start
            };
            let was_paused = self.player.as_ref().is_some_and(Run::paused);
            self.player = None;
            match spawn(
                &self.path.clone().unwrap(),
                position,
                pixels,
                TAG_VIEW,
                self.tx.clone(),
                self.pepper.as_deref(),
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
    /// has a place; in Pepper, places the box where the picture goes. Called
    /// after each frame ratatui draws, so the picture lands over the board's
    /// blank cells.
    pub fn send_frame(&mut self, terminal: &mut Terminal) {
        #[cfg(unix)]
        if let Some(Run::Pepper(player)) = &mut self.player {
            player.place(terminal, self.rect);
            return;
        }
        let Some(player) = self.player.as_ref().and_then(Run::frames) else {
            return;
        };
        let (Some(at), Some((w, h))) = (self.rect, self.pixels) else {
            return;
        };
        if !self.pending {
            return;
        }
        self.pending = false;
        let path = kitty_path(&player.frame_file());
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

/// A frame file's path as kitty's protocol takes it, in base64.
fn kitty_path(file: &Path) -> String {
    use base64::Engine;
    base64::engine::general_purpose::STANDARD.encode(file.as_os_str().as_encoded_bytes())
}

/// The video's frames go away with the view: kitty's image is deleted, the
/// decoder and the sound die with the player, its folder with them.
impl Drop for VideoView {
    fn drop(&mut self) {
        let (start, escape, end) = Parser::tmux_start_escape_end(self.tmux);
        let code = format!("{start}{escape}_Ga=d,d=I,i={KITTY_ID},q=2{escape}\\{end}");
        use std::io::Write;
        let _ = std::io::stdout().write_all(code.as_bytes());
        let _ = std::io::stdout().flush();
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
    /// Playing it is to tell its sender, once a frame of it shows: see
    /// [`Inline::tell_now`].
    pub tell: bool,
    /// The downloaded file, once TDLib brings it.
    path: Option<PathBuf>,
    /// The square the decoder scales to, in pixels: the note's own size.
    side: u32,
    player: Option<Run>,
    /// Pepper's player, while it's the one to try: see [`crate::pepper`].
    pepper: Option<PathBuf>,
    tx: UnboundedSender<VideoEvent>,
    /// A frame waits to be sent to the terminal.
    pending: bool,
    /// A frame of it came from the decoder.
    framed: bool,
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
    /// In Pepper, the cells the ui left blank for the picture this frame:
    /// `None` while a popup is over them, which Pepper's box would cover.
    pub area: Option<Rect>,
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
        pepper: Option<PathBuf>,
    ) -> Option<Self> {
        let note = msg.video.clone()?;
        msg.preview.as_ref()?;
        Some(Self {
            chat_id,
            message_id,
            side: note.length,
            note,
            tell: false,
            path: None,
            player: None,
            pepper,
            tx,
            pending: false,
            framed: false,
            medium: false,
            seen: false,
            placement: None,
            area: None,
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
        self.path = Some(path);
        self.start_over()
    }

    /// Plays it from the start.
    fn start_over(&mut self) -> Result<(), String> {
        let Some(path) = &self.path else {
            return Ok(());
        };
        let side = (self.side, self.side);
        let pepper = self.pepper.as_deref();
        let run = spawn(path, 0.0, side, TAG_INLINE, self.tx.clone(), pepper);
        self.player = Some(run.map_err(|e| format!("{e:#}"))?);
        self.framed = false;
        Ok(())
    }

    /// True once, when a frame of it has come and playing it is to tell
    /// its sender: the app tells them then, as Telegram's apps do.
    pub fn tell_now(&mut self) -> bool {
        let now = self.tell && self.framed;
        if now {
            self.tell = false;
        }
        now
    }

    /// The decoder is up: the bubble can show frames instead of the still.
    pub fn started(&self) -> bool {
        self.player.is_some()
    }

    /// The picture plays through Pepper's player, in a box over the cells
    /// the ui leaves blank, not in kitty's placeholder cells.
    pub fn in_pepper(&self) -> bool {
        self.player.as_ref().is_some_and(|p| p.frames().is_none())
    }

    /// Where it is, in seconds: where [`VideoView`] takes over on Enter.
    pub fn position(&self) -> f64 {
        self.player.as_ref().map_or(0.0, |p| p.position(0.0))
    }

    /// Play or pause; after it played out, play it again from the start.
    pub fn toggle(&mut self) {
        match &mut self.player {
            Some(player) => player.toggle(),
            None => {
                let _ = self.start_over();
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
    /// then the bubble shows the still again until Space starts it over. A
    /// run stopped for another is no longer heard.
    pub fn on_event(&mut self, event: VideoEvent) -> Result<(), String> {
        if self.player.as_ref().is_none_or(|p| p.run() != event.run) {
            return Ok(());
        }
        match event.news {
            News::Frame => {
                self.pending = true;
                self.framed = true;
                Ok(())
            }
            News::Ended => {
                self.player = None;
                Ok(())
            }
            // pepper-play couldn't play it at all: kitty frames play it
            // instead.
            News::Failed(_) if !self.framed && self.in_pepper() => {
                self.pepper = None;
                self.start_over()
            }
            News::Failed(why) => Err(why),
        }
    }

    /// Sends the newest frame to the terminal as a virtual placement sized
    /// to the slot's cells (`c`/`r`): kitty fits the frame into it, keeping
    /// the aspect. The bubble's placeholder cells (see [`InlineImage`]) say
    /// where it shows, so it follows the text as it scrolls. In Pepper, places
    /// the box over [`Inline::area`] instead. Called after each frame ratatui
    /// draws.
    pub fn send_frame(&mut self, terminal: &mut Terminal) {
        #[cfg(unix)]
        if let Some(Run::Pepper(player)) = &mut self.player {
            player.place(terminal, self.area);
            return;
        }
        let Some(player) = self.player.as_ref().and_then(Run::frames) else {
            return;
        };
        let Some((c, r)) = self.placement else {
            return;
        };
        if !self.pending {
            return;
        }
        self.pending = false;
        let path = kitty_path(&player.frame_file());
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
                symbol.push(crate::images::PLACEHOLDER);
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

    #[test]
    fn a_senders_file_is_read_only_as_a_local_mp4() {
        let path = Path::new("/files/video_notes/file_3.mp4");
        let args = ffmpeg_args(path, "0.000", "fps=30");
        let input = args.iter().position(|a| a == "-i").unwrap();
        let before = &args[..input];
        assert!(
            before
                .windows(2)
                .any(|w| w == ["-protocol_whitelist", "file"]),
            "{args:?}"
        );
        assert!(before.windows(2).any(|w| w == ["-f", "mov"]), "{args:?}");
        assert_eq!(args[input + 1], "file:/files/video_notes/file_3.mp4");

        let mut players = vec![mpv_args(path, "0.000")];
        #[cfg(unix)]
        players.push(pepper_args(path, "0.000"));
        for args in players {
            for wanted in [
                "--no-config",
                "--load-scripts=no",
                "--ytdl=no",
                "--demuxer=lavf",
                "--demuxer-lavf-format=mov",
                "--demuxer-lavf-o=protocol_whitelist=file",
            ] {
                assert!(args.iter().any(|a| a == wanted), "{wanted}: {args:?}");
            }
            assert_eq!(
                args[args.len() - 2..],
                ["--", "/files/video_notes/file_3.mp4"]
            );
        }
    }

    #[test]
    fn frames_go_into_the_players_own_folder_tagged_with_its_run() {
        let dir = crate::config::new_private_dir(&std::env::temp_dir(), "tuigram-test").unwrap();
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
        // Two frames of 2 × 2 pixels, then the end.
        let pipe = [255u8; 2 * 2 * 4 * 2];
        let stop = AtomicBool::new(false);
        pump_frames(
            &pipe[..],
            2,
            2,
            &dir,
            &AtomicU64::new(0),
            &stop,
            (TAG_INLINE, 7),
            &tx,
        );
        let events: Vec<VideoEvent> = std::iter::from_fn(|| rx.try_recv().ok()).collect();
        assert!(events.iter().all(|e| e.tag == TAG_INLINE && e.run == 7));
        let news: Vec<&News> = events.iter().map(|e| &e.news).collect();
        assert!(matches!(news[..], [News::Frame, News::Frame, News::Ended]));
        assert_eq!(std::fs::read(dir.join("frame.rgb")).unwrap().len(), 16);
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn what_pepper_play_says_is_read_as_decoder_words() {
        // As pepper-play 0.4.0 said it, playing a clip.
        let lines = [
            r#"{"data": null, "request_id": 0, "error": "success"}"#,
            r#"{"event": "start-file", "playlist_entry_id": 1}"#,
            r#"{"event": "property-change", "id": 1, "name": "time-pos", "data": null}"#,
            r#"{"event": "file-loaded"}"#,
            r#"{"event": "property-change", "id": 1, "name": "time-pos", "data": 0.5}"#,
            r#"{"event": "playback-restart"}"#,
            r#"{"event": "property-change", "id": 1, "name": "time-pos", "data": 1.92}"#,
            r#"{"event": "playback-restart"}"#,
            r#"{"event": "end-file", "reason": "eof", "playlist_entry_id": 1}"#,
        ];
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
        let seconds = AtomicU64::new(0);
        read_pepper(lines.join("\n").as_bytes(), &seconds, (TAG_VIEW, 3), &tx);
        let events: Vec<VideoEvent> = std::iter::from_fn(|| rx.try_recv().ok()).collect();
        assert!(events.iter().all(|e| e.tag == TAG_VIEW && e.run == 3));
        let news: Vec<&News> = events.iter().map(|e| &e.news).collect();
        assert!(matches!(news[..], [News::Frame, News::Ended]), "one start");
        assert_eq!(f64::from_bits(seconds.load(Relaxed)), 1.92);

        let failed = r#"{"event": "end-file", "reason": "error", "file_error": "loading failed"}"#;
        assert_eq!(said(failed), Some(Said::Failed("loading failed".into())));
        // It quit before the picture started: it couldn't play it.
        read_pepper(&b""[..], &seconds, (TAG_VIEW, 4), &tx);
        assert!(matches!(rx.try_recv().unwrap().news, News::Failed(_)));
    }

    #[cfg(unix)]
    #[test]
    fn a_video_message_pepper_play_cant_play_plays_as_kitty_frames() {
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
        let (_, mut msg) = crate::messages::tests::page([1]).remove(0);
        msg.file = Some(MediaFile {
            id: 5,
            label: "Video message".into(),
            photo: false,
            downloaded: true,
        });
        msg.video = Some(Note {
            file_id: 5,
            size: 10,
            duration: 1,
            length: 240,
        });
        // A pepper-play that quits at once, as one without libmpv does.
        let quits = PathBuf::from("/bin/sh");
        let mut view = VideoView::of(1, 1, &msg, tx.clone(), false, Some(quits)).unwrap();
        let path = Path::new("/nowhere/file_5.mp4");
        let run = spawn(path, 0.0, (8, 8), TAG_VIEW, tx, view.pepper.as_deref()).unwrap();
        assert!(run.frames().is_none(), "Pepper's player first");
        view.player = Some(run);
        let event = rx.blocking_recv().unwrap();
        assert!(view.on_event(event).is_ok(), "not an error to show");
        assert!(
            view.player.is_none() && view.pepper.is_none(),
            "kitty frames next"
        );
    }
}

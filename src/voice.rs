//! Voice messages: the waveform their bubble draws, and playing them in
//! tuigram itself. Telegram's are Opus in an Ogg file, decoded here in Rust
//! on a thread of their own; the file is never handed to another app, so
//! one that self-destructs once played can be played too.

use std::fs::File;
use std::io::{BufReader, Read};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering::Relaxed};
use std::time::{Duration, Instant};

use anyhow::{Context, Result, bail};
use tdlib_rs::types::VoiceNote;
use tokio::sync::mpsc::UnboundedSender;

use crate::opus::OpusDecoder;
use crate::sound::{self, Sink};

/// Samples a second. Opus decodes at this rate, and ALSA or the resampler
/// takes it to the device's.
pub const RATE: u32 = 48_000;

/// The longest file played. An hour of voice is a few megabytes.
pub const MAX_FILE: u64 = 64 << 20;

/// Bytes of a waveform read. Telegram's apps send 63, 100 levels.
const MAX_WAVEFORM: usize = 256;

/// How often the playing thread looks for a pause or a stop.
const POLL: Duration = Duration::from_millis(20);

/// Playback fails if nothing more is heard for this long, so a device that
/// stalls can't leave it playing forever.
const STALLED: Duration = Duration::from_secs(3);

/// How often the screen follows the message playing.
const TICK: Duration = Duration::from_millis(200);

/// A voice message, as its bubble shows it.
#[derive(Clone, Debug, PartialEq)]
pub struct Voice {
    pub file_id: i32,
    /// Bytes, as TDLib knows them before downloading; 0 if it doesn't.
    pub size: u64,
    /// Seconds, as the sender's app counted them.
    pub duration: i32,
    /// Its loudness along the way, each 0–31.
    pub levels: Vec<u8>,
    /// Opus in an Ogg file, which tuigram plays. Telegram also takes MP3
    /// and M4A, which open in another app, as other audio does.
    pub ogg: bool,
    /// Played: by you, or for your own, by someone it was sent to.
    pub listened: bool,
}

impl Voice {
    pub fn new(note: &VoiceNote, listened: bool) -> Self {
        let mime = note.mime_type.to_ascii_lowercase();
        let size = note.voice.size.max(note.voice.expected_size);
        Self {
            file_id: note.voice.id,
            size: u64::try_from(size).unwrap_or(0),
            duration: note.duration.max(0),
            levels: levels(&note.waveform),
            ogg: mime.is_empty() || mime.starts_with("audio/ogg") || mime.starts_with("audio/opus"),
            listened,
        }
    }

    /// Columns its waveform takes: a longer message gets more, as in
    /// Telegram.
    pub fn columns(&self) -> usize {
        (self.duration as usize).saturating_add(10).clamp(16, 40)
    }
}

/// The waveform TDLib sends (base64) as levels: 5 bits each, packed lowest
/// bits first, as Telegram's apps read them.
fn levels(waveform: &str) -> Vec<u8> {
    use base64::Engine;
    let Ok(bytes) = base64::engine::general_purpose::STANDARD.decode(waveform) else {
        return Vec::new();
    };
    unpack(&bytes[..bytes.len().min(MAX_WAVEFORM)])
}

fn unpack(bytes: &[u8]) -> Vec<u8> {
    (0..bytes.len() * 8 / 5)
        .map(|i| {
            let (byte, shift) = (i * 5 / 8, i * 5 % 8);
            let next = bytes.get(byte + 1).copied().unwrap_or(0);
            let pair = u16::from(bytes[byte]) | u16::from(next) << 8;
            (pair >> shift & 0x1f) as u8
        })
        .collect()
}

/// The waveform `columns` wide, from ▁ to █: each column the loudest of the
/// levels under it, against the loudest of all.
pub fn bars(levels: &[u8], columns: usize) -> String {
    const BARS: [char; 8] = ['▁', '▂', '▃', '▄', '▅', '▆', '▇', '█'];
    let loudest = usize::from(levels.iter().copied().max().unwrap_or(0).max(1));
    let n = levels.len();
    (0..columns)
        .map(|c| {
            let from = c * n / columns;
            let to = ((c + 1) * n / columns).clamp(from + 1, n.max(1));
            let level = usize::from(
                levels
                    .get(from..to)
                    .and_then(|l| l.iter().max())
                    .copied()
                    .unwrap_or(0),
            );
            BARS[(level * (BARS.len() - 1) + loudest / 2) / loudest]
        })
        .collect()
}

/// The message playing, paused or about to, as the screen shows it.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Playback {
    pub message_id: i64,
    /// How much has been heard.
    pub at: Duration,
    pub paused: bool,
}

/// Where the sound goes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Output {
    Speakers,
    /// Nowhere, at once: tests and the demo must make no sound.
    Nowhere,
}

/// Plays one voice message at a time.
pub struct Player {
    tx: UnboundedSender<VoiceEvent>,
    output: Output,
    now: Option<Playing>,
    /// Numbers each playback, so word from one that was stopped is dropped.
    tickets: u64,
}

struct Playing {
    chat_id: i64,
    message_id: i64,
    file_id: i32,
    /// Its sender is to be told once it plays.
    tell: bool,
    ticket: u64,
    /// Shared with the thread playing it; `None` while the file downloads.
    shared: Option<Arc<Shared>>,
    paused: bool,
}

/// What the app and the thread playing tell each other.
#[derive(Default)]
struct Shared {
    stop: AtomicBool,
    paused: AtomicBool,
    /// Samples heard so far.
    played: AtomicU64,
}

/// Word from the thread playing a voice message.
#[derive(Debug)]
pub struct VoiceEvent {
    ticket: u64,
    happened: Happened,
}

/// A [`VoiceEvent`] about what's playing, and which message that is.
#[derive(Debug, PartialEq)]
pub struct News {
    pub chat_id: i64,
    pub message_id: i64,
    /// Its sender is to be told it was played, now that it is.
    pub tell: bool,
    pub happened: Happened,
}

#[derive(Debug, PartialEq)]
pub enum Happened {
    /// Sound is on its way out.
    Started,
    /// It played to the end.
    Ended,
    /// Why it couldn't play.
    Failed(String),
}

impl Player {
    pub fn new(tx: UnboundedSender<VoiceEvent>, output: Output) -> Self {
        Self {
            tx,
            output,
            now: None,
            tickets: 0,
        }
    }

    /// Plays message `message_id`'s voice once its file, `file_id`, is
    /// downloaded, stopping whatever was playing. `tell` is kept for when
    /// sound starts.
    pub fn play(&mut self, chat_id: i64, message_id: i64, file_id: i32, tell: bool) {
        self.stop();
        self.tickets += 1;
        self.now = Some(Playing {
            chat_id,
            message_id,
            file_id,
            tell,
            ticket: self.tickets,
            shared: None,
            paused: false,
        });
    }

    /// File `file_id` is the one waited for, to play.
    pub fn waits_for(&self, file_id: i32) -> bool {
        self.now
            .as_ref()
            .is_some_and(|n| n.file_id == file_id && n.shared.is_none())
    }

    /// Plays the downloaded file, at `path`, of what's waiting.
    pub fn start(&mut self, path: PathBuf) {
        let Some(now) = self.now.as_mut().filter(|n| n.shared.is_none()) else {
            return;
        };
        let shared = Arc::new(Shared::default());
        shared.paused.store(now.paused, Relaxed);
        now.shared = Some(Arc::clone(&shared));
        let (tx, ticket, output) = (self.tx.clone(), now.ticket, self.output);
        let failed = tx.clone();
        let spawned = std::thread::Builder::new()
            .name("voice".into())
            .spawn(move || {
                let send = |happened| {
                    let _ = tx.send(VoiceEvent { ticket, happened });
                };
                // Another user's file: a panic decoding it ends this
                // playback, not tuigram.
                let result = crate::images::contained(|| {
                    Ok(play(&path, &shared, output, &mut || {
                        send(Happened::Started)
                    }))
                });
                send(match result {
                    Ok(Ok(())) => Happened::Ended,
                    Ok(Err(e)) => Happened::Failed(format!("{e:#}")),
                    Err(_) => Happened::Failed("it couldn't be decoded".into()),
                });
            });
        if spawned.is_err() {
            let happened = Happened::Failed("there's no thread to play it on".into());
            let _ = failed.send(VoiceEvent { ticket, happened });
        }
    }

    /// Pauses what's playing, or plays on. Returns whether it's paused now.
    pub fn toggle(&mut self) -> bool {
        let Some(now) = self.now.as_mut() else {
            return false;
        };
        now.paused = !now.paused;
        if let Some(shared) = &now.shared {
            shared.paused.store(now.paused, Relaxed);
        }
        now.paused
    }

    pub fn stop(&mut self) {
        if let Some(shared) = self.now.take().and_then(|n| n.shared) {
            shared.stop.store(true, Relaxed);
        }
    }

    /// Message `message_id` of chat `chat_id` is playing, paused, or
    /// waiting for its file.
    pub fn is_on(&self, chat_id: i64, message_id: i64) -> bool {
        self.now
            .as_ref()
            .is_some_and(|n| n.chat_id == chat_id && n.message_id == message_id)
    }

    /// What's playing in chat `chat_id`, for the screen.
    pub fn playback(&self, chat_id: i64) -> Option<Playback> {
        let now = self.now.as_ref().filter(|n| n.chat_id == chat_id)?;
        let played = now.shared.as_ref().map_or(0, |s| s.played.load(Relaxed));
        Some(Playback {
            message_id: now.message_id,
            at: Duration::from_secs_f64(played as f64 / f64::from(RATE)),
            paused: now.paused,
        })
    }

    /// How soon the screen should follow the message playing.
    pub fn next_tick(&self) -> Option<Duration> {
        self.now
            .as_ref()
            .filter(|n| !n.paused && n.shared.is_some())
            .map(|_| TICK)
    }

    /// What `event` says, if it's about what's playing now. Once it ended
    /// or failed, nothing is.
    pub fn on_event(&mut self, event: VoiceEvent) -> Option<News> {
        let now = self.now.as_ref().filter(|n| n.ticket == event.ticket)?;
        let news = News {
            chat_id: now.chat_id,
            message_id: now.message_id,
            tell: now.tell,
            happened: event.happened,
        };
        if news.happened != Happened::Started {
            self.now = None;
        }
        Some(news)
    }

    /// An event about what's playing now, as its thread would send it.
    #[cfg(test)]
    pub fn event(&self, happened: Happened) -> VoiceEvent {
        let ticket = self.now.as_ref().map_or(0, |n| n.ticket);
        VoiceEvent { ticket, happened }
    }
}

impl Drop for Player {
    fn drop(&mut self) {
        self.stop();
    }
}

/// Plays the file at `path` until it ends or `shared` says stop. `started`
/// is called once sound is on its way.
fn play(path: &Path, shared: &Shared, output: Output, started: &mut dyn FnMut()) -> Result<()> {
    let file = File::open(path).context("the file can't be read")?;
    if file.metadata().map_or(true, |m| m.len() > MAX_FILE) {
        bail!("it's too long");
    }
    let mut sound = OggOpus::new(BufReader::new(file))?;
    let mut sink: Box<dyn Sink> = match output {
        Output::Speakers => sound::open()?,
        Output::Nowhere => Box::new(sound::Silent::default()),
    };
    let mut samples = Vec::new();
    let mut heard = Heard::new(sink.as_mut());
    // Decoded a little ahead of what's heard, until the file runs out.
    loop {
        if !heard.go_on(sink.as_mut(), shared, started)? {
            return Ok(());
        }
        if !sink.wants_more() {
            heard.check()?;
            std::thread::sleep(POLL);
            continue;
        }
        if !sound.next(&mut samples)? {
            break;
        }
        sink.write(&samples)?;
    }
    sink.finish();
    // Then what's queued plays out.
    loop {
        if !heard.go_on(sink.as_mut(), shared, started)? || sink.done()? {
            return Ok(());
        }
        heard.check()?;
        std::thread::sleep(POLL);
    }
}

/// What's been heard, which says when sound has really started and notices
/// a device that stops taking it.
struct Heard {
    played: u64,
    since: Instant,
    began: bool,
}

impl Heard {
    fn new(sink: &mut dyn Sink) -> Self {
        Self {
            played: sink.played(),
            since: Instant::now(),
            began: false,
        }
    }

    /// Waits out a pause and says whether to go on. `started` is called
    /// once something has been heard, not just queued: a device that never
    /// starts tells nobody it was played.
    fn go_on(
        &mut self,
        sink: &mut dyn Sink,
        shared: &Shared,
        started: &mut dyn FnMut(),
    ) -> Result<bool> {
        let flow = keep_going(sink, shared)?;
        let played = sink.played();
        if flow == Flow::Resumed || played != self.played {
            (self.played, self.since) = (played, Instant::now());
        }
        if !self.began && played > 0 {
            self.began = true;
            started();
        }
        Ok(flow != Flow::Stop)
    }

    /// Fails once nothing more has been heard for a while.
    fn check(&self) -> Result<()> {
        if self.since.elapsed() > STALLED {
            bail!("the sound device stopped playing");
        }
        Ok(())
    }
}

#[derive(Debug, PartialEq)]
enum Flow {
    Stop,
    On,
    /// It was paused, and plays again.
    Resumed,
}

/// Says how much has been heard, and waits out a pause.
fn keep_going(sink: &mut dyn Sink, shared: &Shared) -> Result<Flow> {
    shared.played.store(sink.played(), Relaxed);
    let stop = || shared.stop.load(Relaxed);
    if stop() {
        return Ok(Flow::Stop);
    }
    if !shared.paused.load(Relaxed) {
        return Ok(Flow::On);
    }
    sink.pause()?;
    shared.played.store(sink.played(), Relaxed);
    while shared.paused.load(Relaxed) && !stop() {
        std::thread::sleep(POLL);
    }
    if stop() {
        return Ok(Flow::Stop);
    }
    sink.resume()?;
    Ok(Flow::Resumed)
}

/// Bytes a packet may take. Opus's are a few hundred, a lot of padding
/// included; the comment header, which can be bigger, is skipped unread.
const MAX_PACKET: usize = 64 << 10;

/// A packet of an Ogg stream.
struct Packet {
    data: Vec<u8>,
    /// The sound ends with it: the last page's last packet. Its granule
    /// position says where, maybe partway through it.
    end: Option<u64>,
}

/// The packets of an Ogg file's first stream (RFC 3533), read page by page.
/// Pages of other streams are passed over, and a packet is never more than
/// [`MAX_PACKET`], so what a file holds can't make this hold much. Page
/// checksums aren't checked: someone making a file to break tuigram would
/// write right ones.
struct OggPackets<R: Read> {
    reader: R,
    /// The stream read, the first page's.
    serial: Option<u32>,
    /// The page read: its segments' lengths, how many were taken, its
    /// data and where the next segment starts in it.
    lacing: Vec<u8>,
    segment: usize,
    page: Vec<u8>,
    at: usize,
    /// The page's granule position, and whether it ends the stream.
    granule: u64,
    last_page: bool,
    /// The packet being put together, which may go on over pages.
    packet: Vec<u8>,
}

impl<R: Read> OggPackets<R> {
    fn new(reader: R) -> Self {
        Self {
            reader,
            serial: None,
            lacing: Vec::new(),
            segment: 0,
            page: Vec::new(),
            at: 0,
            granule: 0,
            last_page: false,
            packet: Vec::new(),
        }
    }

    /// The next packet, its data dropped unless `keep`; `None` at the end.
    fn next(&mut self, keep: bool) -> Result<Option<Packet>> {
        self.packet.clear();
        loop {
            if self.segment == self.lacing.len() {
                if !self.read_page()? {
                    return Ok(None);
                }
                continue;
            }
            let len = usize::from(self.lacing[self.segment]);
            self.segment += 1;
            let data = &self.page[self.at..self.at + len];
            self.at += len;
            if keep {
                if self.packet.len() + len > MAX_PACKET {
                    bail!("part of it is too big");
                }
                self.packet.extend_from_slice(data);
            }
            // A segment shorter than 255 bytes ends its packet.
            if len < 255 {
                let rest = &self.lacing[self.segment..];
                let last_on_page = rest.iter().all(|&l| l == 255);
                return Ok(Some(Packet {
                    data: std::mem::take(&mut self.packet),
                    end: (last_on_page && self.last_page).then_some(self.granule),
                }));
            }
        }
    }

    /// Reads the next page of the stream; false at the end of the file.
    fn read_page(&mut self) -> Result<bool> {
        loop {
            let mut header = [0; 27];
            let got = read_up_to(&mut self.reader, &mut header).context("it can't be read")?;
            if got == 0 {
                return Ok(false);
            }
            if &header[..4] != b"OggS" || header[4] != 0 {
                bail!("it isn't an Ogg file");
            }
            if got < header.len() {
                bail!("it's cut short");
            }
            let mut lacing = vec![0; usize::from(header[26])];
            self.reader
                .read_exact(&mut lacing)
                .context("it's cut short")?;
            let len = lacing.iter().map(|&l| usize::from(l)).sum();
            self.page.resize(len, 0);
            self.reader
                .read_exact(&mut self.page)
                .context("it's cut short")?;
            let serial = u32::from_le_bytes([header[14], header[15], header[16], header[17]]);
            if *self.serial.get_or_insert(serial) != serial {
                continue;
            }
            // A page that doesn't go on with the packet before means that
            // one was cut short; it's dropped.
            if header[5] & 1 == 0 {
                self.packet.clear();
            }
            self.granule = u64::from_le_bytes(header[6..14].try_into().expect("8 bytes"));
            self.last_page = header[5] & 4 != 0;
            (self.lacing, self.segment, self.at) = (lacing, 0, 0);
            return Ok(true);
        }
    }
}

/// Reads into `buf` until it's full or the file ends, returning how much
/// was read.
fn read_up_to(reader: &mut impl Read, buf: &mut [u8]) -> std::io::Result<usize> {
    let mut got = 0;
    while got < buf.len() {
        match reader.read(&mut buf[got..]) {
            Ok(0) => break,
            Ok(n) => got += n,
            Err(e) if e.kind() == std::io::ErrorKind::Interrupted => {}
            Err(e) => return Err(e),
        }
    }
    Ok(got)
}

/// The sound of an Ogg Opus file (RFC 7845), packet by packet, mixed down
/// to one channel.
struct OggOpus<R: Read> {
    packets: OggPackets<R>,
    decoder: OpusDecoder,
    /// Samples at the start that are the encoder's warm-up, not sound.
    skip: usize,
    /// The volume change the file asks for.
    gain: f32,
    /// Samples decoded so far, the warm-up included.
    decoded: u64,
    buffer: Vec<i16>,
}

impl<R: Read> OggOpus<R> {
    fn new(reader: R) -> Result<Self> {
        let mut packets = OggPackets::new(reader);
        let head = packets.next(true)?.context("it's empty")?;
        let h = &head.data;
        if h.len() < 19 || !h.starts_with(b"OpusHead") {
            bail!("it isn't Opus");
        }
        // Versions 0.x; a new major version may be laid out otherwise.
        if h[8] >> 4 != 0 {
            bail!("it's a newer kind of Opus file");
        }
        // Family 0 is mono or stereo; others have more channels.
        if h[18] != 0 || !(1..=2).contains(&h[9]) {
            bail!("it has more than two channels");
        }
        let gain = f32::from(i16::from_le_bytes([h[16], h[17]])) / 256.0;
        let skip = usize::from(u16::from_le_bytes([h[10], h[11]]));
        // The comment header: the encoder's name, maybe pictures.
        packets.next(false)?;
        Ok(Self {
            packets,
            decoder: OpusDecoder::new(RATE, 1)?,
            skip,
            gain: 10f32.powf(gain / 20.0),
            decoded: 0,
            buffer: vec![0; OpusDecoder::MAX_FRAME_SIZE_48K],
        })
    }

    /// The next stretch of sound, into `out` (maybe none); false once
    /// there's no more.
    fn next(&mut self, out: &mut Vec<i16>) -> Result<bool> {
        out.clear();
        let Some(packet) = self.packets.next(true)? else {
            return Ok(false);
        };
        let n = self
            .decoder
            .decode(&packet.data, &mut self.buffer)
            .context("part of it is broken")?;
        let n = n.min(self.buffer.len());
        let end = match packet.end {
            Some(granule) => granule.saturating_sub(self.decoded).min(n as u64) as usize,
            None => n,
        };
        self.decoded += n as u64;
        let start = self.skip.min(end);
        self.skip -= self.skip.min(n);
        let samples = &self.buffer[start..end];
        if self.gain == 1.0 {
            out.extend_from_slice(samples);
        } else {
            let scale = |s: i16| (f32::from(s) * self.gain).clamp(-32768.0, 32767.0) as i16;
            out.extend(samples.iter().map(|&s| scale(s)));
        }
        Ok(true)
    }
}

#[cfg(test)]
mod tests {
    use std::io::Cursor;

    use super::*;

    /// A second of a 440 Hz tone, as ffmpeg's libopus writes it.
    const TONE: &[u8] = include_bytes!("../testdata/voice.ogg");

    fn decode_all(bytes: &[u8]) -> Result<Vec<i16>> {
        decode(OggOpus::new(Cursor::new(bytes))?)
    }

    fn decode<R: Read>(mut sound: OggOpus<R>) -> Result<Vec<i16>> {
        let (mut all, mut some) = (Vec::new(), Vec::new());
        while sound.next(&mut some)? {
            all.extend_from_slice(&some);
        }
        Ok(all)
    }

    #[test]
    fn a_voice_message_decodes_to_the_length_it_was_recorded() {
        let samples = decode_all(TONE).unwrap();
        assert_eq!(
            samples.len(),
            RATE as usize,
            "the warm-up and padding are cut"
        );
        let loudest = samples.iter().map(|s| s.unsigned_abs()).max().unwrap();
        assert!(loudest > 3000, "the tone is there: {loudest}");
    }

    /// An Ogg page of `serial`'s stream, with these segments and data.
    fn page(serial: u32, flags: u8, granule: u64, lacing: &[u8], data: &[u8]) -> Vec<u8> {
        let mut page = b"OggS\0".to_vec();
        page.push(flags);
        page.extend(granule.to_le_bytes());
        page.extend(serial.to_le_bytes());
        // The page's number and checksum, which aren't read.
        page.extend([0; 8]);
        page.push(lacing.len() as u8);
        page.extend(lacing);
        page.extend(data);
        page
    }

    /// Pages holding one packet of `len` bytes.
    fn packet_pages(serial: u32, len: usize) -> Vec<u8> {
        let (mut pages, mut left, mut flags) = (Vec::new(), len, 0);
        loop {
            let mut lacing = Vec::new();
            while lacing.len() < 255 && left >= 255 {
                lacing.push(255);
                left -= 255;
            }
            let done = lacing.len() < 255;
            if done {
                lacing.push(left as u8);
            }
            let size: usize = lacing.iter().map(|&l| usize::from(l)).sum();
            pages.extend(page(serial, flags, u64::MAX, &lacing, &vec![0; size]));
            flags = 1;
            if done {
                return pages;
            }
        }
    }

    /// The tone's pages: the identification header, the comment header,
    /// then the sound.
    fn tone_pages() -> (u32, Vec<&'static [u8]>) {
        let starts: Vec<usize> = (0..TONE.len())
            .filter(|&i| TONE[i..].starts_with(b"OggS"))
            .chain([TONE.len()])
            .collect();
        let pages = starts.windows(2).map(|w| &TONE[w[0]..w[1]]).collect();
        (u32::from_le_bytes(TONE[14..18].try_into().unwrap()), pages)
    }

    #[test]
    fn other_streams_in_the_file_are_passed_over_and_not_kept() {
        let (serial, pages) = tone_pages();
        let mut file = pages[0].to_vec();
        // Each a stream of its own, which a general reader keeps track of.
        for other in 1..20_000u32 {
            file.extend(page(serial ^ other, 2, 0, &[3], b"abc"));
        }
        pages[1..].iter().for_each(|p| file.extend(*p));
        assert_eq!(decode_all(&file).unwrap().len(), RATE as usize);
    }

    #[test]
    fn a_huge_comment_header_is_skipped_but_a_huge_packet_of_sound_refused() {
        let (serial, pages) = tone_pages();
        let mut file = pages[0].to_vec();
        file.extend(packet_pages(serial, 4 << 20));
        pages[2..].iter().for_each(|p| file.extend(*p));
        assert_eq!(decode_all(&file).unwrap().len(), RATE as usize);

        let mut file = [pages[0], pages[1]].concat();
        file.extend(packet_pages(serial, MAX_PACKET + 1));
        let refused = decode_all(&file).unwrap_err().to_string();
        assert_eq!(refused, "part of it is too big");
    }

    #[test]
    fn what_isnt_ogg_opus_is_refused_rather_than_played() {
        let refused = |bytes: &[u8]| decode_all(bytes).unwrap_err().to_string();
        assert_eq!(refused(b"ID3\x04 an mp3"), "it isn't an Ogg file");
        let mut vorbis = TONE.to_vec();
        let at = vorbis.windows(8).position(|w| w == b"OpusHead").unwrap();
        vorbis[at..at + 8].copy_from_slice(b"\x01vorbis\0");
        assert_eq!(refused(&vorbis), "it isn't Opus");
    }

    #[test]
    fn a_damaged_file_ends_playback_without_taking_tuigram_down() {
        // Cut short anywhere: an error, or less sound.
        for cut in (0..TONE.len()).step_by(101) {
            let _ = decode_all(&TONE[..cut]);
        }
        // The decoder panics on some packets a sender can make; playing
        // runs it where that's caught, which this does too.
        let mut packets = OggPackets::new(Cursor::new(TONE));
        packets.next(false).unwrap();
        packets.next(false).unwrap();
        let mut seed = 1u32;
        let mut buffer = vec![0; OpusDecoder::MAX_FRAME_SIZE_48K];
        for _ in 0..12 {
            let packet = packets.next(true).unwrap().unwrap().data;
            for at in 0..packet.len() {
                seed = seed.wrapping_mul(1_103_515_245).wrapping_add(12_345);
                let mut bad = packet.clone();
                bad[at] ^= (seed >> 16) as u8 | 1;
                let _ = crate::images::contained(|| {
                    let mut decoder = OpusDecoder::new(RATE, 1)?;
                    Ok(decoder.decode(&bad, &mut buffer)?)
                });
            }
        }
        assert!(!crate::images::panic_is_contained(), "only while decoding");
    }

    #[test]
    fn waveforms_unpack_five_bits_at_a_time_lowest_first() {
        // 31, 0, 31, 1, 2, 3, 4, 5: 40 bits.
        let levels = [31u8, 0, 31, 1, 2, 3, 4, 5];
        let mut bits = 0u64;
        for (i, l) in levels.iter().enumerate() {
            bits |= u64::from(*l) << (i * 5);
        }
        let bytes = &bits.to_le_bytes()[..5];
        assert_eq!(unpack(bytes), levels);
        use base64::Engine;
        let sent = base64::engine::general_purpose::STANDARD.encode(bytes);
        assert_eq!(super::levels(&sent), levels);
        assert_eq!(super::levels("not base64!"), Vec::<u8>::new());
    }

    #[test]
    fn bars_scale_to_the_loudest_and_fit_any_width() {
        assert_eq!(bars(&[0, 8, 16, 31], 4), "▁▃▅█");
        assert_eq!(bars(&[0, 31], 4), "▁▁██", "stretched");
        assert_eq!(
            bars(&[31, 0, 0, 0, 0, 0, 0, 31], 2),
            "██",
            "the loudest of each"
        );
        assert_eq!(bars(&[], 3), "▁▁▁", "flat without a waveform");
        assert_eq!(bars(&[0, 0], 2), "▁▁", "silence");
    }

    fn player() -> (Player, tokio::sync::mpsc::UnboundedReceiver<VoiceEvent>) {
        let (tx, rx) = tokio::sync::mpsc::unbounded_channel();
        (Player::new(tx, Output::Nowhere), rx)
    }

    fn fixture(name: &str) -> PathBuf {
        let path = std::env::temp_dir().join(format!("tuigram-test-{name}.ogg"));
        std::fs::write(&path, TONE).unwrap();
        path
    }

    #[test]
    fn a_voice_message_plays_once_its_file_is_downloaded_and_then_ends() {
        let (mut player, mut rx) = player();
        player.play(1, 10, 99, true);
        assert!(player.waits_for(99) && !player.waits_for(98));
        assert_eq!(player.next_tick(), None, "nothing moves while it downloads");
        let shown = player.playback(1).unwrap();
        assert_eq!((shown.message_id, shown.at), (10, Duration::ZERO));
        assert_eq!(player.playback(2), None, "only in its own chat");

        player.start(fixture("plays"));
        assert!(!player.waits_for(99));
        let news = |happened| News {
            chat_id: 1,
            message_id: 10,
            tell: true,
            happened,
        };
        let started = player.on_event(rx.blocking_recv().unwrap());
        assert_eq!(started, Some(news(Happened::Started)));
        assert!(player.is_on(1, 10), "still on");
        let ended = player.on_event(rx.blocking_recv().unwrap());
        assert_eq!(ended, Some(news(Happened::Ended)));
        assert!(!player.is_on(1, 10));
    }

    #[test]
    fn a_file_that_isnt_a_voice_message_says_why() {
        let (mut player, mut rx) = player();
        player.play(1, 10, 99, true);
        let path = std::env::temp_dir().join("tuigram-test-not-voice.ogg");
        std::fs::write(&path, b"<html>").unwrap();
        player.start(path);
        let failed = player.on_event(rx.blocking_recv().unwrap()).unwrap();
        assert_eq!(
            failed.happened,
            Happened::Failed("it isn't an Ogg file".into())
        );
    }

    #[test]
    fn word_from_a_stopped_playback_is_dropped() {
        let (mut player, mut rx) = player();
        player.play(1, 10, 99, true);
        player.toggle();
        player.start(fixture("stopped"));
        // Paused before a sound: nothing is said until it plays on.
        player.play(1, 11, 98, true);
        assert_eq!(player.on_event(rx.blocking_recv().unwrap()), None);
        assert!(player.is_on(1, 11), "the new one is untouched");
    }

    #[test]
    fn pausing_before_the_download_ends_starts_it_paused() {
        let (mut player, _rx) = player();
        player.play(1, 10, 99, true);
        assert!(player.toggle(), "paused");
        assert!(player.playback(1).unwrap().paused);
        player.start(fixture("paused"));
        assert_eq!(player.next_tick(), None, "nothing moves while paused");
        assert!(!player.toggle(), "plays on");
        assert!(player.next_tick().is_some());
    }
}

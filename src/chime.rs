//! The sound new messages make (`notification_sound` in `settings.toml`):
//! one of tuigram's own, made here when it's picked, or one of the user's,
//! a WAV or Ogg Opus file in `sounds/` in the data directory. It plays on
//! this computer's speakers ([`crate::voice::Player::ring`]), so over SSH,
//! where those are another computer's, nothing plays ([`over_ssh`]) and the
//! terminal makes its own sound as before.

use std::collections::BTreeMap;
use std::f32::consts::TAU;
use std::io::{Cursor, Read};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use crate::text;
use crate::voice::{OggOpus, RATE};

/// The biggest sound file read. Ten seconds of WAV at 96 kHz in stereo,
/// 32 bits a sample, is under 8 MB.
const MAX_FILE_BYTES: u64 = 16 << 20;
/// The longest a sound plays; the rest of a longer file is cut.
const MAX_SECONDS: u32 = 10;
const MAX_SAMPLES: usize = (RATE * MAX_SECONDS) as usize;
/// How loud tuigram's own sounds are at their loudest, of full scale. The
/// user's play as loud as they are.
const PEAK: f32 = 0.5;
/// A sound fades out over its last 20 ms, so one cut short doesn't click.
const FADE: usize = RATE as usize / 50;
/// The files in `sounds/` that are listed, by extension.
const KINDS: [&str; 3] = ["wav", "ogg", "opus"];

/// Makes one of tuigram's own sounds, at any loudness.
type Make = fn() -> Vec<f32>;

/// tuigram's own sounds: what `settings.toml` calls each, what the `?`
/// popup does, and how it's made.
const BUILT_IN: [(&str, &str, Make); 5] = [
    ("ding", "Ding", ding),
    ("chime", "Chime", chime),
    ("trill", "Trill", trill),
    ("pop", "Pop", pop),
    ("knock", "Knock", knock),
];

/// A sound to pick in the `?` popup.
pub struct Sound {
    /// What `settings.toml` calls it: a built-in sound's name, or the
    /// user's file name without its extension.
    pub id: String,
    pub label: String,
    source: Source,
}

enum Source {
    BuiltIn(Make),
    File(PathBuf),
}

/// Every sound: the built-in ones, then the user's by name.
pub struct Sounds {
    pub list: Vec<Sound>,
    /// Where the user's sounds were read from.
    pub dir: Option<PathBuf>,
}

impl Sounds {
    /// Only the built-in sounds.
    #[cfg(test)]
    pub fn built_in() -> Self {
        Self::new(BTreeMap::new(), None)
    }

    /// The built-in sounds and the WAV and Ogg files in `dir`. A file is
    /// only read once it's picked.
    pub fn load(dir: &Path) -> Self {
        let mut paths: Vec<PathBuf> = std::fs::read_dir(dir)
            .into_iter()
            .flatten()
            .flatten()
            .map(|entry| entry.path())
            .collect();
        // Of two files with one name, `a.ogg` and `a.wav`, always the same.
        paths.sort();
        let mut user = BTreeMap::new();
        for path in paths {
            let listed = path
                .extension()
                .and_then(|e| e.to_str())
                .is_some_and(|e| KINDS.iter().any(|kind| e.eq_ignore_ascii_case(kind)));
            if let Some(id) = path.file_stem().and_then(|s| s.to_str())
                && listed
            {
                user.entry(id.to_string()).or_insert(path);
            }
        }
        Self::new(user, Some(dir.to_path_buf()))
    }

    fn new(mut user: BTreeMap<String, PathBuf>, dir: Option<PathBuf>) -> Self {
        // A file named like a built-in sound takes its place.
        let mut list: Vec<Sound> = BUILT_IN
            .iter()
            .map(|&(id, label, make)| Sound {
                id: id.into(),
                label: label.into(),
                source: user.remove(id).map_or(Source::BuiltIn(make), Source::File),
            })
            .collect();
        let mut own: Vec<Sound> = user
            .into_iter()
            .map(|(id, path)| Sound {
                label: text::clean(&id),
                id,
                source: Source::File(path),
            })
            .collect();
        own.sort_by_key(|sound| sound.label.to_lowercase());
        list.extend(own);
        Self { list, dir }
    }

    /// The sound called `id`, ready to play, or why it can't be.
    pub fn samples(&self, id: &str) -> Result<Arc<[i16]>, String> {
        let Some(sound) = self.list.iter().find(|sound| sound.id == id) else {
            return Err(format!("there is no sound called {:?}", text::clean(id)));
        };
        let samples = match &sound.source {
            Source::BuiltIn(make) => level(&make()),
            Source::File(path) => {
                let name = path.file_name().unwrap_or_default().to_string_lossy();
                read(path).map_err(|e| text::clean(&format!("{name}: {e}")))?
            }
        };
        Ok(samples.into())
    }
}

/// tuigram runs on another computer, reached over SSH: a sound would play
/// there, not where the user is.
pub fn over_ssh(env: impl Fn(&str) -> Option<String>) -> bool {
    ["SSH_CONNECTION", "SSH_CLIENT", "SSH_TTY"]
        .iter()
        .any(|name| env(name).is_some_and(|v| !v.is_empty()))
}

/// A sound file's sound. Links are followed, but only to a plain file of a
/// sound's size: a link to `/dev/zero` or a pipe would otherwise hang
/// tuigram, or fill its memory.
fn read(path: &Path) -> Result<Vec<i16>, String> {
    let meta = std::fs::metadata(path).map_err(|e| e.to_string())?;
    if !meta.is_file() {
        return Err("not a file".into());
    }
    if meta.len() > MAX_FILE_BYTES {
        return Err(format!("bigger than {} MB", MAX_FILE_BYTES >> 20));
    }
    let mut bytes = Vec::new();
    std::fs::File::open(path)
        .and_then(|file| file.take(MAX_FILE_BYTES).read_to_end(&mut bytes))
        .map_err(|e| e.to_string())?;
    let samples = if bytes.starts_with(b"RIFF") {
        wav(&bytes)?
    } else if bytes.starts_with(b"OggS") {
        ogg(&bytes)?
    } else {
        return Err("it's neither WAV nor Ogg".into());
    };
    if samples.is_empty() {
        return Err("it has no sound".into());
    }
    Ok(faded(samples))
}

/// What a WAV file's `fmt ` chunk says.
struct Format {
    float: bool,
    channels: usize,
    rate: u32,
    /// Bytes a sample.
    bytes: usize,
}

/// The sound of a WAV file, its channels mixed into one.
fn wav(bytes: &[u8]) -> Result<Vec<i16>, String> {
    if bytes.get(8..12) != Some(b"WAVE") {
        return Err("it isn't a WAV file".into());
    }
    let mut format = None;
    let mut at: usize = 12;
    while let Some(head) = at.checked_add(8).and_then(|end| bytes.get(at..end)) {
        let len = u32::from_le_bytes([head[4], head[5], head[6], head[7]]) as usize;
        let start = at + 8;
        // A file cut short, or one whose `data` goes on to its end.
        let body = &bytes[start..start.saturating_add(len).min(bytes.len())];
        match &head[..4] {
            b"fmt " => format = Some(wav_format(body)?),
            b"data" => {
                let format = format.ok_or("its sound comes before its format")?;
                return Ok(resampled(&wav_samples(&format, body), format.rate));
            }
            _ => {}
        }
        // Chunks take an even number of bytes.
        at = start.saturating_add(len).saturating_add(len & 1);
    }
    Err("it has no sound".into())
}

fn wav_format(body: &[u8]) -> Result<Format, String> {
    let short = || "its format is cut short".to_string();
    let u16_at = |i: usize| body.get(i..i + 2).map(|b| u16::from_le_bytes([b[0], b[1]]));
    let (Some(tag), Some(channels), Some(bits)) = (u16_at(0), u16_at(2), u16_at(14)) else {
        return Err(short());
    };
    let rate = body
        .get(4..8)
        .map(|b| u32::from_le_bytes([b[0], b[1], b[2], b[3]]))
        .ok_or_else(short)?;
    // WAVE_FORMAT_EXTENSIBLE has the format in its sub-format's first bytes.
    let tag = match tag {
        0xfffe => u16_at(24).ok_or_else(short)?,
        tag => tag,
    };
    let float = match (tag, bits) {
        (1, 8 | 16 | 24 | 32) => false,
        (3, 32 | 64) => true,
        (1 | 3, _) => return Err(format!("{bits}-bit samples can't be played")),
        _ => return Err("it's compressed, which can't be played: save it as PCM".into()),
    };
    if channels == 0 || !(1000..=384_000).contains(&rate) {
        return Err("its format is broken".into());
    }
    Ok(Format {
        float,
        channels: usize::from(channels),
        rate,
        bytes: usize::from(bits / 8),
    })
}

/// The samples of a WAV file's `data`, from -1 to 1, at most
/// [`MAX_SECONDS`] of them.
fn wav_samples(format: &Format, data: &[u8]) -> Vec<f32> {
    let most = (format.rate * MAX_SECONDS) as usize;
    data.chunks_exact(format.channels * format.bytes)
        .take(most)
        .map(|frame| {
            let all: f32 = frame
                .chunks_exact(format.bytes)
                .map(|b| sample(b, format.float))
                .sum();
            all / format.channels as f32
        })
        .collect()
}

/// One sample, from -1 to 1, little-endian as in WAV files.
fn sample(b: &[u8], float: bool) -> f32 {
    match (b.len(), float) {
        (1, _) => (f32::from(b[0]) - 128.0) / 128.0,
        (2, _) => f32::from(i16::from_le_bytes([b[0], b[1]])) / 32768.0,
        (3, _) => (i32::from_le_bytes([0, b[0], b[1], b[2]]) >> 8) as f32 / 8_388_608.0,
        (4, false) => i32::from_le_bytes([b[0], b[1], b[2], b[3]]) as f32 / 2_147_483_648.0,
        (4, true) => f32::from_le_bytes([b[0], b[1], b[2], b[3]]),
        _ => <[u8; 8]>::try_from(b).map_or(0.0, |b| f64::from_le_bytes(b) as f32),
    }
}

/// `sound`, `rate` samples a second, at [`RATE`], drawing straight lines
/// between its samples: good enough for a short sound.
fn resampled(sound: &[f32], rate: u32) -> Vec<i16> {
    if rate == RATE {
        return sound.iter().map(|&s| to_i16(s)).collect();
    }
    let step = f64::from(rate) / f64::from(RATE);
    let len = (sound.len() as f64 / step) as usize;
    (0..len)
        .map(|i| {
            let at = i as f64 * step;
            let (n, part) = (at as usize, at.fract() as f32);
            let a = sound.get(n).copied().unwrap_or(0.0);
            let b = sound.get(n + 1).copied().unwrap_or(a);
            to_i16(a + (b - a) * part)
        })
        .collect()
}

/// The sound of an Ogg Opus file, as voice messages are decoded, at most
/// [`MAX_SECONDS`] of it.
fn ogg(bytes: &[u8]) -> Result<Vec<i16>, String> {
    let decode = || -> anyhow::Result<Vec<i16>> {
        let mut sound = OggOpus::new(Cursor::new(bytes))?;
        let (mut all, mut some) = (Vec::new(), Vec::new());
        while all.len() < MAX_SAMPLES && sound.next(&mut some)? {
            all.extend_from_slice(&some);
        }
        all.truncate(MAX_SAMPLES);
        Ok(all)
    };
    // The decoder panics on some broken packets.
    match crate::images::contained(|| Ok(decode())) {
        Ok(Ok(samples)) => Ok(samples),
        Ok(Err(e)) => Err(format!("{e:#}")),
        Err(_) => Err("it couldn't be decoded".into()),
    }
}

fn to_i16(sample: f32) -> i16 {
    (sample * 32768.0).clamp(-32768.0, 32767.0) as i16
}

/// `sound` fading out over its last [`FADE`] samples, to nothing.
fn faded(mut sound: Vec<i16>) -> Vec<i16> {
    let fade = FADE.min(sound.len());
    let start = sound.len() - fade;
    for (i, s) in sound[start..].iter_mut().enumerate() {
        *s = (f32::from(*s) * (fade - 1 - i) as f32 / fade as f32) as i16;
    }
    sound
}

/// tuigram's own `sound`, at [`PEAK`] at its loudest.
fn level(sound: &[f32]) -> Vec<i16> {
    let loudest = sound.iter().fold(0f32, |most, s| most.max(s.abs()));
    let scale = if loudest > 0.0 { PEAK / loudest } else { 0.0 };
    faded(sound.iter().map(|s| to_i16(s * scale)).collect())
}

/// `secs` seconds of silence, to strike notes in.
fn silence(secs: f32) -> Vec<f32> {
    vec![0.0; (secs * RATE as f32) as usize]
}

/// A note's overtones: times its pitch, how loud, and how fast each fades
/// (a second takes its loudness times e to the minus this).
type Partials = [(f32, f32, f32)];

/// A small bell's: the overtones of a struck bar, which aren't whole
/// multiples of the note, fading faster the higher they are.
const BELL: [(f32, f32, f32); 3] = [(1.0, 1.0, 4.5), (2.76, 0.3, 10.0), (5.4, 0.1, 18.0)];
/// The same, struck short.
const TAP: [(f32, f32, f32); 2] = [(1.0, 1.0, 12.0), (2.76, 0.25, 22.0)];
/// A hollow block of wood: low, and gone at once.
const WOOD: [(f32, f32, f32); 3] = [(1.0, 1.0, 28.0), (2.6, 0.5, 40.0), (4.7, 0.25, 60.0)];

/// Strikes a note of `pitch` Hz `at` seconds into `out`, starting over
/// 4 ms so it doesn't click.
fn strike(out: &mut [f32], at: f32, pitch: f32, partials: &Partials) {
    let rate = RATE as f32;
    let attack = 0.004 * rate;
    let start = (at * rate) as usize;
    for (i, sample) in out.iter_mut().skip(start).enumerate() {
        let t = i as f32 / rate;
        let tone: f32 = partials
            .iter()
            .map(|&(times, loud, fade)| loud * (-fade * t).exp() * (TAU * pitch * times * t).sin())
            .sum();
        *sample += (i as f32 / attack).min(1.0) * tone;
    }
}

/// One bell, a high C.
fn ding() -> Vec<f32> {
    let mut out = silence(0.9);
    strike(&mut out, 0.0, 1046.5, &BELL);
    out
}

/// Two bells, going up a fourth: G, then C.
fn chime() -> Vec<f32> {
    let mut out = silence(1.0);
    strike(&mut out, 0.0, 784.0, &BELL);
    strike(&mut out, 0.14, 1046.5, &BELL);
    out
}

/// Three quick notes going up, C, E and G, the last left to ring.
fn trill() -> Vec<f32> {
    let mut out = silence(0.8);
    strike(&mut out, 0.0, 1046.5, &TAP);
    strike(&mut out, 0.07, 1318.5, &TAP);
    strike(&mut out, 0.14, 1568.0, &BELL);
    out
}

/// A drop: a short note falling from 1200 Hz to 400.
fn pop() -> Vec<f32> {
    let rate = RATE as f32;
    let mut out = silence(0.12);
    let mut phase = 0f32;
    for (i, sample) in out.iter_mut().enumerate() {
        let t = i as f32 / rate;
        phase += TAU * (400.0 + 800.0 * (-40.0 * t).exp()) / rate;
        *sample = (t / 0.002).min(1.0) * (-35.0 * t).exp() * phase.sin();
    }
    out
}

/// Two knocks on wood.
fn knock() -> Vec<f32> {
    let mut out = silence(0.35);
    strike(&mut out, 0.0, 330.0, &WOOD);
    strike(&mut out, 0.13, 330.0, &WOOD);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A WAV file with the format `tag` (1 for whole numbers, 3 for
    /// floating point) and these samples.
    fn wav_file(tag: u16, channels: u16, rate: u32, bits: u16, data: &[u8]) -> Vec<u8> {
        let align = channels * bits / 8;
        let mut format = Vec::new();
        format.extend(tag.to_le_bytes());
        format.extend(channels.to_le_bytes());
        format.extend(rate.to_le_bytes());
        format.extend((rate * u32::from(align)).to_le_bytes());
        format.extend(align.to_le_bytes());
        format.extend(bits.to_le_bytes());
        let mut file = b"RIFF\0\0\0\0WAVE".to_vec();
        for (id, body) in [(b"fmt ", &format[..]), (b"data", data)] {
            file.extend(id);
            file.extend((body.len() as u32).to_le_bytes());
            file.extend(body);
            if body.len() % 2 == 1 {
                file.push(0);
            }
        }
        file
    }

    fn pcm16(samples: &[i16]) -> Vec<u8> {
        samples.iter().flat_map(|s| s.to_le_bytes()).collect()
    }

    #[test]
    fn every_built_in_sound_is_short_equally_loud_and_starts_and_ends_quietly() {
        let sounds = Sounds::built_in();
        assert_eq!(sounds.list.len(), BUILT_IN.len());
        let peak = (PEAK * 32768.0) as u16;
        for sound in &sounds.list {
            let samples = sounds.samples(&sound.id).unwrap();
            let secs = samples.len() as f32 / RATE as f32;
            assert!((0.1..=1.0).contains(&secs), "{}: {secs} s", sound.id);
            let loudest = samples.iter().map(|s| s.unsigned_abs()).max().unwrap();
            assert!(
                loudest <= peak && loudest > peak - 100,
                "{}: {loudest}",
                sound.id
            );
            assert!(samples[0].unsigned_abs() < 100, "{}", sound.id);
            assert_eq!(samples[samples.len() - 1], 0, "{}", sound.id);
        }
    }

    #[test]
    fn a_wav_file_plays_in_one_channel_at_tuigrams_rate() {
        // Stereo at 24 kHz, a tenth of a second.
        let frames = [1000, 3000].repeat(2400);
        let samples = wav(&wav_file(1, 2, 24_000, 16, &pcm16(&frames))).unwrap();
        assert_eq!(samples.len(), 4800);
        assert!(
            samples.iter().all(|s| (1999..=2001).contains(s)),
            "the channels mixed"
        );
    }

    #[test]
    fn wav_samples_of_every_usual_size_are_read() {
        let first = |tag, bits, data: &[u8]| wav(&wav_file(tag, 1, RATE, bits, data)).unwrap()[0];
        let half = 16384..=16384;
        assert!(half.contains(&first(1, 8, &[192])), "8-bit, unsigned");
        assert!(half.contains(&first(1, 16, &16384i16.to_le_bytes())));
        assert!(half.contains(&first(1, 24, &[0, 0, 0x40])));
        assert!(half.contains(&-first(1, 24, &[0, 0, 0xc0])), "signed");
        assert!(half.contains(&first(1, 32, &0x4000_0000i32.to_le_bytes())));
        assert!(half.contains(&first(3, 32, &0.5f32.to_le_bytes())));
        assert!(half.contains(&first(3, 64, &0.5f64.to_le_bytes())));
        assert_eq!(first(3, 32, &f32::NAN.to_le_bytes()), 0);
    }

    #[test]
    fn a_wav_file_is_read_past_chunks_it_doesnt_know_and_cut_after_ten_seconds() {
        let mut file = wav_file(1, 1, 8000, 8, &vec![200; 8000 * 11]);
        // An odd-sized chunk, padded to an even size, before the format.
        file.splice(12..12, *b"LIST\x03\0\0\0abc\0");
        assert_eq!(wav(&file).unwrap().len(), MAX_SAMPLES);
    }

    #[test]
    fn a_broken_wav_file_says_why() {
        let data = pcm16(&[1, 2, 3]);
        assert_eq!(wav(b"RIFF").unwrap_err(), "it isn't a WAV file");
        assert_eq!(
            wav(b"RIFF\0\0\0\0WAVEdata\x02\0\0\0\x01\0").unwrap_err(),
            "its sound comes before its format"
        );
        assert_eq!(
            wav(b"RIFF\0\0\0\0WAVEfmt \x04\0\0\0\x01\0\x01\0").unwrap_err(),
            "its format is cut short"
        );
        let broken = "its format is broken";
        assert_eq!(wav(&wav_file(1, 0, RATE, 16, &data)).unwrap_err(), broken);
        assert_eq!(wav(&wav_file(1, 1, 0, 16, &data)).unwrap_err(), broken);
        let twelve = wav(&wav_file(1, 1, RATE, 12, &data)).unwrap_err();
        assert!(twelve.contains("12-bit"), "{twelve}");
        let adpcm = wav(&wav_file(2, 1, RATE, 4, &data)).unwrap_err();
        assert!(adpcm.contains("compressed"), "{adpcm}");
        // Chunks longer than the file.
        let junk = b"RIFF\0\0\0\0WAVEJUNK\xff\xff\xff\xff";
        assert_eq!(wav(junk).unwrap_err(), "it has no sound");
        let mut cut = wav_file(1, 1, RATE, 16, &data);
        cut.truncate(cut.len() - 2);
        assert_eq!(wav(&cut).unwrap().len(), 2, "what's there plays");
    }

    #[test]
    fn an_ogg_opus_file_is_read_as_voice_messages_are() {
        let samples = ogg(include_bytes!("../testdata/voice.ogg")).unwrap();
        assert_eq!(samples.len(), RATE as usize);
        assert!(ogg(b"OggS, but no more").is_err());
    }

    #[test]
    fn the_users_sounds_come_after_the_built_in_ones_and_are_read_when_picked() {
        let dir = std::env::temp_dir().join(format!("tuigram-sounds-{}", std::process::id()));
        std::fs::create_dir_all(dir.join("folder.wav")).unwrap();
        let tone = wav_file(1, 1, RATE, 16, &pcm16(&[8000; 4800]));
        std::fs::write(dir.join("ding.wav"), &tone).unwrap();
        std::fs::write(dir.join("Zap.WAV"), &tone).unwrap();
        std::fs::write(dir.join("broken.ogg"), b"not a sound").unwrap();
        std::fs::write(dir.join("notes.txt"), b"not listed").unwrap();
        let big = std::fs::File::create(dir.join("big.opus")).unwrap();
        big.set_len(MAX_FILE_BYTES + 1).unwrap();
        let sounds = Sounds::load(&dir);
        let ids: Vec<&str> = sounds.list.iter().map(|s| s.id.as_str()).collect();
        assert_eq!(
            ids,
            [
                "ding", "chime", "trill", "pop", "knock", "big", "broken", "folder", "Zap"
            ]
        );
        assert_eq!(
            sounds.samples("ding").unwrap().len(),
            4800,
            "the file named like a built-in sound takes its place"
        );
        assert_eq!(sounds.samples("Zap").unwrap()[0], 8000);
        let why = |id| sounds.samples(id).unwrap_err();
        assert_eq!(why("broken"), "broken.ogg: it's neither WAV nor Ogg");
        assert_eq!(why("folder"), "folder.wav: not a file");
        assert_eq!(why("big"), "big.opus: bigger than 16 MB");
        assert_eq!(why("notes"), "there is no sound called \"notes\"");
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn ssh_is_recognized_from_the_environment() {
        let with = |name: &'static str, value: &'static str| {
            over_ssh(move |n| (n == name).then(|| value.to_string()))
        };
        assert!(with("SSH_CONNECTION", "10.0.0.2 51234 10.0.0.1 22"));
        assert!(with("SSH_TTY", "/dev/pts/3"));
        assert!(!with("SSH_TTY", ""));
        assert!(!with("TERM", "xterm-256color"));
    }
}

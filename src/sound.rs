//! Where voice messages play: the computer's default speakers or
//! headphones. macOS and Windows go through cpal (CoreAudio, WASAPI), whose
//! libraries every such computer has. Linux goes through ALSA, loaded only
//! when something first plays rather than linked: servers and containers
//! often have no libasound, and tuigram must still start there.

use anyhow::Result;

/// Plays mono 16-bit samples at [`crate::voice::RATE`].
pub trait Sink {
    /// What's queued is running low, so more can be written.
    fn wants_more(&self) -> bool;
    /// Queues samples to play. ALSA waits here while its buffer is full.
    fn write(&mut self, samples: &[i16]) -> Result<()>;
    /// Nothing more is coming: what's queued plays out, even if it's less
    /// than ALSA waits for before it starts.
    fn finish(&mut self);
    /// Goes quiet at once; what was queued plays on [`Sink::resume`].
    fn pause(&mut self) -> Result<()>;
    fn resume(&mut self) -> Result<()>;
    /// Samples heard so far.
    fn played(&mut self) -> u64;
    /// Everything written has played.
    fn done(&mut self) -> Result<bool>;
}

/// The default output device.
pub fn open() -> Result<Box<dyn Sink>> {
    device::open()
}

/// Takes the samples and plays nothing: tests and the demo must make no
/// sound.
#[derive(Default)]
pub struct Silent {
    written: u64,
}

impl Sink for Silent {
    fn wants_more(&self) -> bool {
        true
    }

    fn write(&mut self, samples: &[i16]) -> Result<()> {
        self.written += samples.len() as u64;
        Ok(())
    }

    fn finish(&mut self) {}

    fn pause(&mut self) -> Result<()> {
        Ok(())
    }

    fn resume(&mut self) -> Result<()> {
        Ok(())
    }

    fn played(&mut self) -> u64 {
        self.written
    }

    fn done(&mut self) -> Result<bool> {
        Ok(true)
    }
}

#[cfg(any(target_os = "macos", target_os = "windows"))]
mod device {
    use std::collections::VecDeque;
    use std::sync::{Arc, Mutex, MutexGuard, PoisonError};

    use anyhow::{Context, Result, anyhow, bail};
    use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
    use cpal::{FromSample, SampleFormat, SizedSample, StreamConfig};

    use super::Sink;
    use crate::voice::RATE;

    /// Samples queued ahead of what's heard: enough that a busy moment
    /// doesn't cut the sound, and little enough that a pause comes at once.
    const AHEAD: u64 = RATE as u64 / 5;

    /// What the device plays next, shared with its callback.
    #[derive(Default)]
    struct Queue {
        /// Interleaved, at the device's rate and channels.
        samples: VecDeque<f32>,
        /// Frames the device has taken.
        played: u64,
        paused: bool,
        /// The device failed, or went away.
        failed: Option<String>,
    }

    pub struct Device {
        /// Plays while it's kept.
        _stream: cpal::Stream,
        queue: Arc<Mutex<Queue>>,
        rate: u32,
        channels: usize,
        resampler: Resampler,
        scratch: Vec<f32>,
    }

    fn lock(queue: &Mutex<Queue>) -> MutexGuard<'_, Queue> {
        queue.lock().unwrap_or_else(PoisonError::into_inner)
    }

    pub fn open() -> Result<Box<dyn Sink>> {
        let device = cpal::default_host()
            .default_output_device()
            .context("no speakers or headphones found")?;
        let supported = device
            .default_output_config()
            .map_err(|e| anyhow!("the sound device can't be used: {e}"))?;
        let format = supported.sample_format();
        let config: StreamConfig = supported.into();
        let queue = Arc::new(Mutex::new(Queue::default()));
        let stream = match format {
            SampleFormat::F32 => stream::<f32>(&device, config, &queue)?,
            SampleFormat::I16 => stream::<i16>(&device, config, &queue)?,
            SampleFormat::U16 => stream::<u16>(&device, config, &queue)?,
            SampleFormat::I32 => stream::<i32>(&device, config, &queue)?,
            other => bail!("the sound device takes {other} samples, which tuigram can't make"),
        };
        stream
            .play()
            .map_err(|e| anyhow!("the sound device didn't start: {e}"))?;
        Ok(Box::new(Device {
            _stream: stream,
            queue,
            rate: config.sample_rate,
            channels: usize::from(config.channels).max(1),
            resampler: Resampler::new(RATE, config.sample_rate),
            scratch: Vec::new(),
        }))
    }

    fn stream<T: SizedSample + FromSample<f32>>(
        device: &cpal::Device,
        config: StreamConfig,
        queue: &Arc<Mutex<Queue>>,
    ) -> Result<cpal::Stream> {
        let channels = usize::from(config.channels).max(1);
        let fill = Arc::clone(queue);
        let failed = Arc::clone(queue);
        device
            .build_output_stream(
                config,
                move |data: &mut [T], _: &cpal::OutputCallbackInfo| {
                    let mut queue = lock(&fill);
                    let take = match queue.paused {
                        true => 0,
                        false => data.len().min(queue.samples.len()) / channels * channels,
                    };
                    for (out, sample) in data.iter_mut().zip(queue.samples.drain(..take)) {
                        *out = T::from_sample(sample);
                    }
                    data[take..].fill(T::EQUILIBRIUM);
                    queue.played += (take / channels) as u64;
                },
                // Not printed: the screen is tuigram's.
                move |e| lock(&failed).failed = Some(e.to_string()),
                None,
            )
            .map_err(|e| anyhow!("the sound device can't be used: {e}"))
    }

    impl Device {
        fn check(&self) -> Result<()> {
            match &lock(&self.queue).failed {
                Some(e) => bail!("the sound device stopped: {e}"),
                None => Ok(()),
            }
        }
    }

    impl Sink for Device {
        fn wants_more(&self) -> bool {
            let ahead = AHEAD * u64::from(self.rate) / u64::from(RATE);
            (lock(&self.queue).samples.len() / self.channels) < ahead as usize
        }

        fn write(&mut self, samples: &[i16]) -> Result<()> {
            self.check()?;
            self.scratch.clear();
            self.resampler.run(samples, &mut self.scratch);
            let mut queue = lock(&self.queue);
            for &sample in &self.scratch {
                // Speakers left and right; any others stay quiet.
                queue.samples.push_back(sample);
                if self.channels > 1 {
                    queue.samples.push_back(sample);
                }
                for _ in 2..self.channels {
                    queue.samples.push_back(0.0);
                }
            }
            Ok(())
        }

        fn finish(&mut self) {}

        fn pause(&mut self) -> Result<()> {
            lock(&self.queue).paused = true;
            self.check()
        }

        fn resume(&mut self) -> Result<()> {
            lock(&self.queue).paused = false;
            self.check()
        }

        fn played(&mut self) -> u64 {
            lock(&self.queue).played * u64::from(RATE) / u64::from(self.rate)
        }

        fn done(&mut self) -> Result<bool> {
            self.check()?;
            Ok(lock(&self.queue).samples.is_empty())
        }
    }

    /// Changes the sample rate, drawing a straight line between samples:
    /// plenty for a voice.
    pub(super) struct Resampler {
        /// Input samples per output sample.
        step: f64,
        /// Where the next output sample falls, in input samples from the
        /// start of the next input; -1 is `last`.
        at: f64,
        /// The previous input's last sample.
        last: f32,
    }

    impl Resampler {
        pub(super) fn new(from: u32, to: u32) -> Self {
            Self {
                step: f64::from(from) / f64::from(to.max(1)),
                at: 0.0,
                last: 0.0,
            }
        }

        pub(super) fn run(&mut self, input: &[i16], out: &mut Vec<f32>) {
            let Some(&end) = input.last() else {
                return;
            };
            let sample = |i: isize| match i {
                -1 => self.last,
                i => f32::from(input[i as usize]) / 32768.0,
            };
            let len = input.len() as f64;
            while self.at < len - 1.0 {
                let i = self.at.floor();
                let (a, b) = (sample(i as isize), sample(i as isize + 1));
                out.push(a + (b - a) * (self.at - i) as f32);
                self.at += self.step;
            }
            self.at -= len;
            self.last = f32::from(end) / 32768.0;
        }
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        fn resample(from: u32, to: u32, chunks: &[&[i16]]) -> Vec<f32> {
            let mut resampler = Resampler::new(from, to);
            let mut out = Vec::new();
            for chunk in chunks {
                resampler.run(chunk, &mut out);
            }
            out
        }

        #[test]
        fn the_same_rate_passes_samples_through_across_chunks() {
            let out = resample(48_000, 48_000, &[&[0, 16384], &[-16384, 8192]]);
            assert_eq!(out, [0.0, 0.5, -0.5]);
        }

        #[test]
        fn a_lower_rate_keeps_the_length_in_time() {
            let second = vec![1000i16; 48_000];
            let chunks: Vec<&[i16]> = second.chunks(960).collect();
            let out = resample(48_000, 44_100, &chunks);
            assert!((44_090..=44_100).contains(&out.len()), "{}", out.len());
            assert!(
                out[10..]
                    .iter()
                    .all(|s| (s - 1000.0 / 32768.0).abs() < 1e-6)
            );
        }
    }
}

#[cfg(target_os = "linux")]
mod device {
    use std::collections::VecDeque;
    use std::ffi::{CStr, c_char, c_int, c_long, c_uint, c_ulong, c_void};
    use std::ptr::null_mut;
    use std::sync::OnceLock;

    use anyhow::{Result, bail};

    use super::Sink;
    use crate::voice::RATE;

    /// ALSA's `snd_pcm_t`, only ever behind a pointer.
    type Pcm = c_void;

    const PLAYBACK: c_int = 0;
    #[cfg(target_endian = "little")]
    const S16: c_int = 2;
    #[cfg(target_endian = "big")]
    const S16: c_int = 3;
    const RW_INTERLEAVED: c_int = 3;
    /// How much ALSA buffers, in microseconds.
    const LATENCY: c_uint = 100_000;

    /// ALSA calls this with its errors, which it would otherwise print over
    /// the screen. It's declared variadic in C; on the platforms here a
    /// function ignoring the extra arguments takes such a call fine.
    type ErrorHandler =
        unsafe extern "C" fn(*const c_char, c_int, *const c_char, c_int, *const c_char);

    unsafe extern "C" fn quiet(
        _: *const c_char,
        _: c_int,
        _: *const c_char,
        _: c_int,
        _: *const c_char,
    ) {
    }

    // Their C signatures, from alsa/pcm.h and alsa/error.h.
    type SetHandler = unsafe extern "C" fn(Option<ErrorHandler>) -> c_int;
    type Open = unsafe extern "C" fn(*mut *mut Pcm, *const c_char, c_int, c_int) -> c_int;
    type SetParams =
        unsafe extern "C" fn(*mut Pcm, c_int, c_int, c_uint, c_uint, c_int, c_uint) -> c_int;
    type Write = unsafe extern "C" fn(*mut Pcm, *const c_void, c_ulong) -> c_long;
    type Recover = unsafe extern "C" fn(*mut Pcm, c_int, c_int) -> c_int;
    type Delay = unsafe extern "C" fn(*mut Pcm, *mut c_long) -> c_int;
    type Call = unsafe extern "C" fn(*mut Pcm) -> c_int;
    type StrError = unsafe extern "C" fn(c_int) -> *const c_char;

    /// The few functions of libasound tuigram uses.
    struct Alsa {
        open: Open,
        set_params: SetParams,
        writei: Write,
        recover: Recover,
        delay: Delay,
        start: Call,
        drop: Call,
        prepare: Call,
        close: Call,
        strerror: StrError,
    }

    fn alsa() -> Result<&'static Alsa> {
        static ALSA: OnceLock<Option<Alsa>> = OnceLock::new();
        match ALSA.get_or_init(load) {
            Some(alsa) => Ok(alsa),
            None => bail!("there's no sound here (ALSA's libasound.so.2 isn't installed)"),
        }
    }

    fn load() -> Option<Alsa> {
        // SAFETY: the names are NUL-terminated and each symbol is given its
        // C signature. The library is never closed, so the functions stay
        // loaded.
        unsafe {
            let lib = libc::dlopen(
                c"libasound.so.2".as_ptr(),
                libc::RTLD_NOW | libc::RTLD_LOCAL,
            );
            if lib.is_null() {
                return None;
            }
            macro_rules! function {
                ($name:literal as $signature:ty) => {{
                    let f = libc::dlsym(lib, $name.as_ptr());
                    if f.is_null() {
                        return None;
                    }
                    std::mem::transmute::<*mut c_void, $signature>(f)
                }};
            }
            let set_handler = function!(c"snd_lib_error_set_handler" as SetHandler);
            set_handler(Some(quiet));
            Some(Alsa {
                open: function!(c"snd_pcm_open" as Open),
                set_params: function!(c"snd_pcm_set_params" as SetParams),
                writei: function!(c"snd_pcm_writei" as Write),
                recover: function!(c"snd_pcm_recover" as Recover),
                delay: function!(c"snd_pcm_delay" as Delay),
                start: function!(c"snd_pcm_start" as Call),
                drop: function!(c"snd_pcm_drop" as Call),
                prepare: function!(c"snd_pcm_prepare" as Call),
                close: function!(c"snd_pcm_close" as Call),
                strerror: function!(c"snd_strerror" as StrError),
            })
        }
    }

    pub struct Device {
        alsa: &'static Alsa,
        pcm: *mut Pcm,
        written: u64,
        /// The last samples written. Pausing drops what ALSA hasn't played
        /// yet, which is the end of these, and resuming writes it again.
        recent: VecDeque<i16>,
        /// What a pause took back from ALSA.
        again: Vec<i16>,
        /// Nothing more is coming, so ALSA is started by hand.
        finished: bool,
    }

    pub fn open() -> Result<Box<dyn Sink>> {
        let alsa = alsa()?;
        quietly(|| open_with(alsa))
    }

    /// Runs `f` with stderr going nowhere. ALSA's own errors go to `quiet`,
    /// but the libraries it loads for the device (PulseAudio's, JACK's)
    /// print theirs, mostly while connecting, over the screen.
    fn quietly<T>(f: impl FnOnce() -> T) -> T {
        // SAFETY: plain descriptor calls; the saved stderr is put back and
        // both descriptors closed whatever `f` returns.
        unsafe {
            let null = libc::open(c"/dev/null".as_ptr(), libc::O_WRONLY | libc::O_CLOEXEC);
            let saved = libc::fcntl(2, libc::F_DUPFD_CLOEXEC, 3);
            if null < 0 || saved < 0 || libc::dup2(null, 2) < 0 {
                for fd in [null, saved].into_iter().filter(|&fd| fd >= 0) {
                    libc::close(fd);
                }
                return f();
            }
            let out = f();
            libc::dup2(saved, 2);
            libc::close(saved);
            libc::close(null);
            out
        }
    }

    fn open_with(alsa: &'static Alsa) -> Result<Box<dyn Sink>> {
        let mut pcm = null_mut();
        // SAFETY: `pcm` is written by ALSA; "default" is NUL-terminated.
        let e = unsafe { (alsa.open)(&mut pcm, c"default".as_ptr(), PLAYBACK, 0) };
        if e < 0 {
            bail!("the sound device can't be opened: {}", message(alsa, e));
        }
        let device = Device {
            alsa,
            pcm,
            written: 0,
            recent: VecDeque::new(),
            again: Vec::new(),
            finished: false,
        };
        // ALSA converts to whatever the device takes.
        // SAFETY: `pcm` is open.
        let e = unsafe { (alsa.set_params)(pcm, S16, RW_INTERLEAVED, 1, RATE, 1, LATENCY) };
        if e < 0 {
            bail!("the sound device can't be used: {}", message(alsa, e));
        }
        Ok(Box::new(device))
    }

    fn message(alsa: &Alsa, e: c_int) -> String {
        // SAFETY: snd_strerror returns a static string for any code.
        let text = unsafe { (alsa.strerror)(e) };
        if text.is_null() {
            return format!("error {e}");
        }
        // SAFETY: not null, and NUL-terminated.
        unsafe { CStr::from_ptr(text) }
            .to_string_lossy()
            .into_owned()
    }

    impl Device {
        /// Plays what's written, even if it's less than ALSA waits for.
        fn start(&self) {
            // SAFETY: `pcm` is open. It fails if it's already playing.
            unsafe { (self.alsa.start)(self.pcm) };
        }

        /// Samples written that ALSA hasn't played yet.
        fn queued(&self) -> Option<u64> {
            let mut delay: c_long = 0;
            // SAFETY: `pcm` is open; `delay` is written by ALSA.
            let e = unsafe { (self.alsa.delay)(self.pcm, &mut delay) };
            (e == 0).then(|| u64::try_from(delay).unwrap_or(0))
        }

        fn write_all(&mut self, mut samples: &[i16]) -> Result<()> {
            while !samples.is_empty() {
                // SAFETY: `pcm` is open and the pointer and length are of
                // `samples`, one 16-bit sample per frame.
                let n = unsafe {
                    (self.alsa.writei)(self.pcm, samples.as_ptr().cast(), samples.len() as c_ulong)
                };
                if n < 0 {
                    // Most often an underrun, which ALSA recovers from.
                    // SAFETY: `pcm` is open.
                    let e = unsafe { (self.alsa.recover)(self.pcm, n as c_int, 1) };
                    if e < 0 {
                        bail!("the sound device stopped: {}", message(self.alsa, e));
                    }
                    continue;
                }
                if n == 0 {
                    // Not expected of a blocking write, but no busy loop.
                    std::thread::sleep(std::time::Duration::from_millis(10));
                    continue;
                }
                let (done, rest) = samples.split_at((n as usize).min(samples.len()));
                self.written += done.len() as u64;
                self.recent.extend(done);
                // A second: more than ALSA buffers.
                let extra = self.recent.len().saturating_sub(RATE as usize);
                self.recent.drain(..extra);
                samples = rest;
            }
            Ok(())
        }
    }

    impl Sink for Device {
        fn wants_more(&self) -> bool {
            true
        }

        fn write(&mut self, samples: &[i16]) -> Result<()> {
            self.write_all(samples)
        }

        fn finish(&mut self) {
            self.finished = true;
            self.start();
        }

        fn pause(&mut self) -> Result<()> {
            let queued = (self.queued().unwrap_or(0) as usize).min(self.recent.len());
            // SAFETY: `pcm` is open.
            unsafe { (self.alsa.drop)(self.pcm) };
            let from = self.recent.len() - queued;
            self.again = self.recent.drain(from..).collect();
            self.written -= queued as u64;
            Ok(())
        }

        fn resume(&mut self) -> Result<()> {
            // SAFETY: `pcm` is open.
            let e = unsafe { (self.alsa.prepare)(self.pcm) };
            if e < 0 {
                bail!(
                    "the sound device didn't start again: {}",
                    message(self.alsa, e)
                );
            }
            let again = std::mem::take(&mut self.again);
            self.write_all(&again)?;
            // Less than ALSA waits for may be left.
            if self.finished {
                self.start();
            }
            Ok(())
        }

        fn played(&mut self) -> u64 {
            self.written - self.queued().unwrap_or(0).min(self.written)
        }

        fn done(&mut self) -> Result<bool> {
            // Once it has all played, ALSA is in an underrun and says so.
            Ok(self.queued().is_none_or(|n| n == 0))
        }
    }

    impl Drop for Device {
        fn drop(&mut self) {
            // SAFETY: `pcm` is open, and not used after this. Closing drops
            // what hasn't played, so stopping is immediate.
            unsafe { (self.alsa.close)(self.pcm) };
        }
    }
}

#[cfg(not(any(target_os = "macos", target_os = "windows", target_os = "linux")))]
mod device {
    use anyhow::{Result, bail};

    use super::Sink;

    pub fn open() -> Result<Box<dyn Sink>> {
        bail!("tuigram can't play sound on this system yet")
    }
}

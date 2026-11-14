//! LTC, linear timecode: the timecode a playback machine puts on an audio track.
//!
//! LTC is an 80-bit frame, biphase-mark coded, once per video frame. This decodes it from audio samples: it finds the transitions,
//! tells a short interval (half a bit) from a long one (a whole bit), reads bits until the 16-bit sync word, then the hours, minutes,
//! seconds and frames. It does not care about the signal's polarity or level, follows small speed changes, and ignores noise
//! between frames.
//!
//! The position it reports is the time at the *end* of the frame it just read (the decoded frame plus one), which is when the last
//! bit arrived: the same moment MIDI Time Code reports.
//!
//! Sources: raw audio read from a pipe or file (`ffmpeg`, `arecord` or `sox` can feed it), or, in a build with `--features audio`, an
//! audio input device. Decoding itself needs no system library and is tested with synthetic LTC. Reading a real sound card is not
//! tested here.

use crate::governance::{Actor, Perm};
use crate::inputs::InputInfo;
use crate::timecode::{Fps, Tc};
use crate::Show;
use serde::Deserialize;
use serde_json::{json, Value};
use std::collections::VecDeque;
use std::sync::atomic::{AtomicI64, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

/// The sync word, in the order its bits are sent: bits 64 to 79 of a frame.
const SYNC: [u8; 16] = [0, 0, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 0, 1];

pub struct LtcDecoder {
    /// Samples per bit cell, followed as the speed drifts.
    cell: f64,
    sample_rate: f64,
    pos: u64,
    last_edge: Option<u64>,
    positive: bool,
    peak: f32,
    /// The length of the first half of a one, while its second half is awaited.
    pending_half: Option<f64>,
    bits: VecDeque<u8>,
    hint: Fps,
}

fn bcd(bits: &[u8], from: usize, n: usize) -> u32 {
    (0..n).map(|i| (bits[from + i] as u32) << i).sum()
}

impl LtcDecoder {
    /// `hint` is the frame rate to assume where the signal cannot say (29.97 against 30, 23.976 against 24).
    pub fn new(sample_rate: u32, hint: Fps) -> Self {
        let fps = hint.frame_ms().recip() * 1000.0;
        LtcDecoder { cell: sample_rate as f64 / (fps * 80.0), sample_rate: sample_rate as f64, pos: 0, last_edge: None, positive: false, peak: 0.0, pending_half: None, bits: VecDeque::new(), hint }
    }

    /// Feed audio (mono, any level). Returns each frame completed, as its position.
    pub fn push(&mut self, samples: &[f32]) -> Vec<Tc> {
        let mut out = Vec::new();
        for &s in samples {
            self.pos += 1;
            // follow the signal's level so quiet LTC and loud LTC read the same, and use a dead band so noise is not an edge
            self.peak = (self.peak * 0.9995).max(s.abs());
            let band = self.peak * 0.1;
            let crossed = if self.positive { s < -band } else { s > band };
            if !crossed {
                continue;
            }
            self.positive = !self.positive;
            let Some(prev) = self.last_edge.replace(self.pos) else { continue };
            if let Some(tc) = self.interval((self.pos - prev) as f64) {
                out.push(tc);
            }
        }
        out
    }

    fn lose_sync(&mut self) {
        self.pending_half = None;
        self.bits.clear();
    }

    /// An interval between two edges: half a cell is half of a one, a whole cell is a zero.
    fn interval(&mut self, d: f64) -> Option<Tc> {
        let r = d / self.cell;
        let bit = if (0.75..1.3).contains(&r) {
            if self.pending_half.is_some() {
                self.lose_sync();
                return None;
            }
            self.cell = self.cell * 0.95 + d * 0.05;
            0
        } else if (0.35..0.65).contains(&r) {
            let Some(first) = self.pending_half.take() else {
                self.pending_half = Some(d);
                return None;
            };
            // the two halves together are one whole cell: the steadiest thing to follow the speed by
            self.cell = self.cell * 0.95 + (first + d) * 0.05;
            1
        } else {
            self.lose_sync();
            return None;
        };
        self.bits.push_back(bit);
        if self.bits.len() > 80 {
            self.bits.pop_front();
        }
        if self.bits.len() == 80 && self.bits.iter().skip(64).copied().eq(SYNC.iter().copied()) {
            let frame: Vec<u8> = self.bits.iter().copied().collect();
            self.bits.clear();
            return self.read(&frame);
        }
        None
    }

    fn read(&self, b: &[u8]) -> Option<Tc> {
        let (f, s, m, h) = (bcd(b, 0, 4) + 10 * bcd(b, 8, 2), bcd(b, 16, 4) + 10 * bcd(b, 24, 3), bcd(b, 32, 4) + 10 * bcd(b, 40, 3), bcd(b, 48, 4) + 10 * bcd(b, 56, 2));
        let drop = b[10] == 1;
        // the frame rate from how long a frame took, with the hint to settle what the signal cannot say
        let measured = self.sample_rate / (self.cell * 80.0);
        let hint_rate = 1000.0 / self.hint.frame_ms();
        let fps = if drop {
            Fps::F2997Df
        } else if (measured - hint_rate).abs() / hint_rate < 0.04 {
            // close to what the show expects (a tape a few percent fast or slow still is): believe the show
            self.hint
        } else {
            let near = |n: f64| (measured - n).abs() / n < 0.03;
            match (near(24.0), near(25.0), near(30.0)) {
                (true, _, _) => Fps::F24,
                (_, true, _) => Fps::F25,
                (_, _, true) => Fps::F30,
                _ => return None,
            }
        };
        Tc::new(h, m, s, f, fps).ok().map(|t| t.plus_frames(1))
    }
}

// ---- as an input of the show ----

#[derive(Deserialize, Clone, Debug)]
pub struct LtcSpec {
    pub name: String,
    /// Read raw audio from standard input (a pipe from ffmpeg, arecord or sox).
    #[serde(default)]
    pub stdin: bool,
    /// Read raw audio from a file or a named pipe.
    #[serde(default)]
    pub file: Option<String>,
    /// Read an audio input device whose name contains this (a build with `--features audio`).
    #[serde(default)]
    pub device: Option<String>,
    #[serde(default = "rate48k")]
    pub sample_rate: u32,
    /// `s16le` (default), `f32le` or `u8`.
    #[serde(default)]
    pub format: Option<String>,
    #[serde(default = "one")]
    pub channels: usize,
    /// Which channel carries the LTC (0 is the first).
    #[serde(default)]
    pub channel: usize,
    /// The frame rate to assume where the signal cannot say (29.97 against 30). Default 25.
    #[serde(default)]
    pub fps: Option<f64>,
    #[serde(default)]
    pub drop: bool,
    /// A file is read at the speed it plays, like a live signal (default true). Turn off to read it as fast as possible.
    #[serde(default = "yes")]
    pub pace: bool,
}

fn rate48k() -> u32 {
    48_000
}
fn one() -> usize {
    1
}
fn yes() -> bool {
    true
}

/// Check the `ltc` list of an inputs file.
pub fn check(specs: &[LtcSpec]) -> Result<(), String> {
    for s in specs {
        let name = s.name.trim();
        if name.is_empty() || name.len() > 40 || name.chars().any(|c| c.is_control()) {
            return Err("every LTC input needs a short name (it appears in the record)".into());
        }
        let sources = s.stdin as u8 + s.file.is_some() as u8 + s.device.is_some() as u8;
        if sources != 1 {
            return Err(format!("{name}: give exactly one of stdin, file or device"));
        }
        if !(8_000..=192_000).contains(&s.sample_rate) {
            return Err(format!("{name}: sample_rate {} is not between 8000 and 192000", s.sample_rate));
        }
        if !matches!(s.format.as_deref(), None | Some("s16le") | Some("f32le") | Some("u8")) {
            return Err(format!("{name}: format must be s16le, f32le or u8"));
        }
        if s.channels == 0 || s.channels > 64 || s.channel >= s.channels {
            return Err(format!("{name}: channel {} does not exist in {} channel(s)", s.channel, s.channels));
        }
        if let Some(f) = s.fps {
            if Fps::from_number(f, s.drop).is_none() {
                return Err(format!("{name}: fps {f} is not one of 23.976, 24, 25, 29.97, 30"));
            }
        }
    }
    Ok(())
}

pub struct LtcInput {
    pub spec: LtcSpec,
    frames: AtomicU64,
    last_ms: AtomicI64,
    status: Mutex<String>,
}

fn now_ms() -> i64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_millis() as i64).unwrap_or(0)
}

impl LtcInput {
    pub fn actor(&self) -> Actor {
        Actor::Member { name: self.spec.name.clone(), role: "button".into() }
    }
    fn hint(&self) -> Fps {
        Fps::from_number(self.spec.fps.unwrap_or(25.0), self.spec.drop).unwrap_or(Fps::F25)
    }
}

impl InputInfo for LtcInput {
    fn json(&self) -> Value {
        let last = self.last_ms.load(Ordering::Relaxed);
        let source = if self.spec.stdin { "stdin".to_string() } else if let Some(f) = &self.spec.file { format!("file {f}") } else { format!("device {}", self.spec.device.clone().unwrap_or_default()) };
        json!({
            "name": self.spec.name, "listen": format!("LTC {source}"), "bindings": 1,
            "received": self.frames.load(Ordering::Relaxed), "pressed": self.frames.load(Ordering::Relaxed),
            "unlisted_sender": 0, "unbound": 0, "dropped": 0, "refused": 0,
            "heard_ms_ago": (last > 0).then(|| (now_ms() - last).max(0)),
            "last_address": self.status.lock().unwrap().clone(),
        })
    }
}

/// Raw audio bytes into mono samples of one channel.
fn to_samples(bytes: &[u8], format: &str, channels: usize, channel: usize, out: &mut Vec<f32>) {
    let width = match format {
        "f32le" => 4,
        "u8" => 1,
        _ => 2,
    };
    for frame in bytes.chunks_exact(width * channels) {
        let s = &frame[width * channel..width * (channel + 1)];
        out.push(match format {
            "f32le" => f32::from_le_bytes([s[0], s[1], s[2], s[3]]),
            "u8" => (s[0] as f32 - 128.0) / 128.0,
            _ => i16::from_le_bytes([s[0], s[1]]) as f32 / 32768.0,
        });
    }
}

/// Decode what the input hears and hand each frame's position to the show.
fn feed(show: &Arc<Show>, input: &LtcInput, decoder: &mut LtcDecoder, samples: &[f32]) {
    for tc in decoder.push(samples) {
        input.frames.fetch_add(1, Ordering::Relaxed);
        input.last_ms.store(now_ms(), Ordering::Relaxed);
        *input.status.lock().unwrap() = tc.display();
        if show.allow(&input.actor(), Perm::Go, None) {
            show.timecode_update(tc.to_ms(), tc.fps, "ltc");
        }
    }
}

/// Start reading. A pipe, a file or (with `--features audio`) a device; a thread does the reading.
pub fn start(show: &Arc<Show>, spec: LtcSpec) -> Result<Arc<LtcInput>, String> {
    check(std::slice::from_ref(&spec))?;
    let input = Arc::new(LtcInput { spec, frames: AtomicU64::new(0), last_ms: AtomicI64::new(0), status: Mutex::new(String::new()) });
    if input.spec.device.is_some() {
        device::start(show, &input)?;
        show.add_input(Arc::clone(&input) as Arc<dyn InputInfo>);
        return Ok(input);
    }
    let reader: Box<dyn std::io::Read + Send> = if input.spec.stdin {
        Box::new(std::io::stdin())
    } else {
        let path = input.spec.file.clone().unwrap_or_default();
        Box::new(std::fs::File::open(&path).map_err(|e| format!("{}: cannot open {path}: {e}", input.spec.name))?)
    };
    show.add_input(Arc::clone(&input) as Arc<dyn InputInfo>);
    let (weak, me, rt) = (Arc::downgrade(show), Arc::clone(&input), tokio::runtime::Handle::current());
    std::thread::spawn(move || {
        use std::io::Read;
        let mut reader = reader;
        let spec = me.spec.clone();
        let format = spec.format.clone().unwrap_or_else(|| "s16le".into());
        let width = match format.as_str() {
            "f32le" => 4,
            "u8" => 1,
            _ => 2,
        } * spec.channels;
        let mut decoder = LtcDecoder::new(spec.sample_rate, me.hint());
        let mut buf = vec![0u8; width * 1024];
        let mut carry: Vec<u8> = Vec::new();
        let mut samples: Vec<f32> = Vec::new();
        loop {
            let n = match reader.read(&mut buf) {
                Ok(0) | Err(_) => break,
                Ok(n) => n,
            };
            carry.extend_from_slice(&buf[..n]);
            let usable = carry.len() - carry.len() % width;
            samples.clear();
            to_samples(&carry[..usable], &format, spec.channels, spec.channel, &mut samples);
            carry.drain(..usable);
            let Some(show) = weak.upgrade() else { return };
            let _guard = rt.enter(); // the show spawns tasks: give this thread the runtime
            feed(&show, &me, &mut decoder, &samples);
            if spec.pace && spec.file.is_some() {
                std::thread::sleep(std::time::Duration::from_secs_f64(samples.len() as f64 / spec.sample_rate as f64));
            }
        }
        *me.status.lock().unwrap() = "the audio stream ended".into();
    });
    Ok(input)
}

/// Reading an audio input device needs the system's audio library, so it is built only with `--features audio`.
#[cfg(not(feature = "audio"))]
mod device {
    use super::*;
    pub fn start(_show: &Arc<Show>, input: &Arc<LtcInput>) -> Result<(), String> {
        Err(format!("{}: this build cannot read an audio device. Build with: cargo build --release -p pchi-studio-api --features audio (on Linux it needs libasound2-dev), or pipe the audio in with stdin or file", input.spec.name))
    }
}

#[cfg(feature = "audio")]
mod device {
    use super::*;
    use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};

    pub fn list() -> Vec<String> {
        cpal::default_host().input_devices().map(|d| d.filter_map(|d| d.name().ok()).collect()).unwrap_or_default()
    }

    pub fn start(show: &Arc<Show>, input: &Arc<LtcInput>) -> Result<(), String> {
        let want = input.spec.device.clone().unwrap_or_default().to_lowercase();
        let host = cpal::default_host();
        let dev = host
            .input_devices()
            .map_err(|e| format!("{}: cannot list audio devices: {e}", input.spec.name))?
            .find(|d| d.name().map(|n| n.to_lowercase().contains(&want)).unwrap_or(false))
            .ok_or_else(|| format!("{}: no audio input matches '{want}'. Found: {}", input.spec.name, list().join(", ")))?;
        let cfg = dev.default_input_config().map_err(|e| format!("{}: {e}", input.spec.name))?;
        let (rate, channels) = (cfg.sample_rate().0, cfg.channels() as usize);
        if input.spec.channel >= channels {
            return Err(format!("{}: the device has {channels} channel(s), channel {} does not exist", input.spec.name, input.spec.channel));
        }
        let (weak, me, rt) = (Arc::downgrade(show), Arc::clone(input), tokio::runtime::Handle::current());
        let pick = input.spec.channel;
        let mut decoder = LtcDecoder::new(rate, me.hint());
        let mut mono: Vec<f32> = Vec::new();
        let err = |e: cpal::StreamError| eprintln!("LTC audio stream error: {e}");
        macro_rules! build {
            ($t:ty) => {{
                let (weak, me, rt) = (weak.clone(), Arc::clone(&me), rt.clone());
                dev.build_input_stream(
                    &cfg.clone().into(),
                    move |data: &[$t], _: &cpal::InputCallbackInfo| {
                        mono.clear();
                        for frame in data.chunks_exact(channels) {
                            mono.push(cpal::Sample::to_sample::<f32>(frame[pick]));
                        }
                        if let Some(show) = weak.upgrade() {
                            let _guard = rt.enter();
                            feed(&show, &me, &mut decoder, &mono);
                        }
                    },
                    err,
                    None,
                )
            }};
        }
        let stream = match cfg.sample_format() {
            cpal::SampleFormat::F32 => build!(f32),
            cpal::SampleFormat::I16 => build!(i16),
            cpal::SampleFormat::U16 => build!(u16),
            other => return Err(format!("{}: the device's sample format {other:?} is not supported", input.spec.name)),
        }
        .map_err(|e| format!("{}: cannot open the audio device: {e}", input.spec.name))?;
        stream.play().map_err(|e| format!("{}: {e}", input.spec.name))?;
        // keep the stream alive for the life of the runtime
        std::mem::forget(stream);
        Ok(())
    }
}

/// The audio input devices this machine has (empty without `--features audio`).
pub fn list_devices() -> Vec<String> {
    #[cfg(feature = "audio")]
    {
        return device::list();
    }
    #[cfg(not(feature = "audio"))]
    Vec::new()
}

/// The 80 bits of an LTC frame for `t`, in the order they are sent. For tests.
#[cfg(test)]
pub fn frame_bits(t: Tc) -> Vec<u8> {
    let mut b = vec![0u8; 80];
    let mut put = |from: usize, n: usize, v: u32| {
        for i in 0..n {
            b[from + i] = ((v >> i) & 1) as u8;
        }
    };
    put(0, 4, t.f % 10);
    put(8, 2, t.f / 10);
    put(16, 4, t.s % 10);
    put(24, 3, t.s / 10);
    put(32, 4, t.m % 10);
    put(40, 3, t.m / 10);
    put(48, 4, t.h % 10);
    put(56, 2, t.h / 10);
    if t.fps.is_drop() {
        b[10] = 1;
    }
    b[64..80].copy_from_slice(&SYNC);
    b
}

/// Audio for a run of frames, biphase-mark coded, at `sample_rate`, `amplitude` and optionally inverted. For tests.
#[cfg(test)]
pub fn synth(first: Tc, frames: usize, sample_rate: u32, amplitude: f32, invert: bool, speed: f64) -> Vec<f32> {
    let cell = sample_rate as f64 / (first.fps.frame_ms().recip() * 1000.0 * 80.0) / speed;
    let mut out = Vec::new();
    let (mut level, mut clock) = (if invert { -1.0f32 } else { 1.0 }, 0.0f64);
    let mut t = first;
    for _ in 0..frames {
        for bit in frame_bits(t) {
            level = -level; // a transition at the start of every cell
            for half in 0..2 {
                if half == 1 && bit == 1 {
                    level = -level; // and one in the middle of a one
                }
                clock += cell / 2.0;
                while (out.len() as f64) < clock {
                    out.push(level * amplitude);
                }
            }
        }
        t = t.plus_frames(1);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tc(h: u32, m: u32, s: u32, f: u32, fps: Fps) -> Tc {
        Tc::new(h, m, s, f, fps).unwrap()
    }

    fn decode(samples: &[f32], sr: u32, hint: Fps) -> Vec<Tc> {
        LtcDecoder::new(sr, hint).push(samples)
    }

    #[test]
    fn a_frame_is_80_bits_ending_in_the_sync_word() {
        let b = frame_bits(tc(12, 34, 56, 17, Fps::F25));
        assert_eq!(b.len(), 80);
        assert_eq!(&b[64..], &SYNC);
        assert_eq!(bcd(&b, 0, 4) + 10 * bcd(&b, 8, 2), 17);
        assert_eq!(bcd(&b, 48, 4) + 10 * bcd(&b, 56, 2), 12);
        assert_eq!(b[10], 0, "not drop-frame");
        assert_eq!(frame_bits(tc(0, 1, 0, 2, Fps::F2997Df))[10], 1, "the drop-frame flag");
    }

    #[test]
    fn a_run_of_frames_is_read_back_as_the_positions_at_their_ends() {
        for (fps, sr) in [(Fps::F25, 48_000), (Fps::F24, 48_000), (Fps::F30, 44_100), (Fps::F2997Df, 48_000)] {
            let start = tc(1, 2, 3, 4, fps);
            let got = decode(&synth(start, 12, sr, 0.6, false, 1.0), sr, fps);
            // the first frame is spent finding the bit rate; after that every frame is read
            assert!(got.len() >= 10, "{fps:?}: only {} frames", got.len());
            let first = got[0];
            assert_eq!(first.fps, fps, "{fps:?}");
            for (i, g) in got.iter().enumerate() {
                assert_eq!(*g, first.plus_frames(i as u64), "{fps:?} frame {i}");
            }
            assert!(first.frames() > start.frames() && first.frames() <= start.plus_frames(3).frames(), "{fps:?}: the first frame read is near the start");
        }
    }

    #[test]
    fn level_and_polarity_do_not_matter() {
        let start = tc(0, 0, 10, 0, Fps::F25);
        let loud = decode(&synth(start, 8, 48_000, 0.95, false, 1.0), 48_000, Fps::F25);
        let quiet = decode(&synth(start, 8, 48_000, 0.02, false, 1.0), 48_000, Fps::F25);
        let inverted = decode(&synth(start, 8, 48_000, 0.4, true, 1.0), 48_000, Fps::F25);
        for run in [&loud, &quiet, &inverted] {
            assert!(run.len() >= 5, "{run:?}");
            assert!(run.windows(2).all(|w| w[1] == w[0].plus_frames(1)), "{run:?}");
            assert!(run[0].frames() > start.frames() && run[0].frames() <= start.plus_frames(4).frames());
        }
        // wherever two of them both read a frame, they read the same one
        for other in [&quiet, &inverted] {
            assert!(loud.iter().filter(|t| other.contains(t)).count() >= 4);
        }
    }

    #[test]
    fn a_tape_a_little_fast_or_slow_is_still_read_and_the_chunks_do_not_matter() {
        let start = tc(0, 5, 0, 0, Fps::F25);
        for speed in [0.97, 1.0, 1.03] {
            let audio = synth(start, 14, 48_000, 0.5, false, speed);
            let whole = decode(&audio, 48_000, Fps::F25);
            assert!(whole.len() >= 11, "speed {speed}: {} frames", whole.len());
            // fed a few hundred samples at a time, as a sound card hands them over
            let mut d = LtcDecoder::new(48_000, Fps::F25);
            let chunks: Vec<Tc> = audio.chunks(333).flat_map(|c| d.push(c)).collect();
            assert_eq!(chunks, whole, "speed {speed}");
        }
    }

    #[test]
    fn noise_and_a_start_in_the_middle_do_not_make_up_timecode() {
        // hiss alone is nothing
        let mut seed = 12345u32;
        let hiss: Vec<f32> = (0..48_000).map(|_| { seed = seed.wrapping_mul(1664525).wrapping_add(1013904223); ((seed >> 16) as f32 / 32768.0 - 1.0) * 0.05 }).collect();
        assert!(decode(&hiss, 48_000, Fps::F25).is_empty());
        // joining part way through a frame: nothing until a whole frame has been seen, then the right ones
        let start = tc(2, 0, 0, 0, Fps::F25);
        let audio = synth(start, 10, 48_000, 0.5, false, 1.0);
        let got = decode(&audio[777..], 48_000, Fps::F25);
        assert!(!got.is_empty());
        assert!(got.windows(2).all(|w| w[1] == w[0].plus_frames(1)), "{got:?}");
        // silence in the middle breaks the run but does not produce a wrong frame
        let mut gap = synth(start, 5, 48_000, 0.5, false, 1.0);
        gap.extend(std::iter::repeat(0.0).take(48_000));
        gap.extend(synth(start.plus_frames(100), 6, 48_000, 0.5, false, 1.0));
        let got = decode(&gap, 48_000, Fps::F25);
        assert!(!got.is_empty() && got.iter().all(|t| t.h == 2 && t.m == 0 && t.s <= 5), "{got:?}");
    }

    #[test]
    fn the_hint_settles_what_the_signal_cannot_say() {
        let start = tc(0, 0, 1, 0, Fps::F30);
        let audio = synth(start, 8, 48_000, 0.5, false, 1.0);
        assert_eq!(decode(&audio, 48_000, Fps::F30)[0].fps, Fps::F30);
        assert_eq!(decode(&audio, 48_000, Fps::F2997)[0].fps, Fps::F2997, "29.97 and 30 differ by 0.1%: only the hint can tell");
        let audio = synth(tc(0, 0, 1, 0, Fps::F24), 8, 48_000, 0.5, false, 1.0);
        assert_eq!(decode(&audio, 48_000, Fps::F23976)[0].fps, Fps::F23976);
    }
}

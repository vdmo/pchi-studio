//! LTC output: the runtime puts SMPTE linear timecode on an audio channel, so other gear (a playback machine, a recorder, a desk) can
//! chase it.
//!
//! The encoder writes the 80-bit LTC frame biphase-mark coded: a transition at the start of every bit and another in the middle of a one.
//! Each frame carries the hours, minutes, seconds and frames in BCD, the drop-frame flag, the sync word, and the **polarity bit** that makes
//! the number of zero bits in a frame even so every frame starts at the same signal polarity. Output is a square wave at a fixed level (not
//! shaped to the 25 to 40 microsecond rise time the standard describes), so a strict hardware reader may want it passed through a
//! line driver or a low-pass.
//!
//! Where the numbers come from (`ltc_out` in the inputs file):
//!
//! - `"start": "01:00:00:00"`: free-running, counting from that timecode from the moment the runtime starts, at the stated frame rate.
//! - `"follow": true`: re-generate the timecode the show is receiving (from MIDI Time Code, OSC or LTC input). While that timecode is
//!   running the output carries it, a locate on the input is a locate on the output; while it is stopped or has never been heard the output
//!   is **silent**, so a chasing machine stops instead of being fed a frozen or invented number.
//!
//! Where the audio goes: `"file"` (raw audio written at the speed it plays, to a regular file or a named pipe that `aplay`, `ffmpeg` or
//! `sox` reads) or, in a build with `--features audio`, `"device"` (an audio output device). **Nothing here has been checked against a
//! real LTC reader or a real sound card**: the encoder is tested by reading its own output back with the LTC decoder in `ltc.rs`, which is a
//! test of consistency, not of agreement with another maker's reader. The polarity-bit position (27, or 59 at 25 fps) is from memory of the
//! standard, not from a copy of it.

use crate::timecode::{Fps, Tc};
use crate::Show;
use serde::Deserialize;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

/// The sync word, in the order its bits are sent: bits 64 to 79 of a frame.
const SYNC: [u8; 16] = [0, 0, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 0, 1];

#[derive(Deserialize, Clone, Debug)]
#[serde(deny_unknown_fields)]
pub struct LtcOutSpec {
    pub name: String,
    /// Write raw audio to this file or named pipe, at the speed it plays.
    #[serde(default)]
    pub file: Option<String>,
    /// Play on an audio output device whose name contains this (a build with `--features audio`).
    #[serde(default)]
    pub device: Option<String>,
    /// Free-run from this timecode (`HH:MM:SS:FF`, `;` before the frames for drop-frame).
    #[serde(default)]
    pub start: Option<String>,
    /// Re-generate the show's own timecode.
    #[serde(default)]
    pub follow: bool,
    #[serde(default = "rate48k")]
    pub sample_rate: u32,
    /// `s16le` (default), `f32le` or `u8`. (A device is always played as the device wants it.)
    #[serde(default)]
    pub format: Option<String>,
    #[serde(default = "one")]
    pub channels: usize,
    /// Which channel carries the LTC (0 is the first); the others are silent.
    #[serde(default)]
    pub channel: usize,
    /// 23.976, 24, 25, 29.97 or 30. Default 25.
    #[serde(default)]
    pub fps: Option<f64>,
    #[serde(default)]
    pub drop: bool,
    /// Peak level, 0.05 to 1.0 of full scale. Default 0.5.
    #[serde(default)]
    pub amplitude: Option<f32>,
}

fn rate48k() -> u32 {
    48_000
}
fn one() -> usize {
    1
}

impl LtcOutSpec {
    pub fn fps_value(&self) -> Option<Fps> {
        Fps::from_number(self.fps.unwrap_or(25.0), self.drop)
    }

    /// The timecode a free-running output starts from.
    pub fn start_tc(&self) -> Result<Tc, String> {
        let fps = self.fps_value().ok_or_else(|| format!("{}: fps {:?} is not one of 23.976, 24, 25, 29.97, 30", self.name, self.fps))?;
        let text = self.start.as_deref().ok_or("no start")?;
        let (h, m, s, f, written_drop) = crate::timecode::parse_fields(text)?;
        if written_drop != fps.is_drop() {
            return Err(format!("{}: start '{text}' is written {}, but the frame rate is {}", self.name, if written_drop { "with ; (drop-frame)" } else { "with : (non-drop)" }, fps.label()));
        }
        Tc::new(h, m, s, f, fps).map_err(|e| format!("{}: {e}", self.name))
    }
}

/// Check the `ltc_out` list of an inputs file.
pub fn check(specs: &[LtcOutSpec]) -> Result<(), String> {
    let mut names: Vec<String> = Vec::new();
    for s in specs {
        let name = s.name.trim();
        if name.is_empty() || name.len() > 40 || name.chars().any(|c| c.is_control()) {
            return Err("every LTC output needs a short name".into());
        }
        if names.iter().any(|n| n.eq_ignore_ascii_case(name)) {
            return Err(format!("two LTC outputs are named {name}"));
        }
        names.push(name.to_string());
        if s.file.is_some() as u8 + s.device.is_some() as u8 != 1 {
            return Err(format!("{name}: give exactly one of file or device"));
        }
        if s.start.is_some() as u8 + s.follow as u8 != 1 {
            return Err(format!("{name}: give exactly one of start (free-run from a timecode) or follow (re-generate the show's timecode)"));
        }
        if !(22_050..=192_000).contains(&s.sample_rate) {
            return Err(format!("{name}: sample_rate {} is not between 22050 and 192000", s.sample_rate));
        }
        if !matches!(s.format.as_deref(), None | Some("s16le") | Some("f32le") | Some("u8")) {
            return Err(format!("{name}: format must be s16le, f32le or u8"));
        }
        if s.channels == 0 || s.channels > 64 || s.channel >= s.channels {
            return Err(format!("{name}: channel {} does not exist in {} channel(s)", s.channel, s.channels));
        }
        if s.fps_value().is_none() {
            return Err(format!("{name}: fps {:?} is not one of 23.976, 24, 25, 29.97, 30 (with drop only at 29.97)", s.fps));
        }
        if let Some(a) = s.amplitude {
            if !(0.05..=1.0).contains(&a) {
                return Err(format!("{name}: amplitude {a} is not 0.05 to 1.0"));
            }
        }
        if s.start.is_some() {
            s.start_tc()?;
        }
        if let Some(f) = &s.file {
            if f.trim().is_empty() || f.contains('\0') {
                return Err(format!("{name}: file is empty"));
            }
        }
    }
    Ok(())
}

/// The 80 bits of an LTC frame for `t`, in the order they are sent, with the polarity bit set.
pub fn frame_bits(t: Tc) -> [u8; 80] {
    let mut b = [0u8; 80];
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
    // the polarity bit makes the count of zeros even: bit 59 at 25 fps, bit 27 at the others
    let pb = if t.fps == Fps::F25 { 59 } else { 27 };
    if b.iter().filter(|x| **x == 0).count() % 2 == 1 {
        b[pb] = 1;
    }
    b
}

/// Turns timecodes into audio, keeping the signal's polarity and its timing exact across frames.
pub struct Encoder {
    sample_rate: u32,
    amplitude: f32,
    level: f32,
    clock: f64,
    emitted: u64,
}

impl Encoder {
    pub fn new(sample_rate: u32, amplitude: f32) -> Encoder {
        Encoder { sample_rate, amplitude, level: 1.0, clock: 0.0, emitted: 0 }
    }

    fn samples_per_frame(&self, fps: Fps) -> f64 {
        self.sample_rate as f64 * fps.frame_ms() / 1000.0
    }

    /// The audio of one frame. Its length is the frame's duration in samples, carried as a fraction so it never drifts.
    pub fn frame(&mut self, t: Tc, out: &mut Vec<f32>) {
        let cell = self.samples_per_frame(t.fps) / 80.0;
        for bit in frame_bits(t) {
            self.level = -self.level; // a transition at the start of every cell
            for half in 0..2 {
                if half == 1 && bit == 1 {
                    self.level = -self.level; // and one in the middle of a one
                }
                self.clock += cell / 2.0;
                while (self.emitted as f64) < self.clock {
                    out.push(self.level * self.amplitude);
                    self.emitted += 1;
                }
            }
        }
    }

    /// One frame's worth of silence, for a source that is not running.
    pub fn silence(&mut self, fps: Fps, out: &mut Vec<f32>) {
        self.clock += self.samples_per_frame(fps);
        while (self.emitted as f64) < self.clock {
            out.push(0.0);
            self.emitted += 1;
        }
    }
}

const DAY_25: u64 = 25 * 86_400;

/// Chooses which timecode the next frame carries.
pub struct Generator {
    enc: Encoder,
    fps: Fps,
    /// Free-running: the next frame number. Following: not used.
    next: Option<u64>,
    follow: bool,
}

impl Generator {
    pub fn free_run(enc: Encoder, start: Tc) -> Generator {
        Generator { enc, fps: start.fps, next: Some(start.frames()), follow: false }
    }

    pub fn follow(enc: Encoder, fps: Fps) -> Generator {
        Generator { enc, fps, next: None, follow: true }
    }

    fn day_frames(&self) -> u64 {
        // frames in 24 hours at this rate (drop-frame skips numbers, so it is a little shorter)
        Tc::new(23, 59, 59, self.fps.nominal() - 1, self.fps).map(|t| t.frames() + 1).unwrap_or(DAY_25)
    }

    /// The next frame of audio. `heard` is the show's running timecode as `(position in ms, frame rate)` when following, `None` when it is
    /// not running. A free-running generator ignores it.
    pub fn next_frame(&mut self, heard: Option<(f64, Fps)>, out: &mut Vec<f32>) {
        if self.follow {
            match heard {
                Some((ms, fps)) => {
                    self.fps = fps;
                    let n = (ms / fps.frame_ms()).round().max(0.0) as u64;
                    self.enc.frame(Tc::from_frames(n, fps), out);
                }
                None => self.enc.silence(self.fps, out),
            }
            return;
        }
        let n = self.next.unwrap_or(0) % self.day_frames();
        self.enc.frame(Tc::from_frames(n, self.fps), out);
        self.next = Some(n + 1);
    }
}

/// Raw bytes for a run of mono samples, in the output's format, on its chosen channel (the others silent).
pub fn to_bytes(mono: &[f32], format: &str, channels: usize, channel: usize) -> Vec<u8> {
    let mut out = Vec::with_capacity(mono.len() * channels * 2);
    for s in mono {
        for c in 0..channels {
            let v = if c == channel { *s } else { 0.0 };
            match format {
                "f32le" => out.extend_from_slice(&v.to_le_bytes()),
                "u8" => out.push((v.clamp(-1.0, 1.0) * 127.0 + 128.0).round() as u8),
                _ => out.extend_from_slice(&((v.clamp(-1.0, 1.0) * 32767.0).round() as i16).to_le_bytes()),
            }
        }
    }
    out
}

/// A running output. Dropping it stops the thread.
pub struct LtcOut {
    pub spec: LtcOutSpec,
    stop: Arc<AtomicBool>,
    status: Arc<Mutex<String>>,
    frames: Arc<std::sync::atomic::AtomicU64>,
}

impl LtcOut {
    pub fn frames_written(&self) -> u64 {
        self.frames.load(Ordering::Relaxed)
    }
    /// A problem the output thread hit (a file that cannot be opened, a pipe that closed), or empty.
    pub fn status(&self) -> String {
        self.status.lock().unwrap().clone()
    }
}

impl crate::inputs::InputInfo for LtcOut {
    /// Shown in the Live tab beside the inputs: what this output is doing, and any problem it hit.
    fn json(&self) -> serde_json::Value {
        let to = match (&self.spec.file, &self.spec.device) {
            (Some(f), _) => format!("file {f}"),
            (_, Some(d)) => format!("device {d}"),
            _ => String::new(),
        };
        let problem = self.status();
        serde_json::json!({
            "kind": "ltc-out", "name": self.spec.name,
            "listen": format!("LTC out {to}"), "mode": if self.spec.follow { "follows the show timecode" } else { "free-running" },
            "frames": self.frames_written(), "problem": problem,
            "bindings": 0, "received": 0, "pressed": 0, "unlisted_sender": 0, "unbound": 0, "dropped": 0, "refused": 0,
            "heard_ms_ago": serde_json::Value::Null, "last_address": problem,
        })
    }
}

impl Drop for LtcOut {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
    }
}

/// How far ahead of real time the output is kept written.
const AHEAD_S: f64 = 0.15;

/// Start the output: a thread that writes frames at the speed they play.
pub fn start(show: &Arc<Show>, spec: LtcOutSpec) -> Result<LtcOut, String> {
    check(std::slice::from_ref(&spec))?;
    let out = LtcOut { spec: spec.clone(), stop: Arc::new(AtomicBool::new(false)), status: Arc::new(Mutex::new(String::new())), frames: Arc::new(Default::default()) };
    let (stop, status, frames) = (Arc::clone(&out.stop), Arc::clone(&out.status), Arc::clone(&out.frames));
    let fps = spec.fps_value().ok_or("no fps")?;
    let amplitude = spec.amplitude.unwrap_or(0.5);
    let weak = Arc::downgrade(show);
    if spec.device.is_some() {
        return device::start(spec, out, weak);
    }
    let path = spec.file.clone().unwrap_or_default();
    std::thread::spawn(move || {
        use std::io::Write;
        // opening a named pipe waits for a reader, so it is done here and not in the caller
        let mut file = match std::fs::OpenOptions::new().write(true).create(true).truncate(true).open(&path) {
            Ok(f) => f,
            Err(e) => {
                *status.lock().unwrap() = format!("cannot open {path}: {e}");
                return;
            }
        };
        let enc = Encoder::new(spec.sample_rate, amplitude);
        let mut gen = if spec.follow { Generator::follow(enc, fps) } else { Generator::free_run(enc, spec.start_tc().unwrap_or_else(|_| Tc::from_frames(0, fps))) };
        let format = spec.format.clone().unwrap_or_else(|| "s16le".into());
        let (t0, mut written) = (std::time::Instant::now(), 0u64);
        let mut mono: Vec<f32> = Vec::new();
        while !stop.load(Ordering::Relaxed) {
            let due = ((t0.elapsed().as_secs_f64() + AHEAD_S) * spec.sample_rate as f64) as u64;
            while written < due && !stop.load(Ordering::Relaxed) {
                mono.clear();
                let heard = if spec.follow { weak.upgrade().and_then(|s| s.timecode_running()) } else { None };
                if spec.follow && weak.upgrade().is_none() {
                    return;
                }
                gen.next_frame(heard, &mut mono);
                if let Err(e) = file.write_all(&to_bytes(&mono, &format, spec.channels, spec.channel)) {
                    *status.lock().unwrap() = format!("writing to {path} failed: {e}");
                    return;
                }
                written += mono.len() as u64;
                frames.fetch_add(1, Ordering::Relaxed);
            }
            let _ = file.flush();
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
    });
    Ok(out)
}

/// Playing on an audio output device needs the system's audio library, so it is built only with `--features audio`.
#[cfg(not(feature = "audio"))]
mod device {
    use super::*;
    pub fn start(spec: LtcOutSpec, _out: LtcOut, _show: std::sync::Weak<Show>) -> Result<LtcOut, String> {
        Err(format!("{}: this build cannot play to an audio device. Build with: cargo build --release -p pchi-studio-api --features audio (on Linux it needs libasound2-dev), or write to a file or pipe", spec.name))
    }
}

#[cfg(feature = "audio")]
mod device {
    use super::*;
    use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
    use std::collections::VecDeque;

    pub fn list() -> Vec<String> {
        cpal::default_host().output_devices().map(|d| d.filter_map(|d| d.name().ok()).collect()).unwrap_or_default()
    }

    /// Open the device and keep it playing. The audio stream is not `Send` on every platform, so a thread opens it, owns it for its life,
    /// and reports back whether opening worked.
    pub fn start(spec: LtcOutSpec, out: LtcOut, weak: std::sync::Weak<Show>) -> Result<LtcOut, String> {
        let fps = spec.fps_value().ok_or("no fps")?;
        let start_tc = if spec.follow { None } else { Some(spec.start_tc()?) };
        let (stop, frames) = (Arc::clone(&out.stop), Arc::clone(&out.frames));
        let (tx, rx) = std::sync::mpsc::channel::<Result<(), String>>();
        std::thread::spawn(move || {
            let opened = (|| -> Result<(cpal::Stream, Arc<Mutex<VecDeque<f32>>>, u32), String> {
                let want = spec.device.clone().unwrap_or_default().to_lowercase();
                let host = cpal::default_host();
                let dev = host
                    .output_devices()
                    .map_err(|e| format!("{}: cannot list audio devices: {e}", spec.name))?
                    .find(|d| d.name().map(|n| n.to_lowercase().contains(&want)).unwrap_or(false))
                    .ok_or_else(|| format!("{}: no audio output matches '{want}'. Found: {}", spec.name, list().join(", ")))?;
                let cfg = dev.default_output_config().map_err(|e| format!("{}: {e}", spec.name))?;
                let (rate, channels) = (cfg.sample_rate().0, cfg.channels() as usize);
                if spec.channel >= channels {
                    return Err(format!("{}: the device has {channels} channel(s), channel {} does not exist", spec.name, spec.channel));
                }
                let queue: Arc<Mutex<VecDeque<f32>>> = Arc::new(Mutex::new(VecDeque::new()));
                let pick = spec.channel;
                let err = |e: cpal::StreamError| eprintln!("LTC audio output error: {e}");
                macro_rules! build {
                    ($t:ty) => {{
                        let q = Arc::clone(&queue);
                        dev.build_output_stream(
                            &cfg.clone().into(),
                            move |data: &mut [$t], _: &cpal::OutputCallbackInfo| {
                                let mut q = q.lock().unwrap();
                                for frame in data.chunks_exact_mut(channels) {
                                    let s = q.pop_front().unwrap_or(0.0);
                                    for (c, slot) in frame.iter_mut().enumerate() {
                                        *slot = cpal::Sample::from_sample::<f32>(if c == pick { s } else { 0.0 });
                                    }
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
                    other => return Err(format!("{}: the device's sample format {other:?} is not supported", spec.name)),
                }
                .map_err(|e| format!("{}: cannot open the audio device: {e}", spec.name))?;
                stream.play().map_err(|e| format!("{}: {e}", spec.name))?;
                Ok((stream, queue, rate))
            })();
            let (_stream, queue, rate) = match opened {
                Ok(o) => {
                    let _ = tx.send(Ok(()));
                    o
                }
                Err(e) => {
                    let _ = tx.send(Err(e));
                    return;
                }
            };
            let enc = Encoder::new(rate, spec.amplitude.unwrap_or(0.5));
            let mut gen = match start_tc {
                Some(t) => Generator::free_run(enc, t),
                None => Generator::follow(enc, fps),
            };
            let mut mono: Vec<f32> = Vec::new();
            while !stop.load(Ordering::Relaxed) {
                while queue.lock().unwrap().len() < (rate as f64 * AHEAD_S) as usize {
                    mono.clear();
                    let heard = if spec.follow { weak.upgrade().and_then(|s| s.timecode_running()) } else { None };
                    gen.next_frame(heard, &mut mono);
                    queue.lock().unwrap().extend(mono.iter().copied());
                    frames.fetch_add(1, Ordering::Relaxed);
                }
                std::thread::sleep(std::time::Duration::from_millis(10));
            }
        });
        rx.recv().map_err(|_| "the audio thread ended before it opened the device".to_string())??;
        Ok(out)
    }
}

/// The audio output devices this machine has (empty without `--features audio`).
pub fn list_devices() -> Vec<String> {
    #[cfg(feature = "audio")]
    {
        return device::list();
    }
    #[cfg(not(feature = "audio"))]
    Vec::new()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ltc::LtcDecoder;

    fn tc(h: u32, m: u32, s: u32, f: u32, fps: Fps) -> Tc {
        Tc::new(h, m, s, f, fps).unwrap()
    }
    fn bcd(b: &[u8], from: usize, n: usize) -> u32 {
        (0..n).map(|i| (b[from + i] as u32) << i).sum()
    }
    fn spec(extra: &str) -> LtcOutSpec {
        let start = if extra.contains("\"start\"") { "" } else { r#","start":"01:00:00:00""# };
        serde_json::from_str(&format!(r#"{{"name":"Master","file":"/tmp/x.raw"{start}{extra}}}"#)).unwrap()
    }

    #[test]
    fn a_frame_carries_the_time_the_flags_the_sync_word_and_an_even_count_of_zeros() {
        let b = frame_bits(tc(12, 34, 56, 17, Fps::F25));
        assert_eq!((bcd(&b, 0, 4), bcd(&b, 8, 2)), (7, 1));
        assert_eq!((bcd(&b, 16, 4), bcd(&b, 24, 3)), (6, 5));
        assert_eq!((bcd(&b, 32, 4), bcd(&b, 40, 3)), (4, 3));
        assert_eq!((bcd(&b, 48, 4), bcd(&b, 56, 2)), (2, 1));
        assert_eq!(&b[64..], &SYNC);
        assert_eq!(b[10], 0, "not drop-frame");
        assert_eq!(frame_bits(tc(0, 1, 0, 2, Fps::F2997Df))[10], 1, "the drop-frame flag");
        for fps in [Fps::F23976, Fps::F24, Fps::F25, Fps::F2997, Fps::F2997Df, Fps::F30] {
            for n in (0..40_000u64).step_by(7) {
                let b = frame_bits(Tc::from_frames(n, fps));
                assert_eq!(b.iter().filter(|x| **x == 0).count() % 2, 0, "{fps:?} frame {n}: an even number of zeros");
            }
        }
    }

    #[test]
    fn the_polarity_bit_is_bit_59_at_25_fps_and_bit_27_at_the_others() {
        // a frame whose zeros are odd without the polarity bit gets exactly that bit set
        let find = |fps: Fps| {
            (0..2_000u64).find_map(|n| {
                let t = Tc::from_frames(n, fps);
                let b = frame_bits(t);
                let pb = if fps == Fps::F25 { 59 } else { 27 };
                (b[pb] == 1).then_some((n, pb))
            })
        };
        assert!(find(Fps::F25).is_some() && find(Fps::F30).is_some());
        // and the other candidate position is never touched
        for n in 0..2_000u64 {
            assert_eq!(frame_bits(Tc::from_frames(n, Fps::F25))[27], 0);
            assert_eq!(frame_bits(Tc::from_frames(n, Fps::F30))[59], 0);
        }
    }

    #[test]
    fn what_is_encoded_is_read_back_by_the_decoder_at_every_rate() {
        for (fps, sr) in [(Fps::F23976, 48_000), (Fps::F24, 48_000), (Fps::F25, 48_000), (Fps::F25, 44_100), (Fps::F2997, 48_000), (Fps::F2997Df, 48_000), (Fps::F30, 96_000)] {
            let start = Tc::from_frames(1_000, fps);
            let mut enc = Encoder::new(sr, 0.5);
            let mut audio = Vec::new();
            for i in 0..40 {
                enc.frame(start.plus_frames(i), &mut audio);
            }
            let got = LtcDecoder::new(sr, fps).push(&audio);
            assert!(got.len() >= 30, "{fps:?} {sr}: only {} frames read", got.len());
            for (i, g) in got.iter().enumerate() {
                assert_eq!(*g, got[0].plus_frames(i as u64), "{fps:?} {sr} frame {i}");
            }
            assert!(got[0].frames() > start.frames() && got[0].frames() <= start.plus_frames(3).frames(), "{fps:?}: starts at the start");
            assert_eq!(got[0].fps, fps);
        }
    }

    #[test]
    fn drop_frame_skips_the_numbers_it_should_across_a_minute() {
        let fps = Fps::F2997Df;
        let mut enc = Encoder::new(48_000, 0.5);
        let mut audio = Vec::new();
        let start = tc(0, 0, 59, 25, fps);
        for i in 0..20 {
            enc.frame(start.plus_frames(i), &mut audio);
        }
        let got = LtcDecoder::new(48_000, fps).push(&audio);
        let shown: Vec<String> = got.iter().map(|t| t.display()).collect();
        assert!(shown.iter().any(|s| s == "00:01:00;02"), "frame 00:01:00;02 follows 00:00:59;29: {shown:?}");
        assert!(!shown.iter().any(|s| s == "00:01:00;00" || s == "00:01:00;01"), "{shown:?}");
    }

    #[test]
    fn every_frame_starts_at_the_same_polarity_and_the_length_never_drifts() {
        for fps in [Fps::F25, Fps::F2997Df, Fps::F23976] {
            let mut enc = Encoder::new(48_000, 0.5);
            let mut audio = Vec::new();
            let mut starts = Vec::new();
            let n = 120u64;
            for i in 0..n {
                let before = audio.len();
                enc.frame(Tc::from_frames(i * 13, fps), &mut audio);
                starts.push(audio[before]);
            }
            assert!(starts.windows(2).all(|w| w[0] == w[1]), "{fps:?}: the polarity bit keeps every frame starting the same way");
            let want = 48_000.0 * fps.frame_ms() / 1000.0 * n as f64;
            assert!((audio.len() as f64 - want).abs() <= 1.0, "{fps:?}: {} samples, wanted {want}", audio.len());
        }
    }

    #[test]
    fn a_free_running_generator_counts_frames_and_wraps_at_24_hours() {
        let mut g = Generator::free_run(Encoder::new(48_000, 0.5), tc(23, 59, 59, 20, Fps::F25));
        let mut audio = Vec::new();
        for _ in 0..9 {
            g.next_frame(None, &mut audio);
        }
        let got = LtcDecoder::new(48_000, Fps::F25).push(&audio);
        let shown: Vec<String> = got.iter().map(|t| t.display()).collect();
        // the decoder reports the end of each frame: the last frame of the day (23:59:59:24) ends at 24:00:00:00, and the frame after the
        // wrap is 00:00:00:00, which ends at 00:00:00:01. A count that did not wrap would read 24:00:00:01 here.
        assert!(shown.iter().any(|s| s == "00:00:00:01"), "{shown:?}");
        assert!(!shown.iter().any(|s| s.starts_with("24:") && s != "24:00:00:00"), "{shown:?}");
    }

    #[test]
    fn following_carries_the_heard_timecode_and_goes_silent_when_it_stops() {
        let mut g = Generator::follow(Encoder::new(48_000, 0.5), Fps::F25);
        let mut audio = Vec::new();
        for i in 0..20u64 {
            g.next_frame(Some((i as f64 * 40.0 + 3_600_000.0, Fps::F25)), &mut audio);
        }
        let heard = LtcDecoder::new(48_000, Fps::F25).push(&audio);
        assert!(heard.len() >= 15);
        assert!(heard[0].frames() >= 25 * 3600, "{:?}", heard[0]);
        // a locate on the input is a locate on the output
        let mut audio = Vec::new();
        for i in 0..6u64 {
            g.next_frame(Some((i as f64 * 40.0 + 10_000.0, Fps::F25)), &mut audio);
        }
        for i in 0..6u64 {
            g.next_frame(Some((i as f64 * 40.0 + 500_000.0, Fps::F25)), &mut audio);
        }
        let got = LtcDecoder::new(48_000, Fps::F25).push(&audio);
        assert!(got.iter().any(|t| t.frames() < 25 * 20) && got.iter().any(|t| t.frames() > 25 * 400), "{got:?}");
        // not running: exactly one frame of silence per call, no signal at all
        let mut quiet = Vec::new();
        for _ in 0..25 {
            g.next_frame(None, &mut quiet);
        }
        assert!(quiet.iter().all(|s| *s == 0.0));
        assert!((quiet.len() as i64 - 48_000).abs() <= 1, "{}", quiet.len());
    }

    #[test]
    fn samples_are_written_in_the_chosen_format_on_the_chosen_channel() {
        let mono = [1.0f32, -1.0, 0.0];
        assert_eq!(to_bytes(&mono, "s16le", 1, 0), [0xFF, 0x7F, 0x01, 0x80, 0x00, 0x00]);
        assert_eq!(to_bytes(&mono, "s16le", 2, 1), [0, 0, 0xFF, 0x7F, 0, 0, 0x01, 0x80, 0, 0, 0, 0], "the other channel is silent");
        assert_eq!(to_bytes(&mono, "u8", 1, 0), [255, 1, 128]);
        assert_eq!(to_bytes(&[0.5], "f32le", 1, 0), 0.5f32.to_le_bytes());
        assert_eq!(to_bytes(&[2.0, -2.0], "s16le", 1, 0), [0xFF, 0x7F, 0x01, 0x80], "never wraps past full scale");
    }

    #[test]
    fn an_output_must_be_sound() {
        let ok = |s: &LtcOutSpec| check(std::slice::from_ref(s));
        assert!(ok(&spec("")).is_ok());
        assert!(ok(&spec(r#","fps":29.97,"drop":true,"start":"01:00:00;02""#)).is_ok() || ok(&spec(r#","fps":29.97,"drop":true"#)).is_err());
        let bad = |extra: &str| ok(&spec(extra)).err().unwrap_or_default();
        assert!(bad(r#","follow":true"#).contains("exactly one of start"));
        assert!(bad(r#","device":"x""#).contains("exactly one of file or device"));
        assert!(bad(r#","sample_rate":8000"#).contains("sample_rate"));
        assert!(bad(r#","format":"mp3""#).contains("format"));
        assert!(bad(r#","channels":2,"channel":2"#).contains("channel"));
        assert!(bad(r#","fps":50"#).contains("fps"));
        assert!(bad(r#","amplitude":0"#).contains("amplitude"));
        assert!(bad(r#","amplitude":2"#).contains("amplitude"));
        assert!(bad(r#","start":"01:61:00:00""#).contains("59"));
        assert!(bad(r#","start":"01:00:00:30""#).contains("frame 30"));
        assert!(bad(r#","start":"01:00:00;00""#).contains("non-drop") || bad(r#","start":"01:00:00;00""#).contains("drop"), "a drop-frame start needs the drop frame rate");
        assert!(serde_json::from_str::<LtcOutSpec>(r#"{"name":"A","file":"x","follow":true,"extra":1}"#).is_err(), "unknown keys are refused");
        let two: Vec<LtcOutSpec> = serde_json::from_str(r#"[{"name":"A","file":"x","follow":true},{"name":"a","file":"y","follow":true}]"#).unwrap();
        assert!(check(&two).unwrap_err().contains("two LTC outputs"));
        assert_eq!(spec(r#","fps":29.97,"drop":true,"start":"01:00:00;02""#).start_tc().unwrap().display(), "01:00:00;02");
    }

    #[cfg(not(feature = "audio"))]
    #[tokio::test]
    async fn a_device_output_in_a_build_without_audio_says_so() {
        let show = crate::Show::start(crate::tests::plain_show_for_ltc_out(), crate::EventStore::open_in_memory().unwrap()).unwrap();
        let s: LtcOutSpec = serde_json::from_str(r#"{"name":"Out","device":"Scarlett","start":"00:00:00:00"}"#).unwrap();
        let e = start(&show, s).err().unwrap();
        assert!(e.contains("--features audio"), "{e}");
    }

    /// The thread writes real frames into a real file, at the speed they play, and the decoder reads them back.
    #[tokio::test]
    async fn a_free_running_output_writes_a_file_that_reads_back_as_counting_timecode() {
        let show = crate::Show::start(crate::tests::plain_show_for_ltc_out(), crate::EventStore::open_in_memory().unwrap()).unwrap();
        let path = std::env::temp_dir().join(format!("pchi-ltc-out-{}.raw", std::process::id()));
        let s: LtcOutSpec = serde_json::from_str(&format!(r#"{{"name":"Out","file":{},"start":"01:00:00:00","fps":25,"sample_rate":48000}}"#, serde_json::to_string(path.to_str().unwrap()).unwrap())).unwrap();
        let out = start(&show, s).unwrap();
        tokio::time::sleep(std::time::Duration::from_millis(700)).await;
        assert!(out.status().is_empty(), "{}", out.status());
        let n = out.frames_written();
        let j = crate::inputs::InputInfo::json(&out);
        assert_eq!(j["kind"], "ltc-out");
        assert_eq!(j["mode"], "free-running");
        assert!(j["listen"].as_str().unwrap().starts_with("LTC out file "));
        assert!(j["frames"].as_u64().unwrap() >= n.min(1));
        assert_eq!(j["problem"], "");
        drop(out);
        let bytes = std::fs::read(&path).unwrap();
        let _ = std::fs::remove_file(&path);
        // 0.7 s plus the 0.15 s it keeps ahead: about 21 frames, written at the speed they play (not all at once)
        assert!((14..=26).contains(&n), "{n} frames in 0.7 s");
        let samples: Vec<f32> = bytes.chunks_exact(2).map(|b| i16::from_le_bytes([b[0], b[1]]) as f32 / 32768.0).collect();
        let got = LtcDecoder::new(48_000, Fps::F25).push(&samples);
        assert!(got.len() >= 10, "{} frames read", got.len());
        assert!(got.windows(2).all(|w| w[1] == w[0].plus_frames(1)));
        assert_eq!(got[0].h, 1, "starts from 01:00:00:00: {:?}", got[0]);
    }

    /// An output that cannot open its file says so, and does not take anything else down.
    #[tokio::test]
    async fn an_output_that_cannot_open_its_file_reports_it() {
        let show = crate::Show::start(crate::tests::plain_show_for_ltc_out(), crate::EventStore::open_in_memory().unwrap()).unwrap();
        let s: LtcOutSpec = serde_json::from_str(r#"{"name":"Out","file":"/nonexistent-dir/x.raw","start":"00:00:00:00"}"#).unwrap();
        let out = start(&show, s).unwrap();
        tokio::time::sleep(std::time::Duration::from_millis(200)).await;
        assert!(out.status().contains("cannot open"), "{}", out.status());
    }

    #[test]
    fn the_shared_sync_word_matches_the_decoders() {
        assert_eq!(SYNC, [0, 0, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 0, 1]);
    }
}

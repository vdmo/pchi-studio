//! Timecode: positions, frame rates (drop-frame included), the MIDI Time Code decoder, and a tracker that says whether the
//! source is running, stopped or has jumped.
//!
//! Nothing here touches the show. `Show::timecode` (in `lib.rs`) uses these to fire timecode-locked cues and `tc:` transitions.

/// The frame rates a timecode can run at.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Fps {
    F23976,
    F24,
    F25,
    /// 29.97 fps non-drop (counts 30 frames a second, runs 0.1% slow).
    F2997,
    /// 29.97 fps drop-frame (skips frame numbers 0 and 1 each minute except every tenth, so the clock tracks real time).
    F2997Df,
    F30,
}

impl Fps {
    /// Frame numbers counted in a second.
    pub fn nominal(self) -> u32 {
        match self {
            Fps::F23976 | Fps::F24 => 24,
            Fps::F25 => 25,
            Fps::F2997 | Fps::F2997Df | Fps::F30 => 30,
        }
    }

    /// Real milliseconds one frame lasts.
    pub fn frame_ms(self) -> f64 {
        match self {
            Fps::F23976 => 1001.0 / 24.0,
            Fps::F24 => 1000.0 / 24.0,
            Fps::F25 => 40.0,
            Fps::F2997 | Fps::F2997Df => 1001.0 / 30.0,
            Fps::F30 => 1000.0 / 30.0,
        }
    }

    pub fn is_drop(self) -> bool {
        self == Fps::F2997Df
    }

    pub fn label(self) -> &'static str {
        match self {
            Fps::F23976 => "23.976",
            Fps::F24 => "24",
            Fps::F25 => "25",
            Fps::F2997 => "29.97",
            Fps::F2997Df => "29.97 drop",
            Fps::F30 => "30",
        }
    }

    /// From a number a tool sends: 24, 25, 30, 29.97, 23.976. `drop` selects drop-frame for 29.97.
    pub fn from_number(n: f64, drop: bool) -> Option<Fps> {
        let near = |a: f64| (n - a).abs() < 0.01;
        Some(if near(24.0) {
            Fps::F24
        } else if near(25.0) {
            Fps::F25
        } else if near(30.0) {
            Fps::F30
        } else if near(29.97) {
            if drop { Fps::F2997Df } else { Fps::F2997 }
        } else if near(23.976) {
            Fps::F23976
        } else {
            return None;
        })
    }
}

/// A timecode value: hours, minutes, seconds, frames, at a frame rate.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Tc {
    pub h: u32,
    pub m: u32,
    pub s: u32,
    pub f: u32,
    pub fps: Fps,
}

impl Tc {
    pub fn new(h: u32, m: u32, s: u32, f: u32, fps: Fps) -> Result<Tc, String> {
        if m > 59 || s > 59 {
            return Err(format!("{h:02}:{m:02}:{s:02}:{f:02}: minutes and seconds go up to 59"));
        }
        if f >= fps.nominal() {
            return Err(format!("frame {f} does not exist at {} fps (0 to {})", fps.label(), fps.nominal() - 1));
        }
        if fps.is_drop() && s == 0 && f < 2 && m % 10 != 0 {
            return Err(format!("{h:02}:{m:02}:{s:02};{f:02} does not exist in drop-frame: frames 00 and 01 are skipped at the start of this minute"));
        }
        Ok(Tc { h, m, s, f, fps })
    }

    /// How many frames since 00:00:00:00 (drop-frame aware).
    pub fn frames(&self) -> u64 {
        let nominal = self.fps.nominal() as u64;
        let secs = self.h as u64 * 3600 + self.m as u64 * 60 + self.s as u64;
        let mut n = secs * nominal + self.f as u64;
        if self.fps.is_drop() {
            let minutes = self.h as u64 * 60 + self.m as u64;
            n -= 2 * (minutes - minutes / 10);
        }
        n
    }

    /// The inverse of `frames`.
    pub fn from_frames(mut n: u64, fps: Fps) -> Tc {
        let nominal = fps.nominal() as u64;
        if fps.is_drop() {
            let d = n / 17_982;
            let m = n % 17_982;
            n += 18 * d + if m < 2 { 0 } else { 2 * ((m - 2) / 1_798) };
        }
        let f = (n % nominal) as u32;
        let secs = n / nominal;
        Tc { h: (secs / 3600) as u32, m: ((secs / 60) % 60) as u32, s: (secs % 60) as u32, f, fps }
    }

    /// Position in real milliseconds.
    pub fn to_ms(&self) -> f64 {
        self.frames() as f64 * self.fps.frame_ms()
    }

    /// `01:02:03:04`, with `;` before the frames for drop-frame.
    pub fn display(&self) -> String {
        format!("{:02}:{:02}:{:02}{}{:02}", self.h, self.m, self.s, if self.fps.is_drop() { ';' } else { ':' }, self.f)
    }

    pub fn plus_frames(&self, n: u64) -> Tc {
        Tc::from_frames(self.frames() + n, self.fps)
    }
}

/// The fields of a written timecode, `HH:MM:SS:FF` or `HH:MM:SS;FF` (drop-frame), with no frame rate yet.
pub fn parse_fields(text: &str) -> Result<(u32, u32, u32, u32, bool), String> {
    let t = text.trim();
    let drop = t.rfind(';').is_some();
    let parts: Vec<&str> = t.split([':', ';']).collect();
    if parts.len() != 4 || parts.iter().any(|p| p.is_empty() || p.len() > 3 || !p.chars().all(|c| c.is_ascii_digit())) {
        return Err(format!("'{text}' is not a timecode: write it HH:MM:SS:FF, for example 01:02:03:12"));
    }
    let n: Vec<u32> = parts.iter().map(|p| p.parse().unwrap_or(0)).collect();
    if n[1] > 59 || n[2] > 59 {
        return Err(format!("'{text}': minutes and seconds go up to 59"));
    }
    Ok((n[0], n[1], n[2], n[3], drop))
}

/// A written timecode in real milliseconds at the live frame rate. `;` makes 29.97 drop-frame.
pub fn written_to_ms(text: &str, live: Fps) -> Result<f64, String> {
    let (h, m, s, f, drop) = parse_fields(text)?;
    let fps = if drop && live == Fps::F2997 { Fps::F2997Df } else { live };
    // a label the live rate cannot show (frame 29 at 25 fps) still reads as that many frames: the show file is rate-neutral
    let frames_in_sec = fps.nominal();
    let f = f.min(frames_in_sec - 1);
    Ok(Tc::new(h, m, s, f, fps).or_else(|_| Tc::new(h, m, s, f.max(2), fps))?.to_ms())
}

/// MIDI Time Code: eight quarter-frame messages make one position, sent as the frame two before the last message arrives.
#[derive(Default)]
pub struct MtcDecoder {
    nibbles: [u8; 8],
    seen: u8,
    last: Option<u8>,
}

impl MtcDecoder {
    /// Feed the data byte of a quarter-frame message (`F1 dd`). Returns the position when a full, in-order sequence ends.
    pub fn quarter_frame(&mut self, data: u8) -> Option<Tc> {
        let piece = (data >> 4) & 7;
        let value = data & 0x0F;
        let in_order = match self.last {
            None => piece == 0,
            Some(l) => piece == (l + 1) % 8,
        };
        if piece == 0 {
            self.seen = 0;
        } else if !in_order {
            // a lost message: wait for the next piece 0
            self.seen = 0;
            self.last = None;
            return None;
        }
        self.nibbles[piece as usize] = value;
        self.seen |= 1 << piece;
        self.last = Some(piece);
        if piece == 7 && self.seen == 0xFF {
            self.seen = 0;
            let n = &self.nibbles;
            let (f, s, m, h) = (n[0] | n[1] << 4, n[2] | n[3] << 4, n[4] | n[5] << 4, n[6] | (n[7] & 1) << 4);
            let fps = match (n[7] >> 1) & 3 {
                0 => Fps::F24,
                1 => Fps::F25,
                2 => Fps::F2997Df,
                _ => Fps::F30,
            };
            // by the time the eighth message arrives, two frames have gone by since the first
            return Tc::new(h as u32, m as u32, s as u32, f as u32, fps).ok().map(|t| t.plus_frames(2));
        }
        None
    }

    /// A full-frame message (`F0 7F <device> 01 01 hh mm ss ff F7`): sent when MTC locates, with the exact position.
    pub fn full_frame(sysex: &[u8]) -> Option<Tc> {
        if sysex.len() < 10 || sysex[0] != 0xF0 || sysex[1] != 0x7F || sysex[3] != 0x01 || sysex[4] != 0x01 {
            return None;
        }
        let hh = sysex[5];
        let fps = match (hh >> 5) & 3 {
            0 => Fps::F24,
            1 => Fps::F25,
            2 => Fps::F2997Df,
            _ => Fps::F30,
        };
        Tc::new((hh & 0x1F) as u32, sysex[6] as u32, sysex[7] as u32, sysex[8] as u32, fps).ok()
    }
}

/// How a new position relates to the one before it.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Step {
    /// The first position heard: nothing to compare with.
    First,
    /// It moved forward by a normal amount: everything in `from..to` has just passed.
    Forward { from: f64, to: f64 },
    /// It did not move.
    Same,
    /// It moved by more than half a second, or backwards: a locate, not a play.
    Jump { from: f64, to: f64 },
}

/// A jump is anything bigger than this, or any step backwards.
pub const JUMP_MS: f64 = 500.0;
/// The clock counts as running while it has moved within this long.
pub const RUNNING_MS: i64 = 400;

#[derive(Default)]
pub struct Clock {
    last: Option<(f64, Fps)>,
    advanced_at: i64,
    heard_at: i64,
    pub source: String,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Status {
    /// "none" (never heard), "running" or "stopped".
    pub state: &'static str,
    pub position_ms: f64,
    pub fps: Option<Fps>,
    pub heard_ms_ago: Option<i64>,
}

impl Clock {
    pub fn observe(&mut self, pos_ms: f64, fps: Fps, now: i64, source: &str) -> Step {
        self.source = source.to_string();
        self.heard_at = now;
        let step = match self.last {
            None => Step::First,
            Some((prev, _)) if (pos_ms - prev).abs() < 0.5 => Step::Same,
            Some((prev, _)) if pos_ms < prev || pos_ms - prev > JUMP_MS => Step::Jump { from: prev, to: pos_ms },
            Some((prev, _)) => Step::Forward { from: prev, to: pos_ms },
        };
        if step != Step::Same {
            self.advanced_at = now;
        }
        self.last = Some((pos_ms, fps));
        step
    }

    pub fn fps(&self) -> Option<Fps> {
        self.last.map(|(_, f)| f)
    }

    pub fn position(&self) -> Option<f64> {
        self.last.map(|(p, _)| p)
    }

    pub fn status(&self, now: i64) -> Status {
        match self.last {
            None => Status { state: "none", position_ms: 0.0, fps: None, heard_ms_ago: None },
            Some((p, f)) => Status { state: if now - self.advanced_at <= RUNNING_MS { "running" } else { "stopped" }, position_ms: p, fps: Some(f), heard_ms_ago: Some((now - self.heard_at).max(0)) },
        }
    }
}

/// The eight quarter-frame data bytes that announce `t` (what a device sends over two frames). For tests.
#[cfg(test)]
pub fn quarter_frames(t: Tc) -> [u8; 8] {
    let rate = match t.fps {
        Fps::F24 | Fps::F23976 => 0,
        Fps::F25 => 1,
        Fps::F2997Df => 2,
        _ => 3,
    };
    let n = [t.f & 0xF, t.f >> 4, t.s & 0xF, t.s >> 4, t.m & 0xF, t.m >> 4, t.h & 0xF, (t.h >> 4) | rate << 1];
    std::array::from_fn(|i| (i as u8) << 4 | n[i] as u8)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tc(h: u32, m: u32, s: u32, f: u32, fps: Fps) -> Tc {
        Tc::new(h, m, s, f, fps).unwrap()
    }

    #[test]
    fn frames_and_milliseconds_agree_at_every_rate() {
        let near = |a: f64, b: f64| assert!((a - b).abs() < 1e-6, "{a} vs {b}");
        near(tc(0, 0, 1, 0, Fps::F25).to_ms(), 1000.0);
        near(tc(0, 0, 0, 12, Fps::F24).to_ms(), 500.0);
        near(tc(1, 0, 0, 0, Fps::F30).to_ms(), 3_600_000.0);
        assert!((tc(0, 0, 1, 0, Fps::F2997).to_ms() - 1001.0).abs() < 1e-6, "29.97 non-drop runs 0.1% slow");
        assert!((tc(0, 0, 1, 0, Fps::F23976).to_ms() - 1000.0 * 1001.0 / 1000.0).abs() < 1e-3);
        for fps in [Fps::F24, Fps::F25, Fps::F30, Fps::F2997, Fps::F2997Df, Fps::F23976] {
            let t = tc(2, 41, 7, 5.min(fps.nominal() - 1), fps);
            assert_eq!(Tc::from_frames(t.frames(), fps), t, "{fps:?}");
        }
    }

    #[test]
    fn drop_frame_skips_two_frame_numbers_a_minute_except_every_tenth() {
        let df = Fps::F2997Df;
        assert_eq!(tc(0, 0, 59, 29, df).plus_frames(1).display(), "00:01:00;02", "after 00:00:59;29 comes 00:01:00;02");
        assert_eq!(tc(0, 9, 59, 29, df).plus_frames(1).display(), "00:10:00;00", "every tenth minute keeps frames 00 and 01");
        // an hour of drop-frame timecode is an hour of clock time, to within a few milliseconds
        assert!((tc(1, 0, 0, 0, df).to_ms() - 3_600_000.0).abs() < 5.0, "{}", tc(1, 0, 0, 0, df).to_ms());
        assert!(Tc::new(0, 1, 0, 0, df).is_err() && Tc::new(0, 1, 0, 1, df).is_err(), "those frames do not exist");
        assert!(Tc::new(0, 10, 0, 0, df).is_ok());
    }

    #[test]
    fn written_timecode_is_checked() {
        assert_eq!(parse_fields("01:02:03:04").unwrap(), (1, 2, 3, 4, false));
        assert_eq!(parse_fields("01:02:03;04").unwrap(), (1, 2, 3, 4, true));
        for bad in ["", "1:2:3", "01:02:03", "aa:bb:cc:dd", "01:61:00:00", "01:00:61:00", "01:02:03:04:05"] {
            assert!(parse_fields(bad).is_err(), "{bad}");
        }
        assert!((written_to_ms("00:00:02:00", Fps::F25).unwrap() - 2000.0).abs() < 1e-6);
        assert!((written_to_ms("00:00:02:12", Fps::F24).unwrap() - 2500.0).abs() < 1e-6);
    }

    #[test]
    fn eight_quarter_frames_make_a_position_two_frames_on() {
        let sent = tc(1, 2, 3, 10, Fps::F25);
        let mut d = MtcDecoder::default();
        let got: Vec<Option<Tc>> = quarter_frames(sent).iter().map(|b| d.quarter_frame(*b)).collect();
        assert!(got[..7].iter().all(|g| g.is_none()), "nothing until the last piece");
        assert_eq!(got[7], Some(tc(1, 2, 3, 12, Fps::F25)), "the frame at piece 0, plus the two that passed");
        // the next sequence decodes too
        let again: Vec<Option<Tc>> = quarter_frames(sent.plus_frames(2)).iter().map(|b| d.quarter_frame(*b)).collect();
        assert_eq!(again[7], Some(tc(1, 2, 3, 14, Fps::F25)));
        // every rate comes through with its own frame rate
        for fps in [Fps::F24, Fps::F25, Fps::F2997Df, Fps::F30] {
            let t = tc(0, 10, 0, 0, fps);
            let mut d = MtcDecoder::default();
            let out = quarter_frames(t).iter().filter_map(|b| d.quarter_frame(*b)).last().unwrap();
            assert_eq!(out, t.plus_frames(2), "{fps:?}");
        }
    }

    #[test]
    fn a_lost_quarter_frame_is_not_guessed() {
        let sent = tc(0, 0, 5, 0, Fps::F25);
        let q = quarter_frames(sent);
        let mut d = MtcDecoder::default();
        let out: Vec<Option<Tc>> = [q[0], q[1], q[3], q[4], q[5], q[6], q[7]].iter().map(|b| d.quarter_frame(*b)).collect();
        assert!(out.iter().all(|o| o.is_none()), "piece 2 never arrived, so no position");
        // and it recovers on the next whole sequence
        assert!(quarter_frames(sent.plus_frames(2)).iter().filter_map(|b| d.quarter_frame(*b)).last().is_some());
        // joining mid-sequence waits for piece 0
        let mut d = MtcDecoder::default();
        assert!(q[3..].iter().all(|b| d.quarter_frame(*b).is_none()));
    }

    #[test]
    fn a_full_frame_message_gives_the_exact_position() {
        let msg = [0xF0, 0x7F, 0x7F, 0x01, 0x01, 0x20 | 1, 2, 3, 4, 0xF7]; // 25 fps, 01:02:03:04
        assert_eq!(MtcDecoder::full_frame(&msg), Some(tc(1, 2, 3, 4, Fps::F25)));
        assert_eq!(MtcDecoder::full_frame(&[0xF0, 0x7E, 0x7F, 0x01, 0x01, 0, 0, 0, 0, 0xF7]), None, "not a timecode message");
        assert_eq!(MtcDecoder::full_frame(&[0xF0, 0x7F]), None);
    }

    #[test]
    fn the_clock_tells_play_from_a_locate_and_running_from_stopped() {
        let mut c = Clock::default();
        assert_eq!(c.status(0).state, "none");
        assert_eq!(c.observe(1000.0, Fps::F25, 0, "midi"), Step::First);
        assert_eq!(c.observe(1040.0, Fps::F25, 40, "midi"), Step::Forward { from: 1000.0, to: 1040.0 });
        assert_eq!(c.observe(1040.0, Fps::F25, 60, "midi"), Step::Same);
        assert_eq!(c.status(100).state, "running");
        assert_eq!(c.status(1000).state, "stopped", "no movement for a while: stopped (or the cable is out: the clock cannot tell)");
        assert_eq!(c.observe(9000.0, Fps::F25, 1100, "midi"), Step::Jump { from: 1040.0, to: 9000.0 });
        assert_eq!(c.observe(2000.0, Fps::F25, 1140, "midi"), Step::Jump { from: 9000.0, to: 2000.0 });
        assert_eq!(c.status(1150).state, "running");
        assert_eq!(c.source, "midi");
    }
}

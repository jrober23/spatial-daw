//! Musical time.
//!
//! The project is measured in *ticks* with 960 ticks per quarter note, the
//! same resolution as a typical MIDI sequencer. Audio is measured in frames;
//! the conversion only depends on tempo and the current sample rate.

use serde::{Deserialize, Serialize};

/// Pulses per quarter note. 960 divides cleanly into triplets and 32nds.
pub const TICKS_PER_BEAT: i64 = 960;

/// A musical time signature.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct TimeSignature {
    pub numerator: u8,
    pub denominator: u8,
}

impl Default for TimeSignature {
    fn default() -> Self {
        Self {
            numerator: 4,
            denominator: 4,
        }
    }
}

impl TimeSignature {
    pub const FOUR_FOUR: Self = Self {
        numerator: 4,
        denominator: 4,
    };

    pub fn new(numerator: u8, denominator: u8) -> Self {
        Self {
            numerator: numerator.max(1),
            denominator: denominator.max(1),
        }
    }

    /// Ticks in one counted beat (in 6/8 a beat is an eighth note).
    pub fn ticks_per_beat(&self) -> i64 {
        TICKS_PER_BEAT * 4 / self.denominator as i64
    }

    /// Ticks in one bar.
    pub fn ticks_per_bar(&self) -> i64 {
        self.ticks_per_beat() * self.numerator as i64
    }

    pub fn label(&self) -> String {
        format!("{}/{}", self.numerator, self.denominator)
    }
}

/// Seconds per tick at a given tempo.
pub fn seconds_per_tick(bpm: f64) -> f64 {
    60.0 / (bpm.max(1.0) * TICKS_PER_BEAT as f64)
}

/// Frames per tick at a given tempo and sample rate.
pub fn samples_per_tick(sample_rate: f64, bpm: f64) -> f64 {
    sample_rate * seconds_per_tick(bpm)
}

pub fn ticks_to_samples(ticks: f64, sample_rate: f64, bpm: f64) -> f64 {
    ticks * samples_per_tick(sample_rate, bpm)
}

pub fn samples_to_ticks(samples: f64, sample_rate: f64, bpm: f64) -> f64 {
    let spt = samples_per_tick(sample_rate, bpm);
    if spt <= f64::EPSILON {
        0.0
    } else {
        samples / spt
    }
}

pub fn ticks_to_seconds(ticks: f64, bpm: f64) -> f64 {
    ticks * seconds_per_tick(bpm)
}

pub fn seconds_to_ticks(seconds: f64, bpm: f64) -> f64 {
    seconds / seconds_per_tick(bpm)
}

/// Formats a position as `bar.beat.tick` using the project time signature.
///
/// Bars and beats are one-based, ticks are zero-padded to three digits.
pub fn format_position(tick: i64, sig: TimeSignature) -> String {
    let tick = tick.max(0);
    let bar_ticks = sig.ticks_per_bar().max(1);
    let beat_ticks = sig.ticks_per_beat().max(1);
    let bar = tick / bar_ticks;
    let in_bar = tick % bar_ticks;
    let beat = in_bar / beat_ticks;
    let rem = in_bar % beat_ticks;
    format!("{}.{}.{:03}", bar + 1, beat + 1, rem)
}

/// Formats a length in ticks as `bar.beat.tick` collapsed to bars/beats.
pub fn format_length(ticks: i64, sig: TimeSignature) -> String {
    let bar_ticks = sig.ticks_per_bar().max(1);
    let beat_ticks = sig.ticks_per_beat().max(1);
    let ticks = ticks.max(0);
    let bars = ticks / bar_ticks;
    let beats = (ticks % bar_ticks) / beat_ticks;
    if bars > 0 {
        format!("{bars} bar{}, {beats} beat{}", plural(bars), plural(beats))
    } else {
        format!("{beats} beat{}", plural(beats))
    }
}

fn plural(n: i64) -> &'static str {
    if n == 1 {
        ""
    } else {
        "s"
    }
}

/// Grid resolution used for snapping edits.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum Grid {
    Off,
    Bar,
    Beat,
    #[default]
    Sixteenth,
    Eighth,
    ThirtySecond,
    QuarterTriplet,
    EighthTriplet,
}

impl Grid {
    pub const ALL: [Grid; 8] = [
        Grid::Bar,
        Grid::Beat,
        Grid::Eighth,
        Grid::Sixteenth,
        Grid::ThirtySecond,
        Grid::QuarterTriplet,
        Grid::EighthTriplet,
        Grid::Off,
    ];

    pub fn label(self) -> &'static str {
        match self {
            Grid::Off => "Off",
            Grid::Bar => "1 Bar",
            Grid::Beat => "1/4",
            Grid::Eighth => "1/8",
            Grid::Sixteenth => "1/16",
            Grid::ThirtySecond => "1/32",
            Grid::QuarterTriplet => "1/4T",
            Grid::EighthTriplet => "1/8T",
        }
    }

    /// Grid size in ticks for the given time signature.
    pub fn ticks(self, sig: TimeSignature) -> i64 {
        let quarter = TICKS_PER_BEAT;
        match self {
            Grid::Off => 1,
            Grid::Bar => sig.ticks_per_bar(),
            Grid::Beat => sig.ticks_per_beat(),
            Grid::Eighth => quarter / 2,
            Grid::Sixteenth => quarter / 4,
            Grid::ThirtySecond => quarter / 8,
            Grid::QuarterTriplet => quarter / 3,
            Grid::EighthTriplet => quarter / 6,
        }
    }
}

/// Snaps a tick position to the nearest grid line.
pub fn snap(tick: i64, grid: Grid, sig: TimeSignature) -> i64 {
    if grid == Grid::Off {
        return tick;
    }
    let step = grid.ticks(sig).max(1);
    let snapped = (tick as f64 / step as f64).round() as i64 * step;
    snapped.max(0)
}

/// Snaps a tick position down to the grid line at or before `tick`.
pub fn snap_floor(tick: i64, grid: Grid, sig: TimeSignature) -> i64 {
    if grid == Grid::Off {
        return tick;
    }
    let step = grid.ticks(sig).max(1);
    (tick / step) * step
}

/// Number of whole grid steps between two positions.
pub fn grid_steps(ticks: i64, grid: Grid, sig: TimeSignature) -> i64 {
    if ticks <= 0 {
        return 0;
    }
    let step = grid.ticks(sig).max(1);
    ticks / step
}

/// Converts a MIDI-style beat position to ticks.
pub fn beats_to_ticks(beats: f64) -> i64 {
    (beats * TICKS_PER_BEAT as f64).round() as i64
}

/// Converts ticks to beats (quarter notes).
pub fn ticks_to_beats(ticks: i64) -> f64 {
    ticks as f64 / TICKS_PER_BEAT as f64
}

/// Rounds a tick position to the nearest 1/16 note.
pub fn quantize_sixteenth(tick: i64) -> i64 {
    snap(tick, Grid::Sixteenth, TimeSignature::FOUR_FOUR)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tick_sample_round_trip() {
        let sr = 48_000.0;
        let bpm = 128.0;
        let ticks = 7_680.0; // two bars of 4/4
        let samples = ticks_to_samples(ticks, sr, bpm);
        let back = samples_to_ticks(samples, sr, bpm);
        assert!((back - ticks).abs() < 1e-6);
        // Two bars at 128 bpm = 3.75 seconds.
        assert!((samples / sr - 3.75).abs() < 1e-9);
    }

    #[test]
    fn position_formatting() {
        let sig = TimeSignature::FOUR_FOUR;
        assert_eq!(format_position(0, sig), "1.1.000");
        assert_eq!(format_position(TICKS_PER_BEAT, sig), "1.2.000");
        assert_eq!(format_position(TICKS_PER_BEAT * 4, sig), "2.1.000");
        assert_eq!(format_position(TICKS_PER_BEAT * 5 + 120, sig), "2.2.120");
    }

    #[test]
    fn snapping_is_stable() {
        let sig = TimeSignature::FOUR_FOUR;
        assert_eq!(snap(100, Grid::Beat, sig), 0);
        assert_eq!(snap(500, Grid::Beat, sig), 960);
        assert_eq!(snap(100, Grid::Off, sig), 100);
        assert_eq!(snap_floor(1000, Grid::Beat, sig), 960);
        assert_eq!(snap(119, Grid::Sixteenth, sig), 0);
        assert_eq!(snap(121, Grid::Sixteenth, sig), 240);
    }

    #[test]
    fn odd_meter_grids() {
        let sig = TimeSignature::new(7, 8);
        assert_eq!(sig.ticks_per_beat(), 480);
        assert_eq!(sig.ticks_per_bar(), 3360);
        assert_eq!(Grid::Bar.ticks(sig), 3360);
        assert_eq!(format_position(3360, sig), "2.1.000");
    }
}

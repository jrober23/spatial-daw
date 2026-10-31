//! Spatial mix theory, after the "energy field" approach popularised by
//! Dave Gibson's *The Art of Mixing*.
//!
//! The mixer visualises a mix as a three-dimensional field around the
//! listener:
//!
//! * **X — pan**: left and right of the listener.
//! * **Y — frequency**: low frequencies at the floor, highs at the ceiling.
//! * **Z — depth**: loud, important elements sit close to the listener;
//!   quiet, supporting elements sit further back.
//!
//! The size of an element in the field is its perceived level. The mix is
//! "right" when the important elements are large and central/front, and the
//! supporting elements sit around and behind them without masking each other.
//! The analysis below compares a mix against a target image for a style, and
//! produces plain-language notes about how far each element is from its target
//! and which way to move it.

use std::collections::HashMap;

use serde::{Deserialize, Serialize};

use crate::ids::TrackId;
use crate::model::{Project, Track, DEPTH_RANGE_DB};

pub const FREQ_MIN_HZ: f32 = 20.0;
pub const FREQ_MAX_HZ: f32 = 20_000.0;

/// Converts a fader value into the 0 (far) .. 1 (near) depth axis.
pub fn db_to_depth(db: f32) -> f32 {
    ((db + DEPTH_RANGE_DB) / DEPTH_RANGE_DB).clamp(0.0, 1.0)
}

/// Inverse of [`db_to_depth`].
pub fn depth_to_db(depth: f32) -> f32 {
    depth.clamp(0.0, 1.0) * DEPTH_RANGE_DB - DEPTH_RANGE_DB
}

/// Maps a frequency to 0 (20 Hz) .. 1 (20 kHz) on a logarithmic scale.
pub fn frequency_to_height(hz: f32) -> f32 {
    let hz = hz.clamp(FREQ_MIN_HZ, FREQ_MAX_HZ);
    (hz.ln() - FREQ_MIN_HZ.ln()) / (FREQ_MAX_HZ.ln() - FREQ_MIN_HZ.ln())
}

pub fn height_to_frequency(height: f32) -> f32 {
    let height = height.clamp(0.0, 1.0);
    (FREQ_MIN_HZ.ln() + height * (FREQ_MAX_HZ.ln() - FREQ_MIN_HZ.ln())).exp()
}

/// Maps a measured RMS level to a visual radius. Quiet material is still
/// visible, loud material fills its region of the field.
pub fn rms_to_size(rms_db: f32) -> f32 {
    let t = ((rms_db + 54.0) / 54.0).clamp(0.0, 1.0);
    0.18 + 0.82 * t.powf(0.65)
}

/// The musical job a track does. Roles drive target placement.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum ElementRole {
    Kick,
    Drums,
    Percussion,
    Bass,
    Chords,
    Pad,
    Keys,
    Guitar,
    Lead,
    Vocals,
    Fx,
    #[default]
    Other,
}

impl ElementRole {
    pub const ALL: [ElementRole; 12] = [
        ElementRole::Kick,
        ElementRole::Drums,
        ElementRole::Percussion,
        ElementRole::Bass,
        ElementRole::Chords,
        ElementRole::Pad,
        ElementRole::Keys,
        ElementRole::Guitar,
        ElementRole::Lead,
        ElementRole::Vocals,
        ElementRole::Fx,
        ElementRole::Other,
    ];

    pub fn label(self) -> &'static str {
        match self {
            ElementRole::Kick => "Kick",
            ElementRole::Drums => "Drums",
            ElementRole::Percussion => "Percussion",
            ElementRole::Bass => "Bass",
            ElementRole::Chords => "Chords",
            ElementRole::Pad => "Pad",
            ElementRole::Keys => "Keys",
            ElementRole::Guitar => "Guitar",
            ElementRole::Lead => "Lead",
            ElementRole::Vocals => "Vocals",
            ElementRole::Fx => "FX",
            ElementRole::Other => "Other",
        }
    }

    pub fn short(self) -> &'static str {
        match self {
            ElementRole::Kick => "K",
            ElementRole::Drums => "D",
            ElementRole::Percussion => "P",
            ElementRole::Bass => "B",
            ElementRole::Chords => "C",
            ElementRole::Pad => "Pd",
            ElementRole::Keys => "Ky",
            ElementRole::Guitar => "G",
            ElementRole::Lead => "L",
            ElementRole::Vocals => "V",
            ElementRole::Fx => "X",
            ElementRole::Other => "O",
        }
    }

    /// Rough nominal centre frequency, used for targets and for sorting.
    pub fn nominal_hz(self) -> f32 {
        match self {
            ElementRole::Kick => 55.0,
            ElementRole::Drums => 220.0,
            ElementRole::Percussion => 3_500.0,
            ElementRole::Bass => 90.0,
            ElementRole::Chords => 650.0,
            ElementRole::Pad => 1_400.0,
            ElementRole::Keys => 950.0,
            ElementRole::Guitar => 1_100.0,
            ElementRole::Lead => 2_200.0,
            ElementRole::Vocals => 1_150.0,
            ElementRole::Fx => 4_500.0,
            ElementRole::Other => 800.0,
        }
    }

    /// Best-effort role for a newly created or imported track.
    pub fn guess(name: &str, is_drum: bool) -> Self {
        let n = name.to_ascii_lowercase();
        let has = |needles: &[&str]| needles.iter().any(|n2| n.contains(n2));
        if has(&["kick", "bd", "808"]) {
            ElementRole::Kick
        } else if has(&["snare", "drum", "tom", "beat"]) || is_drum {
            ElementRole::Drums
        } else if has(&["hat", "perc", "clap", "shaker", "rim", "cymbal", "crash"]) {
            ElementRole::Percussion
        } else if has(&["bass", "sub"]) {
            ElementRole::Bass
        } else if has(&["vox", "vocal", "voice", "choir", "sing"]) {
            ElementRole::Vocals
        } else if has(&["gtr", "guitar"]) {
            ElementRole::Guitar
        } else if has(&["chord", "stab", "organ", "rhodes", "piano", "keys"]) {
            ElementRole::Keys
        } else if has(&["pad", "string", "atmos", "ambien"]) {
            ElementRole::Pad
        } else if has(&["lead", "melod", "arp", "hook", "synth"]) {
            ElementRole::Lead
        } else if has(&["fx", "riser", "sweep", "noise"]) {
            ElementRole::Fx
        } else {
            ElementRole::Other
        }
    }
}

/// One target element in a style's reference image.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct StyleTarget {
    pub role: ElementRole,
    /// -1 (left) .. 1 (right).
    pub pan: f32,
    /// 0 (far) .. 1 (near).
    pub depth: f32,
    pub frequency_hz: f32,
}

impl StyleTarget {
    pub const fn new(role: ElementRole, pan: f32, depth: f32, frequency_hz: f32) -> Self {
        Self {
            role,
            pan,
            depth,
            frequency_hz,
        }
    }

    pub fn position(self) -> SpatialPosition {
        SpatialPosition {
            pan: self.pan,
            depth: self.depth,
            frequency_hz: self.frequency_hz,
        }
    }

    pub fn level_db(self) -> f32 {
        depth_to_db(self.depth)
    }

    /// Target radius: near elements are large, far elements are small.
    pub fn size(self) -> f32 {
        0.32 + 0.6 * self.depth
    }
}

/// Reference images for common production styles.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum MixStyle {
    #[default]
    RockPop,
    Electronic,
    HipHop,
    Jazz,
    Orchestral,
    VocalBallad,
}

impl MixStyle {
    pub const ALL: [MixStyle; 6] = [
        MixStyle::RockPop,
        MixStyle::Electronic,
        MixStyle::HipHop,
        MixStyle::Jazz,
        MixStyle::Orchestral,
        MixStyle::VocalBallad,
    ];

    pub fn label(self) -> &'static str {
        match self {
            MixStyle::RockPop => "Rock / Pop",
            MixStyle::Electronic => "Electronic",
            MixStyle::HipHop => "Hip-Hop",
            MixStyle::Jazz => "Jazz",
            MixStyle::Orchestral => "Orchestral",
            MixStyle::VocalBallad => "Vocal Ballad",
        }
    }

    pub fn description(self) -> &'static str {
        match self {
            MixStyle::RockPop => {
                "Lead vocal dead centre and close; kick and bass low and forward; \
                 guitars spread wide; drums behind the vocal."
            }
            MixStyle::Electronic => {
                "A big low end on the floor of the field; pads and atmosphere in the \
                 back corners; lead centred and close; percussion wide and bright."
            }
            MixStyle::HipHop => {
                "Kick and bass carry the front-centre; vocals sit on top of them; \
                 supporting keys and pads step back and to the sides."
            }
            MixStyle::Jazz => {
                "A natural ensemble: drums and bass in the middle distance, piano \
                 slightly left, horns in front and centre."
            }
            MixStyle::Orchestral => {
                "Strings spread wide and behind; brass further back; soloist and \
                 woodwinds in front; timpani low and deep."
            }
            MixStyle::VocalBallad => {
                "The voice is the field: centred, largest and closest. Everything \
                 else forms a soft arc behind it."
            }
        }
    }

    pub fn targets(self) -> &'static [StyleTarget] {
        match self {
            MixStyle::RockPop => &ROCK_POP,
            MixStyle::Electronic => &ELECTRONIC,
            MixStyle::HipHop => &HIP_HOP,
            MixStyle::Jazz => &JAZZ,
            MixStyle::Orchestral => &ORCHESTRAL,
            MixStyle::VocalBallad => &VOCAL_BALLAD,
        }
    }

    pub fn target_for(self, role: ElementRole) -> Option<StyleTarget> {
        self.targets().iter().copied().find(|t| t.role == role)
    }
}

const ROCK_POP: [StyleTarget; 11] = [
    StyleTarget::new(ElementRole::Kick, 0.0, 1.0, 55.0),
    StyleTarget::new(ElementRole::Drums, 0.08, 0.84, 210.0),
    StyleTarget::new(ElementRole::Percussion, -0.55, 0.68, 2_800.0),
    StyleTarget::new(ElementRole::Bass, 0.0, 0.95, 95.0),
    StyleTarget::new(ElementRole::Chords, -0.42, 0.55, 700.0),
    StyleTarget::new(ElementRole::Pad, 0.5, 0.35, 1_500.0),
    StyleTarget::new(ElementRole::Keys, 0.34, 0.6, 950.0),
    StyleTarget::new(ElementRole::Guitar, -0.66, 0.65, 1_200.0),
    StyleTarget::new(ElementRole::Lead, 0.5, 0.7, 2_000.0),
    StyleTarget::new(ElementRole::Vocals, 0.0, 1.0, 1_200.0),
    StyleTarget::new(ElementRole::Fx, 0.0, 0.2, 4_000.0),
];

const ELECTRONIC: [StyleTarget; 10] = [
    StyleTarget::new(ElementRole::Kick, 0.0, 1.0, 50.0),
    StyleTarget::new(ElementRole::Drums, 0.14, 0.86, 2_500.0),
    StyleTarget::new(ElementRole::Percussion, -0.6, 0.75, 6_000.0),
    StyleTarget::new(ElementRole::Bass, 0.0, 0.95, 90.0),
    StyleTarget::new(ElementRole::Chords, -0.4, 0.55, 900.0),
    StyleTarget::new(ElementRole::Pad, 0.6, 0.3, 2_000.0),
    StyleTarget::new(ElementRole::Keys, 0.4, 0.6, 1_400.0),
    StyleTarget::new(ElementRole::Lead, 0.0, 0.9, 3_000.0),
    StyleTarget::new(ElementRole::Vocals, 0.0, 0.95, 1_200.0),
    StyleTarget::new(ElementRole::Fx, 0.3, 0.25, 5_000.0),
];

const HIP_HOP: [StyleTarget; 8] = [
    StyleTarget::new(ElementRole::Kick, 0.0, 1.0, 55.0),
    StyleTarget::new(ElementRole::Drums, 0.05, 0.9, 200.0),
    StyleTarget::new(ElementRole::Percussion, -0.5, 0.65, 3_000.0),
    StyleTarget::new(ElementRole::Bass, 0.0, 0.95, 80.0),
    StyleTarget::new(ElementRole::Chords, -0.3, 0.5, 800.0),
    StyleTarget::new(ElementRole::Keys, 0.45, 0.55, 1_200.0),
    StyleTarget::new(ElementRole::Lead, 0.35, 0.65, 2_000.0),
    StyleTarget::new(ElementRole::Vocals, 0.0, 1.0, 1_100.0),
];

const JAZZ: [StyleTarget; 8] = [
    StyleTarget::new(ElementRole::Kick, -0.1, 0.7, 60.0),
    StyleTarget::new(ElementRole::Drums, 0.2, 0.75, 200.0),
    StyleTarget::new(ElementRole::Percussion, -0.6, 0.5, 4_000.0),
    StyleTarget::new(ElementRole::Bass, 0.0, 0.8, 90.0),
    StyleTarget::new(ElementRole::Keys, -0.35, 0.7, 800.0),
    StyleTarget::new(ElementRole::Guitar, 0.5, 0.65, 1_000.0),
    StyleTarget::new(ElementRole::Lead, 0.2, 0.8, 1_500.0),
    StyleTarget::new(ElementRole::Vocals, 0.0, 0.95, 1_100.0),
];

const ORCHESTRAL: [StyleTarget; 9] = [
    StyleTarget::new(ElementRole::Drums, 0.0, 0.55, 80.0),
    StyleTarget::new(ElementRole::Percussion, -0.5, 0.45, 3_000.0),
    StyleTarget::new(ElementRole::Bass, -0.4, 0.5, 120.0),
    StyleTarget::new(ElementRole::Chords, -0.6, 0.4, 700.0),
    StyleTarget::new(ElementRole::Pad, 0.6, 0.35, 1_200.0),
    StyleTarget::new(ElementRole::Keys, 0.35, 0.6, 1_000.0),
    StyleTarget::new(ElementRole::Lead, 0.1, 0.9, 1_400.0),
    StyleTarget::new(ElementRole::Vocals, 0.0, 0.3, 900.0),
    StyleTarget::new(ElementRole::Fx, 0.7, 0.2, 4_000.0),
];

const VOCAL_BALLAD: [StyleTarget; 7] = [
    StyleTarget::new(ElementRole::Bass, 0.0, 0.8, 90.0),
    StyleTarget::new(ElementRole::Drums, 0.0, 0.4, 150.0),
    StyleTarget::new(ElementRole::Chords, -0.4, 0.35, 600.0),
    StyleTarget::new(ElementRole::Pad, 0.55, 0.25, 1_500.0),
    StyleTarget::new(ElementRole::Keys, 0.25, 0.5, 800.0),
    StyleTarget::new(ElementRole::Guitar, -0.6, 0.55, 900.0),
    StyleTarget::new(ElementRole::Vocals, 0.0, 1.0, 1_100.0),
];

/// A point in the spatial field.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct SpatialPosition {
    pub pan: f32,
    pub depth: f32,
    pub frequency_hz: f32,
}

impl SpatialPosition {
    pub fn height(&self) -> f32 {
        frequency_to_height(self.frequency_hz)
    }

    pub fn size(&self) -> f32 {
        0.32 + 0.6 * self.depth
    }

    pub fn from_target(target: StyleTarget) -> Self {
        target.position()
    }
}

/// Metering data supplied by the audio engine for one track.
#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub struct TrackMeasurement {
    pub rms_db: f32,
    /// Linear band energies in 0..1.
    pub low: f32,
    pub mid: f32,
    pub high: f32,
}

impl Default for TrackMeasurement {
    fn default() -> Self {
        Self {
            rms_db: -90.0,
            low: 0.0,
            mid: 0.0,
            high: 0.0,
        }
    }
}

impl TrackMeasurement {
    pub fn new(rms_db: f32, low: f32, mid: f32, high: f32) -> Self {
        Self {
            rms_db,
            low,
            mid,
            high,
        }
    }

    pub fn silent() -> Self {
        Self::default()
    }

    /// Approximate spectral centroid from three band energies.
    pub fn centroid_hz(&self) -> f32 {
        let (low, mid, high) = (self.low.max(0.0), self.mid.max(0.0), self.high.max(0.0));
        let total = low + mid + high;
        if total <= 1e-9 {
            return ElementRole::Other.nominal_hz();
        }
        (low * 100.0 + mid * 1_000.0 + high * 6_000.0) / total
    }

    pub fn size(&self) -> f32 {
        rms_to_size(self.rms_db)
    }
}

/// Computes the actual field position of a track from its controls and its
/// measured spectrum. Pan and fader set X/Z; the spectrum sets Y.
pub fn track_position(track: &Track, measurement: &TrackMeasurement) -> SpatialPosition {
    let hz = measurement.centroid_hz() * 2f32.powf(track.spatial.frequency_bias);
    SpatialPosition {
        pan: track.pan,
        depth: db_to_depth(track.volume_db),
        frequency_hz: hz.clamp(FREQ_MIN_HZ, FREQ_MAX_HZ),
    }
}

/// How closely one track matches its target image.
#[derive(Debug, Clone)]
pub struct TrackAnalysis {
    pub track: TrackId,
    pub name: String,
    pub role: ElementRole,
    pub actual: SpatialPosition,
    pub actual_size: f32,
    pub measured_hz: f32,
    pub target: Option<StyleTarget>,
    pub pan_error: f32,
    pub depth_error: f32,
    pub frequency_error_octaves: f32,
    /// 0..100.
    pub score: f32,
    pub suggestions: Vec<String>,
}

impl TrackAnalysis {
    pub fn target_position(&self) -> Option<SpatialPosition> {
        self.target.map(SpatialPosition::from_target)
    }
}

#[derive(Debug, Clone, Default)]
pub struct MixAnalysis {
    pub tracks: Vec<TrackAnalysis>,
    pub score: f32,
    pub headline: String,
    pub style: Option<MixStyle>,
}

impl MixAnalysis {
    pub fn suggested(&self) -> impl Iterator<Item = &TrackAnalysis> {
        self.tracks.iter().filter(|t| !t.suggestions.is_empty())
    }
}

/// Compares a project against a style's reference image.
pub fn analyze(
    project: &Project,
    measurements: &HashMap<TrackId, TrackMeasurement>,
    style: MixStyle,
) -> MixAnalysis {
    let mut tracks = Vec::new();
    let mut total_score = 0.0;
    let mut scored = 0usize;

    for track in &project.tracks {
        if track.mute || !track.spatial.visible {
            continue;
        }
        let measurement = measurements.get(&track.id).copied().unwrap_or_default();
        let actual = track_position(track, &measurement);
        let target = style.target_for(track.spatial.role);
        let mut suggestions = Vec::new();
        let mut score = 100.0f32;
        let (mut pan_error, mut depth_error, mut freq_error_octaves) = (0.0, 0.0, 0.0);

        if let Some(target) = target {
            pan_error = actual.pan - target.pan;
            depth_error = actual.depth - target.depth;
            freq_error_octaves = (actual.frequency_hz / target.frequency_hz.max(1.0)).log2();
            score -= pan_error.abs() * 55.0;
            score -= depth_error.abs() * 38.0;
            score -= (freq_error_octaves.abs().min(3.0) / 3.0) * 26.0;
            score = score.clamp(0.0, 100.0);

            if pan_error.abs() > 0.07 {
                let pct = (pan_error.abs() * 100.0).round() as i32;
                let dir = if pan_error > 0.0 { "left" } else { "right" };
                suggestions.push(format!("Move {} {pct}% {dir} in the field.", track.name));
            }
            if depth_error.abs() > 0.055 {
                let db = (depth_error.abs() * DEPTH_RANGE_DB).round() as i32;
                if depth_error > 0.0 {
                    suggestions.push(format!(
                        "{} is too close: pull it back {db} dB.",
                        track.name
                    ));
                } else {
                    suggestions.push(format!(
                        "{} is buried: bring it forward {db} dB.",
                        track.name
                    ));
                }
            }
            if freq_error_octaves.abs() > 0.45 {
                let dir = if freq_error_octaves > 0.0 {
                    "above"
                } else {
                    "below"
                };
                suggestions.push(format!(
                    "{} sits {dir} its {:.0} Hz target; shape it around there.",
                    track.name, target.frequency_hz
                ));
            }
        } else {
            // No target for this role in this style: score on placement alone.
            score = 100.0 - (actual.pan.abs() * 10.0) - ((1.0 - actual.depth).abs() * 20.0);
        }

        total_score += score;
        scored += 1;
        tracks.push(TrackAnalysis {
            track: track.id,
            name: track.name.clone(),
            role: track.spatial.role,
            actual,
            actual_size: (measurement.size() * track.spatial.size_scale).clamp(0.1, 1.4),
            measured_hz: actual.frequency_hz,
            target,
            pan_error,
            depth_error,
            frequency_error_octaves: freq_error_octaves,
            score,
            suggestions,
        });
    }

    let score = if scored > 0 {
        total_score / scored as f32
    } else {
        100.0
    };
    let headline = if scored == 0 {
        "No visible tracks to analyse.".to_string()
    } else if score >= 87.0 {
        format!(
            "{} image is close to the reference — {:.0}% match.",
            style.label(),
            score
        )
    } else if score >= 70.0 {
        format!(
            "{} image is in the right neighbourhood — {:.0}% match.",
            style.label(),
            score
        )
    } else {
        format!(
            "{} image is far from the reference — {:.0}% match.",
            style.label(),
            score
        )
    };

    MixAnalysis {
        tracks,
        score,
        headline,
        style: Some(style),
    }
}

/// One proposed fader/pan move from [`auto_place`].
#[derive(Debug, Clone, Copy)]
pub struct AutoPlacement {
    pub track: TrackId,
    pub pan: f32,
    pub volume_db: f32,
    pub matched_target: bool,
}

/// Suggests pan and fader values that place every track on its role's target.
pub fn auto_place(project: &Project, style: MixStyle) -> Vec<AutoPlacement> {
    let mut out = Vec::new();
    let mut role_counts: HashMap<ElementRole, usize> = HashMap::new();
    for track in &project.tracks {
        *role_counts.entry(track.spatial.role).or_insert(0) += 1;
    }
    let mut role_seen: HashMap<ElementRole, usize> = HashMap::new();

    for track in &project.tracks {
        let role = track.spatial.role;
        let seen = role_seen.entry(role).or_insert(0);
        let index = *seen;
        *seen += 1;
        let spread = if role_counts.get(&role).copied().unwrap_or(1) > 1 {
            (index as f32 - (role_counts[&role] as f32 - 1.0) / 2.0) * 0.18
        } else {
            0.0
        };
        if let Some(target) = style.target_for(role) {
            out.push(AutoPlacement {
                track: track.id,
                pan: (target.pan + spread).clamp(-1.0, 1.0),
                volume_db: target.level_db(),
                matched_target: true,
            });
        } else {
            out.push(AutoPlacement {
                track: track.id,
                pan: (track.pan + spread * 0.5).clamp(-1.0, 1.0),
                volume_db: depth_to_db(0.55),
                matched_target: false,
            });
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{InstrumentKind, SynthParams};

    #[test]
    fn frequency_axis_is_logarithmic() {
        assert!((frequency_to_height(20.0) - 0.0).abs() < 1e-6);
        assert!((frequency_to_height(20_000.0) - 1.0).abs() < 1e-6);
        assert!((height_to_frequency(0.5) - 632.45).abs() < 1.0);
        let mut prev = -1.0;
        for hz in [30.0, 80.0, 250.0, 1000.0, 4000.0, 12000.0] {
            let h = frequency_to_height(hz);
            assert!(h > prev);
            prev = h;
        }
    }

    #[test]
    fn depth_maps_to_fader_db() {
        assert!((db_to_depth(0.0) - 1.0).abs() < 1e-6);
        assert!((db_to_depth(-36.0) - 0.0).abs() < 1e-6);
        assert!((depth_to_db(0.5) + 18.0).abs() < 1e-6);
        assert!((db_to_depth(depth_to_db(0.3)) - 0.3).abs() < 1e-6);
    }

    #[test]
    fn role_guessing() {
        assert_eq!(ElementRole::guess("Kick", true), ElementRole::Kick);
        assert_eq!(ElementRole::guess("Lead Vox", false), ElementRole::Vocals);
        assert_eq!(ElementRole::guess("Sub Bass", false), ElementRole::Bass);
        assert_eq!(ElementRole::guess("Snare", true), ElementRole::Drums);
        assert_eq!(ElementRole::guess("Rhodes", false), ElementRole::Keys);
        assert_eq!(ElementRole::guess("Unknown", false), ElementRole::Other);
    }

    #[test]
    fn centroid_tracks_band_balance() {
        let low = TrackMeasurement::new(-20.0, 1.0, 0.05, 0.0);
        let high = TrackMeasurement::new(-20.0, 0.0, 0.05, 1.0);
        assert!(low.centroid_hz() < high.centroid_hz());
        assert!(low.size() > 0.0);
    }

    #[test]
    fn analysis_points_at_the_error() {
        let mut project = Project::demo("Mix");
        let lead = project.tracks.iter().find(|t| t.name == "Lead").unwrap().id;
        project.track_mut(lead).unwrap().pan = -1.0;
        let mut measurements = HashMap::new();
        for track in &project.tracks {
            measurements.insert(track.id, TrackMeasurement::new(-18.0, 0.2, 0.5, 0.3));
        }
        let analysis = analyze(&project, &measurements, MixStyle::RockPop);
        let lead_analysis = analysis.tracks.iter().find(|t| t.track == lead).unwrap();
        assert!(lead_analysis
            .suggestions
            .iter()
            .any(|s| s.contains("right")));
        assert!(analysis.score < 100.0);
    }

    #[test]
    fn auto_place_matches_targets() {
        let project = Project::demo("Mix");
        let moves = auto_place(&project, MixStyle::RockPop);
        assert_eq!(moves.len(), project.tracks.len());
        let kick = project.tracks.iter().find(|t| t.name == "Kick").unwrap();
        let kick_move = moves.iter().find(|m| m.track == kick.id).unwrap();
        assert!((kick_move.volume_db + 0.0).abs() < 0.001);
        assert!(moves
            .iter()
            .all(|m| m.volume_db <= 0.0 && m.volume_db >= -36.0));
    }

    #[test]
    fn every_style_places_a_vocal() {
        for style in MixStyle::ALL {
            let t = style.target_for(ElementRole::Vocals);
            if let Some(t) = t {
                assert_eq!(t.role, ElementRole::Vocals);
                assert!(t.depth > 0.2);
                assert!(t.pan.abs() < 0.2);
            }
        }
    }

    #[test]
    fn silent_track_is_small_but_not_invisible() {
        let s = TrackMeasurement::silent();
        assert!(s.size() >= 0.18);
        assert_eq!(s.centroid_hz(), ElementRole::Other.nominal_hz());
    }

    #[test]
    fn plugin_instrument_is_not_a_synth() {
        let kind = InstrumentKind::Synth(SynthParams::default());
        assert!(!kind.is_plugin());
        assert_eq!(kind.default_note(), 60);
    }
}

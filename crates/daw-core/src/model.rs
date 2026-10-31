//! The song document.
//!
//! A project is a flat list of tracks. Each instrument track owns its own
//! sequencer patterns, which is what makes the step sequencer feel like part of
//! the instrument rather than a separate editor. Audio tracks hold clips that
//! reference the media store.

use serde::{Deserialize, Serialize};

use crate::gibson::{ElementRole, MixStyle};
use crate::ids::{ClipId, NoteId, PatternId, PluginInstanceId, TrackId};
use crate::time::{samples_per_tick, Grid};

pub use crate::time::TimeSignature;

/// Bump when the on-disk schema changes in an incompatible way.
pub const FORMAT_VERSION: u32 = 1;

/// Lowest usable fader value in dB. At or below this the track is silent.
pub const MIN_DB: f32 = -60.0;
/// Highest fader value in dB.
pub const MAX_DB: f32 = 6.0;
/// How many dB of fader travel map to the front-to-back axis of the 3D room.
pub const DEPTH_RANGE_DB: f32 = 36.0;

pub fn clamp_db(db: f32) -> f32 {
    db.clamp(MIN_DB, MAX_DB)
}

/// Converts dB to a linear amplitude, with the fader floor as silence.
pub fn db_to_gain(db: f32) -> f32 {
    if db <= MIN_DB {
        0.0
    } else {
        10f32.powf(db / 20.0)
    }
}

pub fn gain_to_db(gain: f32) -> f32 {
    if gain <= 1e-6 {
        MIN_DB
    } else {
        (20.0 * gain.log10()).clamp(MIN_DB, MAX_DB)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum TrackKind {
    #[default]
    Instrument,
    Audio,
}

impl TrackKind {
    pub fn label(self) -> &'static str {
        match self {
            TrackKind::Instrument => "Instrument",
            TrackKind::Audio => "Audio",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum Waveform {
    Sine,
    #[default]
    Saw,
    Square,
    Triangle,
    Noise,
}

impl Waveform {
    pub const ALL: [Waveform; 5] = [
        Waveform::Sine,
        Waveform::Saw,
        Waveform::Square,
        Waveform::Triangle,
        Waveform::Noise,
    ];

    pub fn label(self) -> &'static str {
        match self {
            Waveform::Sine => "Sine",
            Waveform::Saw => "Saw",
            Waveform::Square => "Square",
            Waveform::Triangle => "Triangle",
            Waveform::Noise => "Noise",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum FilterMode {
    #[default]
    LowPass,
    HighPass,
    BandPass,
}

impl FilterMode {
    pub const ALL: [FilterMode; 3] = [
        FilterMode::LowPass,
        FilterMode::HighPass,
        FilterMode::BandPass,
    ];

    pub fn label(self) -> &'static str {
        match self {
            FilterMode::LowPass => "Low pass",
            FilterMode::HighPass => "High pass",
            FilterMode::BandPass => "Band pass",
        }
    }
}

/// A standard ADSR envelope. Times are in seconds, sustain is 0..=1.
#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub struct Adsr {
    pub attack: f32,
    pub decay: f32,
    pub sustain: f32,
    pub release: f32,
}

impl Adsr {
    pub fn new(attack: f32, decay: f32, sustain: f32, release: f32) -> Self {
        Self {
            attack: attack.max(0.0005),
            decay: decay.max(0.001),
            sustain: sustain.clamp(0.0, 1.0),
            release: release.max(0.001),
        }
    }

    pub fn pluck() -> Self {
        Self::new(0.001, 0.12, 0.0, 0.18)
    }

    pub fn organ() -> Self {
        Self::new(0.005, 0.08, 0.85, 0.25)
    }

    pub fn pad() -> Self {
        Self::new(0.45, 1.2, 0.7, 1.6)
    }
}

impl Default for Adsr {
    fn default() -> Self {
        Self::new(0.005, 0.25, 0.6, 0.35)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SynthParams {
    pub osc1: Waveform,
    pub osc2: Waveform,
    pub osc2_level: f32,
    pub detune_cents: f32,
    pub octave: i8,
    pub noise: f32,
    pub cutoff_hz: f32,
    pub resonance: f32,
    pub filter_mode: FilterMode,
    pub filter_env_amount: f32,
    pub key_track: f32,
    pub filter_env: Adsr,
    pub amp_env: Adsr,
    pub lfo_rate_hz: f32,
    pub lfo_pitch: f32,
    pub lfo_cutoff: f32,
    pub glide: f32,
    pub velocity_sensitivity: f32,
    pub level_db: f32,
}

impl Default for SynthParams {
    fn default() -> Self {
        Self {
            osc1: Waveform::Saw,
            osc2: Waveform::Saw,
            osc2_level: 0.55,
            detune_cents: 8.0,
            octave: 0,
            noise: 0.0,
            cutoff_hz: 2_800.0,
            resonance: 0.22,
            filter_mode: FilterMode::LowPass,
            filter_env_amount: 0.35,
            key_track: 0.4,
            filter_env: Adsr::new(0.004, 0.32, 0.15, 0.3),
            amp_env: Adsr::new(0.005, 0.4, 0.7, 0.28),
            lfo_rate_hz: 0.0,
            lfo_pitch: 0.0,
            lfo_cutoff: 0.0,
            glide: 0.0,
            velocity_sensitivity: 0.6,
            level_db: -6.0,
        }
    }
}

impl SynthParams {
    pub fn bass() -> Self {
        Self {
            osc1: Waveform::Saw,
            osc2: Waveform::Square,
            osc2_level: 0.35,
            detune_cents: 2.0,
            octave: -1,
            cutoff_hz: 620.0,
            resonance: 0.35,
            filter_env_amount: 0.55,
            filter_env: Adsr::new(0.002, 0.18, 0.05, 0.15),
            amp_env: Adsr::new(0.002, 0.22, 0.55, 0.12),
            level_db: -5.0,
            ..Default::default()
        }
    }

    pub fn keys() -> Self {
        Self {
            osc1: Waveform::Triangle,
            osc2: Waveform::Sine,
            osc2_level: 0.4,
            detune_cents: 4.0,
            cutoff_hz: 1_900.0,
            resonance: 0.15,
            filter_env_amount: 0.2,
            amp_env: Adsr::new(0.01, 0.5, 0.45, 0.6),
            level_db: -9.0,
            ..Default::default()
        }
    }

    pub fn lead() -> Self {
        Self {
            osc1: Waveform::Square,
            osc2: Waveform::Saw,
            osc2_level: 0.5,
            detune_cents: 12.0,
            cutoff_hz: 4_500.0,
            resonance: 0.3,
            filter_env_amount: 0.3,
            amp_env: Adsr::new(0.01, 0.3, 0.6, 0.4),
            lfo_rate_hz: 5.2,
            lfo_pitch: 0.12,
            level_db: -10.0,
            ..Default::default()
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum DrumSound {
    Kick,
    Snare,
    Clap,
    HatClosed,
    HatOpen,
    Tom,
    Rim,
    Crash,
}

impl DrumSound {
    pub const ALL: [DrumSound; 8] = [
        DrumSound::Kick,
        DrumSound::Snare,
        DrumSound::Clap,
        DrumSound::HatClosed,
        DrumSound::HatOpen,
        DrumSound::Tom,
        DrumSound::Rim,
        DrumSound::Crash,
    ];

    pub fn label(self) -> &'static str {
        match self {
            DrumSound::Kick => "Kick",
            DrumSound::Snare => "Snare",
            DrumSound::Clap => "Clap",
            DrumSound::HatClosed => "Closed Hat",
            DrumSound::HatOpen => "Open Hat",
            DrumSound::Tom => "Tom",
            DrumSound::Rim => "Rim",
            DrumSound::Crash => "Crash",
        }
    }

    pub fn default_pitch(self) -> u8 {
        36 + self as u8
    }

    /// Suggested role used when a track is created with this sound.
    pub fn default_role(self) -> ElementRole {
        match self {
            DrumSound::Kick => ElementRole::Kick,
            DrumSound::Snare | DrumSound::Tom => ElementRole::Drums,
            DrumSound::Clap
            | DrumSound::HatClosed
            | DrumSound::HatOpen
            | DrumSound::Rim
            | DrumSound::Crash => ElementRole::Percussion,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DrumParams {
    pub sound: DrumSound,
    pub tune: f32,
    pub decay: f32,
    pub tone: f32,
    pub level_db: f32,
    pub pan: f32,
}

impl DrumParams {
    pub fn new(sound: DrumSound) -> Self {
        Self {
            sound,
            tune: 0.0,
            decay: 1.0,
            tone: 0.5,
            level_db: 0.0,
            pan: 0.0,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum LoopMode {
    #[default]
    Off,
    Forward,
}

impl LoopMode {
    pub fn label(self) -> &'static str {
        match self {
            LoopMode::Off => "One shot",
            LoopMode::Forward => "Loop",
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SamplerParams {
    /// Key into the project media store. Empty until a sample is loaded.
    pub media: String,
    pub root_note: u8,
    pub loop_mode: LoopMode,
    /// Normalised positions into the sample.
    pub loop_start: f32,
    pub loop_end: f32,
    pub transpose: i8,
    pub reverse: bool,
    pub gain_db: f32,
    pub amp_env: Adsr,
    pub velocity_sensitivity: f32,
}

impl Default for SamplerParams {
    fn default() -> Self {
        Self {
            media: String::new(),
            root_note: 60,
            loop_mode: LoopMode::Off,
            loop_start: 0.0,
            loop_end: 1.0,
            transpose: 0,
            reverse: false,
            gain_db: 0.0,
            amp_env: Adsr::new(0.002, 0.4, 1.0, 0.15),
            velocity_sensitivity: 0.4,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum PluginFormat {
    Vst2,
    Vst3,
}

impl PluginFormat {
    pub fn label(self) -> &'static str {
        match self {
            PluginFormat::Vst2 => "VST2",
            PluginFormat::Vst3 => "VST3",
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PluginParamValue {
    pub id: u32,
    pub value: f64,
}

/// Serializable description of a plugin. Audio-thread ownership of the actual
/// plugin instance is keyed by the containing slot id.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PluginSpec {
    pub format: PluginFormat,
    pub path: String,
    #[serde(default)]
    pub class_id: Option<String>,
    pub name: String,
    #[serde(default)]
    pub vendor: String,
    pub is_instrument: bool,
    #[serde(default)]
    pub params: Vec<PluginParamValue>,
}

impl PluginSpec {
    pub fn new(
        format: PluginFormat,
        path: impl Into<String>,
        name: impl Into<String>,
        is_instrument: bool,
    ) -> Self {
        Self {
            format,
            path: path.into(),
            class_id: None,
            name: name.into(),
            vendor: String::new(),
            is_instrument,
            params: Vec::new(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FxSlot {
    pub id: PluginInstanceId,
    pub enabled: bool,
    pub spec: PluginSpec,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Instrument {
    pub kind: InstrumentKind,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum InstrumentKind {
    Drum(DrumParams),
    Synth(SynthParams),
    Sampler(SamplerParams),
    Plugin(PluginSpec),
}

impl InstrumentKind {
    pub fn label(&self) -> &'static str {
        match self {
            InstrumentKind::Drum(d) => d.sound.label(),
            InstrumentKind::Synth(_) => "Synth",
            InstrumentKind::Sampler(_) => "Sampler",
            InstrumentKind::Plugin(_) => "Plugin",
        }
    }

    pub fn default_note(&self) -> u8 {
        match self {
            InstrumentKind::Drum(d) => d.sound.default_pitch(),
            InstrumentKind::Sampler(s) => s.root_note,
            _ => 60,
        }
    }

    pub fn is_plugin(&self) -> bool {
        matches!(self, InstrumentKind::Plugin(_))
    }

    pub fn plugin_spec(&self) -> Option<&PluginSpec> {
        match self {
            InstrumentKind::Plugin(p) => Some(p),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Note {
    pub id: NoteId,
    pub pitch: u8,
    pub start: i64,
    pub length: i64,
    pub velocity: u8,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Pattern {
    pub id: PatternId,
    pub name: String,
    pub length_ticks: i64,
    pub notes: Vec<Note>,
    #[serde(default)]
    pub color: [u8; 3],
}

impl Pattern {
    pub fn end_tick(&self) -> i64 {
        self.notes
            .iter()
            .map(|n| n.start + n.length)
            .max()
            .unwrap_or(0)
    }

    pub fn note_at(&self, start: i64, pitch: u8, tolerance: i64) -> Option<&Note> {
        self.notes
            .iter()
            .find(|n| n.pitch == pitch && (n.start - start).abs() <= tolerance)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AudioClip {
    pub media: String,
    /// First frame of the media used by this clip.
    pub offset_frames: i64,
    /// Number of frames to play. Audio duration is independent of tempo.
    pub length_frames: i64,
    pub gain_db: f32,
    pub fade_in_seconds: f32,
    pub fade_out_seconds: f32,
    pub reverse: bool,
}

impl AudioClip {
    pub fn source_end_frame(&self) -> i64 {
        self.offset_frames + self.length_frames
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PatternClip {
    pub pattern: PatternId,
    pub transpose: i8,
    /// Tick length. Normally the pattern length, but clips can be shortened.
    pub length_ticks: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum ClipKind {
    Audio(AudioClip),
    Pattern(PatternClip),
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Clip {
    pub id: ClipId,
    pub name: String,
    pub start: i64,
    pub kind: ClipKind,
    #[serde(default)]
    pub color: Option<[u8; 3]>,
    #[serde(default)]
    pub muted: bool,
}

impl Clip {
    /// Display/arrangement length in ticks.
    pub fn length_ticks(&self, bpm: f64, sample_rate: f64) -> i64 {
        match &self.kind {
            ClipKind::Pattern(p) => p.length_ticks.max(1),
            ClipKind::Audio(a) => {
                let spt = samples_per_tick(sample_rate, bpm);
                if spt <= 0.0 {
                    1
                } else {
                    (a.length_frames as f64 / spt).round() as i64
                }
            }
        }
    }

    pub fn end_tick(&self, bpm: f64, sample_rate: f64) -> i64 {
        self.start + self.length_ticks(bpm, sample_rate)
    }

    pub fn is_audio(&self) -> bool {
        matches!(self.kind, ClipKind::Audio(_))
    }
}

/// Per-track spatial metadata that tunes how the track is drawn and analysed.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SpatialMeta {
    pub role: ElementRole,
    /// Frequency bias in octaves applied to the measured spectrum for display.
    #[serde(default)]
    pub frequency_bias: f32,
    /// Multiplier applied to the sphere radius.
    #[serde(default = "one")]
    pub size_scale: f32,
    #[serde(default = "yes")]
    pub visible: bool,
}

fn one() -> f32 {
    1.0
}
fn yes() -> bool {
    true
}

impl Default for SpatialMeta {
    fn default() -> Self {
        Self {
            role: ElementRole::Other,
            frequency_bias: 0.0,
            size_scale: 1.0,
            visible: true,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Track {
    pub id: TrackId,
    pub name: String,
    pub kind: TrackKind,
    pub color: [u8; 3],
    pub volume_db: f32,
    /// -1.0 hard left .. +1.0 hard right.
    pub pan: f32,
    #[serde(default)]
    pub mute: bool,
    #[serde(default)]
    pub solo: bool,
    pub height: f32,
    #[serde(default)]
    pub clips: Vec<Clip>,
    #[serde(default)]
    pub instrument: Option<Instrument>,
    #[serde(default)]
    pub patterns: Vec<Pattern>,
    #[serde(default)]
    pub active_pattern: Option<PatternId>,
    #[serde(default)]
    pub fx: Vec<FxSlot>,
    #[serde(default)]
    pub spatial: SpatialMeta,
}

impl Track {
    pub fn new(id: TrackId, name: impl Into<String>, kind: TrackKind, color: [u8; 3]) -> Self {
        Self {
            id,
            name: name.into(),
            kind,
            color,
            volume_db: 0.0,
            pan: 0.0,
            mute: false,
            solo: false,
            height: 92.0,
            clips: Vec::new(),
            instrument: None,
            patterns: Vec::new(),
            active_pattern: None,
            fx: Vec::new(),
            spatial: SpatialMeta::default(),
        }
    }

    pub fn is_audio(&self) -> bool {
        self.kind == TrackKind::Audio
    }

    pub fn instrument_kind(&self) -> Option<&InstrumentKind> {
        self.instrument.as_ref().map(|i| &i.kind)
    }

    pub fn default_note(&self) -> u8 {
        self.instrument_kind()
            .map(|k| k.default_note())
            .unwrap_or(60)
    }

    pub fn pattern(&self, id: PatternId) -> Option<&Pattern> {
        self.patterns.iter().find(|p| p.id == id)
    }

    pub fn pattern_mut(&mut self, id: PatternId) -> Option<&mut Pattern> {
        self.patterns.iter_mut().find(|p| p.id == id)
    }

    pub fn active_pattern(&self) -> Option<&Pattern> {
        self.active_pattern
            .and_then(|id| self.pattern(id))
            .or_else(|| self.patterns.first())
    }

    pub fn active_pattern_mut(&mut self) -> Option<&mut Pattern> {
        let id = self
            .active_pattern
            .filter(|id| self.patterns.iter().any(|p| p.id == *id))
            .or_else(|| self.patterns.first().map(|p| p.id));
        id.and_then(move |id| self.pattern_mut(id))
    }

    pub fn clip(&self, id: ClipId) -> Option<&Clip> {
        self.clips.iter().find(|c| c.id == id)
    }

    pub fn clip_mut(&mut self, id: ClipId) -> Option<&mut Clip> {
        self.clips.iter_mut().find(|c| c.id == id)
    }

    pub fn end_tick(&self, bpm: f64, sample_rate: f64) -> i64 {
        self.clips
            .iter()
            .map(|c| c.end_tick(bpm, sample_rate))
            .max()
            .unwrap_or(0)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MasterBus {
    pub volume_db: f32,
    #[serde(default)]
    pub limiter: bool,
    #[serde(default)]
    pub fx: Vec<FxSlot>,
}

impl Default for MasterBus {
    fn default() -> Self {
        Self {
            volume_db: 0.0,
            limiter: true,
            fx: Vec::new(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Project {
    pub format_version: u32,
    pub name: String,
    pub bpm: f64,
    pub time_signature: TimeSignature,
    /// Native project rate used for rendering and for the reference grid.
    pub sample_rate: u32,
    pub loop_region: Option<(i64, i64)>,
    pub metronome: bool,
    pub snap: Grid,
    pub tracks: Vec<Track>,
    pub master: MasterBus,
    pub spatial_style: MixStyle,
    pub next_id: u64,
}

impl Default for Project {
    fn default() -> Self {
        Self::demo("Untitled")
    }
}

const PALETTE: [[u8; 3]; 10] = [
    [239, 68, 68],
    [245, 158, 11],
    [234, 179, 8],
    [59, 130, 246],
    [139, 92, 246],
    [16, 185, 129],
    [236, 72, 153],
    [6, 182, 212],
    [249, 115, 22],
    [168, 85, 247],
];

pub fn palette_color(index: usize) -> [u8; 3] {
    PALETTE[index % PALETTE.len()]
}

impl Project {
    /// Creates a ready-to-play demo song so the DAW makes sound on first launch.
    pub fn demo(name: &str) -> Self {
        let mut project = Self {
            format_version: FORMAT_VERSION,
            name: name.to_string(),
            bpm: 120.0,
            time_signature: TimeSignature::FOUR_FOUR,
            sample_rate: 48_000,
            loop_region: None,
            metronome: false,
            snap: Grid::Sixteenth,
            tracks: Vec::new(),
            master: MasterBus::default(),
            spatial_style: MixStyle::RockPop,
            next_id: 1,
        };

        let kick = project.add_instrument_track(
            "Kick",
            InstrumentKind::Drum(DrumParams::new(DrumSound::Kick)),
            ElementRole::Kick,
        );
        let snare = project.add_instrument_track(
            "Snare",
            InstrumentKind::Drum(DrumParams::new(DrumSound::Snare)),
            ElementRole::Drums,
        );
        let hats = project.add_instrument_track(
            "Hats",
            InstrumentKind::Drum(DrumParams::new(DrumSound::HatClosed)),
            ElementRole::Percussion,
        );
        let bass = project.add_instrument_track(
            "Bass",
            InstrumentKind::Synth(SynthParams::bass()),
            ElementRole::Bass,
        );
        let keys = project.add_instrument_track(
            "Keys",
            InstrumentKind::Synth(SynthParams::keys()),
            ElementRole::Keys,
        );
        let lead = project.add_instrument_track(
            "Lead",
            InstrumentKind::Synth(SynthParams::lead()),
            ElementRole::Lead,
        );

        project.set_track_pan(kick, 0.0);
        project.set_track_db(kick, -2.0);
        project.set_track_pan(snare, 0.06);
        project.set_track_db(snare, -7.0);
        project.set_track_pan(hats, -0.45);
        project.set_track_db(hats, -14.0);
        project.set_track_pan(bass, 0.0);
        project.set_track_db(bass, -5.0);
        project.set_track_pan(keys, 0.32);
        project.set_track_db(keys, -12.0);
        project.set_track_pan(lead, -0.28);
        project.set_track_db(lead, -16.0);

        let bar = project.time_signature.ticks_per_bar().max(1);
        let sixteenth = bar / 16;
        let pattern_len = bar * 2;

        // Drums: four-on-the-floor kick, backbeat snare, eighth-note hats.
        if let Some(pat) = project.add_pattern(kick, "Beat", pattern_len) {
            for i in 0..8 {
                project.add_note(kick, pat, 36, i * bar / 4, sixteenth, 118);
            }
        }
        if let Some(pat) = project.add_pattern(snare, "Backbeat", pattern_len) {
            for bar_i in 0..2 {
                for step in [1, 3] {
                    project.add_note(snare, pat, 37, bar_i * bar + step * bar / 4, sixteenth, 108);
                }
            }
        }
        if let Some(pat) = project.add_pattern(hats, "Eighths", pattern_len) {
            for i in 0..16 {
                let vel = if i % 2 == 0 { 96 } else { 62 };
                project.add_note(hats, pat, 39, i * bar / 8, sixteenth, vel);
            }
        }
        if let Some(pat) = project.add_pattern(bass, "Bassline", pattern_len) {
            let riff = [(0, 36, 3), (3, 36, 2), (6, 43, 2), (10, 41, 4), (14, 39, 2)];
            for (step, pitch, len) in riff {
                let start = bar * 2 * step / 16;
                project.add_note(bass, pat, pitch, start, sixteenth * len, 104);
            }
        }
        if let Some(pat) = project.add_pattern(keys, "Chord Bed", pattern_len) {
            for (chord_idx, chord) in [[48u8, 51, 55], [44, 48, 51]].iter().enumerate() {
                let start = chord_idx as i64 * bar;
                for pitch in chord {
                    project.add_note(keys, pat, *pitch, start, bar - sixteenth, 80);
                }
            }
        }
        if let Some(pat) = project.add_pattern(lead, "Hook", pattern_len) {
            let melody = [
                (8, 63, 3),
                (12, 67, 2),
                (16, 70, 4),
                (24, 67, 2),
                (26, 63, 4),
                (30, 60, 2),
            ];
            for (step, pitch, len) in melody {
                let start = bar * 2 * step / 16;
                project.add_note(lead, pat, pitch, start, sixteenth * len, 96);
            }
        }

        // Turn each pattern into one arrangement clip covering the two bars.
        let pattern_ids: Vec<(TrackId, PatternId, i64)> = project
            .tracks
            .iter()
            .filter_map(|t| {
                let pattern = t.active_pattern().or_else(|| t.patterns.first())?;
                Some((t.id, pattern.id, pattern.length_ticks))
            })
            .collect();
        for (track, pattern, length) in pattern_ids {
            project.add_pattern_clip(track, pattern, 0, length);
        }

        project.loop_region = Some((0, pattern_len));
        project.repair();
        project
    }

    /// A project with the default instrument tracks but no clips or notes.
    pub fn empty(name: &str) -> Self {
        let mut project = Self {
            format_version: FORMAT_VERSION,
            name: name.to_string(),
            bpm: 120.0,
            time_signature: TimeSignature::FOUR_FOUR,
            sample_rate: 48_000,
            loop_region: None,
            metronome: false,
            snap: Grid::Sixteenth,
            tracks: Vec::new(),
            master: MasterBus::default(),
            spatial_style: MixStyle::RockPop,
            next_id: 1,
        };
        project.add_instrument_track(
            "Kick",
            InstrumentKind::Drum(DrumParams::new(DrumSound::Kick)),
            ElementRole::Kick,
        );
        project.add_instrument_track(
            "Snare",
            InstrumentKind::Drum(DrumParams::new(DrumSound::Snare)),
            ElementRole::Drums,
        );
        project.add_instrument_track(
            "Bass",
            InstrumentKind::Synth(SynthParams::bass()),
            ElementRole::Bass,
        );
        project.add_instrument_track(
            "Keys",
            InstrumentKind::Synth(SynthParams::keys()),
            ElementRole::Keys,
        );
        project.repair();
        project
    }

    pub fn alloc_id(&mut self) -> u64 {
        let id = self.next_id.max(1);
        self.next_id = id + 1;
        id
    }

    pub fn add_instrument_track(
        &mut self,
        name: impl Into<String>,
        kind: InstrumentKind,
        role: ElementRole,
    ) -> TrackId {
        let id = TrackId(self.alloc_id());
        let index = self.tracks.len();
        let mut track = Track::new(id, name, TrackKind::Instrument, palette_color(index));
        track.instrument = Some(Instrument { kind });
        track.spatial.role = role;
        let pattern_id = PatternId(self.alloc_id());
        let pattern_len = self.time_signature.ticks_per_bar() * 2;
        let mut pattern = Pattern {
            id: pattern_id,
            name: "Pattern 1".to_string(),
            length_ticks: pattern_len,
            notes: Vec::new(),
            color: track.color,
        };
        pattern.notes.clear();
        track.patterns.push(pattern);
        track.active_pattern = Some(pattern_id);
        self.tracks.push(track);
        id
    }

    pub fn add_audio_track(&mut self, name: impl Into<String>) -> TrackId {
        let id = TrackId(self.alloc_id());
        let index = self.tracks.len();
        let mut track = Track::new(id, name, TrackKind::Audio, palette_color(index));
        track.spatial.role = ElementRole::Other;
        self.tracks.push(track);
        id
    }

    pub fn duplicate_track(&mut self, track: TrackId) -> Option<TrackId> {
        let source = self.track(track)?.clone();
        let new_id = TrackId(self.alloc_id());
        let mut copy = source.clone();
        copy.id = new_id;
        copy.name = format!("{} copy", copy.name);
        copy.clips.clear();
        copy.patterns.clear();
        copy.active_pattern = None;
        for clip in source.clips.iter().cloned() {
            let mut clip = clip;
            clip.id = ClipId(self.alloc_id());
            copy.clips.push(clip);
        }
        for pattern in source.patterns.iter().cloned() {
            let mut pattern = pattern;
            pattern.id = PatternId(self.alloc_id());
            for note in &mut pattern.notes {
                note.id = NoteId(self.alloc_id());
            }
            if copy.active_pattern.is_none() {
                copy.active_pattern = Some(pattern.id);
            }
            copy.patterns.push(pattern);
        }
        // Remap pattern-clip references after re-issuing pattern ids.
        let remap: Vec<(PatternId, PatternId)> = source
            .patterns
            .iter()
            .zip(copy.patterns.iter())
            .map(|(a, b)| (a.id, b.id))
            .collect();
        for clip in &mut copy.clips {
            if let ClipKind::Pattern(p) = &mut clip.kind {
                if let Some((_, new)) = remap.iter().find(|(old, _)| *old == p.pattern) {
                    p.pattern = *new;
                }
            }
        }
        copy.fx.clear();
        let index = self.tracks.len();
        copy.color = palette_color(index);
        self.tracks.push(copy);
        Some(new_id)
    }

    pub fn remove_track(&mut self, track: TrackId) -> bool {
        let before = self.tracks.len();
        self.tracks.retain(|t| t.id != track);
        before != self.tracks.len()
    }

    pub fn track(&self, id: TrackId) -> Option<&Track> {
        self.tracks.iter().find(|t| t.id == id)
    }

    pub fn track_mut(&mut self, id: TrackId) -> Option<&mut Track> {
        self.tracks.iter_mut().find(|t| t.id == id)
    }

    pub fn track_index(&self, id: TrackId) -> Option<usize> {
        self.tracks.iter().position(|t| t.id == id)
    }

    pub fn set_track_db(&mut self, id: TrackId, db: f32) {
        if let Some(t) = self.track_mut(id) {
            t.volume_db = clamp_db(db);
        }
    }

    pub fn set_track_pan(&mut self, id: TrackId, pan: f32) {
        if let Some(t) = self.track_mut(id) {
            t.pan = pan.clamp(-1.0, 1.0);
        }
    }

    pub fn any_solo(&self) -> bool {
        self.tracks.iter().any(|t| t.solo)
    }

    pub fn is_audible(&self, track: &Track) -> bool {
        if track.mute {
            return false;
        }
        if self.any_solo() {
            track.solo
        } else {
            true
        }
    }

    pub fn add_pattern(
        &mut self,
        track: TrackId,
        name: impl Into<String>,
        length_ticks: i64,
    ) -> Option<PatternId> {
        let id = PatternId(self.alloc_id());
        let color = self.track(track)?.color;
        let track = self.track_mut(track)?;
        track.patterns.push(Pattern {
            id,
            name: name.into(),
            length_ticks: length_ticks.max(1),
            notes: Vec::new(),
            color,
        });
        track.active_pattern = Some(id);
        Some(id)
    }

    pub fn add_note(
        &mut self,
        track: TrackId,
        pattern: PatternId,
        pitch: u8,
        start: i64,
        length: i64,
        velocity: u8,
    ) -> Option<NoteId> {
        let id = NoteId(self.alloc_id());
        let pat = self.track_mut(track)?.pattern_mut(pattern)?;
        let len = length.max(1);
        let start = start.max(0);
        pat.notes.push(Note {
            id,
            pitch: pitch.min(127),
            start,
            length: len,
            velocity: velocity.clamp(1, 127),
        });
        pat.notes.sort_by_key(|n| (n.start, n.pitch));
        Some(id)
    }

    pub fn remove_note(&mut self, track: TrackId, pattern: PatternId, note: NoteId) -> bool {
        if let Some(pat) = self.track_mut(track).and_then(|t| t.pattern_mut(pattern)) {
            let before = pat.notes.len();
            pat.notes.retain(|n| n.id != note);
            return before != pat.notes.len();
        }
        false
    }

    /// Adds or removes a step note at `start`, returning true when a note was added.
    pub fn toggle_step(
        &mut self,
        track: TrackId,
        pattern: PatternId,
        start: i64,
        length: i64,
        pitch: u8,
        velocity: u8,
    ) -> bool {
        let existing = self
            .track(track)
            .and_then(|t| t.pattern(pattern))
            .and_then(|p| p.note_at(start, pitch, 0))
            .map(|n| n.id);
        if let Some(note) = existing {
            self.remove_note(track, pattern, note);
            false
        } else {
            self.add_note(track, pattern, pitch, start, length, velocity);
            true
        }
    }

    pub fn clear_notes_in_range(
        &mut self,
        track: TrackId,
        pattern: PatternId,
        start: i64,
        end: i64,
    ) {
        if let Some(pat) = self.track_mut(track).and_then(|t| t.pattern_mut(pattern)) {
            pat.notes
                .retain(|n| n.start < start || n.start >= end || n.length <= 0);
        }
    }

    pub fn add_pattern_clip(
        &mut self,
        track: TrackId,
        pattern: PatternId,
        start: i64,
        length: i64,
    ) -> Option<ClipId> {
        let id = ClipId(self.alloc_id());
        let t = self.track_mut(track)?;
        if t.pattern(pattern).is_none() {
            return None;
        }
        let name = t
            .pattern(pattern)
            .map(|p| p.name.clone())
            .unwrap_or_default();
        let color = t.pattern(pattern).map(|p| p.color).unwrap_or(t.color);
        t.clips.push(Clip {
            id,
            name,
            start: start.max(0),
            kind: ClipKind::Pattern(PatternClip {
                pattern,
                transpose: 0,
                length_ticks: length.max(1),
            }),
            color: Some(color),
            muted: false,
        });
        t.clips.sort_by_key(|c| c.start);
        Some(id)
    }

    pub fn add_audio_clip(
        &mut self,
        track: TrackId,
        media: impl Into<String>,
        name: impl Into<String>,
        start: i64,
        length_frames: i64,
        source_offset: i64,
    ) -> Option<ClipId> {
        let id = ClipId(self.alloc_id());
        let t = self.track_mut(track)?;
        if !t.is_audio() {
            return None;
        }
        t.clips.push(Clip {
            id,
            name: name.into(),
            start: start.max(0),
            kind: ClipKind::Audio(AudioClip {
                media: media.into(),
                offset_frames: source_offset.max(0),
                length_frames: length_frames.max(1),
                gain_db: 0.0,
                fade_in_seconds: 0.0,
                fade_out_seconds: 0.0,
                reverse: false,
            }),
            color: None,
            muted: false,
        });
        t.clips.sort_by_key(|c| c.start);
        Some(id)
    }

    pub fn remove_clip(&mut self, track: TrackId, clip: ClipId) -> bool {
        if let Some(t) = self.track_mut(track) {
            let before = t.clips.len();
            t.clips.retain(|c| c.id != clip);
            return before != t.clips.len();
        }
        false
    }

    /// Splits a clip at an absolute tick. Returns the id of the new right-hand clip.
    pub fn split_clip(&mut self, track: TrackId, clip: ClipId, at: i64) -> Option<ClipId> {
        let bpm = self.bpm;
        let sr = self.sample_rate as f64;
        let t = self.track_mut(track)?;
        let index = t.clips.iter().position(|c| c.id == clip)?;
        let original = t.clips[index].clone();
        let length = original.length_ticks(bpm, sr);
        if at <= original.start || at >= original.start + length {
            return None;
        }
        let left_len_ticks = at - original.start;
        let new_id = ClipId(self.alloc_id());
        let spt = samples_per_tick(sr, bpm);
        let split_frames = ((left_len_ticks as f64) * spt).round() as i64;
        let mut right = original.clone();
        right.id = new_id;
        right.start = at;
        match (&original.kind, &mut right.kind) {
            (ClipKind::Pattern(_), ClipKind::Pattern(p)) => {
                p.length_ticks = (length - left_len_ticks).max(1);
            }
            (ClipKind::Audio(a), ClipKind::Audio(r)) => {
                if a.reverse {
                    // Playing backwards means the right-hand part starts earlier.
                    let a_end = a.offset_frames + a.length_frames;
                    let new_offset = (a_end - split_frames).max(0);
                    r.length_frames = (a_end - new_offset).max(1);
                    r.offset_frames = new_offset;
                } else {
                    r.offset_frames = a.offset_frames + split_frames;
                    r.length_frames = (a.length_frames - split_frames).max(1);
                }
            }
            _ => return None,
        }
        {
            let t = self.track_mut(track)?;
            if let Some(left) = t.clip_mut(clip) {
                match &mut left.kind {
                    ClipKind::Pattern(p) => p.length_ticks = left_len_ticks.max(1),
                    ClipKind::Audio(a) => a.length_frames = split_frames.max(1),
                }
            }
        }
        let t = self.track_mut(track)?;
        t.clips.push(right);
        t.clips.sort_by_key(|c| c.start);
        Some(new_id)
    }

    pub fn timeline_end_ticks(&self) -> i64 {
        let mut end = 0;
        for track in &self.tracks {
            end = end.max(track.end_tick(self.bpm, self.sample_rate as f64));
            for pattern in &track.patterns {
                end = end.max(pattern.end_tick());
            }
        }
        end.max(self.time_signature.ticks_per_bar())
    }

    pub fn media_keys(&self) -> Vec<String> {
        let mut keys = std::collections::BTreeSet::new();
        for track in &self.tracks {
            for clip in &track.clips {
                if let ClipKind::Audio(a) = &clip.kind {
                    if !a.media.is_empty() {
                        keys.insert(a.media.clone());
                    }
                }
            }
            if let Some(InstrumentKind::Sampler(s)) = track.instrument_kind() {
                if !s.media.is_empty() {
                    keys.insert(s.media.clone());
                }
            }
        }
        keys.into_iter().collect()
    }

    /// Re-establishes invariants after loading or structural edits.
    pub fn repair(&mut self) {
        self.format_version = FORMAT_VERSION;
        let mut next_id = self.next_id.max(1);
        self.bpm = self.bpm.clamp(20.0, 300.0);
        self.sample_rate = match self.sample_rate {
            44_100 | 48_000 | 88_200 | 96_000 => self.sample_rate,
            _ => 48_000,
        };
        if self.time_signature.numerator == 0 || self.time_signature.denominator == 0 {
            self.time_signature = TimeSignature::FOUR_FOUR;
        }
        let mut max_id = 0u64;
        for track in &mut self.tracks {
            max_id = max_id.max(track.id.0);
            track.volume_db = clamp_db(track.volume_db);
            track.pan = track.pan.clamp(-1.0, 1.0);
            track.height = track.height.clamp(48.0, 240.0);
            if track.is_audio() {
                track.instrument = None;
                track.patterns.clear();
                track.active_pattern = None;
            } else {
                if track.patterns.is_empty() {
                    let pattern_id = PatternId(next_id);
                    next_id += 1;
                    track.patterns.push(Pattern {
                        id: pattern_id,
                        name: "Pattern 1".to_string(),
                        length_ticks: self.time_signature.ticks_per_bar() * 2,
                        notes: Vec::new(),
                        color: track.color,
                    });
                }
                for pattern in &mut track.patterns {
                    max_id = max_id.max(pattern.id.0);
                    pattern.length_ticks = pattern.length_ticks.max(1);
                    pattern.notes.retain(|n| n.length > 0);
                    for note in &mut pattern.notes {
                        max_id = max_id.max(note.id.0);
                        note.pitch = note.pitch.min(127);
                        note.start = note.start.max(0);
                        note.length = note.length.max(1);
                    }
                }
                let active_valid = track
                    .active_pattern
                    .map(|id| track.patterns.iter().any(|p| p.id == id))
                    .unwrap_or(false);
                if !active_valid {
                    track.active_pattern = track.patterns.first().map(|p| p.id);
                }
            }
            let pattern_ids: Vec<PatternId> = track.patterns.iter().map(|p| p.id).collect();
            let first_pattern_id = track.patterns.first().map(|p| p.id);
            for clip in &mut track.clips {
                max_id = max_id.max(clip.id.0);
                clip.start = clip.start.max(0);
                if let ClipKind::Pattern(p) = &mut clip.kind {
                    p.length_ticks = p.length_ticks.max(1);
                    if !pattern_ids.contains(&p.pattern) {
                        if let Some(first) = first_pattern_id {
                            p.pattern = first;
                        }
                    }
                }
            }
            track.clips.sort_by_key(|c| c.start);
            for fx in &mut track.fx {
                max_id = max_id.max(fx.id.0);
            }
        }
        for fx in &mut self.master.fx {
            max_id = max_id.max(fx.id.0);
        }
        self.next_id = self.next_id.max(max_id + 1).max(next_id);
        if let Some((a, b)) = self.loop_region {
            let lo = a.min(b).max(0);
            let hi = a.max(b).max(lo + 1);
            self.loop_region = Some((lo, hi));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn demo_project_is_playable() {
        let p = Project::demo("Demo");
        assert_eq!(p.tracks.len(), 6);
        assert!(p.tracks.iter().all(|t| t.instrument.is_some()));
        assert!(p
            .tracks
            .iter()
            .all(|t| !t.clips.is_empty() && !t.patterns.is_empty()));
        assert_eq!(p.tracks.iter().map(|t| t.clips.len()).sum::<usize>(), 6);
        assert!(p.timeline_end_ticks() > 0);
    }

    #[test]
    fn ids_are_unique() {
        let mut p = Project::empty("x");
        let mut ids = std::collections::BTreeSet::new();
        for _ in 0..100 {
            assert!(ids.insert(p.alloc_id()));
        }
        p.repair();
        assert!(p.next_id > *ids.iter().max().unwrap());
    }

    #[test]
    fn solo_logic() {
        let mut p = Project::empty("x");
        let ids: Vec<TrackId> = p.tracks.iter().map(|t| t.id).collect();
        assert!(p.is_audible(p.track(ids[0]).unwrap()));
        p.track_mut(ids[1]).unwrap().solo = true;
        assert!(p.is_audible(p.track(ids[1]).unwrap()));
        assert!(!p.is_audible(p.track(ids[0]).unwrap()));
        p.track_mut(ids[1]).unwrap().mute = true;
        assert!(!p.is_audible(p.track(ids[1]).unwrap()));
    }

    #[test]
    fn step_toggle_round_trip() {
        let mut p = Project::empty("x");
        let track = p.tracks[0].id;
        let pattern = p.track(track).unwrap().active_pattern.unwrap();
        assert!(p.toggle_step(track, pattern, 0, 240, 36, 100));
        assert_eq!(
            p.track(track)
                .unwrap()
                .pattern(pattern)
                .unwrap()
                .notes
                .len(),
            1
        );
        assert!(p.toggle_step(track, pattern, 240, 240, 36, 100));
        assert_eq!(
            p.track(track)
                .unwrap()
                .pattern(pattern)
                .unwrap()
                .notes
                .len(),
            2
        );
        assert!(!p.toggle_step(track, pattern, 240, 240, 36, 100));
        assert_eq!(
            p.track(track)
                .unwrap()
                .pattern(pattern)
                .unwrap()
                .notes
                .len(),
            1
        );
        assert!(!p.toggle_step(track, pattern, 0, 240, 36, 100));
        assert_eq!(
            p.track(track)
                .unwrap()
                .pattern(pattern)
                .unwrap()
                .notes
                .len(),
            0
        );
    }

    #[test]
    fn split_audio_clip_keeps_durations() {
        let mut p = Project::empty("x");
        let audio = p.add_audio_track("Vocal");
        p.bpm = 120.0;
        let clip = p
            .add_audio_clip(audio, "vox.wav", "vox", 0, 96_000, 0)
            .unwrap();
        let right = p.split_clip(audio, clip, 1_920).expect("split");
        let t = p.track(audio).unwrap();
        assert_eq!(t.clips.len(), 2);
        let left = t.clip(clip).unwrap();
        let right = t.clip(right).unwrap();
        assert_eq!(left.start, 0);
        assert_eq!(right.start, 1_920);
        let total_frames: i64 = t
            .clips
            .iter()
            .map(|c| match &c.kind {
                ClipKind::Audio(a) => a.length_frames,
                _ => 0,
            })
            .sum();
        assert_eq!(total_frames, 96_000);
    }

    #[test]
    fn db_gain_conversion() {
        assert!((db_to_gain(0.0) - 1.0).abs() < 1e-6);
        assert_eq!(db_to_gain(MIN_DB), 0.0);
        assert!((db_to_gain(-6.0206) - 0.5).abs() < 1e-3);
        assert!((gain_to_db(1.0)).abs() < 1e-6);
    }

    #[test]
    fn repair_makes_invalid_project_usable() {
        let mut p = Project::empty("x");
        p.bpm = -10.0;
        p.sample_rate = 1234;
        p.tracks[0].volume_db = 400.0;
        p.tracks[0].pan = 9.0;
        p.repair();
        assert_eq!(p.bpm, 20.0);
        assert_eq!(p.sample_rate, 48_000);
        assert_eq!(p.tracks[0].volume_db, MAX_DB);
        assert_eq!(p.tracks[0].pan, 1.0);
    }
}

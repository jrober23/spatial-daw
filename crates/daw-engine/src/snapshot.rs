//! Immutable render snapshot.
//!
//! The UI edits a [`Project`]; the audio thread never touches it. Instead the
//! UI flattens the arrangement into absolute sample positions here, and hands
//! the resulting snapshot to the engine through a channel. This keeps the
//! realtime path allocation-free (beyond voice startup) and pattern lookup out
//! of the audio callback.

use std::sync::Arc;

use daw_core::model::{
    db_to_gain, ClipKind, DrumParams, FxSlot, InstrumentKind, PluginSpec, Project, SamplerParams,
    SynthParams, Track,
};
use daw_core::time::ticks_to_samples;
use daw_core::{MediaStore, TrackId};

use crate::cache::PlaybackCache;
use crate::dsp::pan_gains;

#[derive(Debug, Clone)]
pub struct Snapshot {
    pub sample_rate: u32,
    pub bpm: f64,
    pub time_signature: daw_core::model::TimeSignature,
    pub loop_region: Option<(u64, u64)>,
    pub metronome: bool,
    pub tracks: Vec<TrackSnapshot>,
    pub master: MasterSnapshot,
    pub length_samples: u64,
    pub revision: u64,
}

impl Snapshot {
    pub fn empty(sample_rate: u32) -> Arc<Self> {
        Arc::new(Self {
            sample_rate,
            bpm: 120.0,
            time_signature: daw_core::model::TimeSignature::FOUR_FOUR,
            loop_region: None,
            metronome: false,
            tracks: Vec::new(),
            master: MasterSnapshot {
                gain: 1.0,
                limiter: true,
                fx: Vec::new(),
            },
            length_samples: 0,
            revision: 0,
        })
    }

    pub fn build(
        project: &Project,
        media: &MediaStore,
        cache: &mut PlaybackCache,
        revision: u64,
    ) -> Arc<Self> {
        let sample_rate = cache.sample_rate();
        let sr = sample_rate as f64;
        let bpm = project.bpm.max(1.0);
        let ticks_to_frames =
            |ticks: f64| -> u64 { ticks_to_samples(ticks, sr, bpm).round().max(0.0) as u64 };

        let mut tracks = Vec::with_capacity(project.tracks.len());
        let mut length_samples = 0u64;

        for (index, track) in project.tracks.iter().enumerate() {
            let instrument = build_instrument(track, media, cache, sample_rate);
            let mut notes = Vec::new();
            let mut audio = Vec::new();

            if !track.is_audio() {
                flatten_pattern_clips(track, project, sr, &mut notes);
            }
            flatten_audio_clips(track, media, cache, sample_rate, bpm, &mut audio);

            if let Some(note) = notes.last() {
                length_samples = length_samples.max(note.start + note.duration);
            }
            for event in &audio {
                length_samples = length_samples.max(event.start + event.length);
            }

            let (pan_l, pan_r) = pan_gains(track.pan);
            tracks.push(TrackSnapshot {
                id: track.id,
                name: track.name.clone(),
                color: track.color,
                audible: project.is_audible(track),
                volume: db_to_gain(track.volume_db),
                volume_db: track.volume_db,
                pan_l: pan_l * std::f32::consts::SQRT_2,
                pan_r: pan_r * std::f32::consts::SQRT_2,
                instrument,
                notes,
                audio,
                fx: build_fx(&track.fx),
                meter_index: index,
            });
        }

        let loop_region = project.loop_region.map(|(a, b)| {
            let start = ticks_to_frames(a as f64);
            let end = ticks_to_frames(b as f64);
            (start.min(end), start.max(end).max(start.min(end) + 1))
        });
        if let Some((_, end)) = loop_region {
            length_samples = length_samples.max(end);
        }

        Arc::new(Self {
            sample_rate,
            bpm,
            time_signature: project.time_signature,
            loop_region,
            metronome: project.metronome,
            tracks,
            master: MasterSnapshot {
                gain: db_to_gain(project.master.volume_db),
                limiter: project.master.limiter,
                fx: build_fx(&project.master.fx),
            },
            length_samples,
            revision,
        })
    }

    pub fn track_count(&self) -> usize {
        self.tracks.len()
    }
}

fn build_fx(slots: &[FxSlot]) -> Vec<FxSnapshot> {
    slots
        .iter()
        .map(|slot| FxSnapshot {
            id: slot.id,
            enabled: slot.enabled,
            spec: slot.spec.clone(),
        })
        .collect()
}

#[derive(Debug, Clone)]
pub struct TrackSnapshot {
    pub id: TrackId,
    pub name: String,
    pub color: [u8; 3],
    pub audible: bool,
    pub volume: f32,
    pub volume_db: f32,
    pub pan_l: f32,
    pub pan_r: f32,
    pub instrument: InstrumentSnapshot,
    pub notes: Vec<NoteEvent>,
    pub audio: Vec<AudioEvent>,
    pub fx: Vec<FxSnapshot>,
    pub meter_index: usize,
}

#[derive(Debug, Clone)]
pub enum InstrumentSnapshot {
    Drum(DrumParams),
    Synth(SynthParams),
    Sampler(SamplerSnapshot),
    Plugin(PluginSpec),
}

impl InstrumentSnapshot {
    pub fn is_plugin(&self) -> bool {
        matches!(self, InstrumentSnapshot::Plugin(_))
    }

    pub fn as_kind(&self) -> Option<InstrumentKind> {
        match self {
            InstrumentSnapshot::Drum(p) => Some(InstrumentKind::Drum(p.clone())),
            InstrumentSnapshot::Synth(p) => Some(InstrumentKind::Synth(p.clone())),
            InstrumentSnapshot::Sampler(s) => Some(InstrumentKind::Sampler(s.params.clone())),
            InstrumentSnapshot::Plugin(_) => None,
        }
    }
}

#[derive(Debug, Clone)]
pub struct SamplerSnapshot {
    pub params: SamplerParams,
    /// Buffer at the engine sample rate, or `None` when no sample is loaded.
    pub buffer: Option<Arc<[f32]>>,
    pub buffer_sample_rate: u32,
}

#[derive(Debug, Clone, Copy)]
pub struct NoteEvent {
    pub start: u64,
    pub duration: u64,
    pub pitch: u8,
    pub velocity: u8,
}

#[derive(Debug, Clone)]
pub struct AudioEvent {
    pub start: u64,
    pub length: u64,
    /// Source offset in frames of the cached (engine-rate) buffer.
    pub offset: u64,
    pub buffer: Arc<[f32]>,
    pub gain: f32,
    pub fade_in: u64,
    pub fade_out: u64,
    pub reverse: bool,
}

#[derive(Debug, Clone)]
pub struct FxSnapshot {
    pub id: daw_core::PluginInstanceId,
    pub enabled: bool,
    pub spec: PluginSpec,
}

#[derive(Debug, Clone)]
pub struct MasterSnapshot {
    pub gain: f32,
    pub limiter: bool,
    pub fx: Vec<FxSnapshot>,
}

fn build_instrument(
    track: &Track,
    media: &MediaStore,
    cache: &mut PlaybackCache,
    sample_rate: u32,
) -> InstrumentSnapshot {
    match track.instrument_kind() {
        Some(InstrumentKind::Drum(params)) => InstrumentSnapshot::Drum(params.clone()),
        Some(InstrumentKind::Synth(params)) => InstrumentSnapshot::Synth(params.clone()),
        Some(InstrumentKind::Sampler(params)) => {
            let buffer = if params.media.is_empty() {
                None
            } else {
                cache.buffer(&params.media, media)
            };
            InstrumentSnapshot::Sampler(SamplerSnapshot {
                params: params.clone(),
                buffer,
                buffer_sample_rate: sample_rate,
            })
        }
        Some(InstrumentKind::Plugin(spec)) => InstrumentSnapshot::Plugin(spec.clone()),
        None => InstrumentSnapshot::Drum(DrumParams::new(daw_core::DrumSound::Kick)),
    }
}

fn flatten_pattern_clips(
    track: &Track,
    project: &Project,
    sample_rate: f64,
    out: &mut Vec<NoteEvent>,
) {
    let bpm = project.bpm.max(1.0);
    for clip in &track.clips {
        if clip.muted {
            continue;
        }
        let ClipKind::Pattern(pattern_clip) = &clip.kind else {
            continue;
        };
        let Some(pattern) = track.pattern(pattern_clip.pattern) else {
            continue;
        };
        let pattern_len = pattern.length_ticks.max(1);
        let clip_len = pattern_clip.length_ticks.max(1);
        for note in &pattern.notes {
            let mut local = note.start.max(0);
            // Repeat the pattern for as long as the clip covers it.
            while local < clip_len {
                let remaining = clip_len - local;
                let length = note.length.clamp(1, remaining.max(1));
                let start_tick = clip.start + local;
                let pitch = (note.pitch as i16 + pattern_clip.transpose as i16).clamp(0, 127);
                let start = ticks_to_samples(start_tick as f64, sample_rate, bpm).round() as u64;
                let duration = ticks_to_samples(length as f64, sample_rate, bpm)
                    .round()
                    .max(1.0) as u64;
                out.push(NoteEvent {
                    start,
                    duration,
                    pitch: pitch as u8,
                    velocity: note.velocity,
                });
                if pattern_len == 0 {
                    break;
                }
                local += pattern_len;
            }
        }
    }
    out.sort_by_key(|n| (n.start, n.pitch));
}

fn flatten_audio_clips(
    track: &Track,
    media: &MediaStore,
    cache: &mut PlaybackCache,
    sample_rate: u32,
    bpm: f64,
    out: &mut Vec<AudioEvent>,
) {
    for clip in &track.clips {
        if clip.muted {
            continue;
        }
        let ClipKind::Audio(audio) = &clip.kind else {
            continue;
        };
        if audio.length_frames <= 0 || audio.media.is_empty() {
            continue;
        }
        let Some(source) = media.get(&audio.media) else {
            continue;
        };
        let Some(buffer) = cache.buffer(&audio.media, media) else {
            continue;
        };
        let scale = sample_rate as f64 / source.sample_rate.max(1) as f64;
        let offset = (audio.offset_frames.max(0) as f64 * scale).round() as u64;
        let length = (audio.length_frames as f64 * scale).round().max(1.0) as u64;
        let buffer_frames = (buffer.len() / 2) as u64;
        if offset >= buffer_frames {
            continue;
        }
        let length = length.min(buffer_frames - offset);
        let start = ticks_to_samples(clip.start as f64, sample_rate as f64, bpm)
            .round()
            .max(0.0) as u64;
        out.push(AudioEvent {
            start,
            length,
            offset,
            buffer,
            gain: db_to_gain(audio.gain_db),
            fade_in: (audio.fade_in_seconds.max(0.0) * sample_rate as f32) as u64,
            fade_out: (audio.fade_out_seconds.max(0.0) * sample_rate as f32) as u64,
            reverse: audio.reverse,
        });
    }
    out.sort_by_key(|a| a.start);
}

#[cfg(test)]
mod tests {
    use super::*;
    use daw_core::ids::ClipId;
    use daw_core::model::{AudioClip, Clip, Project};

    fn project_with_pattern() -> Project {
        let mut p = Project::empty("Snapshot");
        let track = p.tracks[0].id;
        let pattern = p.track(track).unwrap().active_pattern.unwrap();
        for i in 0..4 {
            p.add_note(track, pattern, 36, i * 240, 240, 100);
        }
        p
    }

    #[test]
    fn demo_snapshot_has_notes_on_every_instrument() {
        let project = Project::demo("Snapshot");
        let media = MediaStore::new();
        let mut cache = PlaybackCache::new(48_000);
        let snap = Snapshot::build(&project, &media, &mut cache, 1);
        assert_eq!(snap.tracks.len(), 6);
        for track in &snap.tracks {
            assert!(!track.notes.is_empty(), "{} has no notes", track.name);
            assert!(track.audible);
            assert!(track.pan_l > 0.0 && track.pan_r > 0.0);
        }
        assert!(snap.length_samples > 0);
        assert!(snap.loop_region.is_some());
    }

    #[test]
    fn pattern_clips_tile_to_fill_longer_clip() {
        let mut p = project_with_pattern();
        let track = p.tracks[0].id;
        let pattern = p.track(track).unwrap().active_pattern.unwrap();
        // Remove the default clip and add one covering four pattern lengths.
        p.track_mut(track).unwrap().clips.clear();
        let pattern_len = p
            .track(track)
            .unwrap()
            .pattern(pattern)
            .unwrap()
            .length_ticks;
        p.add_pattern_clip(track, pattern, 0, pattern_len * 4);
        let snap = Snapshot::build(&p, &MediaStore::new(), &mut PlaybackCache::new(48_000), 0);
        assert_eq!(snap.tracks[0].notes.len(), 16);
    }

    #[test]
    fn mute_and_solo_reach_the_snapshot() {
        let mut p = project_with_pattern();
        let track = p.tracks[0].id;
        p.track_mut(track).unwrap().solo = true;
        let snap = Snapshot::build(&p, &MediaStore::new(), &mut PlaybackCache::new(48_000), 0);
        assert!(snap.tracks[0].audible);
        assert!(!snap.tracks[1].audible);
        p.track_mut(track).unwrap().mute = true;
        let snap = Snapshot::build(&p, &MediaStore::new(), &mut PlaybackCache::new(48_000), 0);
        assert!(!snap.tracks[0].audible);
    }

    #[test]
    fn audio_clip_is_scaled_to_engine_rate() {
        let mut p = Project::empty("Audio");
        let audio_track = p.add_audio_track("Vocal");
        p.bpm = 120.0;
        let mut media = MediaStore::new();
        let mut raw = Vec::new();
        for i in 0..44_100 {
            let s = (i as f32 * 0.01).sin();
            raw.push(s);
            raw.push(s);
        }
        let key = media.insert_decoded("vox.wav", 44_100, 2, &raw, None);
        let clip = Clip {
            id: ClipId(9_999),
            name: "vox".into(),
            start: 0,
            kind: ClipKind::Audio(AudioClip {
                media: key.clone(),
                offset_frames: 0,
                length_frames: 44_100,
                gain_db: 0.0,
                fade_in_seconds: 0.0,
                fade_out_seconds: 0.0,
                reverse: false,
            }),
            color: None,
            muted: false,
        };
        p.track_mut(audio_track).unwrap().clips.push(clip);
        p.repair();
        let mut cache = PlaybackCache::new(48_000);
        let snap = Snapshot::build(&p, &media, &mut cache, 0);
        let event = &snap.tracks.last().unwrap().audio[0];
        assert!((event.length as i64 - 48_000).abs() < 2);
        assert!(((event.buffer.len() / 2) as i64 - 48_000).abs() <= 2);
    }
}

//! One playing instrument voice.

use std::sync::Arc;

use daw_core::model::InstrumentKind;

use crate::dsp::drum::DrumVoice;
use crate::dsp::sampler::SamplerVoice;
use crate::dsp::synth::SynthVoice;

#[derive(Debug, Clone)]
pub enum Voice {
    Drum(DrumVoice),
    Synth(SynthVoice),
    Sampler(SamplerVoice),
}

impl Voice {
    pub fn process(&mut self) -> (f32, f32) {
        match self {
            Voice::Drum(v) => v.process(),
            Voice::Synth(v) => {
                let s = v.process();
                (s, s)
            }
            Voice::Sampler(v) => v.process(),
        }
    }

    pub fn note_off(&mut self) {
        match self {
            Voice::Drum(v) => v.note_off(),
            Voice::Synth(v) => v.note_off(),
            Voice::Sampler(v) => v.note_off(),
        }
    }

    pub fn is_active(&self) -> bool {
        match self {
            Voice::Drum(v) => v.is_active(),
            Voice::Synth(v) => v.is_active(),
            Voice::Sampler(v) => v.is_active(),
        }
    }

    pub fn is_releasing(&self) -> bool {
        match self {
            Voice::Drum(_) => false,
            Voice::Synth(v) => v.is_releasing(),
            Voice::Sampler(v) => !v.is_active(),
        }
    }

    /// Pitch of a synth voice, used for voice stealing and glide.
    pub fn note(&self) -> Option<u8> {
        match self {
            Voice::Synth(v) => Some(v.note()),
            _ => None,
        }
    }
}

/// Audio-thread sample data for a sampler instrument.
#[derive(Clone)]
pub struct SamplerSource {
    pub buffer: Arc<[f32]>,
    pub sample_rate: u32,
}

pub fn make_voice(
    kind: &InstrumentKind,
    note: u8,
    velocity: u8,
    sample_rate: f32,
    sampler: Option<&SamplerSource>,
    glide_from: Option<f32>,
) -> Option<Voice> {
    match kind {
        InstrumentKind::Drum(params) => Some(Voice::Drum(DrumVoice::trigger(
            params,
            note,
            velocity,
            sample_rate,
        ))),
        InstrumentKind::Synth(params) => Some(Voice::Synth(SynthVoice::trigger(
            params,
            note,
            velocity,
            sample_rate,
            glide_from,
        ))),
        InstrumentKind::Sampler(params) => {
            let source = sampler?;
            SamplerVoice::trigger(
                params,
                note,
                velocity,
                sample_rate,
                source.buffer.clone(),
                source.sample_rate,
            )
            .map(Voice::Sampler)
        }
        InstrumentKind::Plugin(_) => None,
    }
}

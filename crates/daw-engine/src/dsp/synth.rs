//! Polyphonic subtractive synthesiser voice.

use daw_core::model::{db_to_gain, SynthParams, Waveform};

use crate::dsp::env::Envelope;
use crate::dsp::filter::Svf;
use crate::dsp::midi_to_hz;
use crate::dsp::osc::Oscillator;

use std::f32::consts::TAU;

#[derive(Debug, Clone)]
pub struct SynthVoice {
    patch: SynthParams,
    sample_rate: f32,
    note: u8,
    velocity: f32,
    base_freq: f32,
    current_freq: f32,
    glide_remaining: f32,
    glide_factor: f32,
    osc1: Oscillator,
    osc2: Oscillator,
    noise: Oscillator,
    amp_env: Envelope,
    filter_env: Envelope,
    filter: Svf,
    lfo_phase: f32,
    control_counter: u32,
    active: bool,
}

impl SynthVoice {
    pub fn trigger(
        patch: &SynthParams,
        note: u8,
        velocity: u8,
        sample_rate: f32,
        glide_from: Option<f32>,
    ) -> Self {
        let base_freq = midi_to_hz(note as f32 + patch.octave as f32 * 12.0);
        let start = glide_from.filter(|f| *f > 0.0).unwrap_or(base_freq);
        let glide_samples = (patch.glide.max(0.0) * sample_rate).max(0.0);
        let (current_freq, glide_factor, glide_remaining) =
            if glide_samples > 1.0 && (start - base_freq).abs() > 0.01 {
                let factor = (base_freq / start).powf(1.0 / glide_samples);
                (start, factor, glide_samples)
            } else {
                (base_freq, 1.0, 0.0)
            };

        let mut amp_env = Envelope::new(sample_rate);
        amp_env.set_params(patch.amp_env);
        amp_env.note_on();
        let mut filter_env = Envelope::new(sample_rate);
        filter_env.set_params(patch.filter_env);
        filter_env.note_on();

        Self {
            patch: patch.clone(),
            sample_rate,
            note,
            velocity: 0.1 + 0.9 * (velocity as f32 / 127.0),
            base_freq,
            current_freq,
            glide_remaining,
            glide_factor,
            osc1: Oscillator::new(),
            osc2: Oscillator::with_seed(0x51A3_2C7D),
            noise: Oscillator::with_seed(0xBEEF_1234),
            amp_env,
            filter_env,
            filter: Svf::new(),
            lfo_phase: 0.0,
            control_counter: 0,
            active: true,
        }
    }

    pub fn note_off(&mut self) {
        self.amp_env.note_off();
    }

    pub fn is_active(&self) -> bool {
        self.active && self.amp_env.is_active()
    }

    pub fn is_releasing(&self) -> bool {
        self.amp_env.is_releasing()
    }

    pub fn note(&self) -> u8 {
        self.note
    }

    pub fn process(&mut self) -> f32 {
        if !self.active {
            return 0.0;
        }
        let amp = self.amp_env.process();
        if !self.amp_env.is_active() {
            self.active = false;
            return 0.0;
        }
        let fenv = self.filter_env.process();
        let patch = &self.patch;
        let sr = self.sample_rate;

        let lfo = if patch.lfo_rate_hz > 0.0 {
            let value = (TAU * self.lfo_phase).sin();
            self.lfo_phase = (self.lfo_phase + patch.lfo_rate_hz / sr).fract();
            value
        } else {
            0.0
        };

        if self.glide_remaining > 0.0 {
            self.current_freq *= self.glide_factor;
            self.glide_remaining -= 1.0;
            if self.glide_remaining <= 0.0 {
                self.current_freq = self.base_freq;
            }
        } else {
            self.current_freq = self.base_freq;
        }
        let freq = self.current_freq * 2f32.powf(patch.lfo_pitch * lfo / 12.0);

        let detune = 2f32.powf(patch.detune_cents / 1_200.0);
        let s1 = self.osc1.next(patch.osc1, freq, sr);
        let s2 = self.osc2.next(patch.osc2, freq * detune, sr);
        let n = if patch.noise > 0.0 {
            self.noise.next(Waveform::Noise, 0.0, sr)
        } else {
            0.0
        };
        let norm = 1.0 + patch.osc2_level + patch.noise;
        let raw = (s1 + s2 * patch.osc2_level + n * patch.noise) / norm.max(1.0);

        self.control_counter = self.control_counter.wrapping_add(1);
        if self.control_counter % 16 == 0 {
            let key_offset = patch.key_track * (self.note as f32 - 60.0) / 12.0;
            let env_offset = patch.filter_env_amount * fenv * 4.0;
            let lfo_offset = patch.lfo_cutoff * lfo * 2.0;
            let cutoff = patch.cutoff_hz * 2f32.powf(key_offset + env_offset + lfo_offset);
            self.filter
                .set(sr, cutoff, patch.resonance, patch.filter_mode);
        }

        let filtered = self.filter.process(raw);
        let velocity =
            1.0 - patch.velocity_sensitivity + patch.velocity_sensitivity * self.velocity;
        let output = filtered * amp * db_to_gain(patch.level_db) * velocity;
        if output.is_finite() {
            output
        } else {
            0.0
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use daw_core::model::Adsr;

    #[test]
    fn synth_renders_and_releases() {
        let patch = SynthParams {
            amp_env: Adsr::new(0.001, 0.1, 0.5, 0.05),
            filter_env: Adsr::new(0.001, 0.1, 0.0, 0.05),
            ..Default::default()
        };
        let mut voice = SynthVoice::trigger(&patch, 60, 100, 48_000.0, None);
        let mut energy = 0.0;
        for _ in 0..4_800 {
            energy += voice.process().abs();
        }
        assert!(energy > 1.0);
        voice.note_off();
        for _ in 0..48_000 {
            voice.process();
        }
        assert!(!voice.is_active());
    }

    #[test]
    fn glide_slides_between_notes() {
        let patch = SynthParams {
            glide: 0.05,
            ..Default::default()
        };
        let mut voice = SynthVoice::trigger(&patch, 72, 100, 48_000.0, Some(110.0));
        assert!((voice.current_freq - 110.0).abs() < 0.01);
        for _ in 0..4_800 {
            voice.process();
        }
        assert!((voice.current_freq - 523.25).abs() < 5.0);
    }

    #[test]
    fn high_resonance_is_stable() {
        let patch = SynthParams {
            resonance: 0.98,
            cutoff_hz: 3_000.0,
            ..Default::default()
        };
        let mut voice = SynthVoice::trigger(&patch, 36, 127, 48_000.0, None);
        for _ in 0..48_000 {
            let s = voice.process();
            assert!(s.is_finite());
            assert!(s.abs() < 12.0);
        }
    }
}

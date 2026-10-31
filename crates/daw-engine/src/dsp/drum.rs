//! Synthesised drum voices (one drum per instrument track).

use daw_core::model::{db_to_gain, DrumParams, DrumSound, FilterMode, Waveform};

use crate::dsp::env::ExpEnv;
use crate::dsp::filter::Svf;
use crate::dsp::osc::Oscillator;
use crate::dsp::pan_gains;

use std::f32::consts::TAU;

#[derive(Debug, Clone)]
pub struct DrumVoice {
    params: DrumParams,
    sample_rate: f32,
    amp: ExpEnv,
    pitch: ExpEnv,
    click: ExpEnv,
    noise: Oscillator,
    filter: Svf,
    phase: f32,
    phase2: f32,
    metallic_phase: f32,
    time: f32,
    active: bool,
    gain: f32,
    pan_l: f32,
    pan_r: f32,
    velocity: f32,
    tune: f32,
}

impl DrumVoice {
    pub fn trigger(params: &DrumParams, _pitch: u8, velocity: u8, sample_rate: f32) -> Self {
        let (pan_l, pan_r) = pan_gains(params.pan);
        let velocity = 0.15 + 0.85 * (velocity as f32 / 127.0);
        let tune = 2f32.powf(params.tune / 12.0);
        let mut voice = Self {
            params: params.clone(),
            sample_rate,
            amp: ExpEnv::new(),
            pitch: ExpEnv::new(),
            click: ExpEnv::new(),
            noise: Oscillator::with_seed(0x1234_5678),
            filter: Svf::new(),
            phase: 0.0,
            phase2: 0.0,
            metallic_phase: 0.0,
            time: 0.0,
            active: true,
            gain: db_to_gain(params.level_db),
            pan_l,
            pan_r,
            velocity,
            tune,
        };
        let decay = params.decay.clamp(0.15, 4.0);
        match params.sound {
            DrumSound::Kick => {
                voice.amp.trigger(0.34 * decay, sample_rate);
                voice.pitch.trigger(0.055, sample_rate);
                voice.click.trigger(0.006, sample_rate);
            }
            DrumSound::Snare => {
                voice.amp.trigger(0.19 * decay, sample_rate);
                voice.click.trigger(0.16 * decay, sample_rate);
                voice
                    .filter
                    .set(sample_rate, 1_700.0 * tune, 0.45, FilterMode::BandPass);
            }
            DrumSound::Clap => {
                voice.amp.trigger(0.22 * decay, sample_rate);
                voice.click.trigger(0.25 * decay, sample_rate);
                voice
                    .filter
                    .set(sample_rate, 1_250.0 * tune, 0.55, FilterMode::BandPass);
            }
            DrumSound::HatClosed => {
                voice.amp.trigger(0.045 * decay, sample_rate);
                voice
                    .filter
                    .set(sample_rate, 7_500.0 * tune, 0.2, FilterMode::HighPass);
            }
            DrumSound::HatOpen => {
                voice.amp.trigger(0.33 * decay, sample_rate);
                voice
                    .filter
                    .set(sample_rate, 7_000.0 * tune, 0.2, FilterMode::HighPass);
            }
            DrumSound::Tom => {
                voice.amp.trigger(0.32 * decay, sample_rate);
                voice.pitch.trigger(0.14, sample_rate);
                voice
                    .filter
                    .set(sample_rate, 2_200.0 * tune, 0.1, FilterMode::LowPass);
            }
            DrumSound::Rim => {
                voice.amp.trigger(0.035 * decay, sample_rate);
                voice.click.trigger(0.03, sample_rate);
                voice
                    .filter
                    .set(sample_rate, 1_900.0 * tune, 0.5, FilterMode::BandPass);
            }
            DrumSound::Crash => {
                voice.amp.trigger(1.5 * decay, sample_rate);
                voice
                    .filter
                    .set(sample_rate, 4_500.0 * tune, 0.15, FilterMode::HighPass);
            }
        }
        voice
    }

    pub fn note_off(&mut self) {
        // One-shots ignore note-off; they ring out naturally.
    }

    pub fn is_active(&self) -> bool {
        self.active
    }

    pub fn process(&mut self) -> (f32, f32) {
        if !self.active {
            return (0.0, 0.0);
        }
        let amp = self.amp.process();
        if !self.amp.is_active() {
            self.active = false;
            return (0.0, 0.0);
        }
        self.time += 1.0 / self.sample_rate;
        let sr = self.sample_rate;
        let tone = self.params.tone.clamp(0.0, 1.0);
        let tune = self.tune;

        let sample = match self.params.sound {
            DrumSound::Kick => {
                let pitch = self.pitch.process();
                let freq = (42.0 + 125.0 * pitch) * tune;
                self.phase = (self.phase + freq / sr).fract();
                let body = (TAU * self.phase).sin();
                let click = self.noise.next(Waveform::Noise, 0.0, sr)
                    * self.click.process()
                    * (0.25 + tone * 0.4);
                body + click
            }
            DrumSound::Snare => {
                self.phase = (self.phase + 185.0 * tune / sr).fract();
                self.phase2 = (self.phase2 + 335.0 * tune / sr).fract();
                let body = (TAU * self.phase).sin() * 0.35 + (TAU * self.phase2).sin() * 0.22;
                let noise = self
                    .filter
                    .process(self.noise.next(Waveform::Noise, 0.0, sr));
                noise * (0.75 + tone * 0.35) + body * (1.0 - tone * 0.4)
            }
            DrumSound::Clap => {
                let noise = self
                    .filter
                    .process(self.noise.next(Waveform::Noise, 0.0, sr));
                let cell = (self.time * 95.0).fract();
                let rattle = if cell < 0.28 { 1.0 } else { 0.35 };
                noise * rattle * (0.9 + tone * 0.3)
            }
            DrumSound::HatClosed | DrumSound::HatOpen => {
                self.metallic_phase = (self.metallic_phase + 317.0 * tune / sr).fract();
                let m = (TAU * self.metallic_phase).sin() * 0.5
                    + (TAU * self.metallic_phase * 1.617).sin() * 0.3
                    + (TAU * self.metallic_phase * 2.473).sin() * 0.2;
                let noise = self
                    .filter
                    .process(self.noise.next(Waveform::Noise, 0.0, sr));
                noise * (0.55 + 0.45 * m) * (1.0 + tone * 0.3)
            }
            DrumSound::Tom => {
                let pitch = self.pitch.process();
                let freq = (95.0 + 120.0 * pitch) * tune;
                self.phase = (self.phase + freq / sr).fract();
                let body = (TAU * self.phase).sin();
                self.filter.process(body) * (0.9 + tone * 0.2)
            }
            DrumSound::Rim => {
                self.phase = (self.phase + 420.0 * tune / sr).fract();
                let body = (TAU * self.phase).sin() * 0.5;
                let click = self
                    .filter
                    .process(self.noise.next(Waveform::Noise, 0.0, sr))
                    * self.click.process();
                body + click
            }
            DrumSound::Crash => {
                self.metallic_phase = (self.metallic_phase + 523.0 * tune / sr).fract();
                let m = (TAU * self.metallic_phase).sin() * 0.4
                    + (TAU * self.metallic_phase * 1.414).sin() * 0.3
                    + (TAU * self.metallic_phase * 2.111).sin() * 0.3;
                let noise = self
                    .filter
                    .process(self.noise.next(Waveform::Noise, 0.0, sr));
                noise * (0.55 + 0.45 * m)
            }
        };

        let mut out = sample * amp * self.gain * self.velocity;
        // Gentle saturation gives drums weight without hard clipping.
        out = (out * (1.0 + tone * 4.0)).tanh() / (1.0 + tone * 2.0);
        if !out.is_finite() {
            out = 0.0;
        }
        (out * self.pan_l, out * self.pan_r)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn render(sound: DrumSound) -> Vec<f32> {
        let params = DrumParams::new(sound);
        let mut voice = DrumVoice::trigger(&params, sound.default_pitch(), 120, 48_000.0);
        let mut out = Vec::new();
        for _ in 0..48_000 {
            let (l, _) = voice.process();
            out.push(l);
            if !voice.is_active() {
                break;
            }
        }
        out
    }

    #[test]
    fn every_drum_produces_audio_and_ends() {
        for sound in DrumSound::ALL {
            let out = render(sound);
            assert!(out.len() > 100, "{} too short", sound.label());
            assert!(
                out.iter().any(|s| s.abs() > 1e-4),
                "{} silent",
                sound.label()
            );
            assert!(out.iter().all(|s| s.is_finite()));
        }
    }

    #[test]
    fn kick_is_low_and_snare_is_brighter() {
        let kick = render(DrumSound::Kick);
        let snare = render(DrumSound::Snare);
        let zero_crossings =
            |buf: &[f32]| buf.windows(2).filter(|w| w[0] <= 0.0 && w[1] > 0.0).count();
        // Compare the first 100 ms only.
        let n = 4_800;
        assert!(zero_crossings(&snare[..n]) > zero_crossings(&kick[..n]));
    }

    #[test]
    fn panning_produces_stereo_difference() {
        let mut params = DrumParams::new(DrumSound::HatClosed);
        params.pan = -1.0;
        let mut voice = DrumVoice::trigger(&params, 39, 127, 48_000.0);
        let mut left = 0.0;
        let mut right = 0.0;
        for _ in 0..2_000 {
            let (l, r) = voice.process();
            left += l.abs();
            right += r.abs();
        }
        assert!(left > right * 10.0);
    }
}

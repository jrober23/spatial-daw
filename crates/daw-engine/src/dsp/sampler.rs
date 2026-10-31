//! Sample playback voice.

use std::sync::Arc;

use daw_core::model::{db_to_gain, LoopMode, SamplerParams};

use crate::dsp::env::Envelope;

#[derive(Debug, Clone)]
pub struct SamplerVoice {
    samples: Arc<[f32]>,
    frames: usize,
    pos: f64,
    step: f64,
    sample_rate: f32,
    env: Envelope,
    gain: f32,
    velocity_gain: f32,
    loop_enabled: bool,
    loop_start: usize,
    loop_end: usize,
    reverse: bool,
    active: bool,
}

impl SamplerVoice {
    pub fn trigger(
        params: &SamplerParams,
        note: u8,
        velocity: u8,
        sample_rate: f32,
        buffer: Arc<[f32]>,
        buffer_sample_rate: u32,
    ) -> Option<Self> {
        let frames = buffer.len() / 2;
        if frames == 0 {
            return None;
        }
        let semitones = note as f32 - params.root_note as f32 + params.transpose as f32;
        let pitch_ratio = 2f32.powf(semitones / 12.0) as f64;
        let rate_ratio = buffer_sample_rate as f64 / sample_rate as f64;
        let step = pitch_ratio * rate_ratio;

        let mut env = Envelope::new(sample_rate);
        env.set_params(params.amp_env);
        env.note_on();

        let loop_start = ((params.loop_start.clamp(0.0, 1.0) * frames as f32) as usize)
            .min(frames.saturating_sub(1));
        let loop_end = ((params.loop_end.clamp(0.0, 1.0) * frames as f32) as usize)
            .clamp(loop_start + 1, frames);
        let loop_enabled =
            params.loop_mode == LoopMode::Forward && !params.reverse && loop_end > loop_start + 1;

        let pos = if params.reverse {
            frames as f64 - 1.0
        } else if loop_enabled {
            loop_start as f64
        } else {
            0.0
        };

        Some(Self {
            samples: buffer,
            frames,
            pos,
            step: if params.reverse { -step } else { step },
            sample_rate,
            env,
            gain: db_to_gain(params.gain_db),
            velocity_gain: 1.0 - params.velocity_sensitivity
                + params.velocity_sensitivity * (velocity as f32 / 127.0),
            loop_enabled,
            loop_start,
            loop_end,
            reverse: params.reverse,
            active: true,
        })
    }

    pub fn note_off(&mut self) {
        self.env.note_off();
    }

    pub fn is_active(&self) -> bool {
        self.active && self.env.is_active()
    }

    fn sample_at(&self, frame: i64) -> (f32, f32) {
        if frame < 0 || frame as usize >= self.frames {
            return (0.0, 0.0);
        }
        let index = frame as usize * 2;
        (self.samples[index], self.samples[index + 1])
    }

    pub fn process(&mut self) -> (f32, f32) {
        if !self.active {
            return (0.0, 0.0);
        }
        let amp = self.env.process();
        if !self.env.is_active() {
            self.active = false;
            return (0.0, 0.0);
        }
        if self.pos < 0.0 || self.pos >= self.frames as f64 {
            self.active = false;
            return (0.0, 0.0);
        }
        let base = self.pos.floor();
        let frac = (self.pos - base) as f32;
        let (l0, r0) = self.sample_at(base as i64);
        let (l1, r1) = self.sample_at(base as i64 + 1);
        let l = l0 + (l1 - l0) * frac;
        let r = r0 + (r1 - r0) * frac;

        self.pos += self.step;
        if self.loop_enabled && self.step > 0.0 {
            if self.pos >= self.loop_end as f64 {
                let span = (self.loop_end - self.loop_start) as f64;
                self.pos = self.loop_start as f64 + (self.pos - self.loop_end as f64) % span;
            }
        } else if self.step > 0.0 && self.pos >= self.frames as f64 {
            self.active = false;
        } else if self.step < 0.0 && self.pos < 0.0 {
            self.active = false;
        }

        let gain = amp * self.gain * self.velocity_gain;
        let _ = self.sample_rate;
        let _ = self.reverse;
        (l * gain, r * gain)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use daw_core::model::Adsr;

    fn ramp(frames: usize) -> Arc<[f32]> {
        let mut v = Vec::with_capacity(frames * 2);
        for i in 0..frames {
            let s = i as f32 / frames as f32;
            v.push(s);
            v.push(s);
        }
        Arc::from(v)
    }

    #[test]
    fn plays_forward_and_stops() {
        let params = SamplerParams {
            amp_env: Adsr::new(0.001, 0.01, 1.0, 0.01),
            ..Default::default()
        };
        let mut v = SamplerVoice::trigger(&params, 60, 100, 48_000.0, ramp(4_800), 48_000).unwrap();
        let mut heard = 0.0;
        for _ in 0..10_000 {
            let (l, _) = v.process();
            heard += l.abs();
            if !v.is_active() {
                break;
            }
        }
        assert!(heard > 1.0);
        assert!(!v.is_active());
    }

    #[test]
    fn pitch_shift_shortens_playback() {
        let params = SamplerParams::default();
        let mut low =
            SamplerVoice::trigger(&params, 60, 100, 48_000.0, ramp(48_000), 48_000).unwrap();
        let mut high =
            SamplerVoice::trigger(&params, 72, 100, 48_000.0, ramp(48_000), 48_000).unwrap();
        let mut low_steps = 0;
        let mut high_steps = 0;
        for _ in 0..48_000 {
            low.process();
            high.process();
            if low.is_active() {
                low_steps += 1;
            }
            if high.is_active() {
                high_steps += 1;
            }
        }
        assert!(high_steps < low_steps);
    }

    #[test]
    fn loop_keeps_playing_until_released() {
        let params = SamplerParams {
            loop_mode: LoopMode::Forward,
            loop_start: 0.0,
            loop_end: 0.5,
            amp_env: Adsr::new(0.001, 0.01, 1.0, 0.01),
            ..Default::default()
        };
        let mut v = SamplerVoice::trigger(&params, 60, 100, 48_000.0, ramp(4_800), 48_000).unwrap();
        for _ in 0..48_000 {
            v.process();
        }
        assert!(v.is_active());
        v.note_off();
        for _ in 0..2_000 {
            v.process();
        }
        assert!(!v.is_active());
    }
}

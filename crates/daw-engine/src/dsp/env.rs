//! Envelopes.

use daw_core::model::Adsr;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Stage {
    Idle,
    Attack,
    Decay,
    Sustain,
    Release,
}

/// Linear ADSR envelope with click-free release from the current level.
#[derive(Debug, Clone)]
pub struct Envelope {
    sample_rate: f32,
    adsr: Adsr,
    stage: Stage,
    level: f32,
    release_start: f32,
}

impl Envelope {
    pub fn new(sample_rate: f32) -> Self {
        Self {
            sample_rate: sample_rate.max(1.0),
            adsr: Adsr::default(),
            stage: Stage::Idle,
            level: 0.0,
            release_start: 0.0,
        }
    }

    pub fn set_params(&mut self, adsr: Adsr) {
        self.adsr = Adsr::new(adsr.attack, adsr.decay, adsr.sustain, adsr.release);
    }

    pub fn note_on(&mut self) {
        self.stage = Stage::Attack;
        self.level = 0.0;
        self.release_start = 0.0;
    }

    pub fn note_off(&mut self) {
        if matches!(self.stage, Stage::Attack | Stage::Decay | Stage::Sustain) {
            self.stage = Stage::Release;
            self.release_start = self.level.max(1e-4);
        }
    }

    pub fn is_active(&self) -> bool {
        self.stage != Stage::Idle
    }

    pub fn is_releasing(&self) -> bool {
        self.stage == Stage::Release
    }

    pub fn level(&self) -> f32 {
        self.level
    }

    pub fn process(&mut self) -> f32 {
        match self.stage {
            Stage::Idle => 0.0,
            Stage::Attack => {
                let step = 1.0 / (self.adsr.attack.max(0.0005) * self.sample_rate);
                self.level += step;
                if self.level >= 1.0 {
                    self.level = 1.0;
                    self.stage = Stage::Decay;
                }
                self.level
            }
            Stage::Decay => {
                let sustain = self.adsr.sustain.clamp(0.0, 1.0);
                let step = (1.0 - sustain) / (self.adsr.decay.max(0.001) * self.sample_rate);
                self.level -= step;
                if self.level <= sustain {
                    self.level = sustain;
                    self.stage = Stage::Sustain;
                }
                self.level
            }
            Stage::Sustain => self.adsr.sustain.clamp(0.0, 1.0),
            Stage::Release => {
                let step = self.release_start / (self.adsr.release.max(0.001) * self.sample_rate);
                self.level -= step;
                if self.level <= 0.0 {
                    self.level = 0.0;
                    self.stage = Stage::Idle;
                }
                self.level
            }
        }
    }
}

/// Exponential one-shot decay, used for drum amplitudes and pitch sweeps.
#[derive(Debug, Clone)]
pub struct ExpEnv {
    level: f32,
    coeff: f32,
    active: bool,
}

impl ExpEnv {
    pub fn new() -> Self {
        Self {
            level: 0.0,
            coeff: 0.0,
            active: false,
        }
    }

    pub fn trigger(&mut self, seconds: f32, sample_rate: f32) {
        self.level = 1.0;
        self.coeff = (-1.0 / (seconds.max(0.001) * sample_rate.max(1.0))).exp();
        self.active = true;
    }

    pub fn retrigger(&mut self) {
        self.level = 1.0;
        self.active = true;
    }

    pub fn process(&mut self) -> f32 {
        if !self.active {
            return 0.0;
        }
        let value = self.level;
        self.level *= self.coeff;
        if self.level < 1e-5 {
            self.level = 0.0;
            self.active = false;
        }
        value
    }

    pub fn is_active(&self) -> bool {
        self.active
    }

    pub fn value(&self) -> f32 {
        self.level
    }
}

impl Default for ExpEnv {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn adsr_reaches_sustain_and_releases() {
        let mut env = Envelope::new(48_000.0);
        env.set_params(Adsr::new(0.01, 0.05, 0.5, 0.1));
        env.note_on();
        for _ in 0..48_000 {
            env.process();
        }
        assert!((env.level() - 0.5).abs() < 0.01);
        assert!(env.is_active());
        env.note_off();
        for _ in 0..48_000 {
            env.process();
        }
        assert!(!env.is_active());
        assert_eq!(env.level(), 0.0);
    }

    #[test]
    fn exp_env_decays_to_zero() {
        let mut e = ExpEnv::new();
        e.trigger(0.05, 48_000.0);
        let first = e.process();
        assert!(first > 0.9);
        for _ in 0..48_000 {
            e.process();
        }
        assert!(!e.is_active());
        assert_eq!(e.process(), 0.0);
    }
}

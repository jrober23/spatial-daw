//! Band-limited oscillators and noise.

use daw_core::model::Waveform;

#[derive(Debug, Clone)]
pub struct Oscillator {
    phase: f32,
    noise_state: u32,
}

impl Oscillator {
    pub fn new() -> Self {
        Self {
            phase: 0.0,
            noise_state: 0x9E37_79B9,
        }
    }

    pub fn with_seed(seed: u32) -> Self {
        Self {
            phase: 0.0,
            noise_state: seed.max(1),
        }
    }

    pub fn reset(&mut self) {
        self.phase = 0.0;
    }

    pub fn set_phase(&mut self, phase: f32) {
        self.phase = phase.fract();
    }

    pub fn phase(&self) -> f32 {
        self.phase
    }

    /// Produces one sample. `frequency` may be zero for noise.
    pub fn next(&mut self, waveform: Waveform, frequency: f32, sample_rate: f32) -> f32 {
        let dt = (frequency.abs() / sample_rate.max(1.0)).clamp(0.0, 0.45);
        let t = self.phase;
        let value = match waveform {
            Waveform::Sine => (std::f32::consts::TAU * t).sin(),
            Waveform::Saw => 2.0 * t - 1.0 - poly_blep(t, dt),
            Waveform::Square => {
                let mut s = if t < 0.5 { 1.0 } else { -1.0 };
                s += poly_blep(t, dt);
                s -= poly_blep((t + 0.5).fract(), dt);
                s
            }
            Waveform::Triangle => 4.0 * (t - 0.5).abs() - 1.0,
            Waveform::Noise => self.noise(),
        };
        self.phase += dt;
        if self.phase >= 1.0 {
            self.phase -= 1.0;
        }
        value
    }

    fn noise(&mut self) -> f32 {
        let mut x = self.noise_state;
        x ^= x << 13;
        x ^= x >> 17;
        x ^= x << 5;
        self.noise_state = x;
        (x as f32 / u32::MAX as f32) * 2.0 - 1.0
    }
}

impl Default for Oscillator {
    fn default() -> Self {
        Self::new()
    }
}

fn poly_blep(t: f32, dt: f32) -> f32 {
    if dt <= 0.0 {
        return 0.0;
    }
    if t < dt {
        let x = t / dt;
        x + x - x * x - 1.0
    } else if t > 1.0 - dt {
        let x = (t - 1.0) / dt;
        x * x + x + x + 1.0
    } else {
        0.0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rms(buffer: &[f32]) -> f32 {
        (buffer.iter().map(|s| s * s).sum::<f32>() / buffer.len() as f32).sqrt()
    }

    #[test]
    fn sine_has_expected_frequency() {
        let sr = 48_000.0;
        let mut osc = Oscillator::new();
        let mut crossings: i32 = 0;
        let mut prev = 0.0;
        for _ in 0..48_000 {
            let s = osc.next(Waveform::Sine, 100.0, sr);
            if prev <= 0.0 && s > 0.0 {
                crossings += 1;
            }
            prev = s;
        }
        // 100 Hz over one second, one rising crossing per cycle.
        assert!((crossings - 100).abs() <= 1, "crossings={crossings}");
    }

    #[test]
    fn oscillators_are_bounded() {
        for wf in Waveform::ALL {
            let mut osc = Oscillator::new();
            for _ in 0..10_000 {
                let s = osc.next(wf, 220.0, 48_000.0);
                assert!(s.is_finite());
                assert!(s.abs() <= 1.5);
            }
        }
    }

    #[test]
    fn noise_is_not_silent() {
        let mut osc = Oscillator::new();
        let buf: Vec<f32> = (0..4_800)
            .map(|_| osc.next(Waveform::Noise, 0.0, 48_000.0))
            .collect();
        assert!(rms(&buf) > 0.5);
    }

    #[test]
    fn aliasing_is_reduced_by_polyblep() {
        let mut osc = Oscillator::new();
        let buf: Vec<f32> = (0..48_000)
            .map(|_| osc.next(Waveform::Saw, 4_000.0, 48_000.0))
            .collect();
        assert!(rms(&buf) > 0.4);
        assert!(buf.iter().all(|s| s.abs() < 1.3));
    }
}

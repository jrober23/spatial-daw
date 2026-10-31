//! Filters.

use daw_core::model::FilterMode;

/// Topology-preserving state variable filter (Zavalishin).
#[derive(Debug, Clone)]
pub struct Svf {
    ic1: f32,
    ic2: f32,
    a1: f32,
    a2: f32,
    a3: f32,
    k: f32,
    mode: FilterMode,
}

impl Svf {
    pub fn new() -> Self {
        let mut filter = Self {
            ic1: 0.0,
            ic2: 0.0,
            a1: 0.0,
            a2: 0.0,
            a3: 0.0,
            k: 1.0,
            mode: FilterMode::LowPass,
        };
        filter.set(48_000.0, 1_000.0, 0.0, FilterMode::LowPass);
        filter
    }

    pub fn set(&mut self, sample_rate: f32, cutoff_hz: f32, resonance: f32, mode: FilterMode) {
        let cutoff = cutoff_hz.clamp(20.0, sample_rate.max(48_000.0) * 0.45);
        let g = (std::f32::consts::PI * cutoff / sample_rate.max(1.0)).tan();
        let k = 2.0 - resonance.clamp(0.0, 0.98) * 1.96;
        self.a1 = 1.0 / (1.0 + g * (g + k));
        self.a2 = g * self.a1;
        self.a3 = g * self.a2;
        self.k = k;
        self.mode = mode;
    }

    pub fn reset(&mut self) {
        self.ic1 = 0.0;
        self.ic2 = 0.0;
    }

    pub fn process(&mut self, x: f32) -> f32 {
        let v3 = x - self.ic2;
        let v1 = self.a1 * self.ic1 + self.a2 * v3;
        let v2 = self.ic2 + self.a2 * self.ic1 + self.a3 * v3;
        self.ic1 = 2.0 * v1 - self.ic1;
        self.ic2 = 2.0 * v2 - self.ic2;
        match self.mode {
            FilterMode::LowPass => v2,
            FilterMode::BandPass => v1,
            FilterMode::HighPass => x - self.k * v1 - v2,
        }
    }
}

impl Default for Svf {
    fn default() -> Self {
        Self::new()
    }
}

/// Simple one-pole low pass used for metering and smoothing.
#[derive(Debug, Clone)]
pub struct OnePole {
    z: f32,
    coeff: f32,
}

impl OnePole {
    pub fn new(cutoff_hz: f32, sample_rate: f32) -> Self {
        let mut filter = Self { z: 0.0, coeff: 0.0 };
        filter.set_cutoff(cutoff_hz, sample_rate);
        filter
    }

    pub fn set_cutoff(&mut self, cutoff_hz: f32, sample_rate: f32) {
        let cutoff = cutoff_hz.clamp(1.0, sample_rate * 0.45);
        self.coeff = 1.0 - (-std::f32::consts::TAU * cutoff / sample_rate.max(1.0)).exp();
    }

    pub fn process(&mut self, x: f32) -> f32 {
        self.z += self.coeff * (x - self.z);
        self.z
    }

    pub fn reset(&mut self) {
        self.z = 0.0;
    }
}

/// Splits a stereo pair into low/mid/high bands using a pair of one-poles.
/// This is deliberately cheap: it drives the 3D field's vertical axis, not
/// the audio output.
#[derive(Debug, Clone)]
pub struct BandSplitter {
    low_l: OnePole,
    low_r: OnePole,
    highcut_l: OnePole,
    highcut_r: OnePole,
}

impl BandSplitter {
    pub fn new(sample_rate: f32) -> Self {
        Self {
            low_l: OnePole::new(200.0, sample_rate),
            low_r: OnePole::new(200.0, sample_rate),
            highcut_l: OnePole::new(2_500.0, sample_rate),
            highcut_r: OnePole::new(2_500.0, sample_rate),
        }
    }

    pub fn split(&mut self, l: f32, r: f32) -> (f32, f32, f32) {
        let low = (self.low_l.process(l) + self.low_r.process(r)) * 0.5;
        let below = (self.highcut_l.process(l) + self.highcut_r.process(r)) * 0.5;
        let mono = (l + r) * 0.5;
        let high = mono - below;
        let mid = below - low;
        (low, mid, high)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sine_at(hz: f32) -> Vec<f32> {
        (0..48_000)
            .map(|i| (std::f32::consts::TAU * hz * i as f32 / 48_000.0).sin())
            .collect()
    }

    fn rms(buf: &[f32]) -> f32 {
        (buf.iter().map(|s| s * s).sum::<f32>() / buf.len() as f32).sqrt()
    }

    #[test]
    fn low_pass_attenuates_highs() {
        let low = sine_at(200.0);
        let high = sine_at(8_000.0);
        let mut f1 = Svf::new();
        f1.set(48_000.0, 1_000.0, 0.2, FilterMode::LowPass);
        let mut f2 = f1.clone();
        let low_out: Vec<f32> = low.iter().map(|s| f1.process(*s)).collect();
        let high_out: Vec<f32> = high.iter().map(|s| f2.process(*s)).collect();
        assert!(rms(&low_out) > rms(&high_out) * 3.0);
    }

    #[test]
    fn high_pass_attenuates_lows() {
        let low = sine_at(80.0);
        let high = sine_at(6_000.0);
        let mut f1 = Svf::new();
        f1.set(48_000.0, 1_000.0, 0.2, FilterMode::HighPass);
        let mut f2 = f1.clone();
        let low_out: Vec<f32> = low.iter().map(|s| f1.process(*s)).collect();
        let high_out: Vec<f32> = high.iter().map(|s| f2.process(*s)).collect();
        assert!(rms(&high_out) > rms(&low_out) * 3.0);
    }

    #[test]
    fn filter_stays_finite_at_extremes() {
        let mut f = Svf::new();
        f.set(48_000.0, 19_000.0, 0.98, FilterMode::BandPass);
        for i in 0..48_000 {
            let s = if i % 2 == 0 { 1.0 } else { -1.0 };
            assert!(f.process(s).is_finite());
        }
    }

    #[test]
    fn band_splitter_separates_content() {
        let mut splitter = BandSplitter::new(48_000.0);
        let mut low_energy = 0.0;
        let mut high_energy = 0.0;
        for i in 0..4_800 {
            let s = (std::f32::consts::TAU * 80.0 * i as f32 / 48_000.0).sin();
            let (low, _, _) = splitter.split(s, s);
            low_energy += low * low;
        }
        let mut splitter = BandSplitter::new(48_000.0);
        for i in 0..4_800 {
            let s = (std::f32::consts::TAU * 9_000.0 * i as f32 / 48_000.0).sin();
            let (_, _, high) = splitter.split(s, s);
            high_energy += high * high;
        }
        assert!(low_energy > high_energy);
    }
}

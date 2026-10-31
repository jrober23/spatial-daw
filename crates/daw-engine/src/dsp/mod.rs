//! Sample-accurate DSP primitives used by the instrument voices and the mixer.

pub mod drum;
pub mod env;
pub mod filter;
pub mod osc;
pub mod sampler;
pub mod synth;
pub mod voice;

pub use drum::DrumVoice;
pub use env::{Envelope, ExpEnv};
pub use filter::{BandSplitter, OnePole, Svf};
pub use osc::Oscillator;
pub use sampler::SamplerVoice;
pub use synth::SynthVoice;
pub use voice::{make_voice, SamplerSource, Voice};

/// Constant-power pan gains for a -1..1 position.
pub fn pan_gains(pan: f32) -> (f32, f32) {
    let angle = (pan.clamp(-1.0, 1.0) + 1.0) * (std::f32::consts::FRAC_PI_4);
    (angle.cos(), angle.sin())
}

/// Equal-tempered frequency for a MIDI note.
pub fn midi_to_hz(note: f32) -> f32 {
    440.0 * 2f32.powf((note - 69.0) / 12.0)
}

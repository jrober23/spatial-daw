//! Realtime audio engine for Spatial DAW.
//!
//! The UI never touches the audio callback directly. It builds an immutable
//! [`snapshot::Snapshot`] from the project, sends it to the engine, and reads
//! transport/meter state back through [`engine::SharedState`].

pub mod cache;
pub mod devices;
pub mod dsp;
pub mod engine;
pub mod export;
pub mod import;
pub mod midi;
pub mod plugin;
pub mod snapshot;

pub use cache::PlaybackCache;
pub use devices::{AudioSession, DeviceInfo, DeviceSelection, HostInfo};
pub use engine::{
    measurements, render_offline, EngineCommand, EngineHandle, MeterBank, SharedState, TrackMeter,
};
pub use export::export_mixdown;
pub use import::{decode_file, DecodedAudio};
pub use midi::{MidiHub, MidiPort};
pub use plugin::{PluginDescriptor, PluginParamInfo};
pub use snapshot::Snapshot;

pub fn version() -> &'static str {
    env!("CARGO_PKG_VERSION")
}

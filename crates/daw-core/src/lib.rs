//! Core data model for Spatial DAW.
//!
//! This crate deliberately has no audio or UI dependencies. It owns the
//! serialisable song document, the media store, and the spatial-mix theory
//! (see [`gibson`]) that drives the 3D mixer.

pub mod gibson;
pub mod ids;
pub mod media;
pub mod model;
pub mod project_io;
pub mod time;

pub use gibson::{
    db_to_depth, depth_to_db, frequency_to_height, height_to_frequency, ElementRole, MixAnalysis,
    MixStyle, SpatialPosition, StyleTarget, TrackAnalysis, TrackMeasurement,
};
pub use ids::{ClipId, NoteId, PatternId, PluginInstanceId, TrackId};
pub use media::{MediaItem, MediaStore, Peak};
pub use model::{
    AudioClip, Clip, ClipKind, DrumParams, DrumSound, FxSlot, Instrument, InstrumentKind,
    MasterBus, Note, Pattern, PatternClip, PluginFormat, PluginSpec, Project, SamplerParams,
    SpatialMeta, SynthParams, TimeSignature, Track, TrackKind, Waveform, FORMAT_VERSION,
};
pub use project_io::{load_project, save_project};
pub use time::{Grid, TICKS_PER_BEAT};

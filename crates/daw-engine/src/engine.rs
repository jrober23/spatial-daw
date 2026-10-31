//! Realtime engine: transport, voice allocation, track mixing, metering and
//! plugin inserts.
//!
//! Processing is done in fixed 64-frame chunks. That keeps plugin processing
//! simple (a constant maximum block size), makes loop edges land within ~1.3 ms
//! of the requested tick, and lets the engine accept any device buffer size.

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Instant;

use crossbeam_channel::{Receiver, Sender};
use daw_core::model::{gain_to_db, Project};
use daw_core::{MediaStore, TrackId};
use parking_lot::Mutex;

use crate::cache::PlaybackCache;
use crate::dsp::{make_voice, midi_to_hz, SamplerSource, Voice};
use crate::plugin::{insert_key, instrument_key, PluginHost, PluginParamInfo, PluginRequest};
use crate::snapshot::{AudioEvent, InstrumentSnapshot, NoteEvent, Snapshot, TrackSnapshot};

/// Internal processing quantum.
pub const CHUNK: usize = 64;
/// Maximum number of tracks the meter bank can represent.
pub const MAX_TRACKS: usize = 512;
/// Maximum number of simultaneous voices per track.
pub const MAX_VOICES: usize = 32;

// ---------------------------------------------------------------------------
// Shared state
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, Default)]
pub struct TrackMeter {
    pub rms_db: f32,
    pub peak_db: f32,
    pub low: f32,
    pub mid: f32,
    pub high: f32,
    pub active_notes: u32,
}

fn store_f32(slot: &AtomicU32, value: f32) {
    slot.store(value.to_bits(), Ordering::Relaxed);
}

fn load_f32(slot: &AtomicU32) -> f32 {
    f32::from_bits(slot.load(Ordering::Relaxed))
}

#[derive(Debug)]
pub struct MeterBank {
    track_count: AtomicU32,
    rms_db: Vec<AtomicU32>,
    peak_db: Vec<AtomicU32>,
    low: Vec<AtomicU32>,
    mid: Vec<AtomicU32>,
    high: Vec<AtomicU32>,
    active: Vec<AtomicU32>,
}

impl MeterBank {
    pub fn new() -> Self {
        let make = || {
            (0..MAX_TRACKS)
                .map(|_| AtomicU32::new(0))
                .collect::<Vec<_>>()
        };
        Self {
            track_count: AtomicU32::new(0),
            rms_db: make(),
            peak_db: make(),
            low: make(),
            mid: make(),
            high: make(),
            active: make(),
        }
    }

    pub fn set_track_count(&self, count: usize) {
        self.track_count
            .store(count.min(MAX_TRACKS) as u32, Ordering::Relaxed);
    }

    pub fn write(&self, index: usize, meter: TrackMeter) {
        if index >= MAX_TRACKS {
            return;
        }
        store_f32(&self.rms_db[index], meter.rms_db);
        store_f32(&self.peak_db[index], meter.peak_db);
        store_f32(&self.low[index], meter.low);
        store_f32(&self.mid[index], meter.mid);
        store_f32(&self.high[index], meter.high);
        self.active[index].store(meter.active_notes, Ordering::Relaxed);
    }

    pub fn read(&self, index: usize) -> TrackMeter {
        if index >= MAX_TRACKS {
            return TrackMeter::default();
        }
        TrackMeter {
            rms_db: load_f32(&self.rms_db[index]),
            peak_db: load_f32(&self.peak_db[index]),
            low: load_f32(&self.low[index]),
            mid: load_f32(&self.mid[index]),
            high: load_f32(&self.high[index]),
            active_notes: self.active[index].load(Ordering::Relaxed),
        }
    }

    pub fn clear(&self) {
        for index in 0..MAX_TRACKS {
            self.write(index, TrackMeter::default());
        }
    }
}

impl Default for MeterBank {
    fn default() -> Self {
        Self::new()
    }
}

#[derive(Debug)]
pub struct SharedState {
    pub sample_rate: AtomicU32,
    pub playhead: AtomicU64,
    pub playing: AtomicBool,
    pub cpu_load: AtomicU32,
    pub master_rms_l: AtomicU32,
    pub master_rms_r: AtomicU32,
    pub master_peak_l: AtomicU32,
    pub master_peak_r: AtomicU32,
    pub master_clip: AtomicBool,
    pub midi_target: AtomicU64,
    pub midi_activity: AtomicU32,
    pub meters: MeterBank,
    pub plugin_params: Mutex<HashMap<u64, Vec<PluginParamInfo>>>,
    pub plugin_errors: Mutex<Vec<String>>,
}

impl SharedState {
    pub fn new() -> Arc<Self> {
        Arc::new(Self {
            sample_rate: AtomicU32::new(48_000),
            playhead: AtomicU64::new(0),
            playing: AtomicBool::new(false),
            cpu_load: AtomicU32::new(0),
            master_rms_l: AtomicU32::new(0),
            master_rms_r: AtomicU32::new(0),
            master_peak_l: AtomicU32::new(0),
            master_peak_r: AtomicU32::new(0),
            master_clip: AtomicBool::new(false),
            midi_target: AtomicU64::new(0),
            midi_activity: AtomicU32::new(0),
            meters: MeterBank::new(),
            plugin_params: Mutex::new(HashMap::new()),
            plugin_errors: Mutex::new(Vec::new()),
        })
    }

    pub fn master_peaks(&self) -> (f32, f32, f32, f32) {
        (
            load_f32(&self.master_rms_l),
            load_f32(&self.master_rms_r),
            load_f32(&self.master_peak_l),
            load_f32(&self.master_peak_r),
        )
    }

    pub fn cpu_load(&self) -> f32 {
        load_f32(&self.cpu_load)
    }
}

/// Handle used by the UI. Anything fallible is sent as a command; the audio
/// thread owns the actual engine.
#[derive(Clone)]
pub struct EngineHandle {
    commands: Sender<EngineCommand>,
    shared: Arc<SharedState>,
    sample_rate: u32,
}

impl EngineHandle {
    pub fn new(
        commands: Sender<EngineCommand>,
        shared: Arc<SharedState>,
        sample_rate: u32,
    ) -> Self {
        Self {
            commands,
            shared,
            sample_rate,
        }
    }

    pub fn send(&self, command: EngineCommand) -> bool {
        self.commands.send(command).is_ok()
    }

    pub fn command_sender(&self) -> Sender<EngineCommand> {
        self.commands.clone()
    }

    pub fn shared(&self) -> &Arc<SharedState> {
        &self.shared
    }

    pub fn sample_rate(&self) -> u32 {
        self.sample_rate
    }
}

#[derive(Debug)]
pub enum EngineCommand {
    SetSnapshot(Arc<Snapshot>),
    Play,
    Pause,
    Stop,
    SeekSamples(u64),
    SeekTicks(i64),
    NoteOn {
        track: TrackId,
        pitch: u8,
        velocity: u8,
    },
    NoteOff {
        track: TrackId,
        pitch: u8,
    },
    AllNotesOff,
    SetPluginParam {
        key: u64,
        param: u32,
        value: f64,
    },
    SetMidiTarget(TrackId),
    Shutdown,
}

// ---------------------------------------------------------------------------
// Engine
// ---------------------------------------------------------------------------

struct TransportState {
    playing: bool,
    position: u64,
    loop_region: Option<(u64, u64)>,
}

struct TrackRuntime {
    voices: Vec<VoiceSlot>,
    plugin_notes: Vec<PluginNote>,
    last_pitch: Option<u8>,
    scratch_l: Vec<f32>,
    scratch_r: Vec<f32>,
    splitter: crate::dsp::BandSplitter,
    meter_rms: f32,
    meter_peak: f32,
    smooth_low: f32,
    smooth_mid: f32,
    smooth_high: f32,
}

impl TrackRuntime {
    fn new(sample_rate: u32) -> Self {
        Self {
            voices: Vec::new(),
            plugin_notes: Vec::new(),
            last_pitch: None,
            scratch_l: vec![0.0; CHUNK],
            scratch_r: vec![0.0; CHUNK],
            splitter: crate::dsp::BandSplitter::new(sample_rate as f32),
            meter_rms: -90.0,
            meter_peak: -90.0,
            smooth_low: 0.0,
            smooth_mid: 0.0,
            smooth_high: 0.0,
        }
    }
}

struct VoiceSlot {
    voice: Voice,
    remaining: u64,
    released: bool,
    start_offset: usize,
    pitch: u8,
}

struct PluginNote {
    pitch: u8,
    remaining: u64,
}

struct LiveNote {
    track: TrackId,
    pitch: u8,
    velocity: u8,
}

pub struct Engine {
    snapshot: Arc<Snapshot>,
    runtimes: Vec<TrackRuntime>,
    transport: TransportState,
    sample_rate: u32,
    plugins: PluginHost,
    shared: Arc<SharedState>,
    commands: Receiver<EngineCommand>,
    chunk_l: Vec<f32>,
    chunk_r: Vec<f32>,
    live_notes: Vec<LiveNote>,
    shutdown: bool,
}

impl Engine {
    pub fn new(
        sample_rate: u32,
        snapshot: Arc<Snapshot>,
        shared: Arc<SharedState>,
        commands: Receiver<EngineCommand>,
    ) -> Self {
        let track_count = snapshot.tracks.len();
        let mut engine = Self {
            snapshot,
            runtimes: (0..track_count)
                .map(|_| TrackRuntime::new(sample_rate))
                .collect(),
            transport: TransportState {
                playing: false,
                position: 0,
                loop_region: None,
            },
            sample_rate,
            plugins: PluginHost::new(sample_rate as f64, CHUNK),
            shared,
            commands,
            chunk_l: vec![0.0; CHUNK],
            chunk_r: vec![0.0; CHUNK],
            live_notes: Vec::new(),
            shutdown: false,
        };
        engine
            .shared
            .sample_rate
            .store(sample_rate, Ordering::Relaxed);
        engine
            .shared
            .meters
            .set_track_count(engine.snapshot.tracks.len());
        engine.apply_snapshot(engine.snapshot.clone());
        engine
    }

    fn drain_commands(&mut self) {
        for _ in 0..512 {
            match self.commands.try_recv() {
                Ok(command) => self.handle_command(command),
                Err(_) => break,
            }
        }
    }

    fn handle_command(&mut self, command: EngineCommand) {
        match command {
            EngineCommand::SetSnapshot(snapshot) => {
                self.apply_snapshot(snapshot);
            }
            EngineCommand::Play => {
                self.transport.playing = true;
                self.shared.playing.store(true, Ordering::Relaxed);
            }
            EngineCommand::Pause => {
                self.transport.playing = false;
                self.shared.playing.store(false, Ordering::Relaxed);
                self.release_all();
            }
            EngineCommand::Stop => {
                self.transport.playing = false;
                self.transport.position = 0;
                self.shared.playing.store(false, Ordering::Relaxed);
                self.shared.playhead.store(0, Ordering::Relaxed);
                self.release_all();
                self.shared.meters.clear();
            }
            EngineCommand::SeekSamples(position) => {
                self.transport.position = position;
                self.shared.playhead.store(position, Ordering::Relaxed);
                self.release_all();
            }
            EngineCommand::SeekTicks(ticks) => {
                let bpm = self.snapshot.bpm.max(1.0);
                let sr = self.sample_rate as f64;
                let samples =
                    daw_core::time::ticks_to_samples(ticks.max(0) as f64, sr, bpm).round() as u64;
                self.transport.position = samples;
                self.shared.playhead.store(samples, Ordering::Relaxed);
                self.release_all();
            }
            EngineCommand::NoteOn {
                track,
                pitch,
                velocity,
            } => {
                self.live_notes.push(LiveNote {
                    track,
                    pitch,
                    velocity,
                });
                self.shared.midi_activity.fetch_add(1, Ordering::Relaxed);
            }
            EngineCommand::NoteOff { track, pitch } => {
                if let Some(index) = self.snapshot.tracks.iter().position(|t| t.id == track) {
                    self.runtimes[index].release_pitch(pitch);
                    let key = instrument_key(track);
                    self.plugins.note_off(key, pitch, 0);
                }
            }
            EngineCommand::AllNotesOff => self.release_all(),
            EngineCommand::SetPluginParam { key, param, value } => {
                if self.plugins.set_param(key, param, value) {
                    if let Some(params) = self.plugins.params(key) {
                        self.shared
                            .plugin_params
                            .lock()
                            .insert(key, params.to_vec());
                    }
                }
            }
            EngineCommand::SetMidiTarget(track) => {
                self.shared.midi_target.store(track.0, Ordering::Relaxed);
            }
            EngineCommand::Shutdown => {
                self.shutdown = true;
                self.transport.playing = false;
                self.release_all();
            }
        }
    }

    fn release_all(&mut self) {
        for runtime in &mut self.runtimes {
            for slot in &mut runtime.voices {
                slot.voice.note_off();
                slot.released = true;
                slot.remaining = 0;
            }
            runtime.voices.clear();
            runtime.plugin_notes.clear();
        }
        self.plugins.all_notes_off();
        self.live_notes.clear();
    }

    fn apply_snapshot(&mut self, snapshot: Arc<Snapshot>) {
        let old = std::mem::replace(&mut self.snapshot, snapshot);
        let loop_region = self.snapshot.loop_region;
        self.transport.loop_region = loop_region;
        self.shared
            .meters
            .set_track_count(self.snapshot.tracks.len());

        let mut old_runtimes: Vec<Option<TrackRuntime>> =
            self.runtimes.drain(..).map(Some).collect();
        let mut new_runtimes = Vec::with_capacity(self.snapshot.tracks.len());
        for (index, track) in self.snapshot.tracks.iter().enumerate() {
            let mut runtime = if index < old_runtimes.len() {
                old_runtimes[index]
                    .take()
                    .unwrap_or_else(|| TrackRuntime::new(self.sample_rate))
            } else {
                TrackRuntime::new(self.sample_rate)
            };
            let same_track = old.tracks.get(index).map(|t| t.id) == Some(track.id);
            let same_instrument = old
                .tracks
                .get(index)
                .map(|t| same_instrument(&t.instrument, &track.instrument))
                .unwrap_or(false);
            if !same_track || !same_instrument {
                runtime.voices.clear();
                runtime.plugin_notes.clear();
                runtime.last_pitch = None;
            }
            new_runtimes.push(runtime);
        }
        self.runtimes = new_runtimes;
        self.sync_plugins();
    }

    fn sync_plugins(&mut self) {
        let mut requests = Vec::new();
        for track in &self.snapshot.tracks {
            if let InstrumentSnapshot::Plugin(spec) = &track.instrument {
                requests.push(PluginRequest {
                    key: instrument_key(track.id),
                    spec: spec.clone(),
                });
            }
            for fx in &track.fx {
                requests.push(PluginRequest {
                    key: insert_key(fx.id),
                    spec: fx.spec.clone(),
                });
            }
        }
        for fx in &self.snapshot.master.fx {
            requests.push(PluginRequest {
                key: insert_key(fx.id),
                spec: fx.spec.clone(),
            });
        }
        let params = self.plugins.sync(requests);
        self.shared.plugin_params.lock().clone_from(&params);
        let errors = self.plugins.take_errors();
        if !errors.is_empty() {
            self.shared.plugin_errors.lock().extend(errors);
        }
    }

    // -- transport ---------------------------------------------------------

    fn advance(&mut self, frames: usize) {
        self.transport.position = self.transport.position.saturating_add(frames as u64);
        if let Some((start, end)) = self.transport.loop_region {
            if end > start && self.transport.position >= end {
                let span = end - start;
                self.transport.position = start + (self.transport.position - end) % span;
            }
        }
    }

    // -- render ------------------------------------------------------------

    fn render_chunk(&mut self, frames: usize) {
        let frames = frames.min(CHUNK);
        self.chunk_l[..frames].fill(0.0);
        self.chunk_r[..frames].fill(0.0);
        let position = self.transport.position;
        let end = position + frames as u64;

        // Live notes are triggered at the top of the chunk.
        let live = std::mem::take(&mut self.live_notes);
        for note in live {
            if let Some(index) = self.snapshot.tracks.iter().position(|t| t.id == note.track) {
                let track = &self.snapshot.tracks[index];
                let runtime = &mut self.runtimes[index];
                trigger_single(
                    runtime,
                    track,
                    &mut self.plugins,
                    self.sample_rate as f32,
                    note.pitch,
                    note.velocity,
                    0,
                    u64::MAX / 2,
                );
            }
        }

        let track_count = self.snapshot.tracks.len();
        for index in 0..track_count {
            let track = &self.snapshot.tracks[index];
            let runtime = &mut self.runtimes[index];
            runtime.scratch_l[..frames].fill(0.0);
            runtime.scratch_r[..frames].fill(0.0);

            if !track.audible {
                runtime.meter_rms = (runtime.meter_rms + -90.0) * 0.5;
                runtime.meter_peak = -90.0;
                runtime.smooth_low *= 0.5;
                runtime.smooth_mid *= 0.5;
                runtime.smooth_high *= 0.5;
                self.shared
                    .meters
                    .write(index, meter_from_runtime(runtime, 0));
                continue;
            }

            let start_index = track.notes.partition_point(|n| n.start < position);
            trigger_events(
                runtime,
                track,
                &mut self.plugins,
                self.sample_rate as f32,
                &track.notes[start_index..],
                position,
                end,
            );
            if matches!(track.instrument, InstrumentSnapshot::Plugin(_)) {
                let key = instrument_key(track.id);
                for note in &mut runtime.plugin_notes {
                    if note.remaining <= frames as u64 {
                        let offset = note.remaining.min(frames as u64) as usize;
                        self.plugins.note_off(
                            key,
                            note.pitch,
                            offset.min(frames.saturating_sub(1)),
                        );
                        note.remaining = 0;
                    } else {
                        note.remaining -= frames as u64;
                    }
                }
                runtime.plugin_notes.retain(|n| n.remaining > 0);

                self.plugins.process(
                    key,
                    &mut runtime.scratch_l[..frames],
                    &mut runtime.scratch_r[..frames],
                );
            } else {
                render_voices(runtime, frames);
            }

            mix_audio_events(
                &track.audio,
                position,
                frames,
                &mut runtime.scratch_l,
                &mut runtime.scratch_r,
            );

            for fx in &track.fx {
                if !fx.enabled {
                    continue;
                }
                self.plugins.process(
                    insert_key(fx.id),
                    &mut runtime.scratch_l[..frames],
                    &mut runtime.scratch_r[..frames],
                );
            }

            // Fader and pan.
            let gain_l = track.volume * track.pan_l;
            let gain_r = track.volume * track.pan_r;
            let mut sum_l = 0.0f32;
            let mut sum_r = 0.0f32;
            for i in 0..frames {
                runtime.scratch_l[i] *= gain_l;
                runtime.scratch_r[i] *= gain_r;
                sum_l += runtime.scratch_l[i] * runtime.scratch_l[i];
                sum_r += runtime.scratch_r[i] * runtime.scratch_r[i];
            }
            for i in 0..frames {
                self.chunk_l[i] += runtime.scratch_l[i];
                self.chunk_r[i] += runtime.scratch_r[i];
            }

            // Metering: RMS and a cheap 3-band split for the spatial field.
            let rms = ((sum_l + sum_r) / (frames.max(1) as f32 * 2.0)).sqrt();
            let rms_db = gain_to_db(rms);
            let peak_db = {
                let peak = runtime
                    .scratch_l
                    .iter()
                    .take(frames)
                    .chain(runtime.scratch_r.iter().take(frames))
                    .fold(0.0f32, |acc, s| acc.max(s.abs()));
                gain_to_db(peak)
            };
            runtime.meter_rms = if rms_db > runtime.meter_rms {
                rms_db
            } else {
                runtime.meter_rms * 0.82 + rms_db * 0.18
            };
            runtime.meter_peak = peak_db;

            let mut low_energy = 0.0f32;
            let mut mid_energy = 0.0f32;
            let mut high_energy = 0.0f32;
            {
                for i in 0..frames {
                    let band = runtime
                        .splitter
                        .split(runtime.scratch_l[i], runtime.scratch_r[i]);
                    low_energy += band.0 * band.0;
                    mid_energy += band.1 * band.1;
                    high_energy += band.2 * band.2;
                }
            }
            let norm = frames.max(1) as f32;
            let low = (low_energy / norm).sqrt().min(1.0);
            let mid = (mid_energy / norm).sqrt().min(1.0);
            let high = (high_energy / norm).sqrt().min(1.0);
            runtime.smooth_low = runtime.smooth_low * 0.8 + low * 0.2;
            runtime.smooth_mid = runtime.smooth_mid * 0.8 + mid * 0.2;
            runtime.smooth_high = runtime.smooth_high * 0.8 + high * 0.2;

            let active = runtime.voices.len() as u32 + runtime.plugin_notes.len() as u32;
            self.shared.meters.write(
                index,
                TrackMeter {
                    rms_db: runtime.meter_rms,
                    peak_db: runtime.meter_peak,
                    low: runtime.smooth_low,
                    mid: runtime.smooth_mid,
                    high: runtime.smooth_high,
                    active_notes: active,
                },
            );
        }

        let click = self.metronome_click_frames(position, frames);
        for (offset, accent) in click {
            add_click(
                &mut self.chunk_l,
                &mut self.chunk_r,
                offset,
                accent,
                self.sample_rate as f32,
            );
        }

        for fx in &self.snapshot.master.fx {
            if !fx.enabled {
                continue;
            }
            self.plugins.process(
                insert_key(fx.id),
                &mut self.chunk_l[..frames],
                &mut self.chunk_r[..frames],
            );
        }

        let master_gain = self.snapshot.master.gain;
        let limiter = self.snapshot.master.limiter;
        let mut clip = false;
        let mut peak_l = 0.0f32;
        let mut peak_r = 0.0f32;
        for i in 0..frames {
            let mut l = self.chunk_l[i] * master_gain;
            let mut r = self.chunk_r[i] * master_gain;
            if limiter {
                l = l.tanh();
                r = r.tanh();
            }
            if l.abs() > 1.0 || r.abs() > 1.0 {
                clip = true;
            }
            peak_l = peak_l.max(l.abs());
            peak_r = peak_r.max(r.abs());
            self.chunk_l[i] = l;
            self.chunk_r[i] = r;
        }
        self.shared.master_clip.store(
            clip || self.shared.master_clip.load(Ordering::Relaxed),
            Ordering::Relaxed,
        );
        store_f32(&self.shared.master_peak_l, gain_to_db(peak_l));
        store_f32(&self.shared.master_peak_r, gain_to_db(peak_r));
    }

    fn metronome_click_frames(&self, position: u64, frames: usize) -> Vec<(usize, bool)> {
        if !self.snapshot.metronome {
            return Vec::new();
        }
        let bpm = self.snapshot.bpm.max(1.0);
        let samples_per_beat = self.sample_rate as f64 * 60.0 / bpm;
        if samples_per_beat <= 1.0 {
            return Vec::new();
        }
        let numerator = self.snapshot.time_signature.numerator.max(1) as u64;
        let first = (position as f64 / samples_per_beat).ceil() as u64;
        let last = ((position + frames as u64) as f64 / samples_per_beat).ceil() as u64;
        let mut out = Vec::new();
        for beat in first..=last {
            let sample = (beat as f64 * samples_per_beat).round() as u64;
            if sample >= position && sample < position + frames as u64 {
                out.push(((sample - position) as usize, beat % numerator == 0));
            }
        }
        out
    }

    /// Renders interleaved output. `channels` must be at least 1; stereo is
    /// mapped directly, mono is summed, extra channels are silent.
    pub fn process_interleaved(&mut self, output: &mut [f32], channels: usize) {
        let channels = channels.max(1);
        self.drain_commands();
        let frames = output.len() / channels;
        if frames == 0 {
            return;
        }
        let started = Instant::now();
        let playing = self.transport.playing;

        if !playing {
            self.chunk_l[..CHUNK.min(frames)].fill(0.0);
            self.chunk_r[..CHUNK.min(frames)].fill(0.0);
            for i in 0..frames {
                let base = i * channels;
                for c in 0..channels {
                    output[base + c] = 0.0;
                }
            }
        } else {
            let mut done = 0;
            while done < frames {
                let n = (frames - done).min(CHUNK);
                self.render_chunk(n);
                for i in 0..n {
                    let base = (done + i) * channels;
                    let l = self.chunk_l[i];
                    let r = self.chunk_r[i];
                    if channels == 1 {
                        output[base] = (l + r) * 0.5;
                    } else {
                        output[base] = l;
                        output[base + 1] = r;
                        for c in 2..channels {
                            output[base + c] = 0.0;
                        }
                    }
                }
                done += n;
                self.advance(n);
            }
        }

        self.shared
            .playhead
            .store(self.transport.position, Ordering::Relaxed);
        self.shared
            .playing
            .store(self.transport.playing, Ordering::Relaxed);
        let elapsed = started.elapsed().as_secs_f32();
        let budget = frames as f32 / self.sample_rate.max(1) as f32;
        let load = if budget > 0.0 {
            (elapsed / budget * 100.0).clamp(0.0, 999.0)
        } else {
            0.0
        };
        store_f32(&self.shared.cpu_load, load);
    }

    pub fn is_shutdown(&self) -> bool {
        self.shutdown
    }
}

impl TrackRuntime {
    fn release_pitch(&mut self, pitch: u8) {
        for slot in &mut self.voices {
            if slot.pitch == pitch && !slot.released {
                slot.voice.note_off();
                slot.remaining = 0;
                slot.released = true;
            }
        }
        self.plugin_notes.retain(|n| n.pitch != pitch);
    }
}

fn trigger_events(
    runtime: &mut TrackRuntime,
    track: &TrackSnapshot,
    plugins: &mut PluginHost,
    sample_rate: f32,
    events: &[NoteEvent],
    position: u64,
    end: u64,
) {
    for event in events {
        if event.start >= end {
            break;
        }
        let offset = (event.start.saturating_sub(position)) as usize;
        trigger_single(
            runtime,
            track,
            plugins,
            sample_rate,
            event.pitch,
            event.velocity,
            offset,
            event.duration,
        );
    }
}

fn trigger_single(
    runtime: &mut TrackRuntime,
    track: &TrackSnapshot,
    plugins: &mut PluginHost,
    sample_rate: f32,
    pitch: u8,
    velocity: u8,
    offset: usize,
    duration: u64,
) {
    if matches!(track.instrument, InstrumentSnapshot::Plugin(_)) {
        plugins.note_on(instrument_key(track.id), pitch, velocity, offset);
        runtime.plugin_notes.push(PluginNote {
            pitch,
            remaining: offset as u64 + duration,
        });
        return;
    }
    let kind = match track.instrument.as_kind() {
        Some(kind) => kind,
        None => return,
    };
    let sampler = match &track.instrument {
        InstrumentSnapshot::Sampler(s) => s.buffer.as_ref().map(|buffer| SamplerSource {
            buffer: buffer.clone(),
            sample_rate: s.buffer_sample_rate,
        }),
        _ => None,
    };
    let glide = runtime.last_pitch.map(|p| midi_to_hz(p as f32));
    if let Some(voice) = make_voice(&kind, pitch, velocity, sample_rate, sampler.as_ref(), glide) {
        if runtime.voices.len() >= MAX_VOICES {
            let steal = runtime.voices.iter().position(|v| v.released).unwrap_or(0);
            runtime.voices.remove(steal);
        }
        runtime.voices.push(VoiceSlot {
            voice,
            remaining: duration,
            released: false,
            start_offset: offset,
            pitch,
        });
        runtime.last_pitch = Some(pitch);
    }
}

fn same_instrument(a: &InstrumentSnapshot, b: &InstrumentSnapshot) -> bool {
    match (a, b) {
        (InstrumentSnapshot::Sampler(x), InstrumentSnapshot::Sampler(y)) => {
            x.params.media == y.params.media
        }
        (InstrumentSnapshot::Plugin(x), InstrumentSnapshot::Plugin(y)) => {
            x.path == y.path && x.class_id == y.class_id
        }
        (InstrumentSnapshot::Drum(_), InstrumentSnapshot::Drum(_)) => true,
        (InstrumentSnapshot::Synth(_), InstrumentSnapshot::Synth(_)) => true,
        _ => false,
    }
}

fn render_voices(runtime: &mut TrackRuntime, frames: usize) {
    for slot in &mut runtime.voices {
        if !slot.voice.is_active() {
            continue;
        }
        let mut index = slot.start_offset.min(frames);
        slot.start_offset = 0;
        if slot.remaining == 0 && !slot.released {
            slot.voice.note_off();
            slot.released = true;
        }
        while index < frames {
            let limit = if slot.released {
                frames
            } else {
                (index as u64 + slot.remaining).min(frames as u64) as usize
            };
            let limit = limit.max(index);
            for i in index..limit {
                let (l, r) = slot.voice.process();
                runtime.scratch_l[i] += l;
                runtime.scratch_r[i] += r;
            }
            let consumed = (limit - index) as u64;
            slot.remaining = slot.remaining.saturating_sub(consumed);
            index = limit;
            if !slot.released && slot.remaining == 0 {
                slot.voice.note_off();
                slot.released = true;
            }
            if index >= frames {
                break;
            }
        }
    }
    runtime.voices.retain(|slot| slot.voice.is_active());
}

fn mix_audio_events(
    events: &[AudioEvent],
    position: u64,
    frames: usize,
    out_l: &mut [f32],
    out_r: &mut [f32],
) {
    let end = position + frames as u64;
    for event in events {
        let event_end = event.start + event.length;
        if event_end <= position || event.start >= end {
            continue;
        }
        let start = event.start.max(position);
        let stop = event_end.min(end);
        for absolute in start..stop {
            let local = absolute - event.start;
            let mut fade = 1.0f32;
            if event.fade_in > 0 && local < event.fade_in {
                fade *= local as f32 / event.fade_in as f32;
            }
            if event.fade_out > 0 && absolute + event.fade_out > event_end {
                fade *= (event_end - absolute) as f32 / event.fade_out as f32;
            }
            let source_frame = if event.reverse {
                event.offset + event.length - 1 - local
            } else {
                event.offset + local
            } as usize;
            let index = source_frame * 2;
            if index + 1 >= event.buffer.len() {
                continue;
            }
            let gain = event.gain * fade;
            let offset = (absolute - position) as usize;
            out_l[offset] += event.buffer[index] * gain;
            out_r[offset] += event.buffer[index + 1] * gain;
        }
    }
}

fn meter_from_runtime(runtime: &TrackRuntime, active: u32) -> TrackMeter {
    TrackMeter {
        rms_db: runtime.meter_rms,
        peak_db: runtime.meter_peak,
        low: runtime.smooth_low,
        mid: runtime.smooth_mid,
        high: runtime.smooth_high,
        active_notes: active,
    }
}

fn add_click(l: &mut [f32], r: &mut [f32], offset: usize, accent: bool, sample_rate: f32) {
    let frequency = if accent { 1_600.0 } else { 1_050.0 };
    let length = (sample_rate * 0.028) as usize;
    let decay = sample_rate * 0.007;
    for i in 0..length {
        let index = offset + i;
        if index >= l.len() {
            break;
        }
        let envelope = (-(i as f32) / decay).exp();
        let sample =
            (std::f32::consts::TAU * frequency * i as f32 / sample_rate).sin() * envelope * 0.22;
        l[index] += sample;
        r[index] += sample;
    }
}

// ---------------------------------------------------------------------------
// Offline render
// ---------------------------------------------------------------------------

/// Renders the whole arrangement once (ignoring the loop region) for export.
pub fn render_offline(
    project: &Project,
    media: &MediaStore,
    sample_rate: u32,
    tail_seconds: f32,
) -> Vec<f32> {
    let mut cache = PlaybackCache::new(sample_rate);
    let built = Snapshot::build(project, media, &mut cache, 0);
    let mut inner = (*built).clone();
    inner.loop_region = None;
    inner.metronome = false;
    inner.length_samples += (tail_seconds.max(0.0) * sample_rate as f32) as u64;
    let snapshot = Arc::new(inner);
    let shared = SharedState::new();
    let (_tx, rx) = crossbeam_channel::unbounded();
    let total_frames = snapshot.length_samples.max(1) as usize;
    let mut engine = Engine::new(sample_rate, snapshot, shared, rx);
    engine.transport.playing = true;
    let mut out = vec![0.0f32; total_frames * 2];
    let mut offset = 0;
    while offset < total_frames {
        let frames = (total_frames - offset).min(1_024);
        engine.process_interleaved(&mut out[offset * 2..(offset + frames) * 2], 2);
        offset += frames;
    }
    out
}

/// Meter values for the spatial analyser.
pub fn measurements(
    shared: &SharedState,
    tracks: &[TrackId],
) -> HashMap<TrackId, daw_core::gibson::TrackMeasurement> {
    let mut out = HashMap::new();
    for (index, id) in tracks.iter().enumerate() {
        let meter = shared.meters.read(index);
        out.insert(
            *id,
            daw_core::gibson::TrackMeasurement::new(meter.rms_db, meter.low, meter.mid, meter.high),
        );
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn offline(project: &Project) -> Vec<f32> {
        render_offline(project, &MediaStore::new(), 48_000, 0.5)
    }

    #[test]
    fn demo_song_renders_audio() {
        let project = Project::demo("Render");
        let out = offline(&project);
        assert!(out.len() > 96_000);
        let energy: f32 = out.iter().map(|s| s.abs()).sum();
        assert!(energy > 10.0, "render is silent");
        assert!(out.iter().all(|s| s.is_finite()));
        assert!(out.iter().all(|s| s.abs() <= 1.01));
    }

    #[test]
    fn muting_everything_renders_silence() {
        let mut project = Project::demo("Silent");
        for track in &mut project.tracks {
            track.mute = true;
        }
        let out = offline(&project);
        assert!(out.iter().all(|s| s.abs() < 1e-6));
    }

    #[test]
    fn transport_advances_and_loops() {
        let project = Project::demo("Loop");
        let mut cache = PlaybackCache::new(48_000);
        let snapshot = Snapshot::build(&project, &MediaStore::new(), &mut cache, 1);
        let shared = SharedState::new();
        let (tx, rx) = crossbeam_channel::unbounded();
        let mut engine = Engine::new(48_000, snapshot.clone(), shared.clone(), rx);
        let handle = EngineHandle::new(tx, shared.clone(), 48_000);
        handle.send(EngineCommand::Play);
        let mut out = vec![0.0; 4_800 * 2];
        engine.process_interleaved(&mut out, 2);
        assert!(shared.playing.load(Ordering::Relaxed));
        assert!(shared.playhead.load(Ordering::Relaxed) > 0);
        assert!(out.iter().any(|s| s.abs() > 1e-5));
        // Loop start is zero, so a long run must wrap and stay inside the loop.
        for _ in 0..50 {
            engine.process_interleaved(&mut out, 2);
        }
        let position = shared.playhead.load(Ordering::Relaxed);
        let (start, end) = snapshot.loop_region.unwrap();
        assert!(position >= start && position < end);
    }
}

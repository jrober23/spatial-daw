use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::atomic::Ordering;
use std::time::Duration;

use daw_core::gibson::{
    self, analyze, auto_place, ElementRole, MixAnalysis, MixStyle, TrackMeasurement,
};
use daw_core::model::{
    db_to_gain, ClipKind, DrumParams, DrumSound, FxSlot, InstrumentKind, PluginFormat, PluginSpec,
    Project, SamplerParams, SynthParams,
};
use daw_core::time::{
    format_position, samples_to_ticks, snap as snap_tick, ticks_to_samples, Grid,
};
use daw_core::{
    AudioClip, Clip, ClipId, MediaStore, NoteId, PatternClip, PatternId, PluginInstanceId, Track,
    TrackId, TrackKind,
};
use daw_engine::devices::{hosts as audio_hosts, output_devices, start_output};
use daw_engine::midi::{input_ports, MidiHub, MidiPort};
use daw_engine::plugin::{insert_key, instrument_key, scan_plugins};
use daw_engine::{
    export_mixdown, measurements, AudioSession, DeviceInfo, DeviceSelection, EngineCommand,
    HostInfo, PlaybackCache, PluginDescriptor, SharedState, Snapshot,
};
use egui::{
    pos2, vec2, Align2, Color32, CornerRadius, FontId, Pos2, Rect, Sense, Stroke, StrokeKind, Ui,
};
use egui_wgpu::wgpu;
use glam::Vec3;

use crate::render3d::{world_position, Camera, Mixer3D, Scene, Sphere};

const HEADER_W: f32 = 232.0;
const RULER_H: f32 = 28.0;
const ROW_GAP: f32 = 2.0;

#[derive(Clone, Copy, PartialEq, Eq)]
enum EditorTab {
    Steps,
    Piano,
    Mixer,
    Space,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum DragMode {
    Move,
    TrimLeft,
    TrimRight,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum NoteDragMode {
    Move,
    Resize,
}

#[derive(Clone, Copy)]
struct ClipDrag {
    track: TrackId,
    clip: ClipId,
    mode: DragMode,
    pointer: Pos2,
    start: i64,
    offset: i64,
    length_frames: i64,
    length_ticks: i64,
}

#[derive(Clone, Copy)]
struct NoteDrag {
    track: TrackId,
    pattern: PatternId,
    note: NoteId,
    mode: NoteDragMode,
    pointer: Pos2,
    start: i64,
    pitch: u8,
}

struct SpatialMarker {
    track: TrackId,
    sphere: Sphere,
    target: Option<Sphere>,
    role: ElementRole,
    score: f32,
    measured_hz: f32,
}

fn dashed_line(painter: &egui::Painter, a: Pos2, b: Pos2, stroke: Stroke, dash: f32, gap: f32) {
    let delta = b - a;
    let length = delta.length();
    if length < 0.5 {
        return;
    }
    let direction = delta / length;
    let mut t = 0.0;
    while t < length {
        let end = (t + dash).min(length);
        painter.line_segment([a + direction * t, a + direction * end], stroke);
        t += dash + gap;
    }
}

fn draw_tag(painter: &egui::Painter, pos: Pos2, text: &str, color: Color32) {
    let size = vec2(text.len() as f32 * 5.6 + 6.0, 14.0);
    let rect = Rect::from_center_size(pos, size);
    painter.rect_filled(rect, CornerRadius::same(3), Color32::from_black_alpha(155));
    painter.text(
        pos,
        Align2::CENTER_CENTER,
        text,
        FontId::monospace(9.0),
        color,
    );
}

#[derive(Default)]
struct AudioSettings {
    hosts: Vec<HostInfo>,
    devices: Vec<DeviceInfo>,
    host_idx: usize,
    device_idx: usize,
    rate: u32,
    buffer: u32,
    error: Option<String>,
}

pub struct App {
    project: Project,
    media: MediaStore,
    cache: PlaybackCache,
    session: Option<AudioSession>,
    path: Option<PathBuf>,
    undo: Vec<Project>,
    redo: Vec<Project>,
    revision: u64,
    dirty: bool,
    need_snapshot: bool,
    auto_started: bool,
    selected_track: Option<TrackId>,
    selected_clip: Option<(TrackId, ClipId)>,
    selected_note: Option<(TrackId, PatternId, NoteId)>,
    tab: EditorTab,
    px_per_bar: f32,
    scroll_x: f32,
    scroll_y: f32,
    drag: Option<ClipDrag>,
    note_drag: Option<NoteDrag>,
    status: String,
    audio: AudioSettings,
    midi_ports: Vec<MidiPort>,
    midi_idx: usize,
    midi_connected: bool,
    midi_hub: Option<MidiHub>,
    plugins: Vec<PluginDescriptor>,
    plugin_idx: usize,
    plugin_dirs: Vec<PathBuf>,
    show_devices: bool,
    show_plugins: bool,
    renderer3d: Option<Mixer3D>,
    camera: Camera,
    show_targets: bool,
    spatial_drag: Option<TrackId>,
    scrollbar_drag: Option<f32>,
    analysis: MixAnalysis,
    auto_follow: bool,
    last_device: String,
}

impl App {
    pub fn new(cc: &eframe::CreationContext<'_>) -> Self {
        let mut visuals = egui::Visuals::dark();
        visuals.panel_fill = Color32::from_rgb(19, 22, 29);
        visuals.window_fill = Color32::from_rgb(24, 28, 37);
        visuals.extreme_bg_color = Color32::from_rgb(12, 15, 21);
        visuals.selection.bg_fill = Color32::from_rgb(45, 90, 130);
        visuals.widgets.active.bg_fill = Color32::from_rgb(43, 71, 98);
        cc.egui_ctx.set_visuals(visuals);
        cc.egui_ctx.all_styles_mut(|style| {
            style.spacing.item_spacing = vec2(6.0, 5.0);
            style.spacing.button_padding = vec2(8.0, 4.0);
        });
        let project = Project::demo("Spatial Demo");
        let cache = PlaybackCache::new(project.sample_rate);
        let selected = project.tracks.first().map(|t| t.id);
        Self {
            project,
            media: MediaStore::new(),
            cache,
            session: None,
            path: None,
            undo: Vec::new(),
            redo: Vec::new(),
            revision: 1,
            dirty: false,
            need_snapshot: false,
            auto_started: false,
            selected_track: selected,
            selected_clip: None,
            selected_note: None,
            tab: EditorTab::Steps,
            px_per_bar: 110.0,
            scroll_x: 0.0,
            scroll_y: 0.0,
            drag: None,
            note_drag: None,
            status: "Ready. Space starts playback.".to_string(),
            audio: AudioSettings {
                rate: 48_000,
                buffer: 256,
                ..Default::default()
            },
            midi_ports: Vec::new(),
            midi_idx: 0,
            midi_connected: false,
            midi_hub: None,
            plugins: Vec::new(),
            plugin_idx: 0,
            plugin_dirs: Vec::new(),
            show_devices: false,
            show_plugins: false,
            renderer3d: None,
            camera: Camera::default(),
            show_targets: true,
            spatial_drag: None,
            scrollbar_drag: None,
            analysis: MixAnalysis::default(),
            auto_follow: true,
            last_device: String::new(),
        }
    }

    fn touch(&mut self) {
        self.revision = self.revision.wrapping_add(1);
        self.dirty = true;
        self.need_snapshot = true;
    }

    fn push_undo(&mut self) {
        self.undo.push(self.project.clone());
        if self.undo.len() > 80 {
            self.undo.remove(0);
        }
        self.redo.clear();
    }

    fn restore(&mut self, project: Project) {
        self.project = project;
        self.revision = self.revision.wrapping_add(1);
        self.dirty = true;
        self.need_snapshot = true;
        let still_exists = self
            .selected_track
            .and_then(|id| self.project.track(id))
            .is_some();
        if !still_exists {
            self.selected_track = self.project.tracks.first().map(|t| t.id);
            self.selected_clip = None;
            self.selected_note = None;
        }
        if let Some((track, clip)) = self.selected_clip {
            if self
                .project
                .track(track)
                .and_then(|t| t.clip(clip))
                .is_none()
            {
                self.selected_clip = None;
            }
        }
    }

    fn undo(&mut self) {
        if let Some(project) = self.undo.pop() {
            self.redo.push(self.project.clone());
            self.restore(project);
            self.status = "Undo".to_string();
        }
    }

    fn redo(&mut self) {
        if let Some(project) = self.redo.pop() {
            self.undo.push(self.project.clone());
            self.restore(project);
            self.status = "Redo".to_string();
        }
    }

    fn rebuild_snapshot(&mut self) {
        let Some(session) = &self.session else {
            return;
        };
        let sr = session.sample_rate;
        self.cache.set_sample_rate(sr);
        self.cache.prune(&self.media);
        let snapshot = Snapshot::build(&self.project, &self.media, &mut self.cache, self.revision);
        session.handle.send(EngineCommand::SetSnapshot(snapshot));
    }

    fn refresh_hosts(&mut self) {
        self.audio.hosts = audio_hosts();
        if self.audio.host_idx >= self.audio.hosts.len() {
            self.audio.host_idx = self
                .audio
                .hosts
                .iter()
                .position(|h| h.available)
                .unwrap_or(0);
        }
        self.refresh_devices();
    }

    fn refresh_devices(&mut self) {
        let host_id = self
            .audio
            .hosts
            .get(self.audio.host_idx)
            .map(|h| h.id.clone())
            .unwrap_or_default();
        self.audio.devices = output_devices(&host_id);
        let default = self
            .audio
            .devices
            .iter()
            .position(|d| d.is_default)
            .unwrap_or(0);
        if self.audio.device_idx >= self.audio.devices.len() {
            self.audio.device_idx = default;
        }
        if let Some(device) = self.audio.devices.get(self.audio.device_idx) {
            if !device.sample_rates.is_empty() && !device.sample_rates.contains(&self.audio.rate) {
                self.audio.rate = device.sample_rates[0];
            }
        }
    }

    fn start_audio(&mut self) {
        let host_id = self
            .audio
            .hosts
            .get(self.audio.host_idx)
            .map(|h| h.id.clone())
            .unwrap_or_default();
        let device_name = self
            .audio
            .devices
            .get(self.audio.device_idx)
            .map(|d| d.name.clone())
            .unwrap_or_default();
        let selection = DeviceSelection {
            host: host_id,
            device: device_name,
            sample_rate: Some(self.audio.rate.max(8_000)),
            buffer_frames: Some(self.audio.buffer.max(32)),
        };
        let target = self
            .project
            .tracks
            .iter()
            .find(|t| t.instrument.is_some())
            .map(|t| t.id)
            .unwrap_or(TrackId::NONE);
        self.session = None;
        self.midi_hub = None;
        self.midi_connected = false;
        match start_output(&selection, target) {
            Ok(session) => {
                self.cache.set_sample_rate(session.sample_rate);
                self.cache.clear();
                self.audio.rate = session.sample_rate;
                self.audio.buffer = session.buffer_frames;
                self.last_device = format!(
                    "{} · {} · {} Hz",
                    session.host_name, session.device_name, session.sample_rate
                );
                self.audio.error = None;
                self.status = format!("Audio running: {}", self.last_device);
                self.session = Some(session);
                self.rebuild_snapshot();
            }
            Err(err) => {
                self.audio.error = Some(err.clone());
                self.status = format!("Audio unavailable: {err}");
            }
        }
    }

    fn playhead_ticks(&self) -> f64 {
        let Some(session) = &self.session else {
            return 0.0;
        };
        let samples = session.shared.playhead.load(Ordering::Relaxed) as f64;
        samples_to_ticks(
            samples,
            session.shared.sample_rate.load(Ordering::Relaxed) as f64,
            self.project.bpm,
        )
    }

    fn toggle_play(&mut self) {
        let Some(session) = &self.session else {
            self.status = "Start an audio device first (Audio/MIDI window).".to_string();
            return;
        };
        let playing = session.shared.playing.load(Ordering::Relaxed);
        let _ = session.handle.send(if playing {
            EngineCommand::Pause
        } else {
            EngineCommand::Play
        });
    }

    fn seek(&mut self, tick: i64) {
        if let Some(session) = &self.session {
            let _ = session.handle.send(EngineCommand::SeekTicks(tick.max(0)));
        }
    }

    fn add_pattern_clip_for(&mut self, track: TrackId) -> bool {
        let Some((pattern, length)) = self
            .project
            .track(track)
            .and_then(|t| t.active_pattern().map(|p| (p.id, p.length_ticks)))
        else {
            return false;
        };
        let start = snap_tick(
            self.playhead_ticks() as i64,
            self.project.snap,
            self.project.time_signature,
        );
        self.project
            .add_pattern_clip(track, pattern, start, length)
            .is_some()
    }

    fn add_instrument(&mut self, kind: InstrumentKind, name: &str, role: ElementRole) {
        self.push_undo();
        let id = self.project.add_instrument_track(name, kind, role);
        self.add_pattern_clip_for(id);
        self.selected_track = Some(id);
        self.selected_clip = None;
        self.touch();
        self.status = format!("Added {name}");
    }

    fn import_audio(&mut self, as_sampler: bool) {
        let Some(files) = rfd::FileDialog::new()
            .add_filter(
                "Audio",
                &["wav", "mp3", "ogg", "flac", "aiff", "aif", "m4a", "aac"],
            )
            .pick_files()
        else {
            return;
        };
        self.push_undo();
        let mut imported = 0usize;
        for path in files {
            let label = path
                .file_stem()
                .map(|s| s.to_string_lossy().to_string())
                .unwrap_or_else(|| "audio".to_string());
            match daw_engine::decode_file(&path) {
                Ok(decoded) => {
                    let key = self.media.insert_decoded(
                        &path.to_string_lossy(),
                        decoded.sample_rate,
                        decoded.channels,
                        &decoded.samples,
                        Some(path.to_string_lossy().to_string()),
                    );
                    if as_sampler {
                        if let Some(track) = self.selected_track {
                            if let Some(t) = self.project.track_mut(track) {
                                if let Some(instrument) = &mut t.instrument {
                                    if let InstrumentKind::Sampler(s) = &mut instrument.kind {
                                        s.media = key.clone();
                                    }
                                }
                            }
                        }
                    } else {
                        let track = match self.selected_track.filter(|id| {
                            self.project
                                .track(*id)
                                .map(|t| t.is_audio())
                                .unwrap_or(false)
                        }) {
                            Some(id) => id,
                            None => {
                                let id = self.project.add_audio_track(&label);
                                self.selected_track = Some(id);
                                id
                            }
                        };
                        let start = snap_tick(
                            self.playhead_ticks() as i64 + imported as i64 * 0,
                            self.project.snap,
                            self.project.time_signature,
                        );
                        self.project.add_audio_clip(
                            track,
                            key,
                            label,
                            start,
                            decoded.frames as i64,
                            0,
                        );
                        self.selected_track = Some(track);
                    }
                    imported += 1;
                }
                Err(err) => {
                    self.status = format!("Could not import {}: {err}", path.display());
                }
            }
        }
        if imported > 0 {
            self.touch();
            self.status = format!("Imported {imported} file(s)");
        }
    }

    fn save_project(&mut self, save_as: bool) {
        let path = if save_as || self.path.is_none() {
            rfd::FileDialog::new()
                .add_filter("Spatial DAW project", &["sdaw"])
                .set_file_name(format!("{}.sdaw", self.project.name))
                .save_file()
        } else {
            self.path.clone()
        };
        let Some(path) = path else {
            return;
        };
        match daw_core::save_project(&self.project, &self.media, &path) {
            Ok(report) => {
                self.path = Some(path.clone());
                self.dirty = false;
                self.status = format!(
                    "Saved {} · {} media file(s){}",
                    path.display(),
                    report.media_written,
                    if report.media_skipped.is_empty() {
                        String::new()
                    } else {
                        format!(" · {} missing", report.media_skipped.len())
                    }
                );
            }
            Err(err) => self.status = format!("Save failed: {err}"),
        }
    }

    fn open_project(&mut self) {
        let Some(path) = rfd::FileDialog::new()
            .add_filter("Spatial DAW project", &["sdaw"])
            .pick_file()
        else {
            return;
        };
        match daw_core::load_project(&path) {
            Ok((project, media, warnings)) => {
                self.project = project;
                self.media = media;
                self.cache.clear();
                self.revision = self.revision.wrapping_add(1);
                self.need_snapshot = true;
                self.undo.clear();
                self.redo.clear();
                self.path = Some(path.clone());
                self.dirty = false;
                self.selected_track = self.project.tracks.first().map(|t| t.id);
                self.selected_clip = None;
                self.selected_note = None;
                self.status = if warnings.is_empty() {
                    format!("Opened {}", path.display())
                } else {
                    format!(
                        "Opened with {} warning(s): {}",
                        warnings.len(),
                        warnings.join(" ")
                    )
                };
            }
            Err(err) => self.status = format!("Open failed: {err}"),
        }
    }

    fn export_audio(&mut self) {
        let path = rfd::FileDialog::new()
            .add_filter("WAV audio", &["wav"])
            .set_file_name(format!("{} mix.wav", self.project.name))
            .save_file();
        let Some(path) = path else {
            return;
        };
        self.status = "Rendering mixdown…".to_string();
        match export_mixdown(&self.project, &self.media, &path, 1.0) {
            Ok(report) => {
                self.status = format!(
                    "Exported {} ({:.1}s @ {} Hz)",
                    path.display(),
                    report.frames as f64 / report.sample_rate as f64,
                    report.sample_rate
                )
            }
            Err(err) => self.status = format!("Export failed: {err}"),
        }
    }

    fn new_project(&mut self) {
        self.push_undo();
        self.project = Project::empty("Untitled");
        self.media = MediaStore::new();
        self.cache.clear();
        self.revision = self.revision.wrapping_add(1);
        self.need_snapshot = true;
        self.path = None;
        self.dirty = false;
        self.selected_track = self.project.tracks.first().map(|t| t.id);
        self.selected_clip = None;
        self.selected_note = None;
        self.status = "New project".to_string();
    }

    fn selected_track(&self) -> Option<&Track> {
        self.selected_track.and_then(|id| self.project.track(id))
    }

    fn selected_pattern(&self) -> Option<(TrackId, PatternId)> {
        let track = self.selected_track()?;
        Some((track.id, track.active_pattern()?.id))
    }

    fn delete_selection(&mut self) {
        if let Some((track, pattern, note)) = self.selected_note {
            if self.project.remove_note(track, pattern, note) {
                self.push_undo();
                self.selected_note = None;
                self.touch();
            }
            return;
        }
        if let Some((track, clip)) = self.selected_clip {
            self.push_undo();
            self.project.remove_clip(track, clip);
            self.selected_clip = None;
            self.touch();
        }
    }

    fn split_selected(&mut self) {
        let Some((track, clip)) = self.selected_clip else {
            self.status = "Select a clip to split.".to_string();
            return;
        };
        let at = self.playhead_ticks() as i64;
        self.push_undo();
        if self.project.split_clip(track, clip, at).is_some() {
            self.touch();
            self.status = "Clip split at playhead".to_string();
        } else {
            self.undo.pop();
            self.status = "Playhead is outside the selected clip".to_string();
        }
    }

    fn tick_from_x(&self, x: f32, timeline_left: f32) -> i64 {
        let bar_ticks = self.project.time_signature.ticks_per_bar() as f32;
        let ptt = self.px_per_bar / bar_ticks;
        let tick = ((x - timeline_left + self.scroll_x) / ptt).round() as i64;
        snap_tick(tick.max(0), self.project.snap, self.project.time_signature)
    }

    fn clip_at(&self, pos: Pos2, rect: Rect) -> Option<(TrackId, ClipId, DragMode)> {
        let timeline_left = rect.left() + HEADER_W;
        if pos.x < timeline_left {
            return None;
        }
        let mut y = rect.top() + RULER_H - self.scroll_y;
        let bar_ticks = self.project.time_signature.ticks_per_bar() as f32;
        let ptt = self.px_per_bar / bar_ticks;
        for track in &self.project.tracks {
            let row = Rect::from_min_size(
                pos2(timeline_left, y),
                vec2(rect.width() - HEADER_W, track.height),
            );
            if pos.y >= row.top() && pos.y <= row.bottom() {
                for clip in track.clips.iter().rev() {
                    let x = timeline_left + clip.start as f32 * ptt - self.scroll_x;
                    let w = clip.length_ticks(self.project.bpm, self.project.sample_rate as f64)
                        as f32
                        * ptt;
                    let cr = Rect::from_min_size(
                        row.min + vec2(0.0, 4.0),
                        vec2(w.max(3.0), track.height - 8.0),
                    );
                    if cr.contains(pos) {
                        let mode = if pos.x - cr.left() < 7.0 {
                            DragMode::TrimLeft
                        } else if cr.right() - pos.x < 7.0 {
                            DragMode::TrimRight
                        } else {
                            DragMode::Move
                        };
                        return Some((track.id, clip.id, mode));
                    }
                }
            }
            y += track.height + ROW_GAP;
        }
        None
    }

    fn apply_clip_drag(&mut self, pointer: Pos2, ptt: f32) {
        let Some(drag) = self.drag else {
            return;
        };
        let grid = self.project.snap;
        let sig = self.project.time_signature;
        let bpm = self.project.bpm;
        let media_sr = self
            .project
            .track(drag.track)
            .and_then(|t| t.clip(drag.clip))
            .and_then(|c| match &c.kind {
                ClipKind::Audio(a) => self.media.get(&a.media),
                _ => None,
            })
            .map(|m| m.sample_rate as f64)
            .unwrap_or(self.project.sample_rate as f64);
        let delta_ticks = ((pointer.x - drag.pointer.x) / ptt.max(0.0001)) as i64;
        let target_start = snap_tick(drag.start + delta_ticks, grid, sig);
        let min_len = grid.ticks(sig).max(1);
        if drag.mode == DragMode::Move {
            if let Some(clip) = self
                .project
                .track_mut(drag.track)
                .and_then(|t| t.clip_mut(drag.clip))
            {
                clip.start = target_start.max(0);
            }
        } else if drag.mode == DragMode::TrimLeft {
            let new_start = target_start.clamp(0, drag.start + drag.length_ticks - min_len);
            let delta = new_start - drag.start;
            let new_offset = drag.offset + ticks_to_samples(delta as f64, media_sr, bpm) as i64;
            if let Some(clip) = self
                .project
                .track_mut(drag.track)
                .and_then(|t| t.clip_mut(drag.clip))
            {
                clip.start = new_start;
                match &mut clip.kind {
                    ClipKind::Pattern(p) => p.length_ticks = (drag.length_ticks - delta).max(1),
                    ClipKind::Audio(a) => {
                        a.offset_frames = new_offset.max(0);
                        a.length_frames = (drag.length_frames - (new_offset - drag.offset)).max(1);
                    }
                }
            }
        } else {
            let end = snap_tick(drag.start + drag.length_ticks + delta_ticks, grid, sig);
            let new_len = (end - drag.start).max(min_len);
            if let Some(clip) = self
                .project
                .track_mut(drag.track)
                .and_then(|t| t.clip_mut(drag.clip))
            {
                match &mut clip.kind {
                    ClipKind::Pattern(p) => p.length_ticks = new_len,
                    ClipKind::Audio(a) => {
                        let frames = ticks_to_samples(new_len as f64, media_sr, bpm) as i64;
                        let max_frames = self
                            .media
                            .get(&a.media)
                            .map(|m| (m.frames as i64 - a.offset_frames).max(1))
                            .unwrap_or(frames);
                        a.length_frames = frames.clamp(1, max_frames);
                    }
                }
            }
        }
        self.touch();
    }

    fn top_bar(&mut self, ui_root: &mut Ui) {
        egui::Panel::top("toolbar").show(ui_root, |ui| {
            ui.add_space(3.0);
            ui.horizontal(|ui| {
                ui.label(
                    egui::RichText::new("SPATIAL DAW")
                        .strong()
                        .size(17.0)
                        .color(Color32::from_rgb(118, 210, 255)),
                );
                ui.separator();
                if ui.button("New").clicked() {
                    self.new_project();
                }
                if ui.button("Open").clicked() {
                    self.open_project();
                }
                if ui.button("Save").clicked() {
                    self.save_project(false);
                }
                if ui.button("Save As").clicked() {
                    self.save_project(true);
                }
                if ui.button("Export WAV").clicked() {
                    self.export_audio();
                }
                ui.separator();
                if ui
                    .add_enabled(!self.undo.is_empty(), egui::Button::new("↶"))
                    .clicked()
                {
                    self.undo();
                }
                if ui
                    .add_enabled(!self.redo.is_empty(), egui::Button::new("↷"))
                    .clicked()
                {
                    self.redo();
                }
                ui.separator();
                ui.add(egui::TextEdit::singleline(&mut self.project.name).desired_width(150.0));
                if ui.button("Devices / MIDI").clicked() {
                    self.show_devices = true;
                }
                if ui.button("Plugins").clicked() {
                    self.show_plugins = true;
                }
                if self.dirty {
                    ui.label(egui::RichText::new("●").color(Color32::from_rgb(245, 190, 80)));
                }
            });
            ui.horizontal(|ui| {
                if ui.button("⏹").on_hover_text("Stop").clicked() {
                    if let Some(session) = &self.session {
                        let _ = session.handle.send(EngineCommand::Stop);
                    }
                }
                let playing = self
                    .session
                    .as_ref()
                    .map(|s| s.shared.playing.load(Ordering::Relaxed))
                    .unwrap_or(false);
                if ui.button(if playing { "⏸" } else { "▶" }).clicked() {
                    self.toggle_play();
                }
                if ui.button("⏮").clicked() {
                    self.seek(0);
                }
                ui.label(
                    egui::RichText::new(format_position(
                        self.playhead_ticks().round() as i64,
                        self.project.time_signature,
                    ))
                    .monospace()
                    .size(15.0)
                    .color(Color32::from_rgb(210, 230, 245)),
                );
                ui.separator();
                ui.label("BPM");
                let bpm = ui.add(
                    egui::DragValue::new(&mut self.project.bpm)
                        .speed(0.5)
                        .range(20.0..=300.0),
                );
                if bpm.changed() {
                    self.touch();
                }
                ui.label(self.project.time_signature.label());
                let mut metro = self.project.metronome;
                if ui.checkbox(&mut metro, "Click").changed() {
                    self.project.metronome = metro;
                    self.touch();
                }
                let mut looped = self.project.loop_region.is_some();
                if ui.checkbox(&mut looped, "Loop").changed() {
                    self.project.loop_region = if looped {
                        Some((0, self.project.time_signature.ticks_per_bar() * 2))
                    } else {
                        None
                    };
                    self.touch();
                }
                ui.separator();
                egui::ComboBox::from_id_salt("snap")
                    .width(78.0)
                    .selected_text(format!("Snap {}", self.project.snap.label()))
                    .show_ui(ui, |ui| {
                        for grid in Grid::ALL {
                            ui.selectable_value(&mut self.project.snap, grid, grid.label());
                        }
                    });
                if ui.small_button("−").clicked() {
                    self.px_per_bar = (self.px_per_bar / 1.2).clamp(24.0, 900.0);
                }
                if ui.small_button("+").clicked() {
                    self.px_per_bar = (self.px_per_bar * 1.2).clamp(24.0, 900.0);
                }
                let mut follow = self.auto_follow;
                if ui.checkbox(&mut follow, "Follow").changed() {
                    self.auto_follow = follow;
                }
            });
            ui.add_space(3.0);
        });
    }

    fn browser(&mut self, ui_root: &mut Ui) {
        egui::Panel::left("browser")
            .resizable(true)
            .default_size(262.0)
            .min_size(230.0)
            .show(ui_root, |ui| {
                egui::ScrollArea::vertical().show(ui, |ui| {
                    ui.add_space(4.0);
                    ui.heading("Instruments");
                    egui::Grid::new("drum-grid").num_columns(3).show(ui, |ui| {
                        for sound in DrumSound::ALL {
                            if ui.button(sound.label()).clicked() {
                                self.add_instrument(
                                    InstrumentKind::Drum(DrumParams::new(sound)),
                                    sound.label(),
                                    sound.default_role(),
                                );
                            }
                        }
                    });
                    ui.horizontal(|ui| {
                        if ui.button("Synth").clicked() {
                            self.add_instrument(
                                InstrumentKind::Synth(SynthParams::lead()),
                                "Synth",
                                ElementRole::Lead,
                            );
                        }
                        if ui.button("Sampler").clicked() {
                            self.add_instrument(
                                InstrumentKind::Sampler(SamplerParams::default()),
                                "Sampler",
                                ElementRole::Other,
                            );
                        }
                        if ui.button("Audio track").clicked() {
                            self.push_undo();
                            let id = self.project.add_audio_track("Audio");
                            self.selected_track = Some(id);
                            self.touch();
                        }
                    });
                    ui.separator();
                    if ui.button("Import audio…").clicked() {
                        self.import_audio(false);
                    }
                    ui.separator();
                    ui.heading("Track");
                    let Some(id) = self.selected_track else {
                        ui.label("Select a track in the arrangement.");
                        return;
                    };
                    let mut changed = false;
                    let pattern_names: Vec<(PatternId, String)> = self
                        .project
                        .track(id)
                        .map(|t| t.patterns.iter().map(|p| (p.id, p.name.clone())).collect())
                        .unwrap_or_default();
                    let media_names: Vec<String> = self.media.keys().cloned().collect();
                    let mut load_sampler = false;
                    let mut add_pattern = false;
                    if let Some(track) = self.project.track_mut(id) {
                        ui.horizontal(|ui| {
                            if ui
                                .add(egui::TextEdit::singleline(&mut track.name).desired_width(140.0))
                                .changed()
                            {
                                changed = true;
                            }
                            ui.label(format!("#{}", track.id.0));
                        });
                        ui.label(
                            egui::RichText::new(match &track.instrument {
                                Some(i) => i.kind.label().to_string(),
                                None => "Audio track".to_string(),
                            })
                            .color(Color32::from_rgb(150, 190, 215)),
                        );
                        ui.horizontal(|ui| {
                            let mut role = track.spatial.role;
                            egui::ComboBox::from_id_salt("role")
                                .selected_text(role.label())
                                .show_ui(ui, |ui| {
                                    for r in ElementRole::ALL {
                                        ui.selectable_value(&mut role, r, r.label());
                                    }
                                });
                            if role != track.spatial.role {
                                track.spatial.role = role;
                                changed = true;
                            }
                        });
                        if ui
                            .add(
                                egui::Slider::new(&mut track.volume_db, -60.0..=6.0)
                                    .text("Volume dB"),
                            )
                            .changed()
                        {
                            changed = true;
                        }
                        if ui
                            .add(
                                egui::Slider::new(&mut track.pan, -1.0..=1.0)
                                    .text("Pan L/R")
                                    .custom_formatter(|v, _| format!("{:+.2}", v)),
                            )
                            .changed()
                        {
                            changed = true;
                        }
                        ui.horizontal(|ui| {
                            if ui.checkbox(&mut track.mute, "Mute").changed() {
                                changed = true;
                            }
                            if ui.checkbox(&mut track.solo, "Solo").changed() {
                                changed = true;
                            }
                        });
                        if let Some(pattern) = track.active_pattern() {
                            let current = pattern.id;
                            let mut chosen = current;
                            egui::ComboBox::from_id_salt("pattern")
                                .selected_text(pattern.name.clone())
                                .show_ui(ui, |ui| {
                                    for (pid, name) in &pattern_names {
                                        ui.selectable_value(&mut chosen, *pid, name);
                                    }
                                });
                            if chosen != current {
                                track.active_pattern = Some(chosen);
                                changed = true;
                            }
                        }
                        if ui.button("+ Pattern").clicked() {
                            add_pattern = true;
                        }
                        if let Some(InstrumentKind::Sampler(s)) = track
                            .instrument
                            .as_ref()
                            .map(|i| &i.kind)
                        {
                            ui.label(if s.media.is_empty() {
                                "No sample loaded".to_string()
                            } else {
                                format!("Sample: {}", s.media)
                            });
                            if ui.button("Load sample…").clicked() {
                                load_sampler = true;
                            }
                        }
                        if !media_names.is_empty() {
                            ui.collapsing("Media pool", |ui| {
                                for name in &media_names {
                                    ui.small(name);
                                }
                            });
                        }
                    }
                    if add_pattern {
                        self.push_undo();
                        let new_id = self.project.alloc_id();
                        let len = self.project.time_signature.ticks_per_bar() * 2;
                        if let Some(track) = self.project.track_mut(id) {
                            let index = track.patterns.len() + 1;
                            let color = track.color;
                            let pid = PatternId::new(new_id);
                            track.patterns.push(daw_core::Pattern {
                                id: pid,
                                name: format!("Pattern {index}"),
                                length_ticks: len,
                                notes: Vec::new(),
                                color,
                            });
                            track.active_pattern = Some(pid);
                        }
                        self.touch();
                    }
                    if load_sampler {
                        self.import_audio(true);
                    }
                    ui.separator();
                    ui.heading("Clip");
                    if let Some((track_id, clip_id)) = self.selected_clip {
                        let media_frames = self
                            .project
                            .track(track_id)
                            .and_then(|t| t.clip(clip_id))
                            .and_then(|c| match &c.kind {
                                ClipKind::Audio(a) => self.media.get(&a.media).map(|m| m.frames),
                                _ => None,
                            });
                        let mut changed_clip = false;
                        let mut split_clip = false;
                        let mut delete_clip = false;
                        let sig = self.project.time_signature;
                        if let Some(clip) = self
                            .project
                            .track_mut(track_id)
                            .and_then(|t| t.clip_mut(clip_id))
                        {
                            ui.horizontal(|ui| {
                                ui.label("Name");
                                if ui
                                    .add(egui::TextEdit::singleline(&mut clip.name).desired_width(140.0))
                                    .changed()
                                {
                                    changed_clip = true;
                                }
                            });
                            ui.label(format!("Starts at {}", format_position(clip.start, sig)));
                            if let ClipKind::Audio(a) = &mut clip.kind {
                                if ui
                                    .add(egui::Slider::new(&mut a.gain_db, -24.0..=24.0).text("Clip gain dB"))
                                    .changed()
                                {
                                    changed_clip = true;
                                }
                                ui.horizontal(|ui| {
                                    if ui
                                        .add(egui::Slider::new(&mut a.fade_in_seconds, 0.0..=4.0).text("Fade in"))
                                        .changed()
                                    {
                                        changed_clip = true;
                                    }
                                    if ui
                                        .add(egui::Slider::new(&mut a.fade_out_seconds, 0.0..=4.0).text("Fade out"))
                                        .changed()
                                    {
                                        changed_clip = true;
                                    }
                                });
                                if ui.checkbox(&mut a.reverse, "Reverse").changed() {
                                    changed_clip = true;
                                }
                                ui.label(format!(
                                    "Source frames {}–{}{}",
                                    a.offset_frames,
                                    a.offset_frames + a.length_frames,
                                    media_frames
                                        .map(|f| format!(" / {f}"))
                                        .unwrap_or_default()
                                ));
                            }
                            ui.horizontal(|ui| {
                                if ui.button("Split at playhead").clicked() {
                                    split_clip = true;
                                }
                                if ui.button("Delete").clicked() {
                                    delete_clip = true;
                                }
                            });
                        }
                        if split_clip {
                            self.split_selected();
                        }
                        if delete_clip {
                            self.selected_clip = Some((track_id, clip_id));
                            self.delete_selection();
                        }
                        if changed_clip {
                            self.touch();
                        }
                    } else {
                        ui.label("Select a clip in the arrangement.");
                    }
                    ui.separator();
                    ui.label(
                        egui::RichText::new("Drag clips to move. Drag edges to trim. Select a clip and press S to split.")
                            .small()
                            .color(Color32::from_gray(140)),
                    );
                    if changed {
                        self.touch();
                    }
                });
            });
    }

    fn coach(&mut self, ui_root: &mut Ui) {
        egui::Panel::right("coach")
            .resizable(true)
            .default_size(276.0)
            .min_size(235.0)
            .show(ui_root, |ui| {
                egui::ScrollArea::vertical().show(ui, |ui| {
                    ui.add_space(4.0);
                    ui.heading("Spatial image");
                    let analysis = self.analysis.clone();
                    let mut style = self.project.spatial_style;
                    egui::ComboBox::from_id_salt("style")
                        .width(220.0)
                        .selected_text(style.label())
                        .show_ui(ui, |ui| {
                            for s in MixStyle::ALL {
                                ui.selectable_value(&mut style, s, s.label());
                            }
                        });
                    if style != self.project.spatial_style {
                        self.project.spatial_style = style;
                        self.dirty = true;
                    }
                    ui.label(
                        egui::RichText::new(self.project.spatial_style.description())
                            .small()
                            .color(Color32::from_gray(150)),
                    );
                    ui.add(
                        egui::ProgressBar::new((analysis.score / 100.0).clamp(0.0, 1.0))
                            .text(format!("{:.0}% match", analysis.score)),
                    );
                    ui.label(
                        egui::RichText::new(analysis.headline.clone())
                            .strong()
                            .color(Color32::from_rgb(150, 215, 240)),
                    );
                    ui.horizontal(|ui| {
                        ui.checkbox(&mut self.show_targets, "Target ghosts");
                        if ui.button("Auto-place").clicked() {
                            self.push_undo();
                            let placements = auto_place(&self.project, self.project.spatial_style);
                            for p in placements {
                                self.project.set_track_pan(p.track, p.pan);
                                self.project.set_track_db(p.track, p.volume_db);
                            }
                            self.touch();
                            self.status = "Auto-placed tracks on the reference image".to_string();
                        }
                    });
                    ui.separator();
                    ui.label(egui::RichText::new("AXES").small().strong());
                    ui.label(egui::RichText::new("X  pan: left ↔ right").small());
                    ui.label(egui::RichText::new("Y  frequency: low ↔ high").small());
                    ui.label(egui::RichText::new("Z  depth: far ↔ near (fader)").small());
                    ui.label(egui::RichText::new("Size  perceived loudness").small());
                    ui.separator();
                    let mut roles: Vec<(TrackId, ElementRole)> = Vec::new();
                    for ta in &analysis.tracks {
                        ui.horizontal(|ui| {
                            let color = self
                                .project
                                .track(ta.track)
                                .map(|t| Color32::from_rgb(t.color[0], t.color[1], t.color[2]))
                                .unwrap_or(Color32::WHITE);
                            ui.colored_label(color, "●");
                            ui.label(egui::RichText::new(&ta.name).strong());
                            ui.with_layout(
                                egui::Layout::right_to_left(egui::Align::Center),
                                |ui| {
                                    ui.label(format!("{:.0}", ta.score));
                                },
                            );
                        });
                        let mut role = self
                            .project
                            .track(ta.track)
                            .map(|t| t.spatial.role)
                            .unwrap_or(ElementRole::Other);
                        egui::ComboBox::from_id_salt(("coach-role", ta.track.0))
                            .width(130.0)
                            .selected_text(role.label())
                            .show_ui(ui, |ui| {
                                for r in ElementRole::ALL {
                                    ui.selectable_value(&mut role, r, r.label());
                                }
                            });
                        roles.push((ta.track, role));
                        for suggestion in &ta.suggestions {
                            ui.label(
                                egui::RichText::new(suggestion)
                                    .small()
                                    .color(Color32::from_rgb(228, 185, 110)),
                            );
                        }
                    }
                    for (track, role) in roles {
                        if let Some(t) = self.project.track_mut(track) {
                            if t.spatial.role != role {
                                t.spatial.role = role;
                                self.dirty = true;
                            }
                        }
                    }
                    if analysis.tracks.is_empty() {
                        ui.label("Add tracks to build a spatial image.");
                    }
                });
            });
    }

    fn arranger(&mut self, ui_root: &mut Ui) {
        egui::CentralPanel::no_frame().show(ui_root, |ui| {
            let size = ui.available_size();
            let (response, mut painter) = ui.allocate_painter(size, Sense::click_and_drag());
            let rect = response.rect;
            painter.rect_filled(rect, CornerRadius::ZERO, Color32::from_rgb(15, 18, 25));
            let timeline_left = rect.left() + HEADER_W;
            let timeline_rect =
                Rect::from_min_max(pos2(timeline_left, rect.top() + RULER_H), rect.max);
            if response.hovered() {
                let (scroll, shift, ctrl) =
                    ui.input(|i| (i.smooth_scroll_delta.y, i.modifiers.shift, i.modifiers.ctrl));
                if ctrl {
                    if scroll != 0.0 {
                        self.px_per_bar =
                            (self.px_per_bar * (1.0 + scroll * 0.002)).clamp(24.0, 900.0);
                    }
                } else if shift {
                    self.scroll_x = (self.scroll_x - scroll).max(0.0);
                } else {
                    self.scroll_y = (self.scroll_y - scroll).max(0.0);
                }
            }
            let bar_ticks = self.project.time_signature.ticks_per_bar().max(1) as f32;
            let ptt = self.px_per_bar / bar_ticks;
            let content_height: f32 = self.project.tracks.iter().map(|t| t.height + ROW_GAP).sum();
            let view_h = rect.height() - RULER_H - 18.0;
            self.scroll_y = self.scroll_y.clamp(0.0, (content_height - view_h).max(0.0));

            painter.rect_filled(
                Rect::from_min_size(rect.min, vec2(HEADER_W, rect.height())),
                CornerRadius::ZERO,
                Color32::from_rgb(22, 26, 34),
            );
            painter.line_segment(
                [
                    pos2(timeline_left, rect.top()),
                    pos2(timeline_left, rect.bottom()),
                ],
                Stroke::new(1.0, Color32::from_rgb(45, 52, 65)),
            );
            painter.rect_filled(
                Rect::from_min_max(rect.min, pos2(rect.right(), rect.top() + RULER_H)),
                CornerRadius::ZERO,
                Color32::from_rgb(26, 31, 40),
            );
            // Loop region + ruler.
            if let Some((loop_start, loop_end)) = self.project.loop_region {
                let x1 = timeline_left + loop_start as f32 * ptt - self.scroll_x;
                let x2 = timeline_left + loop_end as f32 * ptt - self.scroll_x;
                painter.rect_filled(
                    Rect::from_min_max(
                        pos2(x1.max(timeline_left), rect.top() + RULER_H),
                        pos2(x2.min(rect.right()), rect.bottom()),
                    ),
                    CornerRadius::ZERO,
                    Color32::from_rgba_unmultiplied(70, 130, 170, 16),
                );
                painter.rect_filled(
                    Rect::from_min_max(
                        pos2(x1.max(timeline_left), rect.top()),
                        pos2(x2.min(rect.right()), rect.top() + 4.0),
                    ),
                    CornerRadius::ZERO,
                    Color32::from_rgb(70, 150, 200),
                );
            }
            let first_bar = (self.scroll_x / self.px_per_bar).floor() as i64;
            let bars_visible = (rect.width() / self.px_per_bar).ceil() as i64 + 2;
            for bar in first_bar..first_bar + bars_visible {
                let x = timeline_left + bar as f32 * self.px_per_bar - self.scroll_x;
                if x < timeline_left - 1.0 || x > rect.right() {
                    continue;
                }
                painter.line_segment(
                    [pos2(x, rect.top() + RULER_H), pos2(x, rect.bottom())],
                    Stroke::new(
                        if bar % 4 == 0 { 1.2 } else { 1.0 },
                        if bar % 4 == 0 {
                            Color32::from_rgb(42, 49, 62)
                        } else {
                            Color32::from_rgb(29, 35, 45)
                        },
                    ),
                );
                if self.px_per_bar > 55.0 {
                    painter.text(
                        pos2(x + 4.0, rect.top() + 6.0),
                        Align2::LEFT_TOP,
                        format!("{}", bar + 1),
                        FontId::monospace(11.0),
                        Color32::from_gray(160),
                    );
                }
                if self.px_per_bar > 150.0 {
                    for beat in 1..self.project.time_signature.numerator {
                        let bx = x + beat as f32 * self.px_per_bar
                            / self.project.time_signature.numerator as f32;
                        painter.line_segment(
                            [pos2(bx, rect.top() + RULER_H), pos2(bx, rect.bottom())],
                            Stroke::new(1.0, Color32::from_rgb(25, 31, 40)),
                        );
                    }
                }
            }
            // Tracks.
            let mut y = rect.top() + RULER_H - self.scroll_y;
            let clip_rect = Rect::from_min_max(pos2(rect.left(), rect.top() + RULER_H), rect.max);
            for track in &self.project.tracks {
                let row =
                    Rect::from_min_size(pos2(rect.left(), y), vec2(rect.width(), track.height));
                if row.bottom() < clip_rect.top() || row.top() > rect.bottom() {
                    y += track.height + ROW_GAP;
                    continue;
                }
                let selected = self.selected_track == Some(track.id);
                painter.rect_filled(
                    Rect::from_min_size(row.min, vec2(HEADER_W, row.height())),
                    CornerRadius::ZERO,
                    if selected {
                        Color32::from_rgb(34, 43, 56)
                    } else {
                        Color32::from_rgb(26, 31, 40)
                    },
                );
                painter.rect_filled(
                    Rect::from_min_size(row.min, vec2(4.0, row.height())),
                    CornerRadius::ZERO,
                    Color32::from_rgb(track.color[0], track.color[1], track.color[2]),
                );
                painter.text(
                    pos2(row.left() + 58.0, row.top() + 8.0),
                    Align2::LEFT_TOP,
                    &track.name,
                    FontId::proportional(13.0),
                    if selected {
                        Color32::WHITE
                    } else {
                        Color32::from_gray(210)
                    },
                );
                painter.text(
                    pos2(row.left() + 58.0, row.top() + 27.0),
                    Align2::LEFT_TOP,
                    track
                        .instrument_kind()
                        .map(|k| k.label())
                        .unwrap_or("Audio"),
                    FontId::proportional(10.0),
                    Color32::from_gray(130),
                );
                let mute_rect =
                    Rect::from_min_size(pos2(row.left() + 12.0, row.top() + 8.0), vec2(18.0, 17.0));
                let solo_rect =
                    Rect::from_min_size(pos2(row.left() + 34.0, row.top() + 8.0), vec2(18.0, 17.0));
                for (r, on, label, color) in [
                    (mute_rect, track.mute, "M", Color32::from_rgb(220, 120, 80)),
                    (solo_rect, track.solo, "S", Color32::from_rgb(235, 195, 80)),
                ] {
                    painter.rect_filled(
                        r,
                        CornerRadius::same(3),
                        if on {
                            color
                        } else {
                            Color32::from_rgb(40, 47, 58)
                        },
                    );
                    painter.text(
                        r.center(),
                        Align2::CENTER_CENTER,
                        label,
                        FontId::monospace(10.0),
                        if on {
                            Color32::BLACK
                        } else {
                            Color32::from_gray(170)
                        },
                    );
                }
                painter.text(
                    pos2(row.left() + 58.0, row.bottom() - 18.0),
                    Align2::LEFT_TOP,
                    format!(
                        "{}  ·  {:+.0}%  ·  {:.0} dB  ·  {}",
                        track.spatial.role.short(),
                        track.pan * 100.0,
                        track.volume_db,
                        track
                            .active_pattern()
                            .map(|p| p.name.as_str())
                            .unwrap_or("")
                    ),
                    FontId::monospace(9.5),
                    Color32::from_gray(120),
                );
                // Clips.
                painter.set_clip_rect(Rect::from_min_max(
                    pos2(timeline_left, row.top()),
                    pos2(rect.right(), row.bottom()),
                ));
                for clip in &track.clips {
                    let x = timeline_left + clip.start as f32 * ptt - self.scroll_x;
                    let w = clip.length_ticks(self.project.bpm, self.project.sample_rate as f64)
                        as f32
                        * ptt;
                    let cr = Rect::from_min_size(
                        pos2(x, row.top() + 4.0),
                        vec2(w.max(3.0), row.height() - 8.0),
                    );
                    if cr.right() < timeline_left || cr.left() > rect.right() {
                        continue;
                    }
                    let color = clip
                        .color
                        .map(|c| Color32::from_rgb(c[0], c[1], c[2]))
                        .unwrap_or(Color32::from_rgb(
                            track.color[0],
                            track.color[1],
                            track.color[2],
                        ));
                    let fill = if clip.muted {
                        color.gamma_multiply(0.3)
                    } else {
                        color.gamma_multiply(0.55)
                    };
                    painter.rect_filled(cr, CornerRadius::same(4), fill);
                    painter.rect_stroke(
                        cr,
                        CornerRadius::same(4),
                        Stroke::new(1.0, color),
                        StrokeKind::Inside,
                    );
                    match &clip.kind {
                        ClipKind::Audio(a) => {
                            if let Some(item) = self.media.get(&a.media) {
                                let columns = (w / 2.0).clamp(1.0, 1200.0) as usize;
                                let start_peak =
                                    (a.offset_frames.max(0) as usize / 256).min(item.peaks.len());
                                let end_peak =
                                    ((a.offset_frames + a.length_frames).max(0) as usize / 256)
                                        .min(item.peaks.len());
                                for col in 0..columns {
                                    let idx = start_peak
                                        + (end_peak.saturating_sub(start_peak)) * col
                                            / columns.max(1);
                                    if let Some(peak) = item.peaks.get(idx) {
                                        let px =
                                            cr.left() + col as f32 * cr.width() / columns as f32;
                                        let mid = cr.center().y;
                                        let half = (cr.height() * 0.35).max(2.0);
                                        painter.line_segment(
                                            [
                                                pos2(px, mid - peak.max.max(-1.0) * half),
                                                pos2(px, mid - peak.min.min(1.0) * half),
                                            ],
                                            Stroke::new(
                                                1.0,
                                                Color32::from_rgba_unmultiplied(245, 250, 255, 150),
                                            ),
                                        );
                                    }
                                }
                            }
                        }
                        ClipKind::Pattern(p) => {
                            if let Some(pattern) = track.pattern(p.pattern) {
                                let length = p.length_ticks.max(1) as f32;
                                for note in &pattern.notes {
                                    let nx = cr.left() + (note.start as f32 / length) * cr.width();
                                    let nw = ((note.length as f32 / length) * cr.width()).max(2.0);
                                    let pitch_norm =
                                        ((note.pitch as f32 - 36.0) / 48.0).clamp(0.0, 1.0);
                                    let ny = cr.bottom() - 4.0 - pitch_norm * (cr.height() - 8.0);
                                    painter.rect_filled(
                                        Rect::from_min_size(
                                            pos2(nx, ny),
                                            vec2(nw.min(cr.right() - nx), 3.0),
                                        ),
                                        CornerRadius::ZERO,
                                        Color32::from_rgba_unmultiplied(250, 252, 255, 200),
                                    );
                                }
                            }
                        }
                    }
                    if self.selected_clip == Some((track.id, clip.id)) {
                        painter.rect_stroke(
                            cr,
                            CornerRadius::same(4),
                            Stroke::new(2.0, Color32::WHITE),
                            StrokeKind::Outside,
                        );
                    }
                    painter.text(
                        pos2(cr.left() + 5.0, cr.top() + 3.0),
                        Align2::LEFT_TOP,
                        &clip.name,
                        FontId::proportional(10.0),
                        Color32::from_gray(240),
                    );
                }
                painter.set_clip_rect(rect);
                y += track.height + ROW_GAP;
            }
            // Playhead.
            let play_x = timeline_left + self.playhead_ticks() as f32 * ptt - self.scroll_x;
            if play_x >= timeline_left && play_x <= rect.right() {
                painter.line_segment(
                    [pos2(play_x, rect.top()), pos2(play_x, rect.bottom())],
                    Stroke::new(1.5, Color32::from_rgb(255, 110, 110)),
                );
                let tri = vec![
                    pos2(play_x - 5.0, rect.top()),
                    pos2(play_x + 5.0, rect.top()),
                    pos2(play_x, rect.top() + 9.0),
                ];
                painter.add(egui::Shape::convex_polygon(
                    tri,
                    Color32::from_rgb(255, 110, 110),
                    Stroke::NONE,
                ));
            }
            if self.auto_follow && self.session.is_some() {
                let playing = self
                    .session
                    .as_ref()
                    .map(|s| s.shared.playing.load(Ordering::Relaxed))
                    .unwrap_or(false);
                if playing {
                    let visible = rect.width() - HEADER_W;
                    if play_x > rect.right() - 60.0 || play_x < timeline_left {
                        self.scroll_x =
                            (self.playhead_ticks() as f32 * ptt - visible * 0.3).max(0.0);
                    }
                }
            }
            // Bottom scrollbar. Keep its geometry around so the main
            // interaction response can drive it without a second widget.
            let bar = Rect::from_min_size(
                pos2(timeline_left, rect.bottom() - 8.0),
                vec2(rect.width() - HEADER_W, 6.0),
            );
            let total_bars = (self.project.timeline_end_ticks() as f32 / bar_ticks).max(8.0);
            let content_w = total_bars * self.px_per_bar;
            let scroll_max = (content_w - bar.width()).max(0.0);
            let thumb_w = if content_w > bar.width() {
                (bar.width() * (bar.width() / content_w)).max(30.0)
            } else {
                bar.width()
            };
            let thumb_x =
                bar.left() + (bar.width() - thumb_w) * (self.scroll_x / scroll_max.max(1.0));
            let thumb_rect =
                Rect::from_min_size(pos2(thumb_x, bar.top()), vec2(thumb_w, bar.height()));
            painter.rect_filled(bar, CornerRadius::same(3), Color32::from_rgb(28, 34, 43));
            if content_w > bar.width() {
                painter.rect_filled(
                    thumb_rect,
                    CornerRadius::same(3),
                    if self.scrollbar_drag.is_some() {
                        Color32::from_rgb(115, 145, 175)
                    } else {
                        Color32::from_rgb(80, 100, 125)
                    },
                );
            }

            // Interaction.
            if response.clicked() {
                if let Some(pos) = response.interact_pointer_pos() {
                    if bar.expand2(vec2(0.0, 4.0)).contains(pos) && scroll_max > 0.0 {
                        let track_w = (bar.width() - thumb_w).max(1.0);
                        let target = (pos.x - bar.left() - thumb_w * 0.5) / track_w;
                        self.scroll_x = target.clamp(0.0, 1.0) * scroll_max;
                    } else if pos.y < rect.top() + RULER_H && pos.x > timeline_left {
                        self.seek(self.tick_from_x(pos.x, timeline_left));
                    } else if pos.x < timeline_left {
                        let mut row_top = rect.top() + RULER_H - self.scroll_y;
                        let mut clicked: Option<(TrackId, bool, bool)> = None;
                        for track in &self.project.tracks {
                            let row = Rect::from_min_size(
                                pos2(rect.left(), row_top),
                                vec2(HEADER_W, track.height),
                            );
                            if row.contains(pos) {
                                let mute = Rect::from_min_size(
                                    pos2(row.left() + 12.0, row.top() + 8.0),
                                    vec2(18.0, 17.0),
                                );
                                let solo = Rect::from_min_size(
                                    pos2(row.left() + 34.0, row.top() + 8.0),
                                    vec2(18.0, 17.0),
                                );
                                clicked = Some((track.id, mute.contains(pos), solo.contains(pos)));
                                break;
                            }
                            row_top += track.height + ROW_GAP;
                        }
                        if let Some((id, on_mute, on_solo)) = clicked {
                            self.selected_track = Some(id);
                            self.selected_clip = None;
                            self.selected_note = None;
                            if on_mute || on_solo {
                                self.push_undo();
                                if let Some(t) = self.project.track_mut(id) {
                                    if on_mute {
                                        t.mute = !t.mute;
                                    } else {
                                        t.solo = !t.solo;
                                    }
                                }
                                self.touch();
                            }
                        }
                    } else if let Some((track, clip, _)) = self.clip_at(pos, rect) {
                        self.selected_track = Some(track);
                        self.selected_clip = Some((track, clip));
                        self.selected_note = None;
                    } else {
                        self.seek(self.tick_from_x(pos.x, timeline_left));
                    }
                }
            }
            if response.drag_started() {
                if let Some(pos) = response.interact_pointer_pos() {
                    if bar.expand2(vec2(0.0, 4.0)).contains(pos) && scroll_max > 0.0 {
                        let offset = if thumb_rect.contains(pos) {
                            pos.x - thumb_x
                        } else {
                            thumb_w * 0.5
                        };
                        self.scrollbar_drag = Some(offset);
                        let track_w = (bar.width() - thumb_w).max(1.0);
                        let target = (pos.x - offset - bar.left()) / track_w;
                        self.scroll_x = target.clamp(0.0, 1.0) * scroll_max;
                        self.drag = None;
                    } else if pos.y > rect.top() + RULER_H {
                        if let Some((track, clip, mode)) = self.clip_at(pos, rect) {
                            let Some((clip_start, offset, length_frames, length_ticks)) = self
                                .project
                                .track(track)
                                .and_then(|t| t.clip(clip))
                                .map(|source| {
                                    let (offset, length_frames) = match &source.kind {
                                        ClipKind::Audio(a) => (a.offset_frames, a.length_frames),
                                        _ => (0, 0),
                                    };
                                    (
                                        source.start,
                                        offset,
                                        length_frames,
                                        source.length_ticks(
                                            self.project.bpm,
                                            self.project.sample_rate as f64,
                                        ),
                                    )
                                })
                            else {
                                return;
                            };
                            self.push_undo();
                            self.drag = Some(ClipDrag {
                                track,
                                clip,
                                mode,
                                pointer: pos,
                                start: clip_start,
                                offset,
                                length_frames,
                                length_ticks,
                            });
                            self.selected_track = Some(track);
                            self.selected_clip = Some((track, clip));
                        }
                    }
                }
            }
            if response.dragged() {
                if let Some(offset) = self.scrollbar_drag {
                    if let Some(pos) = response.interact_pointer_pos() {
                        let track_w = (bar.width() - thumb_w).max(1.0);
                        let target = (pos.x - offset - bar.left()) / track_w;
                        self.scroll_x = target.clamp(0.0, 1.0) * scroll_max;
                    }
                } else if let Some(pos) = response.interact_pointer_pos() {
                    self.apply_clip_drag(pos, ptt);
                }
            }
            if response.drag_stopped() {
                self.drag = None;
                self.scrollbar_drag = None;
            }
        });
    }

    fn editor(&mut self, ui_root: &mut Ui, frame: &eframe::Frame) {
        egui::Panel::bottom("editor")
            .resizable(true)
            .default_size(330.0)
            .min_size(180.0)
            .show(ui_root, |ui| {
                ui.horizontal(|ui| {
                    ui.selectable_value(&mut self.tab, EditorTab::Steps, "Step sequencer");
                    ui.selectable_value(&mut self.tab, EditorTab::Piano, "Piano roll");
                    ui.selectable_value(&mut self.tab, EditorTab::Mixer, "Mixer");
                    ui.selectable_value(&mut self.tab, EditorTab::Space, "3D mix field");
                    ui.separator();
                    if let Some(track) = self.selected_track() {
                        ui.label(
                            egui::RichText::new(format!(
                                "{} · {}",
                                track.name,
                                track
                                    .active_pattern()
                                    .map(|p| p.name.as_str())
                                    .unwrap_or("no pattern")
                            ))
                            .small()
                            .color(Color32::from_gray(150)),
                        );
                    }
                });
                ui.separator();
                match self.tab {
                    EditorTab::Steps => self.steps(ui),
                    EditorTab::Piano => self.piano(ui),
                    EditorTab::Mixer => self.mixer(ui),
                    EditorTab::Space => self.space(ui, frame),
                }
            });
    }

    fn steps(&mut self, ui: &mut Ui) {
        let Some((selected, pattern_id)) = self.selected_pattern() else {
            ui.label("Select an instrument track with a pattern.");
            return;
        };
        let track_count = self
            .project
            .tracks
            .iter()
            .filter(|t| t.instrument.is_some())
            .count();
        let row_h = 26.0;
        let label_w = 150.0;
        let height = (track_count as f32 * row_h + 34.0).max(80.0);
        egui::ScrollArea::vertical().show(ui, |ui| {
            let (response, painter) =
                ui.allocate_painter(vec2(ui.available_width(), height), Sense::click());
            let rect = response.rect;
            painter.rect_filled(rect, CornerRadius::ZERO, Color32::from_rgb(14, 17, 23));
            let sixteenth = 240i64;
            let mut row = 0usize;
            let mut toggle: Option<(TrackId, PatternId, i64, u8)> = None;
            for track in self
                .project
                .tracks
                .iter()
                .filter(|t| t.instrument.is_some())
            {
                let y = rect.top() + 26.0 + row as f32 * row_h;
                let active = track.active_pattern().map(|p| p.id) == Some(pattern_id);
                if track.id == selected {
                    painter.rect_filled(
                        Rect::from_min_size(pos2(rect.left(), y), vec2(rect.width(), row_h)),
                        CornerRadius::ZERO,
                        Color32::from_rgb(23, 29, 38),
                    );
                }
                painter.text(
                    pos2(rect.left() + 8.0, y + row_h * 0.5),
                    Align2::LEFT_CENTER,
                    &track.name,
                    FontId::proportional(11.0),
                    if active {
                        Color32::WHITE
                    } else {
                        Color32::from_gray(165)
                    },
                );
                if let Some(pattern) = track.active_pattern() {
                    let steps = (pattern.length_ticks / sixteenth).clamp(1, 128) as usize;
                    let cell_w = ((rect.width() - label_w - 12.0) / steps as f32)
                        .min(26.0)
                        .max(6.0);
                    let start_x = rect.left() + label_w;
                    for step in 0..steps {
                        let cx = start_x + step as f32 * cell_w;
                        let note_start = step as i64 * sixteenth;
                        let on = pattern
                            .notes
                            .iter()
                            .any(|n| n.start == note_start && n.pitch == track.default_note());
                        let beat = step % 4 == 0;
                        let cell =
                            Rect::from_min_size(pos2(cx, y + 4.0), vec2(cell_w - 2.0, row_h - 8.0));
                        painter.rect_filled(
                            cell,
                            CornerRadius::same(2),
                            if on {
                                Color32::from_rgb(track.color[0], track.color[1], track.color[2])
                            } else if beat {
                                Color32::from_rgb(38, 46, 58)
                            } else {
                                Color32::from_rgb(28, 34, 43)
                            },
                        );
                        if response.clicked() {
                            if let Some(pos) = response.interact_pointer_pos() {
                                if cell.contains(pos) {
                                    toggle = Some((
                                        track.id,
                                        pattern.id,
                                        note_start,
                                        track.default_note(),
                                    ));
                                }
                            }
                        }
                    }
                }
                row += 1;
            }
            if let Some((track, pattern, start, pitch)) = toggle {
                self.push_undo();
                self.project
                    .toggle_step(track, pattern, start, 240, pitch, 100);
                self.touch();
            }
        });
    }

    fn piano(&mut self, ui: &mut Ui) {
        let Some((track_id, pattern_id)) = self.selected_pattern() else {
            ui.label("Select an instrument track with a pattern.");
            return;
        };
        let pattern = self
            .project
            .track(track_id)
            .and_then(|t| t.pattern(pattern_id))
            .cloned();
        let Some(pattern) = pattern else {
            return;
        };
        let row_h = 12.0;
        let low = 36u8;
        let high = 84u8;
        let rows = (high - low + 1) as f32;
        let height = rows * row_h + 26.0;
        egui::ScrollArea::vertical().show(ui, |ui| {
            let (response, painter) =
                ui.allocate_painter(vec2(ui.available_width(), height), Sense::click_and_drag());
            let rect = response.rect;
            painter.rect_filled(rect, CornerRadius::ZERO, Color32::from_rgb(14, 17, 23));
            let keys_w = 46.0;
            let grid = Rect::from_min_max(pos2(rect.left() + keys_w, rect.top() + 24.0), rect.max);
            let length = pattern.length_ticks.max(1) as f32;
            let px = grid.width() / length;
            let mut pitch = high;
            let mut y = grid.top();
            while pitch >= low {
                let note_row = Rect::from_min_size(pos2(grid.left(), y), vec2(grid.width(), row_h));
                let black = matches!(pitch % 12, 1 | 3 | 6 | 8 | 10);
                painter.rect_filled(
                    note_row,
                    CornerRadius::ZERO,
                    if black {
                        Color32::from_rgb(18, 22, 29)
                    } else {
                        Color32::from_rgb(22, 27, 35)
                    },
                );
                if pitch % 12 == 0 {
                    painter.text(
                        pos2(rect.left() + 5.0, y + 1.0),
                        Align2::LEFT_TOP,
                        format!("C{}", pitch as i32 / 12 - 1),
                        FontId::monospace(9.0),
                        Color32::from_gray(145),
                    );
                    painter.line_segment(
                        [pos2(grid.left(), y), pos2(grid.right(), y)],
                        Stroke::new(1.0, Color32::from_rgb(45, 55, 70)),
                    );
                }
                pitch -= 1;
                y += row_h;
            }
            let bar_ticks = self.project.time_signature.ticks_per_bar() as f32;
            let mut bar = 0;
            while bar as f32 * bar_ticks <= length {
                let x = grid.left() + (bar as f32 * bar_ticks) * px;
                painter.line_segment(
                    [pos2(x, grid.top()), pos2(x, grid.bottom())],
                    Stroke::new(1.0, Color32::from_rgb(42, 51, 64)),
                );
                bar += 1;
            }
            let mut hover_note: Option<NoteId> = None;
            let mut hover_resize = false;
            let pointer = response.interact_pointer_pos();
            for note in &pattern.notes {
                if note.pitch < low || note.pitch > high {
                    continue;
                }
                let nx = grid.left() + note.start as f32 * px;
                let nw = (note.length as f32 * px).max(3.0);
                let ny = grid.top() + (high - note.pitch) as f32 * row_h;
                let nr = Rect::from_min_size(pos2(nx, ny + 1.0), vec2(nw, row_h - 2.0));
                let selected = self.selected_note == Some((track_id, pattern_id, note.id));
                painter.rect_filled(
                    nr,
                    CornerRadius::same(2),
                    if selected {
                        Color32::WHITE
                    } else {
                        Color32::from_rgb(105, 200, 160)
                    },
                );
                painter.rect_filled(
                    Rect::from_min_size(pos2(nr.right() - 2.0, nr.top()), vec2(2.0, nr.height())),
                    CornerRadius::ZERO,
                    if selected {
                        Color32::from_rgb(20, 70, 55)
                    } else {
                        Color32::from_rgba_unmultiplied(10, 40, 30, 180)
                    },
                );
                if let Some(p) = pointer {
                    if nr.contains(p) {
                        hover_note = Some(note.id);
                        hover_resize = nr.right() - p.x <= 5.0;
                    }
                }
            }
            if hover_resize {
                ui.ctx().set_cursor_icon(egui::CursorIcon::ResizeHorizontal);
            }
            if response.drag_started() {
                if let Some(pos) = response.interact_pointer_pos() {
                    if grid.contains(pos) {
                        if let Some(note_id) = hover_note {
                            if let Some(note) = pattern.notes.iter().find(|n| n.id == note_id) {
                                self.push_undo();
                                self.note_drag = Some(NoteDrag {
                                    track: track_id,
                                    pattern: pattern_id,
                                    note: note_id,
                                    mode: if hover_resize {
                                        NoteDragMode::Resize
                                    } else {
                                        NoteDragMode::Move
                                    },
                                    pointer: pos,
                                    start: note.start,
                                    pitch: note.pitch,
                                });
                                self.selected_note = Some((track_id, pattern_id, note_id));
                            }
                        }
                    }
                }
            }
            if response.dragged() {
                if let (Some(drag), Some(pos)) = (self.note_drag, response.interact_pointer_pos()) {
                    if drag.mode == NoteDragMode::Resize {
                        let end = snap_tick(
                            ((pos.x - grid.left()) / px.max(0.0001)) as i64,
                            self.project.snap,
                            self.project.time_signature,
                        );
                        if let Some(note) = self
                            .project
                            .track_mut(drag.track)
                            .and_then(|t| t.pattern_mut(drag.pattern))
                            .and_then(|p| p.notes.iter_mut().find(|n| n.id == drag.note))
                        {
                            note.length = (end - note.start).max(30);
                        }
                    } else {
                        let delta_ticks = ((pos.x - drag.pointer.x) / px.max(0.0001)) as i64;
                        let new_start = snap_tick(
                            drag.start + delta_ticks,
                            self.project.snap,
                            self.project.time_signature,
                        )
                        .max(0);
                        let row = ((pos.y - grid.top()) / row_h).floor() as i32;
                        let new_pitch = (high as i32 - row).clamp(low as i32, high as i32) as u8;
                        if let Some(note) = self
                            .project
                            .track_mut(drag.track)
                            .and_then(|t| t.pattern_mut(drag.pattern))
                            .and_then(|p| p.notes.iter_mut().find(|n| n.id == drag.note))
                        {
                            note.start = new_start;
                            note.pitch = new_pitch;
                        }
                    }
                    self.touch();
                }
            }
            if response.drag_stopped() {
                self.note_drag = None;
            }
            if response.clicked() {
                if let Some(pos) = response.interact_pointer_pos() {
                    if grid.contains(pos) {
                        if let Some(note_id) = hover_note {
                            self.selected_note = Some((track_id, pattern_id, note_id));
                        } else {
                            self.push_undo();
                            let tick = snap_tick(
                                ((pos.x - grid.left()) / px.max(0.0001)) as i64,
                                self.project.snap,
                                self.project.time_signature,
                            );
                            let row = ((pos.y - grid.top()) / row_h).floor() as i32;
                            let pitch = (high as i32 - row).clamp(low as i32, high as i32) as u8;
                            if let Some(id) = self.project.add_note(
                                track_id,
                                pattern_id,
                                pitch,
                                tick,
                                self.project.snap.ticks(self.project.time_signature).max(60),
                                100,
                            ) {
                                self.selected_note = Some((track_id, pattern_id, id));
                                self.touch();
                            }
                        }
                    }
                }
            }
            if response.secondary_clicked() {
                if let Some(note_id) = hover_note {
                    self.push_undo();
                    self.project.remove_note(track_id, pattern_id, note_id);
                    self.touch();
                }
            }
        });
    }

    fn mixer(&mut self, ui: &mut Ui) {
        let meters: HashMap<TrackId, TrackMeasurement> = if let Some(session) = &self.session {
            let ids: Vec<TrackId> = self.project.tracks.iter().map(|t| t.id).collect();
            measurements(&session.shared, &ids)
        } else {
            HashMap::new()
        };
        let mut changed = false;
        let mut remove_fx: Option<(TrackId, usize)> = None;
        let mut open_plugins: Option<TrackId> = None;
        egui::ScrollArea::horizontal().show(ui, |ui| {
            ui.horizontal_top(|ui| {
                for index in 0..self.project.tracks.len() {
                    let meter = self
                        .project
                        .tracks
                        .get(index)
                        .map(|t| meters.get(&t.id).copied().unwrap_or_default())
                        .unwrap_or_default();
                    let track = &mut self.project.tracks[index];
                    ui.allocate_ui_with_layout(
                        vec2(94.0, ui.available_height() - 6.0),
                        egui::Layout::top_down(egui::Align::Center),
                        |ui| {
                            ui.label(egui::RichText::new(&track.name).strong().size(11.0).color(
                                Color32::from_rgb(track.color[0], track.color[1], track.color[2]),
                            ));
                            ui.label(
                                egui::RichText::new(track.spatial.role.short())
                                    .small()
                                    .color(Color32::from_gray(140)),
                            );
                            let (meter_rect, _) =
                                ui.allocate_exact_size(vec2(64.0, 8.0), Sense::hover());
                            let level = db_to_gain(meter.rms_db).clamp(0.0, 1.0);
                            ui.painter().rect_filled(
                                meter_rect,
                                CornerRadius::same(2),
                                Color32::from_rgb(24, 30, 38),
                            );
                            ui.painter().rect_filled(
                                Rect::from_min_size(
                                    meter_rect.min,
                                    vec2(meter_rect.width() * level, meter_rect.height()),
                                ),
                                CornerRadius::same(2),
                                if meter.rms_db > -1.0 {
                                    Color32::from_rgb(235, 90, 70)
                                } else {
                                    Color32::from_rgb(80, 200, 140)
                                },
                            );
                            if ui
                                .add_sized(
                                    [58.0, 120.0],
                                    egui::Slider::new(&mut track.volume_db, -60.0..=6.0)
                                        .vertical()
                                        .show_value(false),
                                )
                                .changed()
                            {
                                changed = true;
                            }
                            ui.label(
                                egui::RichText::new(format!("{:.1} dB", track.volume_db)).small(),
                            );
                            if ui
                                .add(
                                    egui::Slider::new(&mut track.pan, -1.0..=1.0).show_value(false),
                                )
                                .changed()
                            {
                                changed = true;
                            }
                            ui.horizontal(|ui| {
                                if ui.selectable_label(track.mute, "M").clicked() {
                                    track.mute = !track.mute;
                                    changed = true;
                                }
                                if ui.selectable_label(track.solo, "S").clicked() {
                                    track.solo = !track.solo;
                                    changed = true;
                                }
                            });
                            for (fx_index, fx) in track.fx.iter().enumerate() {
                                ui.horizontal(|ui| {
                                    ui.label(egui::RichText::new(&fx.spec.name).small());
                                    if ui.small_button("x").clicked() {
                                        remove_fx = Some((track.id, fx_index));
                                    }
                                });
                            }
                            if ui.small_button("+ FX").clicked() {
                                open_plugins = Some(track.id);
                            }
                        },
                    );
                    ui.separator();
                }
                ui.allocate_ui_with_layout(
                    vec2(100.0, ui.available_height() - 6.0),
                    egui::Layout::top_down(egui::Align::Center),
                    |ui| {
                        ui.label(egui::RichText::new("MASTER").strong());
                        let (rms_l, _rms_r, peak_l, _peak_r) = self
                            .session
                            .as_ref()
                            .map(|s| s.shared.master_peaks())
                            .unwrap_or((0.0, 0.0, 0.0, 0.0));
                        ui.label(format!("{:>5.1} dB", rms_l));
                        if ui
                            .add_sized(
                                [58.0, 120.0],
                                egui::Slider::new(&mut self.project.master.volume_db, -60.0..=6.0)
                                    .vertical()
                                    .show_value(false),
                            )
                            .changed()
                        {
                            changed = true;
                        }
                        ui.label(format!("peak {:.1}", peak_l));
                        if ui
                            .checkbox(&mut self.project.master.limiter, "Limit")
                            .changed()
                        {
                            changed = true;
                        }
                    },
                );
            });
        });
        if let Some((track, index)) = remove_fx {
            self.push_undo();
            if let Some(t) = self.project.track_mut(track) {
                if index < t.fx.len() {
                    t.fx.remove(index);
                }
            }
            self.touch();
        }
        if let Some(track) = open_plugins {
            self.selected_track = Some(track);
            self.show_plugins = true;
        }
        if changed {
            self.touch();
        }
    }

    fn space(&mut self, ui: &mut Ui, frame: &eframe::Frame) {
        ui.horizontal(|ui| {
            if ui.button("3D").clicked() {
                self.camera.preset_3d();
            }
            if ui.button("Top").clicked() {
                self.camera.preset_top();
            }
            if ui.button("Front").clicked() {
                self.camera.preset_front();
            }
            if ui.button("Side").clicked() {
                self.camera.preset_side();
            }
            ui.checkbox(&mut self.show_targets, "Ghosts");
            if ui.button("Auto-place").clicked() {
                self.push_undo();
                let placements = auto_place(&self.project, self.project.spatial_style);
                for p in placements {
                    self.project.set_track_pan(p.track, p.pan);
                    self.project.set_track_db(p.track, p.volume_db);
                }
                self.touch();
            }
            ui.label(
                egui::RichText::new(
                    "Drag sphere: pan ↔ / level ↕ · drag empty: orbit · wheel: zoom",
                )
                .small()
                .color(Color32::from_gray(140)),
            );
        });
        let size = ui.available_size();
        let (response, painter) = ui.allocate_painter(size, Sense::click_and_drag());
        let rect = response.rect;
        painter.rect_filled(rect, CornerRadius::ZERO, Color32::from_rgb(10, 13, 19));

        let analysis = self.analysis.clone();
        let mut scene = Scene::default();
        let mut markers: Vec<SpatialMarker> = Vec::new();
        for ta in &analysis.tracks {
            let Some(track) = self.project.track(ta.track) else {
                continue;
            };
            let color = [
                track.color[0] as f32 / 255.0,
                track.color[1] as f32 / 255.0,
                track.color[2] as f32 / 255.0,
                0.96,
            ];
            let sphere = Sphere {
                center: world_position(ta.actual.pan, ta.actual.height(), ta.actual.depth),
                radius: 0.26 + ta.actual_size * 0.95,
                color,
            };
            let ghost = if self.show_targets {
                ta.target.map(|target| {
                    let position = target.position();
                    Sphere {
                        center: world_position(position.pan, position.height(), position.depth),
                        radius: 0.22 + target.size() * 0.95,
                        color: [color[0], color[1], color[2], 0.18],
                    }
                })
            } else {
                None
            };
            if let Some(ghost) = ghost {
                scene.ghosts.push(ghost);
            }
            scene.spheres.push(sphere);
            markers.push(SpatialMarker {
                track: ta.track,
                sphere,
                target: ghost,
                role: ta.role,
                score: ta.score,
                measured_hz: ta.measured_hz,
            });
        }

        let texture = if let Some(render_state) = frame.wgpu_render_state() {
            if self.renderer3d.is_none() {
                self.renderer3d = Some(Mixer3D::new(render_state));
            }
            let ppp = ui.ctx().pixels_per_point();
            let physical = (
                (rect.width() * ppp).round().max(16.0) as u32,
                (rect.height() * ppp).round().max(16.0) as u32,
            );
            self.renderer3d
                .as_mut()
                .map(|r| r.render(render_state, physical, &self.camera, &scene))
        } else {
            None
        };

        if let Some(texture) = texture {
            painter.image(
                texture,
                rect,
                Rect::from_min_max(pos2(0.0, 0.0), pos2(1.0, 1.0)),
                Color32::WHITE,
            );
        } else {
            // 2D fallback seen when wgpu is not available.
            let cx = rect.center().x;
            let cy = rect.center().y;
            for (index, marker) in markers.iter().enumerate() {
                let sphere = marker.sphere;
                let x = cx + (sphere.center.x / 6.0) * rect.width() * 0.8;
                let y = cy + (sphere.center.z / 6.0) * rect.height() * 0.7;
                let radius = 5.0 + sphere.radius * 8.0;
                painter.circle_filled(
                    pos2(x, y),
                    radius,
                    Color32::from_rgba_unmultiplied(
                        (sphere.color[0] * 255.0) as u8,
                        (sphere.color[1] * 255.0) as u8,
                        (sphere.color[2] * 255.0) as u8,
                        220,
                    ),
                );
                let name = self
                    .project
                    .track(marker.track)
                    .map(|t| t.name.clone())
                    .unwrap_or_else(|| index.to_string());
                painter.text(
                    pos2(x + radius + 3.0, y),
                    Align2::LEFT_CENTER,
                    name,
                    FontId::proportional(10.0),
                    Color32::WHITE,
                );
            }
        }

        let pan_color = Color32::from_rgb(225, 110, 110);
        let freq_color = Color32::from_rgb(115, 205, 150);
        let depth_color = Color32::from_rgb(110, 170, 240);
        for (world, text, color) in [
            (Vec3::new(-6.1, -2.85, 0.0), "L", pan_color),
            (Vec3::new(6.1, -2.85, 0.0), "R", pan_color),
            (Vec3::new(0.0, -2.85, -6.1), "FAR", depth_color),
            (Vec3::new(0.0, -2.85, 6.1), "NEAR", depth_color),
            (Vec3::new(0.0, -2.98, 0.0), "LOW", freq_color),
            (Vec3::new(0.0, 3.0, 0.0), "HIGH", freq_color),
        ] {
            if let Some(pos) = self.camera.project(world, rect) {
                if rect.shrink(8.0).contains(pos) {
                    draw_tag(&painter, pos, text, color);
                }
            }
        }
        for (hz, label) in [
            (100.0_f32, "100 Hz"),
            (1_000.0, "1 kHz"),
            (10_000.0, "10 kHz"),
        ] {
            let y = -3.0 + gibson::frequency_to_height(hz) * 6.0;
            if let Some(pos) = self.camera.project(Vec3::new(-6.05, y, -6.0), rect) {
                if rect.shrink(8.0).contains(pos) {
                    draw_tag(&painter, pos, label, Color32::from_gray(135));
                }
            }
        }

        // Tether each element to its reference target so the coaching image is
        // readable without having to match colours by eye.
        for marker in &markers {
            if let Some(target) = marker.target {
                if let (Some(from), Some(to)) = (
                    self.camera.project(marker.sphere.center, rect),
                    self.camera.project(target.center, rect),
                ) {
                    let selected = self.selected_track == Some(marker.track);
                    let stroke = if selected {
                        Stroke::new(1.4, Color32::from_rgba_unmultiplied(220, 235, 245, 130))
                    } else {
                        Stroke::new(1.0, Color32::from_rgba_unmultiplied(120, 175, 215, 55))
                    };
                    dashed_line(&painter, from, to, stroke, 5.0, 5.0);
                    if let Some(radius) =
                        self.camera
                            .project_radius(target.center, target.radius, rect)
                    {
                        painter.circle_stroke(
                            to,
                            radius + 2.0,
                            Stroke::new(
                                if selected { 1.2 } else { 0.8 },
                                Color32::from_rgba_unmultiplied(
                                    200,
                                    225,
                                    245,
                                    if selected { 120 } else { 45 },
                                ),
                            ),
                        );
                    }
                }
            }
        }

        let hover_pos = response.hover_pos();
        let hit = hover_pos.and_then(|pos| {
            markers
                .iter()
                .enumerate()
                .filter_map(|(index, marker)| {
                    let screen = self.camera.project(marker.sphere.center, rect)?;
                    let radius = self
                        .camera
                        .project_radius(marker.sphere.center, marker.sphere.radius, rect)
                        .unwrap_or(10.0)
                        .max(12.0);
                    let distance = screen.distance(pos);
                    (distance <= radius + 7.0).then_some((index, distance))
                })
                .min_by(|a, b| a.1.total_cmp(&b.1))
                .map(|(index, _)| index)
        });

        let mut label_screens: Vec<(usize, Pos2, f32, String, bool)> = markers
            .iter()
            .enumerate()
            .filter_map(|(index, marker)| {
                let screen = self.camera.project(marker.sphere.center, rect)?;
                let radius = self
                    .camera
                    .project_radius(marker.sphere.center, marker.sphere.radius, rect)
                    .unwrap_or(10.0);
                let name = self
                    .project
                    .track(marker.track)
                    .map(|t| t.name.clone())
                    .unwrap_or_default();
                Some((
                    index,
                    screen,
                    radius,
                    name,
                    self.selected_track == Some(marker.track),
                ))
            })
            .collect();
        label_screens.sort_by(|a, b| a.1.y.total_cmp(&b.1.y));
        let mut occupied: Vec<Rect> = Vec::new();
        for (_, screen, radius, name, selected) in &label_screens {
            let screen = *screen;
            let radius = *radius;
            let selected = *selected;
            if selected {
                painter.circle_stroke(screen, radius + 3.0, Stroke::new(1.6, Color32::WHITE));
                painter.circle_stroke(
                    screen,
                    radius + 6.0,
                    Stroke::new(2.5, Color32::from_rgba_unmultiplied(255, 255, 255, 22)),
                );
            }
            let width = name.chars().count() as f32 * 5.6 + 4.0;
            let x = (screen.x + radius + 6.0)
                .min(rect.right() - width - 4.0)
                .max(rect.left() + 4.0);
            let mut y = (screen.y - 8.0).clamp(rect.top() + 4.0, rect.bottom() - 16.0);
            let mut label_rect = Rect::from_min_size(pos2(x, y), vec2(width, 12.0));
            let mut attempts = 0;
            while occupied.iter().any(|r| r.intersects(label_rect)) && attempts < 10 {
                y += 13.0;
                if y + 12.0 > rect.bottom() - 4.0 {
                    break;
                }
                label_rect = Rect::from_min_size(pos2(x, y), vec2(width, 12.0));
                attempts += 1;
            }
            if label_rect.top() > screen.y - 2.0 || label_rect.left() > screen.x + radius {
                painter.line_segment(
                    [screen, label_rect.left_center()],
                    Stroke::new(1.0, Color32::from_rgba_unmultiplied(180, 200, 220, 80)),
                );
            }
            occupied.push(label_rect);
            painter.text(
                label_rect.min + vec2(1.0, 1.0),
                Align2::LEFT_TOP,
                name,
                FontId::proportional(10.0),
                Color32::from_black_alpha(180),
            );
            painter.text(
                label_rect.min,
                Align2::LEFT_TOP,
                name,
                FontId::proportional(10.0),
                if selected {
                    Color32::WHITE
                } else {
                    Color32::from_gray(225)
                },
            );
        }

        if self.spatial_drag.is_none() {
            if let (Some(pointer), Some(index)) = (hover_pos, hit) {
                if let Some(marker) = markers.get(index) {
                    if let Some(track) = self.project.track(marker.track) {
                        let text = format!(
                            "{}\n{} · match {:.0}%\npan {:+.0}% · {:.1} dB\n{:.0} Hz measured",
                            track.name,
                            marker.role.label(),
                            marker.score,
                            track.pan * 100.0,
                            track.volume_db,
                            marker.measured_hz
                        );
                        let card_size = vec2(210.0, 68.0);
                        let x = (pointer.x + 16.0)
                            .min(rect.right() - card_size.x - 8.0)
                            .max(rect.left() + 8.0);
                        let y = (pointer.y + 16.0)
                            .min(rect.bottom() - card_size.y - 8.0)
                            .max(rect.top() + 8.0);
                        let card = Rect::from_min_size(pos2(x, y), card_size);
                        painter.rect_filled(
                            card,
                            CornerRadius::same(4),
                            Color32::from_black_alpha(225),
                        );
                        painter.rect_stroke(
                            card,
                            CornerRadius::same(4),
                            Stroke::new(1.0, Color32::from_rgb(70, 90, 115)),
                            StrokeKind::Inside,
                        );
                        painter.text(
                            card.min + vec2(8.0, 6.0),
                            Align2::LEFT_TOP,
                            text,
                            FontId::monospace(9.5),
                            Color32::from_gray(225),
                        );
                    }
                }
            }
        }

        painter.text(
            pos2(rect.left() + 10.0, rect.bottom() - 18.0),
            Align2::LEFT_CENTER,
            "Drag sphere: pan ↔ / level ↕    ·    drag empty: orbit    ·    wheel: zoom",
            FontId::monospace(9.5),
            Color32::from_gray(120),
        );
        if rect.width() > 620.0 {
            let y = rect.bottom() - 18.0;
            let mut x = rect.right() - 188.0;
            for (label, color) in [
                ("X PAN", pan_color),
                ("Y FREQ", freq_color),
                ("Z DEPTH", depth_color),
            ] {
                painter.circle_filled(pos2(x, y), 3.0, color);
                painter.text(
                    pos2(x + 7.0, y),
                    Align2::LEFT_CENTER,
                    label,
                    FontId::monospace(9.0),
                    Color32::from_gray(140),
                );
                x += 62.0;
            }
        }

        if response.hovered() {
            let scroll = ui.input(|i| i.smooth_scroll_delta.y);
            if scroll != 0.0 {
                self.camera.zoom(scroll);
            }
        }
        if response.drag_started() {
            if let Some(index) = hit {
                let track = markers[index].track;
                self.push_undo();
                self.selected_track = Some(track);
                self.spatial_drag = Some(track);
            } else {
                self.spatial_drag = None;
            }
        }
        if response.dragged() {
            let delta = ui.input(|i| i.pointer.delta());
            if let Some(track) = self.spatial_drag {
                if let Some(t) = self.project.track_mut(track) {
                    t.pan = (t.pan + delta.x * 0.004).clamp(-1.0, 1.0);
                    t.volume_db = (t.volume_db - delta.y * 0.22).clamp(-60.0, 6.0);
                    self.touch();
                }
            } else {
                self.camera.orbit(delta.x, delta.y);
            }
        }
        if response.drag_stopped() {
            self.spatial_drag = None;
        }
    }

    fn devices_window(&mut self, ctx: &egui::Context) {
        let mut open = self.show_devices;
        let mut refresh = false;
        let mut refresh_midi = false;
        let mut start = false;
        let mut stop = false;
        let mut connect_midi = false;
        egui::Window::new("Audio & MIDI")
            .open(&mut open)
            .default_width(520.0)
            .resizable(true)
            .show(ctx, |ui| {
                ui.heading("Audio interface");
                ui.label(
                    egui::RichText::new(if self.last_device.is_empty() {
                        "No stream open".to_string()
                    } else {
                        self.last_device.clone()
                    })
                    .small()
                    .color(Color32::from_gray(155)),
                );
                let host_name = self
                    .audio
                    .hosts
                    .get(self.audio.host_idx)
                    .map(|h| h.name.clone())
                    .unwrap_or_else(|| "default".to_string());
                ui.horizontal(|ui| {
                    ui.label("Host");
                    egui::ComboBox::from_id_salt("audio-host")
                        .width(160.0)
                        .selected_text(host_name)
                        .show_ui(ui, |ui| {
                            for (index, host) in self.audio.hosts.iter().enumerate() {
                                let label = if host.is_asio {
                                    format!("{} (ASIO)", host.name)
                                } else {
                                    host.name.clone()
                                };
                                if ui
                                    .selectable_label(self.audio.host_idx == index, label)
                                    .clicked()
                                {
                                    self.audio.host_idx = index;
                                    refresh = true;
                                }
                            }
                        });
                    if ui.button("Refresh").clicked() {
                        refresh = true;
                    }
                });
                let device_name = self
                    .audio
                    .devices
                    .get(self.audio.device_idx)
                    .map(|d| d.name.clone())
                    .unwrap_or_else(|| "no output".to_string());
                ui.horizontal(|ui| {
                    ui.label("Output");
                    egui::ComboBox::from_id_salt("audio-device")
                        .width(300.0)
                        .selected_text(device_name)
                        .show_ui(ui, |ui| {
                            for (index, device) in self.audio.devices.iter().enumerate() {
                                if ui
                                    .selectable_label(self.audio.device_idx == index, &device.name)
                                    .clicked()
                                {
                                    self.audio.device_idx = index;
                                    if !device.sample_rates.is_empty() {
                                        self.audio.rate = device.sample_rates[0];
                                    }
                                }
                            }
                        });
                });
                ui.horizontal(|ui| {
                    ui.label("Sample rate");
                    egui::ComboBox::from_id_salt("rate")
                        .selected_text(format!("{} Hz", self.audio.rate))
                        .show_ui(ui, |ui| {
                            let rates: Vec<u32> = self
                                .audio
                                .devices
                                .get(self.audio.device_idx)
                                .map(|d| d.sample_rates.clone())
                                .unwrap_or_else(|| vec![44_100, 48_000, 88_200, 96_000]);
                            for rate in rates {
                                ui.selectable_value(&mut self.audio.rate, rate, format!("{rate} Hz"));
                            }
                        });
                    ui.label("Buffer");
                    ui.add(egui::DragValue::new(&mut self.audio.buffer).range(32..=4096));
                    ui.label("frames");
                    if ui.button("Start audio").clicked() {
                        start = true;
                    }
                    if ui.button("Stop").clicked() {
                        stop = true;
                    }
                });
                if let Some(err) = &self.audio.error {
                    ui.colored_label(Color32::from_rgb(245, 130, 110), err);
                }
                ui.label(
                    egui::RichText::new(
                        "ASIO: build with --features asio and the Steinberg ASIO SDK on CPAL_ASIO_DIR. \
                         WASAPI is used by default on Windows.",
                    )
                    .small()
                    .color(Color32::from_gray(140)),
                );
                ui.separator();
                ui.heading("MIDI input");
                ui.horizontal(|ui| {
                    let current = self
                        .midi_ports
                        .get(self.midi_idx)
                        .map(|p| p.name.clone())
                        .unwrap_or_else(|| "No MIDI ports".to_string());
                    egui::ComboBox::from_id_salt("midi-port")
                        .width(280.0)
                        .selected_text(current)
                        .show_ui(ui, |ui| {
                            for (index, port) in self.midi_ports.iter().enumerate() {
                                ui.selectable_value(&mut self.midi_idx, index, &port.name);
                            }
                        });
                    if ui.button("Refresh ports").clicked() {
                        refresh_midi = true;
                    }
                    let label = if self.midi_connected { "Reconnect" } else { "Connect" };
                    if ui.button(label).clicked() {
                        connect_midi = true;
                    }
                });
                ui.label(
                    egui::RichText::new(
                        "MIDI notes play the selected instrument track. The target follows the selected track.",
                    )
                    .small()
                    .color(Color32::from_gray(140)),
                );
            });
        self.show_devices = open;
        if refresh {
            self.refresh_hosts();
        }
        if refresh_midi {
            self.midi_ports = input_ports();
        }
        if start {
            self.start_audio();
        }
        if stop {
            self.session = None;
            self.midi_hub = None;
            self.midi_connected = false;
            self.last_device.clear();
            self.status = "Audio stopped".to_string();
        }
        if connect_midi {
            self.connect_midi();
        }
    }

    fn connect_midi(&mut self) {
        let Some(session) = &self.session else {
            self.status = "Start audio before connecting MIDI.".to_string();
            return;
        };
        match MidiHub::connect(
            self.midi_idx,
            session.handle.command_sender(),
            session.shared.clone(),
        ) {
            Ok(hub) => {
                self.midi_hub = Some(hub);
                self.midi_connected = true;
                self.status = "MIDI input connected".to_string();
            }
            Err(err) => {
                self.midi_connected = false;
                self.status = format!("MIDI: {err}");
            }
        }
    }

    fn plugins_window(&mut self, ctx: &egui::Context) {
        let mut open = self.show_plugins;
        let mut scan = false;
        let mut add_instrument: Option<usize> = None;
        let mut add_insert: Option<usize> = None;
        egui::Window::new("Plugins")
            .open(&mut open)
            .default_width(580.0)
            .resizable(true)
            .show(ctx, |ui| {
                ui.horizontal(|ui| {
                    if ui.button("Scan VST3 + VST2").clicked() {
                        scan = true;
                    }
                    ui.label(format!("{} found", self.plugins.len()));
                    ui.label(
                        egui::RichText::new("Scanning only reads plugin metadata; plugins load on the audio thread.")
                            .small()
                            .color(Color32::from_gray(135)),
                    );
                });
                ui.separator();
                egui::ScrollArea::vertical().max_height(240.0).show(ui, |ui| {
                    for (index, plugin) in self.plugins.iter().enumerate() {
                        let selected = self.plugin_idx == index;
                        if ui
                            .selectable_label(
                                selected,
                                format!("{}  [{}]", plugin.name, plugin.format.label()),
                            )
                            .clicked()
                        {
                            self.plugin_idx = index;
                        }
                    }
                    if self.plugins.is_empty() {
                        ui.label("No plugins scanned yet.");
                    }
                });
                ui.separator();
                ui.horizontal(|ui| {
                    if ui.button("Add as instrument").clicked() {
                        add_instrument = Some(self.plugin_idx);
                    }
                    if ui.button("Add as insert on selected track").clicked() {
                        add_insert = Some(self.plugin_idx);
                    }
                });
                ui.separator();
                ui.heading("Loaded devices");
                let mut loaded = Vec::new();
                for track in &self.project.tracks {
                    if let Some(kind) = track.instrument_kind() {
                        if let InstrumentKind::Plugin(spec) = kind {
                            loaded.push(format!("{} · instrument · {}", track.name, spec.name));
                        }
                    }
                    for fx in &track.fx {
                        loaded.push(format!("{} · insert · {}", track.name, fx.spec.name));
                    }
                }
                for line in loaded {
                    ui.label(line);
                }
                ui.separator();
                ui.label(
                    egui::RichText::new(
                        "VST3 plugins load in-process through vst3-host. VST2 uses the AEffect ABI. \
                         Use 64-bit plugins that match the DAW build.",
                    )
                    .small()
                    .color(Color32::from_gray(140)),
                );
            });
        self.show_plugins = open;
        if scan {
            self.status = "Scanning plugin folders…".to_string();
            self.plugins = scan_plugins(&self.plugin_dirs);
            self.status = format!("Found {} plugin(s)", self.plugins.len());
        }
        if let Some(index) = add_instrument {
            if let Some(plugin) = self.plugins.get(index).cloned() {
                self.push_undo();
                let spec = PluginSpec::new(
                    plugin.format,
                    plugin.path.clone(),
                    plugin.name.clone(),
                    true,
                );
                let id = self.project.add_instrument_track(
                    plugin.name.clone(),
                    InstrumentKind::Plugin(spec),
                    ElementRole::guess(&plugin.name, false),
                );
                self.add_pattern_clip_for(id);
                self.selected_track = Some(id);
                self.touch();
                self.status = format!("Loaded instrument {}", plugin.name);
            }
        }
        if let Some(index) = add_insert {
            if let Some(plugin) = self.plugins.get(index).cloned() {
                let Some(track_id) = self.selected_track else {
                    self.status = "Select a track for the insert.".to_string();
                    return;
                };
                self.push_undo();
                let spec = PluginSpec::new(
                    plugin.format,
                    plugin.path.clone(),
                    plugin.name.clone(),
                    false,
                );
                let id = PluginInstanceId::new(self.project.alloc_id());
                if let Some(track) = self.project.track_mut(track_id) {
                    track.fx.push(FxSlot {
                        id,
                        enabled: true,
                        spec,
                    });
                }
                self.touch();
                self.status = format!("Inserted {}", plugin.name);
            }
        }
    }

    fn status_bar(&mut self, ui_root: &mut Ui) {
        egui::Panel::bottom("status").show(ui_root, |ui| {
            ui.horizontal(|ui| {
                let cpu = self
                    .session
                    .as_ref()
                    .map(|s| s.shared.cpu_load())
                    .unwrap_or(0.0);
                let (rms_l, rms_r, _peak_l, _peak_r) = self
                    .session
                    .as_ref()
                    .map(|s| s.shared.master_peaks())
                    .unwrap_or((0.0, 0.0, 0.0, 0.0));
                ui.label(
                    egui::RichText::new(format!(
                        "CPU {:.0}%  ·  Master L {:.1} dB  R {:.1} dB",
                        cpu * 100.0,
                        rms_l,
                        rms_r
                    ))
                    .monospace()
                    .small()
                    .color(Color32::from_gray(135)),
                );
                ui.separator();
                ui.label(
                    egui::RichText::new(&self.status)
                        .small()
                        .color(Color32::from_rgb(150, 200, 220)),
                );
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    ui.label(
                        egui::RichText::new(format!(
                            "{} tracks · {} BPM · {}",
                            self.project.tracks.len(),
                            self.project.bpm,
                            self.project.spatial_style.label()
                        ))
                        .small()
                        .color(Color32::from_gray(120)),
                    );
                });
            });
        });
    }
}

impl eframe::App for App {
    fn ui(&mut self, root: &mut Ui, frame: &mut eframe::Frame) {
        let ctx = root.ctx().clone();
        if !self.auto_started {
            self.auto_started = true;
            self.refresh_hosts();
            if !self.audio.devices.is_empty() {
                self.start_audio();
            }
            self.midi_ports = input_ports();
        }
        if self.need_snapshot {
            self.rebuild_snapshot();
            self.need_snapshot = false;
        }
        if let Some(session) = &self.session {
            // Follow the selected instrument track; if an audio track is
            // selected, keep MIDI playable on the first instrument track.
            let target = self
                .selected_track
                .filter(|id| {
                    self.project
                        .track(*id)
                        .map(|t| t.instrument.is_some())
                        .unwrap_or(false)
                })
                .or_else(|| {
                    self.project
                        .tracks
                        .iter()
                        .find(|t| t.instrument.is_some())
                        .map(|t| t.id)
                })
                .unwrap_or(TrackId::NONE);
            let current = TrackId::new(session.shared.midi_target.load(Ordering::Relaxed));
            if current != target {
                let _ = session.handle.send(EngineCommand::SetMidiTarget(target));
            }
        }
        let ids: Vec<TrackId> = self.project.tracks.iter().map(|t| t.id).collect();
        let meter_map = if let Some(session) = &self.session {
            measurements(&session.shared, &ids)
        } else {
            ids.iter()
                .map(|id| (*id, TrackMeasurement::default()))
                .collect()
        };
        self.analysis = analyze(&self.project, &meter_map, self.project.spatial_style);

        let typing = ctx.egui_wants_keyboard_input();
        if !typing {
            let (space, undo, redo, save, save_as, open, export, split, delete) = ctx.input(|i| {
                (
                    i.key_pressed(egui::Key::Space),
                    i.key_pressed(egui::Key::Z) && i.modifiers.command,
                    (i.key_pressed(egui::Key::Y) && i.modifiers.command)
                        || (i.key_pressed(egui::Key::Z)
                            && i.modifiers.command
                            && i.modifiers.shift),
                    i.key_pressed(egui::Key::S) && i.modifiers.command && !i.modifiers.shift,
                    i.key_pressed(egui::Key::S) && i.modifiers.command && i.modifiers.shift,
                    i.key_pressed(egui::Key::O) && i.modifiers.command,
                    i.key_pressed(egui::Key::E) && i.modifiers.command,
                    i.key_pressed(egui::Key::S) && !i.modifiers.command,
                    i.key_pressed(egui::Key::Delete) || i.key_pressed(egui::Key::Backspace),
                )
            });
            if space {
                self.toggle_play();
            }
            if undo {
                self.undo();
            }
            if redo {
                self.redo();
            }
            if save {
                self.save_project(false);
            }
            if save_as {
                self.save_project(true);
            }
            if open {
                self.open_project();
            }
            if export {
                self.export_audio();
            }
            if split {
                self.split_selected();
            }
            if delete {
                self.delete_selection();
            }
        }

        self.top_bar(root);
        self.status_bar(root);
        self.editor(root, frame);
        self.browser(root);
        self.coach(root);
        self.arranger(root);
        self.devices_window(&ctx);
        self.plugins_window(&ctx);

        let playing = self
            .session
            .as_ref()
            .map(|s| s.shared.playing.load(Ordering::Relaxed))
            .unwrap_or(false);
        if playing || self.midi_connected {
            ctx.request_repaint_after(Duration::from_millis(16));
        } else if self.session.is_some() {
            ctx.request_repaint_after(Duration::from_millis(60));
        }
        let _ = frame;
    }
}

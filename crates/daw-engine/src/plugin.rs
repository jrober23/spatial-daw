//! VST2 and VST3 plugin hosting.
//!
//! Plugins are loaded in-process on the audio thread. Scanning only walks the
//! filesystem and never executes plugin code; the actual `.dll`/`.vst3` is
//! loaded when the user adds it to a track. A misbehaving plugin can therefore
//! still crash the host, so save often — this is documented as a v1 limitation.
//!
//! VST3 uses `vst3-host`'s safe wrapper. VST2 uses the ABI types from
//! `vst2-sys` and the documented 2.4 dispatcher/process contract.

use std::collections::{HashMap, HashSet};
use std::ffi::{c_char, c_void, CStr};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU32, Ordering};

use daw_core::model::{PluginFormat, PluginSpec};
use daw_core::{PluginInstanceId, TrackId};
use vst2_sys::AEffect;

pub fn insert_key(id: PluginInstanceId) -> u64 {
    id.0
}

pub fn instrument_key(track: TrackId) -> u64 {
    track.0 | (1u64 << 63)
}

pub fn is_instrument_key(key: u64) -> bool {
    key & (1u64 << 63) != 0
}

#[derive(Debug, Clone)]
pub struct PluginParamInfo {
    pub id: u32,
    pub name: String,
    pub value: f64,
    pub default: f64,
    pub unit: String,
    pub step_count: i32,
    pub read_only: bool,
}

#[derive(Debug, Clone)]
pub struct PluginDescriptor {
    pub format: PluginFormat,
    pub path: String,
    pub name: String,
    pub vendor: String,
}

#[derive(Debug, Clone)]
pub struct PluginRequest {
    pub key: u64,
    pub spec: PluginSpec,
}

/// Standard VST3 locations for the current platform.
pub fn vst3_directories() -> Vec<PathBuf> {
    let mut dirs = Vec::new();
    #[cfg(windows)]
    {
        for var in ["PROGRAMFILES", "PROGRAMFILES(X86)"] {
            if let Ok(base) = std::env::var(var) {
                dirs.push(PathBuf::from(base).join("Common Files").join("VST3"));
            }
        }
        if let Ok(local) = std::env::var("LOCALAPPDATA") {
            dirs.push(
                PathBuf::from(local)
                    .join("Programs")
                    .join("Common")
                    .join("VST3"),
            );
        }
    }
    #[cfg(target_os = "macos")]
    {
        dirs.push(PathBuf::from("/Library/Audio/Plug-Ins/VST3"));
        if let Some(home) = dirs::home_dir() {
            dirs.push(home.join("Library/Audio/Plug-Ins/VST3"));
        }
    }
    #[cfg(all(unix, not(target_os = "macos")))]
    {
        dirs.push(PathBuf::from("/usr/lib/vst3"));
        dirs.push(PathBuf::from("/usr/local/lib/vst3"));
        if let Ok(home) = std::env::var("HOME") {
            dirs.push(PathBuf::from(home).join(".vst3"));
        }
    }
    dirs
}

/// Standard VST2 locations for the current platform.
pub fn vst2_directories() -> Vec<PathBuf> {
    let mut dirs = Vec::new();
    #[cfg(windows)]
    {
        for var in ["PROGRAMFILES", "PROGRAMFILES(X86)"] {
            if let Ok(base) = std::env::var(var) {
                let base = PathBuf::from(base);
                dirs.push(base.join("VstPlugins"));
                dirs.push(base.join("Steinberg").join("VstPlugins"));
                dirs.push(base.join("Common Files").join("VST2"));
            }
        }
    }
    #[cfg(target_os = "macos")]
    {
        dirs.push(PathBuf::from("/Library/Audio/Plug-Ins/VST"));
        if let Some(home) = dirs::home_dir() {
            dirs.push(home.join("Library/Audio/Plug-Ins/VST"));
        }
    }
    #[cfg(all(unix, not(target_os = "macos")))]
    {
        dirs.push(PathBuf::from("/usr/lib/vst"));
        dirs.push(PathBuf::from("/usr/local/lib/vst"));
        if let Ok(home) = std::env::var("HOME") {
            dirs.push(PathBuf::from(home).join(".vst"));
        }
    }
    dirs
}

/// Discovers plugin bundles without loading them.
pub fn scan_plugins(extra: &[PathBuf]) -> Vec<PluginDescriptor> {
    let mut found = Vec::new();
    let mut seen = HashSet::new();
    for dir in vst3_directories() {
        collect_vst3(&dir, 0, &mut found, &mut seen);
    }
    for dir in vst2_directories() {
        collect_vst2(&dir, 0, &mut found, &mut seen);
    }
    for dir in extra {
        collect_vst3(dir, 0, &mut found, &mut seen);
        collect_vst2(dir, 0, &mut found, &mut seen);
    }
    found.sort_by(|a, b| {
        a.name
            .to_ascii_lowercase()
            .cmp(&b.name.to_ascii_lowercase())
    });
    found
}

fn collect_vst3(
    dir: &Path,
    depth: usize,
    out: &mut Vec<PluginDescriptor>,
    seen: &mut HashSet<String>,
) {
    if depth > 3 {
        return;
    }
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.extension().and_then(|e| e.to_str()) == Some("vst3") {
            let key = path.to_string_lossy().to_string();
            if seen.insert(key.clone()) {
                out.push(PluginDescriptor {
                    format: PluginFormat::Vst3,
                    path: key,
                    name: display_name(&path),
                    vendor: String::new(),
                });
            }
        } else if path.is_dir() {
            collect_vst3(&path, depth + 1, out, seen);
        }
    }
}

fn collect_vst2(
    dir: &Path,
    depth: usize,
    out: &mut Vec<PluginDescriptor>,
    seen: &mut HashSet<String>,
) {
    if depth > 3 {
        return;
    }
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    let extension = if cfg!(windows) { "dll" } else { "so" };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            collect_vst2(&path, depth + 1, out, seen);
        } else if path.extension().and_then(|e| e.to_str()) == Some(extension) {
            let key = path.to_string_lossy().to_string();
            if seen.insert(key.clone()) {
                out.push(PluginDescriptor {
                    format: PluginFormat::Vst2,
                    path: key,
                    name: display_name(&path),
                    vendor: String::new(),
                });
            }
        }
    }
}

fn display_name(path: &Path) -> String {
    path.file_stem()
        .map(|s| s.to_string_lossy().to_string())
        .unwrap_or_else(|| path.to_string_lossy().to_string())
}

// ---------------------------------------------------------------------------
// Host
// ---------------------------------------------------------------------------

pub struct PluginHost {
    sample_rate: f64,
    block_size: usize,
    vst3_host: Option<vst3_host::Vst3Host>,
    instances: HashMap<u64, HostedPlugin>,
    failed: HashSet<u64>,
    errors: Vec<String>,
}

impl PluginHost {
    pub fn new(sample_rate: f64, block_size: usize) -> Self {
        Self {
            sample_rate,
            block_size,
            vst3_host: None,
            instances: HashMap::new(),
            failed: HashSet::new(),
            errors: Vec::new(),
        }
    }

    pub fn sync(&mut self, requests: Vec<PluginRequest>) -> HashMap<u64, Vec<PluginParamInfo>> {
        let wanted: HashSet<u64> = requests.iter().map(|r| r.key).collect();
        self.instances.retain(|key, _| wanted.contains(key));
        self.failed.retain(|key| wanted.contains(key));

        for request in requests {
            if self.instances.contains_key(&request.key) || self.failed.contains(&request.key) {
                continue;
            }
            match self.load(&request.spec) {
                Ok(instance) => {
                    self.instances.insert(request.key, instance);
                }
                Err(error) => {
                    self.failed.insert(request.key);
                    self.errors.push(format!(
                        "{} could not be loaded: {error}",
                        request.spec.name
                    ));
                }
            }
        }

        self.instances
            .iter()
            .map(|(key, plugin)| (*key, plugin.params.clone()))
            .collect()
    }

    fn load(&mut self, spec: &PluginSpec) -> Result<HostedPlugin, String> {
        match spec.format {
            PluginFormat::Vst3 => {
                if self.vst3_host.is_none() {
                    let host = vst3_host::Vst3Host::builder()
                        .sample_rate(self.sample_rate)
                        .block_size(self.block_size)
                        .build()
                        .map_err(|e| e.to_string())?;
                    self.vst3_host = Some(host);
                }
                let host = self.vst3_host.as_mut().ok_or("VST3 host unavailable")?;
                let mut plugin = match &spec.class_id {
                    Some(class_id) => host
                        .load_plugin_class(&spec.path, class_id)
                        .map_err(|e| e.to_string())?,
                    None => host.load_plugin(&spec.path).map_err(|e| e.to_string())?,
                };
                plugin.start_processing().map_err(|e| e.to_string())?;
                for param in &spec.params {
                    let _ = plugin.set_parameter(param.id, param.value);
                }
                let params = plugin
                    .get_parameters()
                    .map_err(|e| e.to_string())?
                    .into_iter()
                    .map(|param| PluginParamInfo {
                        id: param.id,
                        name: param.name,
                        value: param.value,
                        default: param.default,
                        unit: param.unit,
                        step_count: param.step_count,
                        read_only: param.is_read_only,
                    })
                    .collect();
                Ok(HostedPlugin {
                    inner: PluginInner::Vst3(Vst3Slot {
                        plugin,
                        buffers: vst3_host::AudioBuffers::new(
                            2,
                            2,
                            self.block_size,
                            self.sample_rate,
                        ),
                    }),
                    params,
                })
            }
            PluginFormat::Vst2 => {
                let instance = unsafe { Vst2Slot::load(spec, self.sample_rate, self.block_size)? };
                let params = unsafe { instance.read_params() };
                Ok(HostedPlugin {
                    inner: PluginInner::Vst2(instance),
                    params,
                })
            }
        }
    }

    pub fn process(&mut self, key: u64, left: &mut [f32], right: &mut [f32]) {
        if let Some(plugin) = self.instances.get_mut(&key) {
            if !plugin.process(left, right) {
                // Keep the slot but stop calling a plugin that failed.
                self.failed.insert(key);
            }
        }
    }

    pub fn note_on(&mut self, key: u64, note: u8, velocity: u8, offset: usize) {
        if let Some(plugin) = self.instances.get_mut(&key) {
            plugin.note_on(note, velocity, offset);
        }
    }

    pub fn note_off(&mut self, key: u64, note: u8, offset: usize) {
        if let Some(plugin) = self.instances.get_mut(&key) {
            plugin.note_off(note, offset);
        }
    }

    pub fn all_notes_off(&mut self) {
        for plugin in self.instances.values_mut() {
            plugin.all_notes_off();
        }
    }

    pub fn set_param(&mut self, key: u64, param: u32, value: f64) -> bool {
        match self.instances.get_mut(&key) {
            Some(plugin) => {
                plugin.set_param(param, value);
                if let Some(info) = plugin.params.iter_mut().find(|p| p.id == param) {
                    info.value = value;
                }
                true
            }
            None => false,
        }
    }

    pub fn params(&self, key: u64) -> Option<&[PluginParamInfo]> {
        self.instances.get(&key).map(|p| p.params.as_slice())
    }

    pub fn take_errors(&mut self) -> Vec<String> {
        std::mem::take(&mut self.errors)
    }

    pub fn has(&self, key: u64) -> bool {
        self.instances.contains_key(&key)
    }
}

struct HostedPlugin {
    inner: PluginInner,
    params: Vec<PluginParamInfo>,
}

enum PluginInner {
    Vst3(Vst3Slot),
    Vst2(Vst2Slot),
}

impl HostedPlugin {
    fn process(&mut self, left: &mut [f32], right: &mut [f32]) -> bool {
        match &mut self.inner {
            PluginInner::Vst3(slot) => slot.process(left, right),
            PluginInner::Vst2(slot) => unsafe { slot.process(left, right) },
        }
    }

    fn note_on(&mut self, note: u8, velocity: u8, offset: usize) {
        match &mut self.inner {
            PluginInner::Vst3(slot) => {
                let _ =
                    slot.plugin
                        .send_midi_note(note, velocity, vst3_host::midi::MidiChannel::Ch1);
                let _ = offset;
            }
            PluginInner::Vst2(slot) => unsafe { slot.send_midi([0x90, note, velocity], offset) },
        }
    }

    fn note_off(&mut self, note: u8, offset: usize) {
        match &mut self.inner {
            PluginInner::Vst3(slot) => {
                let _ = slot
                    .plugin
                    .send_midi_note_off(note, vst3_host::midi::MidiChannel::Ch1);
                let _ = offset;
            }
            PluginInner::Vst2(slot) => unsafe { slot.send_midi([0x80, note, 0], offset) },
        }
    }

    fn all_notes_off(&mut self) {
        match &mut self.inner {
            PluginInner::Vst3(slot) => {
                let _ = slot
                    .plugin
                    .send_midi_cc(123, 0, vst3_host::midi::MidiChannel::Ch1);
            }
            PluginInner::Vst2(slot) => unsafe {
                for note in 0..128u8 {
                    slot.send_midi([0x80, note, 0], 0);
                }
            },
        }
    }

    fn set_param(&mut self, param: u32, value: f64) {
        match &mut self.inner {
            PluginInner::Vst3(slot) => {
                let _ = slot.plugin.set_parameter(param, value);
            }
            PluginInner::Vst2(slot) => unsafe {
                if let Some(set) = slot.set_parameter {
                    set(slot.effect, param as i32, value.clamp(0.0, 1.0) as f32);
                }
            },
        }
    }
}

struct Vst3Slot {
    plugin: vst3_host::Plugin,
    buffers: vst3_host::AudioBuffers,
}

impl Vst3Slot {
    fn process(&mut self, left: &mut [f32], right: &mut [f32]) -> bool {
        let frames = left.len().min(right.len()).min(self.buffers.block_size);
        for channel in 0..self.buffers.inputs.len().min(2) {
            let source = if channel == 0 { &*left } else { &*right };
            let target = &mut self.buffers.inputs[channel];
            let n = frames.min(source.len()).min(target.len());
            target[..n].copy_from_slice(&source[..n]);
            for sample in &mut target[n..] {
                *sample = 0.0;
            }
        }
        for output in &mut self.buffers.outputs {
            output.fill(0.0);
        }
        if self.plugin.process_audio(&mut self.buffers).is_err() {
            return false;
        }
        let out_l = &self.buffers.outputs[0];
        let out_r = if self.buffers.outputs.len() > 1 {
            &self.buffers.outputs[1]
        } else {
            &self.buffers.outputs[0]
        };
        left[..frames].copy_from_slice(&out_l[..frames]);
        right[..frames].copy_from_slice(&out_r[..frames]);
        true
    }
}

// VST2 opcodes
const EFF_OPEN: i32 = 0;
const EFF_CLOSE: i32 = 1;
const EFF_SET_SAMPLE_RATE: i32 = 10;
const EFF_SET_BLOCK_SIZE: i32 = 11;
const EFF_MAINS_CHANGED: i32 = 12;
const EFF_PROCESS_EVENTS: i32 = 25;
const EFF_GET_PARAM_LABEL: i32 = 6;
const EFF_GET_PARAM_NAME: i32 = 8;

const AUDIO_MASTER_VERSION: i32 = 1;
const AUDIO_MASTER_GET_SAMPLE_RATE: i32 = 3;
const AUDIO_MASTER_GET_BLOCK_SIZE: i32 = 4;
const AUDIO_MASTER_GET_CURRENT_PROCESS_LEVEL: i32 = 23;

static VST2_SAMPLE_RATE: AtomicU32 = AtomicU32::new(48_000);
static VST2_BLOCK_SIZE: AtomicU32 = AtomicU32::new(64);

extern "C" fn vst2_host_callback(
    _effect: *mut AEffect,
    opcode: i32,
    _index: i32,
    _value: isize,
    _ptr: *mut c_void,
    _opt: f32,
) -> isize {
    match opcode {
        AUDIO_MASTER_VERSION => 2400,
        AUDIO_MASTER_GET_SAMPLE_RATE => VST2_SAMPLE_RATE.load(Ordering::Relaxed) as isize,
        AUDIO_MASTER_GET_BLOCK_SIZE => VST2_BLOCK_SIZE.load(Ordering::Relaxed) as isize,
        AUDIO_MASTER_GET_CURRENT_PROCESS_LEVEL => 2,
        _ => 0,
    }
}

#[repr(C)]
#[derive(Default)]
struct VstMidiEvent {
    event_type: i32,
    byte_size: i32,
    delta_frames: i32,
    flags: i32,
    note_length: i32,
    note_offset: i32,
    midi_data: [i8; 4],
    detune: i8,
    note_off_velocity: i8,
    reserved1: i8,
    reserved2: i8,
}

#[repr(C)]
struct VstEvents {
    num_events: i32,
    reserved: isize,
    events: [*mut VstMidiEvent; 2],
}

struct Vst2Slot {
    _library: libloading::Library,
    effect: *mut AEffect,
    process: extern "C" fn(*mut AEffect, *const *const f32, *mut *mut f32, i32),
    set_parameter: Option<extern "C" fn(*mut AEffect, i32, f32)>,
    get_parameter: Option<extern "C" fn(*mut AEffect, i32) -> f32>,
    dispatcher: extern "C" fn(*mut AEffect, i32, i32, isize, *mut c_void, f32) -> isize,
    out_l: Vec<f32>,
    out_r: Vec<f32>,
    num_params: i32,
    block_size: usize,
}

impl Vst2Slot {
    unsafe fn load(spec: &PluginSpec, sample_rate: f64, block_size: usize) -> Result<Self, String> {
        VST2_SAMPLE_RATE.store(sample_rate as u32, Ordering::Relaxed);
        VST2_BLOCK_SIZE.store(block_size as u32, Ordering::Relaxed);
        let library =
            libloading::Library::new(&spec.path).map_err(|e| format!("open failed: {e}"))?;
        type Entry = unsafe extern "C" fn(
            extern "C" fn(*mut AEffect, i32, i32, isize, *mut c_void, f32) -> isize,
        ) -> *mut AEffect;
        let main: libloading::Symbol<Entry> = library
            .get(b"VSTPluginMain\0")
            .or_else(|_| library.get(b"main\0"))
            .map_err(|e| format!("no VSTPluginMain: {e}"))?;
        let effect = main(vst2_host_callback);
        if effect.is_null() {
            return Err("VSTPluginMain returned null".to_string());
        }
        let dispatcher = (*effect).dispatcher;
        dispatcher(effect, EFF_OPEN, 0, 0, std::ptr::null_mut(), 0.0);
        dispatcher(
            effect,
            EFF_SET_SAMPLE_RATE,
            0,
            0,
            std::ptr::null_mut(),
            sample_rate as f32,
        );
        dispatcher(
            effect,
            EFF_SET_BLOCK_SIZE,
            0,
            0,
            std::ptr::null_mut(),
            block_size as f32,
        );
        dispatcher(effect, EFF_MAINS_CHANGED, 0, 1, std::ptr::null_mut(), 0.0);
        Ok(Self {
            _library: library,
            effect,
            process: (*effect).process,
            set_parameter: Some((*effect).set_parameter),
            get_parameter: Some((*effect).get_parameter),
            dispatcher,
            out_l: vec![0.0; block_size],
            out_r: vec![0.0; block_size],
            num_params: (*effect).num_params,
            block_size,
        })
    }

    unsafe fn read_params(&self) -> Vec<PluginParamInfo> {
        let mut params = Vec::new();
        let get = match self.get_parameter {
            Some(get) => get,
            None => return params,
        };
        for id in 0..self.num_params.max(0) {
            let name = self.read_string(EFF_GET_PARAM_NAME, id);
            let unit = self.read_string(EFF_GET_PARAM_LABEL, id);
            let value = get(self.effect, id) as f64;
            params.push(PluginParamInfo {
                id: id as u32,
                name: if name.is_empty() {
                    format!("Param {}", id + 1)
                } else {
                    name
                },
                value,
                default: 0.0,
                unit,
                step_count: 0,
                read_only: false,
            });
        }
        params
    }

    unsafe fn read_string(&self, opcode: i32, index: i32) -> String {
        let mut buffer = [0i8; 128];
        let written = (self.dispatcher)(
            self.effect,
            opcode,
            index,
            0,
            buffer.as_mut_ptr() as *mut c_void,
            0.0,
        );
        if written <= 0 {
            return String::new();
        }
        CStr::from_ptr(buffer.as_ptr())
            .to_string_lossy()
            .to_string()
    }

    unsafe fn process(&mut self, left: &mut [f32], right: &mut [f32]) -> bool {
        let frames = left.len().min(right.len()).min(self.block_size);
        self.out_l[..frames].fill(0.0);
        self.out_r[..frames].fill(0.0);
        let inputs: [*const f32; 2] = [left.as_ptr(), right.as_ptr()];
        let mut outputs: [*mut f32; 2] = [self.out_l.as_mut_ptr(), self.out_r.as_mut_ptr()];
        (self.process)(
            self.effect,
            inputs.as_ptr(),
            outputs.as_mut_ptr(),
            frames as i32,
        );
        left[..frames].copy_from_slice(&self.out_l[..frames]);
        right[..frames].copy_from_slice(&self.out_r[..frames]);
        true
    }

    unsafe fn send_midi(&mut self, bytes: [u8; 3], offset: usize) {
        let mut event = VstMidiEvent {
            event_type: 1,
            byte_size: std::mem::size_of::<VstMidiEvent>() as i32,
            delta_frames: offset as i32,
            midi_data: [bytes[0] as i8, bytes[1] as i8, bytes[2] as i8, 0],
            ..Default::default()
        };
        let mut events = VstEvents {
            num_events: 1,
            reserved: 0,
            events: [&mut event, std::ptr::null_mut()],
        };
        (self.dispatcher)(
            self.effect,
            EFF_PROCESS_EVENTS,
            0,
            0,
            &mut events as *mut VstEvents as *mut c_void,
            0.0,
        );
    }
}

impl Drop for Vst2Slot {
    fn drop(&mut self) {
        unsafe {
            (self.dispatcher)(
                self.effect,
                EFF_MAINS_CHANGED,
                0,
                0,
                std::ptr::null_mut(),
                0.0,
            );
            (self.dispatcher)(self.effect, EFF_CLOSE, 0, 0, std::ptr::null_mut(), 0.0);
        }
    }
}

#[allow(dead_code)]
fn c_string(bytes: &[u8]) -> String {
    let end = bytes.iter().position(|b| *b == 0).unwrap_or(bytes.len());
    String::from_utf8_lossy(&bytes[..end]).to_string()
}

#[allow(dead_code)]
fn c_char_ptr_eq(a: *const c_char, b: *const c_char) -> bool {
    a == b
}

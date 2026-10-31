//! Audio device enumeration and stream management.
//!
//! WASAPI (Windows default), ALSA (Linux) and CoreAudio (macOS) come from
//! CPAL. ASIO is exposed on Windows when the engine is built with the `asio`
//! feature and the Steinberg SDK is available at build time.

use std::cell::RefCell;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use cpal::{BufferSize, FromSample, SampleFormat, SizedSample, SupportedBufferSize};

use daw_core::TrackId;

use crate::engine::{Engine, EngineCommand, EngineHandle, SharedState};
use crate::snapshot::Snapshot;

#[derive(Debug, Clone)]
pub struct HostInfo {
    pub id: String,
    pub name: String,
    pub is_asio: bool,
    pub available: bool,
}

#[derive(Debug, Clone)]
pub struct DeviceInfo {
    pub name: String,
    pub is_default: bool,
    pub channels: u16,
    pub sample_rates: Vec<u32>,
}

#[derive(Debug, Clone, Default)]
pub struct DeviceSelection {
    pub host: String,
    pub device: String,
    pub sample_rate: Option<u32>,
    pub buffer_frames: Option<u32>,
}

pub fn hosts() -> Vec<HostInfo> {
    let mut out = Vec::new();
    for id in cpal::available_hosts() {
        let name = id.name().to_string();
        let is_asio = name.eq_ignore_ascii_case("asio");
        out.push(HostInfo {
            id: name.clone(),
            name: if is_asio {
                "ASIO (low latency)".to_string()
            } else {
                name.clone()
            },
            is_asio,
            available: cpal::host_from_id(id).is_ok(),
        });
    }
    if out.is_empty() {
        out.push(HostInfo {
            id: "default".to_string(),
            name: "Default device".to_string(),
            is_asio: false,
            available: true,
        });
    }
    out.sort_by(|a, b| b.is_asio.cmp(&a.is_asio));
    out
}

pub fn output_devices(host_id: &str) -> Vec<DeviceInfo> {
    let host = match find_host(host_id) {
        Some(host) => host,
        None => return Vec::new(),
    };
    let default = host.default_output_device().map(|d| d.to_string());
    let Ok(devices) = host.output_devices() else {
        return Vec::new();
    };
    let mut out = Vec::new();
    for device in devices {
        let name = device.to_string();
        let (channels, sample_rates) = match device.supported_output_configs() {
            Ok(configs) => {
                let mut channels = 0u16;
                let mut rates = Vec::new();
                for config in configs {
                    channels = channels.max(config.channels());
                    let min = config.min_sample_rate();
                    let max = config.max_sample_rate();
                    for rate in [44_100, 48_000, 88_200, 96_000, 192_000] {
                        if rate >= min && rate <= max && !rates.contains(&rate) {
                            rates.push(rate);
                        }
                    }
                }
                rates.sort_unstable();
                (channels, rates)
            }
            Err(_) => (2, vec![44_100, 48_000]),
        };
        out.push(DeviceInfo {
            is_default: default.as_deref() == Some(name.as_str()),
            name,
            channels,
            sample_rates,
        });
    }
    out.sort_by(|a, b| b.is_default.cmp(&a.is_default).then(a.name.cmp(&b.name)));
    out
}

fn find_host(host_id: &str) -> Option<cpal::Host> {
    if host_id.is_empty() || host_id == "default" {
        return Some(cpal::default_host());
    }
    cpal::available_hosts()
        .into_iter()
        .find(|id| id.name().eq_ignore_ascii_case(host_id))
        .and_then(|id| cpal::host_from_id(id).ok())
}

pub struct AudioSession {
    pub handle: EngineHandle,
    pub shared: Arc<SharedState>,
    pub sample_rate: u32,
    pub buffer_frames: u32,
    pub host_name: String,
    pub device_name: String,
    _stream: cpal::Stream,
}

static GENERATION: AtomicU64 = AtomicU64::new(1);

thread_local! {
    static ENGINES: RefCell<Option<(u64, Engine)>> = const { RefCell::new(None) };
}

#[derive(Clone)]
struct StreamState {
    generation: u64,
    sample_rate: u32,
    snapshot: Arc<Snapshot>,
    shared: Arc<SharedState>,
    commands: crossbeam_channel::Receiver<EngineCommand>,
}

pub fn start_output(
    selection: &DeviceSelection,
    midi_target: TrackId,
) -> Result<AudioSession, String> {
    let host = find_host(&selection.host).ok_or_else(|| "Audio host not available".to_string())?;
    let device = if selection.device.is_empty() {
        host.default_output_device()
            .ok_or_else(|| "No default output device".to_string())?
    } else {
        host.output_devices()
            .map_err(|e| e.to_string())?
            .find(|d| d.to_string() == selection.device)
            .or_else(|| host.default_output_device())
            .ok_or_else(|| "Output device not found".to_string())?
    };
    let device_name = device.to_string();
    let host_name = host.id().name().to_string();

    let mut ranges: Vec<_> = device
        .supported_output_configs()
        .map_err(|e| e.to_string())?
        .collect();
    if ranges.is_empty() {
        return Err("This device exposes no output configurations".to_string());
    }
    ranges.sort_by_key(|range| {
        let format_rank = match range.sample_format() {
            SampleFormat::F32 => 0,
            SampleFormat::I32 => 1,
            SampleFormat::I16 => 1,
            _ => 2,
        };
        let channel_rank = if range.channels() == 2 { 0 } else { 1 };
        (channel_rank, format_rank)
    });
    let range = ranges[0];
    let requested = selection.sample_rate.unwrap_or(48_000);
    let sample_rate = requested
        .max(range.min_sample_rate())
        .min(range.max_sample_rate());
    let supported = range.with_sample_rate(sample_rate);
    let sample_format = supported.sample_format();
    let mut config: cpal::StreamConfig = supported.into();
    let buffer_frames = match range.buffer_size() {
        SupportedBufferSize::Range { min, max } => {
            let wanted = selection.buffer_frames.unwrap_or(256).clamp(32, 4_096);
            let frames = wanted.max(*min).min(*max);
            config.buffer_size = BufferSize::Fixed(frames);
            frames
        }
        SupportedBufferSize::Unknown => {
            config.buffer_size = BufferSize::Default;
            selection.buffer_frames.unwrap_or(0)
        }
    };

    let (tx, rx) = crossbeam_channel::unbounded();
    let shared = SharedState::new();
    shared.sample_rate.store(sample_rate, Ordering::Relaxed);
    shared.midi_target.store(midi_target.0, Ordering::Relaxed);
    let handle = EngineHandle::new(tx, shared.clone(), sample_rate);
    let state = StreamState {
        generation: GENERATION.fetch_add(1, Ordering::Relaxed),
        sample_rate,
        snapshot: Snapshot::empty(sample_rate),
        shared: shared.clone(),
        commands: rx,
    };
    let channels = config.channels as usize;

    let error_fn = |error| log::error!("audio stream error: {error}");
    let stream = match sample_format {
        SampleFormat::F32 => build_stream::<f32>(&device, config.clone(), state, channels, error_fn)?,
        SampleFormat::I16 => build_stream::<i16>(&device, config.clone(), state, channels, error_fn)?,
        SampleFormat::I32 => build_stream::<i32>(&device, config.clone(), state, channels, error_fn)?,
        other => {
            return Err(format!(
                "The device only offers {other:?} samples; Spatial DAW needs a 16/32-bit or float output"
            ))
        }
    };
    stream.play().map_err(|e| e.to_string())?;

    Ok(AudioSession {
        handle,
        shared,
        sample_rate,
        buffer_frames,
        host_name,
        device_name,
        _stream: stream,
    })
}

fn build_stream<T>(
    device: &cpal::Device,
    config: cpal::StreamConfig,
    state: StreamState,
    channels: usize,
    error_fn: impl FnMut(cpal::Error) + Send + 'static,
) -> Result<cpal::Stream, String>
where
    T: SizedSample + FromSample<f32> + 'static,
{
    let mut scratch: Vec<f32> = vec![0.0; channels * 512];
    let state = state.clone();
    let callback = move |data: &mut [T], _info: &cpal::OutputCallbackInfo| {
        if scratch.len() != data.len() {
            scratch.resize(data.len(), 0.0);
        }
        ENGINES.with(|cell| {
            let mut slot = cell.borrow_mut();
            let needs_new =
                !matches!(&*slot, Some((generation, _)) if *generation == state.generation);
            if needs_new {
                *slot = Some((
                    state.generation,
                    Engine::new(
                        state.sample_rate,
                        state.snapshot.clone(),
                        state.shared.clone(),
                        state.commands.clone(),
                    ),
                ));
            }
            if let Some((_, engine)) = slot.as_mut() {
                engine.process_interleaved(&mut scratch, channels);
            }
        });
        for (out, sample) in data.iter_mut().zip(scratch.iter()) {
            *out = T::from_sample(*sample);
        }
    };
    device
        .build_output_stream(config, callback, error_fn, None)
        .map_err(|e| e.to_string())
}

//! In-memory media pool and waveform peak cache.
//!
//! Audio buffers are always stored as interleaved stereo `f32` so the engine
//! never has to branch on channel count while rendering. Source files keep
//! their native sample rate; the engine resamples them to the device rate.

use std::collections::BTreeMap;
use std::sync::Arc;

/// Frames per waveform peak bucket.
pub const PEAK_WINDOW: usize = 256;

/// One min/max pair used to draw a waveform column.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct Peak {
    pub min: f32,
    pub max: f32,
}

impl Peak {
    pub fn merge(&mut self, other: Peak) {
        self.min = self.min.min(other.min);
        self.max = self.max.max(other.max);
    }

    pub fn amplitude(&self) -> f32 {
        self.max.max(-self.min)
    }
}

#[derive(Debug, Clone)]
pub struct MediaItem {
    /// File name used inside the project's media folder, e.g. `vox.wav`.
    pub key: String,
    pub name: String,
    pub sample_rate: u32,
    /// Stereo interleaved samples.
    pub samples: Arc<[f32]>,
    pub frames: usize,
    pub peaks: Arc<[Peak]>,
    pub source_path: Option<String>,
}

impl MediaItem {
    /// Builds a stereo item from any channel layout (mono is duplicated,
    /// surround takes the first two channels).
    pub fn new(
        key: impl Into<String>,
        name: impl Into<String>,
        sample_rate: u32,
        channels: u16,
        raw: &[f32],
        source_path: Option<String>,
    ) -> Self {
        let channels = channels.max(1) as usize;
        let frames = raw.len() / channels;
        let mut samples = Vec::with_capacity(frames * 2);
        for frame in 0..frames {
            let base = frame * channels;
            if channels == 1 {
                let s = raw[base];
                samples.push(s);
                samples.push(s);
            } else {
                samples.push(raw[base]);
                samples.push(raw[base + 1]);
            }
        }
        let peaks = compute_peaks(&samples);
        Self {
            key: key.into(),
            name: name.into(),
            sample_rate: sample_rate.max(1),
            frames,
            samples: samples.into(),
            peaks: peaks.into(),
            source_path,
        }
    }

    pub fn silent(key: impl Into<String>, sample_rate: u32) -> Self {
        let name = key.into();
        Self {
            key: name.clone(),
            name,
            sample_rate: sample_rate.max(1),
            samples: vec![0.0; 2_048].into(),
            frames: 1_024,
            peaks: vec![Peak::default()].into(),
            source_path: None,
        }
    }

    pub fn duration_seconds(&self) -> f32 {
        self.frames as f32 / self.sample_rate.max(1) as f32
    }

    pub fn sample(&self, frame: i64, channel: usize) -> f32 {
        if frame < 0 || frame as usize >= self.frames {
            return 0.0;
        }
        self.samples[(frame as usize) * 2 + channel.min(1)]
    }

    /// Resamples this item to `target_rate` using a windowed-sinc kernel.
    pub fn resampled(&self, target_rate: u32) -> Arc<[f32]> {
        resample_stereo(&self.samples, self.sample_rate, target_rate)
    }

    /// Aggregates peaks in a frame range into `buckets` columns. Used by the
    /// arranger; the result is in play order and already reversed if needed.
    pub fn peak_envelope(&self, start_frame: i64, length_frames: i64, buckets: usize) -> Vec<Peak> {
        let buckets = buckets.max(1);
        let mut out = vec![Peak::default(); buckets];
        if length_frames <= 0 || self.frames == 0 {
            return out;
        }
        let start = start_frame.max(0) as usize;
        let end = (start_frame + length_frames).clamp(0, self.frames as i64) as usize;
        if start >= end {
            return out;
        }
        let first_peak = start / PEAK_WINDOW;
        let last_peak = (end / PEAK_WINDOW).min(self.peaks.len().saturating_sub(1));
        if first_peak > last_peak {
            return out;
        }
        let span = (last_peak - first_peak + 1) as f32;
        for (i, bucket) in out.iter_mut().enumerate() {
            let a = first_peak + ((i as f32 / buckets as f32) * span) as usize;
            let b = first_peak + (((i + 1) as f32 / buckets as f32) * span) as usize;
            let b = b.min(last_peak + 1).max(a + 1);
            let mut peak = Peak::default();
            for index in a..b {
                peak.merge(self.peaks[index.min(self.peaks.len() - 1)]);
            }
            *bucket = peak;
        }
        out
    }
}

/// Min/max envelope of interleaved stereo samples.
pub fn compute_peaks(stereo: &[f32]) -> Vec<Peak> {
    let frames = stereo.len() / 2;
    let mut peaks = Vec::with_capacity(frames.div_ceil(PEAK_WINDOW).max(1));
    let mut index = 0;
    while index < frames {
        let end = (index + PEAK_WINDOW).min(frames);
        let mut peak = Peak { min: 0.0, max: 0.0 };
        for frame in index..end {
            let l = stereo[frame * 2];
            let r = stereo[frame * 2 + 1];
            peak.min = peak.min.min(l).min(r);
            peak.max = peak.max.max(l).max(r);
        }
        peaks.push(peak);
        index = end;
    }
    if peaks.is_empty() {
        peaks.push(Peak::default());
    }
    peaks
}

/// Stereo windowed-sinc resampler. Good enough for sample-rate conversion of
/// imported audio, with a 16-tap Blackman window and linear phase.
pub fn resample_stereo(input: &[f32], from: u32, to: u32) -> Arc<[f32]> {
    if from == to || from == 0 || to == 0 {
        return Arc::from(input.to_vec());
    }
    let frames_in = input.len() / 2;
    if frames_in == 0 {
        return Arc::from(Vec::<f32>::new());
    }
    let ratio = to as f64 / from as f64;
    let frames_out = ((frames_in as f64) * ratio).ceil() as usize + 1;
    let mut out = vec![0.0f32; frames_out * 2];
    let cutoff = if ratio < 1.0 { ratio } else { 1.0 } * 0.94;
    let half_taps = 8i64;
    let inv_ratio = from as f64 / to as f64;

    for frame_out in 0..frames_out {
        let src_pos = frame_out as f64 * inv_ratio;
        let base = src_pos.floor() as i64;
        let frac = src_pos - base as f64;
        let mut acc_l = 0.0f64;
        let mut acc_r = 0.0f64;
        let mut weight_sum = 0.0f64;
        for tap in -half_taps..=half_taps {
            let index = base + tap;
            if index < 0 || index >= frames_in as i64 {
                continue;
            }
            let distance = frac - tap as f64;
            let x = distance * cutoff;
            let sinc = if x.abs() < 1e-9 {
                1.0
            } else {
                (std::f64::consts::PI * x).sin() / (std::f64::consts::PI * x)
            };
            // Blackman window over the tap span.
            let phase = (distance / (half_taps as f64 + 1.0) + 1.0) * 0.5;
            let window = if (0.0..=1.0).contains(&phase) {
                0.42 - 0.5 * (2.0 * std::f64::consts::PI * phase).cos()
                    + 0.08 * (4.0 * std::f64::consts::PI * phase).cos()
            } else {
                0.0
            };
            let w = sinc * window;
            acc_l += input[index as usize * 2] as f64 * w;
            acc_r += input[index as usize * 2 + 1] as f64 * w;
            weight_sum += w;
        }
        if weight_sum.abs() > 1e-9 {
            acc_l /= weight_sum;
            acc_r /= weight_sum;
        }
        out[frame_out * 2] = acc_l as f32;
        out[frame_out * 2 + 1] = acc_r as f32;
    }
    Arc::from(out)
}

/// Collection of media items keyed by their in-project file name.
#[derive(Debug, Clone, Default)]
pub struct MediaStore {
    items: BTreeMap<String, MediaItem>,
}

impl MediaStore {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn len(&self) -> usize {
        self.items.len()
    }

    pub fn is_empty(&self) -> bool {
        self.items.is_empty()
    }

    pub fn get(&self, key: &str) -> Option<&MediaItem> {
        self.items.get(key)
    }

    pub fn iter(&self) -> impl Iterator<Item = (&String, &MediaItem)> {
        self.items.iter()
    }

    pub fn keys(&self) -> impl Iterator<Item = &String> {
        self.items.keys()
    }

    pub fn insert(&mut self, item: MediaItem) {
        self.items.insert(item.key.clone(), item);
    }

    pub fn insert_decoded(
        &mut self,
        name_hint: &str,
        sample_rate: u32,
        channels: u16,
        raw: &[f32],
        source_path: Option<String>,
    ) -> String {
        let key = self.unique_key(name_hint);
        let item = MediaItem::new(
            key.clone(),
            display_name(name_hint),
            sample_rate,
            channels,
            raw,
            source_path,
        );
        self.items.insert(key.clone(), item);
        key
    }

    pub fn remove(&mut self, key: &str) -> Option<MediaItem> {
        self.items.remove(key)
    }

    /// Produces a filesystem-safe unique key ending in `.wav`.
    pub fn unique_key(&self, name_hint: &str) -> String {
        let stem = sanitize_stem(name_hint);
        let mut key = format!("{stem}.wav");
        let mut n = 2;
        while self.items.contains_key(&key) {
            key = format!("{stem}_{n}.wav");
            n += 1;
        }
        key
    }
}

fn display_name(name_hint: &str) -> String {
    std::path::Path::new(name_hint)
        .file_stem()
        .map(|s| s.to_string_lossy().to_string())
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| name_hint.to_string())
}

pub fn sanitize_stem(name_hint: &str) -> String {
    let stem = std::path::Path::new(name_hint)
        .file_stem()
        .map(|s| s.to_string_lossy().to_string())
        .unwrap_or_else(|| name_hint.to_string());
    let mut out = String::with_capacity(stem.len());
    for ch in stem.chars() {
        if ch.is_ascii_alphanumeric() || ch == '-' || ch == '_' || ch == ' ' {
            out.push(ch);
        } else {
            out.push('_');
        }
    }
    let spaced = out.trim().replace(' ', "_");
    let trimmed = spaced.trim_matches('_').to_string();
    if trimmed.is_empty() {
        "audio".to_string()
    } else {
        trimmed
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sine(frames: usize, hz: f32, sr: u32) -> Vec<f32> {
        let mut v = Vec::with_capacity(frames * 2);
        for i in 0..frames {
            let s = (i as f32 / sr as f32 * hz * std::f32::consts::TAU).sin() * 0.5;
            v.push(s);
            v.push(s);
        }
        v
    }

    #[test]
    fn mono_is_duplicated_to_stereo() {
        let item = MediaItem::new("a.wav", "a", 48_000, 1, &[0.5, -0.5], None);
        assert_eq!(item.frames, 2);
        assert_eq!(item.samples.len(), 4);
        assert_eq!(item.sample(0, 0), 0.5);
        assert_eq!(item.sample(0, 1), 0.5);
    }

    #[test]
    fn unique_keys_do_not_collide() {
        let mut store = MediaStore::new();
        store.insert(MediaItem::silent("vox.wav", 48_000));
        assert_eq!(store.unique_key("vox.wav"), "vox_2.wav");
        assert_eq!(store.unique_key("a/b*c?.wav"), "b_c.wav");
    }

    #[test]
    fn peaks_capture_extremes() {
        let mut samples = vec![0.0f32; PEAK_WINDOW * 4 * 2];
        samples[10] = 0.9;
        samples[PEAK_WINDOW * 2 * 2 + 5 * 2] = -0.7;
        let peaks = compute_peaks(&samples);
        assert_eq!(peaks.len(), 4);
        assert!(peaks[0].max >= 0.9);
        assert!(peaks[2].min <= -0.7);
        assert!(peaks[1].max <= 0.0);
    }

    #[test]
    fn resampling_keeps_length_and_level() {
        let input = sine(4_800, 440.0, 48_000);
        let out = resample_stereo(&input, 48_000, 44_100);
        let frames = out.len() / 2;
        assert!((frames as i64 - 4_410).abs() <= 2);
        let rms_in = (input.iter().map(|s| s * s).sum::<f32>() / input.len() as f32).sqrt();
        let rms_out = (out.iter().map(|s| s * s).sum::<f32>() / out.len() as f32).sqrt();
        assert!((rms_in - rms_out).abs() < 0.06);
    }

    #[test]
    fn peak_envelope_respects_range() {
        let item = MediaItem::new("a.wav", "a", 48_000, 2, &sine(48_000, 100.0, 48_000), None);
        let env = item.peak_envelope(0, 24_000, 100);
        assert_eq!(env.len(), 100);
        assert!(env.iter().any(|p| p.max > 0.1));
        assert!(env.iter().all(|p| p.min >= -1.0 && p.max <= 1.0));
    }
}

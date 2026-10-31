//! Resampled playback cache.
//!
//! Media is stored at its source sample rate so saves are lossless. The engine
//! runs at whatever rate the interface was opened at, so each item is resampled
//! once and reused across snapshot rebuilds.

use std::collections::HashMap;
use std::sync::Arc;

use daw_core::MediaStore;

#[derive(Debug, Clone)]
pub struct PlaybackCache {
    sample_rate: u32,
    buffers: HashMap<String, Arc<[f32]>>,
}

impl PlaybackCache {
    pub fn new(sample_rate: u32) -> Self {
        Self {
            sample_rate: sample_rate.max(1),
            buffers: HashMap::new(),
        }
    }

    pub fn sample_rate(&self) -> u32 {
        self.sample_rate
    }

    pub fn set_sample_rate(&mut self, sample_rate: u32) {
        if self.sample_rate != sample_rate {
            self.sample_rate = sample_rate.max(1);
            self.buffers.clear();
        }
    }

    /// Returns the item's stereo buffer at the cache rate, resampling on first use.
    pub fn buffer(&mut self, key: &str, media: &MediaStore) -> Option<Arc<[f32]>> {
        if let Some(buffer) = self.buffers.get(key) {
            return Some(buffer.clone());
        }
        let item = media.get(key)?;
        let buffer = if item.sample_rate == self.sample_rate {
            item.samples.clone()
        } else {
            item.resampled(self.sample_rate)
        };
        self.buffers.insert(key.to_string(), buffer.clone());
        Some(buffer)
    }

    /// Reference-counted lookup without importing.
    pub fn peek(&self, key: &str) -> Option<Arc<[f32]>> {
        self.buffers.get(key).cloned()
    }

    pub fn prune(&mut self, media: &MediaStore) {
        self.buffers.retain(|key, _| media.get(key).is_some());
    }

    pub fn clear(&mut self) {
        self.buffers.clear();
    }

    pub fn len(&self) -> usize {
        self.buffers.len()
    }

    pub fn is_empty(&self) -> bool {
        self.buffers.is_empty()
    }
}

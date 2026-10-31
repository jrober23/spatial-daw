//! WAV mixdown export.

use std::path::Path;

use hound::{SampleFormat, WavSpec, WavWriter};

use daw_core::{MediaStore, Project};

use crate::engine::render_offline;

pub struct ExportReport {
    pub frames: usize,
    pub sample_rate: u32,
}

/// Renders the arrangement and writes a 32-bit float stereo WAV.
pub fn export_mixdown(
    project: &Project,
    media: &MediaStore,
    path: &Path,
    tail_seconds: f32,
) -> Result<ExportReport, String> {
    let sample_rate = project.sample_rate.max(1);
    let samples = render_offline(project, media, sample_rate, tail_seconds.max(0.1));
    if samples.is_empty() {
        return Err("Nothing to export: the arrangement is empty".to_string());
    }
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    }
    let spec = WavSpec {
        channels: 2,
        sample_rate,
        bits_per_sample: 32,
        sample_format: SampleFormat::Float,
    };
    let mut writer = WavWriter::create(path, spec).map_err(|e| e.to_string())?;
    for sample in &samples {
        writer.write_sample(*sample).map_err(|e| e.to_string())?;
    }
    writer.finalize().map_err(|e| e.to_string())?;
    Ok(ExportReport {
        frames: samples.len() / 2,
        sample_rate,
    })
}

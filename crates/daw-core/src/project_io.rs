//! Project persistence.
//!
//! A project is one JSON file plus a sibling media folder:
//!
//! ```text
//! My Song.sdaw
//! My Song.sdaw.media/
//!     kick.wav
//!     vocal.wav
//! ```
//!
//! WAVs are written as 32-bit float stereo, so saving and reopening a project
//! is lossless for anything imported at up to 32-bit float precision. Missing
//! media never fails a load: the project opens with a warning and a silent
//! placeholder so the arrangement stays intact.

use std::fs;
use std::io::Cursor;
use std::path::{Path, PathBuf};

use hound::{SampleFormat, WavReader, WavSpec, WavWriter};
use thiserror::Error;

use crate::media::{MediaItem, MediaStore};
use crate::model::{Project, FORMAT_VERSION};

#[derive(Debug, Error)]
pub enum ProjectError {
    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),
    #[error("project file is not valid JSON: {0}")]
    Json(#[from] serde_json::Error),
    #[error("audio file error: {0}")]
    Wav(#[from] hound::Error),
    #[error("project was made with a newer version ({0}); this build supports {FORMAT_VERSION}")]
    NewerVersion(u32),
    #[error("project has no media directory")]
    MissingMediaDir,
}

pub type Result<T> = std::result::Result<T, ProjectError>;

/// A folder ending in `.media` next to the project file.
pub fn media_dir_for(project_file: &Path) -> PathBuf {
    let name = project_file
        .file_name()
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_else(|| "project.sdaw".to_string());
    project_file.with_file_name(format!("{name}.media"))
}

fn media_path(dir: &Path, key: &str) -> PathBuf {
    let safe: String = key
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '.' || c == '-' || c == '_' {
                c
            } else {
                '_'
            }
        })
        .collect();
    let safe = if safe.ends_with(".wav") {
        safe
    } else {
        format!("{safe}.wav")
    };
    dir.join(safe)
}

#[derive(Debug, Clone, Default)]
pub struct SaveReport {
    pub media_written: usize,
    pub media_skipped: Vec<String>,
}

/// Saves the project and every media item it references.
pub fn save_project(project: &Project, media: &MediaStore, path: &Path) -> Result<SaveReport> {
    if let Some(parent) = path.parent() {
        if !parent.as_os_str().is_empty() {
            fs::create_dir_all(parent)?;
        }
    }
    let dir = media_dir_for(path);
    fs::create_dir_all(&dir)?;

    let json = serde_json::to_string_pretty(project)?;
    let tmp = path.with_extension("sdaw.tmp");
    fs::write(&tmp, json.as_bytes())?;
    if path.exists() {
        let _ = fs::remove_file(path);
    }
    fs::rename(&tmp, path)?;

    let mut report = SaveReport::default();
    for key in project.media_keys() {
        match media.get(&key) {
            Some(item) => {
                let target = media_path(&dir, &key);
                if let Err(err) = write_wav_item(&target, item) {
                    log::warn!("could not write media {key}: {err}");
                    report.media_skipped.push(key);
                } else {
                    report.media_written += 1;
                }
            }
            None => {
                report.media_skipped.push(key);
            }
        }
    }
    Ok(report)
}

/// Saves only the JSON document (used for autosave).
pub fn save_project_json(project: &Project, path: &Path) -> Result<()> {
    if let Some(parent) = path.parent() {
        if !parent.as_os_str().is_empty() {
            fs::create_dir_all(parent)?;
        }
    }
    let json = serde_json::to_string_pretty(project)?;
    fs::write(path, json)?;
    Ok(())
}

/// Loads a project plus all referenced media. Returns human-readable warnings
/// for media that could not be found or decoded.
pub fn load_project(path: &Path) -> Result<(Project, MediaStore, Vec<String>)> {
    let text = fs::read_to_string(path)?;
    let mut project: Project = serde_json::from_str(&text)?;
    if project.format_version > FORMAT_VERSION {
        return Err(ProjectError::NewerVersion(project.format_version));
    }
    project.repair();

    let dir = media_dir_for(path);
    let mut media = MediaStore::new();
    let mut warnings = Vec::new();
    for key in project.media_keys() {
        let file = media_path(&dir, &key);
        if !file.exists() {
            warnings.push(format!(
                "Missing media \"{key}\" — the clip will play silence."
            ));
            media.insert(MediaItem::silent(key, project.sample_rate));
            continue;
        }
        match read_wav_item(&file, &key) {
            Ok(item) => media.insert(item),
            Err(err) => {
                warnings.push(format!("Could not read \"{key}\": {err}"));
                media.insert(MediaItem::silent(key, project.sample_rate));
            }
        }
    }
    Ok((project, media, warnings))
}

pub fn write_wav_item(path: &Path, item: &MediaItem) -> Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    let spec = WavSpec {
        channels: 2,
        sample_rate: item.sample_rate,
        bits_per_sample: 32,
        sample_format: SampleFormat::Float,
    };
    let mut writer = WavWriter::create(path, spec)?;
    for sample in item.samples.iter() {
        writer.write_sample(*sample)?;
    }
    writer.finalize()?;
    Ok(())
}

pub fn read_wav_item(path: &Path, key: &str) -> Result<MediaItem> {
    let reader = WavReader::open(path)?;
    let spec = reader.spec();
    let name = path
        .file_stem()
        .map(|s| s.to_string_lossy().to_string())
        .unwrap_or_else(|| key.to_string());
    let raw: Vec<f32> = match spec.sample_format {
        SampleFormat::Float => {
            let mut r = reader;
            r.samples::<f32>()
                .collect::<std::result::Result<Vec<_>, _>>()?
        }
        SampleFormat::Int => {
            let bits = spec.bits_per_sample;
            let mut r = reader;
            match bits {
                8 | 16 => r
                    .samples::<i16>()
                    .map(|s| s.map(|v| v as f32 / 32_768.0))
                    .collect::<std::result::Result<Vec<_>, _>>()?,
                24 | 32 => {
                    let scale = (1i64 << (bits - 1)) as f32;
                    r.samples::<i32>()
                        .map(|s| s.map(|v| v as f32 / scale))
                        .collect::<std::result::Result<Vec<_>, _>>()?
                }
                _ => {
                    return Err(ProjectError::Wav(hound::Error::Unsupported));
                }
            }
        }
    };
    Ok(MediaItem::new(
        key,
        name,
        spec.sample_rate,
        spec.channels,
        &raw,
        Some(path.to_string_lossy().to_string()),
    ))
}

/// Encodes a media item as a standalone WAV file in memory (used by "export
/// selected audio"). Kept here so the UI and engine share one writer.
pub fn wav_bytes(item: &MediaItem) -> Result<Vec<u8>> {
    let spec = WavSpec {
        channels: 2,
        sample_rate: item.sample_rate,
        bits_per_sample: 32,
        sample_format: SampleFormat::Float,
    };
    let mut buffer = Cursor::new(Vec::new());
    {
        let mut writer = WavWriter::new(&mut buffer, spec)?;
        for sample in item.samples.iter() {
            writer.write_sample(*sample)?;
        }
        writer.finalize()?;
    }
    Ok(buffer.into_inner())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::ClipKind;

    fn temp_dir(tag: &str) -> PathBuf {
        let base = std::env::temp_dir().join(format!(
            "spatial-daw-test-{}-{}-{}",
            std::process::id(),
            tag,
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::create_dir_all(&base).unwrap();
        base
    }

    #[test]
    fn project_round_trip_with_media() {
        let dir = temp_dir("roundtrip");
        let path = dir.join("Song.sdaw");
        let mut project = Project::demo("Round Trip");
        let audio = project.add_audio_track("Vocal");
        let mut samples = Vec::new();
        for i in 0..48_000 {
            let s = (i as f32 / 48_000.0 * 440.0 * std::f32::consts::TAU).sin() * 0.25;
            samples.push(s);
            samples.push(s);
        }
        let mut media = MediaStore::new();
        let key = media.insert_decoded("vocal.wav", 48_000, 2, &samples, None);
        project
            .add_audio_clip(audio, &key, "Vocal Take", 0, 48_000, 0)
            .unwrap();

        let report = save_project(&project, &media, &path).unwrap();
        assert_eq!(report.media_written, 1);
        assert!(media_dir_for(&path).join("vocal.wav").exists());

        let (loaded, loaded_media, warnings) = load_project(&path).unwrap();
        assert!(warnings.is_empty(), "{warnings:?}");
        assert_eq!(loaded.name, "Round Trip");
        assert_eq!(loaded.tracks.len(), project.tracks.len());
        let loaded_audio = loaded.tracks.iter().find(|t| t.name == "Vocal").unwrap();
        match &loaded_audio.clips[0].kind {
            ClipKind::Audio(a) => {
                assert_eq!(a.media, key);
                assert_eq!(a.length_frames, 48_000);
            }
            _ => panic!("expected audio clip"),
        }
        let item = loaded_media.get(&key).unwrap();
        assert_eq!(item.frames, 48_000);
        assert_eq!(item.sample_rate, 48_000);
        assert!((item.samples[100] - samples[100]).abs() < 1e-6);

        fs::remove_dir_all(dir).ok();
    }

    #[test]
    fn missing_media_loads_with_warning() {
        let dir = temp_dir("missing");
        let path = dir.join("Broken.sdaw");
        let mut project = Project::empty("Broken");
        let audio = project.add_audio_track("Audio");
        project
            .add_audio_clip(audio, "ghost.wav", "Ghost", 0, 1_000, 0)
            .unwrap();
        let media = MediaStore::new();
        save_project(&project, &media, &path).unwrap();
        let (loaded, store, warnings) = load_project(&path).unwrap();
        assert_eq!(warnings.len(), 1);
        assert!(store.get("ghost.wav").is_some());
        assert_eq!(loaded.tracks.len(), project.tracks.len());
        fs::remove_dir_all(dir).ok();
    }

    #[test]
    fn newer_project_version_is_rejected() {
        let dir = temp_dir("version");
        let path = dir.join("Future.sdaw");
        let mut project = Project::empty("Future");
        project.format_version = FORMAT_VERSION + 1;
        fs::write(&path, serde_json::to_string(&project).unwrap()).unwrap();
        let err = load_project(&path).unwrap_err();
        assert!(matches!(err, ProjectError::NewerVersion(_)));
        fs::remove_dir_all(dir).ok();
    }

    #[test]
    fn wav_bytes_are_valid() {
        let item = MediaItem::new("x.wav", "x", 44_100, 1, &[0.0, 0.5, -0.5, 0.0], None);
        let bytes = wav_bytes(&item).unwrap();
        let reader = WavReader::new(Cursor::new(bytes)).unwrap();
        assert_eq!(reader.spec().channels, 2);
        assert_eq!(reader.spec().sample_rate, 44_100);
    }
}

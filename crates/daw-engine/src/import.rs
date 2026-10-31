//! Audio file import through Symphonia (WAV, MP3, OGG/Vorbis and FLAC).
//!
//! Files are decoded at their native sample rate and stored in the project's
//! media pool. The playback cache resamples them to the device rate when a
//! song is opened.

use std::fs::File;
use std::path::Path;

use symphonia::core::codecs::audio::AudioDecoderOptions;
use symphonia::core::codecs::CodecParameters;
use symphonia::core::formats::probe::Hint;
use symphonia::core::formats::{FormatOptions, TrackType};
use symphonia::core::io::MediaSourceStream;
use symphonia::core::meta::MetadataOptions;

#[derive(Debug, Clone)]
pub struct DecodedAudio {
    pub sample_rate: u32,
    pub channels: u16,
    pub samples: Vec<f32>,
    pub frames: usize,
}

impl DecodedAudio {
    pub fn duration_seconds(&self) -> f32 {
        self.frames as f32 / self.sample_rate.max(1) as f32
    }
}

pub fn supported_extensions() -> &'static [&'static str] {
    &["wav", "mp3", "ogg", "flac", "aiff", "m4a", "aac"]
}

pub fn is_supported(path: &Path) -> bool {
    path.extension()
        .and_then(|ext| ext.to_str())
        .map(|ext| {
            supported_extensions()
                .iter()
                .any(|e| e.eq_ignore_ascii_case(ext))
        })
        .unwrap_or(false)
}

/// Decodes an audio file into interleaved f32 samples.
pub fn decode_file(path: &Path) -> Result<DecodedAudio, String> {
    let file = File::open(path).map_err(|e| format!("{}: {e}", path.display()))?;
    let mss = MediaSourceStream::new(Box::new(file), Default::default());
    let mut hint = Hint::new();
    if let Some(ext) = path.extension().and_then(|e| e.to_str()) {
        hint.with_extension(ext);
    }
    let mut format = symphonia::default::get_probe()
        .probe(
            &hint,
            mss,
            FormatOptions::default(),
            MetadataOptions::default(),
        )
        .map_err(|e| format!("{}: {e}", path.display()))?;

    let track = format
        .default_track(TrackType::Audio)
        .ok_or_else(|| format!("{} contains no audio track", path.display()))?;
    let track_id = track.id;
    let params = match &track.codec_params {
        Some(CodecParameters::Audio(params)) => params.clone(),
        _ => return Err("the audio track has no decodable codec parameters".to_string()),
    };
    let mut decoder = symphonia::default::get_codecs()
        .make_audio_decoder(&params, &AudioDecoderOptions::default())
        .map_err(|e| e.to_string())?;

    let mut samples: Vec<f32> = Vec::new();
    let mut sample_rate = 0u32;
    let mut channels = 0u16;

    loop {
        let packet = match format.next_packet() {
            Ok(Some(packet)) => packet,
            Ok(None) => break,
            Err(error) => {
                if samples.is_empty() {
                    return Err(format!("failed to read audio: {error}"));
                }
                log::warn!("{}: stopped at a corrupt packet: {error}", path.display());
                break;
            }
        };
        if packet.track_id != track_id {
            continue;
        }
        let decoded = match decoder.decode(&packet) {
            Ok(decoded) => decoded,
            Err(error) => {
                if samples.is_empty() {
                    return Err(format!("failed to decode audio: {error}"));
                }
                log::warn!("{}: decode stopped: {error}", path.display());
                break;
            }
        };
        let spec = decoded.spec();
        sample_rate = spec.rate();
        channels = spec.channels().count() as u16;
        let frames = decoded.frames();
        if frames == 0 || channels == 0 {
            continue;
        }
        let start = samples.len();
        samples.resize(start + frames * channels as usize, 0.0);
        decoded.copy_to_slice_interleaved::<f32, _>(&mut samples[start..]);
    }

    if samples.is_empty() || sample_rate == 0 {
        return Err("the file contains no decodable audio".to_string());
    }
    let frames = samples.len() / channels.max(1) as usize;
    Ok(DecodedAudio {
        sample_rate,
        channels,
        samples,
        frames,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use hound::{SampleFormat, WavSpec, WavWriter};

    #[test]
    fn decodes_wav_into_interleaved_f32() {
        let dir = std::env::temp_dir().join(format!(
            "sdaw-import-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("tone.wav");
        let spec = WavSpec {
            channels: 2,
            sample_rate: 44_100,
            bits_per_sample: 16,
            sample_format: SampleFormat::Int,
        };
        let mut writer = WavWriter::create(&path, spec).unwrap();
        for i in 0..4_410 {
            let value =
                ((i as f32 / 44_100.0 * 440.0 * std::f32::consts::TAU).sin() * 16_000.0) as i16;
            writer.write_sample(value).unwrap();
            writer.write_sample(value).unwrap();
        }
        writer.finalize().unwrap();

        let decoded = decode_file(&path).unwrap();
        assert_eq!(decoded.sample_rate, 44_100);
        assert_eq!(decoded.channels, 2);
        assert!((decoded.frames as i64 - 4_410).abs() <= 2);
        assert!(decoded.samples.iter().any(|s| s.abs() > 0.1));
        assert!(decoded.samples.iter().all(|s| s.abs() <= 1.0));
        assert!(is_supported(&path));
        std::fs::remove_dir_all(dir).ok();
    }

    #[test]
    fn rejects_non_audio_extension_in_ui() {
        assert!(!is_supported(Path::new("notes.txt")));
        assert!(is_supported(Path::new("song.FLAC")));
    }
}

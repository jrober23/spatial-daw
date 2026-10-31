# Spatial DAW

A Windows-first desktop DAW written in Rust with a **Gibson-inspired 3D spatial mixer**.
Arrange audio and MIDI-like patterns, mix with your eyes as well as your ears, and see
where every element sits in the stereo field, frequency spectrum, and front-to-back depth.

The interface is built with `egui`/`eframe`; the 3D mix field is rendered by `wgpu`
(offscreen texture with a real depth buffer, instanced sphere impostors, ghost targets).

## Features

- **Arrangement / track editor** — audio and instrument tracks, clip move, trim and split,
  fades, clip gain, reverse, mute/solo, loop region, snap grid, waveform drawing.
- **Step sequencer** — per-track patterns, 1/16 steps, pattern clips in the arrangement.
- **Piano roll** — draw, drag, and delete notes; per-pattern editing.
- **Instruments** — 8 drum synths, a subtractive synth (bass/keys/lead presets), and a
  sampler with root note, loop mode, reverse, pitch, and amp envelope.
- **Audio import** — WAV, MP3, OGG/Opus, FLAC, AIFF, AAC/M4A via Symphonia. Files are
  copied into the project's media folder on save.
- **Audio editing** — non-destructive clips, trims, splits, fades, clip gain, reverse.
- **Mixer** — per-track fader, pan, mute/solo, insert list, master fader and limiter,
  live RMS/peak meters.
- **3D spatial field** — X = pan, Y = log frequency, Z = depth (fader), sphere size =
  perceived level. Orbit/top/front/side cameras, draggable spheres, target ghosts.
- **Gibson-style coaching** — six reference images (Rock/Pop, Electronic, Hip-Hop, Jazz,
  Orchestral, Vocal Ballad), per-track role targets, match score, concrete suggestions,
  and one-click auto-placement.
- **Save / load** — `.sdaw` JSON document plus a sibling `.media/` folder of 32-bit float WAVs.
- **Export** — 32-bit float stereo WAV mixdown through the same engine.
- **Audio devices** — WASAPI (Windows), ALSA/CoreAudio, sample-rate/buffer selection,
  device persistence by display name. ASIO is an opt-in Cargo feature (see below).
- **MIDI input** — enumerate and connect MIDI ports; notes play the selected instrument
  track, with a live activity counter.
- **VST3 and VST2** — scan standard folders, load 64-bit plugins as instruments or inserts.
  VST3 uses `vst3-host`; VST2 uses the `AEffect` ABI. Plugins load in-process on the audio
  thread; scanning only inspects metadata.

## Quick start (Windows)

Requirements:

1. Windows 10/11 x64.
2. [Rust](https://rustup.rs) (the repo pins `1.99.1` in `rust-toolchain.toml`).
3. **Visual Studio Build Tools** with the “Desktop development with C++” workload
   (MSVC linker + Windows SDK).

```powershell
git clone <your-repo-url> spatial-daw
cd spatial-daw
cargo run --release -p spatial-daw
```

On first launch the app opens the default WASAPI output at 48 kHz and loads a playable
demo song. Press **Space** to start. Open **Devices / MIDI** to choose another interface,
sample rate, or buffer size.

### Linux development

```bash
sudo apt install libasound2-dev libdbus-1-dev pkg-config
cargo run --release -p spatial-daw
```

Linux uses ALSA by default. File dialogs use the XDG desktop portal (`rfd`); a portal
implementation must be running for open/save dialogs.

### ASIO (optional, Windows)

ASIO support is **not built by default** because Steinberg's SDK cannot be redistributed.
With the SDK and ASIO driver installed:

```powershell
$env:CPAL_ASIO_DIR = "C:\path\to\ASIOSDK"
cargo build --release -p spatial-daw --features asio
```

Full prerequisites and troubleshooting: [docs/ASIO.md](docs/ASIO.md). The generated release
workflow builds the default WASAPI/ALSA binary; build the ASIO variant locally.

## Using the DAW

1. **Add tracks** from the left browser: drum sounds, synth, sampler, or audio track.
2. **Write music** in the bottom editor:
   - *Step sequencer*: click steps for each instrument track.
   - *Piano roll*: click to add notes, drag to move, right-click to delete.
3. **Arrange** in the main view: drag clips to move, drag their left/right edges to trim,
   press **S** to split the selected clip at the playhead.
4. **Import audio** with *Import audio…*; it lands on the selected audio track (a new track
   is created if needed).
5. **Mix** in the *Mixer* tab, then switch to **3D mix field** and drag spheres:
   horizontal = pan, vertical = fader. Drag empty space to orbit; wheel zooms.
6. **Coach** on the right compares the image to the selected style. Pick roles, read the
   suggestions, or press **Auto-place** to land every track on its reference target.
7. **Save** (`.sdaw`) and **Export WAV** from the top bar.

### Keyboard shortcuts

| Key | Action |
| --- | --- |
| Space | Play / pause |
| S | Split selected clip at playhead |
| Delete / Backspace | Delete selected clip or note |
| Ctrl+Z / Ctrl+Y | Undo / redo |
| Ctrl+S / Ctrl+Shift+S | Save / save as |
| Ctrl+O | Open project |
| Ctrl+E | Export WAV |
| Ctrl+wheel | Zoom arrangement |
| Shift+wheel | Scroll arrangement horizontally |

## The spatial model

| Axis / channel | Musical meaning |
| --- | --- |
| X | Stereo pan, left ↔ right |
| Y | Log-frequency centroid, 20 Hz ↔ 20 kHz |
| Z | Depth from the fader, far ↔ near (36 dB range) |
| Sphere size | Perceived loudness / RMS |
| Ghost sphere | Style reference target for that role |

Targets come from the six reference images in `daw-core::gibson`, derived from the
*Art of Mixing* approach of thinking in pictures: a lead is centred and close, a kick and
bass anchor the low end, pads sit wide and behind, and so on. The analyser compares the
live mix against those images and points at pan, depth, and spectral mismatches.

## Repository layout

```
crates/
  daw-core/     Project model, IDs, musical time, media store, Gibson theory, save/load
  daw-engine/   Realtime engine, DSP, snapshots, devices, MIDI, import/export, plugin host
  spatial-daw/  eframe desktop app, arrangement editor, mixer, wgpu 3D field
```

- The audio callback owns an `Engine`; the UI builds immutable `Snapshot`s and sends them
  over a bounded command channel.
- Audio is processed in fixed 64-frame chunks with per-track voices, inserts, metering,
  a master limiter, and sample-accurate clip playback.
- Media is stored at its source rate and resampled once into a playback cache at the
  device rate.
- Plugins are hosted on the audio thread because many plugin APIs are not `Send`.

### Project format

`MySong.sdaw` is human-readable JSON (format version 1). Referenced audio and sampler
samples are written as 32-bit float stereo WAVs into `MySong.sdaw.media/`. Missing media is
reported on load and plays as silence, so a project is never lost because of a moved file.

## Building and testing

```bash
cargo fmt --all --check
cargo test -p daw-core -p daw-engine
cargo build --release -p spatial-daw
```

The UI itself is not covered by automated tests; it must be run on a desktop with a
graphics adapter. A GitHub Actions workflow builds and tests Windows and Linux on pushes,
tags, and pull requests, and attaches release binaries for `v*` tags.

## Plugins: important notes

- Use **64-bit** plugins matching the DAW architecture. 32-bit bridging is not implemented.
- VST3 plugins are loaded in-process with `vst3-host` (no C++ SDK build step required).
- VST2 plugins use the AEffect ABI and must be 64-bit `.dll` files.
- Standard per-user and system VST folders are scanned on Windows; the plugin window has a
  manual scan action. Scanning does not execute plugin code.
- Editor GUIs are not embedded. Parameter persistence is stored in the project, but the
  full opaque plugin state (chunks) is not yet serialised, and a plugin's own UI is not shown.

## Known limitations

- No time-stretching: audio clips are not warped when tempo changes.
- No automation lanes, comping, or MIDI output/CC learn yet.
- ASIO requires a local SDK build; CI binaries use WASAPI on Windows and ALSA on Linux.
- Plugin parameter editing/GUI is minimal; see above.
- The 3D field is a monitoring/coaching view, not a binaural renderer. Pan and fader are
  the audible controls; frequency height is analysis-driven.
- File dialogs on Linux require an XDG desktop portal.

## License

MIT. See [LICENSE](LICENSE). The Steinberg ASIO SDK and third-party VST plugins are not
included and remain under their own licenses.

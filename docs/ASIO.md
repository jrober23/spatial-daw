# Building Spatial DAW with ASIO

ASIO support is optional and **not included in the default binaries**. The Steinberg ASIO
SDK is not redistributable, so it cannot be vendored into this repository or attached to a
release. Build it locally if you need ASIO.

## Prerequisites

- Windows 10/11 x64.
- A 64-bit ASIO driver installed for your interface (the vendor's driver, not ASIO4ALL's
  32-bit build).
- Visual Studio Build Tools with the **Desktop development with C++** workload.
- Rust `1.99.1` (see `rust-toolchain.toml`).
- LLVM/Clang for Windows (`clang` must be on `PATH`; CPAL's ASIO bindings use bindgen).
  Install from <https://releases.llvm.org/> or `winget install LLVM.LLVM`.
- The Steinberg ASIO SDK, downloaded from the Steinberg developer portal after accepting
  their license: <https://www.steinberg.net/developers/>.

## Build

1. Extract the ASIO SDK. You should have a directory containing `ASIOSDK2.x` (or a similar
   SDK root).
2. Point CPAL at it in the same shell:

   ```powershell
   $env:CPAL_ASIO_DIR = "C:\SDKs\ASIOSDK2.3"
   ```

3. Build with the `asio` feature:

   ```powershell
   cargo build --release -p spatial-daw --features asio
   ```

4. Run `target\release\spatial-daw.exe`, open **Devices / MIDI**, choose the host whose
   name ends in **(ASIO)**, select the interface, and start audio.

The feature mapping is `spatial-daw/asio -> daw-engine/asio -> cpal/asio`. No source changes
are required.

## Troubleshooting

- **`CPAL_ASIO_DIR` not found / ASIO SDK missing**: the variable must point at the SDK root,
  not at a parent folder. Use an absolute path; quote paths containing spaces.
- **`clang` / `libclang.dll` not found**: install LLVM and ensure `clang` is on `PATH`.
  Reopen the terminal after installing.
- **No ASIO host listed**: the default build does not include ASIO. Confirm you built with
  `--features asio` and that the 64-bit driver is installed.
- **Driver opens but no sound**: close other applications using the interface (ASIO is
  usually exclusive), then press **Stop** and **Start audio** in the Devices window.
- **Buffer size rejected**: drivers clamp buffer sizes. The app clamps the requested size to
  the range reported by the driver and shows the actual size in the status bar.

## Licensing

The ASIO SDK is governed by Steinberg's license. You are responsible for complying with it
when building this feature. Neither the SDK nor ASIO binaries are distributed by this
project. The default WASAPI build on Windows requires no Steinberg components.

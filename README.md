<div align="center">

<img src="assets/icon.png" width="128" alt="Auris Whisper">

# Auris Whisper

**Offline speech-to-text for macOS, Windows and Linux. Drop a file or dictate — get clean `.txt`.**
No accounts, no uploads. Everything runs on your own computer.

[![macOS](https://img.shields.io/badge/macOS-12%2B-000000?logo=apple&logoColor=white)](#download)
[![Windows](https://img.shields.io/badge/Windows-10%20%7C%2011-0078d4?logo=windows&logoColor=white)](#download)
[![Linux](https://img.shields.io/badge/Linux-AppImage%20%7C%20deb-fcc624?logo=linux&logoColor=black)](#download)
[![License](https://img.shields.io/badge/license-MIT-22c55e)](LICENSE)
[![Made by](https://img.shields.io/badge/made%20by-boopi.ru-7c3aed)](https://boopi.ru)

<img src="docs/screenshot-en.png" width="720" alt="Auris Whisper window">

</div>

---

## What it does

- **Drop in audio or video** — mp3, m4a, wav, flac, aiff, caf, aac, ogg, opus,
  mp4, mov, mkv, webm. Drop several at once, they run one after another.
- **Voice messages work.** Ogg Opus from messengers opens fine; the format is
  detected from the file contents, not the extension.
- **Dictate** (⌘R / Ctrl+R) with a live level meter, then *Stop and transcribe*.
  The recording is kept as `.wav` next to its transcript.
- **Pick a model, from tiny to the most accurate.** Models are not baked into the
  installer: choose one after installing and it downloads once. Keep several,
  switch any time, delete the ones you don't need, or add your own model file.
- **GPU acceleration**: Metal on Apple Silicon, Vulkan on Windows and Linux
  (NVIDIA, AMD, Intel). No GPU — it runs on the CPU, automatically.
- **Almost 30 recognition languages** or automatic detection.
- **Interface in English or Russian**, switched in the top-right corner.
- **Timestamps** on demand: `[00:12 → 00:19] the sentence`.
- **Auto-save**: `recording.mp3` produces `recording.txt` right beside it.
  Dictations land in `Documents/Whisper`.
- **Editable output** — fix the text in the window before saving.
- **Load meter** at the bottom: app CPU, system CPU, GPU, memory.
- **Long recordings stay intact.** Whisper likes to get stuck repeating one phrase
  to the end of the file. Three defences are wired in: Silero voice-activity
  detection, splitting long audio into ~5-minute pieces cut at the quietest spot,
  and a re-run of any piece that derails.

Nothing is a wrapper around a web service. Once a model is downloaded, no
internet connection is needed at all.

## Download

Grab the installer for your system from [Releases](../../releases):

| System | File |
|---|---|
| macOS on Apple Silicon (M1–M4) | `AurisWhisper-x.y.z-macOS-AppleSilicon.dmg` |
| macOS on Intel | `AurisWhisper-x.y.z-macOS-Intel.dmg` |
| Windows 10 / 11 (64-bit) | `AurisWhisper-x.y.z-Windows-x64-Setup.exe` |
| Linux (any distribution) | `AurisWhisper-x.y.z-Linux-x86_64.AppImage` |
| Debian / Ubuntu / Mint | `AurisWhisper-x.y.z-Linux-x86_64.deb` |

Installers are 7–15 MB: the model is chosen and downloaded on first launch.

### First launch

**macOS.** The app is signed ad-hoc, without an Apple Developer certificate, so
macOS will say it cannot verify the developer.
- macOS 15 and newer: try to open it, then go to *System Settings → Privacy &
  Security* and click *Open Anyway* at the bottom.
- macOS 12–14: right-click the app → *Open* → *Open*.
- Or in Terminal:
  `xattr -dr com.apple.quarantine "/Applications/Auris Whisper.app"`

**Windows.** The installer has no paid Microsoft signature, so SmartScreen shows
*Windows protected your PC* → *More info* → *Run anyway*. It installs into your
user profile, no administrator rights needed.

**Linux.** AppImage: `chmod +x AurisWhisper-*.AppImage` and run it.
Package: `sudo apt install ./AurisWhisper-*.deb`.

Dictation asks for microphone access on first use.

## Models

On first launch the app offers a model and suggests the one that fits your
computer. Switch or download another one with the model button at the top.

| Model | Size | Quality | Speed | For |
|---|---|---|---|---|
| Tiny | 74 MB | ●○○○○ | ●●●●● | rough drafts, slow machines |
| Base | 141 MB | ●●○○○ | ●●●●● | clear speech |
| Small | 465 MB | ●●●○○ | ●●●●○ | computers without a GPU |
| Medium | 1.4 GB | ●●●●○ | ●●○○○ | — (Turbo is usually better) |
| **Large v3 Turbo · compact** ★ | 547 MB | ●●●●○ | ●●●●○ | **the best pick with a GPU** |
| Large v3 Turbo | 1.5 GB | ●●●●● | ●●●○○ | slightly more accurate than compact |
| Large v3 | 2.9 GB | ●●●●● | ●○○○○ | maximum quality |

For reference: on an Apple M4, Large v3 Turbo · compact transcribes 7.5 minutes
of speech in 32 seconds (×14 real time).

Models come from [HuggingFace](https://huggingface.co/ggerganov/whisper.cpp);
downloads resume where they stopped and are verified with SHA-256. They live in:

| System | Models folder |
|---|---|
| macOS | `~/Library/Application Support/ru.boopi.auriswhisper/models` |
| Windows | `%APPDATA%\ru.boopi.auriswhisper\models` |
| Linux | `~/.local/share/ru.boopi.auriswhisper/models` |

**Offline machine?** Download the `.bin` elsewhere from the link above and add it
with *Add a model file…*. Any whisper.cpp model works.

## GPU and CPU

| System | Runs on |
|---|---|
| macOS, Apple Silicon | Metal (the M1–M4 GPU) |
| macOS, Intel | CPU + Accelerate |
| Windows, Linux | Vulkan — NVIDIA, AMD and Intel GPUs; needs an up-to-date driver |
| no GPU or no driver | CPU, automatically |

If the GPU misbehaves, turn acceleration off in the models window and the CPU
takes over. The bottom bar shows what is doing the work. x86-64 builds need a
CPU with AVX2 (Intel Haswell 2013+ or AMD Ryzen/Excavator).

## Build from source

You need [Rust](https://rustup.rs), Node.js 18+, CMake and:
- **macOS** — Xcode Command Line Tools;
- **Windows** — Visual Studio 2022 Build Tools (C++), LLVM, the [Vulkan SDK](https://vulkan.lunarg.com);
- **Linux** — `libwebkit2gtk-4.1-dev libayatana-appindicator3-dev librsvg2-dev libasound2-dev libclang-dev libvulkan-dev` and the Vulkan SDK (for `glslc`).

```bash
git clone https://github.com/sibawal/auris-whisper.git
cd auris-whisper
npm install
npm run dev       # run in development mode
npm run build     # build an installer for the current system
```

On Windows, run `scripts\windows-runtime.ps1` once before `npm run build`: it
places the Visual C++ runtime and the Vulkan loader next to the app so it starts
on a clean system.

A console harness for the same decoder and engine:

```bash
cd src-tauri
cargo run --release --example transcribe -- path/to/model.bin recording.m4a en   # --cpu to skip the GPU
```

One build detail worth knowing: [.cargo/config.toml](.cargo/config.toml) sets
`GGML_NATIVE=OFF`. Without it ggml compiles in the instructions of the build
machine (SME/i8mm on an M4, AVX-512 on a CI server) and the app dies with
*Illegal instruction* on users' older CPUs.

GitHub Actions ([build.yml](.github/workflows/build.yml)) builds all four
platforms, runs a transcription smoke test on each, and drafts a release on a
`v*` tag.

## How it is put together

The window is [Tauri](https://tauri.app) (the system web view + Rust), the
engine is [whisper.cpp](https://github.com/ggml-org/whisper.cpp).

| Piece | What it does |
|---|---|
| `src-tauri/src/audio.rs` | any container → 16 kHz mono: Symphonia + libopus, streaming resampler |
| `src-tauri/src/engine.rs` | whisper.cpp via whisper-rs: chunking, voice detection, loop protection |
| `src-tauri/src/models.rs` | model catalog, resumable downloads, SHA-256 check |
| `src-tauri/src/recorder.rs` | microphone capture (CoreAudio / WASAPI / ALSA) |
| `src-tauri/src/monitor.rs` | CPU and memory; GPU via IOKit / PDH / sysfs |
| `src-tauri/src/lib.rs` | commands and events between the window and the engine |
| `ui/` | the interface: HTML, CSS and JS, no bundler |

Version 1.x — the native Swift app for macOS only — lives on in git history
(tag `v1.0.1`).

## License

MIT — see [LICENSE](LICENSE) ([перевод на русский](LICENSE.ru.md)). Use it, change it, ship it.

Built on [whisper.cpp](https://github.com/ggml-org/whisper.cpp) and ggml (MIT),
[Whisper](https://github.com/openai/whisper) models by OpenAI (MIT), Silero VAD (MIT),
Tauri (MIT/Apache-2.0), Symphonia (MPL-2.0) and libopus (BSD). Full list in
[THIRD-PARTY-LICENSES.txt](THIRD-PARTY-LICENSES.txt).

<div align="center">

Made by [boopi.ru](https://boopi.ru) · [Русская версия README](README.ru.md)

</div>

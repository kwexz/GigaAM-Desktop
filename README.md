# GigaAM Desktop

> 🇷🇺 [Документация на русском](README.ru.md)

Desktop application for **local Russian speech transcription**: import audio or video, or record a microphone and system audio live. Recognition runs on-device with GigaAM v3, Silero VAD, and ONNX Runtime; after the one-time model download, transcription works without an internet connection.

**Project status:** actively developed, pre-1.0 · **Platforms:** Windows x64; macOS 15+ on Apple Silicon when a macOS asset is present in the release · **[Download the latest release](https://github.com/kwexz/GigaAM-Desktop/releases/latest)**

Release assets include a Windows x64 installer and portable executable. A signed and notarized macOS arm64 package is available only when it is listed in that release; release jobs for the two platforms complete independently. Windows builds are intentionally unsigned and may trigger SmartScreen. Linux is not supported.

![GigaAM Desktop showing history, a mixed recording player, and an editable per-track transcript](docs/screenshot.jpg)

## What is implemented

- Import WAV, MP3, FLAC, OGG, M4A, MP4, MKV, AVI, and MOV files or drag them into the app.
- Record the microphone, system audio, or both as separate tracks; pause/resume, adjust microphone gain, and archive audio as FLAC, WAV, or MP3.
- Run Silero VAD, chunking, and int8 GigaAM v3 E2E CTC locally through ONNX Runtime, with punctuation and text normalization.
- Keep a durable SQLite queue and searchable history; interrupted jobs are marked and can be retried.
- Play from any transcript timestamp, edit individual segments, copy the full transcript, and re-run recognition.
- Export TXT, SRT, WebVTT, or JSON while preserving microphone/system track labels.
- Switch the application UI between Russian and English.

## 60-second demo

1. Start the app and click **Download** once to install the pinned models (~227 MB).
2. Click **Open** or drop a media file. The item moves through the queue and appears in history with timestamped segments.
3. Click a segment timestamp to play from that point; edit a segment inline and search the history by filename or transcript text.
4. Click **Record**, select microphone, system audio, or both, and then stop the recording. A dual-source session keeps separate tracks labelled **Me** and **System**.
5. Select TXT, SRT, WebVTT, or JSON and click **Export**.

The screenshot above is the repository's real application visual. The steps above are also a reproducible hands-on evaluation path; no mock screens or benchmark claims are used.

## Try a release

1. Download the asset for your OS from [GitHub Releases](https://github.com/kwexz/GigaAM-Desktop/releases/latest).
2. Launch the app. On Windows, SmartScreen may warn because the current executable is unsigned.
3. Click **Download** in the model banner. The app downloads pinned artifacts, verifies their byte size and SHA-256, and installs them atomically.
4. After the model banner disappears, disconnecting from the network does not affect file or live transcription.

Model locations:

- Windows: `%APPDATA%\ru.gigaam.desktop\models`
- macOS: `~/Library/Application Support/ru.gigaam.desktop/models`

See [Model download and offline use](docs/MODEL.md) for hashes, sources, retry behaviour, and manual verification.

## Build from source

Prerequisites:

- Node.js **24.11.1** and npm 11 (the versions used by CI)
- Rust **1.99.0** (pinned by `rust-toolchain.toml`)
- [Tauri 2 system prerequisites](https://v2.tauri.app/start/prerequisites/)
- Windows: Visual Studio Build Tools with the C++ workload and WebView2
- macOS: macOS 15+, Apple Silicon, and Xcode 26+ for the current ScreenCaptureKit dependency

```powershell
npm ci
npm run tauri dev
```

The first transcription or recording still needs the model download described above. Build release packages with:

```powershell
npm run tauri build -- --bundles nsis -- --locked   # Windows: NSIS + release executable
```

```bash
npm run tauri build -- --bundles dmg,app -- --locked # macOS: DMG + app
```

Local macOS packages are not equivalent to the signed and notarized artifacts produced by the release workflow. More setup and troubleshooting details are in [Development and verification](docs/DEVELOPMENT.md).

`npm test` runs the focused UI interaction suite; Rust tests cover the queue, SQLite recovery, export, audio processing, model verification, and streaming pipeline.

## Verify the project

These are the same substantive checks used by `.github/workflows/build.yml`:

```powershell
npm ci
npm run check
npm test
npm run build
cargo test --locked --manifest-path src-tauri/Cargo.toml --lib
cargo test --locked --manifest-path src-tauri/Cargo.toml --features inference-spike --bin inference-spike
cargo check --locked --manifest-path src-tauri/Cargo.toml --bin gigaam-desktop
```

GitHub Actions builds the Windows NSIS bundle and an unsigned macOS app on GitHub-hosted runners for every push and pull request. Tagged `v*` commits publish Windows packages and use a GitHub-hosted Apple Silicon runner plus repository Apple credentials for signed/notarized macOS packages.

## Engineering notes

The implementation includes a persistent single-worker queue, incremental segment commits, streaming VAD with a bounded scan window, time-aligned dual-track capture, on-demand audio mixing, model integrity checks, and recovery of interrupted jobs.

- [Architecture and data flow](docs/ARCHITECTURE.md) — preserved diagrams, live pipeline, storage, commands, and code layout
- [Development and verification](docs/DEVELOPMENT.md) — prerequisites, builds, tests, CI/release behaviour, and troubleshooting
- [Models and offline operation](docs/MODEL.md) — pinned artifacts and installation lifecycle
- [Third-party components and licenses](docs/THIRD_PARTY_NOTICES.md) — model and runtime attribution

## License and attribution

The repository's original source code is licensed under the [MIT License](LICENSE). That license does **not** relicense third-party models, libraries, or platform SDKs. GigaAM, the ONNX conversion, Silero VAD, ONNX Runtime, Symphonia, `shine-rs`, Tauri, and other dependencies retain their own licenses; see [Third-party components and licenses](docs/THIRD_PARTY_NOTICES.md). In particular, `shine-rs`, used for MP3 archive encoding, declares LGPL-2.0.

GigaAM Desktop is an independent client and is not presented as an official GigaAM application.

# Development and verification

## Supported development targets

The source tree has capture backends for Windows and macOS. Release and routine CI targets are:

| Target | Support and packaging |
| --- | --- |
| Windows x64 | GitHub-hosted Windows 2025 CI, NSIS installer, and portable release executable |
| macOS 15+ Apple Silicon | `aarch64-apple-darwin`, DMG/app; CI and releases use GitHub-hosted `macos-26` Apple Silicon runners |
| Linux / Windows arm64 / Intel macOS | Not supported or published by this repository |

The generic non-Windows/non-macOS capture module intentionally returns an unsupported-platform error; icon assets for other Tauri targets do not imply application support.

## Prerequisites

- Git
- Node.js 24.11.1 and npm 11 (matching CI; dependencies are locked by `package-lock.json`)
- Rust 1.99.0 with the target from `rust-toolchain.toml`
- [Tauri 2 prerequisites](https://v2.tauri.app/start/prerequisites/)

Windows additionally needs Visual Studio Build Tools with the Desktop development with C++ workload and WebView2. macOS needs macOS 15+, Apple Silicon, and the Xcode/Metal SDK required by `screencapturekit` and its dependency tree; release CI currently uses Xcode 26+.

## Development run

```powershell
git clone https://github.com/kwexz/GigaAM-Desktop.git
cd GigaAM-Desktop
npm ci
npm run tauri dev
```

`npm run tauri dev` starts Vite HMR and rebuilds the Rust backend. The browser-only `npm run dev` is insufficient for normal use because the UI calls Tauri commands.

On first launch, click **Download** in the model banner. See [MODEL.md](MODEL.md) for the exact files, installation path, and offline preparation.

## Checks and tests

Run from the repository root:

```powershell
npm ci
npm run check
npm test
npm run build
cargo test --locked --manifest-path src-tauri/Cargo.toml --lib
cargo test --locked --manifest-path src-tauri/Cargo.toml --features inference-spike --bin inference-spike
cargo check --locked --manifest-path src-tauri/Cargo.toml --bin gigaam-desktop
```

- `npm run check` runs Svelte/TypeScript diagnostics.
- `npm test` runs focused Svelte UI tests with mocked Tauri boundaries: first-run model state, language switching, history/transcript selection, file import, and live-recording command arguments.
- `npm run build` verifies the static frontend production build.
- Rust library tests cover domain behaviour, storage/recovery, export formats, chunking/live scanner logic, and model-manifest validation without downloading model binaries.
- The `inference-spike` test target compiles/tests the optional diagnostic binary; it does not claim a quality or performance benchmark.
- `cargo check` validates the desktop binary.

## Packages

Windows:

```powershell
npm run tauri build -- --bundles nsis -- --locked
```

The NSIS installer is written below `src-tauri/target/release/bundle/nsis/`; the release executable is `src-tauri/target/release/gigaam-desktop.exe`.

macOS:

```bash
npm run tauri build -- --bundles dmg,app -- --target aarch64-apple-darwin -- --locked
```

Local macOS output is not automatically Developer ID signed or notarized. Those operations depend on repository secrets and run only in `.github/workflows/release.yml`.

## CI and releases

`.github/workflows/build.yml` runs on every push and pull request using GitHub-hosted Windows and Apple Silicon macOS runners. Both jobs run frontend diagnostics, UI tests, and production frontend builds. Windows runs the Rust tests/check and builds NSIS; macOS compiles all Rust targets and builds an unsigned `.app` with Xcode 26. The macOS test harness is compiled but not executed because the current ScreenCaptureKit-linked test binary cannot locate `libswift_Concurrency` on launch.

`.github/workflows/release.yml` runs only for `v*` tags:

- Windows: repeats key checks and publishes NSIS plus the portable executable.
- macOS: runs on GitHub-hosted `macos-26`, imports signing credentials from repository secrets through `tauri-action`, and publishes signed/notarized DMG/app assets.

The two publish jobs update the same GitHub Release independently. A release can therefore appear after the first job finishes while the second platform's assets are still pending. Always verify the asset list and both job results before announcing a release.

Current Windows artifacts are unsigned, as stated in the workflow's release body. Do not describe them as signed until an actual signing step is present and an Authenticode check succeeds.

## Troubleshooting

- **Model banner remains visible:** retry the download on a stable connection. A partial staging generation is not treated as installed. Details are in [MODEL.md](MODEL.md).
- **Windows capture is silent:** check the selected/default capture device and whether the render device is active. Diagnostics are in `%APPDATA%\ru.gigaam.desktop\gigaam-debug.log`.
- **macOS system audio or microphone is unavailable:** grant the requested Screen Recording/System Audio and Microphone permissions, then restart the app.
- **A job is marked Interrupted:** this is expected recovery after the process exits during work; use retry from history.
- **macOS build fails in the ScreenCaptureKit/Metal dependency tree:** verify the expected Xcode 26+ toolchain and the `aarch64-apple-darwin` target.
- **A media container opens but audio decoding fails:** support depends on the audio codecs enabled through Symphonia; a listed container does not imply every codec variant inside it is decodable.

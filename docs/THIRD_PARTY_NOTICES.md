# Third-party components and licenses

This document is an attribution and audit aid, not legal advice. The root [MIT license](../LICENSE) applies to this repository's original code only. It does **not** relicense model weights, converted artifacts, libraries, SDKs, or operating-system components. Each item remains subject to its upstream terms.

## Downloaded model artifacts

These files are downloaded on user request and are not committed to this repository or bundled in release packages. Exact revisions and hashes are in [MODEL.md](MODEL.md).

| Component | How it is used | Upstream / declared license |
| --- | --- | --- |
| GigaAM v3 E2E CTC | Base Russian ASR model | [`salute-developers/GigaAM`](https://github.com/salute-developers/GigaAM), [MIT license](https://github.com/salute-developers/GigaAM/blob/main/LICENSE); the base [Hugging Face card](https://huggingface.co/ai-sage/GigaAM-v3) declares MIT |
| GigaAM v3 ONNX conversion | int8 ONNX model and vocabulary actually downloaded by the app | [`istupakov/gigaam-v3-onnx`](https://huggingface.co/istupakov/gigaam-v3-onnx), whose model card declares MIT and identifies `ai-sage/GigaAM-v3` as the base model |
| Silero VAD | Voice activity detection ONNX model | [`snakers4/silero-vad`](https://github.com/snakers4/silero-vad), [MIT license](https://github.com/snakers4/silero-vad/blob/master/LICENSE) |

The project does not claim authorship of these models. “GigaAM” and other names belong to their respective owners. GigaAM Desktop is an independent client, not an official GigaAM distribution.

## Direct runtime and build dependencies

The complete resolved Rust dependency graph is recorded in `src-tauri/Cargo.lock`; JavaScript dependencies are recorded in `package-lock.json`. Important direct components include:

| Component | Purpose | Declared license |
| --- | --- | --- |
| [ONNX Runtime](https://github.com/microsoft/onnxruntime) | Native inference engine loaded through `ort` | MIT |
| [`ort`](https://github.com/pykeio/ort) | Rust bindings and build integration for ONNX Runtime | MIT OR Apache-2.0 |
| [Tauri](https://github.com/tauri-apps/tauri) | Desktop shell and plugins | Apache-2.0 OR MIT |
| [Svelte](https://github.com/sveltejs/svelte), [SvelteKit](https://github.com/sveltejs/kit), [Vite](https://github.com/vitejs/vite) | Static application UI and build tooling | MIT |
| [Symphonia](https://github.com/pdeljanov/Symphonia) | Media demuxing and audio decoding | MPL-2.0 |
| [`shine-rs`](https://github.com/wshon/shine-rs) | MP3 encoding for optional live archives | LGPL-2.0 as declared by the resolved crate; upstream ships the GNU Library General Public License v2 text |
| [`wasapi`](https://github.com/HEnquist/wasapi-rs) | Windows microphone and loopback capture; a patched copy is vendored under `vendor/wasapi` | MIT; copyright and license text are retained in [`vendor/wasapi/LICENSE.txt`](../vendor/wasapi/LICENSE.txt) |
| [`screencapturekit`](https://github.com/doom-fish/screencapturekit-rs) | macOS microphone/system-audio capture | MIT OR Apache-2.0 |
| [`rusqlite`](https://github.com/rusqlite/rusqlite) | SQLite persistence; SQLite is bundled by the selected crate feature | MIT for `rusqlite`; SQLite itself is public domain per upstream |
| [`rubato`](https://github.com/HEnquist/rubato) | Audio resampling | MIT |
| [`flac-codec`](https://github.com/tuffy/flac-codec) | FLAC archive encoding | MIT OR Apache-2.0 |
| [`hound`](https://github.com/ruuda/hound), [`ndarray`](https://github.com/rust-ndarray/ndarray), [`rustfft`](https://github.com/ejmahler/RustFFT), [`reqwest`](https://github.com/seanmonstar/reqwest) | WAV I/O, tensors, FFT, and HTTPS model download | See each resolved package's license metadata and upstream license |

Operating-system APIs and redistributables (WebView2, Windows audio APIs, Apple ScreenCaptureKit, system WebView) remain governed by their platform terms.

## Distribution review notes

- The current application packages are distributed without downloaded model files. Users fetch them separately through explicit UI action.
- Symphonia's MPL-2.0 and `shine-rs`'s LGPL-2.0 deserve explicit release-compliance review. The repository's own MIT label must not be used as a blanket statement for the complete executable.
- Before each public binary release, generate or refresh a machine-readable dependency/license report from the lockfiles and preserve all notices required by resolved dependency versions.
- Any future decision to bundle model files requires a fresh review of the exact artifact revision and its accompanying license files/model card.

If an upstream license declaration changes, the pinned revision—not the latest default branch—should be used to assess an already published release.

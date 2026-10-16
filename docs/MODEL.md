# Models and offline operation

GigaAM Desktop does not commit model binaries and does not bundle them in application packages. The user explicitly starts a one-time download from the model banner. Application source and downloaded model artifacts remain separately licensed works; the repository's MIT license does not replace model licenses.

## Pinned artifacts

The authoritative manifest is `ARTIFACTS` in `src-tauri/src/model_manager.rs`.

| Artifact | Source revision | Bytes | SHA-256 |
| --- | --- | ---: | --- |
| `v3_e2e_ctc.int8.onnx` | [`istupakov/gigaam-v3-onnx@322c3b2`](https://huggingface.co/istupakov/gigaam-v3-onnx/tree/322c3b29492673eb7d0b434bfa9dfb8653e34d02) | 224,893,347 | `2e3fcb7a7b66030336fd10c2fcfb033bd1dc7e1bf238fe5cfd83b1d0cfc9d28e` |
| `v3_e2e_ctc_vocab.txt` | [`istupakov/gigaam-v3-onnx@322c3b2`](https://huggingface.co/istupakov/gigaam-v3-onnx/tree/322c3b29492673eb7d0b434bfa9dfb8653e34d02) | 2,007 | `142de7570b3de5b3035ce111a89c228e80e6085273731d944093ddf24fa539cd` |
| `silero_vad.onnx` | [`snakers4/silero-vad@1e261b0`](https://github.com/snakers4/silero-vad/blob/1e261b036686cd0017d500ee96acd1c4ba572a9d/src/silero_vad/data/silero_vad.onnx) | 2,327,524 | `1a153a22f4509e292a94e67d6f9b85e8deb25b4988682b7e174c65279d8788e3` |

Total download size is 227,222,878 bytes (about 227 MB decimal / 217 MiB).

The ASR file is an int8 ONNX conversion of GigaAM v3 E2E CTC. The conversion repository identifies `ai-sage/GigaAM-v3` as its base model. Sources and licensing references are collected in [THIRD_PARTY_NOTICES.md](THIRD_PARTY_NOTICES.md).

## Installation lifecycle

1. The app downloads every artifact into a sibling staging directory.
2. It checks the exact byte count and full SHA-256 while downloading.
3. It verifies the complete staged generation again and writes `.verified`.
4. It atomically swaps the staged directory into the active model location, retaining the previous generation until the swap succeeds.
5. On every app start, the model is considered installed only when `.verified` exists and all artifact sizes and hashes still match.

A failed or cancelled attempt removes partial files and does not replace a valid installed generation. The current implementation restarts an interrupted download rather than resuming an HTTP partial.

## Locations

- Windows: `%APPDATA%\ru.gigaam.desktop\models`
- macOS: `~/Library/Application Support/ru.gigaam.desktop/models`

## Offline use

Internet access is required only to fetch missing model artifacts from the pinned Hugging Face and GitHub URLs. Once the app reports that the model is installed, file import, live capture, inference, history, playback, editing, and export use local resources. The app does not silently substitute another model.

To prepare a machine that will later be offline, install and launch a release while online, click **Download**, wait for the banner to disappear, close the app, disconnect the network, and launch it again. Import a short media file to verify the offline setup.

Do not copy arbitrary files into the model directory: the exact manifest above is enforced. If transferring an already installed directory between machines, preserve `.verified` and all three files, then let the app validate it at startup.

## Manual integrity check

Windows PowerShell:

```powershell
$models = Join-Path $env:APPDATA 'ru.gigaam.desktop\models'
Get-ChildItem $models -File | Get-FileHash -Algorithm SHA256
```

macOS:

```bash
cd "$HOME/Library/Application Support/ru.gigaam.desktop/models"
shasum -a 256 v3_e2e_ctc.int8.onnx v3_e2e_ctc_vocab.txt silero_vad.onnx
```

Compare the complete output to the table, not to the abbreviated hashes sometimes used in release notes.

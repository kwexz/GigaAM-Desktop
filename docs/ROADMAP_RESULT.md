# GigaAM Desktop — roadmap result

Date: 2026-10-28
Branch: `feature/gigaam-desktop-web-autonomy`

## Completed phases

- **00** baseline/harness — passed.
- **01** Live bugfixes — passed; physical USB/macOS boundaries remain not-run.
- **02** streaming file ASR — passed; bounded decode/VAD/ASR, batch parity, RSS plateau and bounded archive mix.
- **03** contracts — passed; versioned Rust/TypeScript events, separate FSMs and frontend ports.
- **04** browser capture/storage — passed; mic/system detection, OPFS/IndexedDB, Edge synthetic E2E.
- **05** Rust API — passed; localhost-only authenticated Axum server, bounded upload, Range and session/status APIs.
- **06** remote Live transport — passed at protocol level; durable spool ACK, checksum, sequence continuity, duplicate handling, reconnect replay.
- **07** web resilience — passed locally; local-first WebSession, bounded offline backlog, Edge E2E.
- **08** local summarization — blocked_external/resource. Source-backed extractive fallback exists; no unmeasured GGUF was downloaded or enabled.
- **09** browser ASR — validated NO-GO. Opt-in/checksum-gated adapter exists; no automatic 227 MB model download and no unsupported browser ASR button.
- **10** cloud sync — external OAuth blocked. Logical sync and mock Google/Yandex PKCE provider states are implemented and tested.
- **11** final audit — this document and final evidence/checkpoint.

## Verification commands

- `pwsh -NoProfile -File scripts/verify-baseline.ps1`
- `npm test`
- `npm run check`
- `npm run build`
- `npm run test:web` with `PLAYWRIGHT_CHANNEL=msedge`
- `cargo test --locked --manifest-path src-tauri/Cargo.toml --lib`
- `cargo test --locked --manifest-path src-tauri/Cargo.toml --tests`
- `cargo check --locked --manifest-path src-tauri/Cargo.toml --bin gigaam-server`

## Honest external boundaries

- Physical Windows hotplug and real browser microphone/system-device E2E: final manual/device validation.
- macOS GUI/capture: no Mac available.
- Real dual-track VPS ONNX/RTF: staging host inventory is available, but no Rust toolchain was reported and no unsafe system-wide installation was attempted.
- Real Google/Yandex OAuth: requires client IDs, redirect registrations and user consent.
- GGUF summarization: requires a licensed measured model/runtime; extractive fallback is intentionally not called a model summary.

No production DNS, public bind, system service, credentials, model weights or private audio were added.

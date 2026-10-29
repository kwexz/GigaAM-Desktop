# Local Web ASR integration — 2026-10-29

- Commit153e2b7 adds authenticated `/v1/transcribe`, bounded64MiB upload and single inference permit, native GigaAM in spawn_blocking, temporary input cleanup.
- Actual running topology: existing Docker Nginx proxies `/api/` to Windows host Rust server on8787; token is generated outside repository in approved Temp and injected by Nginx. No provider credentials involved. This is NOT a fully Linux-containerized inference image.
- Existing local pinned models used, no model download. Real POST of licensed besarab-clean.wav through localhost8080/api/v1/transcribe succeeded with2 nonempty native ASR segments. Response stored only in approved Temp/gigaam-asr/result.json.
- Native95 tests pass,2 explicit measurements ignored. Frontend suite51 passed before additional tree pure test; check has unused CSS warnings after removed controls. Tree/layout browser tests3/3 pass. Audio mix browser oracle running at writing time.
- New tree UI replaces per-item select/up/down controls with expanded hierarchy, collapse toggles, double-click folder editing and draggable rows. Existing old application tests targeting removed controls require updates; do not claim full suite green.
- Browser mix currently explicit bounded in-memory decode/render WAV,64MiB input/1h cap. Original per-track archives untouched. Exact offsets and long-archive streaming playback still require follow-up.
- Native Windows installer from previous build predates153e2b7; rebuild after final acceptance before presenting it as current.
- Do not expose host8787 publicly; current all-interface bind is development plumbing for Docker host access, not production deployment. Need loopback/host firewall or fully isolated container deployment before real network use.

## Follow-up reported regressions

- Desktop archive selectors excluded .ogg despite defaultOgg; fixed lib.rs filters and added native Ogg decoder in streaming.rs. Compilation check passes; decoder EOF trimming needs dedicated regression.
- Browser ASR now converts supported capture containers to16k mono PCM WAV before POST. Actual WebM capture conversion browser test passes; full UI→real backend still needs verification.
- Tree/mix/ASR conversion targeted browser tests3/3 pass; frontend52unit pass. Current Windows build failed because running desktop/server lock release executables. Started separate CARGO_TARGET_DIR=src-tauri/target-delivery with20min timeout, without stopping user application. Do not present old installer as updated.

Follow-up: native Opus decoder now applies RFC trim including EOS padding, exact48123-sample regression passes (c5a0189). Separate delivery build encountered Windows archive sharing violation and was retried using cached build; pending completion. No old installer claimed current. Browser normalization/tree/mix tests3/3 passed, conversion test committed82d2153.

Latest requested work: compact export endpoint + folder heading/hover-trash added09f661d; Linux unified image definition d2c0720. Initial Bookworm image failed linking prebuilt ONNX Runtime requiring newer glibc/libstdc++; changed both stages to Trixie4f23a8a, rebuild running. No Linux inference acceptance or live-ASR claim yet. Folder heading/hover-delete browser regression running. Current running service remains older Windows-host backend until validated container replacement.

Folder toolbar/hover-delete regression passed, commite781ce0. Container Nginx now forwards WebSocket Upgrade headers; this only enables transport and is not inference. Linux image build completion still pending; do not run another concurrent image build against same output until notification.

Linux Trixie image built successfully (image83117fcc...). Container gigaam-linux-asr running localhost8081 with read-only existing models mount; no host inference dependency. Real Linux `/api/v1/transcribe` on besarab-clean.wav returned2 segments and phrase anchor; `/api/v1/compact` returned valid Opus verified ffprobe. Responses/exports in approved Temp/gigaam-asr, not repository. Health still model_enabled:false (legacy indicator bug), not evidence that file endpoint failed. Existing8080 not replaced yet. Cloud Live remains unimplemented; no file-ASR result reclassified as Live.

Main address switched with user authorization: stopped own gigaam-web static proxy (preserved for rollback), launched gigaam-web-linux on127.0.0.1:8080 using validated Linux image/read-only models. Real file ASR and compact Opus export through8080 pass. No Windows gigaam-server process needed/running at observation.8081 test container retained. Latest Windows NSIS rebuild running; do not claim current installer before successful bundle notification. Live remains outstanding.

User requested cleanup: removed only six own gigaam* test containers; unrelated tiltium* preserved. compose.yaml now owns single gigaam-web-1 service+data volume on8080. Desktop config dragDropEnabled:false added to allow HTML internal DnD (native external file drop seam no longer works; browser/file dialog remains). Dialog spacing standardized. LiveAsr prototype independent8s PCM windows/bounded8 pending jobs implemented with result repository callback, pause/stop handling; NOT yet real live acceptance, fixed windows may cut speech and errors/backpressure need stronger persistence.52unit/check pass before follow-up. Compose rebuild restarted with explicit local env after missing-env interpolation failure; Windows rebuild in parallel. No completion claim.

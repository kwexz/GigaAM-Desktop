# Local Web ASR integration — 2026-10-29

- Commit153e2b7 adds authenticated `/v1/transcribe`, bounded64MiB upload and single inference permit, native GigaAM in spawn_blocking, temporary input cleanup.
- Actual running topology: existing Docker Nginx proxies `/api/` to Windows host Rust server on8787; token is generated outside repository in approved Temp and injected by Nginx. No provider credentials involved. This is NOT a fully Linux-containerized inference image.
- Existing local pinned models used, no model download. Real POST of licensed besarab-clean.wav through localhost8080/api/v1/transcribe succeeded with2 nonempty native ASR segments. Response stored only in approved Temp/gigaam-asr/result.json.
- Native95 tests pass,2 explicit measurements ignored. Frontend suite51 passed before additional tree pure test; check has unused CSS warnings after removed controls. Tree/layout browser tests3/3 pass. Audio mix browser oracle running at writing time.
- New tree UI replaces per-item select/up/down controls with expanded hierarchy, collapse toggles, double-click folder editing and draggable rows. Existing old application tests targeting removed controls require updates; do not claim full suite green.
- Browser mix currently explicit bounded in-memory decode/render WAV,64MiB input/1h cap. Original per-track archives untouched. Exact offsets and long-archive streaming playback still require follow-up.
- Native Windows installer from previous build predates153e2b7; rebuild after final acceptance before presenting it as current.
- Do not expose host8787 publicly; current all-interface bind is development plumbing for Docker host access, not production deployment. Need loopback/host firewall or fully isolated container deployment before real network use.

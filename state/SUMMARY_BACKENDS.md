# Summary backends
- [ ] Managed GGUF runtime installation and model selection with explicit consent.
- [ ] Managed local inference lifecycle and UI wiring.
- [ ] OpenAI compatible adapter contract tests and safe credentials.
- [ ] Checks, tests and working Windows executable.
- [ ] Real GGUF inference smoke; real API only if configured, otherwise explicitly not-run.

2026-11-05:6d22782 UI/runtime lifecycle compiles; explicit real runtime install+inference test launched against existing Ollama4B GGUF blob without model download. Root approved Temp/opencode/gigaam-gguf-smoke, runtime pinnedb12131 SHA256. Result pending; no acceptance inferred. API adapter exists but contract/credential origin validation tests remain pending.

OpenAI HTTP contract and shared URL validation implemented. Test execution blocked temporarily by Windows linker1104 while earlier real GGUF test binary still running; do not kill inference or claim contract passed. Retry after asynchronous GGUF test completes. No checkbox marked.

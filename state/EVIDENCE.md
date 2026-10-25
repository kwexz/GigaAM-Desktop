# Индекс проверок и измерений

Каждая запись: `phase | commit SHA | команда/среда | фактический результат | путь к stdout/stderr/trace/video/метрикам | статус verified/not-run/blocked_external`.

Агент создаёт каталог `artifacts/verification/NN/` (в `.gitignore` для больших логов, тестового аудио и PII), сохраняет машинно-читаемый короткий `summary.json` и ссылки на детальные логи/трейсы. Для коммитов переносимые короткие результаты допустимо положить в `docs/verification/`, не заливать огромные бинарники в Git.

Никогда не выдумывать outputs. Недоступность настоящего устройства, модели, подключения, сертификата, OAuth либо runner фиксировать честно.

## 00 — partial baseline, 2026-10-25, Windows x64

- Source baseline: `6e1ec3fe14ecbad5c6c45160ff31a097f4f88903`; separate branch `feature/gigaam-desktop-web-autonomy`. Existing untracked roadmap files preserved.
- `verified`: `pwsh -NoProfile -File scripts/verify-baseline.ps1`. Six commands exited 0: npm ci, check, test (6 tests), build, cargo lib tests (53 tests), full cargo tests. Actual stdout/stderr, exit codes and measured durations: `state/runs/00-baseline/results.json` and adjacent `.stdout.txt`/`.stderr.txt`. Repeated after adding Playwright dependency. These are baseline tests, not model E2E.
- `failed`: `npx playwright test`: browser executable absent. Trace captured under ignored `test-results/`. `npx playwright install chromium` began download but timed out after 120 seconds; browser test has not passed. Synthetic Web Audio injection is not real microphone/display capture or ASR.
- `verified`: read-only SSH inventory `sqrthost-ism`: uid 1002, Linux x86_64, 2 CPU, RAM total 3910 MiB / available 2038 MiB at observation, root filesystem available 78 GiB, Docker executable `/usr/bin/docker`. Availability is a snapshot, not reserved capacity; Docker access/ports not verified. No remote changes.
- `blocked_external`: SSH `ismhost` cannot resolve hostname. Needs existing SSH alias/DNS configuration before real smoke.
- Staging boundary: only a new user-owned isolated directory, bind 127.0.0.1, inspect ports immediately before launch, no sudo/service changes; do not run inference on ismhost. No staging deployed.
- `not-run`: macOS GUI/build, physical-device E2E, native Windows packaged build, real GigaAM/Silero integration metrics, licensed multi-phrase Russian golden fixtures, license inventory, independent review. All remain required work; no phase checkpoint created.
- `npm audit --json`: existing Vite dependency reports one high-severity affected package (including Windows launch-editor advisories); no automatic audit fix applied.
- Resume phase 00 only: finish required module/CI inventory, fixtures/provenance, model harness, browser installation/E2E and review. Do not read phase 01 until core gates pass.

# GigaAM Desktop — состояние автономного плана

- `current_phase`: `11`
- `phase_state`: `in_progress`
- `last_verified_commit`: `e6d989e`
- `current_branch`: `feature/gigaam-desktop-web-autonomy`
- `blocking_issue`: `10 completed with external-oauth-blocked outcome; final audit records remaining physical/VPS/model boundaries`
- `last_update_utc`: `2026-10-27`

## Источник истины

Завершённый этап подтверждается только парой: `state/checkpoints/NN.ok` + воспроизводимые записи тестов в `state/EVIDENCE.md`. Если файл маркера есть, но его evidence отсутствует или не соответствует текущему коду, перепроверить и удалить ошибочный checkpoint.

## Матрица этапов

| NN | Этап | Статус | Commit | Основной результат |
|---|---|---|---|---|
| 00 | Baseline & harness | passed | aab7936 | Baseline, real model clean/noisy metrics, browser recording/OPFS harness, VPS static smokes |
| 01 | Live bugfixes | passed | 7c50dae | VAD cursor red/green; async Stop; recovery supervisor; real-model and archive acceptance passed |
| 02 | Streaming core | passed | 22b9ec9 | Bounded file decode/VAD/ASR, batch parity, 5-minute model E2E, RSS plateau, bounded offset-preserving archive mix |
| 03 | Universal contracts | passed | c6556f4 | Versioned Rust/TypeScript events, FSM states, frontend ports, compatibility tests, ADR |
| 04 | Browser capture & storage | passed | 29a9075 | Browser mic/system adapter, OPFS/IndexedDB storage, Edge synthetic E2E |
| 05 | Rust VPS API | passed | d887c13 | Authenticated localhost Axum API, bounded upload, Range download, session/model status, safe server binary |
| 06 | Remote Live transport | passed | fd0798d | Bounded RemoteEngine, authenticated WS, checksum/durable ACK, sequence gap rejection, reconnect replay |
| 07 | Web E2E, resilience | passed | 0eab9dc | Local-first WebSession, bounded offline backlog, Edge E2E, browser resilience contracts |
| 08 | Local summarization | blocked_external | cda23f7 | Extractive source-backed fallback/API; licensed measured GGUF experiment blocked by unavailable weights/runtime |
| 09 | Optional in-browser ASR | pending | — | — |
| 10 | Google/Yandex cloud sync | pending | — | — |
| 11 | Final audit & handoff | pending | — | — |

Статусы: `pending`, `in_progress`, `passed`, `blocked_external`, `failed`. В блокерах указывай минимальное необходимое действие и обоснование; не останавливай независимые тесты/подзадачи.

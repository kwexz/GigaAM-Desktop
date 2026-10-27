# GigaAM Desktop — состояние автономного плана

- `current_phase`: `04`
- `phase_state`: `pending`
- `last_verified_commit`: `c6556f4`
- `current_branch`: `feature/gigaam-desktop-web-autonomy`
- `blocking_issue`: `03 passed; cloud/OAuth and browser inference remain intentionally deferred to their phases`
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
| 04 | Browser capture & storage | pending | — | — |
| 05 | Rust VPS API | pending | — | — |
| 06 | Remote Live transport | pending | — | — |
| 07 | Web E2E, resilience | pending | — | — |
| 08 | Local summarization | pending | — | — |
| 09 | Optional in-browser ASR | pending | — | — |
| 10 | Google/Yandex cloud sync | pending | — | — |
| 11 | Final audit & handoff | pending | — | — |

Статусы: `pending`, `in_progress`, `passed`, `blocked_external`, `failed`. В блокерах указывай минимальное необходимое действие и обоснование; не останавливай независимые тесты/подзадачи.

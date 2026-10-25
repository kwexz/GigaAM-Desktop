# GigaAM Desktop — состояние автономного плана

- `current_phase`: `00`
- `phase_state`: `in_progress`
- `last_verified_commit`: `none`
- `current_branch`: `feature/gigaam-desktop-web-autonomy`
- `blocking_issue`: `00 acceptance incomplete: redistributable speech golden fixtures and remaining harness/CI review pending. Edge browser harness and PS5 baseline/model tests verified; both VPS aliases accessible.`
- `last_update_utc`: `2026-10-25`

## Источник истины

Завершённый этап подтверждается только парой: `state/checkpoints/NN.ok` + воспроизводимые записи тестов в `state/EVIDENCE.md`. Если файл маркера есть, но его evidence отсутствует или не соответствует текущему коду, перепроверить и удалить ошибочный checkpoint.

## Матрица этапов

| NN | Этап | Статус | Commit | Основной результат |
|---|---|---|---|---|
| 00 | Baseline & harness | in_progress | — | Baseline green; reproducible runner and synthetic browser test added; no checkpoint |
| 01 | Live bugfixes | pending | — | — |
| 02 | Streaming core | pending | — | — |
| 03 | Universal contracts | pending | — | — |
| 04 | Browser capture & storage | pending | — | — |
| 05 | Rust VPS API | pending | — | — |
| 06 | Remote Live transport | pending | — | — |
| 07 | Web E2E, resilience | pending | — | — |
| 08 | Local summarization | pending | — | — |
| 09 | Optional in-browser ASR | pending | — | — |
| 10 | Google/Yandex cloud sync | pending | — | — |
| 11 | Final audit & handoff | pending | — | — |

Статусы: `pending`, `in_progress`, `passed`, `blocked_external`, `failed`. В блокерах указывай минимальное необходимое действие и обоснование; не останавливай независимые тесты/подзадачи.

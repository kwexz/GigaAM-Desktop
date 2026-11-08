# Tasks

## 1. Проверка исходных утверждений

- [x] 1.1 Зафиксировать HEAD, рабочий diff и список пользовательских изменений без их перезаписи; результат — baseline-аудит с ревизией и dirty-state.
- [x] 1.2 Проверить кодировку EVIDENCE.md read-only, сверить адресные ссылки и checkpoints с матрицей STATUS/ROADMAP_RESULT/START_HERE; результат — таблица verified/not-run/blocked_external/failed с доказательством каждой строки и противоречиями 09/10/11.
- [x] 1.3 Выполнить npm ci при первом запуске, npm run check, npm test, npm run build и cargo test --locked --manifest-path src-tauri/Cargo.toml --lib; сохранить реальные логи и результаты без присвоения старому passed нового значения.

## 2. Основная веб-версия

- [x] 2.1 Добавить воспроизводящий Playwright E2E основного маршрута / без Tauri: запись synthetic mic, Stop, reload, список, playback и export; проверка — тест выявляет фактические пробелы, harness помечен отдельно.
- [x] 2.2 Подключить browser runtime через contracts/ports/WebSession к основному UI и исключить Desktop IPC из браузерных операций; проверка — unit runtime adapters и E2E / проходят без ошибок IPC.
  - Реализовано: WebSession start/stop/pause/resume, OPFS playback/download, загрузка истории и incoming remote segments. Проверено application.spec.ts и web-session.test.ts. Осталось: убрать Desktop IPC из остальных browser actions (rename/edit/import/export/delete), подключить capabilities и проверить их компонентными тестами.
  - Завершено: browser action branches, runtimeCapabilities и component no-IPC regression; явная accessible rename вместо зависимости от dblclick. 48 frontend tests, state/runs/browser-runtime-final/result.json exit0; full product tests включают import/rename/playback/export/delete storage/replay/recovery. Remote inference/physical devices не заявлены проверенными.
- [x] 2.3 Проверить OPFS/IndexedDB persistence, offline capture, reconnect и отсутствие system audio track с явным fallback; проверка — browser E2E с измеренными saved/duplicate/missing и сохранёнными логами.
  - Проверено: offline capture, pause/resume, reload, playback/download, denied display fallback, непустой архив и SHA-256 для малого файла. Осталось: durable reconnect/replay и missing/duplicate accounting, падение вкладки во время записи, quota/error scenarios.
  - Дополнительно verified: durable OPFS block replay (seq0/seq1 сохранены один раз, gap rejected), mock WebSocket disconnect-before-ACK/replay/drain, interrupted-session восстановление и metadata quota injection с сохранением аудиобайтов. Два targeted Edge теста reconnect/quota прошли; ещё требуется объединённый сохранённый лог полной матрицы и проверка UX fallback/recovery.
  - Завершено: полная application matrix exit0, state/runs/web-recovery-final/{result.json,stdout.txt,stderr.txt}; synthetic/mocked scopes явно отделены от real ASR/devices. Два исходных блока replay восстанавливаются без дублей/пропусков, повтор wire seq0 до ACK ожидаем и идемпотентен.
- [x] 2.4 Документировать поддерживаемые браузеры, secure-context, remote ASR и ограничения устройств; проверка — инструкции соответствуют пройденным сценариям и отдельным not-run.
  - Evidence: docs/WEB_CURRENT_SUPPORT.md; browser scenarios проверены application.spec.ts, ограничения явно отделены от support claims.

## 3. Подключение Google и Яндекс

- [ ] 3.1 По официальным источникам зафиксировать grant/scopes/redirect/CORS и безопасный обмен отдельно для обоих провайдеров и Web/Desktop; проверка — provider compatibility table с URL источников и точной конфигурацией, без секретов.
- [ ] 3.2 Реализовать production OAuth adapters, безопасное хранение и UI подключить/отключить/повторить; проверка — unit/integration на invalid state, replay, expiry, revoke, missing config и отсутствие mock:// в production flow.
  - Реализован и unit-проверен S256 callback helper; конкретные client ID и реальное согласие отложены пользователем до конца. Осталось: native loopback, безопасное хранение ОС, поддерживаемый provider token exchange и UI. Низкоуровневый helper не равен готовой авторизации.
- [ ] 3.3 Реализовать реальные API adapters Google Drive и Яндекс.Диск для логических метаданных и аудио, долговечные outbox/cursors; проверка — контрактные интеграционные тесты upload/download/checksum/retry/restart и отсутствие передачи SQLite-файла.
  - Реализованы cloud-files, cloud-backup (4 MiB chunks, remote readback, manifest-last), browser outbox/Web Locks и journal helpers. Unit проверены retry, integrity, manifest ordering. Осталось: Desktop outbox, discovery/cursors, полная UI-интеграция и provider contract E2E; реальные API ещё не проверены.
- [ ] 3.4 Добавить разрешение конфликтов с сохранением данных и UI статусов отдельно от local capture; проверка — двухклиентский тест конкурентных правок, идемпотентного повтора и отзыва доступа.
- [ ] 3.5 Добавить mock-provider E2E основного UI и документацию OAuth setup; отдельно выполнить real-provider smoke с согласия пользователя при доступной конфигурации, иначе записать blocked_external с конкретным недостающим действием.

## 4. Папки и порядок записей

- [x] 4.1 Добавить версионированные contracts и SQLite/IndexedDB миграции folders/parent/position с root для старых записей; проверка — integration upgrade fixture сохраняет ID, звук, текст и исходный порядок, не допускает частичной миграции.
  - Дополнительно проверено: native migration/move сохраняют recording ID/source_path/audio_path/segments и очередь; повторная миграция и folder reopen проходят. Browser v1→v2 fixture сохраняет ID/name/duration; atomic legacy-audio migration fixture ещё нужен.
  - Завершено: browser migration fixture дополнен побайтовым сохранением OPFS archive; versioned organization contract + валидация перед atomic IDB persistence, native validated transaction rollback. Evidence: application.spec.ts и storage::tests::organization_migration_and_moves_preserve_recordings_segments_and_queue.
- [x] 4.2 Реализовать create/rename/move/reorder/delete-folder в обеих storage реализациях; проверка — unit и integration на циклы, отсутствующего родителя, атомарную перестановку и перенос содержимого удалённой папки.
  - Реализованы pure planner, IndexedDB transactional merge/move, SQLite validated transactional save и optimistic stale-write guard. Unit проходят cycles/missing parent/reorder/delete preservation и native rollback. Осталось: versioned migration/reopen fixtures и конкурентные native integration tests.
  - Завершено: native stale-writer rejection вынесен в тестируемый Store method и проверен; reopen/migration/audio-text-queue сохранение проходят; IDB concurrent independent moves и rename/delete preservation проходят в application.spec.ts. Все операции используют общий planner и атомарное платформенное сохранение.
- [x] 4.3 Добавить вложенный список, DnD и доступные меню переноса/перестановки, RU/EN подписи; проверка — component tests и Playwright create/nest/drag/reorder/reload/search без изменения ASR queue.
  - UI и browser E2E подтверждают create/nest/drag/select/reload. Осталось: reorder/search/delete/rename E2E, полная RU/EN доступность и component tests. Папки не перемещают физическое аудио.
  - Reorder/search/reload и folder rename/delete preservation теперь также проходят в application.spec.ts; RU/EN доступность и component coverage ещё не завершены.
  - Завершено: organizationLabels RU/EN подключены к accessible labels; component test проверяет Desktop create/save contract. Native preservation test подтверждает неизменность ASR queue. Все 5 application browser scenarios проходят.
- [ ] 4.4 Добавить folder/order changes в облачный журнал и документацию миграции/backup; проверка — двухклиентский тест rename/move/delete/retry без потери записей и воспроизводимая backup-инструкция.

## 5. Ogg/Opus и компактный default

- [x] 5.1 Сравнить Rust encoder candidates по лицензии, bounded streaming и сборке Windows/macOS; закрепить выбор и зависимости с lockfile, проверка — native build/test Windows, macOS CI при безопасной доступности либо честный not-run.
  - Evidence: docs/OGG_ENCODER_DECISION.md, Cargo.lock, locked native tests; macOS явно not-run, сборка альтернатив не заявлена.
- [x] 5.2 Добавить Ogg/Opus в ArchiveFormat, streaming finalization/export и UI, не меняя PCM-ASR тракт; проверка — native unit/integration на реальный Ogg header, decode, duration, track offsets, cancel/crash recovery и сохранение spool при ошибке.
  - Реализован native bounded encoder opus-pure, ArchiveFormat::Ogg, export mapping и предварительный UI. Unit подтверждает OggS/OpusHead и packet decode. Осталось: exact duration, independent decoder, cancel/crash и offset integration; выбор формата в UI требует финализации.
  - Завершено: EOS granule exact duration, независимый ffmpeg decode, 500ms dual-track offset manifest, cancel/raw retention и retry после interrupted partial. Ogg option в основном select и component start_live test проходят. Commits f2badc0/d0b694d/75ca089; docs/OGG_MEASUREMENTS.md. Это проверка staged-retry, не физическое отключение питания.
- [x] 5.3 Добавить browser MIME negotiation и честный WebM/Opus fallback; проверка — unit supported/unsupported MIME и browser E2E decode/playback/export с совпадающими MIME и расширением.
  - Evidence: recording-format.test.ts (supported/unsupported), application.spec.ts (реальный container header, MIME/path, playback/export), Edge targeted E2E passed.
- [ ] 5.4 Измерить одинаковые clean/noisy speech fixtures во всех доступных профилях: bytes/minute, decode/duration, качество речи/ASR, CPU/RSS и Stop latency; результат — таблица и обоснованный компактный default без выдуманного абсолютного минимума.
  - Дополнительно: release process CPU/RSS измерены scripts/measure-codec-resources.ps1, acknowledgement seam проходит release test; GUI IPC latency остаётся not-run. Выбран48k default как lossy compromise, а не нулевая ASR-регрессия (docs/OGG_MEASUREMENTS.md, commit2c87262). Пункт остаётся open до финальной полной матрицы/ресурсного scope review.
  - Измерены размеры/время debug WAV/FLAC/MP3/Ogg на одинаковом speech input (APPLY_AUDIT.md). Осталось: качество/ASR, exact duration, release CPU/RSS/Stop. FLAC default пока сохранён; одна экономия размера не закрывает acceptance.
- [x] 5.5 Сохранять явный выбор формата и применять новый default только при отсутствии предпочтения; проверка — preference migration tests, старые архивы не меняются, docs объясняют lossy/lossless и fallback.
  - Evidence: recording-format.test.ts defaults/invalid/disabled-storage tests, component saved-FLAC test, docs/OGG_ENCODER_DECISION.md и OGG_MEASUREMENTS.md. 51 frontend tests/check pass; setting read/write не перекодирует существующие архивы.

## 6. Итоговая интеграционная приёмка

- [x] 6.1 Повторить npm run check, npm test, npm run build, cargo test --locked --manifest-path src-tauri/Cargo.toml --lib, профильные Rust integration и npm run test:web; проверка — логи привязаны к ревизии, Web/Desktop regression matrix заполнена.
  - Evidence: state/runs/final-local-matrix/results.json + REVISION.md, all6 exit0, dirty-tree scope explicitly recorded. Это не финальный checkpoint и не real OAuth/VPS/physical/macOS acceptance.
- [ ] 6.2 Провести независимый review существенного diff и исправить замечания; проверка — отчёт review и повторные целевые тесты, пользовательские изменения не присвоены коммиту.
- [x] 6.3 Обновить STATUS/EVIDENCE/ROADMAP_RESULT по фактам и перечислить real OAuth, physical devices, VPS/macOS gates отдельно; проверка — ни один pending/mock/harness не назван проверенной production-функцией.
  - Evidence: STATUS follow-up section, appended EVIDENCE OpenSpec index и ROADMAP_RESULT warning; current dirty-tree matrix, synthetic/mock scope, user-deferred providers и not-run platforms явно указаны. Исторические checkpoints не удалены/не переименованы.
- [ ] 6.4 Зафиксировать реализацию атомарными коммитами текущей ветки, затем metadata/checkpoint отдельным коммитом только при прохождении core acceptance; проверка — implementation SHA существует, ссылки на логи воспроизводимы, без production publish и force-push.

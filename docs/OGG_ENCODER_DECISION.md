# Выбор native Ogg/Opus encoder

## Решение

Используется `opus-pure 0.2.2`, BSD-3-Clause, минимальный Rust 1.88. Cargo.lock закрепляет версию; текущая Windows toolchain успешно собирает и проверяет её с `--locked`. Это выбор реализации, не утверждение о прохождении качества речи или macOS.

## Сравнение

| Вариант | Свойства | Решение |
|---|---|---|
| opus-pure 0.2.2 | Pure Rust; encoder/decoder и Ogg encapsulation; bounded 20 ms frames без внешнего executable; BSD-3-Clause | Выбран; необходимы independent-decoder и speech acceptance |
| opus 0.4.0 | Rust bindings к libopus; потребуется native C build/link и отдельный Ogg muxer | Не выбран для первого варианта: больше сборочных зависимостей на Windows/macOS; сборка кандидата не запускалась |
| внешний ffmpeg | Готовые codecs/muxers; отдельная поставка executable и лицензионная конфигурация | Не выбран: обязательный внешний encoder не вводится |

Источники: https://crates.io/crates/opus-pure/0.2.2 , https://docs.rs/opus-pure/0.2.2 , https://crates.io/crates/opus/0.4.0 . Метаданные проверены через `cargo info opus-pure` и `cargo search opus`; пример crate изучен до использования API.

## Проверка и ограничения

- Windows: locked library build и тест OggS/OpusHead/packet decode проходят. Память encoder ограничена frame buffers; raw input читается потоково. Проверка отмены выполняется между frames, выход staging `.partial` удаляется при ошибке.
- macOS CI/build: **not-run**, успешная Windows сборка не доказывает macOS совместимость.
- Speech size benchmark — `cargo test --locked --manifest-path src-tauri/Cargo.toml --lib compare_speech_archive_profiles -- --ignored --nocapture`. Он opt-in, не утяжеляет обычную unit suite.
- После сравнения24/48 kbit/s выбран48 kbit/s как compact default новых настроек: независимое декодирование, speech ASR comparison, duration и aggregate process resources описаны в OGG_MEASUREMENTS.md. Это lossy-компромисс, не обещание идентичности текста.
- Явное сохранённое предпочтение FLAC/WAV/MP3/Ogg имеет приоритет. FLAC остаётся lossless выбором; существующие аудиофайлы автоматически не перекодируются. GUI IPC latency и macOS остаются not-run.

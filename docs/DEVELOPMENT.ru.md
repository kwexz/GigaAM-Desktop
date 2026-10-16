# Разработка и проверки

## Целевые системы

В коде есть backends захвата для Windows и macOS. Регулярный CI и релизы ориентированы на:

| Цель | Поддержка и пакеты |
| --- | --- |
| Windows x64 | GitHub-hosted Windows 2025 CI, NSIS-установщик и portable exe |
| macOS 15+ Apple Silicon | `aarch64-apple-darwin`, DMG/app; CI и релизы используют GitHub-hosted Apple Silicon runner `macos-26` |
| Linux / Windows arm64 / Intel macOS | Репозиторий не поддерживает и не публикует сборки |

Общий модуль для систем кроме Windows/macOS намеренно возвращает ошибку неподдерживаемой платформы. Наличие иконок для других целей Tauri не означает поддержку приложения.

## Зависимости

- Git
- Node.js 24.11.1 и npm 11 — как в CI; зависимости зафиксированы `package-lock.json`
- Rust 1.99.0 и target из `rust-toolchain.toml`
- [системные зависимости Tauri 2](https://v2.tauri.app/start/prerequisites/)

Windows дополнительно требует Visual Studio Build Tools с workload Desktop development with C++ и WebView2. Для macOS нужны macOS 15+, Apple Silicon и Xcode/Metal SDK, которые требуются деревом `screencapturekit`; release CI сейчас использует Xcode 26+.

## Запуск разработки

```powershell
git clone https://github.com/kwexz/GigaAM-Desktop.git
cd GigaAM-Desktop
npm ci
npm run tauri dev
```

`npm run tauri dev` запускает Vite HMR и пересборку Rust backend. Одного `npm run dev` в браузере недостаточно: UI вызывает команды Tauri.

При первом старте нажмите **Загрузить** в баннере модели. Точные файлы, путь и подготовка к офлайн-работе описаны в [MODEL.md](MODEL.md).

## Проверки и тесты

Из корня репозитория:

```powershell
npm ci
npm run check
npm test
npm run build
cargo test --locked --manifest-path src-tauri/Cargo.toml --lib
cargo test --locked --manifest-path src-tauri/Cargo.toml --features inference-spike --bin inference-spike
cargo check --locked --manifest-path src-tauri/Cargo.toml --bin gigaam-desktop
```

- `npm run check` запускает диагностику Svelte/TypeScript.
- `npm test` запускает сфокусированные Svelte UI-тесты с mocked Tauri boundary: первый запуск без модели, язык, выбор истории/транскрипта, импорт файла и параметры старта live-записи.
- `npm run build` проверяет production-сборку статического frontend.
- Rust unit-тесты покрывают доменную логику, storage/recovery, форматы экспорта, chunking/live scanner и manifest моделей без скачивания бинарников.
- Target `inference-spike` компилирует и тестирует опциональную диагностическую утилиту; это не бенчмарк качества или производительности.
- `cargo check` проверяет desktop binary.

## Пакеты

Windows:

```powershell
npm run tauri build -- --bundles nsis -- --locked
```

NSIS появляется в `src-tauri/target/release/bundle/nsis/`, release exe — в `src-tauri/target/release/gigaam-desktop.exe`.

macOS:

```bash
npm run tauri build -- --bundles dmg,app -- --target aarch64-apple-darwin -- --locked
```

Локальная macOS-сборка не становится автоматически подписанной Developer ID и нотаризованной. Эти операции требуют secrets репозитория и выполняются только в `.github/workflows/release.yml`.

## CI и релизы

`.github/workflows/build.yml` выполняется для каждого push и pull request на GitHub-hosted Windows и Apple Silicon macOS runners. Обе задачи запускают frontend diagnostics, UI-тесты и production-сборку frontend. Windows выполняет Rust tests/check и собирает NSIS; macOS компилирует все Rust targets и собирает неподписанный `.app` с Xcode 26. macOS test harness компилируется, но не запускается: текущий бинарник с ScreenCaptureKit при старте не находит `libswift_Concurrency`.

`.github/workflows/release.yml` запускается только для тегов `v*`:

- Windows повторяет ключевые проверки и публикует NSIS и portable exe.
- macOS работает на GitHub-hosted `macos-26`, получает signing credentials из repository secrets через `tauri-action` и публикует подписанные/нотаризованные DMG/app.

Обе задачи независимо обновляют один GitHub Release. Поэтому релиз может появиться после первой задачи, пока файлы второй платформы ещё не готовы. Перед анонсом нужно проверить assets и результат обеих задач.

Текущие Windows-файлы не подписаны — это также указано в release body workflow. Нельзя называть их подписанными, пока не появится реальный signing step и Authenticode-проверка.

## Диагностика

- **Баннер модели не исчезает:** повторите загрузку при стабильном соединении. Неполное staging-поколение не считается установленным. См. [MODEL.md](MODEL.md).
- **На Windows нет звука записи:** проверьте выбранный/default capture device и активность render device. Лог: `%APPDATA%\ru.gigaam.desktop\gigaam-debug.log`.
- **На macOS недоступен системный звук или микрофон:** выдайте разрешения Screen Recording/System Audio и Microphone, затем перезапустите приложение.
- **Job получил статус Interrupted:** это штатное восстановление после выхода во время работы; используйте повтор из истории.
- **macOS-сборка падает в ScreenCaptureKit/Metal:** проверьте Xcode 26+ и target `aarch64-apple-darwin`.
- **Контейнер открывается, но аудио не декодируется:** поддержка зависит от codec внутри контейнера и возможностей Symphonia; список контейнеров не означает поддержку любой их codec-комбинации.

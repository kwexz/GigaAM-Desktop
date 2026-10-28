# Проверка OMO для OpenCode V2

Дата: 2026-10-28. Host: Windows, OpenCode v2.2.0, Node v24.11.1. Проверен опубликованный npm `oh-my-opencode@5.1.48`; GitHub dev package.json на момент чтения — 5.1.49. Наш task-continuity не отключался и не изменялся. Глобальный installer OMO не запускался.

## Вердикт

**Не подходит как замена continuation в текущем OpenCode V2.** Пакет загружается, но V2 setup — намеренная заглушка, не регистрирующая агенты, инструменты и hooks. Это не отсутствие правильной настройки: так устроен опубликованный entrypoint. Полный OMO проект предлагает через отдельный OmO Native, то есть другой host/runtime, не усиление существующего интерактивного чата OpenCode.

## Первичные источники

- https://raw.githubusercontent.com/code-yeongyu/oh-my-openagent/dev/packages/omo-opencode/src/opencode-v2/server-entry.ts — `Plugin.define`, setup только вызывает log; комментарий: V2 только регистрирует plugin, V1 server export запускает полную реализацию.
- https://raw.githubusercontent.com/code-yeongyu/oh-my-openagent/dev/packages/omo-opencode/src/opencode-v2/startup-notice.ts — «OpenCode 2 support is limited: the plugin loads, but its agents, tools and hooks are not active here.»
- https://raw.githubusercontent.com/code-yeongyu/oh-my-openagent/dev/docs/guide/migrating-from-opencode.md — Native это `omo-ai`, отдельная команда `omo` и pinned senpi engine. Документ также упоминает перемещение одного файла OpenCode при первой сессии; полную изоляцию настроек нельзя предполагать без проверки.
- https://opencode.ai/v2/docs/build/plugins/migrate-v1 — V1 implementations не работают в V2 без port.

## Что фактически выполнено

1. `npm view oh-my-opencode version dependencies --json`: опубликована 5.1.48.
2. `npm pack oh-my-opencode@5.1.48` в `C:/Users/User/AppData/Local/Temp/opencode/omo-evaluation`; tarball извлечён и изучен, lifecycle scripts не запускались.
3. В опубликованном dist/index.js найден тот же V2 setup (около строк 89855–89869), что и в исходнике: log notice, без регистрации hooks. Конечный default export использует именно этот setup; полный V1 server остаётся отдельным экспортом.
4. Первый Node import failed из-за отсутствующего zod — не runtime test. `npm install` внутри извлечённого monorepo package failed на workspace:*; установка зависимостей выполнена отдельно в родительский temp каталог с `--ignore-scripts --no-save`.
5. Установлены только опубликованные runtime dependencies: zod 4.6.5, detect-libc 2.1.2, @opentui/solid 0.5.11. npm предупредил, что транзитивный @opentui/core требует Node >=26.4.0/Bun >=1.3.0; текущий Node 24.11.1 не соответствует Node requirement. Это отдельное предупреждение, не причина отсутствия V2 hooks.
6. Изолированный Node import dist/index.js успешен. Default keys: id/server/setup. Выполнен default.setup с Proxy-контекстом, который фиксирует и запрещает любое обращение к API. Результат: setup_result=undefined, context_accesses=[]; setup завершился без регистрации событий/инструментов.

Это реальный локальный smoke entrypoint, но не полный OpenCode UI E2E. Он достаточен для узкого вывода, что исследованный V2 setup не устанавливает continuation. Проверка полного V1 enforcer, Native runtime, обращения к модели и токеновые затраты — not-run.

## Рекомендация

- Оставить наш минимальный V2 plugin. Не ставить OMO вторым enforcer: в данной версии он не решит задачу V2, а большой набор функций/зависимостей здесь не нужен.
- Не переходить на V1/Native только ради continuation без отдельного решения пользователя: это изменение рабочего host, а не небольшая настройка.
- Из OMO полезно заимствовать идеи blocked-state, общего stop-state и единственного владельца продолжения, но переносить конкретный V1 код без V2 адаптации нельзя.
- Повторить compatibility smoke после появления настоящих V2 hook registrations в опубликованном OMO; версия/маркетинговое «loads» сами по себе не acceptance.

## Ограничения

Время и стоимость полного OMO не измерялись. Никакая часть OMO не подключалась к рабочему OpenCode. Существующие пользовательские провайдеры, модели, permissions и task-continuity не менялись. Вывод относится к 5.1.48 и исследованному dev source, не ко всем будущим версиям.

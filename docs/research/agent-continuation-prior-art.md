# Автопродолжение coding agents: существующие решения

Дата исследования: 2026-10-28. Исследование первичных документаций, репозиториев и issues. Установка чужих расширений и изменения текущего плагина не выполнялись. Изменяемая документация/main не подтверждает наличие функции в конкретной установленной версии.

## Вывод

Проблема известна. Готовые подходы: todo continuation enforcer, Ralph loops, Stop hooks и supervisor. Наиболее близкий prior art — todo-continuation в oh-my-openagent; однако его совместимость с OpenCode V2 здесь не проверена. Устанавливать весь набор поверх собственного плагина без проверки нельзя: несколько независимых источников продолжения сами вызывают дубли.

## OpenCode V2

[Официальная миграция](https://opencode.ai/v2/docs/build/plugins/migrate-v1) прямо предупреждает, что V1 plugins не работают в V2. V2 использует `@opencode/plugin`, `Plugin.define`, `ctx.event.subscribe` и cleanup подписок. Исторические примеры `session.idle`, `event.properties.sessionID`, `client.session.chat` не следует переносить в V2.

[Server plugin API](https://opencode.ai/v2/docs/build/plugins/server) предоставляет event subscription и prompt admission/durable inbox. Это механизм расширения, не готовое доказательство завершения задачи. [Отмена inbox item](https://opencode.ai/v2/docs/api/session/v2-session-inbox-cancel) относится к ещё не доставленному сообщению: остановка execution и отмена уже queued continuation — разные операции.

[Исторический issue #2021](https://github.com/sst/opencode/issues/2021) обсуждает idle как завершение agent loop и передачу инициативы пользователю. Это история V1, не спецификация V2.

## oh-my-opencode / oh-my-openagent

Старые URL проекта перенаправляются в oh-my-openagent.

- [PR #2140](https://github.com/code-yeongyu/oh-my-openagent/pull/2140): исключение blocked todos из incomplete count во избежание бесконечных циклов. Урок: незавершённая работа и исполнимая сейчас работа не одно и то же.
- [Issue #1511](https://github.com/code-yeongyu/oh-my-openagent/issues/1511): пересечение recovery-путей, повторных session.prompt и todo enforcer. Свидетельство риска нескольких владельцев continuation, но не диагноз нашего случая.
- [PR #6039](https://github.com/code-yeongyu/oh-my-openagent/pull/6039): единый stopContinuation, отмена guards/countdowns/Ralph и очистка состояния для разных entry points. Урок: stop должен охватывать все пути, очереди и callbacks.
- [Исторический issue #354](https://github.com/code-yeongyu/oh-my-opencode/issues/354): completion promise не распознавался, цикл продолжался до iteration cap. Даже terminal marker требует предохранителя.

## Claude Code

[Hooks reference](https://code.claude.com/docs/en/hooks) описывает Stop hook, блокирующее завершение решение и stop_hook_active. Пользовательское прерывание не эквивалентно обычному Stop; ошибки выделены отдельно. Доступность конкретных новых функций необходимо сверять с установленной версией.

Официальный Ralph plugin:
- [README](https://github.com/anthropics/claude-plugins-official/blob/main/plugins/ralph-loop/README.md)
- [Stop hook source](https://github.com/anthropics/claude-plugins-official/blob/main/plugins/ralph-loop/hooks/stop-hook.sh)

Он повторяет prompt в текущей сессии до completion promise либо max iterations, поддерживает cancel. Скрипт проверяет state/session isolation и прекращает цикл при повреждении состояния. Promise модели не заменяет тесты и acceptance evidence.

## Codex

[Официальная hooks documentation](https://developers.openai.com/codex/hooks) описывает Stop, turn_id, stop_hook_active и blocking continuation. Не следует обобщать старые утверждения об отсутствии hooks на все версии.

Первичные сообщения о failure modes:
- [#37937](https://github.com/openai/codex/issues/37937): повторяющийся блокирующий Stop hook без прогресса;
- [#20783](https://github.com/openai/codex/issues/20783): ошибка invalid_id_prefix после hook continuation;
- [#52950](https://github.com/openai/codex/issues/52950): взаимодействие goal continuation boundaries и Stop hook.

Это сообщения о конкретных случаях, не доказательство воспроизводимости на любой сборке. Ссылки полезны как набор regression scenarios: repeated block, resumed history, compaction и конкурирующие механизмы.

## Сравнение

| Подход | Польза | Риск |
|---|---|---|
| V2 server plugin | Встроен в интерактивную сессию | Собственная политика completion, гонки admission |
| Todo enforcer | Опирается на оставшиеся задачи | Stale/blocked задачи, несколько prompt producers |
| Stop hook | Решение до окончательного завершения | Рекурсивное блокирование без прогресса |
| Ralph loop | Готовая bounded iteration | Ложный/нераспознанный promise, расходы |
| Внешний supervisor | Переживает остановку процесса | Повтор действий, нужен durable checkpoint |

## Рекомендация для нашего случая

Не менять текущий plugin и не ставить второй enforcer одновременно. Заимствовать требования из существующих решений, а не обещать exactly-once:

1. Один владелец продолжения на сессию.
2. Stop/cancel имеет приоритет; отменяются не только callbacks, но и принадлежащие механизму queued inbox items.
3. Отдельно решаются event deduplication, single-flight, идемпотентность решения и inbox admission. Один busy-флаг не заменяет все четыре.
4. Перед admission повторно проверяются checklist revision и stop-generation.
5. Лимиты итераций/времени/стоимости, no-progress detector и явный blocked_external.
6. Completion основан на acceptance → тестовый результат → evidence/commit, а не только галочках или слове DONE.

В изученных источниках не установлен универсальный inbox exactly-once/dedup contract. Нельзя заявлять гарантию без проверки runtime API.

## Локальный дубликат: диагноз пока не установлен

Возможны повторное событие, несколько подписок, старый queued prompt, recovery-конкурент, stale checklist или race между чтением и admission. Это гипотезы. Нужны read-only traces: plugin instance/subscription ID, event/session/execution IDs, decision/admission timestamps, inbox ID/status, checklist revision и prompt source. Исследование не доказывает ни одну из этих причин.

## Проверенность

- Исследованы первичные источники; ссылки на issue не означают подтверждённую причину локального дефекта.
- Установка и запуск сторонних решений, проверка совместимости с OpenCode 2.2.0 — not-run.
- Причина локального дубликата и exactly-once гарантия — not established.
- Текущий plugin и приложение в рамках исследования не изменены.

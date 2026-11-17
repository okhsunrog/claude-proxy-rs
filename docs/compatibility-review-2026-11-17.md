# Совместимость клиентов и сравнение с CLIProxyAPI

Проверен `main` CLIProxyAPI на 17 ноября 2026: [`7df56074b252384d878ec4fca840552c6fb5236e`](https://github.com/router-for-me/CLIProxyAPI/tree/7df56074b252384d878ec4fca840552c6fb5236e), дата коммита 01:25:30 UTC. Ниже сравнение прочитанных механизмов с текущим кодом прокси, а не обещание полного совпадения с Claude Code.

## Что уже сделано у нас

- Профили `auto`, `generic`, `openai`, `cline`, `roo`, `opencode`, `claude-code`; совместимые Read/Write/Edit/Bash проходят консервативную проверку схемы, поля аргументов переводятся в обе стороны. Несовместимые контракты сохраняются в MCP-инструментах. Результаты инструментов остаются содержимым клиента.
- Cache policy `auto-5m` по умолчанию, `auto-1h` и `preserve`. Заданные клиентом метки и нативные запросы сохраняются. Общий `llm-relay 1.1.0` переносит разбивку записей кэша по TTL, стоимость часовых записей учитывается отдельно.
- Изолированный HTTP/DB-аудит реальными Python SDK: обычный и потоковый ответ, чтение, ошибка отсутствующего файла, редактирование, продолжение и отмена. Проверены точные суммы токенов и расчётная стоимость в PostgreSQL, включая смешанные TTL и отмену.
- Реальный OpenCode 2.2.7 выполнил чтение, ошибку и редактирование через сервер. Его `read` и `shell` сохранились MCP-инструментами; совместимый `edit` прошёл адаптер. Проверены файл, история и usage в БД. Cline/Roo проверены по контрактам; их extension host в этом аудите не запускался.
- Ранее захваченный профиль macOS x64 Claude Code 2.1.326: TLS inference, заголовки, CCH, OAuth account/device/session identity. Это не делает произвольные приложения побайтно одинаковыми с нативным клиентом: их сообщения, инструменты и управление разговором различаются.
- Из текущего сравнения перенесены два небольших исправления: `type: custom` не исключает клиентский инструмент из нормализации, а ошибки HTTP сохраняют `Retry-After`, `retry-after-ms`, `x-should-retry` и request ID. Сквозной тест проверяет 429 на обоих API; cookies не передаются.

## Дополнительные механизмы CLIProxyAPI

| Механизм | Отличие от нас и применимость |
| --- | --- |
| Retry/cooldown по аккаунту и модели, пул аккаунтов | У нас один OAuth-аккаунт на провайдера и повтор после 401; нет сопоставимого планировщика и переключения аккаунтов. Следующий полезный шаг: ограниченный retry до начала выдачи ответа, с учётом подсказок upstream и общего времени запроса. Пул аккаунтов нужен при появлении нескольких аккаунтов. Не повторять генерацию после начала SSE. |
| Состояние aliases для Threads | У них ограниченное хранилище на 1024 записи связывает aliases с `previous_message_id`, включая продолжение без повторной передачи tools. У нас карта строится из tools текущего запроса. Для поддержки таких продолжений нужна отдельная карта по ключу клиента, аккаунту и thread, с ограничением размера/срока жизни. |
| Signed thinking и replay | У них есть проверка происхождения подписей и отдельные механизмы replay. Наш нативный путь сохраняет переданное клиентом содержимое; восстановление потерянных при преобразовании подписей требует дополнительного состояния. Важно: прочитанный `claude_thinking_replay.go` включает replay для совместимого API-key режима, а не автоматически для OAuth. Копировать его как универсальное решение нельзя. |
| Tool search и inline tools | Есть проверка поддержки моделями `defer_loading`/`tool_reference`, обработка встроенных инструментов и новых форматов. У нас переназначение имён сосредоточено на tools, tool_choice и обычной истории tool_use; полную совместимость tool search и inline tool additions этот аудит не доказывает. Нужны отдельные сценарии поиска, объявления и последующего вызова инструмента. |
| Неподдерживаемые имена полей JSON Schema | Они заменяют схему открытым объектом, переносят исходную схему в description и снимают strict. Это помогает отправить запрос, но сервер больше не обеспечивает исходные ограничения. Автоматически этот fallback не перенесён. |
| Cache diagnostics и continuity | Они добавляют diagnostics и поддерживают состояние продолжений, нормализуют TTL после размещения меток. У нас есть измеренный CCH/identity и новая cache policy, но нет такого же полного слоя diagnostics/continuity. Добавлять его стоит после отдельного нативного захвата: выдуманные счётчики не доказывают совместимость и могут менять кэшируемый префикс. |
| Сохранение web-search результатов | Есть отдельный кэш результатов и восстановление контекста при преобразовании протоколов. У нас нет эквивалентного специального механизма. Это полезно при добавлении полноценных сценариев встроенного поиска и цитат. |

Практический порядок после текущих изменений: корректность thread/tool-search продолжений, затем ограниченный retry; replay подписей — после воспроизводимого случая потери подписанной истории конкретным клиентом. Простое копирование всех обходных преобразований CLIProxyAPI способно менять семантику: например, их helper для Claude 5.5 понижает принудительный tool_choice до auto. Такое поведение не перенесено без отдельного решения.

## Источники

- [Планирование cooldown и retry hints](https://github.com/router-for-me/CLIProxyAPI/blob/7df56074b252384d878ec4fca840552c6fb5236e/sdk/cliproxy/auth/conductor_cooldown.go)
- [Хранилище aliases для Threads](https://github.com/router-for-me/CLIProxyAPI/blob/7df56074b252384d878ec4fca840552c6fb5236e/internal/runtime/executor/claude_executor_tool_state.go)
- [Условия включения thinking replay](https://github.com/router-for-me/CLIProxyAPI/blob/7df56074b252384d878ec4fca840552c6fb5236e/internal/runtime/executor/claude_thinking_replay.go)
- [Server tools и type custom](https://github.com/router-for-me/CLIProxyAPI/blob/7df56074b252384d878ec4fca840552c6fb5236e/internal/runtime/executor/helps/claude_builtin_tools.go), [поддержка tool search моделями](https://github.com/router-for-me/CLIProxyAPI/blob/7df56074b252384d878ec4fca840552c6fb5236e/internal/runtime/executor/helps/claude_tool_search_model.go)
- [Fallback JSON Schema](https://github.com/router-for-me/CLIProxyAPI/blob/7df56074b252384d878ec4fca840552c6fb5236e/internal/runtime/executor/helps/claude_tool_property_names.go)
- [Размещение кэша, diagnostics и контекст поиска](https://github.com/router-for-me/CLIProxyAPI/blob/7df56074b252384d878ec4fca840552c6fb5236e/internal/runtime/executor/claude_executor_execute.go)
- [Понижение tool_choice](https://github.com/router-for-me/CLIProxyAPI/blob/7df56074b252384d878ec4fca840552c6fb5236e/internal/runtime/executor/helps/claude_tool_choice.go)
- [Официальные TTL и тарифы prompt caching](https://platform.claude.com/docs/en/build-with-claude/prompt-caching)

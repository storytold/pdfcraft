# RFC: совместное редактирование PDF в PdfCraft

- Статус: Draft
- Автор: `pdfcraft/envoy` (по ADR-0001…0010 и `ARCHITECTURE.md`)
- ADR: `.craft/bus/decisions/0001-op-unit.md` … `0010-undo-offline.md`
- Upstream issue/PR: будет открыт после review; ветка `collab/impl`
- Основание: `TASK.md`, `INTEGRATION.md`, `docs/collab/study.md`, `docs/collab/options.md`

## 1. Контекст

В PdfCraft нет совместной работы: один пользователь, один файл. Документ — граф
COS-объектов с copy-on-write оверлеем (`crates/cos/src/document.rs:36-80`);
единственная точка правки — `Session::apply(DocId, Edit)` (`crates/engine/src/session/editing.rs:22`).
`Edit` — внутренний enum на 106 вариантов, не serde, адресует страницы
индексами, аннотации — `(page, index)`; публичный API не отдаёт `ObjRef`.
Undo — глобальный снапшот всего документа (`crates/engine/src/history.rs:8,106`).
После каждой правки движок пишет incremental-ревизию и создаёт новый
`RenderPool` (`editing.rs:332,392`); на 100 000 страниц реинспекция ≈5 с
(`docs/gaps.md:315-317`).

Нужны два класса действий (TASK.md):

1. **merge** — комментарии/аннотации, ответы, статусы, значения полей формы;
2. **structural** — rotate/delete/insert/move/duplicate/replace страниц,
   закладки, структура полей.

## 2. Цели и не-цели

Цели:

- 2–5 участников на один PDF; комментарии и аннотации в реальном времени;
- структурные операции сериализованы сервером и не теряют страницы;
- роли `owner/editor/commenter/viewer`, проверка на сервере;
- per-user undo, reconnect, commit в кабинет;
- одиночный режим не ломается; фича за флагом.

Не-цели v1:

- зашифрованные и подписанные PDF (сессия запрещена);
- юридически значимая ЭП, OCR-очередь, мобильный touch;
- офлайн-редактирование structural-операций;
- CRDT/OT.

## 3. Модель данных для синхронизации

### 3.1 Единица операции (ADR-0001)

Wire-формат — не `Edit`, а нормализованная intent-операция:

```text
Op {
  op_id: string,          // UUID клиента, дедуп на сервере
  doc: DocKey,            // file id кабинета + версия файла
  base_version: u64,      // версия, на которой клиент строил операцию
  epoch: u64,             // PDF: меняется structural-операцией
  kind: command | primitive | structural,
  class: merge | structural,   // назначает адаптер, не клиент
  target: Target,
  payload: Value,         // схема операции; blob — по sha256
  preview: Option<string> // id локального оптимистичного результата
}
```

- `command` — нормализованная команда движка (без `Selection`);
- `primitive` — узкая операция (например, правка текста аннотации);
- `structural` — операция над страницами/деревом.

### 3.2 Идентификаторы (ADR-0002)

| Сущность | Адрес на время сессии |
|---|---|
| страница | `page` + `epoch`; `epoch++` на каждую structural |
| аннотация | `/NM` (UUID, префикс `clientId`) + `page` + `epoch`; сервер отвергает дубли |
| ответ в thread | `/IRT` + `/NM` корня; сервер хранит карту |
| поле формы | полное имя `parent.child` |
| вложение | ключ name-tree |
| закладка | путь + `epoch` |
| слой | `ObjRef` OCG `(num, generation)` |

`ObjRef` страницы/аннотации — внутренний идентификатор сервера; на провод он
не попадает. Узкий API движка (`page_ref`, `annotation_ref`) добавляется
следующим шагом, wire-формат от него не зависит.

### 3.3 Документ на сервере

Авторитетная копия = `pdfcraft_engine::Session` с одним документом + журнал
операций + снапшоты (ADR-0005). Журнал хранит `opId`, `version`, `epoch`,
`actor`, `class`, адрес и payload; результат применения — не хранится, его
можно пересчитать.

## 4. Протокол

### 4.1 Транспорт

WebSocket/WSS, один набор сообщений для всех пяти приложений. JSON-фрейм
(текст), лимит одного сообщения — 256 KiB; blob-поля передаются отдельным
бинарным каналом или ссылкой `sha256`. Native-клиент: `tungstenite`;
web-клиент: `web-sys` WebSocket **или** host-мост `postMessage`
(`apps/pdfcraft-web/src/main.rs:120-160`) — выбирается в реализации, конверт
один.

### 4.2 Сообщения

| Тип | Направление | Поля | Семантика |
|---|---|---|---|
| `hello` | C→S | `v`, `app`, `doc`, `jwt`, `client` | JWT только здесь, не в URL |
| `welcome` | S→C | `sessionId`, `actorId`, `role`, `lease`, `version`, `epoch`, `limits` | сессия открыта |
| `op` | C→S | см. §3.1 | намерение |
| `applied` | S→C | `opId`, `version`, `epoch`, `result`, `actorId` | авторитетный результат |
| `rejected` | S→C | `opId`, `code`, `retryable`, `hint{version,epoch}` | отказ |
| `presence` | C↔S | `page`, `viewport`, `cursor`, `selection`, `role`, `color` | эфемерно, ≤2 KiB, ≤10 Гц |
| `resync` | C→S | `lastVersion` | догон после разрыва |
| `snapshot` | S→C | `mode`, `cursor`, `bytes`/`ops` | база + хвост |
| `commit` / `commit-result` | C→S / S→C | см. §8 | сборка файла |
| `ping`/`pong` | C↔S | `ts` | heartbeat, продление lease |

Ошибки: `auth`, `lease`, `stale`, `conflict`, `forbidden`, `invalid`, `limit`,
`unsupported`.

### 4.3 Порядок и версии (ADR-0003)

- Один серверный монотонный `version` на документ; единственный порядок.
- `opId` дедуплицируется; окно последних операций персистентно (переживает
  рестарт) — повтор возвращает сохранённый `applied`, операция не применяется
  второй раз.
- `structural` требует `baseVersion == head`; иначе `rejected{code:"stale",
  retryable:true, hint:{version}}`.
- `merge` принимается на любой базе; при `epoch != head.epoch` —
  `rejected{code:"stale", retryable:true}`.
- Presence не увеличивает `version` и не журналируется.

### 4.4 Конфликты (ADR-0004)

| Ситуация | Политика | Что видит пользователь |
|---|---|---|
| два ответа в один thread | оба применяются, порядок серверный | оба ответа |
| правка текста/rect/цвета одной аннотации | LWW по свойству, побеждает серверный порядок | последнее значение + «изменено другим» |
| удаление аннотации с ответами | удаляются ответы и pop-up | thread исчез |
| ответ на удалённую аннотацию | `rejected{conflict}` | «комментарий удалён» |
| structural при активных участниках | эксклюзивно, `baseVersion == head` | «страницы изменены, обновите» |
| два structural одновременно | один применяется, второй stale | повтор после resync |
| значения полей | LWW по полному имени | последнее значение |
| структурная правка полей | только structural | «форма изменена» |

OT/CRDT не используются. LWW запрещён для содержимого (формулы, геометрия
страниц, текст документа) — только для свойств аннотаций и значений полей.

### 4.5 Reconnect и догон

Клиент присылает `lastVersion`. Если хвост журнала начиная с `lastVersion+1`
в окне — сервер отдаёт `applied`-последовательность; иначе `snapshot`
(база + хвост) и клиент сбрасывает непринятую очередь. Клиентские ops с
`opId`, уже применёнными, не повторяются (дедуп).

### 4.6 Снапшоты и compaction (ADR-0005)

- База — рабочие байты PDF (`Session::save_bytes`), хвост — журнал операций.
- Периодический snapshot: `write_incremental` от текущей базы; compaction по
  N операций / M байт / T минут сводит журнал в новую базу (допустим full
  rewrite только на компакции).
- Хранится 1–2 базы + хвост; TTL сессии — по правилу кабинета.
- Per-user undo-журнал хранится отдельно и в снапшот не входит.

## 5. Presence (ADR-0006)

Отдельный тип в том же WS; не журналируется. Полезная нагрузка: `page`
(индекс + epoch), `viewport {x,y,zoom}`, `cursor` (user space), `selection`
(`/NM` или rect), `role`, `color`. Частота ≤10 Гц, размер ≤2 KiB, при
backpressure — drop-stale, TTL 30 с. На wasm рендер идёт в UI-потоке
(`crates/render/src/raster/pool.rs:261-283`), поэтому presence не должен
вызывать перерисовку страницы.

## 6. Undo/redo на пользователя (ADR-0010)

- Undo — обычная операция с `baseVersion = head`, но с `inverse`-payload.
- Для аннотаций inverse: удалить созданное, восстановить прежние
  `Contents/Rect/State/Color` из серверного снимка свойств.
- Для structural inverse: обратный move/rotate, восстановление удалённых
  страниц из сохранённого объекта (сервер хранит копию до операции).
- Undo не откатывает чужие правки: он применяется поверх head и получает
  новую `version`; при конфликте — `rejected{conflict}`.
- Стек undo — на актора, на сессию; TTL истории — по TTL сессии.
- Offline: только `merge`-операции с `opId`; structural offline запрещены.

## 7. Роли и права (ADR-0008)

JWT в `hello`: `{sub, doc, role, sid, exp}`; `role ∈ owner|editor|commenter|viewer`.
Проверка на сервере для каждой операции:

- `viewer` — только presence/snapshot;
- `commenter` — merge-операции аннотаций/полей, без structural и без
  `Edit::Protect/ApplyRedactions/Sanitize`;
- `editor` — всё, кроме управления сессией;
- `owner` — всё + закрытие сессии.

Дополнительно проверяются PDF-permissions (`crates/engine/src/edit/run.rs:9-140`).
Токен не логируется; `Origin` проверяется для web; lease на документ
(один editor), продление по `ping`, отзыв — разрыв по `sid`.

## 8. Commit и версии (ADR-0009)

- Commit инициирует owner/editor или сервер по TTL/выходу последнего.
- Сервер применяет журнал к своей базе и пишет файл: `save_bytes`
  (incremental) по умолчанию; `save_full_bytes` только если операция требует
  полного rewrite (delete pages, redaction, PDF/A, смена шифрования).
  `/ModDate` инжектируется (`Session::with_clock`) — commit детерминирован.
- Кабинет получает `{fileId, bytes, sha256, version, expectedVersion}` и
  создаёт новую версию; исходная сохраняется.
- Внешнее изменение файла (etag/`file_watch`) → `409`, сессия не
  перезаписывает; кабинет предлагает «сохранить копию».
- После commit сессия жива, `dirty=false`, `version` не сбрасывается.

## 9. Сервер

- `pdfcraft-collab-server` — отдельный native-бинарь (не режим приложения).
- Зависимости: `pdfcraft-engine` (headless, без egui), `pdfcraft-collab-proto`,
  WebSocket-стек, JWT, storage. Лицензии — по allowlist upstream.
- Комната = `(fileId, app)`; воркер на комнату; падение комнаты не роняет
  чужие.
- Healthcheck, метрики (сессии, ops/s, размер журнала, lag), лимиты
  (участники, сообщение, ops/s, размер документа), TTL.
- Логи без содержимого PDF, комментариев и токенов.

## 10. Клиент

- Feature flag `collab` (default off), включается кабинетом через
  `pdfcraft:session` (host-мост) или query-параметр `collab=1`.
- Интеграция: `Session` остаётся источником истины для локального применения;
  collab-слой добавляет `apply_remote(op)` и отдельный путь применения чужих
  операций мимо `push_undo` (`editing.rs:188`).
- UI: список участников, курсоры/выделения, индикатор синхронизации; viewer и
  commenter не видят запрещённых команд (и сервер их отвергает).
- Одиночный режим: те же `save_bytes`, `render`, undo; collab не влияет.

## 11. Тесты

Headless, без GUI, 2–3 клиента:

1. unit: сериализация конверта, дедуп `opId`, `baseVersion`/`epoch`.
2. два клиента комментируют разные страницы — оба комментария видны.
3. два ответа в один thread — оба сохраняются, порядок стабилен.
4. правка/удаление одной аннотации — LWW и каскад ответов.
5. два structural одновременно — один stale, после resync применяется.
6. structural + merge на одной странице — merge отклоняется по epoch.
7. reconnect: `lastVersion` в окне и вне окна (snapshot).
8. роли: viewer/commenter не могут structural; сервер отвергает.
9. формы: значения полей LWW, структура — только structural.
10. undo: отмена своей правки не откатывает чужую.
11. commit: `.pdf` открывается в Chrome/Adobe; ссылки, закладки, формы,
    вложения целы; исходная версия сохранена.
12. большой PDF (500+ страниц) и 100k страниц — бюджет применения.

## 12. Безопасность

- JWT только в `hello`; не в URL, не в логах.
- `Origin` для web; WSS; размер сообщения и число ops/s ограничены.
- Сервер не логирует `Contents`, `T`, `payload` аннотаций, байты PDF.
- Server-side проверка роли и PDF-permissions; не полагаться на UI.
- Зашифрованные и подписанные PDF — сессия запрещена в v1.

## 13. Открытые вопросы

1. Узкий API `engine` для `ObjRef` страницы/аннотации (`page_ref`,
   `annotation_ref`) — нужен для отказа от `epoch`.
2. Headless-применение без пересборки view: `Session::apply` делает
   incremental write + `RenderPool` на каждую правку; нужен `apply_headless`
   или батч на окно commit.
3. Транспорт web: `web-sys` WebSocket vs host-мост `postMessage`.
4. Судьба `/NM` при commit: сохранять клиентские UUID или переназначать.
5. Точная политика snapshot/compaction и TTL.
6. Ограничение размера документа для collab-сессии (500 страниц?).

## 14. План поставки

1. **PR-1:** `pdfcraft-collab-proto` — конверт, типы, валидация, тесты.
2. **PR-2:** узкий API движка для стабильных ID + тесты.
3. **PR-3:** `pdfcraft-collab-server` — комната, журнал, lease, presence,
   apply через `engine`; in-memory storage.
4. **PR-4:** клиентский слой: flag, `apply_remote`, resync, presence UI.
5. **PR-5:** commit в кабинет, 409, тесты 2–3 клиента, большой PDF.
6. **PR-6:** документация и feature flag по умолчанию off.

Каждый PR — отдельная ветка от `main`, без изменений CI; ADR и этот RFC не
переписываются в feature-PR.
